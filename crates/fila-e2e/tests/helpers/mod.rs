#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

/// A running `fila-server` instance for e2e testing.
///
/// Spawns the server binary on a random port with a temporary data directory.
/// The server is killed when this struct is dropped.
pub struct TestServer {
    child: Option<Child>,
    addr: String,
    /// Kept alive for the duration of the test. When dropped, the temp dir is cleaned up.
    /// `None` after `kill_and_take_data()` transfers ownership.
    data_dir: Option<tempfile::TempDir>,
}

#[derive(Default)]
struct TestServerOptions {
    quantum: Option<u32>,
}

impl TestServer {
    /// Start a new fila-server instance on a random port.
    pub fn start() -> Self {
        Self::start_with_options(TestServerOptions::default())
    }

    /// Start a new fila-server instance with a custom DRR quantum.
    pub fn start_with_quantum(quantum: u32) -> Self {
        Self::start_with_options(TestServerOptions {
            quantum: Some(quantum),
        })
    }

    /// Start a new fila-server instance with custom options.
    fn start_with_options(opts: TestServerOptions) -> Self {
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let data_dir = tempfile::tempdir().expect("create temp dir");

        let config_path = data_dir.path().join("fila.toml");
        let scheduler_section = if let Some(q) = opts.quantum {
            format!("\n[scheduler]\nquantum = {q}\n")
        } else {
            String::new()
        };
        let config_content = format!(
            r#"[server]
listen_addr = "{addr}"
{scheduler_section}
[telemetry]
otlp_endpoint = ""
"#
        );
        std::fs::write(&config_path, config_content).expect("write config");

        let binary = server_binary();
        assert!(
            binary.exists(),
            "fila-server binary not found at {binary:?}. Run `cargo build` first."
        );

        let mut child = Command::new(&binary)
            .env(
                "FILA_DATA_DIR",
                data_dir.path().join("data").to_str().unwrap(),
            )
            .current_dir(data_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fila-server");

        // Drain stderr so the process doesn't block on a full pipe.
        let stderr = child.stderr.take().expect("stderr");
        let reader = BufReader::new(stderr);
        let addr_for_thread = addr.clone();
        std::thread::spawn(move || {
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if line.contains(&addr_for_thread)
                            || line.contains("starting gRPC server")
                        {
                            // Server is ready — keep draining.
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Poll TCP until the server is reachable.
        let start = std::time::Instant::now();
        let mut connected = false;
        while start.elapsed() < Duration::from_secs(10) {
            if std::net::TcpStream::connect(&addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            connected,
            "fila-server did not become reachable at {addr} within 10s"
        );

        Self {
            child: Some(child),
            addr: format!("http://{addr}"),
            data_dir: Some(data_dir),
        }
    }

    /// The HTTP address of the running server (e.g., "http://127.0.0.1:12345").
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// The raw host:port address (without http:// prefix).
    pub fn host_port(&self) -> &str {
        self.addr.strip_prefix("http://").unwrap_or(&self.addr)
    }

    /// Kill the server and return the data directory for restarting on the same data.
    /// This simulates a crash — the server is killed with SIGKILL.
    pub fn kill_and_take_data(mut self) -> (tempfile::TempDir, u16) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let port = self.port();
        let data_dir = self.data_dir.take().expect("data_dir already taken");
        (data_dir, port)
    }

    fn port(&self) -> u16 {
        self.host_port()
            .split(':')
            .last()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// Restart a server on the same data directory and port.
    pub fn restart_on(data_dir: tempfile::TempDir, port: u16) -> Self {
        let addr = format!("127.0.0.1:{port}");

        let binary = server_binary();
        assert!(binary.exists(), "fila-server binary not found at {binary:?}");

        let mut child = Command::new(&binary)
            .env(
                "FILA_DATA_DIR",
                data_dir.path().join("data").to_str().unwrap(),
            )
            .current_dir(data_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("restart fila-server");

        let stderr = child.stderr.take().expect("stderr");
        let reader = BufReader::new(stderr);
        let addr_for_thread = addr.clone();
        std::thread::spawn(move || {
            for line in reader.lines() {
                match line {
                    Ok(_line) => {
                        if _line.contains(&addr_for_thread)
                            || _line.contains("starting gRPC server")
                        {
                            // ready
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let start = std::time::Instant::now();
        let mut connected = false;
        while start.elapsed() < Duration::from_secs(10) {
            if std::net::TcpStream::connect(&addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            connected,
            "fila-server did not become reachable at {addr} within 10s after restart"
        );

        Self {
            child: Some(child),
            addr: format!("http://{addr}"),
            data_dir: Some(data_dir),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Output from a CLI invocation.
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Run the `fila` CLI binary with the given arguments and server address.
pub fn cli_run(addr: &str, args: &[&str]) -> CliOutput {
    let binary = cli_binary();
    assert!(
        binary.exists(),
        "fila CLI binary not found at {binary:?}. Run `cargo build` first."
    );

    let output: Output = Command::new(&binary)
        .arg("--addr")
        .arg(addr)
        .args(args)
        .output()
        .expect("run fila CLI");

    CliOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        success: output.status.success(),
    }
}

/// Create a queue via the CLI.
pub fn create_queue_cli(addr: &str, name: &str) {
    let output = cli_run(addr, &["queue", "create", name]);
    assert!(
        output.success,
        "failed to create queue '{name}': {}",
        output.stderr
    );
}

/// Create a queue with Lua scripts via the CLI.
pub fn create_queue_with_scripts_cli(
    addr: &str,
    name: &str,
    on_enqueue: Option<&str>,
    on_failure: Option<&str>,
    visibility_timeout_ms: Option<u64>,
) {
    let mut args = vec!["queue", "create", name];
    let on_enqueue_owned;
    if let Some(script) = on_enqueue {
        args.push("--on-enqueue");
        on_enqueue_owned = script.to_string();
        args.push(&on_enqueue_owned);
    }
    let on_failure_owned;
    if let Some(script) = on_failure {
        args.push("--on-failure");
        on_failure_owned = script.to_string();
        args.push(&on_failure_owned);
    }
    let vt_str;
    if let Some(vt) = visibility_timeout_ms {
        args.push("--visibility-timeout");
        vt_str = vt.to_string();
        args.push(&vt_str);
    }
    let output = cli_run(addr, &args);
    assert!(
        output.success,
        "failed to create queue '{name}' with scripts: {}",
        output.stderr
    );
}

/// Connect an SDK client to the given server address.
pub async fn sdk_client(addr: &str) -> fila_sdk::FilaClient {
    fila_sdk::FilaClient::connect(addr).await.expect("connect SDK client")
}

/// Find a free TCP port.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

/// Resolve the path to the fila-server binary.
fn server_binary() -> PathBuf {
    workspace_binary("fila-server")
}

/// Resolve the path to the fila CLI binary.
fn cli_binary() -> PathBuf {
    workspace_binary("fila")
}

/// Resolve a binary path from the workspace target directory.
fn workspace_binary(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push(name);
    path
}
