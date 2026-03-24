use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single benchmark result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub name: String,
    pub value: f64,
    pub unit: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A complete benchmark report containing all results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    pub version: String,
    pub timestamp: String,
    pub commit: String,
    pub benchmarks: Vec<BenchResult>,
}

impl Default for BenchReport {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchReport {
    pub fn new() -> Self {
        Self {
            version: "1.0".to_string(),
            timestamp: chrono_now(),
            commit: git_commit_hash(),
            benchmarks: Vec::new(),
        }
    }

    pub fn add(&mut self, result: BenchResult) {
        self.benchmarks.push(result);
    }

    /// Serialize the report to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("serialize report")
    }

    /// Convert to github-action-benchmark JSON format.
    ///
    /// Produces two arrays (one per tool type) so that the CI workflow can
    /// invoke the action twice — once for `customSmallerIsBetter` metrics
    /// (latency, overhead, memory) and once for `customBiggerIsBetter`
    /// metrics (throughput).
    ///
    /// See: <https://github.com/benchmark-action/github-action-benchmark>
    pub fn to_gab_json(&self) -> (String, String) {
        let mut smaller: Vec<serde_json::Value> = Vec::new();
        let mut bigger: Vec<serde_json::Value> = Vec::new();

        for b in &self.benchmarks {
            let entry = serde_json::json!({
                "name": b.name,
                "unit": b.unit,
                "value": b.value,
                "extra": format!("commit: {}", self.commit),
            });

            if gab_higher_is_better(&b.name, &b.unit) {
                bigger.push(entry);
            } else {
                smaller.push(entry);
            }
        }

        let smaller_json = serde_json::to_string_pretty(&smaller).expect("serialize gab smaller");
        let bigger_json = serde_json::to_string_pretty(&bigger).expect("serialize gab bigger");

        (smaller_json, bigger_json)
    }

    /// Write the report to a file and print a human-readable summary to stdout.
    ///
    /// Also writes github-action-benchmark format files alongside the main
    /// report: `<stem>-gab-latency.json` (smallerIsBetter) and
    /// `<stem>-gab-throughput.json` (biggerIsBetter).
    pub fn write_and_print(&self, path: &str) {
        // Write JSON
        std::fs::write(path, self.to_json()).expect("write report file");

        // Write GAB format files
        let (smaller_json, bigger_json) = self.to_gab_json();
        let stem = path.strip_suffix(".json").unwrap_or(path);
        let latency_path = format!("{stem}-gab-latency.json");
        let throughput_path = format!("{stem}-gab-throughput.json");
        std::fs::write(&latency_path, smaller_json).expect("write gab latency file");
        std::fs::write(&throughput_path, bigger_json).expect("write gab throughput file");

        // Print human-readable summary
        println!("\n========================================");
        println!("  Fila Benchmark Report");
        println!("  Commit: {}", self.commit);
        println!("  Time:   {}", self.timestamp);
        println!("========================================\n");

        for result in &self.benchmarks {
            println!(
                "  {:<50} {:>12.2} {}",
                result.name, result.value, result.unit
            );
        }
        println!("\n  Results written to: {path}");
        println!("  GAB files: {latency_path}, {throughput_path}");
        println!("========================================\n");
    }
}

/// Determine whether higher values are better for a given metric.
///
/// Mirrors the polarity logic from [`crate::compare::higher_is_better`] but
/// operates on raw name/unit strings so it can be used from the report module
/// without importing compare internals.
fn gab_higher_is_better(name: &str, unit: &str) -> bool {
    // Throughput metrics: higher is better
    if unit == "msg/s"
        || unit == "MB/s"
        || unit == "ops/s"
        || unit == "sel/s"
        || unit == "exec/s"
    {
        return true;
    }

    // Latency, overhead, deviation, memory, per-message time: lower is better
    if unit == "ms"
        || unit == "us"
        || unit == "ns/msg"
        || unit == "% deviation"
        || unit == "%"
        || unit == "MB"
        || unit == "bytes/msg"
    {
        return false;
    }

    // Default: check name patterns
    name.contains("throughput")
}

fn git_commit_hash() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn chrono_now() -> String {
    // Use a simple UTC timestamp without external chrono dependency.
    std::process::Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report(metrics: Vec<(&str, f64, &str)>) -> BenchReport {
        BenchReport {
            version: "1.0".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            commit: "abc1234".to_string(),
            benchmarks: metrics
                .into_iter()
                .map(|(name, value, unit)| BenchResult {
                    name: name.to_string(),
                    value,
                    unit: unit.to_string(),
                    metadata: HashMap::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn gab_splits_by_polarity() {
        let report = make_report(vec![
            ("enqueue_throughput_1kb", 5000.0, "msg/s"),
            ("e2e_latency_p99", 1.5, "ms"),
            ("throughput_MB", 120.0, "MB/s"),
            ("fairness_overhead", 3.2, "%"),
            ("memory_rss", 80.0, "MB"),
            ("storage_bytes_per_msg", 256.0, "bytes/msg"),
        ]);

        let (smaller_json, bigger_json) = report.to_gab_json();
        let smaller: Vec<serde_json::Value> = serde_json::from_str(&smaller_json).unwrap();
        let bigger: Vec<serde_json::Value> = serde_json::from_str(&bigger_json).unwrap();

        // Throughput metrics go to bigger
        assert_eq!(bigger.len(), 2);
        assert_eq!(bigger[0]["name"], "enqueue_throughput_1kb");
        assert_eq!(bigger[1]["name"], "throughput_MB");

        // Latency/overhead/memory go to smaller
        assert_eq!(smaller.len(), 4);
        assert_eq!(smaller[0]["name"], "e2e_latency_p99");
        assert_eq!(smaller[1]["name"], "fairness_overhead");
        assert_eq!(smaller[2]["name"], "memory_rss");
        assert_eq!(smaller[3]["name"], "storage_bytes_per_msg");
    }

    #[test]
    fn gab_entry_has_required_fields() {
        let report = make_report(vec![("test_metric", 42.0, "ms")]);
        let (smaller_json, _) = report.to_gab_json();
        let entries: Vec<serde_json::Value> = serde_json::from_str(&smaller_json).unwrap();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry["name"], "test_metric");
        assert_eq!(entry["unit"], "ms");
        assert_eq!(entry["value"], 42.0);
        assert_eq!(entry["extra"], "commit: abc1234");
    }

    #[test]
    fn gab_empty_report_produces_empty_arrays() {
        let report = make_report(vec![]);
        let (smaller_json, bigger_json) = report.to_gab_json();
        let smaller: Vec<serde_json::Value> = serde_json::from_str(&smaller_json).unwrap();
        let bigger: Vec<serde_json::Value> = serde_json::from_str(&bigger_json).unwrap();
        assert!(smaller.is_empty());
        assert!(bigger.is_empty());
    }

    #[test]
    fn gab_higher_is_better_polarity() {
        // Higher is better
        assert!(gab_higher_is_better("anything", "msg/s"));
        assert!(gab_higher_is_better("anything", "MB/s"));
        assert!(gab_higher_is_better("anything", "ops/s"));
        assert!(gab_higher_is_better("anything", "sel/s"));
        assert!(gab_higher_is_better("anything", "exec/s"));
        assert!(gab_higher_is_better("custom_throughput_metric", "widgets"));

        // Lower is better
        assert!(!gab_higher_is_better("anything", "ms"));
        assert!(!gab_higher_is_better("anything", "us"));
        assert!(!gab_higher_is_better("anything", "ns/msg"));
        assert!(!gab_higher_is_better("anything", "%"));
        assert!(!gab_higher_is_better("anything", "% deviation"));
        assert!(!gab_higher_is_better("anything", "MB"));
        assert!(!gab_higher_is_better("anything", "bytes/msg"));

        // Default (no matching unit, no "throughput" in name)
        assert!(!gab_higher_is_better("some_metric", "widgets"));
    }

    #[test]
    fn write_and_print_creates_gab_files() {
        let dir = std::env::temp_dir().join("fila-bench-test-gab");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bench-results.json");

        let report = make_report(vec![
            ("throughput", 1000.0, "msg/s"),
            ("latency", 5.0, "ms"),
        ]);
        report.write_and_print(path.to_str().unwrap());

        // Main file exists
        assert!(path.exists());

        // GAB files exist
        let latency_path = dir.join("bench-results-gab-latency.json");
        let throughput_path = dir.join("bench-results-gab-throughput.json");
        assert!(latency_path.exists());
        assert!(throughput_path.exists());

        // Validate contents
        let latency: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&latency_path).unwrap()).unwrap();
        let throughput: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&throughput_path).unwrap()).unwrap();
        assert_eq!(latency.len(), 1);
        assert_eq!(latency[0]["name"], "latency");
        assert_eq!(throughput.len(), 1);
        assert_eq!(throughput[0]["name"], "throughput");

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }
}
