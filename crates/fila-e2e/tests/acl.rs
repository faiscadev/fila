mod helpers;

use helpers::{cli_create_superadmin_key, cli_run, start_auth_server};

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Create a regular (non-superadmin) API key via CLI and return (key_id, token).
fn cli_create_regular_key(addr: &str, name: &str) -> (String, String) {
    let out = cli_run(addr, &["auth", "create", "--name", name]);
    assert!(
        out.success,
        "auth create failed: stderr={}\nstdout={}",
        out.stderr,
        out.stdout
    );
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

fn assert_permission_denied_enqueue(
    result: Result<String, fila_sdk::EnqueueError>,
    context: &str,
) {
    match result {
        Ok(_) => panic!("{context}: expected PERMISSION_DENIED, got Ok"),
        Err(fila_sdk::EnqueueError::Status(fila_sdk::StatusError::Rpc { code, .. }))
            if code == tonic::Code::PermissionDenied =>
        {
            // correct
        }
        Err(e) => panic!("{context}: expected PERMISSION_DENIED, got: {e:?}"),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

/// Key without any permissions cannot enqueue.
#[tokio::test]
async fn key_without_permissions_cannot_enqueue() {
    let (_server, addr) = start_auth_server();

    // Admin creates the queue using a superadmin key.
    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    let out = cli_run(
        &addr,
        &["--api-key", &admin_token, "queue", "create", "acl-test-q"],
    );
    assert!(out.success, "create queue: {}", out.stderr);

    // Regular key with no permissions.
    let (_, token) = cli_create_regular_key(&addr, "no-perms-key");

    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    let result = client
        .enqueue("acl-test-q", Default::default(), b"x")
        .await;
    assert_permission_denied_enqueue(result, "key without permissions");
}

/// Key with `produce:<queue>` permission can enqueue to that queue.
#[tokio::test]
async fn key_with_produce_permission_can_enqueue() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    let out = cli_run(
        &addr,
        &["--api-key", &admin_token, "queue", "create", "produce-test-q"],
    );
    assert!(out.success, "create queue: {}", out.stderr);

    // Create a regular key and grant it produce:produce-test-q.
    let (key_id, token) = cli_create_regular_key(&addr, "producer-key");
    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &key_id,
            "--perm",
            "produce:produce-test-q",
        ],
    );
    assert!(out.success, "set acl: {}", out.stderr);

    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    let result = client
        .enqueue("produce-test-q", Default::default(), b"hello")
        .await;
    assert!(
        result.is_ok(),
        "expected enqueue to succeed with produce permission, got: {result:?}"
    );
}

/// Key with `produce:<queue>` cannot enqueue to a *different* queue.
#[tokio::test]
async fn key_with_produce_permission_cannot_enqueue_to_other_queue() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    for q in &["queue-a", "queue-b"] {
        let out = cli_run(
            &addr,
            &["--api-key", &admin_token, "queue", "create", q],
        );
        assert!(out.success, "create queue {q}: {}", out.stderr);
    }

    let (key_id, token) = cli_create_regular_key(&addr, "narrow-producer");
    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &key_id,
            "--perm",
            "produce:queue-a",
        ],
    );
    assert!(out.success, "set acl: {}", out.stderr);

    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    // queue-a: should succeed.
    let result = client
        .enqueue("queue-a", Default::default(), b"hello")
        .await;
    assert!(result.is_ok(), "queue-a should succeed: {result:?}");

    // queue-b: should be denied.
    let result = client
        .enqueue("queue-b", Default::default(), b"hello")
        .await;
    assert_permission_denied_enqueue(result, "wrong queue");
}

/// Wildcard `produce:*` grants enqueue on any queue.
#[tokio::test]
async fn wildcard_produce_permission_allows_any_queue() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    for q in &["wc-queue-1", "wc-queue-2"] {
        let out = cli_run(
            &addr,
            &["--api-key", &admin_token, "queue", "create", q],
        );
        assert!(out.success, "create queue {q}: {}", out.stderr);
    }

    let (key_id, token) = cli_create_regular_key(&addr, "wildcard-producer");
    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &key_id,
            "--perm",
            "produce:*",
        ],
    );
    assert!(out.success, "set acl: {}", out.stderr);

    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    for q in &["wc-queue-1", "wc-queue-2"] {
        let result = client.enqueue(q, Default::default(), b"hello").await;
        assert!(result.is_ok(), "{q}: expected Ok with wildcard, got: {result:?}");
    }
}

/// Superadmin key bypasses all ACL checks.
#[tokio::test]
async fn superadmin_key_bypasses_acl() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "superadmin");
    let out = cli_run(
        &addr,
        &["--api-key", &admin_token, "queue", "create", "super-test-q"],
    );
    assert!(out.success, "create queue: {}", out.stderr);

    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&admin_token),
    )
    .await
    .expect("connect");

    let result = client
        .enqueue("super-test-q", Default::default(), b"hello")
        .await;
    assert!(
        result.is_ok(),
        "superadmin should bypass ACL: {result:?}"
    );
}

/// Admin operations require a key with `admin:*` permission (or superadmin).
#[tokio::test]
async fn admin_operations_require_admin_permission() {
    let (_server, addr) = start_auth_server();

    // Key with only produce:* — should fail admin operations.
    let (key_id, token) = cli_create_regular_key(&addr, "producer-only");

    // Grant produce:* but not admin.
    // We need a superadmin key to call set_acl.
    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &key_id,
            "--perm",
            "produce:*",
        ],
    );
    assert!(out.success, "set acl: {}", out.stderr);

    // Attempt to create a queue using the produce-only key → PERMISSION_DENIED.
    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &token,
            "queue",
            "create",
            "should-fail-queue",
        ],
    );
    assert!(
        !out.success,
        "create queue with produce-only key should fail"
    );
    assert!(
        out.stderr.contains("permission") || out.stderr.contains("Error"),
        "expected permission error, got: {}",
        out.stderr
    );
}

/// `fila auth acl get` shows permissions for a key.
#[test]
fn cli_acl_get_shows_permissions() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    let (key_id, _) = cli_create_regular_key(&addr, "show-perms-key");

    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &key_id,
            "--perm",
            "produce:orders",
            "--perm",
            "consume:orders",
        ],
    );
    assert!(out.success, "set acl: {}", out.stderr);

    let out = cli_run(
        &addr,
        &["--api-key", &admin_token, "auth", "acl", "get", &key_id],
    );
    assert!(out.success, "get acl: {}", out.stderr);
    assert!(
        out.stdout.contains("produce:orders"),
        "expected produce:orders in output: {}",
        out.stdout
    );
    assert!(
        out.stdout.contains("consume:orders"),
        "expected consume:orders in output: {}",
        out.stdout
    );
}

/// `fila auth acl get` shows superadmin status.
#[test]
fn cli_acl_get_shows_superadmin() {
    let (_server, addr) = start_auth_server();

    let (key_id, admin_token) = cli_create_superadmin_key(&addr, "super-key");

    let out = cli_run(
        &addr,
        &["--api-key", &admin_token, "auth", "acl", "get", &key_id],
    );
    assert!(out.success, "get acl: {}", out.stderr);
    assert!(
        out.stdout.contains("Superadmin"),
        "expected Superadmin in output: {}",
        out.stdout
    );
}
