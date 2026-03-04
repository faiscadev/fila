use crate::measurement::ThroughputMeter;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use std::time::Duration;

const PAYLOAD_SIZE: usize = 1024; // 1KB
const WARMUP_SECS: u64 = 3;
const MEASURE_SECS: u64 = 10;

/// Measure single-producer enqueue throughput with 1KB payloads.
pub async fn bench_enqueue_throughput(server: &BenchServer) -> Vec<BenchResult> {
    let queue = "bench-throughput";
    create_queue_cli(server.addr(), queue);

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    let payload = vec![0u8; PAYLOAD_SIZE];
    let headers: HashMap<String, String> = HashMap::new();

    // Warmup
    let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
    while tokio::time::Instant::now() < warmup_deadline {
        let _ = client
            .enqueue(queue, headers.clone(), payload.clone())
            .await;
    }

    // Measure
    let mut meter = ThroughputMeter::start();
    let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
    while tokio::time::Instant::now() < measure_deadline {
        if client
            .enqueue(queue, headers.clone(), payload.clone())
            .await
            .is_ok()
        {
            meter.increment();
        }
    }

    let msg_per_sec = meter.msg_per_sec();
    let mb_per_sec = msg_per_sec * PAYLOAD_SIZE as f64 / (1024.0 * 1024.0);

    vec![
        BenchResult {
            name: "enqueue_throughput_1kb".to_string(),
            value: msg_per_sec,
            unit: "msg/s".to_string(),
            metadata: [
                ("payload_size".to_string(), serde_json::json!(PAYLOAD_SIZE)),
                ("duration_secs".to_string(), serde_json::json!(MEASURE_SECS)),
                (
                    "total_messages".to_string(),
                    serde_json::json!(meter.count()),
                ),
            ]
            .into_iter()
            .collect(),
        },
        BenchResult {
            name: "enqueue_throughput_1kb_mbps".to_string(),
            value: mb_per_sec,
            unit: "MB/s".to_string(),
            metadata: HashMap::new(),
        },
    ]
}
