mod helpers;

/// AC 5i: Queue management — create, list, inspect, delete via CLI.
#[tokio::test]
async fn e2e_queue_management_lifecycle() {
    let server = helpers::TestServer::start();

    // Create a queue
    let create = helpers::cli_run(server.addr(), &["queue", "create", "mgmt-test"]);
    assert!(create.success, "create failed: {}", create.stderr);
    assert!(
        create.stdout.contains("Created queue") && create.stdout.contains("mgmt-test"),
        "unexpected output: {}",
        create.stdout
    );

    // List queues — should include our queue
    let list = helpers::cli_run(server.addr(), &["queue", "list"]);
    assert!(list.success, "list failed: {}", list.stderr);
    assert!(
        list.stdout.contains("mgmt-test"),
        "queue list should include 'mgmt-test', got: {}",
        list.stdout
    );

    // Also verify the auto-created DLQ appears
    assert!(
        list.stdout.contains("mgmt-test.dlq"),
        "queue list should include auto-created DLQ 'mgmt-test.dlq', got: {}",
        list.stdout
    );

    // Inspect the queue
    let inspect = helpers::cli_run(server.addr(), &["queue", "inspect", "mgmt-test"]);
    assert!(inspect.success, "inspect failed: {}", inspect.stderr);
    assert!(
        inspect.stdout.contains("Queue: mgmt-test"),
        "inspect output should show queue name, got: {}",
        inspect.stdout
    );
    assert!(
        inspect.stdout.contains("Depth:"),
        "inspect output should show depth, got: {}",
        inspect.stdout
    );

    // Delete the queue
    let delete = helpers::cli_run(server.addr(), &["queue", "delete", "mgmt-test"]);
    assert!(delete.success, "delete failed: {}", delete.stderr);
    assert!(
        delete.stdout.contains("Deleted queue"),
        "unexpected delete output: {}",
        delete.stdout
    );

    // Verify queue is gone
    let list2 = helpers::cli_run(server.addr(), &["queue", "list"]);
    assert!(list2.success);
    // The main queue should be gone (the DLQ may or may not be cleaned up depending on implementation)
    let lines_with_mgmt: Vec<&str> = list2
        .stdout
        .lines()
        .filter(|l| l.contains("mgmt-test") && !l.contains("mgmt-test.dlq"))
        .collect();
    assert!(
        lines_with_mgmt.is_empty(),
        "deleted queue 'mgmt-test' should not appear in list (excluding DLQ), got: {lines_with_mgmt:?}"
    );
}
