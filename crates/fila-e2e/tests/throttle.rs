mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5g: Throttle — rate-limited key skipped, unthrottled keys served.
///
/// Strategy: Set a very low throttle rate (1 msg/s) for one provider key.
/// Enqueue messages for both throttled and unthrottled keys.
/// Verify that unthrottled messages arrive first/faster.
#[tokio::test]
async fn e2e_throttle_rate_limiting() {
    let server = helpers::TestServer::start();

    let on_enqueue = r#"function on_enqueue(msg) local keys = {} if msg.headers["provider"] then table.insert(keys, "provider:" .. msg.headers["provider"]) end return { fairness_key = msg.headers["tenant"] or "default", weight = 1, throttle_keys = keys } end"#;

    helpers::create_queue_with_scripts_cli(server.addr(), "throttle", Some(on_enqueue), None, None);

    // Set throttle rate: provider:slow = 1 msg/s
    let set_result = helpers::cli_run(
        server.addr(),
        &["config", "set", "throttle.provider:slow", "1,1"],
    );
    assert!(
        set_result.success,
        "set throttle failed: {}",
        set_result.stderr
    );

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue 3 messages for the throttled provider
    for i in 0..3 {
        let mut h = HashMap::new();
        h.insert("tenant".to_string(), "t1".to_string());
        h.insert("provider".to_string(), "slow".to_string());
        client
            .enqueue("throttle", h, format!("slow-{i}").into_bytes())
            .await
            .unwrap();
    }

    // Enqueue 3 messages for an unthrottled tenant (no provider header → no throttle keys)
    for i in 0..3 {
        let mut h = HashMap::new();
        h.insert("tenant".to_string(), "t2".to_string());
        client
            .enqueue("throttle", h, format!("fast-{i}").into_bytes())
            .await
            .unwrap();
    }

    let mut stream = client.lease("throttle").await.unwrap();

    // Collect the first 3 messages quickly — unthrottled messages should arrive first
    // since the throttled key only allows 1 msg/s (starts with 1 token).
    let mut received = Vec::new();
    for _ in 0..4 {
        let msg = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("timeout — expected at least 4 messages quickly")
            .expect("stream ended")
            .expect("lease error");
        received.push(msg);
    }

    // Count: we should have received all 3 unthrottled messages + 1 throttled message
    // (the throttle bucket starts with 1 token, then is exhausted)
    let fast_count = received.iter().filter(|m| m.fairness_key == "t2").count();
    let slow_count = received.iter().filter(|m| m.fairness_key == "t1").count();

    assert!(
        fast_count >= 3,
        "expected at least 3 unthrottled messages in first batch, got {fast_count}"
    );
    assert!(
        slow_count >= 1,
        "expected at least 1 throttled message (initial token), got {slow_count}"
    );

    // Clean up received messages
    for msg in &received {
        client.ack("throttle", &msg.id).await.unwrap();
    }
}
