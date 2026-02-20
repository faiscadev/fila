mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5e: DLQ flow — nack to exhaustion → DLQ → Redrive via CLI → re-Consume from source.
#[tokio::test]
async fn e2e_dlq_redrive() {
    let server = helpers::TestServer::start();

    let on_failure = r#"function on_failure(msg) if msg.attempts >= 2 then return { action = "dlq" } end return { action = "retry", delay_ms = 0 } end"#;

    helpers::create_queue_with_scripts_cli(
        server.addr(),
        "redrive-src",
        None,
        Some(on_failure),
        None,
    );

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue a message
    let msg_id = client
        .enqueue("redrive-src", HashMap::new(), b"redrive-me".to_vec())
        .await
        .unwrap();

    // Nack until DLQ (attempts: nack→on_failure(1)→retry, nack→on_failure(2)→DLQ)
    let mut stream = client.consume("redrive-src").await.unwrap();

    // First delivery: attempt_count=0, nack → on_failure gets attempts=1 → retry
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    client
        .nack("redrive-src", &msg_id, "failing")
        .await
        .unwrap();

    // Second delivery: attempt_count=1, nack → on_failure gets attempts=2 → DLQ
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    client
        .nack("redrive-src", &msg_id, "failing again")
        .await
        .unwrap();

    // Drop the source stream so the scheduler knows the consumer is gone.
    // This prevents the redriven message from being delivered to the old stream.
    drop(stream);

    // Give the scheduler a moment to route to DLQ and unregister the consumer
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify DLQ has the message by consuming from it
    let mut dlq_stream = client.consume("redrive-src.dlq").await.unwrap();
    let dlq_msg = tokio::time::timeout(Duration::from_secs(5), dlq_stream.next())
        .await
        .expect("timeout waiting for DLQ message")
        .expect("DLQ stream ended")
        .expect("DLQ consume error");
    assert_eq!(dlq_msg.id, msg_id);

    // Ack the DLQ message so it's no longer in-flight (redrive only moves pending messages)
    client.ack("redrive-src.dlq", &dlq_msg.id).await.unwrap();
    drop(dlq_stream);

    // Re-enqueue to DLQ for redrive (the original message was acked from DLQ above,
    // so we need a fresh message in the DLQ to redrive)
    // Actually: let's not ack — we need to nack it back to pending in DLQ.
    // The simplest approach: re-do the whole flow with a fresh message.

    // Enqueue a new message and drive it to DLQ
    let msg_id2 = client
        .enqueue("redrive-src", HashMap::new(), b"redrive-me-2".to_vec())
        .await
        .unwrap();

    let mut stream2 = client.consume("redrive-src").await.unwrap();
    for _ in 0..2 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream2.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(msg.id, msg_id2);
        client
            .nack("redrive-src", &msg_id2, "failing")
            .await
            .unwrap();
    }
    drop(stream2);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Now redrive via CLI — the message is pending (not in-flight) in the DLQ
    let redrive = helpers::cli_run(server.addr(), &["redrive", "redrive-src.dlq"]);
    assert!(redrive.success, "redrive failed: {}", redrive.stderr);
    assert!(
        redrive.stdout.contains("1 message"),
        "expected 1 message redriven, got: {}",
        redrive.stdout
    );

    // Consume from source queue — message should be available with reset attempt count
    let mut stream3 = client.consume("redrive-src").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream3.next())
        .await
        .expect("timeout waiting for redriven message")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(msg.id, msg_id2);
    assert_eq!(
        msg.attempt_count, 0,
        "attempt count should be reset after redrive"
    );
    assert_eq!(msg.payload, b"redrive-me-2");

    // Clean up
    client.ack("redrive-src", &msg.id).await.unwrap();
}
