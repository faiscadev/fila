use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::KeyValue;

/// Core OTel metrics for the broker. Created once during scheduler init
/// and used to record counters/gauges on each operation.
pub struct Metrics {
    pub messages_enqueued: Counter<u64>,
    pub messages_leased: Counter<u64>,
    pub messages_acked: Counter<u64>,
    pub messages_nacked: Counter<u64>,
    pub queue_depth: Gauge<u64>,
    pub leases_active: Gauge<u64>,
    pub fairness_throughput: Counter<u64>,
    pub drr_rounds: Counter<u64>,
    pub drr_active_keys: Gauge<u64>,
    pub drr_keys_processed: Counter<u64>,
    pub fairness_ratio: Gauge<f64>,
    pub throttle_decisions: Counter<u64>,
    pub throttle_tokens_remaining: Gauge<f64>,
    pub lua_execution_duration: Histogram<f64>,
    pub lua_errors: Counter<u64>,
    pub lua_cb_activations: Counter<u64>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// Create metrics from the global meter provider. If no meter provider
    /// is configured (OTel disabled), the instruments are no-op.
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter("fila");
        Self::from_meter(&meter)
    }

    /// Create metrics from a specific meter (used in tests with in-memory exporter).
    pub fn from_meter(meter: &Meter) -> Self {
        Self {
            messages_enqueued: meter
                .u64_counter("fila.messages.enqueued")
                .with_description("Total messages enqueued")
                .build(),
            messages_leased: meter
                .u64_counter("fila.messages.leased")
                .with_description("Total messages leased to consumers")
                .build(),
            messages_acked: meter
                .u64_counter("fila.messages.acked")
                .with_description("Total messages acknowledged")
                .build(),
            messages_nacked: meter
                .u64_counter("fila.messages.nacked")
                .with_description("Total messages negatively acknowledged")
                .build(),
            queue_depth: meter
                .u64_gauge("fila.queue.depth")
                .with_description("Current queue depth (pending messages)")
                .build(),
            leases_active: meter
                .u64_gauge("fila.leases.active")
                .with_description("Current active leases")
                .build(),
            fairness_throughput: meter
                .u64_counter("fila.fairness.throughput")
                .with_description("Messages delivered per fairness key")
                .build(),
            drr_rounds: meter
                .u64_counter("fila.scheduler.drr.rounds")
                .with_description("DRR scheduling rounds completed")
                .build(),
            drr_active_keys: meter
                .u64_gauge("fila.scheduler.drr.active_keys")
                .with_description("Current active fairness keys per queue")
                .build(),
            drr_keys_processed: meter
                .u64_counter("fila.scheduler.drr.keys_processed")
                .with_description("Fairness keys processed by DRR scheduler")
                .build(),
            fairness_ratio: meter
                .f64_gauge("fila.fairness.fair_share_ratio")
                .with_description("Actual throughput / expected fair share per key (1.0 = fair)")
                .build(),
            throttle_decisions: meter
                .u64_counter("fila.throttle.decisions")
                .with_description("Throttle decisions per key (pass or hit)")
                .build(),
            throttle_tokens_remaining: meter
                .f64_gauge("fila.throttle.tokens_remaining")
                .with_description("Current token count per throttle key")
                .build(),
            lua_execution_duration: meter
                .f64_histogram("fila.lua.execution_duration_us")
                .with_description("Lua hook execution time in microseconds")
                .build(),
            lua_errors: meter
                .u64_counter("fila.lua.errors")
                .with_description("Lua script execution errors")
                .build(),
            lua_cb_activations: meter
                .u64_counter("fila.lua.circuit_breaker.activations")
                .with_description("Lua circuit breaker activations")
                .build(),
        }
    }

    pub fn record_enqueue(&self, queue_id: &str) {
        self.messages_enqueued
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn record_lease(&self, queue_id: &str) {
        self.messages_leased
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn record_ack(&self, queue_id: &str) {
        self.messages_acked
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn record_nack(&self, queue_id: &str) {
        self.messages_nacked
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn set_queue_depth(&self, queue_id: &str, depth: u64) {
        self.queue_depth
            .record(depth, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn set_leases_active(&self, queue_id: &str, count: u64) {
        self.leases_active
            .record(count, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn record_fairness_delivery(&self, queue_id: &str, fairness_key: &str) {
        self.fairness_throughput.add(
            1,
            &[
                KeyValue::new("queue_id", queue_id.to_string()),
                KeyValue::new("fairness_key", fairness_key.to_string()),
            ],
        );
    }

    pub fn record_throttle_decision(&self, throttle_key: &str, result: &str) {
        self.throttle_decisions.add(
            1,
            &[
                KeyValue::new("throttle_key", throttle_key.to_string()),
                KeyValue::new("result", result.to_string()),
            ],
        );
    }

    pub fn record_drr_round(&self, queue_id: &str) {
        self.drr_rounds
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn set_drr_active_keys(&self, queue_id: &str, count: u64) {
        self.drr_active_keys
            .record(count, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn record_drr_key_processed(&self, queue_id: &str) {
        self.drr_keys_processed
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn set_fairness_ratio(&self, queue_id: &str, fairness_key: &str, ratio: f64) {
        self.fairness_ratio.record(
            ratio,
            &[
                KeyValue::new("queue_id", queue_id.to_string()),
                KeyValue::new("fairness_key", fairness_key.to_string()),
            ],
        );
    }

    pub fn set_throttle_tokens(&self, throttle_key: &str, tokens: f64) {
        self.throttle_tokens_remaining.record(
            tokens,
            &[KeyValue::new("throttle_key", throttle_key.to_string())],
        );
    }

    pub fn record_lua_duration(&self, queue_id: &str, hook: &str, duration_us: f64) {
        self.lua_execution_duration.record(
            duration_us,
            &[
                KeyValue::new("queue_id", queue_id.to_string()),
                KeyValue::new("hook", hook.to_string()),
            ],
        );
    }

    pub fn record_lua_error(&self, queue_id: &str) {
        self.lua_errors
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }

    pub fn record_lua_cb_activation(&self, queue_id: &str) {
        self.lua_cb_activations
            .add(1, &[KeyValue::new("queue_id", queue_id.to_string())]);
    }
}

/// Test harness for asserting OTel metrics using an in-memory exporter.
/// Available for reuse by Stories 6.2 and 6.3.
#[cfg(test)]
pub mod test_harness {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
    use opentelemetry_sdk::metrics::in_memory_exporter::InMemoryMetricExporter;
    use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

    use super::Metrics;

    /// A test setup that wires an in-memory exporter to a meter provider,
    /// creating `Metrics` instruments bound to it.
    pub struct MetricTestHarness {
        pub metrics: Metrics,
        pub exporter: InMemoryMetricExporter,
        pub meter_provider: SdkMeterProvider,
    }

    impl Default for MetricTestHarness {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MetricTestHarness {
        pub fn new() -> Self {
            let exporter = InMemoryMetricExporter::default();
            let reader = PeriodicReader::builder(exporter.clone()).build();
            let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
            let meter = meter_provider.meter("fila-test");
            let metrics = Metrics::from_meter(&meter);
            Self {
                metrics,
                exporter,
                meter_provider,
            }
        }

        /// Force-flush the meter provider so all recorded metrics are exported
        /// to the in-memory exporter. Call this before making assertions.
        pub fn flush(&self) {
            self.meter_provider.force_flush().expect("flush failed");
        }

        /// Collect finished metrics from the exporter.
        pub fn finished_metrics(&self) -> Vec<ResourceMetrics> {
            self.exporter
                .get_finished_metrics()
                .expect("failed to get finished metrics")
        }

        /// Assert a u64 counter has the expected value for a given queue_id.
        pub fn assert_counter(&self, metric_name: &str, queue_id: &str, expected: u64) {
            self.flush();
            let metrics = self.finished_metrics();
            let value = counter_value_u64(&metrics, metric_name, queue_id);
            assert_eq!(
                value,
                Some(expected),
                "expected counter {metric_name}[queue_id={queue_id}] = {expected}, got {value:?}"
            );
        }

        /// Assert a u64 gauge has the expected value for a given queue_id.
        pub fn assert_gauge(&self, metric_name: &str, queue_id: &str, expected: u64) {
            self.flush();
            let metrics = self.finished_metrics();
            let value = gauge_value_u64(&metrics, metric_name, queue_id);
            assert_eq!(
                value,
                Some(expected),
                "expected gauge {metric_name}[queue_id={queue_id}] = {expected}, got {value:?}"
            );
        }

        /// Assert a u64 counter with both queue_id and fairness_key labels.
        /// Convenience wrapper around `assert_counter_with_attrs`.
        pub fn assert_counter_with_labels(
            &self,
            metric_name: &str,
            queue_id: &str,
            fairness_key: &str,
            expected: u64,
        ) {
            self.assert_counter_with_attrs(
                metric_name,
                &[
                    KeyValue::new("queue_id", queue_id.to_string()),
                    KeyValue::new("fairness_key", fairness_key.to_string()),
                ],
                expected,
            );
        }

        /// Assert a u64 counter with arbitrary attribute key-value pairs.
        pub fn assert_counter_with_attrs(
            &self,
            metric_name: &str,
            attrs: &[KeyValue],
            expected: u64,
        ) {
            self.flush();
            let metrics = self.finished_metrics();
            let value = counter_value_u64_with_attrs(&metrics, metric_name, attrs);
            assert_eq!(
                value,
                Some(expected),
                "expected counter {metric_name}{attrs:?} = {expected}, got {value:?}"
            );
        }

        /// Assert an f64 histogram has at least `expected_count` recordings.
        pub fn assert_histogram(&self, metric_name: &str, attrs: &[KeyValue], expected_count: u64) {
            self.flush();
            let metrics = self.finished_metrics();
            let count = histogram_count_f64(&metrics, metric_name, attrs);
            assert!(
                count.is_some() && count.unwrap() >= expected_count,
                "expected histogram {metric_name}{attrs:?} count >= {expected_count}, got {count:?}"
            );
        }

        /// Assert an f64 gauge has a value matching the expected within tolerance.
        pub fn assert_gauge_f64(
            &self,
            metric_name: &str,
            attrs: &[KeyValue],
            expected: f64,
            tolerance: f64,
        ) {
            self.flush();
            let metrics = self.finished_metrics();
            let value = gauge_value_f64(&metrics, metric_name, attrs);
            assert!(
                value.is_some(),
                "expected f64 gauge {metric_name}{attrs:?} to exist"
            );
            let v = value.unwrap();
            assert!(
                (v - expected).abs() <= tolerance,
                "expected f64 gauge {metric_name}{attrs:?} ≈ {expected} (±{tolerance}), got {v}"
            );
        }
    }

    /// Extract the u64 counter value for a metric with a specific queue_id attribute.
    fn counter_value_u64(
        resource_metrics: &[ResourceMetrics],
        name: &str,
        queue_id: &str,
    ) -> Option<u64> {
        let expected_attr = KeyValue::new("queue_id", queue_id.to_string());
        for rm in resource_metrics {
            for sm in rm.scope_metrics() {
                for metric in sm.metrics() {
                    if metric.name() == name {
                        if let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() {
                            for dp in sum.data_points() {
                                if dp.attributes().any(|a| *a == expected_attr) {
                                    return Some(dp.value());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract the u64 gauge value for a metric with a specific queue_id attribute.
    fn gauge_value_u64(
        resource_metrics: &[ResourceMetrics],
        name: &str,
        queue_id: &str,
    ) -> Option<u64> {
        let expected_attr = KeyValue::new("queue_id", queue_id.to_string());
        for rm in resource_metrics {
            for sm in rm.scope_metrics() {
                for metric in sm.metrics() {
                    if metric.name() == name {
                        if let AggregatedMetrics::U64(MetricData::Gauge(gauge)) = metric.data() {
                            for dp in gauge.data_points() {
                                if dp.attributes().any(|a| *a == expected_attr) {
                                    return Some(dp.value());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract u64 counter value for a metric matching ALL given attributes.
    fn counter_value_u64_with_attrs(
        resource_metrics: &[ResourceMetrics],
        name: &str,
        attrs: &[KeyValue],
    ) -> Option<u64> {
        for rm in resource_metrics {
            for sm in rm.scope_metrics() {
                for metric in sm.metrics() {
                    if metric.name() == name {
                        if let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() {
                            for dp in sum.data_points() {
                                let dp_attrs: Vec<KeyValue> = dp.attributes().cloned().collect();
                                if attrs.iter().all(|a| dp_attrs.contains(a)) {
                                    return Some(dp.value());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract f64 histogram count for a metric matching ALL given attributes.
    fn histogram_count_f64(
        resource_metrics: &[ResourceMetrics],
        name: &str,
        attrs: &[KeyValue],
    ) -> Option<u64> {
        for rm in resource_metrics {
            for sm in rm.scope_metrics() {
                for metric in sm.metrics() {
                    if metric.name() == name {
                        if let AggregatedMetrics::F64(MetricData::Histogram(hist)) = metric.data() {
                            for dp in hist.data_points() {
                                let dp_attrs: Vec<KeyValue> = dp.attributes().cloned().collect();
                                if attrs.iter().all(|a| dp_attrs.contains(a)) {
                                    return Some(dp.count());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract f64 gauge value for a metric matching ALL given attributes.
    fn gauge_value_f64(
        resource_metrics: &[ResourceMetrics],
        name: &str,
        attrs: &[KeyValue],
    ) -> Option<f64> {
        for rm in resource_metrics {
            for sm in rm.scope_metrics() {
                for metric in sm.metrics() {
                    if metric.name() == name {
                        if let AggregatedMetrics::F64(MetricData::Gauge(gauge)) = metric.data() {
                            for dp in gauge.data_points() {
                                let dp_attrs: Vec<KeyValue> = dp.attributes().cloned().collect();
                                if attrs.iter().all(|a| dp_attrs.contains(a)) {
                                    return Some(dp.value());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn enqueue_counter_increments() {
            let h = MetricTestHarness::new();
            h.metrics.record_enqueue("q1");
            h.metrics.record_enqueue("q1");
            h.metrics.record_enqueue("q2");
            h.assert_counter("fila.messages.enqueued", "q1", 2);
            h.assert_counter("fila.messages.enqueued", "q2", 1);
        }

        #[test]
        fn lease_counter_increments() {
            let h = MetricTestHarness::new();
            h.metrics.record_lease("q1");
            h.metrics.record_lease("q1");
            h.metrics.record_lease("q1");
            h.assert_counter("fila.messages.leased", "q1", 3);
        }

        #[test]
        fn ack_counter_increments() {
            let h = MetricTestHarness::new();
            h.metrics.record_ack("q1");
            h.assert_counter("fila.messages.acked", "q1", 1);
        }

        #[test]
        fn nack_counter_increments() {
            let h = MetricTestHarness::new();
            h.metrics.record_nack("q1");
            h.metrics.record_nack("q1");
            h.assert_counter("fila.messages.nacked", "q1", 2);
        }

        #[test]
        fn queue_depth_gauge_records() {
            let h = MetricTestHarness::new();
            h.metrics.set_queue_depth("q1", 42);
            h.assert_gauge("fila.queue.depth", "q1", 42);
        }

        #[test]
        fn leases_active_gauge_records() {
            let h = MetricTestHarness::new();
            h.metrics.set_leases_active("q1", 5);
            h.assert_gauge("fila.leases.active", "q1", 5);
        }

        #[test]
        fn gauge_overwrites_previous_value() {
            let h = MetricTestHarness::new();
            h.metrics.set_queue_depth("q1", 10);
            h.metrics.set_queue_depth("q1", 3);
            h.assert_gauge("fila.queue.depth", "q1", 3);
        }

        #[test]
        fn counters_are_per_queue() {
            let h = MetricTestHarness::new();
            h.metrics.record_enqueue("alpha");
            h.metrics.record_enqueue("alpha");
            h.metrics.record_enqueue("beta");
            h.metrics.record_lease("alpha");
            h.metrics.record_ack("beta");

            h.assert_counter("fila.messages.enqueued", "alpha", 2);
            h.assert_counter("fila.messages.enqueued", "beta", 1);
            h.assert_counter("fila.messages.leased", "alpha", 1);
            h.assert_counter("fila.messages.acked", "beta", 1);
        }

        #[test]
        fn gauges_are_per_queue() {
            let h = MetricTestHarness::new();
            h.metrics.set_queue_depth("q1", 10);
            h.metrics.set_queue_depth("q2", 20);
            h.metrics.set_leases_active("q1", 3);
            h.metrics.set_leases_active("q2", 7);

            h.assert_gauge("fila.queue.depth", "q1", 10);
            h.assert_gauge("fila.queue.depth", "q2", 20);
            h.assert_gauge("fila.leases.active", "q1", 3);
            h.assert_gauge("fila.leases.active", "q2", 7);
        }

        #[test]
        fn fairness_throughput_counter_per_key() {
            let h = MetricTestHarness::new();
            h.metrics.record_fairness_delivery("q1", "key_a");
            h.metrics.record_fairness_delivery("q1", "key_a");
            h.metrics.record_fairness_delivery("q1", "key_a");
            h.metrics.record_fairness_delivery("q1", "key_b");

            h.assert_counter_with_labels("fila.fairness.throughput", "q1", "key_a", 3);
            h.assert_counter_with_labels("fila.fairness.throughput", "q1", "key_b", 1);
        }

        #[test]
        fn drr_rounds_counter_increments() {
            let h = MetricTestHarness::new();
            h.metrics.record_drr_round("q1");
            h.metrics.record_drr_round("q1");
            h.metrics.record_drr_round("q2");

            h.assert_counter("fila.scheduler.drr.rounds", "q1", 2);
            h.assert_counter("fila.scheduler.drr.rounds", "q2", 1);
        }

        #[test]
        fn drr_active_keys_gauge() {
            let h = MetricTestHarness::new();
            h.metrics.set_drr_active_keys("q1", 3);
            h.metrics.set_drr_active_keys("q2", 0);

            h.assert_gauge("fila.scheduler.drr.active_keys", "q1", 3);
            h.assert_gauge("fila.scheduler.drr.active_keys", "q2", 0);
        }

        #[test]
        fn drr_keys_processed_counter() {
            let h = MetricTestHarness::new();
            h.metrics.record_drr_key_processed("q1");
            h.metrics.record_drr_key_processed("q1");
            h.metrics.record_drr_key_processed("q1");

            h.assert_counter("fila.scheduler.drr.keys_processed", "q1", 3);
        }

        #[test]
        fn fairness_ratio_gauge_records() {
            let h = MetricTestHarness::new();
            h.metrics.set_fairness_ratio("q1", "key_a", 1.0);
            h.metrics.set_fairness_ratio("q1", "key_b", 0.8);

            h.assert_gauge_f64(
                "fila.fairness.fair_share_ratio",
                &[
                    KeyValue::new("queue_id", "q1".to_string()),
                    KeyValue::new("fairness_key", "key_a".to_string()),
                ],
                1.0,
                0.001,
            );
            h.assert_gauge_f64(
                "fila.fairness.fair_share_ratio",
                &[
                    KeyValue::new("queue_id", "q1".to_string()),
                    KeyValue::new("fairness_key", "key_b".to_string()),
                ],
                0.8,
                0.001,
            );
        }

        #[test]
        fn directional_fairness_throughput() {
            // Higher-weight key should get proportionally more deliveries.
            // Simulate: key_a weight=3, key_b weight=1. After many deliveries
            // in a DRR pattern, key_a should have ~3x the throughput.
            let h = MetricTestHarness::new();

            // Simulate DRR delivery pattern: 3 deliveries for key_a, 1 for key_b
            // repeated over 4 rounds
            for _ in 0..4 {
                h.metrics.record_fairness_delivery("q1", "key_a");
                h.metrics.record_fairness_delivery("q1", "key_a");
                h.metrics.record_fairness_delivery("q1", "key_a");
                h.metrics.record_fairness_delivery("q1", "key_b");
            }

            h.assert_counter_with_labels("fila.fairness.throughput", "q1", "key_a", 12);
            h.assert_counter_with_labels("fila.fairness.throughput", "q1", "key_b", 4);

            // Verify directional correctness: key_a got 3x more than key_b
            h.flush();
            let metrics = h.finished_metrics();
            let attrs_a = &[
                KeyValue::new("queue_id", "q1".to_string()),
                KeyValue::new("fairness_key", "key_a".to_string()),
            ];
            let attrs_b = &[
                KeyValue::new("queue_id", "q1".to_string()),
                KeyValue::new("fairness_key", "key_b".to_string()),
            ];
            let a = counter_value_u64_with_attrs(&metrics, "fila.fairness.throughput", attrs_a)
                .unwrap();
            let b = counter_value_u64_with_attrs(&metrics, "fila.fairness.throughput", attrs_b)
                .unwrap();
            assert!(
                a > b,
                "higher-weight key_a should have more deliveries than key_b"
            );
            let ratio = a as f64 / b as f64;
            assert!(
                (ratio - 3.0).abs() < 0.1,
                "key_a/key_b ratio should be ~3.0, got {ratio}"
            );
        }

        #[test]
        fn throttle_decisions_counter_per_key_and_result() {
            let h = MetricTestHarness::new();
            h.metrics.record_throttle_decision("api:tenant-a", "pass");
            h.metrics.record_throttle_decision("api:tenant-a", "pass");
            h.metrics.record_throttle_decision("api:tenant-a", "hit");
            h.metrics.record_throttle_decision("api:tenant-b", "hit");

            h.assert_counter_with_attrs(
                "fila.throttle.decisions",
                &[
                    KeyValue::new("throttle_key", "api:tenant-a"),
                    KeyValue::new("result", "pass"),
                ],
                2,
            );
            h.assert_counter_with_attrs(
                "fila.throttle.decisions",
                &[
                    KeyValue::new("throttle_key", "api:tenant-a"),
                    KeyValue::new("result", "hit"),
                ],
                1,
            );
            h.assert_counter_with_attrs(
                "fila.throttle.decisions",
                &[
                    KeyValue::new("throttle_key", "api:tenant-b"),
                    KeyValue::new("result", "hit"),
                ],
                1,
            );
        }

        #[test]
        fn throttle_tokens_remaining_gauge() {
            let h = MetricTestHarness::new();
            h.metrics.set_throttle_tokens("api:tenant-a", 7.5);
            h.metrics.set_throttle_tokens("api:tenant-b", 0.0);

            h.assert_gauge_f64(
                "fila.throttle.tokens_remaining",
                &[KeyValue::new("throttle_key", "api:tenant-a")],
                7.5,
                0.01,
            );
            h.assert_gauge_f64(
                "fila.throttle.tokens_remaining",
                &[KeyValue::new("throttle_key", "api:tenant-b")],
                0.0,
                0.01,
            );
        }

        #[test]
        fn lua_execution_duration_histogram() {
            let h = MetricTestHarness::new();
            h.metrics.record_lua_duration("q1", "on_enqueue", 150.0);
            h.metrics.record_lua_duration("q1", "on_enqueue", 200.0);
            h.metrics.record_lua_duration("q1", "on_failure", 50.0);

            h.assert_histogram(
                "fila.lua.execution_duration_us",
                &[
                    KeyValue::new("queue_id", "q1"),
                    KeyValue::new("hook", "on_enqueue"),
                ],
                2,
            );
            h.assert_histogram(
                "fila.lua.execution_duration_us",
                &[
                    KeyValue::new("queue_id", "q1"),
                    KeyValue::new("hook", "on_failure"),
                ],
                1,
            );
        }

        #[test]
        fn lua_errors_counter() {
            let h = MetricTestHarness::new();
            h.metrics.record_lua_error("q1");
            h.metrics.record_lua_error("q1");
            h.metrics.record_lua_error("q2");

            h.assert_counter("fila.lua.errors", "q1", 2);
            h.assert_counter("fila.lua.errors", "q2", 1);
        }

        #[test]
        fn lua_circuit_breaker_activations_counter() {
            let h = MetricTestHarness::new();
            h.metrics.record_lua_cb_activation("q1");

            h.assert_counter("fila.lua.circuit_breaker.activations", "q1", 1);
        }
    }
}
