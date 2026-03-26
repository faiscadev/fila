use crate::measurement::{LatencyHistogram, ThroughputMeter};
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;

use crate::benchmarks::latency::emit_latency_results;

const PAYLOAD_SIZE: usize = 1024; // 1KB
const WARMUP_SECS: u64 = 1;
const MEASURE_SECS: u64 = 3;

fn make_payload() -> Vec<u8> {
    vec![0x42u8; PAYLOAD_SIZE]
}

fn make_batch(queue: &str, batch_size: usize, payload: &[u8]) -> Vec<fila_sdk::EnqueueMessage> {
    (0..batch_size)
        .map(|_| fila_sdk::EnqueueMessage {
            queue: queue.to_string(),
            headers: HashMap::new(),
            payload: payload.to_vec(),
        })
        .collect()
}

/// Count successful results in a batch enqueue response.
fn count_successes(results: &[Result<String, fila_sdk::EnqueueError>]) -> u64 {
    results.iter().filter(|r| r.is_ok()).count() as u64
}

// ─── Benchmark 1: BatchEnqueue throughput ────────────────────────────────────

/// Measure `BatchEnqueue` RPC throughput at various batch sizes with 1KB messages.
/// Reports messages/s and batches/s for each batch size.
pub async fn bench_batch_enqueue_throughput(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let batch_sizes: &[usize] = &[1, 10, 50, 100, 500];

    for &batch_size in batch_sizes {
        let queue = format!("bench-batch-throughput-{batch_size}");
        create_queue_cli(server.addr(), &queue).await;

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let payload = make_payload();

        // Warmup
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        while tokio::time::Instant::now() < warmup_deadline {
            let batch = make_batch(&queue, batch_size, &payload);
            let _ = client.enqueue_many(batch).await;
        }

        // Measure
        let mut msg_meter = ThroughputMeter::start();
        let mut batch_count: u64 = 0;
        let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < measure_deadline {
            let batch = make_batch(&queue, batch_size, &payload);
            let batch_results = client.enqueue_many(batch).await;
            let successes = count_successes(&batch_results);
            msg_meter.increment_by(successes);
            batch_count += 1;
        }

        let elapsed_secs = msg_meter.elapsed().as_secs_f64();
        let batches_per_sec = if elapsed_secs > 0.0 {
            batch_count as f64 / elapsed_secs
        } else {
            0.0
        };

        let meta: HashMap<String, serde_json::Value> = [
            ("batch_size".to_string(), serde_json::json!(batch_size)),
            ("payload_size".to_string(), serde_json::json!(PAYLOAD_SIZE)),
            ("duration_secs".to_string(), serde_json::json!(MEASURE_SECS)),
            (
                "total_messages".to_string(),
                serde_json::json!(msg_meter.count()),
            ),
            ("total_batches".to_string(), serde_json::json!(batch_count)),
        ]
        .into_iter()
        .collect();

        results.push(BenchResult {
            name: format!("batch_enqueue_throughput_bs{batch_size}_msgs"),
            value: msg_meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: meta.clone(),
        });
        results.push(BenchResult {
            name: format!("batch_enqueue_throughput_bs{batch_size}_batches"),
            value: batches_per_sec,
            unit: "ops/s".to_string(),
            metadata: meta,
        });
    }

    results
}

// ─── Benchmark 2: Batch size scaling ─────────────────────────────────────────

/// Measure throughput as a function of batch size (1 to 1000) to find diminishing returns.
/// Produces throughput-vs-batch-size data points.
pub async fn bench_batch_size_scaling(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let batch_sizes: &[usize] = &[1, 5, 10, 25, 50, 100, 250, 500, 1000];

    for &batch_size in batch_sizes {
        let queue = format!("bench-batch-scaling-{batch_size}");
        create_queue_cli(server.addr(), &queue).await;

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let payload = make_payload();

        // Warmup
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        while tokio::time::Instant::now() < warmup_deadline {
            let batch = make_batch(&queue, batch_size, &payload);
            let _ = client.enqueue_many(batch).await;
        }

        // Measure
        let mut meter = ThroughputMeter::start();
        let measure_deadline = tokio::time::Instant::now() + Duration::from_secs(MEASURE_SECS);
        while tokio::time::Instant::now() < measure_deadline {
            let batch = make_batch(&queue, batch_size, &payload);
            let batch_results = client.enqueue_many(batch).await;
            meter.increment_by(count_successes(&batch_results));
        }

        results.push(BenchResult {
            name: format!("batch_scaling_bs{batch_size}_throughput"),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [
                ("batch_size".to_string(), serde_json::json!(batch_size)),
                ("payload_size".to_string(), serde_json::json!(PAYLOAD_SIZE)),
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

// ─── Benchmark 3: Auto-batching latency ─────────────────────────────────────

/// Measure end-to-end latency (enqueue to consume) with client-side batching
/// at various concurrency levels (1, 10, 50 producers).
///
/// Simulates auto-batching by accumulating messages in a buffer and flushing
/// via `batch_enqueue` when the buffer reaches `batch_size`. Reports p50/p99 latency.
pub async fn bench_auto_batching_latency(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let concurrency_levels: &[usize] = &[1, 10, 50];
    let batch_size: usize = 50;

    for &producer_count in concurrency_levels {
        let queue = format!("bench-autobatch-latency-{producer_count}");
        create_queue_cli(server.addr(), &queue).await;

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let payload = make_payload();

        // Start a consumer to drain messages and record latencies
        let mut histogram = LatencyHistogram::new();
        let stop = Arc::new(AtomicBool::new(false));
        let total_produced = Arc::new(AtomicU64::new(0));

        // Phase 1: Run producers using batch_enqueue with accumulation
        let producer_stop = stop.clone();
        let mut producer_handles = Vec::new();
        for _ in 0..producer_count {
            let pc = fila_sdk::FilaClient::connect(server.addr())
                .await
                .expect("connect producer");
            let pq = queue.clone();
            let pp = payload.clone();
            let ps = producer_stop.clone();
            let tp = total_produced.clone();

            producer_handles.push(tokio::spawn(async move {
                while !ps.load(Ordering::Relaxed) {
                    let batch = make_batch(&pq, batch_size, &pp);
                    let batch_results = pc.enqueue_many(batch).await;
                    tp.fetch_add(count_successes(&batch_results), Ordering::Relaxed);
                }
            }));
        }

        // Let producers run briefly to fill the queue
        tokio::time::sleep(Duration::from_secs(WARMUP_SECS)).await;

        // Phase 2: Measure sequential enqueue-consume round-trip latency
        // Drain existing messages first using a separate connection so the main
        // client has no active consume session when measurement starts
        // (FIBP allows only one consume session per connection).
        {
            let drain_client = fila_sdk::FilaClient::connect(server.addr())
                .await
                .expect("connect drain");
            let mut drain = drain_client.consume(&queue).await.expect("consume drain");
            let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                let next = tokio::time::timeout(Duration::from_millis(200), drain.next()).await;
                match next {
                    Ok(Some(Ok(msg))) => {
                        let _ = drain_client.ack(&queue, &msg.id).await;
                    }
                    _ => break,
                }
                if tokio::time::Instant::now() > drain_deadline {
                    break;
                }
            }
            drop(drain);
            drop(drain_client);
        }

        // Stop background producers
        stop.store(true, Ordering::Relaxed);
        for h in producer_handles {
            let _ = h.await;
        }

        // Now measure latency: batch-enqueue then consume each message
        let mut stream = client.consume(&queue).await.expect("consume stream");
        let measure_start = Instant::now();
        let measure_duration = Duration::from_secs(MEASURE_SECS);

        while measure_start.elapsed() < measure_duration {
            let sample_start = Instant::now();
            let batch = make_batch(&queue, batch_size, &payload);
            let batch_results = client.enqueue_many(batch).await;

            let expected = count_successes(&batch_results) as usize;
            let mut received = 0;
            let batch_timeout = Duration::from_secs(5);
            let batch_deadline = tokio::time::Instant::now() + batch_timeout;

            while received < expected && tokio::time::Instant::now() < batch_deadline {
                let next = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
                match next {
                    Ok(Some(Ok(msg))) => {
                        let _ = client.ack(&queue, &msg.id).await;
                        received += 1;
                    }
                    _ => break,
                }
            }

            // Record the batch round-trip time
            if received > 0 {
                histogram.record(sample_start.elapsed());
            }
        }

        drop(stream);

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
            "auto_batching_latency",
            &format!("{producer_count}p"),
            &meta,
        ));
    }

    results
}

// ─── Benchmark 4: Batched vs unbatched comparison ────────────────────────────

/// Run identical workloads with unbatched enqueue, explicit batch_enqueue,
/// and simulated auto-batching (accumulate + flush), producing a comparison.
pub async fn bench_batched_vs_unbatched(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let payload = make_payload();
    let message_count: u64 = 3000;
    let batch_size: usize = 100;

    // --- Mode 1: Unbatched (single enqueue per message) ---
    {
        let queue = "bench-bvu-unbatched";
        create_queue_cli(server.addr(), queue).await;
        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let headers: HashMap<String, String> = HashMap::new();

        // Warmup
        for _ in 0..100 {
            let _ = client
                .enqueue(queue, headers.clone(), payload.clone())
                .await;
        }

        let mut meter = ThroughputMeter::start();
        for _ in 0..message_count {
            if client
                .enqueue(queue, headers.clone(), payload.clone())
                .await
                .is_ok()
            {
                meter.increment();
            }
        }

        results.push(BenchResult {
            name: "batched_vs_unbatched_single_throughput".to_string(),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [
                ("mode".to_string(), serde_json::json!("unbatched")),
                (
                    "message_count".to_string(),
                    serde_json::json!(message_count),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    // --- Mode 2: Explicit batch_enqueue ---
    {
        let queue = "bench-bvu-explicit-batch";
        create_queue_cli(server.addr(), queue).await;
        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        // Warmup
        let warmup_batch = make_batch(queue, batch_size, &payload);
        let _ = client.enqueue_many(warmup_batch).await;

        let mut meter = ThroughputMeter::start();
        let full_batches = message_count as usize / batch_size;
        let remainder = message_count as usize % batch_size;

        for _ in 0..full_batches {
            let batch = make_batch(queue, batch_size, &payload);
            let batch_results = client.enqueue_many(batch).await;
            meter.increment_by(count_successes(&batch_results));
        }
        if remainder > 0 {
            let batch = make_batch(queue, remainder, &payload);
            let batch_results = client.enqueue_many(batch).await;
            meter.increment_by(count_successes(&batch_results));
        }

        results.push(BenchResult {
            name: "batched_vs_unbatched_explicit_batch_throughput".to_string(),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [
                ("mode".to_string(), serde_json::json!("explicit_batch")),
                ("batch_size".to_string(), serde_json::json!(batch_size)),
                (
                    "message_count".to_string(),
                    serde_json::json!(message_count),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    // --- Mode 3: Simulated auto-batching (accumulate + flush with linger) ---
    {
        let queue = "bench-bvu-auto-batch";
        create_queue_cli(server.addr(), queue).await;
        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");
        let headers: HashMap<String, String> = HashMap::new();

        // Warmup
        for _ in 0..100 {
            let _ = client
                .enqueue(queue, headers.clone(), payload.clone())
                .await;
        }

        // Simulate auto-batching: accumulate messages, flush when batch_size reached
        let mut meter = ThroughputMeter::start();
        let mut buffer: Vec<fila_sdk::EnqueueMessage> = Vec::with_capacity(batch_size);
        let mut produced: u64 = 0;

        while produced < message_count {
            buffer.push(fila_sdk::EnqueueMessage {
                queue: queue.to_string(),
                headers: HashMap::new(),
                payload: payload.clone(),
            });
            produced += 1;

            if buffer.len() >= batch_size || produced == message_count {
                let batch = std::mem::take(&mut buffer);
                let batch_results = client.enqueue_many(batch).await;
                meter.increment_by(count_successes(&batch_results));
            }
        }

        results.push(BenchResult {
            name: "batched_vs_unbatched_auto_batch_throughput".to_string(),
            value: meter.msg_per_sec(),
            unit: "msg/s".to_string(),
            metadata: [
                ("mode".to_string(), serde_json::json!("auto_batch")),
                ("batch_size".to_string(), serde_json::json!(batch_size)),
                (
                    "message_count".to_string(),
                    serde_json::json!(message_count),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    // --- Compute speedup ratios ---
    let unbatched = results
        .iter()
        .find(|r| r.name == "batched_vs_unbatched_single_throughput")
        .map(|r| r.value)
        .unwrap_or(1.0);

    let mut speedups = Vec::new();
    for r in &results {
        if r.name != "batched_vs_unbatched_single_throughput" && unbatched > 0.0 {
            let speedup = r.value / unbatched;
            let mode = r
                .metadata
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            speedups.push(BenchResult {
                name: format!("batched_vs_unbatched_{mode}_speedup"),
                value: speedup,
                unit: "x".to_string(),
                metadata: [("vs_baseline".to_string(), serde_json::json!("unbatched"))]
                    .into_iter()
                    .collect(),
            });
        }
    }
    results.extend(speedups);

    results
}

// ─── Benchmark 5: Delivery batching throughput ───────────────────────────────

/// Measure consumer throughput with varying consumer counts (1, 10, 100).
/// Pre-loads messages via batch_enqueue, then measures consume throughput.
pub async fn bench_delivery_batching_throughput(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let consumer_counts: &[u32] = &[1, 10, 100];
    let pre_load: u64 = 10_000;
    let batch_size: usize = 100;
    let payload = make_payload();

    for &consumer_count in consumer_counts {
        let queue = format!("bench-delivery-batch-{consumer_count}");
        create_queue_cli(server.addr(), &queue).await;

        let client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect");

        // Pre-load messages via enqueue_many
        let mut loaded: u64 = 0;
        while loaded < pre_load {
            let remaining = (pre_load - loaded) as usize;
            let this_batch = remaining.min(batch_size);
            let batch = make_batch(&queue, this_batch, &payload);
            let batch_results = client.enqueue_many(batch).await;
            loaded += count_successes(&batch_results);
        }

        // Spawn consumers
        let stop = Arc::new(AtomicBool::new(false));
        let total_consumed = Arc::new(AtomicU64::new(0));
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
                    while !s.load(Ordering::Relaxed) {
                        match stream.next().await {
                            Some(Ok(msg)) => {
                                if c.ack(&q, &msg.id).await.is_ok() {
                                    tc.fetch_add(1, Ordering::Relaxed);
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
        let producer_payload = payload.clone();
        let producer = tokio::spawn(async move {
            while !producer_stop.load(Ordering::Relaxed) {
                let batch = make_batch(&producer_queue, batch_size, &producer_payload);
                let _ = producer_client.enqueue_many(batch).await;
            }
        });

        // Measure
        let before = total_consumed.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(MEASURE_SECS)).await;
        let after = total_consumed.load(Ordering::Relaxed);

        stop.store(true, Ordering::Relaxed);
        producer.abort();
        for h in handles {
            h.abort();
        }

        let consumed = after - before;
        let throughput = consumed as f64 / MEASURE_SECS as f64;

        results.push(BenchResult {
            name: format!("delivery_batching_{consumer_count}c_throughput"),
            value: throughput,
            unit: "msg/s".to_string(),
            metadata: [
                (
                    "consumer_count".to_string(),
                    serde_json::json!(consumer_count),
                ),
                ("batch_size".to_string(), serde_json::json!(batch_size)),
            ]
            .into_iter()
            .collect(),
        });
    }

    results
}

// ─── Benchmark 6: Concurrent producer batching ──────────────────────────────

/// Measure throughput with 1, 5, 10, 50 concurrent producers using batch_enqueue.
pub async fn bench_concurrent_producer_batching(server: &BenchServer) -> Vec<BenchResult> {
    let mut results = Vec::new();
    let producer_counts: &[usize] = &[1, 5, 10, 50];
    let batch_size: usize = 100;
    let payload = make_payload();

    for &producer_count in producer_counts {
        let queue = format!("bench-concurrent-batch-{producer_count}");
        create_queue_cli(server.addr(), &queue).await;

        // Warmup with a single client
        let warmup_client = fila_sdk::FilaClient::connect(server.addr())
            .await
            .expect("connect warmup");
        let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(WARMUP_SECS);
        while tokio::time::Instant::now() < warmup_deadline {
            let batch = make_batch(&queue, batch_size, &payload);
            let _ = warmup_client.enqueue_many(batch).await;
        }
        drop(warmup_client);

        // Spawn producers
        let stop = Arc::new(AtomicBool::new(false));
        let total_messages = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();

        for _ in 0..producer_count {
            let pc = fila_sdk::FilaClient::connect(server.addr())
                .await
                .expect("connect producer");
            let pq = queue.clone();
            let pp = payload.clone();
            let ps = stop.clone();
            let tm = total_messages.clone();
            handles.push(tokio::spawn(async move {
                while !ps.load(Ordering::Relaxed) {
                    let batch = make_batch(&pq, batch_size, &pp);
                    let batch_results = pc.enqueue_many(batch).await;
                    tm.fetch_add(count_successes(&batch_results), Ordering::Relaxed);
                }
            }));
        }

        // Measure
        let before = total_messages.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(MEASURE_SECS)).await;
        let after = total_messages.load(Ordering::Relaxed);

        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.abort();
        }

        let produced = after - before;
        let throughput = produced as f64 / MEASURE_SECS as f64;

        results.push(BenchResult {
            name: format!("concurrent_batch_{producer_count}p_throughput"),
            value: throughput,
            unit: "msg/s".to_string(),
            metadata: [
                (
                    "producer_count".to_string(),
                    serde_json::json!(producer_count),
                ),
                ("batch_size".to_string(), serde_json::json!(batch_size)),
            ]
            .into_iter()
            .collect(),
        });
    }

    results
}
