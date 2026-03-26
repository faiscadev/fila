use crate::measurement::ThroughputMeter;
use crate::report::BenchResult;
use crate::server::{create_queue_cli, create_queue_with_lua_cli, BenchServer};
use std::collections::HashMap;
use std::time::Duration;

const PAYLOAD_SIZE: usize = 1024;
const MEASURE_SECS: u64 = 3;
const WARMUP_SECS: u64 = 1;

/// Measure Lua on_enqueue average execution overhead (NFR4: <50us avg).
///
/// Approach: measure enqueue throughput with and without Lua, using identical
/// headers. The difference in per-message latency is the Lua execution overhead.
pub async fn bench_lua_latency(server: &BenchServer) -> Vec<BenchResult> {
    let headers: HashMap<String, String> = [
        ("tenant_id".to_string(), "t1".to_string()),
        ("weight".to_string(), "1".to_string()),
    ]
    .into_iter()
    .collect();

    // Baseline: no Lua (same headers as Lua version for controlled comparison)
    let no_lua_queue = "bench-lua-baseline";
    create_queue_cli(server.addr(), no_lua_queue).await;
    let baseline_throughput = measure_throughput(server, no_lua_queue, headers.clone()).await;

    // With Lua on_enqueue
    let lua_queue = "bench-lua-overhead";
    let on_enqueue = r#"function on_enqueue(msg) local key = msg.headers["tenant_id"] or "default" local w = tonumber(msg.headers["weight"]) or 1 return { fairness_key = key, weight = w, throttle_keys = {} } end"#;
    create_queue_with_lua_cli(server.addr(), lua_queue, Some(on_enqueue), None).await;
    let lua_throughput = measure_throughput(server, lua_queue, headers).await;

    // Per-message overhead
    let baseline_per_msg_us = if baseline_throughput > 0.0 {
        1_000_000.0 / baseline_throughput
    } else {
        0.0
    };
    let lua_per_msg_us = if lua_throughput > 0.0 {
        1_000_000.0 / lua_throughput
    } else {
        0.0
    };
    let lua_overhead_us = (lua_per_msg_us - baseline_per_msg_us).max(0.0);

    vec![
        BenchResult {
            name: "lua_on_enqueue_overhead_us".to_string(),
            value: lua_overhead_us,
            unit: "us".to_string(),
            metadata: [
                (
                    "baseline_throughput".to_string(),
                    serde_json::json!(baseline_throughput),
                ),
                (
                    "lua_throughput".to_string(),
                    serde_json::json!(lua_throughput),
                ),
                ("nfr4_target".to_string(), serde_json::json!("< 50us avg")),
            ]
            .into_iter()
            .collect(),
        },
        BenchResult {
            name: "lua_throughput_with_hook".to_string(),
            value: lua_throughput,
            unit: "msg/s".to_string(),
            metadata: HashMap::new(),
        },
    ]
}

async fn measure_throughput(
    server: &BenchServer,
    queue: &str,
    headers: HashMap<String, String>,
) -> f64 {
    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    let payload = vec![0u8; PAYLOAD_SIZE];

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

    meter.msg_per_sec()
}
