mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5h: Config — set/get/list via CLI, Lua fila.get() reads value.
#[tokio::test]
async fn e2e_config_set_get_list_lua() {
    let server = helpers::TestServer::start();

    // Set a config value via CLI
    let set = helpers::cli_run(server.addr(), &["config", "set", "myapp.region", "us-east-1"]);
    assert!(set.success, "config set failed: {}", set.stderr);
    assert!(set.stdout.contains("us-east-1"), "expected confirmation, got: {}", set.stdout);

    // Get the value via CLI
    let get = helpers::cli_run(server.addr(), &["config", "get", "myapp.region"]);
    assert!(get.success, "config get failed: {}", get.stderr);
    assert!(
        get.stdout.contains("us-east-1"),
        "expected value in output, got: {}",
        get.stdout
    );

    // Set another value
    let set2 = helpers::cli_run(server.addr(), &["config", "set", "myapp.env", "production"]);
    assert!(set2.success);

    // List all config entries
    let list_all = helpers::cli_run(server.addr(), &["config", "list"]);
    assert!(list_all.success, "config list failed: {}", list_all.stderr);
    assert!(list_all.stdout.contains("myapp.region"), "list should include myapp.region");
    assert!(list_all.stdout.contains("myapp.env"), "list should include myapp.env");

    // List with prefix filter
    let list_prefix = helpers::cli_run(server.addr(), &["config", "list", "--prefix", "myapp."]);
    assert!(list_prefix.success);
    assert!(list_prefix.stdout.contains("myapp.region"));
    assert!(list_prefix.stdout.contains("myapp.env"));

    // Verify Lua fila.get() reads the config value.
    // Create a queue with on_enqueue that uses fila.get("myapp.region") as fairness key.
    let on_enqueue = r#"function on_enqueue(msg) return { fairness_key = fila.get("myapp.region") or "unknown", weight = 1, throttle_keys = {} } end"#;

    helpers::create_queue_with_scripts_cli(
        server.addr(),
        "config-lua",
        Some(on_enqueue),
        None,
        None,
    );

    let client = helpers::sdk_client(server.addr()).await;

    let msg_id = client
        .enqueue("config-lua", HashMap::new(), b"test".to_vec())
        .await
        .unwrap();

    let mut stream = client.lease("config-lua").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(msg.id, msg_id);
    assert_eq!(
        msg.fairness_key, "us-east-1",
        "Lua fila.get() should have read the config value as fairness key"
    );

    client.ack("config-lua", &msg.id).await.unwrap();
}
