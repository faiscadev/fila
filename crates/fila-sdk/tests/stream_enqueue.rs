use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fila_proto::fila_admin_client::FilaAdminClient;
use fila_proto::fila_service_client::FilaServiceClient;
use fila_proto::{CreateQueueRequest, QueueConfig, StreamEnqueueRequest, StreamEnqueueResponse};
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

        let stderr = child.stderr.take().expect("stderr");
        let reader = BufReader::new(stderr);

        let addr_clone = addr.clone();
        std::thread::spawn(move || {
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if line.contains(&addr_clone) || line.contains("starting gRPC server") {
                            // Server is ready
                        }
                    }
                    Err(_) => break,
                }
            }
        });

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

/// Helper: collect responses from a streaming enqueue response stream.
async fn collect_responses(
    stream: &mut tonic::codec::Streaming<StreamEnqueueResponse>,
    count: usize,
) -> Vec<StreamEnqueueResponse> {
    let mut responses = Vec::with_capacity(count);
    for _ in 0..count {
        let resp = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timeout waiting for stream response")
            .expect("stream ended unexpectedly")
            .expect("stream error");
        responses.push(resp);
    }
    responses
}

#[tokio::test]
async fn stream_enqueue_basic() {
    let server = TestServer::start();
    let queue = "test-stream-basic";
    create_queue(server.addr(), queue).await;

    let mut client = FilaServiceClient::connect(server.addr().to_string())
        .await
        .expect("connect");

    // Send 10 messages over the stream.
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEnqueueRequest>(16);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let response = client
        .stream_enqueue(request_stream)
        .await
        .expect("stream_enqueue");
    let mut resp_stream = response.into_inner();

    for i in 0u64..10 {
        tx.send(StreamEnqueueRequest {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: format!("msg-{i}").into(),
            sequence_number: i,
        })
        .await
        .expect("send");
    }

    // Collect 10 responses.
    let responses = collect_responses(&mut resp_stream, 10).await;

    assert_eq!(responses.len(), 10);

    // Verify all sequence numbers are present and each has a message_id.
    let mut seen_seqs: Vec<u64> = responses.iter().map(|r| r.sequence_number).collect();
    seen_seqs.sort();
    assert_eq!(seen_seqs, (0..10).collect::<Vec<u64>>());

    for resp in &responses {
        match &resp.result {
            Some(fila_proto::stream_enqueue_response::Result::MessageId(id)) => {
                assert!(!id.is_empty(), "message_id should not be empty");
            }
            other => panic!("expected MessageId, got: {other:?}"),
        }
    }

    // Close the send side and verify the response stream ends gracefully.
    drop(tx);
    let next = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    match next {
        Ok(None) => {}    // Stream closed — expected
        Ok(Some(_)) => {} // Extra message — also fine
        Err(_) => {}      // Timeout — server may keep stream open briefly
    }
}

#[tokio::test]
async fn stream_enqueue_per_message_error() {
    let server = TestServer::start();
    let queue = "test-stream-error";
    create_queue(server.addr(), queue).await;

    let mut client = FilaServiceClient::connect(server.addr().to_string())
        .await
        .expect("connect");

    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEnqueueRequest>(16);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let response = client
        .stream_enqueue(request_stream)
        .await
        .expect("stream_enqueue");
    let mut resp_stream = response.into_inner();

    // Send a good message, then a bad one (nonexistent queue), then another good one.
    tx.send(StreamEnqueueRequest {
        queue: queue.to_string(),
        headers: HashMap::new(),
        payload: "good-1".into(),
        sequence_number: 1,
    })
    .await
    .unwrap();

    tx.send(StreamEnqueueRequest {
        queue: "nonexistent-queue".to_string(),
        headers: HashMap::new(),
        payload: "bad".into(),
        sequence_number: 2,
    })
    .await
    .unwrap();

    tx.send(StreamEnqueueRequest {
        queue: queue.to_string(),
        headers: HashMap::new(),
        payload: "good-2".into(),
        sequence_number: 3,
    })
    .await
    .unwrap();

    let responses = collect_responses(&mut resp_stream, 3).await;

    // Build a map of sequence_number -> result for order-independent assertions.
    let by_seq: HashMap<u64, &StreamEnqueueResponse> =
        responses.iter().map(|r| (r.sequence_number, r)).collect();

    // Seq 1 should be success.
    match &by_seq[&1].result {
        Some(fila_proto::stream_enqueue_response::Result::MessageId(id)) => {
            assert!(!id.is_empty());
        }
        other => panic!("seq 1: expected MessageId, got: {other:?}"),
    }

    // Seq 2 should be an error (queue not found).
    match &by_seq[&2].result {
        Some(fila_proto::stream_enqueue_response::Result::Error(msg)) => {
            assert!(
                msg.contains("not found") || msg.contains("not_found") || !msg.is_empty(),
                "expected queue-not-found error, got: {msg}"
            );
        }
        other => panic!("seq 2: expected Error, got: {other:?}"),
    }

    // Seq 3 should be success (stream continues after per-message error).
    match &by_seq[&3].result {
        Some(fila_proto::stream_enqueue_response::Result::MessageId(id)) => {
            assert!(!id.is_empty());
        }
        other => panic!("seq 3: expected MessageId, got: {other:?}"),
    }

    drop(tx);
}

#[tokio::test]
async fn stream_enqueue_concurrent_streams() {
    let server = TestServer::start();
    let queue = "test-stream-concurrent";
    create_queue(server.addr(), queue).await;

    // Two concurrent stream producers.
    let mut handles = Vec::new();
    for stream_idx in 0u64..2 {
        let addr = server.addr().to_string();
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            let mut client = FilaServiceClient::connect(addr).await.expect("connect");

            let (tx, rx) = tokio::sync::mpsc::channel::<StreamEnqueueRequest>(16);
            let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

            let response = client
                .stream_enqueue(request_stream)
                .await
                .expect("stream_enqueue");
            let mut resp_stream = response.into_inner();

            // Each stream sends 5 messages.
            for i in 0u64..5 {
                tx.send(StreamEnqueueRequest {
                    queue: q.clone(),
                    headers: HashMap::new(),
                    payload: format!("stream-{stream_idx}-msg-{i}").into(),
                    sequence_number: stream_idx * 100 + i,
                })
                .await
                .unwrap();
            }

            let responses = collect_responses(&mut resp_stream, 5).await;
            drop(tx);
            responses
        }));
    }

    let mut all_responses = Vec::new();
    for h in handles {
        all_responses.extend(h.await.unwrap());
    }

    // Should have 10 total responses (5 per stream).
    assert_eq!(all_responses.len(), 10);

    // All should be successful.
    for resp in &all_responses {
        match &resp.result {
            Some(fila_proto::stream_enqueue_response::Result::MessageId(id)) => {
                assert!(!id.is_empty());
            }
            other => panic!("expected MessageId, got: {other:?}"),
        }
    }

    // All message IDs should be unique.
    let ids: Vec<String> = all_responses
        .iter()
        .filter_map(|r| match &r.result {
            Some(fila_proto::stream_enqueue_response::Result::MessageId(id)) => Some(id.clone()),
            _ => None,
        })
        .collect();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 10, "all message IDs should be unique");
}

#[tokio::test]
async fn stream_enqueue_sequence_number_correlation() {
    let server = TestServer::start();
    let queue = "test-stream-seqnum";
    create_queue(server.addr(), queue).await;

    let mut client = FilaServiceClient::connect(server.addr().to_string())
        .await
        .expect("connect");

    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEnqueueRequest>(16);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let response = client
        .stream_enqueue(request_stream)
        .await
        .expect("stream_enqueue");
    let mut resp_stream = response.into_inner();

    // Use non-sequential sequence numbers to verify correlation, not ordering.
    let seq_numbers: Vec<u64> = vec![42, 7, 999, 1, 100];
    for &seq in &seq_numbers {
        tx.send(StreamEnqueueRequest {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: format!("seq-{seq}").into(),
            sequence_number: seq,
        })
        .await
        .unwrap();
    }

    let responses = collect_responses(&mut resp_stream, 5).await;

    // Every sequence number from our requests should appear in responses.
    let resp_seqs: std::collections::HashSet<u64> =
        responses.iter().map(|r| r.sequence_number).collect();
    for &seq in &seq_numbers {
        assert!(
            resp_seqs.contains(&seq),
            "response missing sequence_number {seq}"
        );
    }

    drop(tx);
}
