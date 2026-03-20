mod helpers;

use helpers::{cli_run, TestServer};

/// Start a fila-server with API key authentication enabled.
///
/// Returns (TestServer, addr).
fn start_auth_server() -> (TestServer, String) {
    use std::net::TcpListener;

    let port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().unwrap().port()
    };
    let addr = format!("127.0.0.1:{port}");

    let data_dir = tempfile::tempdir().expect("temp dir");
    let config_content = format!(
        "[server]\nlisten_addr = \"{addr}\"\n\n[telemetry]\notlp_endpoint = \"\"\n\n[auth]\n"
    );
    let config_path = data_dir.path().join("fila.toml");
    std::fs::write(&config_path, config_content).expect("write config");

    let binary = {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); // crates/
        p.pop(); // workspace root
        p.push("target");
        p.push("debug");
        p.push("fila-server");
        p
    };
    assert!(
        binary.exists(),
        "fila-server binary not found at {binary:?}. Run `cargo build` first."
    );

    let child = std::process::Command::new(&binary)
        .env(
            "FILA_DATA_DIR",
            data_dir.path().join("data").to_str().unwrap(),
        )
        .current_dir(data_dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start fila-server with auth");

    // Poll TCP until the server is reachable.
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let http_addr = format!("http://{addr}");
    let server = TestServer::from_parts(child, http_addr.clone(), data_dir);
    (server, http_addr)
}

/// Create an API key via CLI and return (key_id, token).
fn cli_create_key(addr: &str, name: &str) -> (String, String) {
    let out = cli_run(addr, &["auth", "create", "--name", name]);
    assert!(
        out.success,
        "auth create failed: stderr={}\nstdout={}",
        out.stderr, out.stdout
    );
    // stdout format:
    //   Created API key "name"
    //     Key ID : <key_id>
    //     Token  : <token>
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

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

/// AC 1 + 10: auth disabled by default — plain server accepts requests without key.
#[tokio::test]
async fn auth_disabled_requests_succeed_without_key() {
    let server = TestServer::start();
    let client = fila_sdk::FilaClient::connect(server.addr())
        .await
        .expect("connect");

    // Enqueue to a queue that doesn't exist yet just to test auth (queue not found → EnqueueError,
    // but it should NOT be UNAUTHENTICATED).
    // Actually, let's create a queue first via CLI, then enqueue.
    let out = cli_run(server.addr(), &["queue", "create", "auth-disabled-test"]);
    assert!(out.success, "create queue: {}", out.stderr);

    let result = client
        .enqueue("auth-disabled-test", Default::default(), b"hello")
        .await;
    assert!(
        result.is_ok(),
        "auth disabled: enqueue should succeed without key, got: {result:?}"
    );
}

/// Assert that an EnqueueError is specifically UNAUTHENTICATED.
fn assert_unauthenticated(result: Result<String, fila_sdk::EnqueueError>, context: &str) {
    match result {
        Ok(_) => panic!("{context}: expected UNAUTHENTICATED, got Ok"),
        Err(fila_sdk::EnqueueError::Status(fila_sdk::StatusError::Rpc { code, .. }))
            if code == tonic::Code::Unauthenticated =>
        {
            // correct
        }
        Err(e) => panic!("{context}: expected UNAUTHENTICATED, got: {e:?}"),
    }
}

/// AC 2: auth enabled, request without key → UNAUTHENTICATED.
#[tokio::test]
async fn auth_enabled_request_without_key_is_rejected() {
    let (_server, addr) = start_auth_server();

    let client = fila_sdk::FilaClient::connect(&addr).await.expect("connect");

    // Auth check happens before queue lookup, so UNAUTHENTICATED is returned
    // even for a non-existent queue.
    let result = client.enqueue("any-queue", Default::default(), b"x").await;

    assert_unauthenticated(result, "missing key");
}

/// AC 3: auth enabled, invalid key → UNAUTHENTICATED.
#[tokio::test]
async fn auth_enabled_invalid_key_is_rejected() {
    let (_server, addr) = start_auth_server();

    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key("invalid-key-that-does-not-exist"),
    )
    .await
    .expect("connect");

    let result = client.enqueue("any-queue", Default::default(), b"x").await;

    assert_unauthenticated(result, "invalid key");
}

/// AC 4: auth enabled, valid key → request succeeds.
#[tokio::test]
async fn auth_enabled_valid_key_allows_request() {
    let (_server, addr) = start_auth_server();

    // Create a key via CLI (key-management RPCs bypass auth).
    let (_, token) = cli_create_key(&addr, "test-key");

    // Create the queue first using the valid key.
    let out = cli_run(
        &addr,
        &["--api-key", &token, "queue", "create", "auth-valid-test"],
    );
    assert!(out.success, "create queue: {}", out.stderr);

    // SDK client with the valid key should enqueue successfully.
    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    let result = client
        .enqueue("auth-valid-test", Default::default(), b"hello")
        .await;

    assert!(
        result.is_ok(),
        "expected enqueue to succeed with valid key, got: {result:?}"
    );
}

/// AC 4 (revoke): revoked key is rejected.
#[tokio::test]
async fn auth_enabled_revoked_key_is_rejected() {
    let (_server, addr) = start_auth_server();

    // Create a key.
    let (key_id, token) = cli_create_key(&addr, "revoke-test-key");

    // Verify the key works first.
    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    // Revoke the key using itself for auth (RevokeApiKey requires a valid key).
    let out = cli_run(&addr, &["--api-key", &token, "auth", "revoke", &key_id]);
    assert!(out.success, "revoke: {}", out.stderr);

    // Now the same key should be rejected.
    let result = client.enqueue("any-queue", Default::default(), b"x").await;

    assert_unauthenticated(result, "revoked key");
}
