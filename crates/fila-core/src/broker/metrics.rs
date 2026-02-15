use opentelemetry::metrics::{Counter, Gauge, Meter};
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

    impl MetricTestHarness {
        pub fn new() -> Self {
            let exporter = InMemoryMetricExporter::default();
            let reader = PeriodicReader::builder(exporter.clone()).build();
            let meter_provider = SdkMeterProvider::builder()
                .with_reader(reader)
                .build();
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
    }
}
