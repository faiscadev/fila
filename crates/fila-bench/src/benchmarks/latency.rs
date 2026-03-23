use crate::measurement::{bench_duration_secs, LatencyHistogram, MIN_LATENCY_SAMPLES};
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;

struct LoadLevel {
    name: &'static str,
    producers: usize,
}

const LOAD_LEVELS: &[LoadLevel] = &[
    LoadLevel {
        name: "light",
        producers: 1,
    },
    LoadLevel {
        name: "moderate",
        producers: 5,
    },
    LoadLevel {
        name: "saturated",
        producers: 20,
    },
];

/// Emit all 6 percentile metrics for a latency histogram.
pub fn emit_latency_results(
    histogram: &LatencyHistogram,
    prefix: &str,
    suffix: &str,
    extra_metadata: &HashMap<String, serde_json::Value>,
) -> Vec<BenchResult> {
    let Some(pcts) = histogram.percentiles() else {
        return Vec::new();
    };

    let mut meta = extra_metadata.clone();
    meta.insert(
        "samples".to_string(),
        serde_json::json!(histogram.count()),
    );
    meta.insert(
        "histogram".to_string(),
        serde_json::json!(histogram.serialize_base64()),
    );

    let percentiles = [
        ("p50", pcts.p50),
        ("p95", pcts.p95),
        ("p99", pcts.p99),
        ("p99_9", pcts.p99_9),
        ("p99_99", pcts.p99_99),
        ("max", pcts.max),
    ];

    percentiles
        .into_iter()
        .map(|(label, value_us)| BenchResult {
            name: format!("{prefix}_{label}_{suffix}"),
            value: value_us / 1000.0, // microseconds → milliseconds
            unit: "ms".to_string(),
            metadata: meta.clone(),
        })
        .collect()
}

/// Measure enqueue-to-consume latency at p50/p95/p99/p99.9/p99.99/max under varying load.
///
/// Approach: first pre-load background messages to simulate queue pressure from
/// N producers, then measure sequential enqueue-consume round-trip latency.
/// Collects samples for a configurable duration (default 30s, min 10K samples).
pub async fn bench_e2e_latency(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let duration = Duration::from_secs(bench_duration_secs());

    for level in LOAD_LEVELS {
        let queue = format!("bench-latency-{}", level.name);
        create_queue_cli(server.addr(), &queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();

        // Phase 1: Pre-load background pressure.
        if level.producers > 1 {
            let bg_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut bg_tasks = Vec::new();
            for _ in 1..level.producers {
                let bg_client = fila_sdk::FilaClient::connect(server.addr())
                    .await
                    .expect("connect bg");
                let bg_queue = queue.clone();
                let stop = bg_stop.clone();
                bg_tasks.push(tokio::spawn(async move {
                    let payload = vec![0u8; PAYLOAD_SIZE];
                    let headers: HashMap<String, String> = HashMap::new();
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = bg_client
                            .enqueue(&bg_queue, headers.clone(), payload.clone())
                            .await;
                    }
                }));
            }

            // Let background producers run for 3 seconds to create load
            tokio::time::sleep(Duration::from_secs(3)).await;
            bg_stop.store(true, std::sync::atomic::Ordering::Relaxed);
            for task in bg_tasks {
                let _ = task.await;
            }
        }

        // Phase 2: Drain all queued messages so we start measurement clean.
        {
            let mut drain = client.consume(&queue).await.expect("consume drain");
            let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let next = tokio::time::timeout(Duration::from_millis(200), drain.next()).await;
                match next {
                    Ok(Some(Ok(msg))) => {
                        let _ = client.ack(&queue, &msg.id).await;
                    }
                    _ => break,
                }
                if tokio::time::Instant::now() > drain_deadline {
                    break;
                }
            }
            drop(drain);
        }

        // Phase 3: Measure sequential enqueue-consume round-trip latency.
        // Run for `duration` and collect at least MIN_LATENCY_SAMPLES.
        let mut stream = client.consume(&queue).await.expect("consume stream");
        let mut histogram = LatencyHistogram::new();
        let start = Instant::now();

        loop {
            let elapsed = start.elapsed();
            if elapsed >= duration && histogram.count() >= MIN_LATENCY_SAMPLES {
                break;
            }

            let sample_start = Instant::now();
            let enqueued_id = client
                .enqueue(&queue, headers.clone(), payload.clone())
                .await
                .expect("enqueue");

            let next = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
            match next {
                Ok(Some(Ok(msg))) if msg.id == enqueued_id => {
                    histogram.record(sample_start.elapsed());
                    let _ = client.ack(&queue, &msg.id).await;
                }
                Ok(Some(Ok(msg))) => {
                    let _ = client.ack(&queue, &msg.id).await;
                    continue;
                }
                _ => {
                    continue;
                }
            }
        }

        drop(stream);

        let meta: HashMap<String, serde_json::Value> = [(
            "producers".to_string(),
            serde_json::json!(level.producers),
        )]
        .into_iter()
        .collect();

        results.extend(emit_latency_results(
            &histogram,
            "e2e_latency",
            level.name,
            &meta,
        ));
    }

    results
}

/// Helper to emit latency results — re-exported for use by other benchmark modules.
pub use emit_latency_results as emit_latency;
