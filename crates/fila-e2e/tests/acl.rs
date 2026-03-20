mod helpers;

use helpers::{cli_create_superadmin_key, cli_run, start_auth_server, TEST_BOOTSTRAP_KEY};

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Create a regular (non-superadmin) API key via CLI and return (key_id, token).
fn cli_create_regular_key(addr: &str, name: &str) -> (String, String) {
    let out = cli_run(
        addr,
        &[
            "--api-key",
            TEST_BOOTSTRAP_KEY,
            "auth",
            "create",
            "--name",
            name,
        ],
    );
    assert!(
        out.success,
        "auth create failed: stderr={}\nstdout={}",
        out.stderr, out.stdout
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

fn assert_permission_denied_enqueue(result: Result<String, fila_sdk::EnqueueError>, context: &str) {
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

    let result = client.enqueue("acl-test-q", Default::default(), b"x").await;
    assert_permission_denied_enqueue(result, "key without permissions");
}

/// Key with `produce:<queue>` permission can enqueue to that queue.
#[tokio::test]
async fn key_with_produce_permission_can_enqueue() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "queue",
            "create",
            "produce-test-q",
        ],
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
        let out = cli_run(&addr, &["--api-key", &admin_token, "queue", "create", q]);
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
        let out = cli_run(&addr, &["--api-key", &admin_token, "queue", "create", q]);
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
        assert!(
            result.is_ok(),
            "{q}: expected Ok with wildcard, got: {result:?}"
        );
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
    assert!(result.is_ok(), "superadmin should bypass ACL: {result:?}");
}

/// Admin operations require a key with `admin:*` permission (or superadmin).
#[tokio::test]
async fn admin_operations_require_admin_permission() {
    let (_server, addr) = start_auth_server();

    // Create the superadmin key first (bootstrap — no keys exist yet).
    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");

    // Create a regular key and grant it produce:* but not admin.
    let (key_id, token) = cli_create_regular_key(&addr, "producer-only");
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
        &["--api-key", &token, "queue", "create", "should-fail-queue"],
    );
    assert!(
        !out.success,
        "create queue with produce-only key should fail; stdout={} stderr={}",
        out.stdout, out.stderr
    );
    // The CLI wraps the gRPC PERMISSION_DENIED status as "Error: key does not have admin permission".
    assert!(
        out.stderr.contains("admin permission"),
        "expected admin permission error, got: {}",
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

/// Key without consume permission cannot nack.
#[tokio::test]
async fn key_without_consume_permission_cannot_nack() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    let out = cli_run(
        &addr,
        &["--api-key", &admin_token, "queue", "create", "nack-acl-q"],
    );
    assert!(out.success, "create queue: {}", out.stderr);

    // A key with only produce permission cannot nack.
    let (key_id, token) = cli_create_regular_key(&addr, "produce-only-key");
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
            "produce:nack-acl-q",
        ],
    );
    assert!(out.success, "set acl: {}", out.stderr);

    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    // The ACL check fires before any message lookup, so a bogus message_id is fine here.
    let result = client
        .nack("nack-acl-q", "00000000-0000-0000-0000-000000000000", "err")
        .await;
    match result {
        Err(fila_sdk::NackError::Status(fila_sdk::StatusError::Rpc { code, .. }))
            if code == tonic::Code::PermissionDenied => {}
        other => panic!("expected PERMISSION_DENIED, got: {other:?}"),
    }
}

/// `fila auth acl set` rejects unknown permission kinds.
#[test]
fn cli_acl_set_rejects_invalid_kind() {
    let (_server, addr) = start_auth_server();

    let (_, admin_token) = cli_create_superadmin_key(&addr, "admin");
    let (key_id, _) = cli_create_regular_key(&addr, "target-key");

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
            "read:some-queue",
        ],
    );
    assert!(
        !out.success,
        "expected failure for invalid kind; stdout={} stderr={}",
        out.stdout, out.stderr
    );
    assert!(
        out.stderr.contains("invalid permission kind") || out.stderr.contains("invalid"),
        "expected invalid kind error, got: {}",
        out.stderr
    );
}

/// A key with admin permission can create a new API key (non-superadmin).
#[test]
fn key_with_admin_permission_can_create_api_key() {
    let (_server, addr) = start_auth_server();

    // Bootstrap creates the first superadmin.
    let (_, admin_token) = cli_create_superadmin_key(&addr, "superadmin");

    // Create a key with admin:* and verify it can create another key.
    let (key_id, admin_key_token) = cli_create_regular_key(&addr, "admin-key");
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
            "admin:*",
        ],
    );
    assert!(out.success, "set acl: {}", out.stderr);

    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &admin_key_token,
            "auth",
            "create",
            "--name",
            "new-key",
        ],
    );
    assert!(
        out.success,
        "admin key should be able to create keys; stderr={} stdout={}",
        out.stderr, out.stdout
    );
}

/// A key without admin permission cannot create a new API key.
#[test]
fn key_without_admin_permission_cannot_create_api_key() {
    let (_server, addr) = start_auth_server();

    let (_, _) = cli_create_superadmin_key(&addr, "admin");
    // Regular key with no permissions at all.
    let (_, no_perm_token) = cli_create_regular_key(&addr, "no-perm-key");

    let out = cli_run(
        &addr,
        &[
            "--api-key",
            &no_perm_token,
            "auth",
            "create",
            "--name",
            "sneaky-key",
        ],
    );
    assert!(
        !out.success,
        "non-admin key should not be able to create keys; stdout={} stderr={}",
        out.stdout, out.stderr
    );
    assert!(
        out.stderr.contains("admin permission") || out.stderr.contains("permission"),
        "expected permission error, got: {}",
        out.stderr
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
