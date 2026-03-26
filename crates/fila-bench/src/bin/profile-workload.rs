use std::collections::HashMap;
use std::time::{Duration, Instant};

use fila_bench::server::{create_queue_cli, BenchServer};
use fila_sdk::{AccumulatorMode, ConnectOptions, FilaClient, Transport};
use tokio_stream::StreamExt;

/// Profiling workload driver for flamegraph generation.
///
/// Starts a fila-server, runs a configurable workload, and exits.
/// Designed to be invoked by `scripts/flamegraph.sh` under a profiler.
///
/// Environment variables:
///   PROFILE_WORKLOAD   - enqueue-only, consume-only, lifecycle, batch-enqueue
///   PROFILE_DURATION   - seconds to run (default: 30)
///   PROFILE_MSG_SIZE   - message payload size in bytes (default: 1024)
///   PROFILE_CONCURRENCY- number of concurrent producers/consumers (default: 1)
///   PROFILE_TRANSPORT  - "grpc" (default) or "fibp"
///   PROFILE_SERVER_ADDR- if set, connect to an external server instead of starting one
///   PROFILE_FIBP_ADDR  - if set (with PROFILE_SERVER_ADDR), use this FIBP address for external server
#[tokio::main]
async fn main() {
    let workload = std::env::var("PROFILE_WORKLOAD").unwrap_or_else(|_| "enqueue-only".to_string());
    let duration_secs: u64 = std::env::var("PROFILE_DURATION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let msg_size: usize = std::env::var("PROFILE_MSG_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    let concurrency: usize = std::env::var("PROFILE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let transport = std::env::var("PROFILE_TRANSPORT").unwrap_or_else(|_| "grpc".to_string());
    let use_fibp = transport.eq_ignore_ascii_case("fibp");

    let duration = Duration::from_secs(duration_secs);
    let payload = vec![0x42u8; msg_size];
    let queue_name = "profile-queue";

    // Start embedded server or connect to external.
    let _server;
    let (grpc_addr, connect_addr) = match std::env::var("PROFILE_SERVER_ADDR") {
        Ok(a) => {
            let fibp_a = std::env::var("PROFILE_FIBP_ADDR").ok();
            let ca = if use_fibp {
                fibp_a.unwrap_or_else(|| {
                    panic!("PROFILE_FIBP_ADDR required when using external server with FIBP transport")
                })
            } else {
                a.clone()
            };
            (a, ca)
        }
        Err(_) => {
            if use_fibp {
                let s = if std::env::var("FILA_STORAGE")
                    .map(|v| v.eq_ignore_ascii_case("memory"))
                    .unwrap_or(false)
                {
                    BenchServer::start_with_fibp_in_memory()
                } else {
                    BenchServer::start_with_fibp()
                };
                let grpc = s.addr().to_string();
                let fibp = s.fibp_addr().to_string();
                _server = s;
                (grpc, fibp)
            } else {
                let s = BenchServer::start();
                let a = s.addr().to_string();
                _server = s;
                (a.clone(), a)
            }
        }
    };

    // Create the queue via gRPC (admin operations always use gRPC).
    create_queue_cli(&grpc_addr, queue_name);

    eprintln!(
        "workload={workload} transport={transport} duration={duration_secs}s msg_size={msg_size}B concurrency={concurrency} addr={connect_addr}"
    );

    let start = Instant::now();

    match workload.as_str() {
        "enqueue-only" => {
            run_enqueue_only(&connect_addr, use_fibp, queue_name, &payload, concurrency, duration)
                .await
        }
        "consume-only" => {
            run_consume_only(&connect_addr, use_fibp, queue_name, &payload, concurrency, duration)
                .await
        }
        "lifecycle" => {
            run_lifecycle(&connect_addr, use_fibp, queue_name, &payload, concurrency, duration)
                .await
        }
        "batch-enqueue" => {
            run_batch_enqueue(&connect_addr, use_fibp, queue_name, &payload, concurrency, duration)
                .await
        }
        other => {
            eprintln!("unknown workload: {other}");
            eprintln!("available: enqueue-only, consume-only, lifecycle, batch-enqueue");
            std::process::exit(1);
        }
    }

    let elapsed = start.elapsed();
    eprintln!("workload complete in {:.1}s", elapsed.as_secs_f64());
}

async fn connect(addr: &str, use_fibp: bool) -> FilaClient {
    let mut opts = ConnectOptions::new(addr)
        .with_timeout(Duration::from_secs(30))
        .with_accumulator(AccumulatorMode::Disabled);
    if use_fibp {
        opts = opts.with_transport(Transport::Fibp);
    }
    FilaClient::connect_with_options(opts)
        .await
        .expect("connect to fila-server")
}

async fn connect_with_batching(addr: &str, use_fibp: bool) -> FilaClient {
    let mut opts = ConnectOptions::new(addr).with_timeout(Duration::from_secs(30));
    if use_fibp {
        opts = opts.with_transport(Transport::Fibp);
    }
    FilaClient::connect_with_options(opts)
        .await
        .expect("connect to fila-server")
}

async fn run_enqueue_only(
    addr: &str,
    use_fibp: bool,
    queue: &str,
    payload: &[u8],
    concurrency: usize,
    duration: Duration,
) {
    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let client = connect(addr, use_fibp).await;
        let q = queue.to_string();
        let p = payload.to_vec();
        let d = duration;
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + d;
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
    use_fibp: bool,
    queue: &str,
    payload: &[u8],
    concurrency: usize,
    duration: Duration,
) {
    // Pre-fill the queue with messages.
    let prefill_count = 10_000;
    let client = connect(addr, use_fibp).await;
    for _ in 0..prefill_count {
        client
            .enqueue(queue, HashMap::new(), payload.to_vec())
            .await
            .expect("prefill enqueue");
    }
    eprintln!("prefilled {prefill_count} messages");

    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let c = connect(addr, use_fibp).await;
        let q = queue.to_string();
        let d = duration;
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + d;
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
                    Err(_) => continue, // timeout, try again
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
    use_fibp: bool,
    queue: &str,
    payload: &[u8],
    concurrency: usize,
    duration: Duration,
) {
    // Run producers and consumers concurrently.
    let mut handles = Vec::new();

    // Producers.
    for _ in 0..concurrency {
        let client = connect(addr, use_fibp).await;
        let q = queue.to_string();
        let p = payload.to_vec();
        let d = duration;
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + d;
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

    // Consumers.
    for _ in 0..concurrency {
        let c = connect(addr, use_fibp).await;
        let q = queue.to_string();
        let d = duration;
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + d;
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

async fn run_batch_enqueue(
    addr: &str,
    use_fibp: bool,
    queue: &str,
    payload: &[u8],
    concurrency: usize,
    duration: Duration,
) {
    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let client = connect_with_batching(addr, use_fibp).await;
        let q = queue.to_string();
        let p = payload.to_vec();
        let d = duration;
        handles.push(tokio::spawn(async move {
            let deadline = Instant::now() + d;
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
    eprintln!("batch-enqueued {total} messages");
}
