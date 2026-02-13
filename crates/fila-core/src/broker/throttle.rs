use std::collections::HashMap;
use std::time::Instant;

/// A token bucket rate limiter for a single throttle key.
///
/// Tokens are refilled continuously based on elapsed time since the last
/// refill. The bucket never exceeds the burst capacity.
///
/// Runs on the single-threaded scheduler — no synchronization needed.
pub struct TokenBucket {
    tokens: f64,
    rate_per_second: f64,
    burst: f64,
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket starting at full capacity.
    /// Negative values are clamped to 0.0.
    pub fn new(rate_per_second: f64, burst: f64) -> Self {
        let rate = rate_per_second.max(0.0);
        let burst = burst.max(0.0);
        Self {
            tokens: burst,
            rate_per_second: rate,
            burst,
            last_refill: Instant::now(),
        }
    }

    /// Create a token bucket with a specific start time (for testing).
    #[cfg(test)]
    fn with_time(rate_per_second: f64, burst: f64, now: Instant) -> Self {
        let rate = rate_per_second.max(0.0);
        let burst = burst.max(0.0);
        Self {
            tokens: burst,
            rate_per_second: rate,
            burst,
            last_refill: now,
        }
    }

    /// Refill tokens based on elapsed time since last refill.
    /// Tokens are capped at the burst size.
    pub fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.rate_per_second).min(self.burst);
            self.last_refill = now;
        }
    }

    /// Try to consume `n` tokens. Returns true and decrements if sufficient
    /// tokens are available; returns false without modification otherwise.
    pub fn try_consume(&mut self, n: f64) -> bool {
        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }

    /// Current token count (for inspection/metrics).
    pub fn tokens(&self) -> f64 {
        self.tokens
    }

    /// Update the rate and burst. Preserves current tokens, clamped to new burst.
    /// Negative values are clamped to 0.0.
    pub fn update(&mut self, rate_per_second: f64, burst: f64) {
        self.rate_per_second = rate_per_second.max(0.0);
        self.burst = burst.max(0.0);
        self.tokens = self.tokens.min(self.burst);
    }
}

/// Manages token buckets for all throttle keys. Keys without a configured
/// bucket are treated as unthrottled (always pass).
///
/// Intended to be owned by the single-threaded scheduler.
pub struct ThrottleManager {
    buckets: HashMap<String, TokenBucket>,
}

impl ThrottleManager {
    pub fn new() -> Self {
        Self {
            buckets: HashMap::new(),
        }
    }

    /// Create or update a token bucket for the given throttle key.
    /// If the key already exists, the rate and burst are updated but
    /// current tokens are preserved (clamped to new burst).
    pub fn set_rate(&mut self, key: &str, rate_per_second: f64, burst: f64) {
        match self.buckets.get_mut(key) {
            Some(bucket) => bucket.update(rate_per_second, burst),
            None => {
                self.buckets
                    .insert(key.to_string(), TokenBucket::new(rate_per_second, burst));
            }
        }
    }

    /// Remove the token bucket for a throttle key. The key becomes unthrottled.
    pub fn remove_rate(&mut self, key: &str) {
        self.buckets.remove(key);
    }

    /// Refill all token buckets. Call once per scheduler loop iteration.
    pub fn refill_all(&mut self, now: Instant) {
        for bucket in self.buckets.values_mut() {
            bucket.refill(now);
        }
    }

    /// Check whether all throttle keys have available tokens.
    ///
    /// - Keys without a configured bucket are treated as unthrottled (pass).
    /// - If ALL configured keys have ≥1 token, consumes 1 from each and returns true.
    /// - If ANY configured key is exhausted, consumes nothing and returns false.
    pub fn check_keys(&mut self, keys: &[String]) -> bool {
        // First pass: verify all configured keys have tokens
        for key in keys {
            if let Some(bucket) = self.buckets.get(key) {
                if bucket.tokens() < 1.0 {
                    return false;
                }
            }
            // Key not in manager → unthrottled, passes
        }

        // Second pass: consume 1 token from each configured key
        for key in keys {
            if let Some(bucket) = self.buckets.get_mut(key) {
                bucket.try_consume(1.0);
            }
        }

        true
    }

    /// Returns true if the manager has a bucket for the given key.
    pub fn has_key(&self, key: &str) -> bool {
        self.buckets.contains_key(key)
    }

    /// Number of configured buckets.
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    /// Whether the manager has no configured buckets.
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

impl Default for ThrottleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── TokenBucket tests ──────────────────────────────────────────────

    #[test]
    fn token_bucket_starts_full() {
        let bucket = TokenBucket::new(10.0, 20.0);
        assert_eq!(bucket.tokens(), 20.0);
    }

    #[test]
    fn token_bucket_consume_success() {
        let mut bucket = TokenBucket::new(10.0, 20.0);
        assert!(bucket.try_consume(5.0));
        assert_eq!(bucket.tokens(), 15.0);
    }

    #[test]
    fn token_bucket_consume_insufficient() {
        let mut bucket = TokenBucket::new(10.0, 5.0);
        assert!(!bucket.try_consume(6.0));
        assert_eq!(bucket.tokens(), 5.0); // unchanged
    }

    #[test]
    fn token_bucket_consume_exact() {
        let mut bucket = TokenBucket::new(10.0, 5.0);
        assert!(bucket.try_consume(5.0));
        assert_eq!(bucket.tokens(), 0.0);
        assert!(!bucket.try_consume(1.0));
    }

    #[test]
    fn token_bucket_refill_timing() {
        let now = Instant::now();
        let mut bucket = TokenBucket::with_time(10.0, 20.0, now);

        // Drain all tokens
        bucket.try_consume(20.0);
        assert_eq!(bucket.tokens(), 0.0);

        // Refill after 1 second at rate 10/s → 10 tokens
        bucket.refill(now + Duration::from_secs(1));
        assert!((bucket.tokens() - 10.0).abs() < 0.001);
    }

    #[test]
    fn token_bucket_refill_capped_at_burst() {
        let now = Instant::now();
        let mut bucket = TokenBucket::with_time(100.0, 20.0, now);

        // Already full, refill after 1 second should stay at burst
        bucket.refill(now + Duration::from_secs(1));
        assert_eq!(bucket.tokens(), 20.0);
    }

    #[test]
    fn token_bucket_refill_partial_second() {
        let now = Instant::now();
        let mut bucket = TokenBucket::with_time(10.0, 20.0, now);

        bucket.try_consume(20.0);
        // Refill after 500ms at rate 10/s → 5 tokens
        bucket.refill(now + Duration::from_millis(500));
        assert!((bucket.tokens() - 5.0).abs() < 0.001);
    }

    #[test]
    fn token_bucket_rate_accuracy_one_second() {
        // Over 1 second at rate 100/s, we should be able to consume ~100 tokens
        let now = Instant::now();
        let mut bucket = TokenBucket::with_time(100.0, 100.0, now);

        // Drain completely
        bucket.try_consume(100.0);
        assert_eq!(bucket.tokens(), 0.0);

        // Refill after exactly 1 second
        bucket.refill(now + Duration::from_secs(1));
        assert!((bucket.tokens() - 100.0).abs() < 0.001);

        // Should be able to consume all 100
        let mut consumed = 0;
        while bucket.try_consume(1.0) {
            consumed += 1;
        }
        assert_eq!(consumed, 100);
    }

    #[test]
    fn token_bucket_update_preserves_tokens() {
        let mut bucket = TokenBucket::new(10.0, 20.0);
        bucket.try_consume(10.0); // 10 tokens left
        bucket.update(50.0, 100.0);
        assert_eq!(bucket.tokens(), 10.0); // preserved
    }

    #[test]
    fn token_bucket_update_clamps_tokens_to_new_burst() {
        let mut bucket = TokenBucket::new(10.0, 20.0);
        // 20 tokens, reduce burst to 5
        bucket.update(10.0, 5.0);
        assert_eq!(bucket.tokens(), 5.0); // clamped
    }

    #[test]
    fn token_bucket_zero_rate_never_refills() {
        let now = Instant::now();
        let mut bucket = TokenBucket::with_time(0.0, 10.0, now);
        bucket.try_consume(10.0);
        assert_eq!(bucket.tokens(), 0.0);

        // After 1 second at rate 0/s → still 0 tokens
        bucket.refill(now + Duration::from_secs(1));
        assert_eq!(bucket.tokens(), 0.0);
    }

    #[test]
    fn token_bucket_negative_values_clamped_to_zero() {
        let bucket = TokenBucket::new(-5.0, -10.0);
        assert_eq!(bucket.tokens(), 0.0);
        // rate and burst clamped to 0
    }

    // ── ThrottleManager tests ──────────────────────────────────────────

    #[test]
    fn throttle_manager_create_and_remove() {
        let mut mgr = ThrottleManager::new();
        assert!(mgr.is_empty());

        mgr.set_rate("key_a", 10.0, 10.0);
        assert_eq!(mgr.len(), 1);
        assert!(mgr.has_key("key_a"));

        mgr.remove_rate("key_a");
        assert!(mgr.is_empty());
        assert!(!mgr.has_key("key_a"));
    }

    #[test]
    fn throttle_manager_update_existing() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 10.0, 20.0);

        // Consume some tokens
        let keys = vec!["key_a".to_string()];
        mgr.check_keys(&keys);
        mgr.check_keys(&keys);
        // 18 tokens left

        // Update rate — tokens preserved (clamped)
        mgr.set_rate("key_a", 50.0, 100.0);
        // 18 tokens still, new burst=100
        let keys = vec!["key_a".to_string()];
        assert!(mgr.check_keys(&keys)); // should pass (17 tokens after)
    }

    #[test]
    fn throttle_manager_check_keys_all_pass() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 10.0, 10.0);
        mgr.set_rate("key_b", 10.0, 10.0);

        let keys = vec!["key_a".to_string(), "key_b".to_string()];
        assert!(mgr.check_keys(&keys));
    }

    #[test]
    fn throttle_manager_check_keys_one_exhausted() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 10.0, 10.0);
        mgr.set_rate("key_b", 10.0, 1.0); // only 1 token

        let keys = vec!["key_a".to_string(), "key_b".to_string()];

        // First check: both have tokens → passes, consumes 1 from each
        assert!(mgr.check_keys(&keys));
        // key_a: 9 tokens, key_b: 0 tokens

        // Second check: key_b exhausted → fails, nothing consumed
        assert!(!mgr.check_keys(&keys));
    }

    #[test]
    fn throttle_manager_check_keys_no_partial_drain() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 10.0, 10.0);
        mgr.set_rate("key_b", 10.0, 0.5); // starts with 0.5 — below 1.0

        let keys = vec!["key_a".to_string(), "key_b".to_string()];

        // key_b has < 1.0 tokens → should fail, key_a should NOT be drained
        assert!(!mgr.check_keys(&keys));

        // Verify key_a was not consumed (still 10.0)
        let only_a = vec!["key_a".to_string()];
        assert!(mgr.check_keys(&only_a)); // 9 after
        assert!(mgr.check_keys(&only_a)); // 8 after
                                          // If key_a had been partially drained, this count would be off
    }

    #[test]
    fn throttle_manager_unknown_key_passes() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 10.0, 10.0);

        // key_b not configured → unthrottled
        let keys = vec!["key_a".to_string(), "key_b".to_string()];
        assert!(mgr.check_keys(&keys));
    }

    #[test]
    fn throttle_manager_empty_keys_passes() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 10.0, 10.0);

        // No throttle keys on message → always passes
        let keys: Vec<String> = vec![];
        assert!(mgr.check_keys(&keys));
    }

    #[test]
    fn throttle_manager_refill_all() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 100.0, 10.0);
        mgr.set_rate("key_b", 100.0, 10.0);

        // Drain both
        let keys_a = vec!["key_a".to_string()];
        let keys_b = vec!["key_b".to_string()];
        for _ in 0..10 {
            mgr.check_keys(&keys_a);
            mgr.check_keys(&keys_b);
        }

        // Both exhausted
        assert!(!mgr.check_keys(&keys_a));
        assert!(!mgr.check_keys(&keys_b));

        // Refill (simulate time passing)
        let later = Instant::now() + Duration::from_millis(200);
        mgr.refill_all(later);

        // After refill at 100/s for 200ms → ~20 tokens each (capped at burst=10)
        assert!(mgr.check_keys(&keys_a));
        assert!(mgr.check_keys(&keys_b));
    }

    #[test]
    fn throttle_manager_remove_makes_key_unthrottled() {
        let mut mgr = ThrottleManager::new();
        mgr.set_rate("key_a", 10.0, 1.0);

        let keys = vec!["key_a".to_string()];

        // First check passes (1 token)
        assert!(mgr.check_keys(&keys));
        // Second check fails (0 tokens)
        assert!(!mgr.check_keys(&keys));

        // Remove the key
        mgr.remove_rate("key_a");

        // Now the key is unthrottled — always passes
        assert!(mgr.check_keys(&keys));
    }
}
