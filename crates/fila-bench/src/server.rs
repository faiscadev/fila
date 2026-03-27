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
        Self::start_inner(None, false)
    }

    /// Start a new fila-server instance with in-memory storage (no RocksDB).
    pub fn start_in_memory() -> Self {
        Self::start_inner(None, true)
    }

    /// Start a new fila-server instance with a specific DRR quantum.
    pub fn start_with_quantum(quantum: Option<u32>) -> Self {
        Self::start_inner(quantum, false)
    }

    fn start_inner(quantum: Option<u32>, in_memory: bool) -> Self {
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let data_dir = tempfile::tempdir().expect("create temp dir");

        let scheduler_section = match quantum {
            Some(q) => format!("\n[scheduler]\nquantum = {q}\n"),
            None => String::new(),
        };
        let config_content = format!(
            r#"[fibp]
listen_addr = "{addr}"
{scheduler_section}
[telemetry]
otlp_endpoint = ""
"#
        );
        let config_path = data_dir.path().join("fila.toml");
        std::fs::write(&config_path, config_content).expect("write config");

        let port_file = data_dir.path().join("port");

        let binary = server_binary();
        assert!(
            binary.exists(),
            "fila-server binary not found at {binary:?}. Run `cargo build` first."
        );

        let mut cmd = Command::new(&binary);
        cmd.env(
            "FILA_DATA_DIR",
            data_dir.path().join("data").to_str().unwrap(),
        );
        cmd.env("FILA_PORT_FILE", port_file.to_str().unwrap());
        if in_memory {
            cmd.env("FILA_STORAGE", "memory");
        }
        let mut child = cmd
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

        // Wait for the port file to appear (server writes it after binding).
        let start = std::time::Instant::now();
        let actual_addr = loop {
            if start.elapsed() > Duration::from_secs(10) {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        panic!("fila-server exited with {status} before writing port file")
                    }
                    _ => panic!("fila-server did not write port file within 10s"),
                }
            }
            if let Ok(contents) = std::fs::read_to_string(&port_file) {
                let contents = contents.trim();
                if !contents.is_empty() {
                    break contents.to_string();
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        };

        Self {
            child: Some(child),
            addr: actual_addr,
            _data_dir: data_dir,
        }
    }

    /// The host:port address of the running server (e.g., "127.0.0.1:12345").
    pub fn addr(&self) -> &str {
        &self.addr
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

/// Create a queue via the SDK's FIBP transport.
pub async fn create_queue_cli(addr: &str, name: &str) {
    let transport = fila_sdk::FibpTransport::connect(addr, None)
        .await
        .expect("connect to fila-server for create_queue");
    match transport.create_queue(name, "", "", 0).await {
        Ok(_) => {}
        Err(e) => {
            let msg = format!("{e}");
            if !msg.contains("already exists") {
                panic!("failed to create queue '{name}': {e}");
            }
        }
    }
}

/// Create a queue with Lua scripts via the SDK's FIBP transport.
pub async fn create_queue_with_lua_cli(
    addr: &str,
    name: &str,
    on_enqueue: Option<&str>,
    on_failure: Option<&str>,
) {
    let transport = fila_sdk::FibpTransport::connect(addr, None)
        .await
        .expect("connect to fila-server for create_queue");
    transport
        .create_queue(
            name,
            on_enqueue.unwrap_or_default(),
            on_failure.unwrap_or_default(),
            0,
        )
        .await
        .unwrap_or_else(|e| panic!("failed to create queue '{name}' with Lua: {e}"));
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

fn server_binary() -> PathBuf {
    workspace_binary("fila-server")
}

fn workspace_binary(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("release");
    path.push(name);
    path
}
