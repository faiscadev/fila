use fila_bench::benchmarks::{compaction, fairness, latency, lua, memory, scaling, throughput};
use fila_bench::report::BenchReport;
use fila_bench::server::BenchServer;

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async {
        run_benchmarks().await;
    });
}

async fn run_benchmarks() {
    eprintln!("Starting Fila system benchmark suite...\n");

    let server = BenchServer::start();
    eprintln!("  Server started at {}\n", server.addr());

    let mut report = BenchReport::new();

    // 1. Enqueue throughput (AC 1)
    eprintln!("[1/10] Enqueue throughput (1KB payloads)...");
    let results = throughput::bench_enqueue_throughput(&server).await;
    for r in results {
        report.add(r);
    }

    // 2. End-to-end latency (AC 2)
    eprintln!("[2/10] End-to-end latency (light/moderate/saturated)...");
    let results = latency::bench_e2e_latency(&server).await;
    for r in results {
        report.add(r);
    }

    // 3. Fairness overhead (AC 3)
    eprintln!("[3/10] Fairness scheduling overhead vs FIFO...");
    let results = fairness::bench_fairness_overhead(&server).await;
    for r in results {
        report.add(r);
    }

    // 4. Fairness accuracy (AC 4)
    eprintln!("[4/10] Fairness accuracy (5 keys, varying weights)...");
    let results = fairness::bench_fairness_accuracy(&server).await;
    for r in results {
        report.add(r);
    }

    // 5. Lua on_enqueue latency (AC 5)
    eprintln!("[5/10] Lua on_enqueue execution overhead...");
    let results = lua::bench_lua_latency(&server).await;
    for r in results {
        report.add(r);
    }

    // 6. Memory footprint (AC 9 — run early before heavy tests)
    eprintln!("[6/10] Memory footprint (RSS)...");
    let results = memory::bench_memory_footprint(&server).await;
    for r in results {
        report.add(r);
    }

    // 7. RocksDB compaction impact (AC 10)
    eprintln!("[7/10] RocksDB compaction impact on tail latency...");
    let results = compaction::bench_compaction_impact(&server).await;
    for r in results {
        report.add(r);
    }

    // 8. Key cardinality (AC 7)
    eprintln!("[8/10] Fairness key cardinality scaling (10 to 100K keys)...");
    let results = scaling::bench_key_cardinality(&server).await;
    for r in results {
        report.add(r);
    }

    // 9. Consumer concurrency (AC 8)
    eprintln!("[9/10] Consumer concurrency scaling (1/10/100)...");
    let results = scaling::bench_consumer_concurrency(&server).await;
    for r in results {
        report.add(r);
    }

    // Note: Queue depth scaling (AC 6) is skipped by default as it takes a
    // long time to pre-load 10M messages. Enable it via environment variable.
    if std::env::var("FILA_BENCH_DEPTH").is_ok() {
        eprintln!("[10/10] Queue depth scaling (1M/10M messages)...");
        let results = scaling::bench_queue_depth_scaling(&server).await;
        for r in results {
            report.add(r);
        }
    } else {
        eprintln!("[10/10] Queue depth scaling (skipped — set FILA_BENCH_DEPTH=1 to enable)");
    }

    // Write report
    report.write_and_print("bench-results.json");
}
