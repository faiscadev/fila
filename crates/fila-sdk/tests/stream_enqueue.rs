use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use fila_proto::fila_admin_client::FilaAdminClient;
use fila_proto::fila_service_client::FilaServiceClient;
use fila_proto::{
    CreateQueueRequest, EnqueueMessage, QueueConfig, StreamEnqueueRequest, StreamEnqueueResponse,
};
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

        // Drain stdout so the child process doesn't block on a full pipe buffer.
        let stdout = child.stdout.take().expect("stdout");
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if line.is_err() {
                    break;
                }
            }
        });

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

/// Build a StreamEnqueueRequest with a single message.
fn stream_req(queue: &str, payload: impl Into<bytes::Bytes>, seq: u64) -> StreamEnqueueRequest {
    StreamEnqueueRequest {
        messages: vec![EnqueueMessage {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: payload.into(),
        }],
        sequence_number: seq,
    }
}

/// Extract the message_id from the first result in a stream response.
fn first_message_id(resp: &StreamEnqueueResponse) -> Option<&str> {
    resp.results.first().and_then(|r| match &r.result {
        Some(fila_proto::enqueue_result::Result::MessageId(id)) => Some(id.as_str()),
        _ => None,
    })
}

/// Check if the first result in a stream response is an error.
fn first_error_message(resp: &StreamEnqueueResponse) -> Option<String> {
    resp.results.first().and_then(|r| match &r.result {
        Some(fila_proto::enqueue_result::Result::Error(err)) => Some(err.message.clone()),
        _ => None,
    })
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

    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEnqueueRequest>(16);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let response = client
        .stream_enqueue(request_stream)
        .await
        .expect("stream_enqueue");
    let mut resp_stream = response.into_inner();

    for i in 0u64..10 {
        tx.send(stream_req(queue, format!("msg-{i}"), i))
            .await
            .expect("send");
    }

    let responses = collect_responses(&mut resp_stream, 10).await;

    assert_eq!(responses.len(), 10);

    let mut seen_seqs: Vec<u64> = responses.iter().map(|r| r.sequence_number).collect();
    seen_seqs.sort();
    assert_eq!(seen_seqs, (0..10).collect::<Vec<u64>>());

    for resp in &responses {
        let id = first_message_id(resp);
        assert!(id.is_some(), "expected message_id, got: {:?}", resp.results);
        assert!(!id.unwrap().is_empty(), "message_id should not be empty");
    }

    drop(tx);
    let next = tokio::time::timeout(Duration::from_secs(5), resp_stream.next()).await;
    match next {
        Ok(None) => {}
        Ok(Some(Ok(_))) => {
            panic!("unexpected extra response after closing send side");
        }
        Ok(Some(Err(_))) => {}
        Err(_) => {
            panic!("server did not close response stream within 5s after client disconnect");
        }
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

    tx.send(stream_req(queue, "good-1", 1)).await.unwrap();
    tx.send(stream_req("nonexistent-queue", "bad", 2))
        .await
        .unwrap();
    tx.send(stream_req(queue, "good-2", 3)).await.unwrap();

    let responses = collect_responses(&mut resp_stream, 3).await;

    let by_seq: HashMap<u64, &StreamEnqueueResponse> =
        responses.iter().map(|r| (r.sequence_number, r)).collect();

    // Seq 1 should be success.
    assert!(
        first_message_id(by_seq[&1]).is_some(),
        "seq 1: expected MessageId, got: {:?}",
        by_seq[&1].results
    );

    // Seq 2 should be an error (queue not found).
    let err_msg = first_error_message(by_seq[&2]);
    assert!(
        err_msg.is_some(),
        "seq 2: expected error, got: {:?}",
        by_seq[&2].results
    );
    // Verify the error references the missing queue.
    let err_text = err_msg.unwrap();
    assert!(
        err_text.contains("nonexistent-queue") || err_text.contains("not found"),
        "seq 2: expected queue-not-found error, got: {err_text}"
    );
    // Verify the error code is QueueNotFound.
    let first_result = by_seq[&2].results.first().unwrap();
    match &first_result.result {
        Some(fila_proto::enqueue_result::Result::Error(err)) => {
            assert_eq!(
                err.code,
                fila_proto::EnqueueErrorCode::QueueNotFound as i32,
                "seq 2: expected QueueNotFound error code"
            );
        }
        other => panic!("seq 2: expected Error, got: {other:?}"),
    }

    // Seq 3 should be success (stream continues after per-message error).
    assert!(
        first_message_id(by_seq[&3]).is_some(),
        "seq 3: expected MessageId, got: {:?}",
        by_seq[&3].results
    );

    drop(tx);
}

#[tokio::test]
async fn stream_enqueue_concurrent_streams() {
    let server = TestServer::start();
    let queue = "test-stream-concurrent";
    create_queue(server.addr(), queue).await;

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

            for i in 0u64..5 {
                tx.send(stream_req(
                    &q,
                    format!("stream-{stream_idx}-msg-{i}"),
                    stream_idx * 100 + i,
                ))
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

    assert_eq!(all_responses.len(), 10);

    for resp in &all_responses {
        assert!(
            first_message_id(resp).is_some(),
            "expected MessageId, got: {:?}",
            resp.results
        );
    }

    let ids: Vec<String> = all_responses
        .iter()
        .filter_map(|r| first_message_id(r).map(|s| s.to_string()))
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

    let seq_numbers: Vec<u64> = vec![42, 7, 999, 1, 100];
    for &seq in &seq_numbers {
        tx.send(stream_req(queue, format!("seq-{seq}"), seq))
            .await
            .unwrap();
    }

    let responses = collect_responses(&mut resp_stream, 5).await;

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
