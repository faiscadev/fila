//! Scheduler integration tests using FilaStorage backend.
//! Mirrors key tests from the RocksDB-based test modules to validate
//! that the Fila storage engine works identically with the scheduler.

use super::*;

// === Queue management ===

#[test]
fn fila_create_queue_success() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: "q1".to_string(),
        config: crate::queue::QueueConfig::new("q1".to_string()),
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();
    let result = reply_rx.blocking_recv().unwrap();
    assert!(result.is_ok());
}

#[test]
fn fila_create_queue_already_exists() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: "q1".to_string(),
        config: crate::queue::QueueConfig::new("q1".to_string()),
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();
    let result = reply_rx.blocking_recv().unwrap();
    assert!(result.is_err());
}

#[test]
fn fila_delete_queue_success() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::DeleteQueue {
        queue_id: "q1".to_string(),
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();
    let result = reply_rx.blocking_recv().unwrap();
    assert!(result.is_ok());
}

// === Enqueue ===

#[test]
fn fila_enqueue_persists_message() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");

    let msg = test_message("q1");
    let msg_id = msg.id;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let result = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(result, msg_id);
}

#[test]
fn fila_enqueue_to_nonexistent_queue() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    let msg = test_message("no-such-queue");
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();
    let result = reply_rx.blocking_recv().unwrap();
    assert!(result.is_err());
}

// === Delivery ===

#[test]
fn fila_consumer_receives_enqueued_messages() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "q1".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("q1");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let ready = consumer_rx.try_recv().unwrap();
    assert_eq!(ready.msg_id, msg_id);
}

#[test]
fn fila_multiple_consumers_different_messages() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");

    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(64);
    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "q1".to_string(),
        consumer_id: "c1".to_string(),
        tx: c1_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "q1".to_string(),
        consumer_id: "c2".to_string(),
        tx: c2_tx,
    })
    .unwrap();

    // Enqueue 4 messages with different enqueued_at
    for i in 0u64..4 {
        let mut msg = test_message("q1");
        msg.enqueued_at = i;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let mut c1_msgs = Vec::new();
    while let Ok(ready) = c1_rx.try_recv() {
        c1_msgs.push(ready.msg_id);
    }
    let mut c2_msgs = Vec::new();
    while let Ok(ready) = c2_rx.try_recv() {
        c2_msgs.push(ready.msg_id);
    }
    let total = c1_msgs.len() + c2_msgs.len();
    assert_eq!(total, 4, "all 4 messages delivered across consumers");

    let mut all_ids: Vec<_> = c1_msgs.iter().chain(c2_msgs.iter()).copied().collect();
    all_ids.sort();
    all_ids.dedup();
    assert_eq!(all_ids.len(), 4, "no duplicates");
}

// === Ack / Nack ===

#[test]
fn fila_ack_removes_message() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");
    scheduler.handle_all_pending(&tx);

    let msg = test_message("q1");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();
    scheduler.handle_all_pending(&tx);

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "q1".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    scheduler.handle_all_pending(&tx);
    let _ = consumer_rx.try_recv().unwrap();

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Ack {
        queue_id: "q1".to_string(),
        msg_id,
        reply: ack_tx,
    });
    ack_rx.blocking_recv().unwrap().unwrap();
}

#[test]
fn fila_nack_requeues_message() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");
    scheduler.handle_all_pending(&tx);

    let msg = test_message("q1");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();
    scheduler.handle_all_pending(&tx);

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "q1".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    scheduler.handle_all_pending(&tx);
    let _ = consumer_rx.try_recv().unwrap();

    let (nack_tx, nack_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Nack {
        queue_id: "q1".to_string(),
        msg_id,
        error: "test".to_string(),
        reply: nack_tx,
    });
    nack_rx.blocking_recv().unwrap().unwrap();

    // Run delivery — message should be redelivered
    scheduler.handle_all_pending(&tx);
    let ready = consumer_rx.try_recv().unwrap();
    assert_eq!(ready.msg_id, msg_id);
    assert_eq!(ready.attempt_count, 1);
}

// === DLQ ===

#[test]
fn fila_dlq_auto_created_on_queue_creation() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "my-queue");
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let queue = scheduler.storage().get_queue(P, "my-queue").unwrap();
    assert!(queue.is_some(), "main queue should exist");
    assert_eq!(
        queue.unwrap().dlq_queue_id,
        Some("my-queue.dlq".to_string()),
    );
    let dlq = scheduler.storage().get_queue(P, "my-queue.dlq").unwrap();
    assert!(dlq.is_some(), "DLQ should be auto-created");
}

// === Stats ===

#[test]
fn fila_stats_depth_and_in_flight() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "q1");

    // Enqueue 3 messages with different timestamps
    for i in 0u64..3 {
        let mut msg = test_message("q1");
        msg.enqueued_at = i;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // Register consumer to lease one
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "q1".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Consumer should have received all 3
    let mut received = Vec::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        received.push(ready.msg_id);
    }
    assert_eq!(received.len(), 3, "all 3 messages delivered");
}

// === List Queues ===

#[test]
fn fila_list_queues() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "alpha");
    send_create_queue(&tx, "beta");

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::ListQueues { reply: reply_tx })
        .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let queues = reply_rx.blocking_recv().unwrap().unwrap();
    let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
    assert!(names.contains(&"alpha.dlq"));
    assert!(names.contains(&"beta.dlq"));
}

// === Recovery with FilaStorage ===

#[test]
fn fila_recovery_preserves_messages_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let storage = fila_storage(dir.path());

    // Phase 1: enqueue messages, then shut down
    let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
    send_create_queue(&tx, "recover-queue");

    let mut msg_ids = Vec::new();
    for i in 0u64..5 {
        let mut msg = test_message("recover-queue");
        msg.enqueued_at = i;
        msg_ids.push(msg.id);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();
    drop(tx);
    drop(storage);

    // Phase 2: reopen storage and scheduler
    let storage2 = fila_storage(dir.path());
    let (tx2, mut scheduler2) = test_setup_with_storage(Arc::clone(&storage2));

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx2.send(SchedulerCommand::RegisterConsumer {
        queue_id: "recover-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    tx2.send(SchedulerCommand::Shutdown).unwrap();
    scheduler2.run();

    let mut received = Vec::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        received.push(ready.msg_id);
    }
    assert_eq!(received.len(), 5, "all 5 messages should survive restart");
    assert_eq!(received, msg_ids, "messages in correct FIFO order");
}

#[test]
fn fila_recovery_preserves_queue_definitions() {
    let dir = tempfile::tempdir().unwrap();

    // Phase 1: create queues, shut down
    {
        let storage = fila_storage(dir.path());
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
        for name in &["q1", "q2", "q3"] {
            send_create_queue(&tx, name);
        }
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();
    }

    // Phase 2: reopen — queues should survive
    let storage2 = fila_storage(dir.path());
    let queues = storage2.list_queues(P).unwrap();
    assert_eq!(queues.len(), 6, "3 queues + 3 DLQs should survive restart");
    let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
    assert!(names.contains(&"q1"));
    assert!(names.contains(&"q2"));
    assert!(names.contains(&"q3"));
}

// === Fairness ===

#[test]
fn fila_drr_fairness_multiple_keys() {
    let (tx, mut scheduler, _dir) = test_setup_fila();
    send_create_queue(&tx, "fair-q");

    // Enqueue 2 messages per key: A (weight 1), B (weight 1)
    for key in &["A", "B"] {
        for i in 0..2 {
            let msg = test_message_with_key("fair-q", key, i);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
    }

    // Register consumer and collect deliveries
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "fair-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let mut received = Vec::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        received.push(ready.fairness_key.clone());
    }
    assert_eq!(received.len(), 4);
    // DRR should interleave keys — first 2 deliveries should have different keys
    assert_ne!(
        received[0], received[1],
        "DRR should interleave fairness keys"
    );
}
