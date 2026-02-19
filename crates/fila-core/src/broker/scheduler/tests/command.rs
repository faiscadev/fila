use super::*;

#[test]
fn shutdown_causes_scheduler_to_stop() {
    let (tx, mut scheduler, _dir) = test_setup();

    tx.send(SchedulerCommand::Shutdown).unwrap();

    // Run should return after processing the shutdown command
    scheduler.run();
    // If we get here, the scheduler stopped correctly
}

#[test]
fn commands_processed_in_fifo_order() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create the queue first so enqueue succeeds
    send_create_queue(&tx, "q1");

    let mut expected_ids = Vec::new();
    let mut receivers = Vec::new();

    // Send 5 enqueue commands
    for _ in 0..5 {
        let msg = test_message("q1");
        expected_ids.push(msg.id);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
        receivers.push(reply_rx);
    }

    // Send shutdown so the scheduler stops
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    // Verify all replies were received and IDs match (FIFO order)
    for (i, mut rx) in receivers.into_iter().enumerate() {
        let result = rx.try_recv().unwrap().unwrap();
        assert_eq!(result, expected_ids[i], "command {i} should return its ID");
    }
}

#[test]
fn enqueue_reply_received() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "test-queue");

    let msg = test_message("test-queue");
    let msg_id = msg.id;
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    let result = reply_rx.try_recv().unwrap().unwrap();
    assert_eq!(result, msg_id);
}

#[test]
fn ack_without_lease_returns_error() {
    let (tx, mut scheduler, _dir) = test_setup();

    let msg_id = Uuid::now_v7();
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

    tx.send(SchedulerCommand::Ack {
        queue_id: "q1".to_string(),
        msg_id,
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    // Ack without a lease should fail
    let err = reply_rx.try_recv().unwrap().unwrap_err();
    assert!(matches!(err, crate::error::AckError::MessageNotFound(_)));
}

#[test]
fn channel_disconnect_stops_scheduler() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Drop the sender so the channel disconnects
    drop(tx);

    // Scheduler should detect disconnection and stop
    scheduler.run();
    // If we get here, it handled disconnection correctly
}

#[test]
fn create_queue_success() {
    let (tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let config = crate::queue::QueueConfig::new("test-queue".to_string());

    tx.send(SchedulerCommand::CreateQueue {
        name: "test-queue".to_string(),
        config,
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    let result = reply_rx.try_recv().unwrap().unwrap();
    assert_eq!(result, "test-queue");
}

#[test]
fn create_queue_already_exists() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create queue first time
    let (reply_tx1, mut reply_rx1) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: "dup-queue".to_string(),
        config: crate::queue::QueueConfig::new("dup-queue".to_string()),
        reply: reply_tx1,
    })
    .unwrap();

    // Try to create same queue again
    let (reply_tx2, mut reply_rx2) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: "dup-queue".to_string(),
        config: crate::queue::QueueConfig::new("dup-queue".to_string()),
        reply: reply_tx2,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(reply_rx1.try_recv().unwrap().is_ok());
    let err = reply_rx2.try_recv().unwrap().unwrap_err();
    assert!(
        matches!(err, crate::error::CreateQueueError::QueueAlreadyExists(_)),
        "expected QueueAlreadyExists, got {err:?}"
    );
}

#[test]
fn delete_queue_success() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create then delete
    let (create_tx, mut create_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: "del-queue".to_string(),
        config: crate::queue::QueueConfig::new("del-queue".to_string()),
        reply: create_tx,
    })
    .unwrap();

    let (del_tx, mut del_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::DeleteQueue {
        queue_id: "del-queue".to_string(),
        reply: del_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(create_rx.try_recv().unwrap().is_ok());
    assert!(del_rx.try_recv().unwrap().is_ok());
}

#[test]
fn delete_queue_not_found() {
    let (tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::DeleteQueue {
        queue_id: "nonexistent".to_string(),
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    let err = reply_rx.try_recv().unwrap().unwrap_err();
    assert!(
        matches!(err, crate::error::DeleteQueueError::QueueNotFound(_)),
        "expected QueueNotFound, got {err:?}"
    );
}
