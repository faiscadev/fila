mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use fila_sdk::{AckError, NackError};
use tokio_stream::StreamExt;

/// AC 5a: Enqueue → Consume → Ack lifecycle (basic message flow via SDK).
#[tokio::test]
async fn e2e_enqueue_consume_ack() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "lifecycle-ack");

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue a message
    let mut headers = HashMap::new();
    headers.insert("test".to_string(), "ack-flow".to_string());
    let msg_id = client
        .enqueue("lifecycle-ack", headers, b"payload-1".to_vec())
        .await
        .unwrap();
    assert!(!msg_id.is_empty());

    // Consume and receive
    let mut stream = client.consume("lifecycle-ack").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.payload, b"payload-1");
    assert_eq!(
        msg.headers.get("test").map(|s| s.as_str()),
        Some("ack-flow")
    );

    // Ack
    client.ack("lifecycle-ack", &msg_id).await.unwrap();

    // Double-ack returns error
    let err = client.ack("lifecycle-ack", &msg_id).await.unwrap_err();
    assert!(
        matches!(err, AckError::MessageNotFound(_)),
        "expected MessageNotFound, got: {err:?}"
    );
}

/// AC 5b: Enqueue → Consume → Nack → re-Consume (retry with attempt count increment).
#[tokio::test]
async fn e2e_enqueue_consume_nack_retry() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "lifecycle-nack");

    let client = helpers::sdk_client(server.addr()).await;

    let msg_id = client
        .enqueue("lifecycle-nack", HashMap::new(), b"retry-me".to_vec())
        .await
        .unwrap();

    let mut stream = client.consume("lifecycle-nack").await.unwrap();

    // First delivery
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.attempt_count, 0);

    // Nack
    client
        .nack("lifecycle-nack", &msg_id, "transient error")
        .await
        .unwrap();

    // Redelivery
    let msg2 = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timeout waiting for redelivery")
        .expect("stream ended")
        .expect("consume error");

    assert_eq!(msg2.id, msg_id);
    assert_eq!(msg2.attempt_count, 1);

    // Nack again to verify attempt_count keeps incrementing
    client
        .nack("lifecycle-nack", &msg_id, "still failing")
        .await
        .unwrap();

    let msg3 = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg3.attempt_count, 2);

    // Ack to clean up
    client.ack("lifecycle-nack", &msg_id).await.unwrap();

    // Double-nack on acked message returns error
    let err = client
        .nack("lifecycle-nack", &msg_id, "already gone")
        .await
        .unwrap_err();
    assert!(
        matches!(err, NackError::MessageNotFound(_)),
        "expected MessageNotFound, got: {err:?}"
    );
}
