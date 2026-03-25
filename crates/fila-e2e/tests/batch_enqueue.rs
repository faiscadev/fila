mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use fila_sdk::EnqueueMessage;
use tokio_stream::StreamExt;

/// Enqueue multiple messages in one call and verify all are stored and consumable.
#[tokio::test]
async fn e2e_enqueue_many_all_stored() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-all");

    let client = helpers::sdk_client(server.addr()).await;

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

    let results = client.enqueue_many(messages).await;

    assert_eq!(results.len(), 5);
    let mut msg_ids = Vec::new();
    for (i, result) in results.iter().enumerate() {
        let id = result
            .as_ref()
            .unwrap_or_else(|e| panic!("message {i} failed: {e}"));
        assert!(!id.is_empty(), "message {i} has empty ID");
        msg_ids.push(id.clone());
    }

    let unique: std::collections::HashSet<_> = msg_ids.iter().collect();
    assert_eq!(unique.len(), 5, "all message IDs should be unique");

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

    let consumed_ids: std::collections::HashSet<_> = consumed.iter().map(|m| &m.id).collect();
    for id in &msg_ids {
        assert!(
            consumed_ids.contains(id),
            "enqueued message {id} not found in consumed messages"
        );
    }
}

/// Enqueue many with one invalid message — valid messages succeed, invalid gets error.
#[tokio::test]
async fn e2e_enqueue_many_partial_failure() {
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

    let results = client.enqueue_many(messages).await;

    assert_eq!(results.len(), 3);

    assert!(
        results[0].is_ok(),
        "expected first message to succeed, got: {:?}",
        results[0]
    );

    assert!(
        results[1].is_err(),
        "expected second message to fail, got: {:?}",
        results[1]
    );

    assert!(
        results[2].is_ok(),
        "expected third message to succeed, got: {:?}",
        results[2]
    );

    let mut stream = client.consume("batch-partial").await.unwrap();
    for _ in 0..2 {
        let _msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timeout waiting for message")
            .expect("stream ended")
            .expect("consume error");
    }
}

/// Enqueue many preserves per-queue ordering.
#[tokio::test]
async fn e2e_enqueue_many_ordering_preserved() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "batch-order");

    let client = helpers::sdk_client(server.addr()).await;

    let messages: Vec<EnqueueMessage> = (0..10)
        .map(|i| EnqueueMessage {
            queue: "batch-order".to_string(),
            headers: HashMap::new(),
            payload: format!("seq-{i:03}").into_bytes(),
        })
        .collect();

    let results = client.enqueue_many(messages).await;

    let batch_ids: Vec<String> = results
        .into_iter()
        .map(|r| r.unwrap_or_else(|e| panic!("unexpected error: {e}")))
        .collect();

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

    assert_eq!(
        batch_ids, consumed_ids,
        "order should be preserved in delivery"
    );
}

/// Empty enqueue_many should return empty results.
#[tokio::test]
async fn e2e_enqueue_many_empty() {
    let server = helpers::TestServer::start();

    let client = helpers::sdk_client(server.addr()).await;

    let results = client.enqueue_many(vec![]).await;
    assert!(results.is_empty(), "empty call should return empty results");
}
