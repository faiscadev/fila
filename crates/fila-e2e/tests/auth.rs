mod helpers;

use helpers::{cli_create_superadmin_key, cli_run, start_auth_server, TestServer};

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

/// Assert that an EnqueueError is specifically an authentication failure.
/// With FIBP, auth failures come back as StatusError::Internal or StatusError::Unavailable
/// containing "auth" or "unauthenticated" in the message.
fn assert_unauthenticated(result: Result<String, fila_sdk::EnqueueError>, context: &str) {
    match result {
        Ok(_) => panic!("{context}: expected auth error, got Ok"),
        Err(e) => {
            let msg = format!("{e:?}").to_lowercase();
            assert!(
                msg.contains("auth") || msg.contains("unauthenticated") || msg.contains("denied"),
                "{context}: expected auth error, got: {e:?}"
            );
        }
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

    // Create a superadmin key (superadmin needed so revoke itself works).
    let (key_id, token) = cli_create_superadmin_key(&addr, "revoke-test-key");

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
