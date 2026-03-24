mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use fila_sdk::{BatchEnqueueResult, EnqueueMessage};
use tokio_stream::StreamExt;

/// Batch enqueue multiple messages and verify all are stored and consumable.
#[tokio::test]
async fn e2e_batch_enqueue_all_stored() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-all");

    let client = helpers::sdk_client(server.addr()).await;

    // Build a batch of 5 messages
    let messages: Vec<EnqueueMessage> = (0..5)
        .map(|i| EnqueueMessage {
            queue: "batch-all".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("index".to_string(), i.to_string());
                h
            },
            payload: format!("payload-{i}").into_bytes(),
        })
        .collect();

    let results = client.batch_enqueue(messages).await.unwrap();

    // All 5 should succeed
    assert_eq!(results.len(), 5);
    let mut msg_ids = Vec::new();
    for (i, result) in results.iter().enumerate() {
        match result {
            BatchEnqueueResult::Success(id) => {
                assert!(!id.is_empty(), "message {i} has empty ID");
                msg_ids.push(id.clone());
            }
            BatchEnqueueResult::Error(err) => {
                panic!("message {i} failed: {err}");
            }
        }
    }

    // All IDs should be unique
    let unique: std::collections::HashSet<_> = msg_ids.iter().collect();
    assert_eq!(unique.len(), 5, "all message IDs should be unique");

    // Consume all 5 messages
    let mut stream = client.consume("batch-all").await.unwrap();
    let mut consumed = Vec::new();
    for _ in 0..5 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timeout waiting for message")
            .expect("stream ended")
            .expect("consume error");
        consumed.push(msg);
    }

    // Verify all message IDs are present
    let consumed_ids: std::collections::HashSet<_> = consumed.iter().map(|m| &m.id).collect();
    for id in &msg_ids {
        assert!(
            consumed_ids.contains(id),
            "enqueued message {id} not found in consumed messages"
        );
    }
}

/// Batch enqueue with one invalid message (empty queue name) should succeed
/// for the valid messages and return an error for the invalid one.
#[tokio::test]
async fn e2e_batch_enqueue_partial_failure() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-partial");

    let client = helpers::sdk_client(server.addr()).await;

    let messages = vec![
        EnqueueMessage {
            queue: "batch-partial".to_string(),
            headers: HashMap::new(),
            payload: b"good-1".to_vec(),
        },
        EnqueueMessage {
            queue: "".to_string(), // Invalid: empty queue name
            headers: HashMap::new(),
            payload: b"bad".to_vec(),
        },
        EnqueueMessage {
            queue: "batch-partial".to_string(),
            headers: HashMap::new(),
            payload: b"good-2".to_vec(),
        },
    ];

    let results = client.batch_enqueue(messages).await.unwrap();

    assert_eq!(results.len(), 3);

    // First message: success
    assert!(
        matches!(&results[0], BatchEnqueueResult::Success(_)),
        "expected first message to succeed, got: {:?}",
        results[0]
    );

    // Second message: error (empty queue name)
    assert!(
        matches!(&results[1], BatchEnqueueResult::Error(_)),
        "expected second message to fail, got: {:?}",
        results[1]
    );

    // Third message: success (not affected by the second)
    assert!(
        matches!(&results[2], BatchEnqueueResult::Success(_)),
        "expected third message to succeed, got: {:?}",
        results[2]
    );

    // Verify the two successful messages are consumable
    let mut stream = client.consume("batch-partial").await.unwrap();
    for _ in 0..2 {
        let _msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timeout waiting for message")
            .expect("stream ended")
            .expect("consume error");
    }
}

/// Batch enqueue preserves per-queue ordering: messages to the same queue
/// appear in the order they were submitted in the batch.
#[tokio::test]
async fn e2e_batch_enqueue_ordering_preserved() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-order");

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue 10 messages with sequenced payloads
    let messages: Vec<EnqueueMessage> = (0..10)
        .map(|i| EnqueueMessage {
            queue: "batch-order".to_string(),
            headers: HashMap::new(),
            payload: format!("seq-{i:03}").into_bytes(),
        })
        .collect();

    let results = client.batch_enqueue(messages).await.unwrap();

    // Collect IDs in batch order
    let batch_ids: Vec<String> = results
        .into_iter()
        .map(|r| match r {
            BatchEnqueueResult::Success(id) => id,
            BatchEnqueueResult::Error(e) => panic!("unexpected error: {e}"),
        })
        .collect();

    // Consume all 10 messages
    let mut stream = client.consume("batch-order").await.unwrap();
    let mut consumed_ids = Vec::new();
    for _ in 0..10 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timeout waiting for message")
            .expect("stream ended")
            .expect("consume error");
        consumed_ids.push(msg.id);
    }

    // Messages should be delivered in the same order they were enqueued
    assert_eq!(
        batch_ids, consumed_ids,
        "batch order should be preserved in delivery"
    );
}

/// Empty batch should return empty results.
#[tokio::test]
async fn e2e_batch_enqueue_empty_batch() {
    let server = helpers::TestServer::start();

    let client = helpers::sdk_client(server.addr()).await;

    let results = client.batch_enqueue(vec![]).await.unwrap();
    assert!(
        results.is_empty(),
        "empty batch should return empty results"
    );
}
