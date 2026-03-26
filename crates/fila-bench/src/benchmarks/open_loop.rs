use crate::benchmarks::latency::emit_latency_results;
use crate::measurement::{bench_duration_secs, LatencyHistogram, ThroughputMeter};
use crate::report::BenchResult;
use crate::server::{create_queue_cli, BenchServer};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_stream::StreamExt;

const PAYLOAD_SIZE: usize = 1024;

/// Duration for the closed-loop saturation probe used to discover max throughput.
const SATURATION_PROBE_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// Saturation probe: discover max throughput via closed-loop burst
// ---------------------------------------------------------------------------

/// Run a short closed-loop burst and return the observed messages-per-second.
async fn saturation_probe(server: &BenchServer, queue: &str) -> f64 {
    let probe_queue = format!("{queue}-satprobe");
    create_queue_cli(server.addr(), &probe_queue).await;

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect for saturation probe");

    let payload = vec![0u8; PAYLOAD_SIZE];
    let headers: HashMap<String, String> = HashMap::new();

    let mut meter = ThroughputMeter::start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(SATURATION_PROBE_SECS);

    while tokio::time::Instant::now() < deadline {
        if client
            .enqueue(&probe_queue, headers.clone(), payload.clone())
            .await
            .is_ok()
        {
            meter.increment();
        }
    }

    let rate = meter.msg_per_sec();
    eprintln!(
        "    Saturation probe: {:.0} msg/s in {}s",
        rate, SATURATION_PROBE_SECS
    );
    rate
}

// ---------------------------------------------------------------------------
// Open-loop generator core
// ---------------------------------------------------------------------------

/// Run an open-loop load generation pass at `target_rate` msg/s for `duration`.
///
/// Each request is spawned as an independent task. Latency is measured as
/// `completed_time - scheduled_time` (includes queuing delay). Results are
/// recorded with CO-corrected histograms.
///
/// Returns (merged histogram, actual messages completed).
async fn open_loop_run(
    server: &BenchServer,
    queue: &str,
    target_rate: f64,
    duration: Duration,
) -> (LatencyHistogram, u64) {
    let interval = Duration::from_micros((1_000_000.0 / target_rate) as u64);
    let mut ticker = tokio::time::interval(interval);
    // Use burst mode so ticks that were missed are fired immediately to keep
    // the scheduled cadence accurate for latency measurement.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

    let histogram = Arc::new(Mutex::new(LatencyHistogram::new()));
    let completed = Arc::new(std::sync::atomic::AtomicU64::new(0));

    let start = Instant::now();
    let mut handles = Vec::new();

    loop {
        let scheduled = ticker.tick().await;
        if start.elapsed() >= duration {
            break;
        }

        let hist = histogram.clone();
        let done = completed.clone();
        let addr = server.addr().to_string();
        let queue_name = queue.to_string();
        let iv = interval;

        handles.push(tokio::spawn(async move {
            let client = match fila_sdk::FilaClient::connect(&addr).await {
                Ok(c) => c,
                Err(_) => return,
            };

            let payload = vec![0u8; PAYLOAD_SIZE];
            let headers: HashMap<String, String> = HashMap::new();

            if client.enqueue(&queue_name, headers, payload).await.is_err() {
                return;
            }

            let completed_time = tokio::time::Instant::now();
            let latency = completed_time - scheduled;

            hist.lock().unwrap().record_corrected(latency, iv);
            done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
    }

    // Wait for all in-flight tasks (with a timeout to avoid hanging).
    let join_deadline = Duration::from_secs(30);
    let join_start = Instant::now();
    for handle in handles {
        let remaining = join_deadline.saturating_sub(join_start.elapsed());
        if remaining.is_zero() {
            handle.abort();
        } else {
            let _ = tokio::time::timeout(remaining, handle).await;
        }
    }

    let count = completed.load(std::sync::atomic::Ordering::Relaxed);
    let merged = Arc::try_unwrap(histogram)
        .unwrap_or_else(|arc| {
            let lock = arc.lock().unwrap();
            // Clone by serializing + deserializing since LatencyHistogram
            // doesn't derive Clone.
            let b64 = lock.serialize_base64();
            Mutex::new(LatencyHistogram::deserialize_base64(&b64).unwrap())
        })
        .into_inner()
        .unwrap();

    (merged, count)
}

/// Run open-loop with concurrent consumption: producer sends at `target_rate`,
/// consumers ack messages with an optional processing delay.
///
/// Returns (histogram of e2e latency, messages consumed).
async fn open_loop_produce_consume(
    server: &BenchServer,
    queue: &str,
    target_rate: f64,
    duration: Duration,
    processing_delay: Duration,
) -> (LatencyHistogram, u64) {
    let interval = Duration::from_micros((1_000_000.0 / target_rate).max(1.0) as u64);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

    let histogram = Arc::new(Mutex::new(LatencyHistogram::new()));
    let completed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Start consumer tasks (4 concurrent consumers for throughput).
    let num_consumers = 4;
    let mut consumer_handles = Vec::new();
    for _ in 0..num_consumers {
        let s = stop.clone();
        let addr = server.addr().to_string();
        let q = queue.to_string();
        let delay = processing_delay;

        consumer_handles.push(tokio::spawn(async move {
            let client = fila_sdk::FilaClient::connect(&addr)
                .await
                .expect("consumer connect");
            let mut stream = client.consume(&q).await.expect("consume stream");

            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                let next = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
                match next {
                    Ok(Some(Ok(msg))) => {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        let _ = client.ack(&q, &msg.id).await;
                    }
                    _ => continue,
                }
            }
        }));
    }

    // Give consumers a moment to establish streams.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let start = Instant::now();
    let mut producer_handles = Vec::new();

    loop {
        let scheduled = ticker.tick().await;
        if start.elapsed() >= duration {
            break;
        }

        let hist = histogram.clone();
        let done = completed.clone();
        let addr = server.addr().to_string();
        let queue_name = queue.to_string();
        let iv = interval;

        producer_handles.push(tokio::spawn(async move {
            let client = match fila_sdk::FilaClient::connect(&addr).await {
                Ok(c) => c,
                Err(_) => return,
            };

            let payload = vec![0u8; PAYLOAD_SIZE];
            let headers: HashMap<String, String> = HashMap::new();

            let enqueue_result = client.enqueue(&queue_name, headers, payload).await;
            if enqueue_result.is_err() {
                return;
            }

            let completed_time = tokio::time::Instant::now();
            let latency = completed_time - scheduled;

            hist.lock().unwrap().record_corrected(latency, iv);
            done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
    }

    // Wait for producers to finish.
    for handle in producer_handles {
        let _ = tokio::time::timeout(Duration::from_secs(30), handle).await;
    }

    // Signal consumers to stop and join them.
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for handle in consumer_handles {
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    let count = completed.load(std::sync::atomic::Ordering::Relaxed);
    let merged = Arc::try_unwrap(histogram)
        .unwrap_or_else(|arc| {
            let lock = arc.lock().unwrap();
            let b64 = lock.serialize_base64();
            Mutex::new(LatencyHistogram::deserialize_base64(&b64).unwrap())
        })
        .into_inner()
        .unwrap();

    (merged, count)
}

// ---------------------------------------------------------------------------
// Benchmark 1: Latency under load
// ---------------------------------------------------------------------------

/// Test latency at 50%, 80%, and 100% of max throughput using open-loop generation.
///
/// First runs a short closed-loop saturation probe, then generates load at
/// each percentage. Each level runs for at least 30 seconds.
pub async fn bench_latency_under_load(server: &BenchServer) -> Vec<BenchResult> {
    let queue = "bench-openloop-latency-load";
    create_queue_cli(server.addr(), queue).await;

    let max_rate = saturation_probe(server, queue).await;
    let duration = Duration::from_secs(bench_duration_secs());

    let load_levels: &[(f64, &str)] = &[(0.50, "50pct"), (0.80, "80pct"), (1.00, "100pct")];

    let mut results = Vec::new();

    for &(fraction, label) in load_levels {
        let target = max_rate * fraction;
        eprintln!(
            "    Load level {label}: {target:.0} msg/s ({:.0}% of {max_rate:.0})",
            fraction * 100.0
        );

        let level_queue = format!("{queue}-{label}");
        create_queue_cli(server.addr(), &level_queue).await;

        let (histogram, count) = open_loop_run(server, &level_queue, target, duration).await;

        eprintln!("    Completed {count} messages");

        let meta: HashMap<String, serde_json::Value> = [
            ("target_rate".to_string(), serde_json::json!(target)),
            ("max_rate".to_string(), serde_json::json!(max_rate)),
            ("load_fraction".to_string(), serde_json::json!(fraction)),
            ("completed".to_string(), serde_json::json!(count)),
        ]
        .into_iter()
        .collect();

        results.extend(emit_latency_results(
            &histogram,
            "latency_under_load",
            label,
            &meta,
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 2: Consumer processing time
// ---------------------------------------------------------------------------

/// Test throughput and latency with 4 processing delays: 0ms, 1ms, 10ms, 100ms.
///
/// Uses open-loop production at a fixed rate with concurrent consumption.
/// Consumers simulate processing time with `tokio::time::sleep` before acking.
pub async fn bench_consumer_processing_time(server: &BenchServer) -> Vec<BenchResult> {
    let delays: &[(Duration, &str)] = &[
        (Duration::ZERO, "0ms"),
        (Duration::from_millis(1), "1ms"),
        (Duration::from_millis(10), "10ms"),
        (Duration::from_millis(100), "100ms"),
    ];

    // Use a moderate fixed rate that can be sustained even with slow consumers.
    // We pick a rate based on the fastest case (0ms delay), then use the same
    // rate for all levels to make comparisons meaningful.
    let probe_queue = "bench-openloop-proctime-probe";
    create_queue_cli(server.addr(), probe_queue).await;
    let max_rate = saturation_probe(server, probe_queue).await;
    // Use 50% of max so the server isn't saturated even with 100ms delays.
    let target_rate = max_rate * 0.50;

    let duration = Duration::from_secs(bench_duration_secs());
    let mut results = Vec::new();

    for &(delay, label) in delays {
        let queue = format!("bench-openloop-proctime-{label}");
        create_queue_cli(server.addr(), &queue).await;

        eprintln!("    Processing delay {label}: target {target_rate:.0} msg/s");

        let (histogram, count) =
            open_loop_produce_consume(server, &queue, target_rate, duration, delay).await;

        let elapsed_secs = duration.as_secs_f64();
        let achieved_rate = count as f64 / elapsed_secs;

        eprintln!("    Completed {count} messages ({achieved_rate:.0} msg/s)");

        let mut meta: HashMap<String, serde_json::Value> = [
            ("delay_ms".to_string(), serde_json::json!(delay.as_millis())),
            ("target_rate".to_string(), serde_json::json!(target_rate)),
            (
                "achieved_rate".to_string(),
                serde_json::json!(achieved_rate),
            ),
            ("completed".to_string(), serde_json::json!(count)),
        ]
        .into_iter()
        .collect();

        // Add throughput result.
        results.push(BenchResult {
            name: format!("consumer_proctime_throughput_{label}"),
            value: achieved_rate,
            unit: "msg/s".to_string(),
            metadata: meta.clone(),
        });

        // Add latency percentiles.
        meta.insert("samples".to_string(), serde_json::json!(histogram.count()));
        results.extend(emit_latency_results(
            &histogram,
            "consumer_proctime_latency",
            label,
            &meta,
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 3: Backpressure ramp
// ---------------------------------------------------------------------------

/// Ramp producer rate from 10% to 150% of max throughput in 10% increments.
///
/// Each step runs for at least 10 seconds with open-loop generation.
/// The saturation inflection point is identifiable from the results.
pub async fn bench_backpressure_ramp(server: &BenchServer) -> Vec<BenchResult> {
    let probe_queue = "bench-openloop-ramp-probe";
    create_queue_cli(server.addr(), probe_queue).await;
    let max_rate = saturation_probe(server, probe_queue).await;

    let step_duration = Duration::from_secs(10);
    let mut results = Vec::new();

    // 10% to 150% in 10% steps = 15 steps.
    for pct in (10..=150).step_by(10) {
        let fraction = pct as f64 / 100.0;
        let target = max_rate * fraction;
        let label = format!("{pct}pct");

        let queue = format!("bench-openloop-ramp-{label}");
        create_queue_cli(server.addr(), &queue).await;

        eprintln!("    Ramp step {pct}%: target {target:.0} msg/s");

        let (histogram, count) = open_loop_run(server, &queue, target, step_duration).await;

        let elapsed_secs = step_duration.as_secs_f64();
        let achieved_rate = count as f64 / elapsed_secs;

        eprintln!("    Achieved {achieved_rate:.0} msg/s ({count} messages)");

        let meta: HashMap<String, serde_json::Value> = [
            ("target_pct".to_string(), serde_json::json!(pct)),
            ("target_rate".to_string(), serde_json::json!(target)),
            ("max_rate".to_string(), serde_json::json!(max_rate)),
            (
                "achieved_rate".to_string(),
                serde_json::json!(achieved_rate),
            ),
            ("completed".to_string(), serde_json::json!(count)),
        ]
        .into_iter()
        .collect();

        // Add throughput result for this step.
        results.push(BenchResult {
            name: format!("backpressure_ramp_throughput_{label}"),
            value: achieved_rate,
            unit: "msg/s".to_string(),
            metadata: meta.clone(),
        });

        // Add latency percentiles for this step.
        results.extend(emit_latency_results(
            &histogram,
            "backpressure_ramp_latency",
            &label,
            &meta,
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 4: Queue depth latency
// ---------------------------------------------------------------------------

/// Pre-load the queue to depths of 0, 1K, 10K, and 100K messages, then
/// measure e2e consume latency for newly produced messages at each depth.
///
/// Uses open-loop generation for 10 seconds at each depth level.
pub async fn bench_queue_depth_latency(server: &BenchServer) -> Vec<BenchResult> {
    let depths: &[(u64, &str)] = &[(0, "0"), (1_000, "1k"), (10_000, "10k"), (100_000, "100k")];

    // Use a moderate rate for latency measurement.
    let probe_queue = "bench-openloop-depth-probe";
    create_queue_cli(server.addr(), probe_queue).await;
    let max_rate = saturation_probe(server, probe_queue).await;
    let target_rate = max_rate * 0.50;

    let measure_duration = Duration::from_secs(10);
    let mut results = Vec::new();

    for &(depth, label) in depths {
        let queue = format!("bench-openloop-depth-{label}");
        create_queue_cli(server.addr(), &queue).await;

        // Pre-load messages to reach target depth.
        if depth > 0 {
            eprintln!("    Pre-loading {depth} messages for depth {label}...");
            let client = fila_sdk::FilaClient::connect(server.addr())
                .await
                .expect("connect for preload");
            let payload = vec![0u8; PAYLOAD_SIZE];
            let headers: HashMap<String, String> = HashMap::new();

            for _ in 0..depth {
                let _ = client
                    .enqueue(&queue, headers.clone(), payload.clone())
                    .await;
            }
            eprintln!("    Pre-loaded {depth} messages");
        }

        eprintln!("    Queue depth {label}: measuring at {target_rate:.0} msg/s");

        let (histogram, count) = open_loop_produce_consume(
            server,
            &queue,
            target_rate,
            measure_duration,
            Duration::ZERO,
        )
        .await;

        eprintln!("    Completed {count} messages");

        let meta: HashMap<String, serde_json::Value> = [
            ("queue_depth".to_string(), serde_json::json!(depth)),
            ("target_rate".to_string(), serde_json::json!(target_rate)),
            ("completed".to_string(), serde_json::json!(count)),
        ]
        .into_iter()
        .collect();

        results.extend(emit_latency_results(
            &histogram,
            "queue_depth_latency",
            label,
            &meta,
        ));
    }

    results
}
