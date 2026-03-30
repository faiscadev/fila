mod helpers;

use helpers::{cli_create_superadmin_key, cli_run, start_auth_server, TestServer};

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

/// AC 1 + 10: auth disabled by default — plain server accepts requests without key.
#[tokio::test]
async fn auth_disabled_requests_succeed_without_key() {
    let server = TestServer::start();
    let client = fila_sdk::FilaClient::connect(server.binary_addr())
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

/// AC 2: auth enabled, request without key → Unauthorized.
///
/// With the binary protocol, the Handshake frame without an API key is rejected
/// during connection when auth is enabled, so connect itself fails.
#[tokio::test]
async fn auth_enabled_request_without_key_is_rejected() {
    let (server, _addr) = start_auth_server();

    // Without an API key, the handshake should fail when auth is enabled.
    let result = fila_sdk::FilaClient::connect(server.binary_addr()).await;
    assert!(
        result.is_err(),
        "expected connect to fail without API key when auth is enabled"
    );
}

/// AC 3: auth enabled, invalid key → handshake failure.
#[tokio::test]
async fn auth_enabled_invalid_key_is_rejected() {
    let (server, _addr) = start_auth_server();

    let result = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(server.binary_addr())
            .with_api_key("invalid-key-that-does-not-exist"),
    )
    .await;

    assert!(
        result.is_err(),
        "expected connect to fail with invalid API key"
    );
}

/// AC 4: auth enabled, valid key → request succeeds.
#[tokio::test]
async fn auth_enabled_valid_key_allows_request() {
    let (server, addr) = start_auth_server();

    // Create a superadmin key via CLI (superadmin needed for queue create).
    let (_, token) = cli_create_superadmin_key(&addr, "test-key");

    // Create the queue first using the valid key.
    let out = cli_run(
        &addr,
        &["--api-key", &token, "queue", "create", "auth-valid-test"],
    );
    assert!(out.success, "create queue: {}", out.stderr);

    // SDK client with the valid key should enqueue successfully.
    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(server.binary_addr()).with_api_key(&token),
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

/// AC 4 (revoke): revoked key is rejected on next connection.
#[tokio::test]
async fn auth_enabled_revoked_key_is_rejected() {
    let (server, addr) = start_auth_server();

    // Create a superadmin key (superadmin needed so revoke itself works).
    let (key_id, token) = cli_create_superadmin_key(&addr, "revoke-test-key");

    // Verify the key works first.
    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(server.binary_addr()).with_api_key(&token),
    )
    .await
    .expect("connect");

    // Revoke the key using itself for auth (RevokeApiKey requires a valid key).
    let out = cli_run(&addr, &["--api-key", &token, "auth", "revoke", &key_id]);
    assert!(out.success, "revoke: {}", out.stderr);

    // Now the same key should be rejected. With binary protocol, the existing
    // connection is already authenticated, so we need to try a new connection
    // to observe the rejection.
    let result = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(server.binary_addr()).with_api_key(&token),
    )
    .await;

    // The revoked key should fail during handshake.
    assert!(result.is_err(), "expected connect with revoked key to fail");
    drop(client); // ensure original client is kept alive until assertion
}
