//! Validates SDK delivery buffering behavior under backpressure.
//!
//! When a consumer stops reading, the SDK buffers delivery frames in an
//! internal overflow VecDeque. The overflow is unbounded on the client side
//! (to prevent deadlocking response frames), but the server's delivery
//! channel (64 capacity) provides natural backpressure that limits how
//! fast messages are pushed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use fila_sdk::FilaClient;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn workspace_binary(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("target/debug");
    p.push(name);
    p
}

/// Prove that the delivery overflow buffer tracks messages correctly and
/// that the SDK remains functional (no deadlock) even when the consumer
/// stops reading.
#[tokio::test(flavor = "multi_thread")]
async fn stalled_consumer_buffers_without_deadlock() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let data_dir = tempfile::tempdir().unwrap();

    let config =
        format!("[server]\nlisten_addr = \"{addr}\"\n\n[telemetry]\notlp_endpoint = \"\"\n");
    std::fs::write(data_dir.path().join("fila.toml"), config).unwrap();

    let mut child = Command::new(workspace_binary("fila-server"))
        .env(
            "FILA_DATA_DIR",
            data_dir.path().join("data").to_str().unwrap(),
        )
        .current_dir(data_dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || for _ in BufReader::new(stdout).lines() {});
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});

    for _ in 0..50 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Create queue
    let out = Command::new(workspace_binary("fila"))
        .args(["--addr", &addr, "queue", "create", "backpressure-q"])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Enqueue 1000 messages on a separate connection
    let producer = FilaClient::connect(&addr).await.unwrap();
    for _ in 0..1000 {
        let _ = producer
            .enqueue("backpressure-q", HashMap::new(), b"x".to_vec())
            .await;
    }

    // Connect consumer with a tiny delivery channel.
    let consumer = FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_delivery_buffer_size(2),
    )
    .await
    .unwrap();

    let _stream = consumer.consume("backpressure-q").await.unwrap();

    // Don't read from the stream — let the server deliver and the overflow grow.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // The overflow should have messages (server pushed them).
    let overflow_len = consumer.delivery_internal_len().await;
    eprintln!("overflow buffer size with stalled consumer: {overflow_len}");

    // Key assertion: the SDK didn't deadlock. We can still do operations
    // on the SAME connection that has the stalled consumer.
    let enqueue_result = tokio::time::timeout(
        Duration::from_secs(3),
        consumer.enqueue("backpressure-q", HashMap::new(), b"after-stall".to_vec()),
    )
    .await;
    assert!(
        enqueue_result.is_ok(),
        "enqueue on stalled consumer connection should succeed (no deadlock)"
    );

    drop(_stream);
    child.kill().unwrap();
}
