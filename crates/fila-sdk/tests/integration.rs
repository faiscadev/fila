use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fila_sdk::{AckError, ConnectOptions, EnqueueError, FibpTransport, FilaClient};
use tokio_stream::StreamExt;

/// Resolve the path to the fila-server binary from the cargo target dir.
fn server_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push("fila-server");
    path
}

struct TestServer {
    child: Option<Child>,
    addr: String,
    _data_dir: tempfile::TempDir,
}

impl TestServer {
    fn start() -> Self {
        let data_dir = tempfile::tempdir().expect("create temp dir");
        let port_file = data_dir.path().join("port");

        // Write config using FIBP listen_addr with port 0 for OS-assigned port.
        let config_path = data_dir.path().join("fila.toml");
        let config_content =
            "[fibp]\nlisten_addr = \"127.0.0.1:0\"\n\n[telemetry]\notlp_endpoint = \"\"\n";
        std::fs::write(&config_path, config_content).expect("write config");

        let binary = server_binary();
        assert!(
            binary.exists(),
            "fila-server binary not found at {binary:?}. Run `cargo build` first."
        );

        let mut child = Command::new(&binary)
            .env(
                "FILA_DATA_DIR",
                data_dir.path().join("data").to_str().unwrap(),
            )
            .env("FILA_PORT_FILE", port_file.to_str().unwrap())
            .current_dir(data_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fila-server");

        // Drain stdout/stderr so the process doesn't block on full pipes.
        let stdout = child.stdout.take().expect("stdout");
        std::thread::spawn(move || for _ in BufReader::new(stdout).lines() {});
        let stderr = child.stderr.take().expect("stderr");
        std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});

        // Wait for the port file to appear (server writes it after binding).
        let start = std::time::Instant::now();
        let addr = loop {
            if start.elapsed() > Duration::from_secs(10) {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        panic!("fila-server exited with {status} before writing port file")
                    }
                    _ => panic!("fila-server did not write port file within 10s"),
                }
            }
            if let Ok(contents) = std::fs::read_to_string(&port_file) {
                let contents = contents.trim();
                if !contents.is_empty() {
                    break contents.to_string();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        Self {
            child: Some(child),
            addr,
            _data_dir: data_dir,
        }
    }

    fn addr(&self) -> &str {
        &self.addr
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Helper to create a queue via the FIBP transport.
async fn create_queue(addr: &str, name: &str) {
    let transport = FibpTransport::connect(addr, None)
        .await
        .expect("connect for create_queue");
    transport
        .create_queue(
            name,
            Some(fila_sdk::proto::QueueConfig {
                on_enqueue_script: String::new(),
                on_failure_script: String::new(),
                visibility_timeout_ms: 30_000,
            }),
        )
        .await
        .expect("create queue");
}

#[tokio::test]
async fn enqueue_consume_ack_lifecycle() {
    let server = TestServer::start();
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let queue = "test-lifecycle";
    create_queue(server.addr(), queue).await;

    // Enqueue a message
    let mut headers = HashMap::new();
    headers.insert("key".to_string(), "value".to_string());
    let msg_id = client
        .enqueue(queue, headers.clone(), b"hello".to_vec())
        .await
        .unwrap();

    assert!(!msg_id.is_empty(), "message ID should not be empty");

    // Consume the message
    let mut stream = client.consume(queue).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for message")
        .expect("stream ended unexpectedly")
        .expect("consume error");

    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.headers.get("key").map(|s| s.as_str()), Some("value"));
    assert_eq!(msg.payload, b"hello");

    // Ack the message
    client.ack(queue, &msg_id).await.unwrap();

    // Ack again should return not found
    let err = client.ack(queue, &msg_id).await.unwrap_err();
    assert!(
        matches!(err, AckError::MessageNotFound(_)),
        "expected MessageNotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn enqueue_consume_nack_release() {
    let server = TestServer::start();
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let queue = "test-nack";
    create_queue(server.addr(), queue).await;

    // Enqueue
    let msg_id = client
        .enqueue(queue, HashMap::new(), b"retry-me".to_vec())
        .await
        .unwrap();

    // Consume
    let mut stream = client.consume(queue).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.attempt_count, 0);

    // Nack -- message should be requeued and delivered again on the same stream
    client
        .nack(queue, &msg_id, "transient failure")
        .await
        .unwrap();

    let msg2 = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for redelivery")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(msg2.id, msg_id);
    assert_eq!(msg2.attempt_count, 1, "attempt count should be incremented");
}

#[tokio::test]
async fn enqueue_to_nonexistent_queue() {
    let server = TestServer::start();
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let err = client
        .enqueue("no-such-queue", HashMap::new(), b"data".to_vec())
        .await
        .unwrap_err();

    assert!(
        matches!(err, EnqueueError::QueueNotFound(_)),
        "expected QueueNotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn enqueue_many_works() {
    use fila_sdk::EnqueueMessage;

    let server = TestServer::start();
    let opts = ConnectOptions::new(server.addr());
    let client = FilaClient::connect_with_options(opts).await.unwrap();

    let queue = "test-enqueue-many";
    create_queue(server.addr(), queue).await;

    let messages = vec![
        EnqueueMessage {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: b"explicit-1".to_vec(),
        },
        EnqueueMessage {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: b"explicit-2".to_vec(),
        },
    ];

    let results = client.enqueue_many(messages).await;
    assert_eq!(results.len(), 2);
    for result in &results {
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }
}
