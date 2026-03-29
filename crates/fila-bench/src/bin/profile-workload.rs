use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fila_bench::server::{create_queue_cli, BenchServer};
use fila_sdk::FilaClient;
use inferno::collapse::Collapse;
use tokio_stream::StreamExt;

fn print_usage() {
    eprintln!(
        r#"Usage: profile-workload [OPTIONS]

Run a fila-server workload and optionally generate a CPU flamegraph.

Options:
  --workload NAME      enqueue-only|consume-only|lifecycle (default: enqueue-only)
  --duration SECS      how long to run the workload (default: 30)
  --message-size N     payload size in bytes (default: 1024)
  --concurrency N      concurrent producers/consumers (default: 1)
  --flamegraph [PATH]  generate flamegraph SVG (default: target/flamegraphs/<workload>.svg)
  --sample-hz N        profiler sampling frequency in Hz (default: 997)
  --help               show this help"#
    );
}

struct Config {
    workload: String,
    duration: u64,
    msg_size: usize,
    concurrency: usize,
    flamegraph: Option<PathBuf>,
    sample_hz: u32,
}

fn parse_args() -> Config {
    let mut config = Config {
        workload: "enqueue-only".to_string(),
        duration: 30,
        msg_size: 1024,
        concurrency: 1,
        flamegraph: None,
        sample_hz: 997,
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--workload" => {
                i += 1;
                config.workload = args[i].clone();
            }
            "--duration" => {
                i += 1;
                config.duration = args[i].parse().expect("invalid --duration");
            }
            "--message-size" => {
                i += 1;
                config.msg_size = args[i].parse().expect("invalid --message-size");
            }
            "--concurrency" => {
                i += 1;
                config.concurrency = args[i].parse().expect("invalid --concurrency");
            }
            "--flamegraph" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    config.flamegraph = Some(PathBuf::from(&args[i]));
                } else {
                    config.flamegraph = Some(PathBuf::new()); // sentinel for default path
                }
            }
            "--sample-hz" => {
                i += 1;
                config.sample_hz = args[i].parse().expect("invalid --sample-hz");
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                eprintln!("error: unknown option: {other}");
                print_usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Resolve default flamegraph path.
    if config.flamegraph == Some(PathBuf::new()) {
        let dir = PathBuf::from("target/flamegraphs");
        std::fs::create_dir_all(&dir).expect("create target/flamegraphs");
        config.flamegraph = Some(dir.join(format!("{}.svg", config.workload)));
    }

    config
}

#[tokio::main]
async fn main() {
    let config = parse_args();

    match &["enqueue-only", "consume-only", "lifecycle"] {
        valid if valid.contains(&config.workload.as_str()) => {}
        _ => {
            eprintln!("error: unknown workload '{}'", config.workload);
            eprintln!("available: enqueue-only, consume-only, lifecycle");
            std::process::exit(1);
        }
    }

    let server = BenchServer::start();
    let addr = server.addr().to_string();
    let server_pid = server.pid().expect("server PID");
    let queue_name = "profile-queue";

    create_queue_cli(&addr, queue_name);

    eprintln!(
        "workload={} duration={}s msg_size={}B concurrency={} server_pid={} addr={}",
        config.workload, config.duration, config.msg_size, config.concurrency, server_pid, addr,
    );

    let profiler = config.flamegraph.as_ref().map(|output_path| {
        start_profiler(server_pid, config.sample_hz, output_path)
    });

    // Let the profiler attach and the server warm up.
    if profiler.is_some() {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let payload = vec![0x42u8; config.msg_size];
    let duration = Duration::from_secs(config.duration);
    let start = Instant::now();

    match config.workload.as_str() {
        "enqueue-only" => {
            run_enqueue_only(&addr, queue_name, &payload, config.concurrency, duration).await;
        }
        "consume-only" => {
            run_consume_only(&addr, queue_name, &payload, config.concurrency, duration).await;
        }
        "lifecycle" => {
            run_lifecycle(&addr, queue_name, &payload, config.concurrency, duration).await;
        }
        _ => unreachable!(),
    }

    let elapsed = start.elapsed();
    eprintln!("workload complete in {:.1}s", elapsed.as_secs_f64());

    if let Some(profiler) = profiler {
        let output_path = config.flamegraph.unwrap();
        finish_profiler(profiler, &config.workload, &output_path);
    }

    drop(server);
}

// --- Profiler ---

enum Profiler {
    #[cfg(target_os = "linux")]
    Perf {
        child: std::process::Child,
        perf_data: PathBuf,
    },
    #[cfg(target_os = "macos")]
    Dtrace {
        child: std::process::Child,
        stacks_file: PathBuf,
    },
}

fn start_profiler(server_pid: u32, sample_hz: u32, _output_path: &PathBuf) -> Profiler {
    #[cfg(target_os = "linux")]
    {
        let perf_data = std::env::temp_dir().join(format!("fila-perf-{}.data", std::process::id()));
        let child = Command::new("perf")
            .args([
                "record",
                "-F",
                &sample_hz.to_string(),
                "-g",
                "--call-graph",
                "dwarf",
                "-p",
                &server_pid.to_string(),
                "-o",
                perf_data.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start perf — is linux-tools installed?");

        eprintln!("profiler: perf attached to PID {server_pid}");
        Profiler::Perf { child, perf_data }
    }

    #[cfg(target_os = "macos")]
    {
        let stacks_file =
            std::env::temp_dir().join(format!("fila-dtrace-{}.stacks", std::process::id()));
        // Pre-cache sudo credentials so dtrace can start non-interactively.
        let sudo_check = Command::new("sudo")
            .args(["-v"])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        if sudo_check.map(|s| !s.success()).unwrap_or(true) {
            eprintln!("error: sudo authentication failed — dtrace requires root");
            std::process::exit(1);
        }

        let child = Command::new("sudo")
            .args([
                "dtrace",
                "-x",
                "ustackframes=100",
                "-n",
                &format!(
                    "profile-{sample_hz} /pid == {server_pid}/ {{ @[ustack()] = count(); }}"
                ),
                "-o",
                stacks_file.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start dtrace");

        eprintln!("profiler: dtrace attached to PID {server_pid} (via sudo)");
        Profiler::Dtrace { child, stacks_file }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        panic!("profiling not supported on this OS — only Linux (perf) and macOS (dtrace)");
    }
}

fn finish_profiler(profiler: Profiler, workload: &str, output_path: &PathBuf) {
    let folded = match profiler {
        #[cfg(target_os = "linux")]
        Profiler::Perf {
            mut child,
            perf_data,
        } => {
            // Send SIGINT to stop recording.
            Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status()
                .ok();
            child.wait().ok();

            eprintln!("collapsing perf stacks...");
            let script_output = Command::new("perf")
                .args(["script", "-i", perf_data.to_str().unwrap()])
                .output()
                .expect("perf script failed");

            let mut folder = inferno::collapse::perf::Folder::default();
            let mut folded = Vec::new();
            folder
                .collapse(BufReader::new(&script_output.stdout[..]), &mut folded)
                .expect("collapse perf stacks");

            let _ = std::fs::remove_file(&perf_data);
            folded
        }
        #[cfg(target_os = "macos")]
        Profiler::Dtrace {
            mut child,
            stacks_file,
        } => {
            // Send SIGINT to the sudo process. sudo forwards signals to dtrace,
            // which flushes its aggregation output and exits cleanly.
            Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status()
                .ok();
            child.wait().ok();

            // Small delay for output file to be flushed.
            std::thread::sleep(Duration::from_millis(500));

            eprintln!("collapsing dtrace stacks...");
            let file = std::fs::File::open(&stacks_file).expect("open dtrace output");
            let mut folder = inferno::collapse::dtrace::Folder::default();
            let mut folded = Vec::new();
            folder
                .collapse(BufReader::new(file), &mut folded)
                .expect("collapse dtrace stacks");

            let _ = std::fs::remove_file(&stacks_file);
            folded
        }
    };

    eprintln!("generating flamegraph...");
    let folded_str = String::from_utf8(folded).expect("folded stacks are UTF-8");
    let mut opts = inferno::flamegraph::Options::default();
    opts.title = format!("fila-server {workload}");

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut svg_file = std::fs::File::create(output_path).expect("create output SVG");
    inferno::flamegraph::from_lines(&mut opts, folded_str.lines(), &mut svg_file)
        .expect("generate flamegraph");

    svg_file.flush().ok();
    let size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
    eprintln!("flamegraph: {} ({} bytes)", output_path.display(), size);
}

// --- Workloads ---

async fn connect(addr: &str) -> FilaClient {
    FilaClient::connect(addr)
        .await
        .expect("connect to fila-server")
}

async fn run_enqueue_only(
    addr: &str,
    queue: &str,
    payload: &[u8],
    concurrency: usize,
    duration: Duration,
) {
    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let client = connect(addr).await;
        let q = queue.to_string();
        let p = payload.to_vec();
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + duration;
            let mut count = 0u64;
            while Instant::now() < deadline {
                client
                    .enqueue(&q, HashMap::new(), p.clone())
                    .await
                    .expect("enqueue");
                count += 1;
            }
            count
        }));
    }
    let mut total = 0u64;
    for h in handles {
        total += h.await.unwrap();
    }
    eprintln!("enqueued {total} messages");
}

async fn run_consume_only(
    addr: &str,
    queue: &str,
    payload: &[u8],
    concurrency: usize,
    duration: Duration,
) {
    let prefill_count = 10_000;
    let client = connect(addr).await;
    for _ in 0..prefill_count {
        client
            .enqueue(queue, HashMap::new(), payload.to_vec())
            .await
            .expect("prefill enqueue");
    }
    eprintln!("prefilled {prefill_count} messages");

    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let c = connect(addr).await;
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + duration;
            let mut count = 0u64;
            let mut stream = c.consume(&q).await.expect("consume");
            while Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_secs(1), stream.next()).await {
                    Ok(Some(Ok(msg))) => {
                        c.ack(&q, &msg.id).await.expect("ack");
                        count += 1;
                    }
                    Ok(Some(Err(e))) => {
                        eprintln!("consume error: {e}");
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
            count
        }));
    }
    let mut total = 0u64;
    for h in handles {
        total += h.await.unwrap();
    }
    eprintln!("consumed {total} messages");
}

async fn run_lifecycle(
    addr: &str,
    queue: &str,
    payload: &[u8],
    concurrency: usize,
    duration: Duration,
) {
    let mut handles = Vec::new();

    for _ in 0..concurrency {
        let client = connect(addr).await;
        let q = queue.to_string();
        let p = payload.to_vec();
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + duration;
            let mut count = 0u64;
            while Instant::now() < deadline {
                client
                    .enqueue(&q, HashMap::new(), p.clone())
                    .await
                    .expect("enqueue");
                count += 1;
            }
            ("producer", count)
        }));
    }

    for _ in 0..concurrency {
        let c = connect(addr).await;
        let q = queue.to_string();
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + duration;
            let mut count = 0u64;
            let mut stream = c.consume(&q).await.expect("consume");
            while Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_secs(1), stream.next()).await {
                    Ok(Some(Ok(msg))) => {
                        c.ack(&q, &msg.id).await.expect("ack");
                        count += 1;
                    }
                    Ok(Some(Err(e))) => {
                        eprintln!("consume error: {e}");
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
            ("consumer", count)
        }));
    }

    let mut produced = 0u64;
    let mut consumed = 0u64;
    for h in handles {
        let (role, count) = h.await.unwrap();
        match role {
            "producer" => produced += count,
            "consumer" => consumed += count,
            _ => {}
        }
    }
    eprintln!("produced {produced}, consumed {consumed}");
}
