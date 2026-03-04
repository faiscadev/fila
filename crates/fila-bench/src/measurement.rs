use std::time::{Duration, Instant};

/// Collect timing samples and compute percentiles.
pub struct LatencySampler {
    samples: Vec<Duration>,
}

impl Default for LatencySampler {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencySampler {
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
        }
    }

    pub fn record(&mut self, d: Duration) {
        self.samples.push(d);
    }

    pub fn count(&self) -> usize {
        self.samples.len()
    }

    /// Compute a percentile (0.0–1.0) from collected samples.
    /// Returns None if no samples exist.
    pub fn percentile(&mut self, p: f64) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        self.samples.sort();
        let idx = ((p * self.samples.len() as f64) as usize).min(self.samples.len() - 1);
        Some(self.samples[idx])
    }

    /// Return p50, p95, p99 as (Duration, Duration, Duration).
    pub fn percentiles(&mut self) -> Option<(Duration, Duration, Duration)> {
        let p50 = self.percentile(0.50)?;
        let p95 = self.percentile(0.95)?;
        let p99 = self.percentile(0.99)?;
        Some((p50, p95, p99))
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
