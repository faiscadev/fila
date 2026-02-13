use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fila_core::broker::throttle::{ThrottleManager, TokenBucket};
use std::time::Instant;

/// Benchmark a single token bucket try_consume decision.
fn bench_token_bucket_decision(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_bucket");

    group.bench_function("try_consume", |b| {
        let mut bucket = TokenBucket::new(1_000_000.0, 1_000_000.0);
        b.iter(|| {
            // Refill to full before each consume so we always measure the success path
            let now = Instant::now();
            bucket.refill(black_box(now));
            black_box(bucket.try_consume(1.0));
        });
    });

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

    // Single key check
    group.bench_function("check_1_key", |b| {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_0", 1_000_000.0, 1_000_000.0);
        let keys = vec!["key_0".to_string()];
        b.iter(|| {
            // Refill before each check so we always measure the success path
            mgr.refill_all(Instant::now());
            black_box(mgr.check_keys(black_box(&keys)));
        });
    });

    // 3 keys (typical hierarchical throttling)
    group.bench_function("check_3_keys", |b| {
        let mut mgr = ThrottleManager::new();
        for i in 0..3 {
            mgr.set_rate(&format!("key_{i}"), 1_000_000.0, 1_000_000.0);
        }
        let keys: Vec<String> = (0..3).map(|i| format!("key_{i}")).collect();
        b.iter(|| {
            // Refill before each check so we always measure the success path
            mgr.refill_all(Instant::now());
            black_box(mgr.check_keys(black_box(&keys)));
        });
    });

    // Refill all with 10 buckets
    group.bench_function("refill_10_buckets", |b| {
        let mut mgr = ThrottleManager::new();
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
