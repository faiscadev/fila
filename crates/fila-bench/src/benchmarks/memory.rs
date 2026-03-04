use crate::measurement::process_rss_bytes;
use crate::progress::Progress;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;

const PAYLOAD_SIZE: usize = 1024;
const MESSAGES_TO_LOAD: u64 = 100_000;

/// Measure memory footprint (RSS) of the server under load.
pub async fn bench_memory_footprint(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    let pid = server.pid().expect("server pid");

    // Measure idle RSS
    let idle_rss = process_rss_bytes(pid).unwrap_or(0);
    results.push(BenchResult {
        name: "memory_rss_idle".to_string(),
        value: idle_rss as f64 / (1024.0 * 1024.0),
        unit: "MB".to_string(),
        metadata: [("raw_bytes".to_string(), serde_json::json!(idle_rss))]
            .into_iter()
            .collect(),
    });

    // Load messages
    let queue = "bench-memory";
    create_queue_cli(server.addr(), queue);

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");
    let payload = vec![0u8; PAYLOAD_SIZE];
    let headers: HashMap<String, String> = HashMap::new();

    let mut progress = Progress::new("enqueue", MESSAGES_TO_LOAD);
    for _ in 0..MESSAGES_TO_LOAD {
        let _ = client
            .enqueue(queue, headers.clone(), payload.clone())
            .await;
        progress.inc();
    }
    progress.finish();

    // Measure loaded RSS
    // Give the server a moment to stabilize
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let loaded_rss = process_rss_bytes(pid).unwrap_or(0);
    results.push(BenchResult {
        name: "memory_rss_loaded_100k".to_string(),
        value: loaded_rss as f64 / (1024.0 * 1024.0),
        unit: "MB".to_string(),
        metadata: [
            ("raw_bytes".to_string(), serde_json::json!(loaded_rss)),
            (
                "messages_loaded".to_string(),
                serde_json::json!(MESSAGES_TO_LOAD),
            ),
        ]
        .into_iter()
        .collect(),
    });

    // Calculate per-message overhead
    let overhead_bytes = loaded_rss.saturating_sub(idle_rss);
    let per_msg_bytes = if MESSAGES_TO_LOAD > 0 {
        overhead_bytes as f64 / MESSAGES_TO_LOAD as f64
    } else {
        0.0
    };
    results.push(BenchResult {
        name: "memory_per_message_overhead".to_string(),
        value: per_msg_bytes,
        unit: "bytes/msg".to_string(),
        metadata: HashMap::new(),
    });

    results
}
