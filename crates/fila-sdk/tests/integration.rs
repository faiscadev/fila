use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fila_proto::fila_admin_client::FilaAdminClient;
use fila_proto::{CreateQueueRequest, QueueConfig};
use fila_sdk::{AckError, BatchMode, ConnectOptions, EnqueueError, FilaClient};
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
async fn auto_batch_flush_on_batch_size() {
    let server = TestServer::start();
    let opts = ConnectOptions::new(server.addr()).with_batch_mode(BatchMode::Linger {
        linger_ms: 30000, // Very high linger — flush MUST happen by batch_size
        batch_size: 5,
    });
    let client = FilaClient::connect_with_options(opts).await.unwrap();

    let queue = "test-auto-batch-size";
    create_queue(server.addr(), queue).await;

    // Send all 5 messages concurrently so they buffer before any result resolves.
    let start = std::time::Instant::now();
    let mut handles = Vec::new();
    for i in 0..5u32 {
        let c = client.clone();
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            c.enqueue(&q, HashMap::new(), format!("msg-{i}").into_bytes())
                .await
                .unwrap()
        }));
    }
    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }
    let elapsed = start.elapsed();

    assert_eq!(ids.len(), 5);
    for id in &ids {
        assert!(
            !id.is_empty(),
            "each message should have a broker-assigned ID"
        );
    }
    // With 30s linger and batch_size=5, the batch must have been flushed by
    // batch_size, not linger. Should resolve well under 30s.
    assert!(
        elapsed < Duration::from_secs(10),
        "batch_size flush took too long ({elapsed:?}), likely waited for linger"
    );

    // Consume all 5 messages and verify uniqueness.
    let consumer = FilaClient::connect(server.addr()).await.unwrap();
    let mut stream = consumer.consume(queue).await.unwrap();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..5 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(ids.contains(&msg.id));
        assert!(
            seen.insert(msg.id.clone()),
            "duplicate message ID: {}",
            msg.id
        );
    }
}

#[tokio::test]
async fn auto_batch_flush_on_linger_timeout() {
    let server = TestServer::start();
    let opts = ConnectOptions::new(server.addr()).with_batch_mode(BatchMode::Linger {
        linger_ms: 100,   // 100ms linger
        batch_size: 1000, // High batch_size — flush should happen by timer
    });
    let client = FilaClient::connect_with_options(opts).await.unwrap();

    let queue = "test-auto-batch-linger";
    create_queue(server.addr(), queue).await;

    // Enqueue 1 message — won't hit batch_size, must wait for linger.
    let start = std::time::Instant::now();
    let id = client
        .enqueue(queue, HashMap::new(), b"linger-test".to_vec())
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert!(!id.is_empty());
    // The enqueue should have resolved after the linger timeout.
    assert!(
        elapsed >= Duration::from_millis(50),
        "expected linger delay, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "linger took too long: {elapsed:?}"
    );
}

#[tokio::test]
async fn batch_disabled_uses_single_message_rpc() {
    let server = TestServer::start();
    let opts = ConnectOptions::new(server.addr()).with_batch_mode(BatchMode::Disabled);
    let client = FilaClient::connect_with_options(opts).await.unwrap();

    let queue = "test-no-batch";
    create_queue(server.addr(), queue).await;

    // Enqueue should return immediately (no batching, no delay).
    let start = std::time::Instant::now();
    let id = client
        .enqueue(queue, HashMap::new(), b"direct".to_vec())
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert!(!id.is_empty());
    assert!(
        elapsed < Duration::from_millis(500),
        "disabled-batch enqueue took too long: {elapsed:?}"
    );
}

#[tokio::test]
async fn nagle_auto_batch_sends_immediately_when_idle() {
    let server = TestServer::start();
    // Default connect() uses BatchMode::Auto (Nagle-style).
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let queue = "test-nagle-idle";
    create_queue(server.addr(), queue).await;

    // Single message with no contention — should send immediately, no batching delay.
    let start = std::time::Instant::now();
    let id = client
        .enqueue(queue, HashMap::new(), b"immediate".to_vec())
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert!(!id.is_empty());
    assert!(
        elapsed < Duration::from_millis(500),
        "Nagle auto-batch should send immediately when idle: {elapsed:?}"
    );
}

#[tokio::test]
async fn nagle_auto_batch_buffers_under_load() {
    let server = TestServer::start();
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let queue = "test-nagle-load";
    create_queue(server.addr(), queue).await;

    // Fire 20 concurrent enqueues. The Nagle batcher should send the first
    // immediately, then batch the rest while the first RPC is in flight.
    let mut handles = Vec::new();
    for i in 0..20u32 {
        let c = client.clone();
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            c.enqueue(&q, HashMap::new(), format!("msg-{i}").into_bytes())
                .await
                .unwrap()
        }));
    }
    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }

    assert_eq!(ids.len(), 20);
    // Verify all messages are unique and stored.
    let mut seen = std::collections::HashSet::new();
    for id in &ids {
        assert!(!id.is_empty());
        assert!(seen.insert(id.clone()), "duplicate ID: {id}");
    }
}

#[tokio::test]
async fn auto_batch_partial_failure_propagation() {
    let server = TestServer::start();
    let opts = ConnectOptions::new(server.addr()).with_batch_mode(BatchMode::Linger {
        linger_ms: 30000,
        batch_size: 2,
    });
    let client = FilaClient::connect_with_options(opts).await.unwrap();

    let valid_queue = "test-partial-failure";
    create_queue(server.addr(), valid_queue).await;

    // Send 2 messages concurrently: one to a valid queue, one to a non-existent queue.
    // They'll be batched together (batch_size=2), and the server should return
    // individual success/error results.
    let c1 = client.clone();
    let c2 = client.clone();
    let h_valid = tokio::spawn(async move {
        c1.enqueue(valid_queue, HashMap::new(), b"good".to_vec())
            .await
    });
    let h_invalid = tokio::spawn(async move {
        c2.enqueue("no-such-queue", HashMap::new(), b"bad".to_vec())
            .await
    });

    let result_valid = h_valid.await.unwrap();
    let result_invalid = h_invalid.await.unwrap();

    // The valid message should succeed.
    assert!(
        result_valid.is_ok(),
        "valid queue should succeed: {result_valid:?}"
    );

    // The invalid message should fail (queue not found → per-message error).
    assert!(
        result_invalid.is_err(),
        "non-existent queue should fail: {result_invalid:?}"
    );
}

#[tokio::test]
async fn explicit_batch_enqueue_works_with_auto_batching() {
    use fila_sdk::{BatchEnqueueResult, EnqueueMessage};

    let server = TestServer::start();
    // Explicit batch_enqueue() should work alongside any batch mode.
    let opts = ConnectOptions::new(server.addr());
    let client = FilaClient::connect_with_options(opts).await.unwrap();

    let queue = "test-explicit-with-auto";
    create_queue(server.addr(), queue).await;

    // Explicit batch_enqueue should still work even with auto-batching enabled.
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

    let results = client.batch_enqueue(messages).await.unwrap();
    assert_eq!(results.len(), 2);
    for result in &results {
        assert!(
            matches!(result, BatchEnqueueResult::Success(_)),
            "expected Success, got: {result:?}"
        );
    }
}

// --- Streaming enqueue tests ---

#[tokio::test]
async fn streaming_enqueue_basic_flow() {
    let server = TestServer::start();
    // Default BatchMode::Auto uses streaming when available.
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let queue = "test-streaming-basic";
    create_queue(server.addr(), queue).await;

    // Enqueue multiple messages — they should go through the streaming path.
    let mut ids = Vec::new();
    for i in 0..10u32 {
        let id = client
            .enqueue(
                queue,
                HashMap::new(),
                format!("stream-msg-{i}").into_bytes(),
            )
            .await
            .unwrap();
        assert!(!id.is_empty());
        ids.push(id);
    }

    // Verify all messages were stored by consuming them.
    let consumer = FilaClient::connect(server.addr()).await.unwrap();
    let mut stream = consumer.consume(queue).await.unwrap();
    let mut consumed = std::collections::HashSet::new();
    for _ in 0..10 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(ids.contains(&msg.id));
        consumed.insert(msg.id);
    }
    assert_eq!(consumed.len(), 10, "all 10 messages should be unique");
}

#[tokio::test]
async fn streaming_concurrent_enqueue_from_multiple_tasks() {
    let server = TestServer::start();
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let queue = "test-streaming-concurrent";
    create_queue(server.addr(), queue).await;

    // Spawn 50 concurrent enqueue tasks — all use the same streaming connection.
    let mut handles = Vec::new();
    for i in 0..50u32 {
        let c = client.clone();
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            c.enqueue(&q, HashMap::new(), format!("concurrent-{i}").into_bytes())
                .await
                .unwrap()
        }));
    }

    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }

    assert_eq!(ids.len(), 50);
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 50, "all 50 message IDs should be unique");
}

#[tokio::test]
async fn streaming_enqueue_per_message_error() {
    let server = TestServer::start();
    let client = FilaClient::connect(server.addr()).await.unwrap();

    let valid_queue = "test-streaming-error";
    create_queue(server.addr(), valid_queue).await;

    // Enqueue to a valid queue — should succeed.
    let id = client
        .enqueue(valid_queue, HashMap::new(), b"good".to_vec())
        .await
        .unwrap();
    assert!(!id.is_empty());

    // Enqueue to a non-existent queue — should return QueueNotFound.
    let err = client
        .enqueue("nonexistent-queue", HashMap::new(), b"bad".to_vec())
        .await
        .unwrap_err();
    assert!(
        matches!(err, EnqueueError::QueueNotFound(_)),
        "expected QueueNotFound, got: {err:?}"
    );

    // Enqueue to the valid queue again — stream should still work after a per-message error.
    let id2 = client
        .enqueue(valid_queue, HashMap::new(), b"still-good".to_vec())
        .await
        .unwrap();
    assert!(!id2.is_empty());
}

#[tokio::test]
async fn streaming_linger_mode_uses_stream() {
    let server = TestServer::start();
    let opts = ConnectOptions::new(server.addr()).with_batch_mode(BatchMode::Linger {
        linger_ms: 100,
        batch_size: 10,
    });
    let client = FilaClient::connect_with_options(opts).await.unwrap();

    let queue = "test-streaming-linger";
    create_queue(server.addr(), queue).await;

    // Enqueue messages via linger batcher — should use streaming.
    let mut handles = Vec::new();
    for i in 0..10u32 {
        let c = client.clone();
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            c.enqueue(&q, HashMap::new(), format!("linger-{i}").into_bytes())
                .await
                .unwrap()
        }));
    }

    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }

    assert_eq!(ids.len(), 10);
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 10, "all message IDs should be unique");
}
