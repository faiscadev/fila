use crate::measurement::LatencySampler;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;
const SAMPLES_PER_LEVEL: usize = 100;

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

/// Measure enqueue-to-consume latency at p50/p95/p99 under varying load.
///
/// Approach: first pre-load background messages to simulate queue pressure from
/// N producers, then measure sequential enqueue-consume round-trip latency.
/// This avoids the O(N) scan problem of searching for a specific message among
/// thousands of background messages.
pub async fn bench_e2e_latency(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for level in LOAD_LEVELS {
        let queue = format!("bench-latency-{}", level.name);
        create_queue_cli(server.grpc_addr(), &queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();

        // Phase 1: Pre-load background pressure.
        // For moderate/saturated, flood the queue with messages from N-1
        // background producers for 3 seconds to create realistic server load.
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
                    _ => break, // timeout or stream end — queue is drained
                }
                if tokio::time::Instant::now() > drain_deadline {
                    break;
                }
            }
            drop(drain);
        }

        // Phase 3: Measure sequential enqueue-consume round-trip latency.
        // Each sample: enqueue one message, consume the next available, record time.
        // Since the queue is drained, the next consumed message IS the one we enqueued.
        let mut stream = client.consume(&queue).await.expect("consume stream");
        let mut sampler = LatencySampler::with_capacity(SAMPLES_PER_LEVEL);

        for _ in 0..SAMPLES_PER_LEVEL {
            let start = Instant::now();
            let enqueued_id = client
                .enqueue(&queue, headers.clone(), payload.clone())
                .await
                .expect("enqueue");

            // Consume the next message and verify it's the one we just enqueued
            let next = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
            match next {
                Ok(Some(Ok(msg))) if msg.id == enqueued_id => {
                    sampler.record(start.elapsed());
                    let _ = client.ack(&queue, &msg.id).await;
                }
                Ok(Some(Ok(msg))) => {
                    // Wrong message — ack it and skip this sample
                    let _ = client.ack(&queue, &msg.id).await;
                    continue;
                }
                _ => {
                    // Timeout or error — skip this sample
                    continue;
                }
            }
        }

        drop(stream);

        if let Some((p50, p95, p99)) = sampler.percentiles() {
            let meta: HashMap<String, serde_json::Value> = [
                ("producers".to_string(), serde_json::json!(level.producers)),
                ("samples".to_string(), serde_json::json!(sampler.count())),
            ]
            .into_iter()
            .collect();

            results.push(BenchResult {
                name: format!("e2e_latency_p50_{}", level.name),
                value: p50.as_secs_f64() * 1000.0,
                unit: "ms".to_string(),
                metadata: meta.clone(),
            });
            results.push(BenchResult {
                name: format!("e2e_latency_p95_{}", level.name),
                value: p95.as_secs_f64() * 1000.0,
                unit: "ms".to_string(),
                metadata: meta.clone(),
            });
            results.push(BenchResult {
                name: format!("e2e_latency_p99_{}", level.name),
                value: p99.as_secs_f64() * 1000.0,
                unit: "ms".to_string(),
                metadata: meta,
            });
        }
    }

    results
}
