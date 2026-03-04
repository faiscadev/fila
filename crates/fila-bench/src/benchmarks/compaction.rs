use crate::measurement::LatencySampler;
use crate::progress::Progress;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;
const LATENCY_SAMPLES: usize = 1000;
const COMPACTION_TRIGGER_MESSAGES: u64 = 100_000;

/// Measure RocksDB compaction impact on tail latency (p99 idle vs during compaction).
pub async fn bench_compaction_impact(server: &BenchServer) -> Vec<BenchResult> {
    let queue = "bench-compaction";
    create_queue_cli(server.addr(), queue);

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");
    let payload = vec![0u8; PAYLOAD_SIZE];
    let headers: HashMap<String, String> = HashMap::new();

    // Phase 1: Measure idle p99 (clean database)
    let mut idle_sampler = LatencySampler::with_capacity(LATENCY_SAMPLES);
    for _ in 0..LATENCY_SAMPLES {
        let start = std::time::Instant::now();
        let _ = client
            .enqueue(queue, headers.clone(), payload.clone())
            .await;
        idle_sampler.record(start.elapsed());
    }

    // Consume to clear
    let mut stream = client.consume(queue).await.expect("consume");
    for _ in 0..LATENCY_SAMPLES {
        if let Some(Ok(msg)) = stream.next().await {
            let _ = client.ack(queue, &msg.id).await;
        }
    }
    drop(stream);

    // Phase 2: Create compaction pressure by writing+acking a large batch
    // This creates many dead entries that RocksDB needs to compact.
    let mut enq_progress = Progress::new("compaction enqueue", COMPACTION_TRIGGER_MESSAGES);
    for _ in 0..COMPACTION_TRIGGER_MESSAGES {
        let _ = client
            .enqueue(queue, headers.clone(), payload.clone())
            .await;
        enq_progress.inc();
    }
    enq_progress.finish();

    // Consume and ack all (creates tombstones)
    let mut stream = client.consume(queue).await.expect("consume");
    let mut ack_progress = Progress::new("compaction consume+ack", COMPACTION_TRIGGER_MESSAGES);
    for _ in 0..COMPACTION_TRIGGER_MESSAGES {
        if let Some(Ok(msg)) = stream.next().await {
            let _ = client.ack(queue, &msg.id).await;
            ack_progress.inc();
        }
    }
    ack_progress.finish();
    drop(stream);

    // Phase 3: Immediately measure latency while compaction is likely active
    // RocksDB triggers compaction in the background after large write+delete bursts.
    let mut compaction_sampler = LatencySampler::with_capacity(LATENCY_SAMPLES);
    for _ in 0..LATENCY_SAMPLES {
        let start = std::time::Instant::now();
        let _ = client
            .enqueue(queue, headers.clone(), payload.clone())
            .await;
        compaction_sampler.record(start.elapsed());
    }

    let idle_p99 = idle_sampler
        .percentile(0.99)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0;
    let compaction_p99 = compaction_sampler
        .percentile(0.99)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0;

    vec![
        BenchResult {
            name: "compaction_idle_p99".to_string(),
            value: idle_p99,
            unit: "ms".to_string(),
            metadata: [("samples".to_string(), serde_json::json!(LATENCY_SAMPLES))]
                .into_iter()
                .collect(),
        },
        BenchResult {
            name: "compaction_active_p99".to_string(),
            value: compaction_p99,
            unit: "ms".to_string(),
            metadata: [
                ("samples".to_string(), serde_json::json!(LATENCY_SAMPLES)),
                (
                    "trigger_messages".to_string(),
                    serde_json::json!(COMPACTION_TRIGGER_MESSAGES),
                ),
            ]
            .into_iter()
            .collect(),
        },
        BenchResult {
            name: "compaction_p99_delta".to_string(),
            value: compaction_p99 - idle_p99,
            unit: "ms".to_string(),
            metadata: HashMap::new(),
        },
    ]
}
