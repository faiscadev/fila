mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5c: Lua on_enqueue assigns fairness key, weight, and throttle keys from headers.
#[tokio::test]
async fn e2e_lua_on_enqueue_assigns_keys() {
    let server = helpers::TestServer::start();

    let on_enqueue = r#"function on_enqueue(msg) return { fairness_key = msg.headers["tenant"] or "default", weight = tonumber(msg.headers["weight"]) or 1, throttle_keys = {} } end"#;

    helpers::create_queue_with_scripts_cli(
        server.addr(),
        "lua-enqueue",
        Some(on_enqueue),
        None,
        None,
    );

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue messages with different tenants
    let mut h1 = HashMap::new();
    h1.insert("tenant".to_string(), "acme".to_string());
    let id1 = client
        .enqueue("lua-enqueue", h1, b"msg1".to_vec())
        .await
        .unwrap();

    let mut h2 = HashMap::new();
    h2.insert("tenant".to_string(), "globex".to_string());
    h2.insert("weight".to_string(), "3".to_string());
    let id2 = client
        .enqueue("lua-enqueue", h2, b"msg2".to_vec())
        .await
        .unwrap();

    // Lease and verify fairness keys
    let mut stream = client.lease("lua-enqueue").await.unwrap();

    let mut received = Vec::new();
    for _ in 0..2 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        received.push(msg);
    }

    let msg1 = received
        .iter()
        .find(|m| m.id == id1)
        .expect("msg1 not received");
    let msg2 = received
        .iter()
        .find(|m| m.id == id2)
        .expect("msg2 not received");

    assert_eq!(msg1.fairness_key, "acme");
    assert_eq!(msg2.fairness_key, "globex");

    // Clean up
    for msg in &received {
        client.ack("lua-enqueue", &msg.id).await.unwrap();
    }
}

/// AC 5d: Lua on_failure decides retry vs DLQ based on attempt count.
#[tokio::test]
async fn e2e_lua_on_failure_retry_vs_dlq() {
    let server = helpers::TestServer::start();

    let on_failure = r#"function on_failure(msg) if msg.attempts >= 3 then return { action = "dlq" } end return { action = "retry", delay_ms = 0 } end"#;

    helpers::create_queue_with_scripts_cli(
        server.addr(),
        "lua-failure",
        None,
        Some(on_failure),
        None,
    );

    let client = helpers::sdk_client(server.addr()).await;

    let msg_id = client
        .enqueue("lua-failure", HashMap::new(), b"fail-me".to_vec())
        .await
        .unwrap();

    let mut stream = client.lease("lua-failure").await.unwrap();

    // Nack 3 times — on_failure should retry first 3, then DLQ on the 4th nack
    // attempt_count starts at 0, so: attempt 0 → nack → attempt 1 → nack → attempt 2 → nack → DLQ
    for expected_attempt in 0..3 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap_or_else(|_| panic!("timeout waiting for attempt {expected_attempt}"))
            .expect("stream ended")
            .expect("lease error");

        assert_eq!(msg.id, msg_id);
        assert_eq!(msg.attempt_count, expected_attempt);

        client
            .nack("lua-failure", &msg_id, "still failing")
            .await
            .unwrap();
    }

    // Message should now be in the DLQ. Lease from the DLQ.
    let mut dlq_stream = client.lease("lua-failure.dlq").await.unwrap();
    let dlq_msg = tokio::time::timeout(Duration::from_secs(5), dlq_stream.next())
        .await
        .expect("timeout waiting for DLQ message")
        .expect("DLQ stream ended")
        .expect("DLQ lease error");

    assert_eq!(dlq_msg.id, msg_id);
    assert_eq!(dlq_msg.payload, b"fail-me");

    // Clean up
    client.ack("lua-failure.dlq", &dlq_msg.id).await.unwrap();
}
