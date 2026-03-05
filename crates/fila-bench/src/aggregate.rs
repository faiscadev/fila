use crate::report::{BenchReport, BenchResult};
use std::collections::HashMap;

/// Aggregate multiple benchmark reports by computing the median value per metric.
///
/// This reduces CI environment variance by running the suite multiple times
/// and taking the median of each metric.
pub fn aggregate_reports(reports: &[BenchReport]) -> BenchReport {
    if reports.is_empty() {
        return BenchReport {
            version: "1.0".to_string(),
            timestamp: "unknown".to_string(),
            commit: "unknown".to_string(),
            benchmarks: Vec::new(),
        };
    }

    // Collect all values per metric name
    let mut values_by_name: HashMap<String, Vec<f64>> = HashMap::new();
    let mut units_by_name: HashMap<String, String> = HashMap::new();
    let mut metadata_by_name: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();

    for report in reports {
        for metric in &report.benchmarks {
            values_by_name
                .entry(metric.name.clone())
                .or_default()
                .push(metric.value);
            units_by_name
                .entry(metric.name.clone())
                .or_insert_with(|| metric.unit.clone());
            metadata_by_name
                .entry(metric.name.clone())
                .or_insert_with(|| metric.metadata.clone());
        }
    }

    // Compute median for each metric
    let mut benchmarks: Vec<BenchResult> = values_by_name
        .into_iter()
        .map(|(name, mut values)| {
            values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let median = if values.len() % 2 == 0 {
                let mid = values.len() / 2;
                (values[mid - 1] + values[mid]) / 2.0
            } else {
                values[values.len() / 2]
            };

            let unit = units_by_name.remove(&name).unwrap_or_default();
            let mut metadata = metadata_by_name.remove(&name).unwrap_or_default();
            metadata.insert("runs".to_string(), serde_json::json!(values.len()));

            BenchResult {
                name,
                value: median,
                unit,
                metadata,
            }
        })
        .collect();

    // Sort by name for stable output
    benchmarks.sort_by(|a, b| a.name.cmp(&b.name));

    // Use the last report's metadata for the aggregate
    let last = reports.last().unwrap();
    BenchReport {
        version: last.version.clone(),
        timestamp: last.timestamp.clone(),
        commit: last.commit.clone(),
        benchmarks,
    }
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
    fn median_of_three_odd() {
        let reports = vec![
            make_report(vec![("throughput", 100.0, "msg/s")]),
            make_report(vec![("throughput", 300.0, "msg/s")]),
            make_report(vec![("throughput", 200.0, "msg/s")]),
        ];

        let agg = aggregate_reports(&reports);
        assert_eq!(agg.benchmarks.len(), 1);
        assert_eq!(agg.benchmarks[0].value, 200.0); // median
    }

    #[test]
    fn median_of_two_even() {
        let reports = vec![
            make_report(vec![("throughput", 100.0, "msg/s")]),
            make_report(vec![("throughput", 200.0, "msg/s")]),
        ];

        let agg = aggregate_reports(&reports);
        assert_eq!(agg.benchmarks[0].value, 150.0); // average of middle two
    }

    #[test]
    fn single_report_passthrough() {
        let reports = vec![make_report(vec![("throughput", 42.0, "msg/s")])];

        let agg = aggregate_reports(&reports);
        assert_eq!(agg.benchmarks[0].value, 42.0);
    }

    #[test]
    fn empty_reports() {
        let agg = aggregate_reports(&[]);
        assert!(agg.benchmarks.is_empty());
    }

    #[test]
    fn multiple_metrics_aggregated() {
        let reports = vec![
            make_report(vec![("a", 10.0, "x"), ("b", 20.0, "y")]),
            make_report(vec![("a", 30.0, "x"), ("b", 40.0, "y")]),
            make_report(vec![("a", 20.0, "x"), ("b", 30.0, "y")]),
        ];

        let agg = aggregate_reports(&reports);
        assert_eq!(agg.benchmarks.len(), 2);

        let a = agg.benchmarks.iter().find(|b| b.name == "a").unwrap();
        let b = agg.benchmarks.iter().find(|b| b.name == "b").unwrap();
        assert_eq!(a.value, 20.0);
        assert_eq!(b.value, 30.0);
    }

    #[test]
    fn runs_count_in_metadata() {
        let reports = vec![
            make_report(vec![("throughput", 100.0, "msg/s")]),
            make_report(vec![("throughput", 200.0, "msg/s")]),
            make_report(vec![("throughput", 150.0, "msg/s")]),
        ];

        let agg = aggregate_reports(&reports);
        assert_eq!(agg.benchmarks[0].metadata["runs"], serde_json::json!(3));
    }
}
