//! Proves that the internal delivery channel is bounded by application-level
//! backpressure. When a consumer stops reading, the reader pauses TCP reads
//! and the internal channel stays capped at the high-water mark.
//!
//! Requires `test-internals` feature: `cargo test --features test-internals`
#![cfg(feature = "test-internals")]

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

/// Prove that the internal delivery channel does not grow unbounded when the
/// consumer stops reading. The server delivers messages, the reader buffers
/// them in the internal channel, but TCP backpressure caps the growth.
#[tokio::test(flavor = "multi_thread")]
async fn internal_channel_bounded_by_tcp_backpressure() {
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

    // Enqueue 10,000 messages on a separate connection
    let producer = FilaClient::connect(&addr).await.unwrap();
    for _ in 0..10_000 {
        let _ = producer
            .enqueue("backpressure-q", HashMap::new(), b"x".to_vec())
            .await;
    }

    // Connect consumer with a tiny user buffer (2) so the forwarder blocks fast.
    // The internal channel should NOT grow to 10,000.
    let consumer = FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_delivery_buffer_size(2),
    )
    .await
    .unwrap();

    let _stream = consumer.consume("backpressure-q").await.unwrap();

    // Don't read from the stream — let the server deliver and the forwarder block.
    // Wait for the system to stabilize.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let internal_len = consumer.delivery_internal_len().await;

    // The internal channel should be bounded by TCP window, not by message count.
    // TCP receive window is typically 64KB-1MB. At ~10 bytes per message, that's
    // at most ~100K messages — but in practice much less because the server's
    // delivery channel (64) and TCP send buffer limit how fast it can push.
    //
    // We assert it's significantly less than the 10,000 messages we enqueued.
    // In practice it should be a few hundred at most.
    assert!(
        internal_len < 5000,
        "internal channel grew to {internal_len} — expected TCP backpressure to cap it well below 10,000"
    );

    eprintln!("internal channel size with stalled consumer: {internal_len}");

    drop(_stream);
    child.kill().unwrap();
}
