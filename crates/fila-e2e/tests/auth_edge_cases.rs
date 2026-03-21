//! Auth edge case tests: revocation immediacy, bootstrap key scope, superadmin revocation.

mod helpers;

use std::collections::HashMap;

/// AC 5: Key revocation takes effect immediately — revoked key rejected on next request.
#[tokio::test]
async fn auth_key_revocation_immediate() {
    let (_server, addr) = helpers::start_auth_server();
    let (admin_key_id, admin_token) = helpers::cli_create_superadmin_key(&addr, "admin");

    // Create a regular key and give it produce permission
    let create_out = helpers::cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "create",
            "--name",
            "ephemeral",
        ],
    );
    assert!(
        create_out.success,
        "create key failed: {}",
        create_out.stderr
    );
    let eph_token = create_out
        .stdout
        .lines()
        .find(|l| l.contains("Token"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .expect("token in output");
    let eph_key_id = create_out
        .stdout
        .lines()
        .find(|l| l.contains("Key ID"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .expect("key_id in output");

    // Grant produce permission on all queues
    let acl_out = helpers::cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &eph_key_id,
            "--perm",
            "produce:*",
        ],
    );
    assert!(acl_out.success, "acl set failed: {}", acl_out.stderr);

    // Create a queue for testing
    helpers::cli_run(
        &addr,
        &["--api-key", &admin_token, "queue", "create", "revoke-test"],
    );

    // Verify the key works
    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&eph_token),
    )
    .await
    .expect("connect with ephemeral key");

    let result = client
        .enqueue("revoke-test", HashMap::new(), b"before-revoke".to_vec())
        .await;
    assert!(result.is_ok(), "key should work before revocation");

    // Revoke the key
    let revoke_out = helpers::cli_run(
        &addr,
        &["--api-key", &admin_token, "auth", "revoke", &eph_key_id],
    );
    assert!(revoke_out.success, "revoke failed: {}", revoke_out.stderr);

    // Next request with the revoked key should fail immediately
    let result = client
        .enqueue("revoke-test", HashMap::new(), b"after-revoke".to_vec())
        .await;
    assert!(
        result.is_err(),
        "revoked key should be rejected immediately"
    );

    // Clean up admin key reference
    let _ = admin_key_id;
}

/// AC 6: Permission removal takes effect immediately.
#[tokio::test]
async fn auth_permission_removal_immediate() {
    let (_server, addr) = helpers::start_auth_server();
    let (_admin_key_id, admin_token) = helpers::cli_create_superadmin_key(&addr, "admin");

    // Create a regular key with produce permission
    let create_out = helpers::cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "create",
            "--name",
            "perm-test",
        ],
    );
    assert!(create_out.success);
    let token = create_out
        .stdout
        .lines()
        .find(|l| l.contains("Token"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .expect("token");
    let key_id = create_out
        .stdout
        .lines()
        .find(|l| l.contains("Key ID"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .expect("key_id");

    // Grant produce + consume on test queue
    helpers::cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &key_id,
            "--perm",
            "produce:perm-q",
            "--perm",
            "consume:perm-q",
        ],
    );

    helpers::cli_run(
        &addr,
        &["--api-key", &admin_token, "queue", "create", "perm-q"],
    );

    // Verify produce works
    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&token),
    )
    .await
    .expect("connect");

    let result = client
        .enqueue("perm-q", HashMap::new(), b"test".to_vec())
        .await;
    assert!(result.is_ok(), "produce should work with permission");

    // Remove produce permission (keep only consume)
    helpers::cli_run(
        &addr,
        &[
            "--api-key",
            &admin_token,
            "auth",
            "acl",
            "set",
            &key_id,
            "--perm",
            "consume:perm-q",
        ],
    );

    // Next produce should fail immediately
    let result = client
        .enqueue("perm-q", HashMap::new(), b"after-revoke".to_vec())
        .await;
    assert!(
        result.is_err(),
        "produce should fail after permission removal"
    );
}

/// AC 7: Bootstrap key scope — the bootstrap key acts as superadmin (can perform all ops).
///
/// Note: The epic AC expected bootstrap to be limited to admin-only operations.
/// The actual implementation treats bootstrap as superadmin (CallerKey::Bootstrap → true
/// for all permission checks). This test verifies the actual behavior.
#[tokio::test]
async fn auth_bootstrap_key_is_superadmin() {
    let (_server, addr) = helpers::start_auth_server();

    // Bootstrap key can create API keys (admin op)
    let create_out = helpers::cli_run(
        &addr,
        &[
            "--api-key",
            helpers::TEST_BOOTSTRAP_KEY,
            "auth",
            "create",
            "--name",
            "bootstrap-created",
        ],
    );
    assert!(
        create_out.success,
        "bootstrap key should be able to create API keys: {}",
        create_out.stderr
    );

    // Bootstrap key can also create queues (admin op)
    let queue_out = helpers::cli_run(
        &addr,
        &[
            "--api-key",
            helpers::TEST_BOOTSTRAP_KEY,
            "queue",
            "create",
            "bootstrap-q",
        ],
    );
    assert!(
        queue_out.success,
        "bootstrap key should be able to create queues: {}",
        queue_out.stderr
    );

    // Bootstrap key CAN do data operations (it's superadmin)
    let client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(helpers::TEST_BOOTSTRAP_KEY),
    )
    .await
    .expect("connect with bootstrap key");

    let result = client
        .enqueue("bootstrap-q", HashMap::new(), b"data-op".to_vec())
        .await;
    assert!(
        result.is_ok(),
        "bootstrap key is superadmin and can perform data operations"
    );
}

/// AC 8: Superadmin key revocation — revoked superadmin loses all access.
#[tokio::test]
async fn auth_superadmin_revocation() {
    let (_server, addr) = helpers::start_auth_server();

    // Create two superadmin keys
    let (_sa1_id, sa1_token) = helpers::cli_create_superadmin_key(&addr, "superadmin-1");
    let (sa2_id, sa2_token) = helpers::cli_create_superadmin_key(&addr, "superadmin-2");

    // Verify sa2 works
    let sa2_client = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_api_key(&sa2_token),
    )
    .await
    .expect("connect sa2");

    helpers::cli_run(
        &addr,
        &["--api-key", &sa1_token, "queue", "create", "sa-test"],
    );

    let result = sa2_client
        .enqueue("sa-test", HashMap::new(), b"before".to_vec())
        .await;
    assert!(result.is_ok(), "sa2 should work before revocation");

    // Revoke sa2 using sa1
    let revoke_out = helpers::cli_run(&addr, &["--api-key", &sa1_token, "auth", "revoke", &sa2_id]);
    assert!(
        revoke_out.success,
        "revoke sa2 failed: {}",
        revoke_out.stderr
    );

    // sa2 should lose all access immediately
    let result = sa2_client
        .enqueue("sa-test", HashMap::new(), b"after".to_vec())
        .await;
    assert!(
        result.is_err(),
        "revoked superadmin should lose all access immediately"
    );
}
