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

/// GetServerInfo bypasses auth — clients can query version info without a key.
#[tokio::test]
async fn get_server_info_succeeds_without_auth() {
    let (_server, addr) = start_auth_server();

    // No API key provided — should still succeed because GetServerInfo is unauthenticated.
    let client = fila_sdk::FilaClient::connect(&addr).await.expect("connect");
    let info = client.get_server_info().await.expect("get_server_info should succeed without auth");

    assert!(!info.server_version.is_empty(), "server_version should not be empty");
    assert_eq!(info.proto_version, "v1");
    assert!(
        info.features.contains(&"fair_scheduling".to_string()),
        "expected fair_scheduling feature in {:?}",
        info.features
    );
}

/// GetServerInfo returns valid semver and features on a plain (auth-disabled) server.
#[tokio::test]
async fn get_server_info_returns_valid_data() {
    let server = helpers::TestServer::start();
    let client = fila_sdk::FilaClient::connect(server.addr()).await.expect("connect");
    let info = client.get_server_info().await.expect("get_server_info");

    // Verify semver format.
    let parts: Vec<&str> = info.server_version.split('.').collect();
    assert_eq!(parts.len(), 3, "version should be MAJOR.MINOR.PATCH");
    for part in &parts {
        part.parse::<u32>().expect("each version segment should be a number");
    }

    assert_eq!(info.proto_version, "v1");
    assert!(!info.features.is_empty(), "features list should not be empty");
}
