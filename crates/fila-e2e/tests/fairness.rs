mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5f: DRR fairness — multi-key weighted delivery.
/// Higher-weight keys get proportionally more throughput.
///
/// Strategy: Use a small DRR quantum (1) so the weighted distribution
/// emerges immediately. With quantum=1, each round delivers exactly
/// weight messages per key: "high" (weight 3) gets 3, "low" (weight 1)
/// gets 1, so the ratio is 75/25 from the start.
///
/// Setup: 2 fairness keys — "high" (weight 3) and "low" (weight 1).
/// Enqueue 100 per key, lease only 60 → distribution should be ~75/25.
#[tokio::test]
async fn e2e_drr_weighted_fairness() {
    let server = helpers::TestServer::start_with_quantum(1);

    let on_enqueue = r#"function on_enqueue(msg) return { fairness_key = msg.headers["key"] or "default", weight = tonumber(msg.headers["weight"]) or 1, throttle_keys = {} } end"#;

    helpers::create_queue_with_scripts_cli(server.addr(), "fairness", Some(on_enqueue), None, None);

    let client = helpers::sdk_client(server.addr()).await;

    let msgs_per_key = 100;

    // Enqueue messages for "high" key (weight 3)
    for i in 0..msgs_per_key {
        let mut h = HashMap::new();
        h.insert("key".to_string(), "high".to_string());
        h.insert("weight".to_string(), "3".to_string());
        client
            .enqueue("fairness", h, format!("high-{i}").into_bytes())
            .await
            .unwrap();
    }

    // Enqueue messages for "low" key (weight 1)
    for i in 0..msgs_per_key {
        let mut h = HashMap::new();
        h.insert("key".to_string(), "low".to_string());
        h.insert("weight".to_string(), "1".to_string());
        client
            .enqueue("fairness", h, format!("low-{i}").into_bytes())
            .await
            .unwrap();
    }

    // Lease only a subset — both keys remain saturated throughout
    let sample_size = 60;
    let mut stream = client.lease("fairness").await.unwrap();

    let mut high_count = 0u32;
    let mut low_count = 0u32;

    for _ in 0..sample_size {
        let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("lease error");

        match msg.fairness_key.as_str() {
            "high" => high_count += 1,
            "low" => low_count += 1,
            other => panic!("unexpected fairness key: {other}"),
        }

        client.ack("fairness", &msg.id).await.unwrap();
    }

    assert_eq!(high_count + low_count, sample_size);

    // Expected: high gets 3/4 = 75%, low gets 1/4 = 25%
    let high_ratio = high_count as f64 / sample_size as f64;
    let low_ratio = low_count as f64 / sample_size as f64;

    assert!(
        (high_ratio - 0.75).abs() < 0.15,
        "high key ratio {high_ratio:.2} not within 15% of expected 0.75 (high={high_count}, low={low_count})"
    );
    assert!(
        (low_ratio - 0.25).abs() < 0.15,
        "low key ratio {low_ratio:.2} not within 15% of expected 0.25 (high={high_count}, low={low_count})"
    );
}
