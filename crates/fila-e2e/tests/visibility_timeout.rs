mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5k: Visibility timeout — lease message → wait for expiry → message available for re-lease.
#[tokio::test]
async fn e2e_visibility_timeout_expiry() {
    let server = helpers::TestServer::start();

    // Create queue with a short visibility timeout (2 seconds)
    helpers::create_queue_with_scripts_cli(
        server.addr(),
        "vt-test",
        None,
        None,
        Some(2000), // 2s visibility timeout
    );

    let client = helpers::sdk_client(server.addr()).await;

    let msg_id = client
        .enqueue("vt-test", HashMap::new(), b"timeout-me".to_vec())
        .await
        .unwrap();

    // First consumer leases the message but does NOT ack/nack
    let mut stream1 = client.lease("vt-test").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream1.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.attempt_count, 0);

    // Drop the first stream (simulates consumer disconnect without ack)
    drop(stream1);

    // Wait for visibility timeout to expire (2s + buffer)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Second consumer should receive the message with incremented attempt count
    let mut stream2 = client.lease("vt-test").await.unwrap();
    let msg2 = tokio::time::timeout(Duration::from_secs(5), stream2.next())
        .await
        .expect("timeout — message should have been re-made available after visibility timeout")
        .expect("stream ended")
        .expect("lease error");

    assert_eq!(msg2.id, msg_id, "should be the same message");
    assert_eq!(msg2.payload, b"timeout-me");
    assert!(
        msg2.attempt_count >= 1,
        "attempt count should be incremented after visibility timeout redelivery, got: {}",
        msg2.attempt_count
    );

    // Clean up
    client.ack("vt-test", &msg2.id).await.unwrap();
}
