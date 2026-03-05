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

    println!("Aggregated {} reports → {output_path}", reports.len());
    for metric in &aggregated.benchmarks {
        println!(
            "  {:<50} {:>12.2} {}",
            metric.name, metric.value, metric.unit
        );
    }
}
