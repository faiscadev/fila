use std::time::Instant;

/// Tracks and logs progress for bulk operations.
pub struct Progress {
    label: String,
    total: u64,
    count: u64,
    log_interval: u64,
    start: Instant,
}

impl Progress {
    pub fn new(label: &str, total: u64) -> Self {
        let log_interval = pick_interval(total);
        eprintln!("    {label}: 0/{total}");
        Self {
            label: label.to_string(),
            total,
            count: 0,
            log_interval,
            start: Instant::now(),
        }
    }

    pub fn inc(&mut self) {
        self.count += 1;
        if self.count % self.log_interval == 0 || self.count == self.total {
            let elapsed = self.start.elapsed().as_secs_f64();
            let rate = self.count as f64 / elapsed;
            eprintln!(
                "    {}: {}/{} ({:.0} ops/s)",
                self.label, self.count, self.total, rate
            );
        }
    }

    pub fn finish(&self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        let rate = self.count as f64 / elapsed;
        eprintln!(
            "    {} done: {} ops in {:.1}s ({:.0} ops/s)",
            self.label, self.count, elapsed, rate
        );
    }
}

fn pick_interval(total: u64) -> u64 {
    match total {
        0..=100 => 10,
        101..=1_000 => 100,
        1_001..=10_000 => 1_000,
        10_001..=100_000 => 10_000,
        _ => 100_000,
    }
}
