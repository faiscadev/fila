use crate::report::{BenchReport, BenchResult};
use std::collections::HashMap;

/// Status of a metric comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareStatus {
    Improved,
    Regressed,
    Unchanged,
}

/// A single metric comparison between baseline and current.
#[derive(Debug, Clone)]
pub struct MetricComparison {
    pub name: String,
    pub baseline: f64,
    pub current: f64,
    pub change_pct: f64,
    pub unit: String,
    pub status: CompareStatus,
}

/// Result of comparing two benchmark reports.
#[derive(Debug)]
pub struct CompareResult {
    pub comparisons: Vec<MetricComparison>,
    pub has_regressions: bool,
    pub threshold_pct: f64,
}

/// Determine whether higher values are better for a given metric.
///
/// Throughput metrics (msg/s, MB/s) → higher is better.
/// Latency, overhead, deviation, memory metrics → lower is better.
fn higher_is_better(result: &BenchResult) -> bool {
    let name = result.name.as_str();
    let unit = result.unit.as_str();

    // Throughput metrics: higher is better
    if unit == "msg/s" || unit == "MB/s" {
        return true;
    }

    // Latency, overhead, deviation, memory: lower is better
    if unit == "ms" || unit == "us" || unit == "% deviation" || unit == "%" || unit == "MB" {
        return false;
    }

    // bytes/msg (per-message overhead): lower is better
    if unit == "bytes/msg" {
        return false;
    }

    // Default: check name patterns
    name.contains("throughput")
}

/// Compare two benchmark reports and produce a comparison result.
///
/// Returns `CompareResult` with per-metric comparisons and whether any
/// metric regressed beyond the given threshold percentage.
pub fn compare_reports(
    baseline: &BenchReport,
    current: &BenchReport,
    threshold_pct: f64,
) -> CompareResult {
    let baseline_map: HashMap<&str, &BenchResult> = baseline
        .benchmarks
        .iter()
        .map(|b| (b.name.as_str(), b))
        .collect();

    let mut comparisons = Vec::new();
    let mut has_regressions = false;

    for metric in &current.benchmarks {
        let Some(base) = baseline_map.get(metric.name.as_str()) else {
            // New metric not in baseline — report as unchanged (no comparison possible)
            comparisons.push(MetricComparison {
                name: metric.name.clone(),
                baseline: 0.0,
                current: metric.value,
                change_pct: 0.0,
                unit: metric.unit.clone(),
                status: CompareStatus::Unchanged,
            });
            continue;
        };

        let base_val = base.value;
        let curr_val = metric.value;

        // Skip comparison if baseline is zero (avoid division by zero)
        if base_val == 0.0 {
            comparisons.push(MetricComparison {
                name: metric.name.clone(),
                baseline: base_val,
                current: curr_val,
                change_pct: 0.0,
                unit: metric.unit.clone(),
                status: CompareStatus::Unchanged,
            });
            continue;
        }

        let change_pct = (curr_val - base_val) / base_val.abs() * 100.0;
        let hib = higher_is_better(base);

        // For higher-is-better: negative change = regression
        // For lower-is-better: positive change = regression
        let regression_amount = if hib { -change_pct } else { change_pct };

        let status = if regression_amount > threshold_pct {
            has_regressions = true;
            CompareStatus::Regressed
        } else if regression_amount < -threshold_pct {
            CompareStatus::Improved
        } else {
            CompareStatus::Unchanged
        };

        comparisons.push(MetricComparison {
            name: metric.name.clone(),
            baseline: base_val,
            current: curr_val,
            change_pct,
            unit: metric.unit.clone(),
            status,
        });
    }

    CompareResult {
        comparisons,
        has_regressions,
        threshold_pct,
    }
}

/// Print a summary table of the comparison to stdout.
pub fn print_summary(result: &CompareResult) {
    println!("\n========================================");
    println!("  Benchmark Regression Report");
    println!("  Threshold: {:.0}%", result.threshold_pct);
    println!("========================================\n");

    println!(
        "  {:<50} {:>12} {:>12} {:>8}  Status",
        "Metric", "Baseline", "Current", "Change"
    );
    println!("  {}", "-".repeat(95));

    for cmp in &result.comparisons {
        let status_str = match cmp.status {
            CompareStatus::Improved => "IMPROVED",
            CompareStatus::Regressed => "REGRESSED",
            CompareStatus::Unchanged => "ok",
        };
        let change_str = if cmp.baseline == 0.0 {
            "new".to_string()
        } else {
            format!("{:+.1}%", cmp.change_pct)
        };

        println!(
            "  {:<50} {:>12.2} {:>12.2} {:>8}  {}",
            cmp.name, cmp.baseline, cmp.current, change_str, status_str
        );
    }

    let regressed_count = result
        .comparisons
        .iter()
        .filter(|c| c.status == CompareStatus::Regressed)
        .count();
    let improved_count = result
        .comparisons
        .iter()
        .filter(|c| c.status == CompareStatus::Improved)
        .count();

    println!("\n  Summary: {} regressed, {} improved, {} unchanged",
        regressed_count,
        improved_count,
        result.comparisons.len() - regressed_count - improved_count,
    );

    if result.has_regressions {
        println!("\n  FAIL: {} metric(s) regressed beyond {:.0}% threshold",
            regressed_count, result.threshold_pct
        );
    } else {
        println!("\n  PASS: No regressions detected");
    }

    println!("========================================\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report(metrics: Vec<(&str, f64, &str)>) -> BenchReport {
        BenchReport {
            version: "1.0".to_string(),
            timestamp: "test".to_string(),
            commit: "abc".to_string(),
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
    fn no_regression_within_threshold() {
        let baseline = make_report(vec![
            ("enqueue_throughput_1kb", 1000.0, "msg/s"),
            ("e2e_latency_p99_light", 1.0, "ms"),
        ]);
        let current = make_report(vec![
            ("enqueue_throughput_1kb", 950.0, "msg/s"), // -5%, within 10%
            ("e2e_latency_p99_light", 1.05, "ms"),      // +5%, within 10%
        ]);

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(!result.has_regressions);
        assert_eq!(result.comparisons.len(), 2);
        assert_eq!(result.comparisons[0].status, CompareStatus::Unchanged);
        assert_eq!(result.comparisons[1].status, CompareStatus::Unchanged);
    }

    #[test]
    fn throughput_regression_detected() {
        let baseline = make_report(vec![("enqueue_throughput_1kb", 1000.0, "msg/s")]);
        let current = make_report(vec![("enqueue_throughput_1kb", 800.0, "msg/s")]); // -20%

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(result.has_regressions);
        assert_eq!(result.comparisons[0].status, CompareStatus::Regressed);
    }

    #[test]
    fn latency_regression_detected() {
        let baseline = make_report(vec![("e2e_latency_p99_light", 1.0, "ms")]);
        let current = make_report(vec![("e2e_latency_p99_light", 1.5, "ms")]); // +50%

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(result.has_regressions);
        assert_eq!(result.comparisons[0].status, CompareStatus::Regressed);
    }

    #[test]
    fn throughput_improvement_detected() {
        let baseline = make_report(vec![("enqueue_throughput_1kb", 1000.0, "msg/s")]);
        let current = make_report(vec![("enqueue_throughput_1kb", 1200.0, "msg/s")]); // +20%

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(!result.has_regressions);
        assert_eq!(result.comparisons[0].status, CompareStatus::Improved);
    }

    #[test]
    fn latency_improvement_detected() {
        let baseline = make_report(vec![("e2e_latency_p99_light", 1.0, "ms")]);
        let current = make_report(vec![("e2e_latency_p99_light", 0.5, "ms")]); // -50%

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(!result.has_regressions);
        assert_eq!(result.comparisons[0].status, CompareStatus::Improved);
    }

    #[test]
    fn new_metric_reported_as_unchanged() {
        let baseline = make_report(vec![("enqueue_throughput_1kb", 1000.0, "msg/s")]);
        let current = make_report(vec![
            ("enqueue_throughput_1kb", 1000.0, "msg/s"),
            ("new_metric", 42.0, "things"),
        ]);

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(!result.has_regressions);
        assert_eq!(result.comparisons.len(), 2);
        assert_eq!(result.comparisons[1].status, CompareStatus::Unchanged);
    }

    #[test]
    fn zero_baseline_skipped() {
        let baseline = make_report(vec![("consumer_concurrency_100_throughput", 0.0, "msg/s")]);
        let current = make_report(vec![("consumer_concurrency_100_throughput", 100.0, "msg/s")]);

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(!result.has_regressions);
        assert_eq!(result.comparisons[0].status, CompareStatus::Unchanged);
    }

    #[test]
    fn overhead_regression_detected() {
        let baseline = make_report(vec![("fairness_overhead_pct", 3.0, "%")]);
        let current = make_report(vec![("fairness_overhead_pct", 8.0, "%")]); // +167% worse

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(result.has_regressions);
        assert_eq!(result.comparisons[0].status, CompareStatus::Regressed);
    }

    #[test]
    fn memory_regression_detected() {
        let baseline = make_report(vec![("memory_rss_loaded_10k", 100.0, "MB")]);
        let current = make_report(vec![("memory_rss_loaded_10k", 130.0, "MB")]); // +30%

        let result = compare_reports(&baseline, &current, 10.0);
        assert!(result.has_regressions);
        assert_eq!(result.comparisons[0].status, CompareStatus::Regressed);
    }
}
