use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single benchmark result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub name: String,
    pub value: f64,
    pub unit: String,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
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

    /// Write the report to a file and print a human-readable summary to stdout.
    pub fn write_and_print(&self, path: &str) {
        // Write JSON
        std::fs::write(path, self.to_json()).expect("write report file");

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
        println!("========================================\n");
    }
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
