use crate::measurement::LatencyHistogram;
use crate::report::{BenchReport, BenchResult};
use std::collections::HashMap;

/// Check if a metric is a latency metric that carries a serialized histogram.
fn has_histogram(result: &BenchResult) -> bool {
    result
        .metadata
        .get("histogram")
        .and_then(|v| v.as_str())
        .is_some()
}

/// Aggregate multiple benchmark reports.
///
/// For latency metrics (those with a serialized histogram in metadata),
/// merges histograms via `Histogram::add()` and recomputes percentiles
/// from the merged histogram. This is statistically correct — unlike
/// median-of-percentiles which loses distribution information.
///
/// For non-latency metrics (throughput, memory, etc.), computes the
/// median value across runs.
pub fn aggregate_reports(reports: &[BenchReport]) -> BenchReport {
    if reports.is_empty() {
        return BenchReport {
            version: "1.0".to_string(),
            timestamp: "unknown".to_string(),
            commit: "unknown".to_string(),
            benchmarks: Vec::new(),
        };
    }

    // Group metrics by base name (strip percentile suffix for latency metrics)
    // For latency: merge histograms, then emit all percentile variants
    // For non-latency: median of scalar values

    let mut scalar_values: HashMap<String, Vec<f64>> = HashMap::new();
    let mut scalar_units: HashMap<String, String> = HashMap::new();
    let mut scalar_metadata: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();

    let mut histogram_groups: HashMap<String, Vec<String>> = HashMap::new(); // base_name -> [histogram_b64]
    let mut histogram_units: HashMap<String, String> = HashMap::new();
    let mut histogram_extra_meta: HashMap<String, HashMap<String, serde_json::Value>> =
        HashMap::new();

    for report in reports {
        for metric in &report.benchmarks {
            if has_histogram(metric) {
                let histogram_b64 = metric.metadata["histogram"].as_str().unwrap().to_string();
                // Use a base name that strips the percentile suffix to group related metrics
                let base_name = extract_latency_base_name(&metric.name);
                histogram_groups
                    .entry(base_name.clone())
                    .or_default()
                    .push(histogram_b64);
                histogram_units
                    .entry(base_name.clone())
                    .or_insert_with(|| metric.unit.clone());
                // Preserve non-histogram metadata from first occurrence
                histogram_extra_meta.entry(base_name).or_insert_with(|| {
                    let mut meta = metric.metadata.clone();
                    meta.remove("histogram");
                    meta.remove("samples");
                    meta
                });
            } else {
                scalar_values
                    .entry(metric.name.clone())
                    .or_default()
                    .push(metric.value);
                scalar_units
                    .entry(metric.name.clone())
                    .or_insert_with(|| metric.unit.clone());
                scalar_metadata
                    .entry(metric.name.clone())
                    .or_insert_with(|| metric.metadata.clone());
            }
        }
    }

    let mut benchmarks: Vec<BenchResult> = Vec::new();

    // Process histogram groups: merge and emit all percentiles
    for (base_name, histograms_b64) in &histogram_groups {
        // Deduplicate: each run has multiple metrics (p50, p95, ...) sharing the same histogram.
        // We only need to merge one histogram per run.
        let unique_histograms: Vec<&str> = {
            let mut seen = Vec::new();
            let runs = histograms_b64.len() / 6; // 6 percentile metrics per histogram
            let runs = runs.max(1);
            for i in 0..runs {
                let idx = i * 6;
                if idx < histograms_b64.len() && !seen.contains(&histograms_b64[idx].as_str()) {
                    seen.push(histograms_b64[idx].as_str());
                }
            }
            if seen.is_empty() && !histograms_b64.is_empty() {
                seen.push(histograms_b64[0].as_str());
            }
            seen
        };

        let mut merged = LatencyHistogram::new();
        for h_b64 in &unique_histograms {
            if let Some(h) = LatencyHistogram::deserialize_base64(h_b64) {
                merged.merge(&h);
            }
        }

        if let Some(pcts) = merged.percentiles() {
            let unit = histogram_units.get(base_name).cloned().unwrap_or_default();
            let mut meta = histogram_extra_meta
                .get(base_name)
                .cloned()
                .unwrap_or_default();
            meta.insert(
                "runs".to_string(),
                serde_json::json!(unique_histograms.len()),
            );
            meta.insert("samples".to_string(), serde_json::json!(merged.count()));
            meta.insert(
                "histogram".to_string(),
                serde_json::json!(merged.serialize_base64()),
            );

            for (label, value_us) in [
                ("p50", pcts.p50),
                ("p95", pcts.p95),
                ("p99", pcts.p99),
                ("p99_9", pcts.p99_9),
                ("p99_99", pcts.p99_99),
                ("max", pcts.max),
            ] {
                benchmarks.push(BenchResult {
                    name: format!("{base_name}_{label}"),
                    value: value_us / 1000.0,
                    unit: unit.clone(),
                    metadata: meta.clone(),
                });
            }
        }
    }

    // Process scalar metrics: median aggregation
    for (name, mut values) in scalar_values {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = if values.len() % 2 == 0 {
            let mid = values.len() / 2;
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[values.len() / 2]
        };

        let unit = scalar_units.remove(&name).unwrap_or_default();
        let mut metadata = scalar_metadata.remove(&name).unwrap_or_default();
        metadata.insert("runs".to_string(), serde_json::json!(values.len()));

        benchmarks.push(BenchResult {
            name,
            value: median,
            unit,
            metadata,
        });
    }

    // Sort by name for stable output
    benchmarks.sort_by(|a, b| a.name.cmp(&b.name));

    let last = reports.last().unwrap();
    BenchReport {
        version: last.version.clone(),
        timestamp: last.timestamp.clone(),
        commit: last.commit.clone(),
        benchmarks,
    }
}

/// Extract the base name from a latency metric name by stripping the percentile suffix.
///
/// Examples:
///   "e2e_latency_p50_light" -> "e2e_latency_light"
///   "compaction_idle_p99_9_enqueue" -> "compaction_idle_enqueue"
///   "kafka_latency_p99" -> "kafka_latency"
fn extract_latency_base_name(name: &str) -> String {
    // Percentile suffixes we recognize (longest first to match greedily)
    let suffixes = ["_p99_99_", "_p99_9_", "_p99_", "_p95_", "_p50_", "_max_"];
    let terminal_suffixes = ["_p99_99", "_p99_9", "_p99", "_p95", "_p50", "_max"];

    // Try mid-string suffixes first (e.g., "e2e_latency_p50_light")
    for suffix in &suffixes {
        if let Some(pos) = name.find(suffix) {
            let before = &name[..pos];
            let after = &name[pos + suffix.len()..];
            return format!("{before}_{after}");
        }
    }

    // Try terminal suffixes (e.g., "kafka_latency_p50")
    for suffix in &terminal_suffixes {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }

    name.to_string()
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
        assert_eq!(agg.benchmarks[0].value, 200.0);
    }

    #[test]
    fn median_of_two_even() {
        let reports = vec![
            make_report(vec![("throughput", 100.0, "msg/s")]),
            make_report(vec![("throughput", 200.0, "msg/s")]),
        ];

        let agg = aggregate_reports(&reports);
        assert_eq!(agg.benchmarks[0].value, 150.0);
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

    #[test]
    fn histogram_merging_for_latency() {
        // Create two reports with histogram metadata
        let mut h1 = LatencyHistogram::new();
        let mut h2 = LatencyHistogram::new();
        for i in 0..1000 {
            h1.record(std::time::Duration::from_micros(100 + i));
            h2.record(std::time::Duration::from_micros(200 + i));
        }

        let h1_b64 = h1.serialize_base64();
        let h2_b64 = h2.serialize_base64();

        let make_latency_report = |h_b64: &str, base_value: f64| -> BenchReport {
            let meta: HashMap<String, serde_json::Value> = [
                ("histogram".to_string(), serde_json::json!(h_b64)),
                ("samples".to_string(), serde_json::json!(1000)),
            ]
            .into_iter()
            .collect();

            BenchReport {
                version: "1.0".to_string(),
                timestamp: "test".to_string(),
                commit: "abc".to_string(),
                benchmarks: vec![
                    BenchResult {
                        name: "test_latency_p50_light".to_string(),
                        value: base_value,
                        unit: "ms".to_string(),
                        metadata: meta.clone(),
                    },
                    BenchResult {
                        name: "test_latency_p99_light".to_string(),
                        value: base_value * 2.0,
                        unit: "ms".to_string(),
                        metadata: meta.clone(),
                    },
                    BenchResult {
                        name: "test_latency_p99_9_light".to_string(),
                        value: base_value * 3.0,
                        unit: "ms".to_string(),
                        metadata: meta.clone(),
                    },
                    BenchResult {
                        name: "test_latency_p99_99_light".to_string(),
                        value: base_value * 4.0,
                        unit: "ms".to_string(),
                        metadata: meta.clone(),
                    },
                    BenchResult {
                        name: "test_latency_p95_light".to_string(),
                        value: base_value * 1.5,
                        unit: "ms".to_string(),
                        metadata: meta.clone(),
                    },
                    BenchResult {
                        name: "test_latency_max_light".to_string(),
                        value: base_value * 5.0,
                        unit: "ms".to_string(),
                        metadata: meta,
                    },
                ],
            }
        };

        let reports = vec![
            make_latency_report(&h1_b64, 0.5),
            make_latency_report(&h2_b64, 0.7),
        ];

        let agg = aggregate_reports(&reports);

        // Should have 6 percentile metrics (merged histogram)
        let latency_metrics: Vec<_> = agg
            .benchmarks
            .iter()
            .filter(|b| b.name.starts_with("test_latency_"))
            .collect();
        assert_eq!(latency_metrics.len(), 6);

        // Verify merged sample count = 2000
        let p50 = latency_metrics
            .iter()
            .find(|b| b.name.contains("p50"))
            .unwrap();
        assert_eq!(p50.metadata["samples"], serde_json::json!(2000u64));
        assert_eq!(p50.metadata["runs"], serde_json::json!(2));
    }

    #[test]
    fn extract_base_name_mid_suffix() {
        assert_eq!(
            extract_latency_base_name("e2e_latency_p50_light"),
            "e2e_latency_light"
        );
        assert_eq!(
            extract_latency_base_name("e2e_latency_p99_9_light"),
            "e2e_latency_light"
        );
        assert_eq!(
            extract_latency_base_name("compaction_idle_p99_99_enqueue"),
            "compaction_idle_enqueue"
        );
    }

    #[test]
    fn extract_base_name_terminal_suffix() {
        assert_eq!(
            extract_latency_base_name("kafka_latency_p50"),
            "kafka_latency"
        );
        assert_eq!(
            extract_latency_base_name("kafka_latency_max"),
            "kafka_latency"
        );
    }

    #[test]
    fn extract_base_name_no_suffix() {
        assert_eq!(
            extract_latency_base_name("enqueue_throughput_1kb"),
            "enqueue_throughput_1kb"
        );
    }
}
