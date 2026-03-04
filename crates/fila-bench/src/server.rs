use std::io::BufRead;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// A running `fila-server` instance for benchmarking.
///
/// Spawns the server binary on a random port with a temporary data directory.
/// The server is killed when this struct is dropped.
pub struct BenchServer {
    child: Option<Child>,
    addr: String,
    _data_dir: tempfile::TempDir,
}

impl BenchServer {
    /// Start a new fila-server instance for benchmarking.
    pub fn start() -> Self {
        Self::start_with_quantum(None)
    }

    /// Start a new fila-server instance with a specific DRR quantum.
    pub fn start_with_quantum(quantum: Option<u32>) -> Self {
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let data_dir = tempfile::tempdir().expect("create temp dir");

        let scheduler_section = match quantum {
            Some(q) => format!("\n[scheduler]\nquantum = {q}\n"),
            None => String::new(),
        };
        let config_content = format!(
            r#"[server]
listen_addr = "{addr}"
{scheduler_section}
[telemetry]
otlp_endpoint = ""
"#
        );
        let config_path = data_dir.path().join("fila.toml");
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
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fila-server");

        // Drain stderr so the process doesn't block on a full pipe.
        let stderr = child.stderr.take().expect("stderr");
        let reader = std::io::BufReader::new(stderr);
        std::thread::spawn(move || {
            for line in reader.lines() {
                if line.is_err() {
                    break;
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
            _data_dir: data_dir,
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

    /// The process ID of the running server.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }
}

impl Drop for BenchServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Create a queue via the fila CLI binary.
pub fn create_queue_cli(addr: &str, name: &str) {
    let binary = cli_binary();
    assert!(
        binary.exists(),
        "fila CLI binary not found at {binary:?}. Run `cargo build` first."
    );
    let output = Command::new(&binary)
        .arg("--addr")
        .arg(addr)
        .args(["queue", "create", name])
        .output()
        .expect("run fila CLI");
    assert!(
        output.status.success(),
        "failed to create queue '{name}': {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Create a queue with Lua scripts via the fila CLI binary.
pub fn create_queue_with_lua_cli(
    addr: &str,
    name: &str,
    on_enqueue: Option<&str>,
    on_failure: Option<&str>,
) {
    let binary = cli_binary();
    assert!(binary.exists(), "fila CLI binary not found at {binary:?}");
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
    let output = Command::new(&binary)
        .arg("--addr")
        .arg(addr)
        .args(&args)
        .output()
        .expect("run fila CLI");
    assert!(
        output.status.success(),
        "failed to create queue '{name}' with Lua: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

fn server_binary() -> PathBuf {
    workspace_binary("fila-server")
}

fn cli_binary() -> PathBuf {
    workspace_binary("fila")
}

fn workspace_binary(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push(name);
    path
}
