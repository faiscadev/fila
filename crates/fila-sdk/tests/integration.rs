use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use fila_sdk::{AckError, EnqueueError, FilaClient};
use tokio_stream::StreamExt;

/// Find a free TCP port by binding to port 0 and reading the assigned port.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

/// Resolve the path to a binary from the cargo target dir.
fn workspace_binary(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push(name);
    path
}

struct TestServer {
    child: Option<Child>,
    /// Address (host:port) for both CLI and SDK connections.
    addr: String,
    _data_dir: tempfile::TempDir,
}

impl TestServer {
    fn start() -> Self {
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let data_dir = tempfile::tempdir().expect("create temp dir");

        let config_path = data_dir.path().join("fila.toml");
        let config_content = format!(
            r#"[server]
listen_addr = "{addr}"

[telemetry]
otlp_endpoint = ""
"#
        );
        std::fs::write(&config_path, config_content).expect("write config");

        let binary = workspace_binary("fila-server");
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

        // Drain stdout/stderr so the server doesn't block on full pipe buffers.
        let stdout = child.stdout.take().expect("stdout");
        std::thread::spawn(move || for _ in BufReader::new(stdout).lines() {});
        let stderr = child.stderr.take().expect("stderr");
        std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});

        // Poll until the port is reachable.
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if std::net::TcpStream::connect(&addr).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            std::net::TcpStream::connect(&addr).is_ok(),
            "fila-server did not become reachable within 10s"
        );

        Self {
            child: Some(child),
            addr,
            _data_dir: data_dir,
        }
    }

    /// Address for CLI and SDK connections.
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

/// Create a queue via the CLI binary.
fn create_queue_cli(addr: &str, name: &str) {
    let cli = workspace_binary("fila");
    assert!(
        cli.exists(),
        "fila CLI binary not found at {cli:?}. Run `cargo build` first."
    );
    let output: Output = Command::new(&cli)
        .arg("--addr")
        .arg(addr)
        .args(["queue", "create", name])
        .output()
        .expect("run fila CLI");
    assert!(
        output.status.success(),
        "failed to create queue '{name}': {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn enqueue_consume_ack_lifecycle() {
    let server = TestServer::start();
    create_queue_cli(server.addr(), "test-lifecycle");

    let client = FilaClient::connect(server.addr()).await.unwrap();
    let queue = "test-lifecycle";

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
    create_queue_cli(server.addr(), "test-nack");

    let client = FilaClient::connect(server.addr()).await.unwrap();
    let queue = "test-nack";

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
async fn double_consume_does_not_hang() {
    let server = TestServer::start();
    // Create queue with short visibility timeout so in-flight messages from
    // the first consumer are redelivered quickly after unregistration.
    {
        let cli = workspace_binary("fila");
        let output: Output = Command::new(&cli)
            .arg("--addr")
            .arg(server.binary_addr())
            .args([
                "queue",
                "create",
                "double-consume",
                "--visibility-timeout",
                "1000",
            ])
            .output()
            .expect("run fila CLI");
        assert!(
            output.status.success(),
            "create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let client = FilaClient::connect(server.binary_addr()).await.unwrap();

    // Enqueue messages
    for i in 0..3 {
        client
            .enqueue(
                "double-consume",
                HashMap::new(),
                format!("msg-{i}").into_bytes(),
            )
            .await
            .unwrap();
    }

    // First consume — drain one message then drop
    let mut stream1 = client.consume("double-consume").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), stream1.next())
        .await
        .expect("timeout on first consume")
        .unwrap()
        .unwrap();
    client.ack("double-consume", &msg.id).await.unwrap();
    drop(stream1);

    // Brief pause for CancelConsume to propagate
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Second consume must not hang
    let stream2 = tokio::time::timeout(Duration::from_secs(3), client.consume("double-consume"))
        .await
        .expect("DEADLOCK: second consume timed out")
        .expect("second consume failed");

    let mut stream2 = stream2;
    let msg2 = tokio::time::timeout(Duration::from_secs(2), stream2.next())
        .await
        .expect("timeout waiting for message on second consume")
        .unwrap()
        .unwrap();

    client.ack("double-consume", &msg2.id).await.unwrap();
}

#[tokio::test]
async fn client_drop_sends_disconnect_and_cleans_up() {
    let server = TestServer::start();
    create_queue_cli(server.binary_addr(), "disconnect-queue");

    // Connect and start consuming so the server registers an active consumer.
    let client = FilaClient::connect(server.binary_addr()).await.unwrap();
    let _stream = client.consume("disconnect-queue").await.unwrap();

    // Use CLI to verify the server sees an active consumer.
    let consumers = parse_active_consumers(&cli_inspect(server.binary_addr(), "disconnect-queue"));
    assert_eq!(consumers, 1, "should have 1 consumer before drop");

    // Drop the consuming client — this should send a Disconnect frame
    // and the server should unregister the consumer.
    drop(_stream);
    drop(client);

    // Poll until the server processes the disconnect (with timeout).
    let start = std::time::Instant::now();
    loop {
        let consumers =
            parse_active_consumers(&cli_inspect(server.binary_addr(), "disconnect-queue"));
        if consumers == 0 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "server still has {consumers} consumers after 5s"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Parse `Active consumers:` value from CLI inspect output.
fn parse_active_consumers(output: &str) -> u32 {
    output
        .lines()
        .find(|l| l.contains("Active consumers:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|s| s.trim().parse().ok())
        .expect("failed to parse Active consumers from CLI output")
}

/// Get queue inspect output via the CLI.
fn cli_inspect(addr: &str, queue: &str) -> String {
    let cli = workspace_binary("fila");
    let output: Output = Command::new(&cli)
        .arg("--addr")
        .arg(addr)
        .args(["queue", "inspect", queue])
        .output()
        .expect("run fila CLI inspect");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    format!("{stdout}{stderr}")
}
