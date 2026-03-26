//! Reads benchmark JSON results and updates docs/benchmarks.md tables in-place.
//!
//! Usage:
//!   bench-update-docs [--self-bench <path>] [--competitive-dir <path>] [--doc <path>]
//!
//! Defaults:
//!   --self-bench       crates/fila-bench/bench-results.json
//!   --competitive-dir  bench/competitive/results
//!   --doc              docs/benchmarks.md
//!
//! The doc file uses HTML comment markers like:
//!   <!-- bench:throughput-start -->
//!   ... table content ...
//!   <!-- bench:throughput-end -->
//!
//! The script replaces content between each marker pair. Sections whose JSON
//! data is missing are left untouched (preserving the last known values).

use std::collections::HashMap;
use std::path::PathBuf;

use fila_bench::report::{BenchReport, BenchResult};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let self_bench_path = arg_value(&args, "--self-bench")
        .unwrap_or_else(|| "crates/fila-bench/bench-results.json".to_string());
    let competitive_dir = arg_value(&args, "--competitive-dir")
        .unwrap_or_else(|| "bench/competitive/results".to_string());
    let doc_path = arg_value(&args, "--doc").unwrap_or_else(|| "docs/benchmarks.md".to_string());

    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("failed to read {doc_path}: {e}"));

    // Load self-benchmark results (optional)
    let self_report = load_report(&self_bench_path);

    // Load competitive results (optional, per broker)
    let competitive = load_competitive(&competitive_dir);

    let mut doc = doc;

    if let Some(ref report) = self_report {
        let bench_map = to_map(&report.benchmarks);
        let date = date_from_timestamp(&report.timestamp);

        // Traceability header
        let header = format!(
            "> Results from commit `{}` on {date}. Run benchmarks on your own hardware for results relevant to your environment. See [Reproducing results](#reproducing-results) for instructions.",
            report.commit
        );
        doc = replace_section(&doc, "header", &header);

        // Traceability footer
        let footer = format!(
            "Results in this document are from commit `{}` ({date}). Run `cargo bench -p fila-bench --bench system` to generate results for the current version. The JSON output includes the commit hash and timestamp for traceability.",
            report.commit
        );
        doc = replace_section(&doc, "traceability", &footer);

        // Self-benchmark tables
        doc = replace_section(&doc, "throughput", &render_throughput(&bench_map));
        doc = replace_section(&doc, "latency", &render_latency(&bench_map));
        doc = replace_section(
            &doc,
            "fair-scheduling-overhead",
            &render_fair_overhead(&bench_map),
        );
        doc = replace_section(
            &doc,
            "fairness-accuracy",
            &render_fairness_accuracy(&bench_map),
        );
        doc = replace_section(&doc, "lua-overhead", &render_lua_overhead(&bench_map));
        doc = replace_section(
            &doc,
            "cardinality-scaling",
            &render_cardinality_scaling(&bench_map),
        );
        doc = replace_section(
            &doc,
            "consumer-scaling",
            &render_consumer_scaling(&bench_map),
        );
        doc = replace_section(&doc, "memory", &render_memory(&bench_map));
        doc = replace_section(&doc, "compaction", &render_compaction(&bench_map));

        // Batch benchmarks (only if batch metrics present)
        if bench_map.contains_key("batch_enqueue_1_throughput") {
            doc = replace_section(
                &doc,
                "batch-enqueue-throughput",
                &render_batch_enqueue_throughput(&bench_map),
            );
            doc = replace_section(
                &doc,
                "batch-size-scaling",
                &render_batch_size_scaling(&bench_map),
            );
            doc = replace_section(
                &doc,
                "auto-batching-latency",
                &render_auto_batching_latency(&bench_map),
            );
            doc = replace_section(
                &doc,
                "batched-vs-unbatched",
                &render_batched_vs_unbatched(&bench_map),
            );
            doc = replace_section(
                &doc,
                "delivery-batching",
                &render_delivery_batching(&bench_map),
            );
            doc = replace_section(
                &doc,
                "concurrent-producer-batching",
                &render_concurrent_producer_batching(&bench_map),
            );
        }

        // Subsystem benchmarks (only if subsystem metrics present)
        if bench_map.contains_key("rocksdb_write_1kb_ops") {
            doc = replace_section(
                &doc,
                "subsystem-rocksdb",
                &render_subsystem_rocksdb(&bench_map),
            );
            doc = replace_section(
                &doc,
                "subsystem-protobuf",
                &render_subsystem_protobuf(&bench_map),
            );
            doc = replace_section(&doc, "subsystem-drr", &render_subsystem_drr(&bench_map));
            doc = replace_section(&doc, "subsystem-grpc", &render_subsystem_fibp(&bench_map));
            doc = replace_section(&doc, "subsystem-lua", &render_subsystem_lua(&bench_map));
        }
    }

    // Competitive tables (only if all 4 brokers have data)
    if competitive.len() == 4 {
        let all_benchmarks: Vec<&BenchResult> =
            competitive.values().flat_map(|r| &r.benchmarks).collect();
        let comp_map: HashMap<&str, f64> = all_benchmarks
            .iter()
            .map(|b| (b.name.as_str(), b.value))
            .collect();

        doc = replace_section(
            &doc,
            "competitive-throughput",
            &render_competitive_throughput(&comp_map),
        );
        doc = replace_section(
            &doc,
            "competitive-latency",
            &render_competitive_latency(&comp_map),
        );
        doc = replace_section(
            &doc,
            "competitive-lifecycle",
            &render_competitive_lifecycle(&comp_map),
        );
        doc = replace_section(
            &doc,
            "competitive-multi-producer",
            &render_competitive_multi_producer(&comp_map),
        );
        doc = replace_section(
            &doc,
            "competitive-resources",
            &render_competitive_resources(&comp_map),
        );
    }

    std::fs::write(&doc_path, &doc).unwrap_or_else(|e| panic!("failed to write {doc_path}: {e}"));

    let sections_updated = doc.matches("<!-- bench:").count() / 2;
    eprintln!("Updated {doc_path} ({sections_updated} marker sections)");
}

// --- Argument parsing ---

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

// --- File loading ---

fn load_report(path: &str) -> Option<BenchReport> {
    let path = PathBuf::from(path);
    if !path.exists() {
        eprintln!("Note: {path:?} not found, skipping self-benchmarks");
        return None;
    }
    let data = std::fs::read_to_string(&path).expect("read self-bench file");
    Some(serde_json::from_str(&data).expect("parse self-bench JSON"))
}

fn load_competitive(dir: &str) -> HashMap<String, BenchReport> {
    let mut map = HashMap::new();
    for broker in &["fila", "kafka", "rabbitmq", "nats"] {
        let path = PathBuf::from(dir).join(format!("bench-{broker}.json"));
        if path.exists() {
            let data = std::fs::read_to_string(&path).expect("read competitive file");
            let report: BenchReport = serde_json::from_str(&data).expect("parse competitive JSON");
            map.insert(broker.to_string(), report);
        }
    }
    if map.len() < 4 {
        eprintln!(
            "Note: only {}/4 competitive results found in {dir}, skipping competitive tables",
            map.len()
        );
    }
    map
}

fn to_map(benchmarks: &[BenchResult]) -> HashMap<&str, &BenchResult> {
    benchmarks.iter().map(|b| (b.name.as_str(), b)).collect()
}

// --- Section replacement ---

fn replace_section(doc: &str, name: &str, content: &str) -> String {
    let start_marker = format!("<!-- bench:{name}-start -->");
    let end_marker = format!("<!-- bench:{name}-end -->");

    let Some(start_idx) = doc.find(&start_marker) else {
        return doc.to_string();
    };
    let after_start = start_idx + start_marker.len();

    let Some(end_idx) = doc[after_start..].find(&end_marker) else {
        eprintln!(
            "Warning: found {start_marker} but missing {end_marker} — skipping section '{name}'"
        );
        return doc.to_string();
    };
    let end_idx = after_start + end_idx;

    format!(
        "{}{start_marker}\n{content}\n{end_marker}{}",
        &doc[..start_idx],
        &doc[end_idx + end_marker.len()..]
    )
}

fn date_from_timestamp(ts: &str) -> &str {
    if ts.len() >= 10 {
        &ts[..10]
    } else {
        ts
    }
}

// --- Number formatting ---

fn fmt_int(v: f64) -> String {
    let n = v.round() as i64;
    if n.abs() < 1000 {
        return n.to_string();
    }
    let s = n.abs().to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    if n < 0 {
        result.push('-');
    }
    result.chars().rev().collect()
}

fn fmt_dec(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}")
}

fn fmt_pct(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}%")
}

fn get_val(map: &HashMap<&str, &BenchResult>, name: &str) -> f64 {
    map.get(name).map(|b| b.value).unwrap_or(0.0)
}

fn get_meta_f64(map: &HashMap<&str, &BenchResult>, name: &str, key: &str) -> f64 {
    map.get(name)
        .and_then(|b| b.metadata.get(key))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

// --- Self-benchmark renderers ---

fn render_throughput(m: &HashMap<&str, &BenchResult>) -> String {
    let msg_s = get_val(m, "enqueue_throughput_1kb");
    let mb_s = get_val(m, "enqueue_throughput_1kb_mbps");
    format!(
        "\
| Metric | Value | Unit |
|--------|------:|------|
| Enqueue throughput (1KB payload) | {} | msg/s |
| Enqueue throughput (1KB payload) | {} | MB/s |",
        fmt_int(msg_s),
        fmt_dec(mb_s, 2)
    )
}

fn render_latency(m: &HashMap<&str, &BenchResult>) -> String {
    let p50 = get_val(m, "e2e_latency_p50_light");
    let p95 = get_val(m, "e2e_latency_p95_light");
    let p99 = get_val(m, "e2e_latency_p99_light");
    format!(
        "\
| Load level | Producers | p50 | p95 | p99 |
|------------|----------:|----:|----:|----:|
| Light | 1 | {} ms | {} ms | {} ms |",
        fmt_dec(p50, 2),
        fmt_dec(p95, 2),
        fmt_dec(p99, 2)
    )
}

fn render_fair_overhead(m: &HashMap<&str, &BenchResult>) -> String {
    let fifo = get_val(m, "fairness_overhead_fifo_throughput");
    let fair = get_val(m, "fairness_overhead_fair_throughput");
    let pct = get_val(m, "fairness_overhead_pct");
    format!(
        "\
| Mode | Throughput (msg/s) |
|------|-------------------:|
| FIFO baseline | {} |
| Fair scheduling (DRR) | {} |
| **Overhead** | **{}** |",
        fmt_int(fifo),
        fmt_int(fair),
        fmt_pct(pct, 1)
    )
}

fn render_fairness_accuracy(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Key | Weight | Expected share | Actual share | Deviation |\n");
    rows.push_str("|-----|-------:|---------------:|-------------:|----------:|");

    for i in 1..=5 {
        let name = format!("fairness_accuracy_tenant-{i}");
        if let Some(b) = m.get(name.as_str()) {
            let weight = get_meta_f64(m, &name, "weight") as i64;
            let expected = get_meta_f64(m, &name, "expected_share") * 100.0;
            let actual = get_meta_f64(m, &name, "actual_share") * 100.0;
            let deviation = b.value;
            rows.push_str(&format!(
                "\n| tenant-{i} | {weight} | {}% | {}% | {}% |",
                fmt_dec(expected, 1),
                fmt_dec(actual, 1),
                fmt_dec(deviation, 1)
            ));
        }
    }
    rows
}

fn render_lua_overhead(m: &HashMap<&str, &BenchResult>) -> String {
    let baseline = get_meta_f64(m, "lua_on_enqueue_overhead_us", "baseline_throughput");
    let with_lua = get_val(m, "lua_throughput_with_hook");
    let overhead = get_val(m, "lua_on_enqueue_overhead_us");
    format!(
        "\
| Metric | Value | Unit |
|--------|------:|------|
| Throughput without Lua | {} | msg/s |
| Throughput with `on_enqueue` hook | {} | msg/s |
| Per-message overhead | {} | us |",
        fmt_int(baseline),
        fmt_int(with_lua),
        fmt_dec(overhead, 1)
    )
}

fn render_cardinality_scaling(m: &HashMap<&str, &BenchResult>) -> String {
    let k10 = get_val(m, "key_cardinality_10_throughput");
    let k1k = get_val(m, "key_cardinality_1k_throughput");
    let k10k = get_val(m, "key_cardinality_10k_throughput");
    format!(
        "\
| Key count | Throughput (msg/s) |
|----------:|-------------------:|
| 10 | {} |
| 1,000 | {} |
| 10,000 | {} |",
        fmt_int(k10),
        fmt_int(k1k),
        fmt_int(k10k)
    )
}

fn render_consumer_scaling(m: &HashMap<&str, &BenchResult>) -> String {
    let c1 = get_val(m, "consumer_concurrency_1_throughput");
    let c10 = get_val(m, "consumer_concurrency_10_throughput");
    let c100 = get_val(m, "consumer_concurrency_100_throughput");
    format!(
        "\
| Consumers | Throughput (msg/s) |
|----------:|-------------------:|
| 1 | {} |
| 10 | {} |
| 100 | {} |",
        fmt_int(c1),
        fmt_int(c10),
        fmt_int(c100)
    )
}

fn render_memory(m: &HashMap<&str, &BenchResult>) -> String {
    let idle = get_val(m, "memory_rss_idle");
    let loaded = get_val(m, "memory_rss_loaded_10k");
    format!(
        "\
| Metric | Value |
|--------|------:|
| RSS idle | {} MB |
| RSS under load (10K messages) | {} MB |",
        fmt_int(idle),
        fmt_int(loaded)
    )
}

fn render_compaction(m: &HashMap<&str, &BenchResult>) -> String {
    let idle = get_val(m, "compaction_idle_p99");
    let active = get_val(m, "compaction_active_p99");
    let delta = get_val(m, "compaction_p99_delta").abs();
    format!(
        "\
| Metric | p99 latency |
|--------|------------:|
| Idle (no compaction) | {} ms |
| Active compaction | {} ms |
| **Delta** | **< {} ms** |",
        fmt_dec(idle, 2),
        fmt_dec(active, 2),
        fmt_dec(delta, 2)
    )
}

// --- Batch benchmark renderers ---

fn render_batch_enqueue_throughput(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Batch size | Throughput (msg/s) | Batches/s |\n");
    rows.push_str("|-----------:|-------------------:|----------:|");
    for size in &[1, 10, 50, 100, 500] {
        let tput_name = format!("batch_enqueue_{size}_throughput");
        let bps_name = format!("batch_enqueue_{size}_batches_per_sec");
        let tput = get_val(m, &tput_name);
        let bps = get_val(m, &bps_name);
        if tput > 0.0 {
            rows.push_str(&format!(
                "\n| {size} | {} | {} |",
                fmt_int(tput),
                fmt_int(bps)
            ));
        }
    }
    rows
}

fn render_batch_size_scaling(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Batch size | Throughput (msg/s) |\n");
    rows.push_str("|-----------:|-------------------:|");
    for size in &[1, 5, 10, 25, 50, 100, 250, 500, 1000] {
        let name = format!("batch_scaling_{size}_throughput");
        let val = get_val(m, &name);
        if val > 0.0 {
            rows.push_str(&format!("\n| {size} | {} |", fmt_int(val)));
        }
    }
    rows
}

fn render_auto_batching_latency(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Producers | p50 | p95 | p99 | p99.9 | p99.99 | max |\n");
    rows.push_str("|----------:|----:|----:|----:|------:|-------:|----:|");
    for producers in &[1, 10, 50] {
        let prefix = format!("auto_batch_{producers}p");
        let p50 = get_val(m, &format!("{prefix}_p50"));
        let p95 = get_val(m, &format!("{prefix}_p95"));
        let p99 = get_val(m, &format!("{prefix}_p99"));
        let p999 = get_val(m, &format!("{prefix}_p99_9"));
        let p9999 = get_val(m, &format!("{prefix}_p99_99"));
        let max = get_val(m, &format!("{prefix}_max"));
        if p50 > 0.0 {
            rows.push_str(&format!(
                "\n| {producers} | {} ms | {} ms | {} ms | {} ms | {} ms | {} ms |",
                fmt_dec(p50, 2),
                fmt_dec(p95, 2),
                fmt_dec(p99, 2),
                fmt_dec(p999, 2),
                fmt_dec(p9999, 2),
                fmt_dec(max, 2)
            ));
        }
    }
    rows
}

fn render_batched_vs_unbatched(m: &HashMap<&str, &BenchResult>) -> String {
    let unbatched = get_val(m, "compare_unbatched_throughput");
    let explicit = get_val(m, "compare_explicit_batch_throughput");
    let auto = get_val(m, "compare_auto_batch_throughput");
    let explicit_speedup = get_val(m, "compare_explicit_batch_speedup");
    let auto_speedup = get_val(m, "compare_auto_batch_speedup");
    format!(
        "\
| Mode | Throughput (msg/s) | Speedup |
|------|-------------------:|--------:|
| Unbatched | {} | 1.0x |
| Explicit batch (size 100) | {} | {}x |
| Auto-batch (size 100) | {} | {}x |",
        fmt_int(unbatched),
        fmt_int(explicit),
        fmt_dec(explicit_speedup, 1),
        fmt_int(auto),
        fmt_dec(auto_speedup, 1)
    )
}

fn render_delivery_batching(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Consumers | Throughput (msg/s) |\n");
    rows.push_str("|----------:|-------------------:|");
    for count in &[1, 10, 100] {
        let name = format!("delivery_batch_{count}c_throughput");
        let val = get_val(m, &name);
        if val > 0.0 {
            rows.push_str(&format!("\n| {count} | {} |", fmt_int(val)));
        }
    }
    rows
}

fn render_concurrent_producer_batching(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Producers | Throughput (msg/s) |\n");
    rows.push_str("|----------:|-------------------:|");
    for count in &[1, 5, 10, 50] {
        let name = format!("concurrent_batch_{count}p_throughput");
        let val = get_val(m, &name);
        if val > 0.0 {
            rows.push_str(&format!("\n| {count} | {} |", fmt_int(val)));
        }
    }
    rows
}

// --- Subsystem benchmark renderers ---

fn render_subsystem_rocksdb(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Payload | Throughput (ops/s) | p50 latency | p99 latency |\n");
    rows.push_str("|---------|-------------------:|------------:|------------:|");
    for payload in &["1kb", "64kb"] {
        let ops = get_val(m, &format!("rocksdb_write_{payload}_ops"));
        let p50 = get_val(m, &format!("rocksdb_write_{payload}_p50"));
        let p99 = get_val(m, &format!("rocksdb_write_{payload}_p99"));
        if ops > 0.0 {
            let label = payload.to_uppercase();
            rows.push_str(&format!(
                "\n| {label} | {} | {} us | {} us |",
                fmt_int(ops),
                fmt_dec(p50, 1),
                fmt_dec(p99, 1)
            ));
        }
    }
    rows
}

fn render_subsystem_protobuf(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Payload | Encode (MB/s) | Encode (ns/msg) | Decode (ns/msg) |\n");
    rows.push_str("|---------|:-------------:|:---------------:|:---------------:|");
    for payload in &["64b", "1kb", "64kb"] {
        let enc_mbps = get_val(m, &format!("proto_encode_{payload}_mbps"));
        let enc_ns = get_val(m, &format!("proto_encode_{payload}_ns"));
        let dec_ns = get_val(m, &format!("proto_decode_{payload}_ns"));
        if enc_mbps > 0.0 {
            let label = payload.to_uppercase();
            rows.push_str(&format!(
                "\n| {label} | {} | {} | {} |",
                fmt_dec(enc_mbps, 1),
                fmt_int(enc_ns),
                fmt_int(dec_ns)
            ));
        }
    }
    rows
}

fn render_subsystem_drr(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Active keys | Throughput (sel/s) |\n");
    rows.push_str("|------------:|-------------------:|");
    for keys in &["10", "1k", "10k"] {
        let val = get_val(m, &format!("drr_throughput_{keys}"));
        if val > 0.0 {
            let label = match *keys {
                "10" => "10",
                "1k" => "1,000",
                "10k" => "10,000",
                _ => keys,
            };
            rows.push_str(&format!("\n| {label} | {} |", fmt_int(val)));
        }
    }
    rows
}

fn render_subsystem_fibp(m: &HashMap<&str, &BenchResult>) -> String {
    let p50 = get_val(m, "fibp_roundtrip_p50");
    let p99 = get_val(m, "fibp_roundtrip_p99");
    let p999 = get_val(m, "fibp_roundtrip_p99_9");
    let ops = get_val(m, "fibp_roundtrip_ops");
    format!(
        "\
| Metric | Value | Unit |
|--------|------:|------|
| p50 latency | {} | us |
| p99 latency | {} | us |
| p99.9 latency | {} | us |
| Throughput | {} | ops/s |",
        fmt_dec(p50, 1),
        fmt_dec(p99, 1),
        fmt_dec(p999, 1),
        fmt_int(ops)
    )
}

fn render_subsystem_lua(m: &HashMap<&str, &BenchResult>) -> String {
    let mut rows = String::new();
    rows.push_str("| Script | Throughput (exec/s) | p50 | p99 |\n");
    rows.push_str("|--------|--------------------:|----:|----:|");
    for (key, label) in &[
        ("noop", "No-op (return defaults)"),
        ("header_set", "Header-set (read 2 headers)"),
        (
            "complex",
            "Complex routing (string ops, conditionals, table insert)",
        ),
    ] {
        let exec = get_val(m, &format!("lua_{key}_exec_s"));
        let p50 = get_val(m, &format!("lua_{key}_p50"));
        let p99 = get_val(m, &format!("lua_{key}_p99"));
        if exec > 0.0 {
            rows.push_str(&format!(
                "\n| {label} | {} | {} us | {} us |",
                fmt_int(exec),
                fmt_dec(p50, 1),
                fmt_dec(p99, 1)
            ));
        }
    }
    rows
}

// --- Competitive renderers ---

fn comp_val(m: &HashMap<&str, f64>, name: &str) -> f64 {
    m.get(name).copied().unwrap_or(0.0)
}

fn render_competitive_throughput(m: &HashMap<&str, f64>) -> String {
    let mut rows = String::new();
    rows.push_str("| Payload | Fila | Kafka | RabbitMQ | NATS |\n");
    rows.push_str("|---------|-----:|------:|---------:|-----:|");
    for (payload, suffix) in &[("64B", "64b"), ("1KB", "1kb"), ("64KB", "64kb")] {
        let fila = comp_val(m, &format!("fila_throughput_{suffix}"));
        let kafka = comp_val(m, &format!("kafka_throughput_{suffix}"));
        let rabbit = comp_val(m, &format!("rabbitmq_throughput_{suffix}"));
        let nats = comp_val(m, &format!("nats_throughput_{suffix}"));
        rows.push_str(&format!(
            "\n| {payload} | {} | {} | {} | {} |",
            fmt_int(fila),
            fmt_int(kafka),
            fmt_int(rabbit),
            fmt_int(nats)
        ));
    }
    rows
}

fn render_competitive_latency(m: &HashMap<&str, f64>) -> String {
    let mut rows = String::new();
    rows.push_str("| Percentile | Fila | Kafka | RabbitMQ | NATS |\n");
    rows.push_str("|-----------|-----:|------:|---------:|-----:|");
    for (label, suffix) in &[("p50", "p50"), ("p95", "p95"), ("p99", "p99")] {
        let fila = comp_val(m, &format!("fila_latency_{suffix}"));
        let kafka = comp_val(m, &format!("kafka_latency_{suffix}"));
        let rabbit = comp_val(m, &format!("rabbitmq_latency_{suffix}"));
        let nats = comp_val(m, &format!("nats_latency_{suffix}"));
        rows.push_str(&format!(
            "\n| {label} | {} ms | {} ms | {} ms | {} ms |",
            fmt_dec(fila, 2),
            fmt_dec(kafka, 2),
            fmt_dec(rabbit, 2),
            fmt_dec(nats, 2)
        ));
    }
    rows
}

fn render_competitive_lifecycle(m: &HashMap<&str, f64>) -> String {
    let mut brokers: Vec<(&str, f64)> = vec![
        ("Fila", comp_val(m, "fila_lifecycle_throughput")),
        ("Kafka", comp_val(m, "kafka_lifecycle_throughput")),
        ("RabbitMQ", comp_val(m, "rabbitmq_lifecycle_throughput")),
        ("NATS", comp_val(m, "nats_lifecycle_throughput")),
    ];
    brokers.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut rows = String::new();
    rows.push_str("| Broker | msg/s |\n");
    rows.push_str("|--------|------:|");
    for (name, val) in &brokers {
        rows.push_str(&format!("\n| {name} | {} |", fmt_int(*val)));
    }
    rows
}

fn render_competitive_multi_producer(m: &HashMap<&str, f64>) -> String {
    let mut brokers: Vec<(&str, f64)> = vec![
        ("Fila", comp_val(m, "fila_multi_producer_throughput")),
        ("Kafka", comp_val(m, "kafka_multi_producer_throughput")),
        (
            "RabbitMQ",
            comp_val(m, "rabbitmq_multi_producer_throughput"),
        ),
        ("NATS", comp_val(m, "nats_multi_producer_throughput")),
    ];
    brokers.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut rows = String::new();
    rows.push_str("| Broker | msg/s |\n");
    rows.push_str("|--------|------:|");
    for (name, val) in &brokers {
        rows.push_str(&format!("\n| {name} | {} |", fmt_int(*val)));
    }
    rows
}

fn render_competitive_resources(m: &HashMap<&str, f64>) -> String {
    let mut brokers: Vec<(&str, f64, f64)> = vec![
        (
            "Fila",
            comp_val(m, "fila_cpu_pct"),
            comp_val(m, "fila_memory_mb"),
        ),
        (
            "Kafka",
            comp_val(m, "kafka_cpu_pct"),
            comp_val(m, "kafka_memory_mb"),
        ),
        (
            "RabbitMQ",
            comp_val(m, "rabbitmq_cpu_pct"),
            comp_val(m, "rabbitmq_memory_mb"),
        ),
        (
            "NATS",
            comp_val(m, "nats_cpu_pct"),
            comp_val(m, "nats_memory_mb"),
        ),
    ];
    brokers.sort_by(|a, b| a.1.total_cmp(&b.1));

    let mut rows = String::new();
    rows.push_str("| Broker | CPU | Memory |\n");
    rows.push_str("|--------|----:|-------:|");
    for (name, cpu, mem) in &brokers {
        rows.push_str(&format!(
            "\n| {name} | {}% | {} MB |",
            fmt_dec(*cpu, 1),
            fmt_int(*mem)
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_section_works() {
        let doc = "before\n<!-- bench:test-start -->\nold content\n<!-- bench:test-end -->\nafter";
        let result = replace_section(doc, "test", "new content");
        assert_eq!(
            result,
            "before\n<!-- bench:test-start -->\nnew content\n<!-- bench:test-end -->\nafter"
        );
    }

    #[test]
    fn replace_section_missing_marker_is_noop() {
        let doc = "no markers here";
        let result = replace_section(doc, "missing", "new content");
        assert_eq!(result, doc);
    }

    #[test]
    fn replace_section_idempotent() {
        let doc = "<!-- bench:x-start -->\nfirst\n<!-- bench:x-end -->";
        let r1 = replace_section(&doc, "x", "content");
        let r2 = replace_section(&r1, "x", "content");
        assert_eq!(r1, r2);
    }

    #[test]
    fn fmt_int_formats_thousands() {
        assert_eq!(fmt_int(1234.0), "1,234");
        assert_eq!(fmt_int(999.0), "999");
        assert_eq!(fmt_int(1000000.0), "1,000,000");
        assert_eq!(fmt_int(42.6), "43");
    }

    #[test]
    fn fmt_dec_formats_decimals() {
        assert_eq!(fmt_dec(3.14159, 2), "3.14");
        assert_eq!(fmt_dec(0.1, 1), "0.1");
    }

    #[test]
    fn date_from_timestamp_handles_normal_and_short() {
        assert_eq!(date_from_timestamp("2026-03-09T01:17:46Z"), "2026-03-09");
        assert_eq!(date_from_timestamp("short"), "short");
        assert_eq!(date_from_timestamp(""), "");
    }

    fn make_bench(name: &str, value: f64, unit: &str) -> BenchResult {
        BenchResult {
            name: name.to_string(),
            value,
            unit: unit.to_string(),
            metadata: HashMap::new(),
        }
    }

    fn make_bench_with_meta(
        name: &str,
        value: f64,
        unit: &str,
        meta: Vec<(&str, f64)>,
    ) -> BenchResult {
        let metadata = meta
            .into_iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect();
        BenchResult {
            name: name.to_string(),
            value,
            unit: unit.to_string(),
            metadata,
        }
    }

    #[test]
    fn render_throughput_maps_correct_keys() {
        let benches = vec![
            make_bench("enqueue_throughput_1kb", 5348.0, "msg/s"),
            make_bench("enqueue_throughput_1kb_mbps", 5.22, "MB/s"),
        ];
        let m = to_map(&benches);
        let table = render_throughput(&m);
        assert!(table.contains("5,348"), "should contain formatted msg/s");
        assert!(table.contains("5.22"), "should contain MB/s");
    }

    #[test]
    fn render_latency_maps_correct_keys() {
        let benches = vec![
            make_bench("e2e_latency_p50_light", 0.22, "ms"),
            make_bench("e2e_latency_p95_light", 0.33, "ms"),
            make_bench("e2e_latency_p99_light", 0.50, "ms"),
        ];
        let m = to_map(&benches);
        let table = render_latency(&m);
        assert!(table.contains("0.22 ms"), "should contain p50");
        assert!(table.contains("0.33 ms"), "should contain p95");
        assert!(table.contains("0.50 ms"), "should contain p99");
    }

    #[test]
    fn render_fairness_accuracy_uses_metadata() {
        let benches = vec![make_bench_with_meta(
            "fairness_accuracy_tenant-1",
            0.2,
            "% deviation",
            vec![
                ("weight", 1.0),
                ("expected_share", 0.06667),
                ("actual_share", 0.068),
            ],
        )];
        let m = to_map(&benches);
        let table = render_fairness_accuracy(&m);
        assert!(table.contains("tenant-1"), "should contain tenant name");
        assert!(table.contains("6.8%"), "should contain actual share");
    }

    #[test]
    fn render_competitive_throughput_maps_all_brokers() {
        let mut comp_map = HashMap::new();
        comp_map.insert("fila_throughput_1kb", 2637.0);
        comp_map.insert("kafka_throughput_1kb", 143278.0);
        comp_map.insert("rabbitmq_throughput_1kb", 38321.0);
        comp_map.insert("nats_throughput_1kb", 137748.0);
        comp_map.insert("fila_throughput_64b", 2845.0);
        comp_map.insert("kafka_throughput_64b", 1473379.0);
        comp_map.insert("rabbitmq_throughput_64b", 36141.0);
        comp_map.insert("nats_throughput_64b", 394950.0);
        comp_map.insert("fila_throughput_64kb", 759.0);
        comp_map.insert("kafka_throughput_64kb", 2335.0);
        comp_map.insert("rabbitmq_throughput_64kb", 2379.0);
        comp_map.insert("nats_throughput_64kb", 2426.0);
        let table = render_competitive_throughput(&comp_map);
        assert!(table.contains("2,637"), "should contain Fila 1KB");
        assert!(table.contains("143,278"), "should contain Kafka 1KB");
        assert!(table.contains("64B"), "should have 64B row");
        assert!(table.contains("64KB"), "should have 64KB row");
    }

    #[test]
    fn render_competitive_lifecycle_sorts_descending() {
        let mut comp_map = HashMap::new();
        comp_map.insert("fila_lifecycle_throughput", 2724.0);
        comp_map.insert("kafka_lifecycle_throughput", 356.0);
        comp_map.insert("rabbitmq_lifecycle_throughput", 658.0);
        comp_map.insert("nats_lifecycle_throughput", 25763.0);
        let table = render_competitive_lifecycle(&comp_map);
        let nats_pos = table.find("NATS").unwrap();
        let fila_pos = table.find("Fila").unwrap();
        let kafka_pos = table.find("Kafka").unwrap();
        assert!(
            nats_pos < fila_pos && fila_pos < kafka_pos,
            "should be sorted by throughput descending"
        );
    }
}
