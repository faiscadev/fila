use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fila_core::broker::drr::DrrScheduler;
use lasso::ThreadedRodeo;

/// Benchmark steady-state DRR round throughput: pre-build the scheduler,
/// then measure only the round execution (start_new_round + next_key/consume_deficit).
/// Uses consistent quantum=1000 across all key counts so total iterations scale linearly.
fn bench_drr_round(c: &mut Criterion) {
    let mut group = c.benchmark_group("drr_round");
    let interner = ThreadedRodeo::default();

    // 1 key — equivalent to FIFO (baseline): 1000 iterations per round
    group.bench_function("1_key_q1000", |b| {
        let q1 = interner.get_or_intern("q1");
        let key_0 = interner.get_or_intern("key_0");
        let mut drr = DrrScheduler::new(1000);
        drr.add_key(q1, key_0, 1);
        b.iter(|| {
            drr.start_new_round(q1);
            while let Some(key) = drr.next_key(q1) {
                drr.consume_deficit(q1, black_box(key));
            }
        });
    });

    // 10 keys — typical multi-tenant: 10,000 iterations per round
    group.bench_function("10_keys_q1000", |b| {
        let q1 = interner.get_or_intern("q1_10");
        let mut drr = DrrScheduler::new(1000);
        let keys: Vec<_> = (0..10)
            .map(|i| interner.get_or_intern(&format!("key_10_{i}")))
            .collect();
        for &k in &keys {
            drr.add_key(q1, k, 1);
        }
        b.iter(|| {
            drr.start_new_round(q1);
            while let Some(key) = drr.next_key(q1) {
                drr.consume_deficit(q1, black_box(key));
            }
        });
    });

    // 100 keys — stress test: 100,000 iterations per round
    group.bench_function("100_keys_q1000", |b| {
        let q1 = interner.get_or_intern("q1_100");
        let mut drr = DrrScheduler::new(1000);
        let keys: Vec<_> = (0..100)
            .map(|i| interner.get_or_intern(&format!("key_100_{i}")))
            .collect();
        for &k in &keys {
            drr.add_key(q1, k, 1);
        }
        b.iter(|| {
            drr.start_new_round(q1);
            while let Some(key) = drr.next_key(q1) {
                drr.consume_deficit(q1, black_box(key));
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_drr_round);
criterion_main!(benches);
