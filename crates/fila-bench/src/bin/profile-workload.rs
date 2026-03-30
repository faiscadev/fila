use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use fila_bench::server::{create_queue_cli, BenchServer};
use fila_sdk::FilaClient;
use inferno::collapse::Collapse;
use tokio_stream::StreamExt;

#[derive(Clone, ValueEnum)]
enum Workload {
    EnqueueOnly,
    ConsumeOnly,
    Lifecycle,
}

impl std::fmt::Display for Workload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Workload::EnqueueOnly => write!(f, "enqueue-only"),
            Workload::ConsumeOnly => write!(f, "consume-only"),
            Workload::Lifecycle => write!(f, "lifecycle"),
        }
    }
}

/// Run a fila-server workload and optionally generate a CPU flamegraph.
///
/// For flamegraph generation, run with sudo (macOS needs dtrace, Linux needs perf).
/// Build first with `cargo build --release`, then `sudo ./target/release/profile-workload --flamegraph`.
#[derive(Parser)]
#[command(name = "profile-workload")]
struct Args {
    /// Workload type to run
    #[arg(long, value_enum, default_value_t = Workload::EnqueueOnly)]
    workload: Workload,

    /// How long to run the workload in seconds
    #[arg(long, default_value_t = 30)]
    duration: u64,

    /// Payload size in bytes
    #[arg(long, default_value_t = 1024)]
    message_size: usize,

    /// Number of concurrent producers/consumers
    #[arg(long, default_value_t = 1)]
    concurrency: usize,

    /// Generate flamegraph SVG at this path (default: target/flamegraphs/<workload>.svg)
    #[arg(long)]
    flamegraph: Option<Option<PathBuf>>,

    /// Profiler sampling frequency in Hz
    #[arg(long, default_value_t = 997)]
    sample_hz: u32,
}

impl Args {
    fn resolved_flamegraph_path(&self) -> Option<PathBuf> {
        match &self.flamegraph {
            None => None,
            Some(Some(path)) => Some(path.clone()),
            Some(None) => {
                let dir = PathBuf::from("target/flamegraphs");
                std::fs::create_dir_all(&dir).expect("create target/flamegraphs");
                Some(dir.join(format!("{}.svg", self.workload)))
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let flamegraph_path = args.resolved_flamegraph_path();

    let server = BenchServer::start();
    let addr = server.addr().to_string();
    let grpc_addr = server.grpc_addr().to_string();
    let server_pid = server.pid().expect("server PID");
    let queue_name = "profile-queue";

    create_queue_cli(&grpc_addr, queue_name);

    eprintln!(
        "workload={} duration={}s msg_size={}B concurrency={} server_pid={} addr={}",
        args.workload, args.duration, args.message_size, args.concurrency, server_pid, addr,
    );

    let profiler = flamegraph_path
        .as_ref()
        .map(|output_path| start_profiler(server_pid, args.sample_hz, output_path));

    // Let the profiler attach and the server warm up.
    if profiler.is_some() {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let payload = vec![0x42u8; args.message_size];
    let duration = Duration::from_secs(args.duration);
    let start = Instant::now();

    match args.workload {
        Workload::EnqueueOnly => {
            run_enqueue_only(&addr, queue_name, &payload, args.concurrency, duration).await;
        }
        Workload::ConsumeOnly => {
            run_consume_only(&addr, queue_name, &payload, args.concurrency, duration).await;
        }
        Workload::Lifecycle => {
            run_lifecycle(&addr, queue_name, &payload, args.concurrency, duration).await;
        }
    }

    let elapsed = start.elapsed();
    eprintln!("workload complete in {:.1}s", elapsed.as_secs_f64());

    if let Some(profiler) = profiler {
        let output_path = flamegraph_path.unwrap();
        finish_profiler(profiler, &args.workload.to_string(), &output_path);
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
        let child = Command::new("dtrace")
            .args([
                "-x",
                "ustackframes=100",
                "-n",
                &format!("profile-{sample_hz} /pid == {server_pid}/ {{ @[ustack()] = count(); }}"),
                "-o",
                stacks_file.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start dtrace");

        eprintln!("profiler: dtrace attached to PID {server_pid}");
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
