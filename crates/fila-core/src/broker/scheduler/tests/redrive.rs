use super::*;

#[test]
fn redrive_moves_messages_from_dlq_to_parent_and_leasable() {
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "redrive-q", on_failure_script, None);
    scheduler.handle_all_pending(&tx);

    // Dead-letter a message
    let msg_id = dlq_one_message(&tx, &mut scheduler, "redrive-q");

    // Verify message is in DLQ
    let dlq_prefix = crate::storage::keys::message_prefix("redrive-q.dlq");
    let dlq_msgs = scheduler.storage().list_messages(&dlq_prefix).unwrap();
    assert_eq!(dlq_msgs.len(), 1);

    // Redrive
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "redrive-q.dlq".to_string(),
        count: 0,
        reply: reply_tx,
    });
    let redriven = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(redriven, 1);

    // Verify message is back in parent queue storage
    let parent_prefix = crate::storage::keys::message_prefix("redrive-q");
    let parent_msgs = scheduler.storage().list_messages(&parent_prefix).unwrap();
    assert_eq!(parent_msgs.len(), 1);
    assert_eq!(parent_msgs[0].1.id, msg_id);

    // Verify DLQ is now empty
    let dlq_msgs = scheduler.storage().list_messages(&dlq_prefix).unwrap();
    assert!(dlq_msgs.is_empty());

    // Verify message is leasable: register consumer on parent queue
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "redrive-q".to_string(),
        consumer_id: "lease-after-redrive".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    scheduler.handle_all_pending(&tx);

    let delivered = consumer_rx
        .try_recv()
        .expect("redriven message should be leasable");
    assert_eq!(delivered.msg_id, msg_id);
}

#[test]
fn redrive_resets_attempt_count_to_zero() {
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "redrive-attempt-q", on_failure_script, None);
    scheduler.handle_all_pending(&tx);

    dlq_one_message(&tx, &mut scheduler, "redrive-attempt-q");

    // Verify attempt_count in DLQ is 1
    let dlq_prefix = crate::storage::keys::message_prefix("redrive-attempt-q.dlq");
    let dlq_msgs = scheduler.storage().list_messages(&dlq_prefix).unwrap();
    assert_eq!(dlq_msgs[0].1.attempt_count, 1);

    // Redrive
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "redrive-attempt-q.dlq".to_string(),
        count: 0,
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Verify attempt_count reset to 0 in parent queue
    let parent_prefix = crate::storage::keys::message_prefix("redrive-attempt-q");
    let parent_msgs = scheduler.storage().list_messages(&parent_prefix).unwrap();
    assert_eq!(parent_msgs[0].1.attempt_count, 0);
}

#[test]
fn redrive_with_count_limit_only_moves_that_many() {
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "redrive-limit-q", on_failure_script, None);
    scheduler.handle_all_pending(&tx);

    // Dead-letter 3 messages
    for _ in 0..3 {
        dlq_one_message(&tx, &mut scheduler, "redrive-limit-q");
    }

    let dlq_prefix = crate::storage::keys::message_prefix("redrive-limit-q.dlq");
    assert_eq!(
        scheduler
            .storage()
            .list_messages(&dlq_prefix)
            .unwrap()
            .len(),
        3
    );

    // Redrive only 2
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "redrive-limit-q.dlq".to_string(),
        count: 2,
        reply: reply_tx,
    });
    let redriven = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(redriven, 2);

    // 1 still in DLQ, 2 in parent
    assert_eq!(
        scheduler
            .storage()
            .list_messages(&dlq_prefix)
            .unwrap()
            .len(),
        1
    );
    let parent_prefix = crate::storage::keys::message_prefix("redrive-limit-q");
    assert_eq!(
        scheduler
            .storage()
            .list_messages(&parent_prefix)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn redrive_count_zero_moves_all_messages() {
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "redrive-all-q", on_failure_script, None);
    scheduler.handle_all_pending(&tx);

    for _ in 0..3 {
        dlq_one_message(&tx, &mut scheduler, "redrive-all-q");
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "redrive-all-q.dlq".to_string(),
        count: 0,
        reply: reply_tx,
    });
    let redriven = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(redriven, 3);

    let dlq_prefix = crate::storage::keys::message_prefix("redrive-all-q.dlq");
    assert!(scheduler
        .storage()
        .list_messages(&dlq_prefix)
        .unwrap()
        .is_empty());
}

#[test]
fn redrive_non_dlq_queue_returns_not_a_dlq() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "normal-q");
    scheduler.handle_all_pending(&tx);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "normal-q".to_string(),
        count: 0,
        reply: reply_tx,
    });
    let err = reply_rx.blocking_recv().unwrap().unwrap_err();
    assert!(matches!(err, crate::error::RedriveError::NotADLQ(_)));
}

#[test]
fn redrive_nonexistent_queue_returns_not_found() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "nonexistent.dlq".to_string(),
        count: 0,
        reply: reply_tx,
    });
    let err = reply_rx.blocking_recv().unwrap().unwrap_err();
    assert!(matches!(err, crate::error::RedriveError::QueueNotFound(_)));
}

#[test]
fn redrive_parent_deleted_returns_parent_not_found() {
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "redrive-orphan-q", on_failure_script, None);
    scheduler.handle_all_pending(&tx);

    // Dead-letter a message
    dlq_one_message(&tx, &mut scheduler, "redrive-orphan-q");

    // Delete the parent queue
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::DeleteQueue {
        queue_id: "redrive-orphan-q".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Redrive should fail because parent is gone
    // But DeleteQueue cascade-deletes auto DLQ, so the DLQ is also gone.
    // We need to test with a scenario where the DLQ still exists but parent doesn't.
    // Create a manual DLQ-named queue without a parent.
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: "orphan-parent.dlq".to_string(),
        config: crate::queue::QueueConfig::new("orphan-parent.dlq".to_string()),
        reply: reply_tx,
    })
    .unwrap();
    scheduler.handle_all_pending(&tx);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "orphan-parent.dlq".to_string(),
        count: 0,
        reply: reply_tx,
    });
    let err = reply_rx.blocking_recv().unwrap().unwrap_err();
    assert!(matches!(
        err,
        crate::error::RedriveError::ParentQueueNotFound(_)
    ));
}

#[test]
fn redrive_skips_leased_messages_in_dlq() {
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "redrive-leased-q", on_failure_script, None);
    scheduler.handle_all_pending(&tx);

    // Dead-letter 2 messages
    dlq_one_message(&tx, &mut scheduler, "redrive-leased-q");
    dlq_one_message(&tx, &mut scheduler, "redrive-leased-q");

    // Register a consumer on the DLQ to lease one message (capacity=1)
    let (dlq_consumer_tx, mut dlq_consumer_rx) = tokio::sync::mpsc::channel(1);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "redrive-leased-q.dlq".to_string(),
        consumer_id: "dlq-inspector".to_string(),
        tx: dlq_consumer_tx,
    })
    .unwrap();
    scheduler.handle_all_pending(&tx);

    // One message is now leased in the DLQ
    let leased = dlq_consumer_rx
        .try_recv()
        .expect("should lease one DLQ message");

    // Redrive all — should skip the leased one
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Redrive {
        dlq_queue_id: "redrive-leased-q.dlq".to_string(),
        count: 0,
        reply: reply_tx,
    });
    let redriven = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(redriven, 1, "should only redrive the non-leased message");

    // The leased message should still be in DLQ storage
    let dlq_prefix = crate::storage::keys::message_prefix("redrive-leased-q.dlq");
    let dlq_msgs = scheduler.storage().list_messages(&dlq_prefix).unwrap();
    assert_eq!(dlq_msgs.len(), 1);
    assert_eq!(dlq_msgs[0].1.id, leased.msg_id);

    // The redriven message should be in parent queue
    let parent_prefix = crate::storage::keys::message_prefix("redrive-leased-q");
    let parent_msgs = scheduler.storage().list_messages(&parent_prefix).unwrap();
    assert_eq!(parent_msgs.len(), 1);
    // The redriven message should be whichever one was NOT leased
    assert_ne!(parent_msgs[0].1.id, leased.msg_id);
}
