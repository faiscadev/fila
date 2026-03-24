//! Batch-specific benchmarks measuring batch_enqueue throughput, batch size
//! scaling, batched-vs-unbatched comparisons, delivery batching, and
//! concurrent producer batching performance.
//!
//! Gated behind `FILA_BENCH_BATCH=1`.

use crate::benchmarks::latency::emit_latency_results;
use crate::measurement::{LatencyHistogram, ThroughputMeter};
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024; // 1KB
const WARMUP_SECS: u64 = 1;
const MEASURE_SECS: u64 = 3;

// ---------------------------------------------------------------------------
// 1. BatchEnqueue throughput at varying batch sizes
// ---------------------------------------------------------------------------

/// Measure `batch_enqueue()` throughput at batch sizes 1, 10, 50, 100, 500
/// with 1KB payloads. Reports messages/s and batches/s for each size.
pub async fn bench_batch_enqueue_throughput(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for batch_size in [1u32, 10, 50, 100, 500] {
        let queue = format!("bench-batch-throughput-{batch_size}");
        create_queue_cli(server.addr(), &queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();

        // Build a reusable batch of messages
        let batch: Vec<fila_sdk::EnqueueMessage> = (0..batch_size)
            .map(|_| fila_sdk::EnqueueMessage {
                queue: queue.clone(),
                headers: headers.clone(),
                payload: payload.clone(),
            })
            .collect();

        // Warmup
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        while tokio::time::Instant::now() < warmup_deadline {
            let _ = client.batch_enqueue(batch.clone()).await;
        }

        // Measure
        let mut msg_meter = ThroughputMeter::start();
        let mut batch_meter = ThroughputMeter::start();
        let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < measure_deadline {
            if let Ok(batch_results) = client.batch_enqueue(batch.clone()).await {
                let success_count = batch_results
                    .iter()
                    .filter(|r| matches!(r, fila_sdk::BatchEnqueueResult::Success(_)))
                    .count() as u64;
                msg_meter.increment_by(success_count);
                batch_meter.increment();
            }
        }

        let msg_per_sec = msg_meter.msg_per_sec();
        let batches_per_sec = batch_meter.msg_per_sec();

        results.push(BenchResult {
            name: format!("batch_enqueue_throughput_bs{batch_size}_msgs"),
            value: msg_per_sec,
            unit: "msg/s".to_string(),
            metadata: [
                ("batch_size".to_string(), serde_json::json!(batch_size)),
                ("payload_size".to_string(), serde_json::json!(PAYLOAD_SIZE)),
                (
                    "total_messages".to_string(),
                    serde_json::json!(msg_meter.count()),
                ),
                (
                    "total_batches".to_string(),
                    serde_json::json!(batch_meter.count()),
                ),
            ]
            .into_iter()
            .collect(),
        });

        results.push(BenchResult {
            name: format!("batch_enqueue_throughput_bs{batch_size}_batches"),
            value: batches_per_sec,
            unit: "ops/s".to_string(),
            metadata: [("batch_size".to_string(), serde_json::json!(batch_size))]
                .into_iter()
                .collect(),
        });
    }

    results
}

// ---------------------------------------------------------------------------
// 2. Batch size scaling — throughput as a function of batch size
// ---------------------------------------------------------------------------

/// Measure throughput as a function of batch size (1 to 1000) to find the
/// point of diminishing returns. Reports messages/s at each batch size.
pub async fn bench_batch_size_scaling(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for batch_size in [1u32, 5, 10, 25, 50, 100, 250, 500, 1000] {
        let queue = format!("bench-batch-scaling-{batch_size}");
        create_queue_cli(server.addr(), &queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();

        let batch: Vec<fila_sdk::EnqueueMessage> = (0..batch_size)
            .map(|_| fila_sdk::EnqueueMessage {
                queue: queue.clone(),
                headers: headers.clone(),
                payload: payload.clone(),
            })
            .collect();

        // Warmup
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        while tokio::time::Instant::now() < warmup_deadline {
            let _ = client.batch_enqueue(batch.clone()).await;
        }

        // Measure
        let mut meter = ThroughputMeter::start();
        let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < measure_deadline {
            if let Ok(batch_results) = client.batch_enqueue(batch.clone()).await {
                let success_count = batch_results
                    .iter()
                    .filter(|r| matches!(r, fila_sdk::BatchEnqueueResult::Success(_)))
                    .count() as u64;
                meter.increment_by(success_count);
            }
        }

        results.push(BenchResult {
            name: format!("batch_scaling_bs{batch_size}_throughput"),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [
                ("batch_size".to_string(), serde_json::json!(batch_size)),
                (
                    "total_messages".to_string(),
                    serde_json::json!(meter.count()),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    results
}

// ---------------------------------------------------------------------------
// 3. Batched enqueue latency — end-to-end latency with batch_enqueue
// ---------------------------------------------------------------------------

/// Measure end-to-end latency (enqueue to consume) using `batch_enqueue()`
/// at varying concurrency levels (1, 10, 50 producers). Reports p50/p99
/// latency per message.
pub async fn bench_batch_latency(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for producer_count in [1u32, 10, 50] {
        let queue = format!("bench-batch-latency-{producer_count}p");
        create_queue_cli(server.addr(), &queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();
        let batch_size = 10u32;

        // Spawn background producers to create load if producer_count > 1
        let bg_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut bg_tasks = Vec::new();
        for _ in 1..producer_count {
            let bg_client = fila_sdk::FilaClient::connect(server.addr())
                .await
                .expect("connect bg");
            let bg_queue = queue.clone();
            let stop = bg_stop.clone();
            let bg_payload = payload.clone();
            let bg_headers = headers.clone();
            bg_tasks.push(tokio::spawn(async move {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let batch: Vec<fila_sdk::EnqueueMessage> = (0..batch_size)
                        .map(|_| fila_sdk::EnqueueMessage {
                            queue: bg_queue.clone(),
                            headers: bg_headers.clone(),
                            payload: bg_payload.clone(),
                        })
                        .collect();
                    let _ = bg_client.batch_enqueue(batch).await;
                }
            }));
        }

        // Let background producers run briefly to create load
        if producer_count > 1 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        // Drain any queued messages
        {
            let mut drain = client.consume(&queue).await.expect("consume drain");
            let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
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

        // Measure: send single-message batches and measure enqueue-to-consume latency
        let mut stream = client.consume(&queue).await.expect("consume stream");
        let mut histogram = LatencyHistogram::new();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);

        while Instant::now() < deadline {
            let sample_start = Instant::now();
            let batch = vec![fila_sdk::EnqueueMessage {
                queue: queue.clone(),
                headers: headers.clone(),
                payload: payload.clone(),
            }];
            let batch_results = client.batch_enqueue(batch).await;
            let enqueued_id = match batch_results {
                Ok(ref r) if !r.is_empty() => match &r[0] {
                    fila_sdk::BatchEnqueueResult::Success(id) => id.clone(),
                    _ => continue,
                },
                _ => continue,
            };

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
                _ => continue,
            }
        }

        drop(stream);
        bg_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for task in bg_tasks {
            task.abort();
        }

        let meta: HashMap<String, serde_json::Value> = [
            (
                "producer_count".to_string(),
                serde_json::json!(producer_count),
            ),
            ("batch_size".to_string(), serde_json::json!(batch_size)),
        ]
        .into_iter()
        .collect();

        results.extend(emit_latency_results(
            &histogram,
            "batch_latency",
            &format!("{producer_count}p"),
            &meta,
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// 4. Batched vs unbatched comparison
// ---------------------------------------------------------------------------

/// Run identical workloads with three modes and produce a comparison:
///   - unbatched: individual `enqueue()` calls
///   - explicit batch: `batch_enqueue()` with batch_size=50
///   - large batch: `batch_enqueue()` with batch_size=200
///
/// Reports messages/s for each mode.
pub async fn bench_batched_vs_unbatched(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let payload = vec![0u8; PAYLOAD_SIZE];
    let headers: HashMap<String, String> = HashMap::new();

    // Mode 1: Unbatched (individual enqueue calls)
    {
        let queue = "bench-batch-cmp-unbatched";
        create_queue_cli(server.addr(), queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

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

        results.push(BenchResult {
            name: "batch_cmp_unbatched_throughput".to_string(),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [(
                "total_messages".to_string(),
                serde_json::json!(meter.count()),
            )]
            .into_iter()
            .collect(),
        });
    }

    // Mode 2: Explicit batch_enqueue (batch_size=50)
    {
        let queue = "bench-batch-cmp-bs50";
        create_queue_cli(server.addr(), queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let batch: Vec<fila_sdk::EnqueueMessage> = (0..50)
            .map(|_| fila_sdk::EnqueueMessage {
                queue: queue.to_string(),
                headers: headers.clone(),
                payload: payload.clone(),
            })
            .collect();

        // Warmup
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        while tokio::time::Instant::now() < warmup_deadline {
            let _ = client.batch_enqueue(batch.clone()).await;
        }

        // Measure
        let mut meter = ThroughputMeter::start();
        let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < measure_deadline {
            if let Ok(batch_results) = client.batch_enqueue(batch.clone()).await {
                let success_count = batch_results
                    .iter()
                    .filter(|r| matches!(r, fila_sdk::BatchEnqueueResult::Success(_)))
                    .count() as u64;
                meter.increment_by(success_count);
            }
        }

        results.push(BenchResult {
            name: "batch_cmp_explicit_bs50_throughput".to_string(),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [
                ("batch_size".to_string(), serde_json::json!(50)),
                (
                    "total_messages".to_string(),
                    serde_json::json!(meter.count()),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    // Mode 3: Explicit batch_enqueue (batch_size=200)
    {
        let queue = "bench-batch-cmp-bs200";
        create_queue_cli(server.addr(), queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        let batch: Vec<fila_sdk::EnqueueMessage> = (0..200)
            .map(|_| fila_sdk::EnqueueMessage {
                queue: queue.to_string(),
                headers: headers.clone(),
                payload: payload.clone(),
            })
            .collect();

        // Warmup
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        while tokio::time::Instant::now() < warmup_deadline {
            let _ = client.batch_enqueue(batch.clone()).await;
        }

        // Measure
        let mut meter = ThroughputMeter::start();
        let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < measure_deadline {
            if let Ok(batch_results) = client.batch_enqueue(batch.clone()).await {
                let success_count = batch_results
                    .iter()
                    .filter(|r| matches!(r, fila_sdk::BatchEnqueueResult::Success(_)))
                    .count() as u64;
                meter.increment_by(success_count);
            }
        }

        results.push(BenchResult {
            name: "batch_cmp_explicit_bs200_throughput".to_string(),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [
                ("batch_size".to_string(), serde_json::json!(200)),
                (
                    "total_messages".to_string(),
                    serde_json::json!(meter.count()),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    results
}

// ---------------------------------------------------------------------------
// 5. Delivery batching throughput — consumer throughput with varying counts
// ---------------------------------------------------------------------------

/// Measure consumer throughput with varying consumer counts (1, 10, 100)
/// when messages were enqueued using batch_enqueue. Pre-loads messages
/// with batch_enqueue then measures consume+ack throughput.
pub async fn bench_delivery_batching_throughput(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for consumer_count in [1u32, 10, 100] {
        let queue = format!("bench-batch-delivery-{consumer_count}c");
        create_queue_cli(server.addr(), &queue);

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();

        // Pre-load messages using batch_enqueue
        let pre_load: u64 = 10_000;
        let batch_size = 100u32;
        let batches_needed = pre_load / batch_size as u64;
        for _ in 0..batches_needed {
            let batch: Vec<fila_sdk::EnqueueMessage> = (0..batch_size)
                .map(|_| fila_sdk::EnqueueMessage {
                    queue: queue.clone(),
                    headers: headers.clone(),
                    payload: payload.clone(),
                })
                .collect();
            client
                .batch_enqueue(batch)
                .await
                .expect("batch_enqueue pre-load");
        }

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
                let batch: Vec<fila_sdk::EnqueueMessage> = (0..100)
                    .map(|_| fila_sdk::EnqueueMessage {
                        queue: producer_queue.clone(),
                        headers: headers.clone(),
                        payload: payload.clone(),
                    })
                    .collect();
                let _ = producer_client.batch_enqueue(batch).await;
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
            name: format!("batch_delivery_{consumer_count}c_throughput"),
            value: throughput,
            unit: "msg/s".to_string(),
            metadata: [
                (
                    "consumer_count".to_string(),
                    serde_json::json!(consumer_count),
                ),
                (
                    "total_consumed".to_string(),
                    serde_json::json!(consumed),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    results
}

// ---------------------------------------------------------------------------
// 6. Concurrent producer batching
// ---------------------------------------------------------------------------

/// Measure aggregate throughput with 1, 5, 10, 50 concurrent producers
/// each using `batch_enqueue()` with batch_size=50.
pub async fn bench_concurrent_producer_batching(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();

    for producer_count in [1u32, 5, 10, 50] {
        let queue = format!("bench-batch-concurrent-{producer_count}p");
        create_queue_cli(server.addr(), &queue);

        let total_messages = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let batch_size = 50u32;

        // Warmup with a single producer
        let warmup_client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect warmup");
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        let payload = vec![0u8; PAYLOAD_SIZE];
        let headers: HashMap<String, String> = HashMap::new();
        while tokio::time::Instant::now() < warmup_deadline {
            let batch: Vec<fila_sdk::EnqueueMessage> = (0..batch_size)
                .map(|_| fila_sdk::EnqueueMessage {
                    queue: queue.clone(),
                    headers: headers.clone(),
                    payload: payload.clone(),
                })
                .collect();
            let _ = warmup_client.batch_enqueue(batch).await;
        }
        drop(warmup_client);

        // Spawn producers
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = Vec::new();

        for _ in 0..producer_count {
            let c = fila_sdk::FilaClient::connect(server.addr())
                .await
                .expect("connect producer");
            let q = queue.clone();
            let s = stop.clone();
            let tm = total_messages.clone();
            handles.push(tokio::spawn(async move {
                let payload = vec![0u8; PAYLOAD_SIZE];
                let headers: HashMap<String, String> = HashMap::new();
                while !s.load(std::sync::atomic::Ordering::Relaxed) {
                    let batch: Vec<fila_sdk::EnqueueMessage> = (0..batch_size)
                        .map(|_| fila_sdk::EnqueueMessage {
                            queue: q.clone(),
                            headers: headers.clone(),
                            payload: payload.clone(),
                        })
                        .collect();
                    if let Ok(batch_results) = c.batch_enqueue(batch).await {
                        let success_count = batch_results
                            .iter()
                            .filter(|r| matches!(r, fila_sdk::BatchEnqueueResult::Success(_)))
                            .count() as u64;
                        tm.fetch_add(success_count, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }));
        }

        // Measure for MEASURE_SECS
        let before = total_messages.load(std::sync::atomic::Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(MEASURE_SECS)).await;
        let after = total_messages.load(std::sync::atomic::Ordering::Relaxed);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for h in handles {
            h.abort();
        }

        let messages = after - before;
        let throughput = messages as f64 / MEASURE_SECS as f64;

        results.push(BenchResult {
            name: format!("batch_concurrent_{producer_count}p_throughput"),
            value: throughput,
            unit: "msg/s".to_string(),
            metadata: [
                (
                    "producer_count".to_string(),
                    serde_json::json!(producer_count),
                ),
                ("batch_size".to_string(), serde_json::json!(batch_size)),
                (
                    "total_messages".to_string(),
                    serde_json::json!(messages),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    results
}
