use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fila_core::broker::drr::DrrScheduler;

/// Benchmark a full DRR round: start_new_round → next_key/consume_deficit until exhausted.
/// This measures pure scheduling overhead without storage I/O.
fn bench_drr_round(c: &mut Criterion) {
    let mut group = c.benchmark_group("drr_round");

    // 1 key — equivalent to FIFO (baseline)
    group.bench_function("1_key", |b| {
        b.iter(|| {
            let mut drr = DrrScheduler::new(1000);
            drr.add_key("q1", "key_0", 1);
            drr.start_new_round("q1");
            while let Some(key) = drr.next_key("q1") {
                drr.consume_deficit("q1", black_box(&key));
            }
        });
    });

    // 10 keys — typical multi-tenant scenario
    group.bench_function("10_keys", |b| {
        b.iter(|| {
            let mut drr = DrrScheduler::new(1000);
            for i in 0..10 {
                drr.add_key("q1", &format!("key_{i}"), 1);
            }
            drr.start_new_round("q1");
            while let Some(key) = drr.next_key("q1") {
                drr.consume_deficit("q1", black_box(&key));
            }
        });
    });

    // 100 keys — stress test
    group.bench_function("100_keys", |b| {
        b.iter(|| {
            let mut drr = DrrScheduler::new(100);
            for i in 0..100 {
                drr.add_key("q1", &format!("key_{i}"), 1);
            }
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
