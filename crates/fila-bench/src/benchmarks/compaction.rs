use crate::measurement::{bench_duration_secs, LatencyHistogram, MIN_LATENCY_SAMPLES};
use crate::progress::Progress;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;
const COMPACTION_TRIGGER_MESSAGES: u64 = 10_000;

/// Measure RocksDB compaction impact on tail latency (idle vs during compaction).
///
/// Reports extended percentiles (p50 through max) for both idle and compaction phases,
/// plus the delta between compaction p99 and idle p99.
pub async fn bench_compaction_impact(server: &BenchServer) -> Vec<BenchResult> {
    let queue = "bench-compaction";
    create_queue_cli(server.addr(), queue);

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");
    let payload = vec![0u8; PAYLOAD_SIZE];
    let headers: HashMap<String, String> = HashMap::new();
    let duration = Duration::from_secs(bench_duration_secs());

    // Phase 1: Measure idle latency (clean database)
    let mut idle_histogram = LatencyHistogram::new();
    let hard_timeout = duration * 3;
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= duration && idle_histogram.count() >= MIN_LATENCY_SAMPLES {
            break;
        }
        if elapsed >= hard_timeout {
            break;
        }
        let sample_start = Instant::now();
        client
            .enqueue(queue, headers.clone(), payload.clone())
            .await
            .expect("enqueue");
        idle_histogram.record(sample_start.elapsed());
    }

    // Consume to clear
    let mut stream = client.consume(queue).await.expect("consume");
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let next = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
        match next {
            Ok(Some(Ok(msg))) => {
                client.ack(queue, &msg.id).await.expect("ack");
            }
            _ => break,
        }
        if tokio::time::Instant::now() > drain_deadline {
            break;
        }
    }
    drop(stream);

    // Phase 2: Create compaction pressure by writing+acking a large batch
    let mut enq_progress = Progress::new("compaction enqueue", COMPACTION_TRIGGER_MESSAGES);
    for _ in 0..COMPACTION_TRIGGER_MESSAGES {
        client
            .enqueue(queue, headers.clone(), payload.clone())
            .await
            .expect("enqueue");
        enq_progress.inc();
    }
    enq_progress.finish();

    // Consume and ack all (creates tombstones)
    let mut stream = client.consume(queue).await.expect("consume");
    let mut ack_progress = Progress::new("compaction consume+ack", COMPACTION_TRIGGER_MESSAGES);
    for _ in 0..COMPACTION_TRIGGER_MESSAGES {
        if let Some(Ok(msg)) = stream.next().await {
            client.ack(queue, &msg.id).await.expect("ack");
            ack_progress.inc();
        }
    }
    ack_progress.finish();
    drop(stream);

    // Phase 3: Immediately measure latency while compaction is likely active
    let mut compaction_histogram = LatencyHistogram::new();
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= duration && compaction_histogram.count() >= MIN_LATENCY_SAMPLES {
            break;
        }
        if elapsed >= hard_timeout {
            break;
        }
        let sample_start = Instant::now();
        client
            .enqueue(queue, headers.clone(), payload.clone())
            .await
            .expect("enqueue");
        compaction_histogram.record(sample_start.elapsed());
    }

    let empty_meta = HashMap::new();
    let mut results = Vec::new();

    // Emit extended percentiles for idle phase
    results.extend(crate::benchmarks::latency::emit_latency(
        &idle_histogram,
        "compaction_idle",
        "enqueue",
        &empty_meta,
    ));

    // Emit extended percentiles for compaction phase
    let compaction_meta: HashMap<String, serde_json::Value> = [(
        "trigger_messages".to_string(),
        serde_json::json!(COMPACTION_TRIGGER_MESSAGES),
    )]
    .into_iter()
    .collect();
    results.extend(crate::benchmarks::latency::emit_latency(
        &compaction_histogram,
        "compaction_active",
        "enqueue",
        &compaction_meta,
    ));

    // Legacy delta metric (p99 only) for backward compatibility with regression detection
    let idle_p99 = idle_histogram
        .percentiles()
        .map(|p| p.p99 / 1000.0)
        .unwrap_or(0.0);
    let compaction_p99 = compaction_histogram
        .percentiles()
        .map(|p| p.p99 / 1000.0)
        .unwrap_or(0.0);
    results.push(BenchResult {
        name: "compaction_p99_delta".to_string(),
        value: compaction_p99 - idle_p99,
        unit: "ms".to_string(),
        metadata: HashMap::new(),
    });

    results
}
