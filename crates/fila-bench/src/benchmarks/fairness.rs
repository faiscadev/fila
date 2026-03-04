use crate::measurement::ThroughputMeter;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, create_queue_with_lua_cli, BenchServer};
use std::collections::HashMap;
use std::time::Duration;
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;
const MEASURE_SECS: u64 = 10;
const WARMUP_SECS: u64 = 3;

/// Measure throughput with fair scheduling enabled vs raw FIFO (NFR2: <5% overhead).
pub async fn bench_fairness_overhead(server: &BenchServer) -> Vec<BenchResult> {
    // Baseline: FIFO queue (no Lua, single implicit fairness key)
    let fifo_queue = "bench-fairness-fifo";
    create_queue_cli(server.host_port(), fifo_queue);
    let fifo_throughput = measure_enqueue_throughput(server, fifo_queue).await;

    // Fair scheduling: queue with Lua assigning fairness_key from header
    let fair_queue = "bench-fairness-fair";
    let on_enqueue = r#"
        local key = msg.headers["tenant_id"] or "default"
        return { fairness_key = key, weight = 1 }
    "#;
    create_queue_with_lua_cli(server.host_port(), fair_queue, Some(on_enqueue), None);
    let fair_throughput = measure_enqueue_throughput_with_headers(
        server,
        fair_queue,
        || {
            let mut h = HashMap::new();
            h.insert("tenant_id".to_string(), "tenant-1".to_string());
            h
        },
    )
    .await;

    let overhead_pct = if fifo_throughput > 0.0 {
        (1.0 - fair_throughput / fifo_throughput) * 100.0
    } else {
        0.0
    };

    vec![
        BenchResult {
            name: "fairness_overhead_fifo_throughput".to_string(),
            value: fifo_throughput,
            unit: "msg/s".to_string(),
            metadata: HashMap::new(),
        },
        BenchResult {
            name: "fairness_overhead_fair_throughput".to_string(),
            value: fair_throughput,
            unit: "msg/s".to_string(),
            metadata: HashMap::new(),
        },
        BenchResult {
            name: "fairness_overhead_pct".to_string(),
            value: overhead_pct,
            unit: "%".to_string(),
            metadata: [("nfr2_target".to_string(), serde_json::json!("< 5%"))]
                .into_iter()
                .collect(),
        },
    ]
}

/// Measure fairness accuracy across keys with varying weights (NFR3: within 5%).
pub async fn bench_fairness_accuracy(server: &BenchServer) -> Vec<BenchResult> {
    let queue = "bench-fairness-accuracy";
    let on_enqueue = r#"
        local key = msg.headers["tenant_id"] or "default"
        local w = tonumber(msg.headers["weight"]) or 1
        return { fairness_key = key, weight = w }
    "#;
    create_queue_with_lua_cli(server.host_port(), queue, Some(on_enqueue), None);

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    // Weights: tenant-1=1, tenant-2=2, tenant-3=3, tenant-4=4, tenant-5=5
    // Total weight = 15. Expected share: 1/15, 2/15, 3/15, 4/15, 5/15
    let weights: Vec<(String, u32)> = (1..=5)
        .map(|i| (format!("tenant-{i}"), i as u32))
        .collect();
    let total_weight: u32 = weights.iter().map(|(_, w)| *w).sum();
    let messages_per_key = 200;

    // Enqueue messages for all keys
    let payload = vec![0u8; PAYLOAD_SIZE];
    for (tenant, weight) in &weights {
        for _ in 0..messages_per_key {
            let headers: HashMap<String, String> = [
                ("tenant_id".to_string(), tenant.clone()),
                ("weight".to_string(), weight.to_string()),
            ]
            .into_iter()
            .collect();
            client
                .enqueue(queue, headers, payload.clone())
                .await
                .expect("enqueue");
        }
    }

    // Consume all and track delivery order per key
    let mut stream = client.consume(queue).await.expect("consume");
    let total_messages = messages_per_key * weights.len();
    let mut delivery_counts: HashMap<String, u64> = HashMap::new();

    for _ in 0..total_messages {
        if let Some(Ok(msg)) = stream.next().await {
            *delivery_counts
                .entry(msg.fairness_key.clone())
                .or_insert(0) += 1;
            client.ack(queue, &msg.id).await.expect("ack");
        }
    }

    // Calculate accuracy
    let mut results = Vec::new();
    let mut max_deviation = 0.0f64;

    for (tenant, weight) in &weights {
        let expected_share = *weight as f64 / total_weight as f64;
        let actual_count = delivery_counts.get(tenant).copied().unwrap_or(0);
        let actual_share = actual_count as f64 / total_messages as f64;
        let deviation = (actual_share - expected_share).abs() / expected_share * 100.0;
        max_deviation = max_deviation.max(deviation);

        results.push(BenchResult {
            name: format!("fairness_accuracy_{tenant}"),
            value: deviation,
            unit: "% deviation".to_string(),
            metadata: [
                ("weight".to_string(), serde_json::json!(weight)),
                ("expected_share".to_string(), serde_json::json!(expected_share)),
                ("actual_share".to_string(), serde_json::json!(actual_share)),
                ("actual_count".to_string(), serde_json::json!(actual_count)),
            ]
            .into_iter()
            .collect(),
        });
    }

    results.push(BenchResult {
        name: "fairness_accuracy_max_deviation".to_string(),
        value: max_deviation,
        unit: "% deviation".to_string(),
        metadata: [("nfr3_target".to_string(), serde_json::json!("< 5%"))]
            .into_iter()
            .collect(),
    });

    results
}

async fn measure_enqueue_throughput(server: &BenchServer, queue: &str) -> f64 {
    measure_enqueue_throughput_with_headers(server, queue, HashMap::new).await
}

async fn measure_enqueue_throughput_with_headers<F>(
    server: &BenchServer,
    queue: &str,
    headers_fn: F,
) -> f64
where
    F: Fn() -> HashMap<String, String>,
{
    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    let payload = vec![0u8; PAYLOAD_SIZE];

    // Warmup
    let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
    while tokio::time::Instant::now() < warmup_deadline {
        let _ = client
            .enqueue(queue, headers_fn(), payload.clone())
            .await;
    }

    // Measure
    let mut meter = ThroughputMeter::start();
    let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
    while tokio::time::Instant::now() < measure_deadline {
        if client
            .enqueue(queue, headers_fn(), payload.clone())
            .await
            .is_ok()
        {
            meter.increment();
        }
    }

    meter.msg_per_sec()
}
