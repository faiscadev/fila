use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use fila_core::broker::throttle::{ThrottleManager, TokenBucket};
use std::time::Instant;

/// Benchmark a single token bucket try_consume decision.
fn bench_token_bucket_decision(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_bucket");

    // Pure decision cost — bucket starts full, no refill in the measured path
    group.bench_function("try_consume", |b| {
        b.iter_batched(
            || TokenBucket::new(1_000_000.0, 1_000_000.0),
            |mut bucket| black_box(bucket.try_consume(1.0)),
            BatchSize::SmallInput,
        );
    });

    // Combined refill + decision cost
    group.bench_function("refill_and_consume", |b| {
        let mut bucket = TokenBucket::new(1_000_000.0, 1_000_000.0);
        b.iter(|| {
            let now = Instant::now();
            bucket.refill(black_box(now));
            black_box(bucket.try_consume(1.0));
        });
    });

    group.finish();
}

/// Benchmark ThrottleManager.check_keys with varying number of keys.
fn bench_throttle_manager(c: &mut Criterion) {
    let mut group = c.benchmark_group("throttle_manager");

    // Single key check — pure decision cost
    group.bench_function("check_1_key", |b| {
        let keys = vec!["key_0".to_string()];
        b.iter_batched(
            || {
                let mut mgr = ThrottleManager::default();
                mgr.set_rate("key_0", 1_000_000.0, 1_000_000.0);
                mgr
            },
            |mut mgr| black_box(mgr.check_keys(black_box(&keys))),
            BatchSize::SmallInput,
        );
    });

    // 3 keys (typical hierarchical throttling) — pure decision cost
    group.bench_function("check_3_keys", |b| {
        let keys: Vec<String> = (0..3).map(|i| format!("key_{i}")).collect();
        b.iter_batched(
            || {
                let mut mgr = ThrottleManager::default();
                for i in 0..3 {
                    mgr.set_rate(&format!("key_{i}"), 1_000_000.0, 1_000_000.0);
                }
                mgr
            },
            |mut mgr| black_box(mgr.check_keys(black_box(&keys))),
            BatchSize::SmallInput,
        );
    });

    // Refill all with 10 buckets
    group.bench_function("refill_10_buckets", |b| {
        let mut mgr = ThrottleManager::default();
        for i in 0..10 {
            mgr.set_rate(&format!("key_{i}"), 1000.0, 1000.0);
        }
        b.iter(|| {
            let now = Instant::now();
            mgr.refill_all(black_box(now));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_token_bucket_decision, bench_throttle_manager);
criterion_main!(benches);
