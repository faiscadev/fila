use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fila_core::broker::drr::DrrScheduler;

/// Benchmark steady-state DRR round throughput: pre-build the scheduler,
/// then measure only the round execution (start_new_round + next_key/consume_deficit).
/// Uses consistent quantum=1000 across all key counts so total iterations scale linearly.
fn bench_drr_round(c: &mut Criterion) {
    let mut group = c.benchmark_group("drr_round");

    // 1 key — equivalent to FIFO (baseline): 1000 iterations per round
    group.bench_function("1_key_q1000", |b| {
        let mut drr = DrrScheduler::new(1000);
        drr.add_key("q1", "key_0", 1);
        b.iter(|| {
            drr.start_new_round("q1");
            while let Some(key) = drr.next_key("q1") {
                drr.consume_deficit("q1", black_box(&key));
            }
        });
    });

    // 10 keys — typical multi-tenant: 10,000 iterations per round
    group.bench_function("10_keys_q1000", |b| {
        let mut drr = DrrScheduler::new(1000);
        for i in 0..10 {
            drr.add_key("q1", &format!("key_{i}"), 1);
        }
        b.iter(|| {
            drr.start_new_round("q1");
            while let Some(key) = drr.next_key("q1") {
                drr.consume_deficit("q1", black_box(&key));
            }
        });
    });

    // 100 keys — stress test: 100,000 iterations per round
    group.bench_function("100_keys_q1000", |b| {
        let mut drr = DrrScheduler::new(1000);
        for i in 0..100 {
            drr.add_key("q1", &format!("key_{i}"), 1);
        }
        b.iter(|| {
            drr.start_new_round("q1");
            while let Some(key) = drr.next_key("q1") {
                drr.consume_deficit("q1", black_box(&key));
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_drr_round);
criterion_main!(benches);
