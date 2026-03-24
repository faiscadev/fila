mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// Consume multiple messages and verify all are received individually through
/// the stream, regardless of whether the server batched them internally.
#[tokio::test]
async fn e2e_delivery_batching_all_messages_received() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-deliver");

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue 20 messages rapidly so the server has a chance to batch deliveries.
    let mut expected_ids = Vec::new();
    for i in 0..20 {
        let id = client
            .enqueue(
                "batch-deliver",
                HashMap::new(),
                format!("payload-{i}").into_bytes(),
            )
            .await
            .unwrap();
        expected_ids.push(id);
    }

    // Open consumer stream and receive all 20 messages.
    let mut stream = client.consume("batch-deliver").await.unwrap();
    let mut received_ids = Vec::new();
    for _ in 0..20 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timeout waiting for message")
            .expect("stream ended")
            .expect("consume error");
        received_ids.push(msg.id.clone());
    }

    // All 20 messages should be received.
    let expected_set: std::collections::HashSet<_> = expected_ids.iter().collect();
    let received_set: std::collections::HashSet<_> = received_ids.iter().collect();
    assert_eq!(
        expected_set, received_set,
        "all enqueued messages should be received"
    );
}

/// Ack and nack individual messages from what may be a batched delivery,
/// verifying that each message has independent lease management.
#[tokio::test]
async fn e2e_delivery_batching_individual_ack_nack() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-ack-nack");

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue 5 messages.
    let mut ids = Vec::new();
    for i in 0..5 {
        let id = client
            .enqueue(
                "batch-ack-nack",
                HashMap::new(),
                format!("msg-{i}").into_bytes(),
            )
            .await
            .unwrap();
        ids.push(id);
    }

    // Consume all 5.
    let mut stream = client.consume("batch-ack-nack").await.unwrap();
    let mut received = Vec::new();
    for _ in 0..5 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("consume error");
        received.push(msg);
    }

    // Ack the first 3 individually.
    for msg in &received[..3] {
        client.ack("batch-ack-nack", &msg.id).await.unwrap();
    }

    // Nack the 4th message -- it should be redelivered.
    client
        .nack("batch-ack-nack", &received[3].id, "test error")
        .await
        .unwrap();

    // Ack the 5th.
    client.ack("batch-ack-nack", &received[4].id).await.unwrap();

    // The nacked message should be redelivered with incremented attempt_count.
    let redelivered = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for redelivery")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(redelivered.id, received[3].id);
    assert_eq!(
        redelivered.attempt_count, 1,
        "redelivered message should have attempt_count=1"
    );

    // Ack the redelivered message to clean up.
    client.ack("batch-ack-nack", &redelivered.id).await.unwrap();
}

/// Single-message delivery still works correctly (backward compatible path).
#[tokio::test]
async fn e2e_delivery_batching_single_message_backward_compat() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-single");

    let client = helpers::sdk_client(server.addr()).await;

    // Open consumer first, then enqueue 1 message. This forces single-message
    // delivery since the message arrives after the stream is already waiting.
    let mut stream = client.consume("batch-single").await.unwrap();

    // Brief delay to let the consumer register.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msg_id = client
        .enqueue("batch-single", HashMap::new(), b"single-msg".to_vec())
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.payload, b"single-msg");

    client.ack("batch-single", &msg_id).await.unwrap();
}
