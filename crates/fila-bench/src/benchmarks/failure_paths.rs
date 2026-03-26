use crate::measurement::ThroughputMeter;
use crate::progress::Progress;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, create_queue_with_lua_cli, BenchServer};
use std::collections::HashMap;
use std::time::Duration;
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;
const MESSAGE_COUNT: usize = 2000;
const NACK_RATE: f64 = 0.10;
const DLQ_EXHAUST_RATE: f64 = 0.05;
const FAIRNESS_KEY_COUNT: usize = 5;
const MESSAGES_PER_KEY: usize = 400;

/// Benchmark: produce messages, 10% nacked (redelivered), 90% acked.
/// Compare throughput and latency against 100%-ack baseline.
pub async fn bench_nack_storm(server: &BenchServer) -> Vec<BenchResult> {
    // --- Baseline: 100% ack ---
    let baseline_queue = "bench-nack-baseline";
    create_queue_cli(server.addr(), baseline_queue).await;
    let baseline_throughput =
        measure_consume_throughput(server, baseline_queue, |_| Action::Ack).await;

    // --- Nack storm: 10% nack, 90% ack ---
    let nack_queue = "bench-nack-storm";
    let on_failure = r#"function on_failure(msg) return { action = "retry", delay_ms = 0 } end"#;
    create_queue_with_lua_cli(server.addr(), nack_queue, None, Some(on_failure)).await;
    let mut nack_counter: usize = 0;
    let nack_throughput = measure_consume_throughput(server, nack_queue, |_| {
        nack_counter += 1;
        if nack_counter.is_multiple_of(10) {
            Action::Nack
        } else {
            Action::Ack
        }
    })
    .await;

    let degradation_pct = if baseline_throughput > 0.0 {
        (1.0 - nack_throughput / baseline_throughput) * 100.0
    } else {
        0.0
    };

    vec![
        BenchResult {
            name: "nack_storm_baseline_throughput".to_string(),
            value: baseline_throughput,
            unit: "msg/s".to_string(),
            metadata: HashMap::new(),
        },
        BenchResult {
            name: "nack_storm_10pct_nack_throughput".to_string(),
            value: nack_throughput,
            unit: "msg/s".to_string(),
            metadata: [("nack_rate".to_string(), serde_json::json!(NACK_RATE))]
                .into_iter()
                .collect(),
        },
        BenchResult {
            name: "nack_storm_throughput_degradation".to_string(),
            value: degradation_pct,
            unit: "%".to_string(),
            metadata: HashMap::new(),
        },
    ]
}

/// Benchmark: queue with on_failure hook routing to DLQ after max retries.
/// 5% of messages exhaust retries and route to DLQ. Report throughput vs pure-ack baseline.
pub async fn bench_dlq_routing_overhead(server: &BenchServer) -> Vec<BenchResult> {
    // --- Baseline: 100% ack, no on_failure hook ---
    let baseline_queue = "bench-dlq-baseline";
    create_queue_cli(server.addr(), baseline_queue).await;
    let baseline_throughput =
        measure_consume_throughput(server, baseline_queue, |_| Action::Ack).await;

    // --- DLQ workload: on_failure with max 2 retries, 5% of messages always nacked ---
    let dlq_queue = "bench-dlq-overhead";
    // DLQ queue must exist first (fila auto-creates <name>.dlq but we need to be safe)
    let dlq_target = "bench-dlq-overhead.dlq";
    create_queue_cli(server.addr(), dlq_target).await;

    let on_failure = r#"function on_failure(msg) if msg.attempts >= 2 then return { action = "dlq" } end return { action = "retry", delay_ms = 0 } end"#;
    create_queue_with_lua_cli(server.addr(), dlq_queue, None, Some(on_failure)).await;

    // Track which message IDs should be "poison" (always nacked).
    // We use a simple counter: every 20th message is poison (5%).
    let mut msg_counter: usize = 0;
    let dlq_throughput = measure_consume_throughput(server, dlq_queue, |_msg| {
        msg_counter += 1;
        if msg_counter.is_multiple_of(20) {
            Action::Nack
        } else {
            Action::Ack
        }
    })
    .await;

    let degradation_pct = if baseline_throughput > 0.0 {
        (1.0 - dlq_throughput / baseline_throughput) * 100.0
    } else {
        0.0
    };

    vec![
        BenchResult {
            name: "dlq_routing_baseline_throughput".to_string(),
            value: baseline_throughput,
            unit: "msg/s".to_string(),
            metadata: HashMap::new(),
        },
        BenchResult {
            name: "dlq_routing_mixed_throughput".to_string(),
            value: dlq_throughput,
            unit: "msg/s".to_string(),
            metadata: [(
                "dlq_exhaust_rate".to_string(),
                serde_json::json!(DLQ_EXHAUST_RATE),
            )]
            .into_iter()
            .collect(),
        },
        BenchResult {
            name: "dlq_routing_throughput_degradation".to_string(),
            value: degradation_pct,
            unit: "%".to_string(),
            metadata: HashMap::new(),
        },
    ]
}

/// Benchmark: queue with multiple fairness keys. One key's messages are all nacked
/// (poison pills), other keys ack normally. Measure per-fairness-key throughput.
pub async fn bench_poison_pill_isolation(server: &BenchServer) -> Vec<BenchResult> {
    let queue = "bench-poison-pill";
    let on_enqueue = r#"function on_enqueue(msg) local key = msg.headers["fk"] or "default" return { fairness_key = key, weight = 1, throttle_keys = {} } end"#;
    let on_failure = r#"function on_failure(msg) return { action = "retry", delay_ms = 0 } end"#;
    create_queue_with_lua_cli(server.addr(), queue, Some(on_enqueue), Some(on_failure)).await;

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    let payload = vec![0u8; PAYLOAD_SIZE];

    // Keys: "key-0" through "key-4", "key-0" is the poison key
    let keys: Vec<String> = (0..FAIRNESS_KEY_COUNT)
        .map(|i| format!("key-{i}"))
        .collect();
    let poison_key = &keys[0];

    // Enqueue messages for all keys
    let total_messages = MESSAGES_PER_KEY * FAIRNESS_KEY_COUNT;
    let mut enq_progress = Progress::new("poison pill enqueue", total_messages as u64);
    for key in &keys {
        for _ in 0..MESSAGES_PER_KEY {
            let headers: HashMap<String, String> =
                [("fk".to_string(), key.clone())].into_iter().collect();
            client
                .enqueue(queue, headers, payload.clone())
                .await
                .expect("enqueue");
            enq_progress.inc();
        }
    }
    enq_progress.finish();

    // Consume and track per-key throughput.
    // Poison key messages get nacked, others get acked.
    let mut stream = client.consume(queue).await.expect("consume");
    let mut per_key_acked: HashMap<String, u64> = HashMap::new();
    let mut per_key_nacked: HashMap<String, u64> = HashMap::new();

    // We want to consume at least all non-poison messages.
    // Non-poison messages = (FAIRNESS_KEY_COUNT - 1) * MESSAGES_PER_KEY
    let target_acks = (FAIRNESS_KEY_COUNT - 1) * MESSAGES_PER_KEY;
    let mut total_acked: usize = 0;
    let mut total_processed: usize = 0;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut consume_progress = Progress::new("poison pill consume", target_acks as u64);

    while total_acked < target_acks && tokio::time::Instant::now() < deadline {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
        let msg = match msg {
            Ok(Some(Ok(m))) => m,
            _ => break,
        };
        total_processed += 1;

        if msg.fairness_key == *poison_key {
            client.nack(queue, &msg.id, "poison").await.expect("nack");
            *per_key_nacked.entry(msg.fairness_key.clone()).or_insert(0) += 1;
        } else {
            client.ack(queue, &msg.id).await.expect("ack");
            *per_key_acked.entry(msg.fairness_key.clone()).or_insert(0) += 1;
            total_acked += 1;
            consume_progress.inc();
        }
    }
    consume_progress.finish();

    let mut results = Vec::new();

    // Per-key throughput as acked messages
    for key in &keys {
        let acked = per_key_acked.get(key).copied().unwrap_or(0);
        let nacked = per_key_nacked.get(key).copied().unwrap_or(0);
        results.push(BenchResult {
            name: format!("poison_pill_{key}_acked"),
            value: acked as f64,
            unit: "messages".to_string(),
            metadata: [
                ("nacked".to_string(), serde_json::json!(nacked)),
                (
                    "is_poison".to_string(),
                    serde_json::json!(key == poison_key),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    // Healthy keys should each have close to MESSAGES_PER_KEY acks
    let healthy_acked: Vec<u64> = keys
        .iter()
        .filter(|k| *k != poison_key)
        .map(|k| per_key_acked.get(k).copied().unwrap_or(0))
        .collect();

    let avg_healthy = if healthy_acked.is_empty() {
        0.0
    } else {
        healthy_acked.iter().sum::<u64>() as f64 / healthy_acked.len() as f64
    };

    let min_healthy = healthy_acked.iter().copied().min().unwrap_or(0);

    results.push(BenchResult {
        name: "poison_pill_healthy_avg_acked".to_string(),
        value: avg_healthy,
        unit: "messages".to_string(),
        metadata: [("expected".to_string(), serde_json::json!(MESSAGES_PER_KEY))]
            .into_iter()
            .collect(),
    });

    results.push(BenchResult {
        name: "poison_pill_healthy_min_acked".to_string(),
        value: min_healthy as f64,
        unit: "messages".to_string(),
        metadata: HashMap::new(),
    });

    results.push(BenchResult {
        name: "poison_pill_total_processed".to_string(),
        value: total_processed as f64,
        unit: "messages".to_string(),
        metadata: HashMap::new(),
    });

    results
}

enum Action {
    Ack,
    Nack,
}

/// Produce MESSAGE_COUNT messages, then consume them with the given decision function.
/// Returns the consume throughput (acked messages per second).
async fn measure_consume_throughput<F>(server: &BenchServer, queue: &str, mut decide: F) -> f64
where
    F: FnMut(&fila_sdk::ConsumeMessage) -> Action,
{
    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    let payload = vec![0u8; PAYLOAD_SIZE];

    // Produce messages
    let mut enq_progress = Progress::new(&format!("{queue} enqueue"), MESSAGE_COUNT as u64);
    for _ in 0..MESSAGE_COUNT {
        client
            .enqueue(queue, HashMap::new(), payload.clone())
            .await
            .expect("enqueue");
        enq_progress.inc();
    }
    enq_progress.finish();

    // Consume and ack/nack
    let mut stream = client.consume(queue).await.expect("consume");
    let mut meter = ThroughputMeter::start();
    let mut acked = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while acked < MESSAGE_COUNT && tokio::time::Instant::now() < deadline {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
        let msg = match msg {
            Ok(Some(Ok(m))) => m,
            _ => break,
        };

        match decide(&msg) {
            Action::Ack => {
                client.ack(queue, &msg.id).await.expect("ack");
                acked += 1;
                meter.increment();
            }
            Action::Nack => {
                client
                    .nack(queue, &msg.id, "bench-nack")
                    .await
                    .expect("nack");
                // Nacked messages get redelivered, don't count toward acked total
            }
        }
    }

    meter.msg_per_sec()
}
