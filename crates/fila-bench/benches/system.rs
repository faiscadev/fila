use fila_bench::benchmarks::{
    batch, compaction, failure_paths, fairness, latency, lua, memory, open_loop, scaling,
    subsystem, throughput,
};
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

    // 10.5. Equal-weight fairness (Jain's Fairness Index)
    eprintln!("[10.5/10] Equal-weight fairness (Jain's Fairness Index)...");
    let results = fairness::bench_equal_weight_fairness(&server).await;
    for r in results {
        report.add(r);
    }

    // Failure-path benchmarks (gated — involve nack storms and DLQ setup)
    if std::env::var("FILA_BENCH_FAILURE_PATHS").is_ok() {
        eprintln!("[F1] Nack storm benchmark (10% nack rate)...");
        let results = failure_paths::bench_nack_storm(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[F2] DLQ routing overhead benchmark...");
        let results = failure_paths::bench_dlq_routing_overhead(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[F3] Poison pill isolation benchmark...");
        let results = failure_paths::bench_poison_pill_isolation(&server).await;
        for r in results {
            report.add(r);
        }
    } else {
        eprintln!(
            "[F1-F3] Failure-path benchmarks (skipped — set FILA_BENCH_FAILURE_PATHS=1 to enable)"
        );
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

    // Open-loop benchmarks (long-running — opt-in via env var)
    if std::env::var("FILA_BENCH_OPENLOOP").is_ok() {
        eprintln!("[11/14] Latency under load (open-loop)...");
        let results = open_loop::bench_latency_under_load(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[12/14] Consumer processing time (open-loop)...");
        let results = open_loop::bench_consumer_processing_time(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[13/14] Backpressure ramp (open-loop)...");
        let results = open_loop::bench_backpressure_ramp(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[14/14] Queue depth latency (open-loop)...");
        let results = open_loop::bench_queue_depth_latency(&server).await;
        for r in results {
            report.add(r);
        }
    } else {
        eprintln!("[11-14] Open-loop benchmarks (skipped — set FILA_BENCH_OPENLOOP=1 to enable)");
    }

    // Subsystem benchmarks (gated — isolate internal components)
    if std::env::var("FILA_BENCH_SUBSYSTEM").is_ok() {
        let mut subsystem_results = Vec::new();

        eprintln!("[S1] RocksDB raw write throughput...");
        subsystem::bench_rocksdb_write(&mut subsystem_results);

        eprintln!("[S2] Protobuf serialization throughput...");
        subsystem::bench_serialization(&mut subsystem_results);

        eprintln!("[S3] DRR scheduler throughput...");
        subsystem::bench_drr(&mut subsystem_results);

        eprintln!("[S4] gRPC round-trip overhead...");
        subsystem::bench_grpc_overhead(&server, &mut subsystem_results).await;

        eprintln!("[S5] Lua execution throughput...");
        subsystem::bench_lua(&mut subsystem_results);

        for r in subsystem_results {
            report.add(r);
        }
    } else {
        eprintln!(
            "[S1-S5] Subsystem benchmarks (skipped — set FILA_BENCH_SUBSYSTEM=1 to enable)"
        );
    }

    // Batch benchmarks (gated — requires batch API support)
    if std::env::var("FILA_BENCH_BATCH").is_ok() {
        eprintln!("[B1] BatchEnqueue throughput (batch sizes 1-500)...");
        let results = batch::bench_batch_enqueue_throughput(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[B2] Batch size scaling (1-1000)...");
        let results = batch::bench_batch_size_scaling(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[B3] Auto-batching latency (1/10/50 producers)...");
        let results = batch::bench_auto_batching_latency(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[B4] Batched vs unbatched comparison...");
        let results = batch::bench_batched_vs_unbatched(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[B5] Delivery batching throughput (1/10/100 consumers)...");
        let results = batch::bench_delivery_batching_throughput(&server).await;
        for r in results {
            report.add(r);
        }

        eprintln!("[B6] Concurrent producer batching (1/5/10/50 producers)...");
        let results = batch::bench_concurrent_producer_batching(&server).await;
        for r in results {
            report.add(r);
        }
    } else {
        eprintln!(
            "[B1-B6] Batch benchmarks (skipped — set FILA_BENCH_BATCH=1 to enable)"
        );
    }

    // Write report
    report.write_and_print("bench-results.json");
}
