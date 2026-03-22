use super::*;

// --- Dead-letter queue auto-creation tests (Story 3.4) ---

#[test]
fn dlq_auto_created_on_queue_creation() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "my-queue");
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Both the queue and its auto-created DLQ should exist
    let queue = scheduler.storage().get_queue("my-queue").unwrap();
    assert!(queue.is_some(), "main queue should exist");
    assert_eq!(
        queue.unwrap().dlq_queue_id,
        Some("my-queue.dlq".to_string()),
        "dlq_queue_id should be set"
    );

    let dlq = scheduler.storage().get_queue("my-queue.dlq").unwrap();
    assert!(dlq.is_some(), "DLQ should be auto-created");
}

#[test]
fn dlq_not_created_for_dlq_queue() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create a queue with .dlq suffix — no DLQ-of-DLQ should be created
    send_create_queue(&tx, "orphan.dlq");
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let queue = scheduler.storage().get_queue("orphan.dlq").unwrap();
    assert!(queue.is_some(), "DLQ queue should exist");
    assert_eq!(
        queue.unwrap().dlq_queue_id,
        None,
        "DLQ queue should not have its own DLQ"
    );

    // Verify no "orphan.dlq.dlq" was created
    let nested_dlq = scheduler.storage().get_queue("orphan.dlq.dlq").unwrap();
    assert!(nested_dlq.is_none(), "DLQ-of-DLQ should not exist");
}

#[test]
fn delete_queue_also_deletes_dlq() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "deletable-queue");

    // Verify both exist
    let (del_reply_tx, mut del_reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::DeleteQueue {
        queue_id: "deletable-queue".to_string(),
        reply: del_reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(
        del_reply_rx.try_recv().unwrap().is_ok(),
        "delete should succeed"
    );

    // Both the queue and its DLQ should be gone
    assert!(
        scheduler
            .storage()
            .get_queue("deletable-queue")
            .unwrap()
            .is_none(),
        "main queue should be deleted"
    );
    assert!(
        scheduler
            .storage()
            .get_queue("deletable-queue.dlq")
            .unwrap()
            .is_none(),
        "DLQ should also be deleted"
    );
}

#[test]
fn dlq_full_flow_enqueue_nack_dlq_lease() {
    // Full flow: enqueue → nack with dlq action → verify message in DLQ → lease from DLQ
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "flow-queue", on_failure_script, None);

    // Register consumers for both main queue and auto-created DLQ
    let (main_tx, mut main_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "flow-queue".to_string(),
        consumer_id: "main-c".to_string(),
        tx: main_tx,
    })
    .unwrap();

    let (dlq_tx, mut dlq_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "flow-queue.dlq".to_string(),
        consumer_id: "dlq-c".to_string(),
        tx: dlq_tx,
    })
    .unwrap();

    // Enqueue message
    let mut msg = test_message("flow-queue");
    msg.headers.insert("tenant".to_string(), "acme".to_string());
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Nack — triggers on_failure which returns "dlq"
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "flow-queue".to_string(),
        msg_id,
        error: "permanent failure".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(nack_rx.try_recv().unwrap().is_ok());

    // Initial delivery on main queue
    let initial = main_rx.try_recv().expect("initial delivery");
    assert_eq!(initial.msg_id, msg_id);
    assert_eq!(initial.attempt_count, 0);

    // No retry on main queue
    assert!(
        main_rx.try_recv().is_err(),
        "should not retry on main queue"
    );

    // DLQ consumer receives the message (leased from DLQ like any regular queue)
    let dlq_delivery = dlq_rx.try_recv().expect("DLQ delivery");
    assert_eq!(dlq_delivery.msg_id, msg_id);
    assert_eq!(dlq_delivery.attempt_count, 1);

    // Verify original metadata preserved: message should have the tenant header
    let dlq_prefix = crate::storage::keys::message_prefix("flow-queue.dlq");
    let dlq_msgs = scheduler.storage().list_messages(&dlq_prefix).unwrap();
    assert_eq!(dlq_msgs.len(), 1);
    let stored_msg = &dlq_msgs[0].1;
    assert_eq!(stored_msg.id, msg_id);
    assert_eq!(
        stored_msg.headers.get("tenant"),
        Some(&"acme".to_string()),
        "original headers should be preserved in DLQ"
    );

    // Message should be gone from the original queue
    let main_prefix = crate::storage::keys::message_prefix("flow-queue");
    let main_msgs = scheduler.storage().list_messages(&main_prefix).unwrap();
    assert!(
        main_msgs.is_empty(),
        "message should be removed from main queue"
    );
}

#[test]
fn dlq_queue_clears_caller_provided_dlq_queue_id() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create a .dlq queue with a caller-provided dlq_queue_id — it should be cleared
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut config = crate::queue::QueueConfig::new("my-queue.dlq".to_string());
    config.dlq_queue_id = Some("sneaky-nested.dlq".to_string());
    tx.send(SchedulerCommand::CreateQueue {
        name: "my-queue.dlq".to_string(),
        config,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(reply_rx.try_recv().unwrap().is_ok());
    let queue = scheduler
        .storage()
        .get_queue("my-queue.dlq")
        .unwrap()
        .unwrap();
    assert_eq!(
        queue.dlq_queue_id, None,
        "dlq_queue_id should be cleared for .dlq queues"
    );
}

#[test]
fn delete_queue_does_not_cascade_delete_custom_dlq() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create a shared DLQ first
    send_create_queue(&tx, "shared.dlq");

    // Create a queue and manually override its dlq_queue_id to point to shared DLQ
    // (simulating a custom DLQ that doesn't match {queue}.dlq naming)
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    let mut config = crate::queue::QueueConfig::new("custom-dlq-queue".to_string());
    config.dlq_queue_id = Some("shared.dlq".to_string());
    tx.send(SchedulerCommand::CreateQueue {
        name: "custom-dlq-queue".to_string(),
        config,
        reply: reply_tx,
    })
    .unwrap();

    // Delete the queue
    let (del_tx, mut del_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::DeleteQueue {
        queue_id: "custom-dlq-queue".to_string(),
        reply: del_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(del_rx.try_recv().unwrap().is_ok());

    // The custom/shared DLQ should NOT have been deleted
    assert!(
        scheduler
            .storage()
            .get_queue("shared.dlq")
            .unwrap()
            .is_some(),
        "custom/shared DLQ should survive parent queue deletion"
    );
}
