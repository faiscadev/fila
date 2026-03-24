//! Subsystem-level benchmarks that isolate and measure each internal
//! component independently, bypassing the full server stack.
//!
//! Gated behind `FILA_BENCH_SUBSYSTEM=1`.

use crate::measurement::{LatencyHistogram, ThroughputMeter};
use crate::report::BenchResult;
use fila_core::storage::StorageEngine as _;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const WARMUP_ITERS: usize = 500;
const MEASURE_SECS: u64 = 3;

// ---------------------------------------------------------------------------
// 1. RocksDB subsystem: raw WriteBatch put + commit throughput
// ---------------------------------------------------------------------------

/// Measure raw RocksDB WriteBatch put + commit throughput, bypassing
/// scheduler/gRPC/serialization. Tests at 1KB and 64KB payloads.
pub fn bench_rocksdb_write(results: &mut Vec<BenchResult>) {
    for &(label, size) in &[("1kb", 1024usize), ("64kb", 65536usize)] {
        let dir = tempfile::tempdir().expect("create temp dir");
        let engine = fila_core::storage::RocksDbEngine::open(dir.path()).expect("open rocksdb");

        let payload = vec![0xABu8; size];

        // Warmup
        for i in 0..WARMUP_ITERS {
            let key = format!("warmup:{i}");
            engine
                .put_message(key.as_bytes(), &make_test_message(&payload))
                .expect("warmup put");
        }

        // Measure
        let mut meter = ThroughputMeter::start();
        let mut hist = LatencyHistogram::new();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
        let mut seq = 0u64;

        while Instant::now() < deadline {
            let key = format!("bench:{seq}");
            seq += 1;
            let msg = make_test_message(&payload);

            let start = Instant::now();
            engine.put_message(key.as_bytes(), &msg).expect("put");
            hist.record(start.elapsed());
            meter.increment();
        }

        let ops_per_sec = meter.msg_per_sec();

        results.push(BenchResult {
            name: format!("subsystem_rocksdb_write_{label}_ops"),
            value: ops_per_sec,
            unit: "ops/s".to_string(),
            metadata: [
                ("payload_size".to_string(), serde_json::json!(size)),
                ("total_ops".to_string(), serde_json::json!(meter.count())),
            ]
            .into_iter()
            .collect(),
        });

        if let Some(pcts) = hist.percentiles() {
            results.push(BenchResult {
                name: format!("subsystem_rocksdb_write_{label}_p50"),
                value: pcts.p50,
                unit: "us".to_string(),
                metadata: HashMap::new(),
            });
            results.push(BenchResult {
                name: format!("subsystem_rocksdb_write_{label}_p99"),
                value: pcts.p99,
                unit: "us".to_string(),
                metadata: HashMap::new(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Serialization subsystem: protobuf encode + decode throughput
// ---------------------------------------------------------------------------

/// Measure protobuf encode + decode throughput for EnqueueRequest and
/// ConsumeResponse at 64B, 1KB, and 64KB payload sizes.
pub fn bench_serialization(results: &mut Vec<BenchResult>) {
    use prost::Message as ProstMessage;

    let sizes: &[(&str, usize)] = &[("64b", 64), ("1kb", 1024), ("64kb", 65536)];

    for &(label, size) in sizes {
        let payload = vec![0xCDu8; size];
        let mut headers = HashMap::new();
        headers.insert("tenant_id".to_string(), "bench-tenant".to_string());

        // -- EnqueueRequest encode/decode --
        let enqueue_req = fila_proto::EnqueueRequest {
            queue: "bench-queue".to_string(),
            headers: headers.clone(),
            payload: bytes::Bytes::copy_from_slice(&payload),
        };

        // Warmup
        for _ in 0..WARMUP_ITERS {
            let encoded = enqueue_req.encode_to_vec();
            let _ = fila_proto::EnqueueRequest::decode(&encoded[..]).unwrap();
        }

        // Measure encode
        let mut meter = ThroughputMeter::start();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
        let mut total_bytes_encoded = 0u64;
        while Instant::now() < deadline {
            let encoded = enqueue_req.encode_to_vec();
            total_bytes_encoded += encoded.len() as u64;
            meter.increment();
        }
        let _encode_msg_per_sec = meter.msg_per_sec();
        let encode_elapsed = meter.elapsed();
        let encode_ns_per_msg = if meter.count() > 0 {
            encode_elapsed.as_nanos() as f64 / meter.count() as f64
        } else {
            0.0
        };
        let encode_mb_per_sec = if encode_elapsed.as_secs_f64() > 0.0 {
            total_bytes_encoded as f64 / (1024.0 * 1024.0) / encode_elapsed.as_secs_f64()
        } else {
            0.0
        };

        results.push(BenchResult {
            name: format!("subsystem_serde_enqueue_encode_{label}_mbps"),
            value: encode_mb_per_sec,
            unit: "MB/s".to_string(),
            metadata: [
                ("payload_size".to_string(), serde_json::json!(size)),
                ("total_ops".to_string(), serde_json::json!(meter.count())),
            ]
            .into_iter()
            .collect(),
        });
        results.push(BenchResult {
            name: format!("subsystem_serde_enqueue_encode_{label}_ns"),
            value: encode_ns_per_msg,
            unit: "ns/msg".to_string(),
            metadata: HashMap::new(),
        });

        // Measure decode
        let encoded_data = enqueue_req.encode_to_vec();
        let mut meter = ThroughputMeter::start();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
        while Instant::now() < deadline {
            let _ = fila_proto::EnqueueRequest::decode(&encoded_data[..]).unwrap();
            meter.increment();
        }
        let decode_elapsed = meter.elapsed();
        let decode_ns_per_msg = if meter.count() > 0 {
            decode_elapsed.as_nanos() as f64 / meter.count() as f64
        } else {
            0.0
        };

        results.push(BenchResult {
            name: format!("subsystem_serde_enqueue_decode_{label}_ns"),
            value: decode_ns_per_msg,
            unit: "ns/msg".to_string(),
            metadata: [("total_ops".to_string(), serde_json::json!(meter.count()))]
                .into_iter()
                .collect(),
        });

        // -- ConsumeResponse encode/decode --
        let consume_resp = make_consume_response(&payload);

        // Warmup
        for _ in 0..WARMUP_ITERS {
            let encoded = consume_resp.encode_to_vec();
            let _ = fila_proto::ConsumeResponse::decode(&encoded[..]).unwrap();
        }

        // Measure encode
        let mut meter = ThroughputMeter::start();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
        let mut total_bytes = 0u64;
        while Instant::now() < deadline {
            let encoded = consume_resp.encode_to_vec();
            total_bytes += encoded.len() as u64;
            meter.increment();
        }
        let resp_encode_elapsed = meter.elapsed();
        let resp_encode_mb_per_sec = if resp_encode_elapsed.as_secs_f64() > 0.0 {
            total_bytes as f64 / (1024.0 * 1024.0) / resp_encode_elapsed.as_secs_f64()
        } else {
            0.0
        };
        let resp_encode_ns_per_msg = if meter.count() > 0 {
            resp_encode_elapsed.as_nanos() as f64 / meter.count() as f64
        } else {
            0.0
        };

        results.push(BenchResult {
            name: format!("subsystem_serde_consume_encode_{label}_mbps"),
            value: resp_encode_mb_per_sec,
            unit: "MB/s".to_string(),
            metadata: HashMap::new(),
        });
        results.push(BenchResult {
            name: format!("subsystem_serde_consume_encode_{label}_ns"),
            value: resp_encode_ns_per_msg,
            unit: "ns/msg".to_string(),
            metadata: HashMap::new(),
        });

        // Measure decode
        let encoded_resp = consume_resp.encode_to_vec();
        let mut meter = ThroughputMeter::start();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
        while Instant::now() < deadline {
            let _ = fila_proto::ConsumeResponse::decode(&encoded_resp[..]).unwrap();
            meter.increment();
        }
        let resp_decode_elapsed = meter.elapsed();
        let resp_decode_ns_per_msg = if meter.count() > 0 {
            resp_decode_elapsed.as_nanos() as f64 / meter.count() as f64
        } else {
            0.0
        };

        results.push(BenchResult {
            name: format!("subsystem_serde_consume_decode_{label}_ns"),
            value: resp_decode_ns_per_msg,
            unit: "ns/msg".to_string(),
            metadata: HashMap::new(),
        });
    }
}

// ---------------------------------------------------------------------------
// 3. DRR subsystem: next_key + consume_deficit cycle throughput
// ---------------------------------------------------------------------------

/// Measure DRR scheduler next_key + consume_deficit throughput at
/// varying active key counts. Isolates scheduling algorithm from storage.
pub fn bench_drr(results: &mut Vec<BenchResult>) {
    for &(label, key_count) in &[("10", 10usize), ("1k", 1_000), ("10k", 10_000)] {
        let quantum = 100u32;
        let mut drr = fila_core::broker::drr::DrrScheduler::new(quantum);

        // Set up keys with varying weights
        for i in 0..key_count {
            let weight = ((i % 5) + 1) as u32; // weights 1-5
            drr.add_key("bench-queue", &format!("key-{i}"), weight);
        }

        // Warmup: run a few rounds
        for _ in 0..3 {
            drr.start_new_round("bench-queue");
            while let Some(key) = drr.next_key("bench-queue") {
                drr.consume_deficit("bench-queue", &key);
            }
        }

        // Measure: count next_key + consume_deficit cycles per second
        let mut meter = ThroughputMeter::start();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);

        while Instant::now() < deadline {
            drr.start_new_round("bench-queue");
            loop {
                let Some(key) = drr.next_key("bench-queue") else {
                    break;
                };
                drr.consume_deficit("bench-queue", &key);
                meter.increment();
            }
        }

        let sel_per_sec = meter.msg_per_sec();

        results.push(BenchResult {
            name: format!("subsystem_drr_{label}_keys_throughput"),
            value: sel_per_sec,
            unit: "sel/s".to_string(),
            metadata: [
                ("key_count".to_string(), serde_json::json!(key_count)),
                ("quantum".to_string(), serde_json::json!(quantum)),
                (
                    "total_selections".to_string(),
                    serde_json::json!(meter.count()),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }
}

// ---------------------------------------------------------------------------
// 4. gRPC overhead: round-trip latency for a lightweight RPC
// ---------------------------------------------------------------------------

/// Measure gRPC round-trip overhead by calling a lightweight RPC
/// (Enqueue with minimal payload) and recording per-call latency.
/// Isolates tonic + HTTP/2 framing overhead from message processing.
pub async fn bench_grpc_overhead(
    server: &crate::server::BenchServer,
    results: &mut Vec<BenchResult>,
) {
    let queue = "bench-grpc-overhead";
    crate::server::create_queue_cli(server.addr(), queue);

    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    // Use a minimal 1-byte payload to minimize serialization/storage cost
    let payload = vec![0u8; 1];
    let headers: HashMap<String, String> = HashMap::new();

    // Warmup
    for _ in 0..WARMUP_ITERS {
        let _ = client
            .enqueue(queue, headers.clone(), payload.clone())
            .await;
    }

    // Measure per-call latency
    let mut hist = LatencyHistogram::new();
    let mut meter = ThroughputMeter::start();
    let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);

    while Instant::now() < deadline {
        let start = Instant::now();
        if client
            .enqueue(queue, headers.clone(), payload.clone())
            .await
            .is_ok()
        {
            hist.record(start.elapsed());
            meter.increment();
        }
    }

    if let Some(pcts) = hist.percentiles() {
        results.push(BenchResult {
            name: "subsystem_grpc_overhead_p50".to_string(),
            value: pcts.p50,
            unit: "us".to_string(),
            metadata: [
                ("total_calls".to_string(), serde_json::json!(meter.count())),
                ("payload_size".to_string(), serde_json::json!(1)),
            ]
            .into_iter()
            .collect(),
        });
        results.push(BenchResult {
            name: "subsystem_grpc_overhead_p99".to_string(),
            value: pcts.p99,
            unit: "us".to_string(),
            metadata: HashMap::new(),
        });
        results.push(BenchResult {
            name: "subsystem_grpc_overhead_p99_9".to_string(),
            value: pcts.p99_9,
            unit: "us".to_string(),
            metadata: HashMap::new(),
        });
    }

    results.push(BenchResult {
        name: "subsystem_grpc_overhead_ops".to_string(),
        value: meter.msg_per_sec(),
        unit: "ops/s".to_string(),
        metadata: HashMap::new(),
    });
}

// ---------------------------------------------------------------------------
// 5. Lua subsystem: on_enqueue hook execution throughput
// ---------------------------------------------------------------------------

/// Measure Lua on_enqueue hook execution throughput for no-op, simple
/// header-set, and complex routing scripts.
pub fn bench_lua(results: &mut Vec<BenchResult>) {
    use fila_core::lua::{on_enqueue, sandbox};

    let scripts: &[(&str, &str)] = &[
        (
            "noop",
            r#"function on_enqueue(msg)
                return { fairness_key = "default", weight = 1, throttle_keys = {} }
            end"#,
        ),
        (
            "header_set",
            r#"function on_enqueue(msg)
                local key = msg.headers["tenant_id"] or "default"
                local w = tonumber(msg.headers["weight"]) or 1
                return { fairness_key = key, weight = w, throttle_keys = {} }
            end"#,
        ),
        (
            "complex_routing",
            r#"function on_enqueue(msg)
                local tenant = msg.headers["tenant_id"] or "default"
                local region = msg.headers["region"] or "us-east"
                local priority = tonumber(msg.headers["priority"]) or 5
                local key = tenant .. ":" .. region
                local w = 1
                if priority <= 3 then w = 5
                elseif priority <= 6 then w = 2
                end
                local throttles = { "tenant:" .. tenant, "region:" .. region }
                if priority <= 1 then
                    table.insert(throttles, "critical")
                end
                return { fairness_key = key, weight = w, throttle_keys = throttles }
            end"#,
        ),
    ];

    let mut headers = HashMap::new();
    headers.insert("tenant_id".to_string(), "acme-corp".to_string());
    headers.insert("weight".to_string(), "3".to_string());
    headers.insert("region".to_string(), "us-west".to_string());
    headers.insert("priority".to_string(), "2".to_string());

    for &(label, source) in scripts {
        let lua = sandbox::create_sandbox().expect("create lua sandbox");
        let func = lua
            .load(source)
            .set_name("on_enqueue_script")
            .into_function()
            .expect("compile script");
        let bytecode = func.dump(true);

        // Warmup
        for _ in 0..WARMUP_ITERS {
            on_enqueue::run_on_enqueue(&lua, &bytecode, &headers, 1024, "bench-queue");
        }

        // Measure
        let mut meter = ThroughputMeter::start();
        let mut hist = LatencyHistogram::new();
        let deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);

        while Instant::now() < deadline {
            let start = Instant::now();
            on_enqueue::run_on_enqueue(&lua, &bytecode, &headers, 1024, "bench-queue");
            hist.record(start.elapsed());
            meter.increment();
        }

        let exec_per_sec = meter.msg_per_sec();

        results.push(BenchResult {
            name: format!("subsystem_lua_{label}_throughput"),
            value: exec_per_sec,
            unit: "exec/s".to_string(),
            metadata: [
                ("script".to_string(), serde_json::json!(label)),
                ("total_execs".to_string(), serde_json::json!(meter.count())),
            ]
            .into_iter()
            .collect(),
        });

        if let Some(pcts) = hist.percentiles() {
            results.push(BenchResult {
                name: format!("subsystem_lua_{label}_p50"),
                value: pcts.p50,
                unit: "us".to_string(),
                metadata: HashMap::new(),
            });
            results.push(BenchResult {
                name: format!("subsystem_lua_{label}_p99"),
                value: pcts.p99,
                unit: "us".to_string(),
                metadata: HashMap::new(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_test_message(payload: &[u8]) -> fila_core::message::Message {
    fila_core::message::Message {
        id: uuid::Uuid::now_v7(),
        queue_id: "bench-queue".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("tenant_id".to_string(), "bench".to_string());
            h
        },
        payload: bytes::Bytes::copy_from_slice(payload),
        fairness_key: "default".to_string(),
        weight: 1,
        throttle_keys: Vec::new(),
        attempt_count: 0,
        enqueued_at: 1_700_000_000_000_000_000,
        leased_at: None,
    }
}

fn make_consume_response(payload: &[u8]) -> fila_proto::ConsumeResponse {
    let msg = fila_proto::Message {
        id: uuid::Uuid::now_v7().to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("tenant_id".to_string(), "bench".to_string());
            h
        },
        payload: bytes::Bytes::copy_from_slice(payload),
        metadata: Some(fila_proto::MessageMetadata {
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: Vec::new(),
            attempt_count: 0,
            queue_id: "bench-queue".to_string(),
        }),
        timestamps: Some(fila_proto::MessageTimestamps {
            enqueued_at: Some(prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
            }),
            leased_at: None,
        }),
    };
    fila_proto::ConsumeResponse {
        message: Some(msg),
        messages: Vec::new(),
    }
}
