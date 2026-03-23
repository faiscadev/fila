use hdrhistogram::serialization::Serializer as _;
use std::time::{Duration, Instant};

/// Percentile results from an HdrHistogram.
#[derive(Debug, Clone)]
pub struct PercentileSet {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub p99_9: f64,
    pub p99_99: f64,
    pub max: f64,
}

/// Latency recorder backed by HdrHistogram.
///
/// Records durations in microseconds. Provides extended percentiles
/// (p50 through max) and supports histogram merging for multi-run aggregation.
pub struct LatencyHistogram {
    histogram: hdrhistogram::Histogram<u64>,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyHistogram {
    /// Create a new histogram covering 1µs to 60s with 3 significant figures.
    pub fn new() -> Self {
        Self {
            histogram: hdrhistogram::Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
                .expect("create histogram"),
        }
    }

    /// Record a duration sample.
    pub fn record(&mut self, d: Duration) {
        let micros = d.as_micros() as u64;
        // Clamp to histogram bounds (1µs min, 60s max) to avoid recording errors
        let micros = micros.clamp(1, 60_000_000);
        self.histogram.record(micros).ok();
    }

    /// Record a duration with coordinated omission (CO) correction.
    ///
    /// Uses HdrHistogram's `record_correct` to fill in missing samples that
    /// would have been recorded during stalls. `expected_interval` is the
    /// expected time between consecutive requests in an open-loop workload.
    pub fn record_corrected(&mut self, d: Duration, expected_interval: Duration) {
        let micros = d.as_micros() as u64;
        let micros = micros.clamp(1, 60_000_000);
        let interval_micros = expected_interval.as_micros() as u64;
        self.histogram.record_correct(micros, interval_micros).ok();
    }

    /// Number of recorded samples.
    pub fn count(&self) -> u64 {
        self.histogram.len()
    }

    /// Compute all standard percentiles. Returns None if no samples.
    pub fn percentiles(&self) -> Option<PercentileSet> {
        if self.histogram.is_empty() {
            return None;
        }
        Some(PercentileSet {
            p50: self.histogram.value_at_quantile(0.50) as f64,
            p95: self.histogram.value_at_quantile(0.95) as f64,
            p99: self.histogram.value_at_quantile(0.99) as f64,
            p99_9: self.histogram.value_at_quantile(0.999) as f64,
            p99_99: self.histogram.value_at_quantile(0.9999) as f64,
            max: self.histogram.max() as f64,
        })
    }

    /// Merge another histogram into this one.
    pub fn merge(&mut self, other: &LatencyHistogram) {
        self.histogram.add(&other.histogram).ok();
    }

    /// Serialize the histogram to a base64-encoded V2 format for JSON embedding.
    pub fn serialize_base64(&self) -> String {
        let mut buf = Vec::new();
        let mut serializer = hdrhistogram::serialization::V2Serializer::new();
        serializer
            .serialize(&self.histogram, &mut buf)
            .expect("serialize histogram");
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf)
    }

    /// Deserialize a histogram from base64-encoded V2 format.
    pub fn deserialize_base64(encoded: &str) -> Option<Self> {
        let bytes =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded).ok()?;
        let mut deserializer = hdrhistogram::serialization::Deserializer::new();
        let histogram: hdrhistogram::Histogram<u64> =
            deserializer.deserialize(&mut &bytes[..]).ok()?;
        Some(Self { histogram })
    }
}

/// Measure throughput over a sustained window.
pub struct ThroughputMeter {
    start: Instant,
    count: u64,
}

impl ThroughputMeter {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
            count: 0,
        }
    }

    pub fn increment(&mut self) {
        self.count += 1;
    }

    pub fn increment_by(&mut self, n: u64) {
        self.count += n;
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Messages per second.
    pub fn msg_per_sec(&self) -> f64 {
        let secs = self.start.elapsed().as_secs_f64();
        if secs == 0.0 {
            return 0.0;
        }
        self.count as f64 / secs
    }
}

/// Get RSS (Resident Set Size) of the current process in bytes.
pub fn current_process_rss_bytes() -> Option<u64> {
    let sys = sysinfo::System::new_all();
    let pid = sysinfo::Pid::from_u32(std::process::id());
    sys.process(pid).map(|p| p.memory())
}

/// Get RSS of a specific process by PID in bytes.
pub fn process_rss_bytes(pid: u32) -> Option<u64> {
    let mut sys = sysinfo::System::new();
    let sysinfo_pid = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo_pid]), true);
    sys.process(sysinfo_pid).map(|p| p.memory())
}

/// Read the benchmark duration from environment variable, with a default.
pub fn bench_duration_secs() -> u64 {
    std::env::var("FILA_BENCH_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30)
}

/// Minimum number of latency samples required per measurement.
pub const MIN_LATENCY_SAMPLES: u64 = 10_000;
