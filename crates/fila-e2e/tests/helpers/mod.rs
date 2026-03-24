#![allow(dead_code)]

pub mod cluster;

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
    /// Construct a TestServer from a pre-spawned child process.
    ///
    /// Used by tests that need custom server configuration (e.g. TLS) and
    /// spawn the process themselves.
    pub fn from_parts(
        child: std::process::Child,
        addr: String,
        data_dir: tempfile::TempDir,
    ) -> Self {
        Self {
            child: Some(child),
            addr,
            data_dir: Some(data_dir),
        }
    }

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
    ///
    /// Uses port 0 so the OS assigns a free port. The server writes the
    /// actual bound address to a port file (`FILA_PORT_FILE` env var).
    /// This eliminates the TOCTOU race entirely — the port is already
    /// bound by the OS when we read it.
    fn start_with_options(opts: TestServerOptions) -> Self {
        let data_dir = tempfile::tempdir().expect("create temp dir");
        let port_file = data_dir.path().join("port");

        let config_path = data_dir.path().join("fila.toml");
        let scheduler_section = if let Some(q) = opts.quantum {
            format!("\n[scheduler]\nquantum = {q}\n")
        } else {
            String::new()
        };
        let config_content = format!(
            "[server]\nlisten_addr = \"127.0.0.1:0\"\n{scheduler_section}\n[telemetry]\notlp_endpoint = \"\"\n"
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
            .env("FILA_PORT_FILE", port_file.to_str().unwrap())
            .current_dir(data_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fila-server");

        // Drain stdout and stderr so the process doesn't block on full pipes.
        let stdout = child.stdout.take().expect("stdout");
        std::thread::spawn(move || for _ in BufReader::new(stdout).lines() {});
        let stderr = child.stderr.take().expect("stderr");
        std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});

        // Wait for the port file to appear (server writes it after binding).
        let start = std::time::Instant::now();
        let addr = loop {
            if start.elapsed() > Duration::from_secs(10) {
                // Check if child died
                match child.try_wait() {
                    Ok(Some(status)) => panic!("fila-server exited with {status} before writing port file"),
                    _ => panic!("fila-server did not write port file within 10s"),
                }
            }
            if let Ok(contents) = std::fs::read_to_string(&port_file) {
                let contents = contents.trim();
                if !contents.is_empty() {
                    break contents.to_string();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        };

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
        self.host_port().split(':').last().unwrap().parse().unwrap()
    }

    /// Restart a server on the same data directory (gets a new OS-assigned port).
    pub fn restart_on(data_dir: tempfile::TempDir, _port: u16) -> Self {
        let port_file = data_dir.path().join("port");
        // Remove stale port file from previous run
        let _ = std::fs::remove_file(&port_file);

        let binary = server_binary();
        assert!(
            binary.exists(),
            "fila-server binary not found at {binary:?}"
        );

        let mut child = Command::new(&binary)
            .env(
                "FILA_DATA_DIR",
                data_dir.path().join("data").to_str().unwrap(),
            )
            .env("FILA_PORT_FILE", port_file.to_str().unwrap())
            .current_dir(data_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("restart fila-server");

        let stdout = child.stdout.take().expect("stdout");
        std::thread::spawn(move || for _ in BufReader::new(stdout).lines() {});
        let stderr = child.stderr.take().expect("stderr");
        std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});

        // Wait for the port file to appear.
        let start = std::time::Instant::now();
        let addr = loop {
            if start.elapsed() > Duration::from_secs(10) {
                match child.try_wait() {
                    Ok(Some(status)) => panic!("fila-server exited with {status} after restart before writing port file"),
                    _ => panic!("fila-server did not write port file within 10s after restart"),
                }
            }
            if let Ok(contents) = std::fs::read_to_string(&port_file) {
                let contents = contents.trim();
                if !contents.is_empty() {
                    break contents.to_string();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        };

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

/// Bootstrap API key used by `start_auth_server` and `cli_create_superadmin_key`.
pub const TEST_BOOTSTRAP_KEY: &str = "test-bootstrap-key-for-e2e";

/// Start a fila-server with API key authentication enabled.
///
/// Returns (TestServer, http_addr). Use `TEST_BOOTSTRAP_KEY` as the initial credential.
pub fn start_auth_server() -> (TestServer, String) {
    let data_dir = tempfile::tempdir().expect("temp dir");
    let port_file = data_dir.path().join("port");
    let config_content = format!(
        "[server]\nlisten_addr = \"127.0.0.1:0\"\n\n[telemetry]\notlp_endpoint = \"\"\n\n[auth]\nbootstrap_apikey = \"{TEST_BOOTSTRAP_KEY}\"\n"
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
        .env("FILA_PORT_FILE", port_file.to_str().unwrap())
        .current_dir(data_dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start fila-server with auth");

    // Drain stdout and stderr so the process does not block on full pipes.
    let stdout = child.stdout.take().expect("stdout");
    std::thread::spawn(move || for _ in BufReader::new(stdout).lines() {});
    let stderr = child.stderr.take().expect("stderr");
    std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});

    // Wait for the port file to appear.
    let start = std::time::Instant::now();
    let addr = loop {
        if start.elapsed() > Duration::from_secs(10) {
            match child.try_wait() {
                Ok(Some(status)) => panic!("fila-server (auth) exited with {status} before writing port file"),
                _ => panic!("fila-server (auth) did not write port file within 10s"),
            }
        }
        if let Ok(contents) = std::fs::read_to_string(&port_file) {
            let contents = contents.trim();
            if !contents.is_empty() {
                break contents.to_string();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let http_addr = format!("http://{addr}");
    let server = TestServer::from_parts(child, http_addr.clone(), data_dir);
    (server, http_addr)
}

/// Create a superadmin API key via CLI and return (key_id, token).
///
/// Superadmin keys bypass all ACL checks and are suitable for tests that
/// need to perform admin operations (queue create, acl set, etc.).
pub fn cli_create_superadmin_key(addr: &str, name: &str) -> (String, String) {
    let out = cli_run(
        addr,
        &[
            "--api-key",
            TEST_BOOTSTRAP_KEY,
            "auth",
            "create",
            "--name",
            name,
            "--superadmin",
        ],
    );
    assert!(
        out.success,
        "auth create --superadmin failed: stderr={}\nstdout={}",
        out.stderr, out.stdout
    );
    // stdout format:
    //   Created API key "name"
    //     Key ID     : <key_id>
    //     Token      : <token>
    //   Store the token...
    let key_id = out
        .stdout
        .lines()
        .find(|l| l.contains("Key ID"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .expect("key_id in output");
    let token = out
        .stdout
        .lines()
        .find(|l| l.contains("Token"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .expect("token in output");
    (key_id, token)
}

/// Connect an SDK client to the given server address.
pub async fn sdk_client(addr: &str) -> fila_sdk::FilaClient {
    fila_sdk::FilaClient::connect(addr)
        .await
        .expect("connect SDK client")
}

/// Extract `addr=<ip:port>` from a tracing log line.
///
/// The server logs lines like:
///   `2026-03-23T... INFO fila_server: addr=127.0.0.1:12345 tls=false starting gRPC server`
fn extract_addr_from_log(line: &str) -> Option<String> {
    // Strip ANSI escape codes before parsing — tracing output includes
    // color codes around field names even when stderr is piped.
    let stripped = strip_ansi(line);
    let addr_prefix = "addr=";
    let start = stripped.find(addr_prefix)? + addr_prefix.len();
    let rest = &stripped[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we find a letter (end of ANSI sequence)
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Find a free TCP port. Used by cluster helper which manages its own server processes.
pub fn free_port() -> u16 {
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
