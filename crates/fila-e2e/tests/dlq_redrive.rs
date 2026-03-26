mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5e: DLQ flow — nack to exhaustion → DLQ → Redrive via CLI → re-Consume from source.
///
/// Each consume session uses a separate client because FIBP supports only one
/// consume stream per connection.
#[tokio::test]
#[ignore = "FIBP redrive does not wake pending consumers — needs scheduler fix"]
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

    // --- Drive a message to DLQ via nack ---
    let client1 = helpers::sdk_client(server.addr()).await;
    let msg_id = client1
        .enqueue("redrive-src", HashMap::new(), b"redrive-me".to_vec())
        .await
        .unwrap();

    let mut stream = client1.consume("redrive-src").await.unwrap();

    // First delivery: nack → retry
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    client1
        .nack("redrive-src", &msg_id, "failing")
        .await
        .unwrap();

    // Second delivery: nack → DLQ
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    client1
        .nack("redrive-src", &msg_id, "failing again")
        .await
        .unwrap();

    drop(stream);
    drop(client1);

    // Wait for scheduler to route to DLQ.
    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Redrive via CLI ---
    let redrive = helpers::cli_run(server.addr(), &["redrive", "redrive-src.dlq"]);
    assert!(redrive.success, "redrive failed: {}", redrive.stderr);
    assert!(
        redrive.stdout.contains("1 message"),
        "expected 1 message redriven, got: {}",
        redrive.stdout
    );

    // --- Consume from source queue — message should be back ---
    let client2 = helpers::sdk_client(server.addr()).await;
    let mut stream2 = client2.consume("redrive-src").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream2.next())
        .await
        .expect("timeout waiting for redriven message")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(msg.id, msg_id);
    assert_eq!(
        msg.attempt_count, 0,
        "attempt count should be reset after redrive"
    );
    assert_eq!(msg.payload, b"redrive-me");

    client2.ack("redrive-src", &msg.id).await.unwrap();
}
