use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fila_proto::fila_admin_client::FilaAdminClient;
use fila_proto::{CreateQueueRequest, QueueConfig};
use fila_sdk::{ClientError, FilaClient};
use tokio_stream::StreamExt;

/// Find a free TCP port by binding to port 0 and reading the assigned port.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

/// Resolve the path to the fila-server binary from the cargo target dir.
fn server_binary() -> PathBuf {
    // Integration tests run from the crate root. The binary is in the workspace
    // target directory under debug/.
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
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let data_dir = tempfile::tempdir().expect("create temp dir");

        // Write a minimal config with our random port
        let config_path = data_dir.path().join("fila.toml");
        let config_content = format!(
            r#"[server]
listen_addr = "{addr}"

[telemetry]
otlp_endpoint = ""
"#
        );
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
            .current_dir(data_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fila-server");

        // Wait for the server to be ready by watching stderr for the listen log line
        let stderr = child.stderr.take().expect("stderr");
        let reader = BufReader::new(stderr);

        let addr_clone = addr.clone();
        std::thread::spawn(move || {
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        // Drain stderr so the process doesn't block
                        if line.contains(&addr_clone) || line.contains("starting gRPC server") {
                            // Server is ready
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Poll until we can connect
        let start = std::time::Instant::now();
        let mut connected = false;
        while start.elapsed() < Duration::from_secs(10) {
            if std::net::TcpStream::connect(&addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            connected,
            "fila-server did not become reachable at {addr} within 10s"
        );

        Self {
            child: Some(child),
            addr: format!("http://{addr}"),
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

/// Helper to create a queue via the admin client.
async fn create_queue(addr: &str, name: &str) {
    let mut admin = FilaAdminClient::connect(addr.to_string())
        .await
        .expect("connect admin");
    admin
        .create_queue(CreateQueueRequest {
            name: name.to_string(),
            config: Some(QueueConfig {
                on_enqueue_script: String::new(),
                on_failure_script: String::new(),
                visibility_timeout_ms: 30_000,
            }),
        })
        .await
        .expect("create queue");
}

#[tokio::test]
async fn enqueue_lease_ack_lifecycle() {
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

    // Lease and receive the message
    let mut stream = client.lease(queue).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for message")
        .expect("stream ended unexpectedly")
        .expect("lease error");

    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.headers.get("key").map(|s| s.as_str()), Some("value"));
    assert_eq!(msg.payload, b"hello");

    // Ack the message
    client.ack(queue, &msg_id).await.unwrap();

    // Ack again should return not found
    let err = client.ack(queue, &msg_id).await.unwrap_err();
    assert!(
        matches!(err, ClientError::MessageNotFound(_)),
        "expected MessageNotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn enqueue_lease_nack_release() {
    let server = TestServer::start();
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let queue = "test-nack";
    create_queue(server.addr(), queue).await;

    // Enqueue
    let msg_id = client
        .enqueue(queue, HashMap::new(), b"retry-me".to_vec())
        .await
        .unwrap();

    // Lease
    let mut stream = client.lease(queue).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.attempt_count, 0);

    // Nack — message should be requeued and delivered again on the same stream
    client
        .nack(queue, &msg_id, "transient failure")
        .await
        .unwrap();

    // The scheduler requeues the nacked message and delivers it to the still-open
    // consumer stream.
    let msg2 = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for redelivery")
        .expect("stream ended")
        .expect("lease error");

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
        matches!(err, ClientError::QueueNotFound(_)),
        "expected QueueNotFound, got: {err:?}"
    );
}
