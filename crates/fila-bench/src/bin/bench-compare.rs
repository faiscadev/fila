use fila_bench::compare::{compare_reports, print_summary};
use fila_bench::report::BenchReport;
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: bench-compare <baseline.json> <current.json> [threshold_pct]");
        eprintln!("  threshold_pct: regression threshold percentage (default: 10)");
        process::exit(2);
    }

    let baseline_path = &args[1];
    let current_path = &args[2];
    let threshold: f64 = match args.get(3) {
        Some(s) => s.parse().unwrap_or_else(|_| {
            eprintln!("Error: invalid threshold value '{s}' — must be a number");
            process::exit(2);
        }),
        None => 10.0,
    };

    let baseline_json = std::fs::read_to_string(baseline_path).unwrap_or_else(|e| {
        eprintln!("Error reading baseline file '{baseline_path}': {e}");
        process::exit(2);
    });
    let baseline: BenchReport = serde_json::from_str(&baseline_json).unwrap_or_else(|e| {
        eprintln!("Error parsing baseline JSON: {e}");
        process::exit(2);
    });

    let current_json = std::fs::read_to_string(current_path).unwrap_or_else(|e| {
        eprintln!("Error reading current file '{current_path}': {e}");
        process::exit(2);
    });
    let current: BenchReport = serde_json::from_str(&current_json).unwrap_or_else(|e| {
        eprintln!("Error parsing current JSON: {e}");
        process::exit(2);
    });

    let result = compare_reports(&baseline, &current, threshold);
    print_summary(&result);

    if result.has_regressions {
        process::exit(1);
    }
}
