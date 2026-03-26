use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fila_sdk::{AckError, ConnectOptions, EnqueueError, FilaClient};
use tokio_stream::StreamExt;

/// Find a free TCP port by binding to port 0 and reading the assigned port.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

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

/// Test server with both gRPC and FIBP enabled.
struct FibpTestServer {
    child: Option<Child>,
    grpc_addr: String,
    fibp_addr: String,
    _data_dir: tempfile::TempDir,
}

impl FibpTestServer {
    fn start() -> Self {
        let grpc_port = free_port();
        let grpc_addr = format!("127.0.0.1:{grpc_port}");
        let data_dir = tempfile::tempdir().expect("create temp dir");

        let fibp_port_file = data_dir.path().join("fibp_port");

        // Write config with both gRPC and FIBP enabled
        let config_path = data_dir.path().join("fila.toml");
        let config_content = format!(
            r#"[server]
listen_addr = "{grpc_addr}"

[fibp]
listen_addr = "127.0.0.1:0"

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
            .env("FILA_FIBP_PORT_FILE", fibp_port_file.to_str().unwrap())
            .current_dir(data_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fila-server");

        // Drain stderr so the process doesn't block
        let stderr = child.stderr.take().expect("stderr");
        let grpc_addr_clone = grpc_addr.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if line.contains(&grpc_addr_clone) || line.contains("starting gRPC") {
                            // Server is ready
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Poll until gRPC is reachable
        let start = std::time::Instant::now();
        let mut connected = false;
        while start.elapsed() < Duration::from_secs(10) {
            if std::net::TcpStream::connect(&grpc_addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            connected,
            "fila-server gRPC did not become reachable at {grpc_addr} within 10s"
        );

        // Read the FIBP port from the port file
        let actual_fibp_addr = {
            let start = std::time::Instant::now();
            let mut addr = String::new();
            while start.elapsed() < Duration::from_secs(5) {
                if let Ok(contents) = std::fs::read_to_string(&fibp_port_file) {
                    if !contents.trim().is_empty() {
                        addr = contents.trim().to_string();
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(!addr.is_empty(), "FIBP port file was not written within 5s");
            addr
        };

        // Poll until FIBP is reachable
        let start = std::time::Instant::now();
        let mut fibp_connected = false;
        while start.elapsed() < Duration::from_secs(5) {
            if std::net::TcpStream::connect(&actual_fibp_addr).is_ok() {
                fibp_connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            fibp_connected,
            "fila-server FIBP did not become reachable at {actual_fibp_addr} within 5s"
        );

        Self {
            child: Some(child),
            grpc_addr: format!("http://{grpc_addr}"),
            fibp_addr: actual_fibp_addr,
            _data_dir: data_dir,
        }
    }

    fn grpc_addr(&self) -> &str {
        &self.grpc_addr
    }

    fn fibp_addr(&self) -> &str {
        &self.fibp_addr
    }
}

impl Drop for FibpTestServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Create a queue via gRPC admin client.
async fn create_queue(grpc_addr: &str, name: &str) {
    use fila_proto::fila_admin_client::FilaAdminClient;
    use fila_proto::{CreateQueueRequest, QueueConfig};

    let mut admin = FilaAdminClient::connect(grpc_addr.to_string())
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

/// Connect to the FIBP transport.
async fn fibp_client(server: &FibpTestServer) -> FilaClient {
    let opts = ConnectOptions::new(server.fibp_addr()).with_fibp();
    FilaClient::connect_with_options(opts).await.unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fibp_enqueue_success() {
    let server = FibpTestServer::start();
    let queue = "test-fibp-enqueue";
    create_queue(server.grpc_addr(), queue).await;

    let client = fibp_client(&server).await;

    let mut headers = HashMap::new();
    headers.insert("key".to_string(), "value".to_string());
    let msg_id = client
        .enqueue(queue, headers, b"hello-fibp".to_vec())
        .await
        .unwrap();

    assert!(!msg_id.is_empty(), "message ID should not be empty");
}

#[tokio::test]
async fn fibp_enqueue_consume_ack_lifecycle() {
    let server = FibpTestServer::start();
    let queue = "test-fibp-lifecycle";
    create_queue(server.grpc_addr(), queue).await;

    let client = fibp_client(&server).await;

    // Enqueue
    let mut headers = HashMap::new();
    headers.insert("key".to_string(), "value".to_string());
    let msg_id = client
        .enqueue(queue, headers.clone(), b"lifecycle-msg".to_vec())
        .await
        .unwrap();

    assert!(!msg_id.is_empty());

    // Consume
    let mut stream = client.consume(queue).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for message")
        .expect("stream ended unexpectedly")
        .expect("consume error");

    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.headers.get("key").map(|s| s.as_str()), Some("value"));
    assert_eq!(msg.payload, b"lifecycle-msg");

    // Ack
    client.ack(queue, &msg_id).await.unwrap();

    // Ack again should return not found
    let err = client.ack(queue, &msg_id).await.unwrap_err();
    assert!(
        matches!(err, AckError::MessageNotFound(_)),
        "expected MessageNotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn fibp_batch_enqueue() {
    use fila_sdk::EnqueueMessage;

    let server = FibpTestServer::start();
    let queue = "test-fibp-batch";
    create_queue(server.grpc_addr(), queue).await;

    let client = fibp_client(&server).await;

    let messages = vec![
        EnqueueMessage {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: b"batch-1".to_vec(),
        },
        EnqueueMessage {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: b"batch-2".to_vec(),
        },
        EnqueueMessage {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: b"batch-3".to_vec(),
        },
    ];

    let results = client.enqueue_many(messages).await;
    assert_eq!(results.len(), 3);
    for (i, result) in results.iter().enumerate() {
        assert!(result.is_ok(), "message {i} should succeed: {result:?}");
    }

    // Verify all 3 messages are consumable.
    let consumer = fibp_client(&server).await;
    let mut stream = consumer.consume(queue).await.unwrap();
    let mut consumed = std::collections::HashSet::new();
    for _ in 0..3 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        consumed.insert(msg.id);
    }
    assert_eq!(consumed.len(), 3, "all 3 messages should be unique");
}

#[tokio::test]
async fn fibp_enqueue_to_nonexistent_queue() {
    let server = FibpTestServer::start();
    let client = fibp_client(&server).await;

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
async fn fibp_admin_create_and_list_queues() {
    let server = FibpTestServer::start();

    // Use the FIBP transport to create and list queues.
    let fibp = fila_sdk::FibpTransport::connect(server.fibp_addr(), None)
        .await
        .unwrap();

    // Create a queue via FIBP admin operation.
    let queue_id = fibp
        .create_queue(
            "fibp-admin-test",
            Some(fila_proto::QueueConfig {
                on_enqueue_script: String::new(),
                on_failure_script: String::new(),
                visibility_timeout_ms: 30_000,
            }),
        )
        .await
        .unwrap();
    assert!(!queue_id.is_empty());

    // List queues via FIBP.
    let resp = fibp.list_queues().await.unwrap();
    let names: Vec<_> = resp.queues.iter().map(|q| q.name.as_str()).collect();
    assert!(
        names.contains(&"fibp-admin-test"),
        "queue list should contain 'fibp-admin-test': {names:?}"
    );

    // Queue stats via FIBP.
    let stats = fibp.queue_stats("fibp-admin-test").await.unwrap();
    assert_eq!(stats.depth, 0);
    assert_eq!(stats.in_flight, 0);
}

#[tokio::test]
async fn fibp_nack_redelivery() {
    let server = FibpTestServer::start();
    let queue = "test-fibp-nack";
    create_queue(server.grpc_addr(), queue).await;

    let client = fibp_client(&server).await;

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

    // Nack — message should be requeued
    client
        .nack(queue, &msg_id, "transient failure")
        .await
        .unwrap();

    // Should be redelivered
    let msg2 = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for redelivery")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(msg2.id, msg_id);
    assert_eq!(msg2.attempt_count, 1, "attempt count should be incremented");
}
