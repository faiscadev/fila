use fila_bench::aggregate::aggregate_reports;
use fila_bench::report::BenchReport;
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: bench-aggregate <output.json> <report1.json> [report2.json] ...");
        process::exit(2);
    }

    let output_path = &args[1];
    let report_paths = &args[2..];

    let mut reports = Vec::new();
    for path in report_paths {
        let json = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Error reading report file '{path}': {e}");
            process::exit(2);
        });
        let report: BenchReport = serde_json::from_str(&json).unwrap_or_else(|e| {
            eprintln!("Error parsing JSON from '{path}': {e}");
            process::exit(2);
        });
        reports.push(report);
    }

    let aggregated = aggregate_reports(&reports);
    let json = serde_json::to_string_pretty(&aggregated).expect("serialize aggregated report");

    std::fs::write(output_path, &json).unwrap_or_else(|e| {
        eprintln!("Error writing output file '{output_path}': {e}");
        process::exit(2);
    });

    // Also write github-action-benchmark format files alongside the main output.
    let (smaller_json, bigger_json) = aggregated.to_gab_json();
    let stem = output_path.strip_suffix(".json").unwrap_or(output_path);
    let latency_path = format!("{stem}-gab-latency.json");
    let throughput_path = format!("{stem}-gab-throughput.json");
    std::fs::write(&latency_path, &smaller_json).unwrap_or_else(|e| {
        eprintln!("Error writing GAB latency file '{latency_path}': {e}");
        process::exit(2);
    });
    std::fs::write(&throughput_path, &bigger_json).unwrap_or_else(|e| {
        eprintln!("Error writing GAB throughput file '{throughput_path}': {e}");
        process::exit(2);
    });

    println!("Aggregated {} reports → {output_path}", reports.len());
    println!("GAB files: {latency_path}, {throughput_path}");
    for metric in &aggregated.benchmarks {
        println!(
            "  {:<50} {:>12.2} {}",
            metric.name, metric.value, metric.unit
        );
    }
}
