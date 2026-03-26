use crate::measurement::ThroughputMeter;
use crate::progress::Progress;
use crate::report::BenchResult;
use crate::server::{create_queue_with_lua_cli, BenchServer};
use std::collections::HashMap;
use std::time::Duration;
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;
const MEASURE_SECS: u64 = 3;

/// Measure enqueue/consume throughput at 1M and 10M queued messages (queue depth scaling).
pub async fn bench_queue_depth_scaling(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for depth in [1_000_000u64, 10_000_000] {
        let depth_label = if depth == 1_000_000 { "1m" } else { "10m" };
        let queue = format!("bench-depth-{depth_label}");
        create_queue_with_lua_cli(server.addr(), &queue, None, None).await;

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();

        // Pre-load messages
        let mut progress = Progress::new(&format!("depth {depth_label} pre-load"), depth);
        for _ in 0..depth {
            client
                .enqueue(&queue, headers.clone(), payload.clone())
                .await
                .expect("enqueue");
            progress.inc();
        }
        progress.finish();

        // Measure enqueue throughput with full queue
        let mut meter = ThroughputMeter::start();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < deadline {
            if client
                .enqueue(&queue, headers.clone(), payload.clone())
                .await
                .is_ok()
            {
                meter.increment();
            }
        }

        results.push(BenchResult {
            name: format!("queue_depth_{depth_label}_enqueue_throughput"),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [("queue_depth".to_string(), serde_json::json!(depth))]
                .into_iter()
                .collect(),
        });

        // Measure consume throughput with full queue (count only successful acks)
        let mut stream = client.consume(&queue).await.expect("consume");
        let mut consume_meter = ThroughputMeter::start();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < deadline {
            if let Some(Ok(msg)) = stream.next().await {
                if client.ack(&queue, &msg.id).await.is_ok() {
                    consume_meter.increment();
                }
            }
        }

        results.push(BenchResult {
            name: format!("queue_depth_{depth_label}_consume_throughput"),
            value: consume_meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [("queue_depth".to_string(), serde_json::json!(depth))]
                .into_iter()
                .collect(),
        });
    }

    results
}

/// Measure fairness key cardinality impact: 10, 1K, 10K, 100K active keys.
pub async fn bench_key_cardinality(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for key_count in [10u64, 1_000, 10_000] {
        let key_label = match key_count {
            10 => "10",
            1_000 => "1k",
            10_000 => "10k",
            _ => unreachable!(),
        };
        let queue = format!("bench-cardinality-{key_label}");
        let on_enqueue = r#"function on_enqueue(msg) local key = msg.headers["fk"] or "default" return { fairness_key = key, weight = 1, throttle_keys = {} } end"#;
        create_queue_with_lua_cli(server.addr(), &queue, Some(on_enqueue), None).await;

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let payload = vec![0u8; PAYLOAD_SIZE];

        // Enqueue with rotating keys
        let mut meter = ThroughputMeter::start();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        let mut key_idx = 0u64;
        while tokio::time::Instant::now() < deadline {
            let headers: HashMap<String, String> =
                [("fk".to_string(), format!("key-{}", key_idx % key_count))]
                    .into_iter()
                    .collect();
            if client
                .enqueue(&queue, headers, payload.clone())
                .await
                .is_ok()
            {
                meter.increment();
            }
            key_idx += 1;
        }

        results.push(BenchResult {
            name: format!("key_cardinality_{key_label}_throughput"),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [("key_count".to_string(), serde_json::json!(key_count))]
                .into_iter()
                .collect(),
        });
    }

    results
}

/// Measure consumer concurrency impact: 1, 10, and 100 simultaneous consumers.
pub async fn bench_consumer_concurrency(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for consumer_count in [1u32, 10, 100] {
        let queue = format!("bench-concurrency-{consumer_count}");
        create_queue_with_lua_cli(server.addr(), &queue, None, None).await;

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();

        // Pre-load messages so consumers have work
        let pre_load: u64 = 10_000;
        let mut progress =
            Progress::new(&format!("concurrency {consumer_count} pre-load"), pre_load);
        for _ in 0..pre_load {
            client
                .enqueue(&queue, headers.clone(), payload.clone())
                .await
                .expect("enqueue");
            progress.inc();
        }
        progress.finish();

        // Spawn consumers
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let total_consumed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut handles = Vec::new();

        for _ in 0..consumer_count {
            let c = fila_sdk::FilaClient::connect(server.addr())
                .await
                .expect("connect consumer");
            let q = queue.clone();
            let s = stop.clone();
            let tc = total_consumed.clone();
            handles.push(tokio::spawn(async move {
                if let Ok(mut stream) = c.consume(&q).await {
                    while !s.load(std::sync::atomic::Ordering::Relaxed) {
                        match stream.next().await {
                            Some(Ok(msg)) => {
                                if c.ack(&q, &msg.id).await.is_ok() {
                                    tc.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                            _ => break,
                        }
                    }
                }
            }));
        }

        // Also keep producing so consumers don't starve
        let producer_stop = stop.clone();
        let producer_client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect producer");
        let producer_queue = queue.clone();
        let producer = tokio::spawn(async move {
            let payload = vec![0u8; PAYLOAD_SIZE];
            let headers: HashMap<String, String> = HashMap::new();
            while !producer_stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = producer_client
                    .enqueue(&producer_queue, headers.clone(), payload.clone())
                    .await;
            }
        });

        // Measure for MEASURE_SECS
        let before = total_consumed.load(std::sync::atomic::Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(MEASURE_SECS)).await;
        let after = total_consumed.load(std::sync::atomic::Ordering::Relaxed);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        producer.abort();
        for h in handles {
            h.abort();
        }

        let consumed = after - before;
        let throughput = consumed as f64 / MEASURE_SECS as f64;

        results.push(BenchResult {
            name: format!("consumer_concurrency_{consumer_count}_throughput"),
            value: throughput,
            unit: "msg/s".to_string(),
            metadata: [(
                "consumer_count".to_string(),
                serde_json::json!(consumer_count),
            )]
            .into_iter()
            .collect(),
        });
    }

    results
}
