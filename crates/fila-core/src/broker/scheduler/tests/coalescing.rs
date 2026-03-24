use super::*;

/// Single-message fast path: when only one enqueue is in the channel,
/// it is processed immediately with no batching delay.
#[test]
fn single_message_fast_path() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "fast-path-queue");
    scheduler.handle_all_pending(&tx);

    let msg = test_message("fast-path-queue");
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

    // Verify message was persisted
    let prefix = crate::storage::keys::message_prefix("fast-path-queue");
    let stored = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(stored.len(), 1, "single message should be persisted");
    assert_eq!(stored[0].1.id, msg_id);
}

/// Multiple concurrent enqueues are coalesced into a single storage write.
/// All messages should be persisted and all callers should receive their msg_ids.
#[test]
fn multiple_enqueues_coalesced() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "coalesce-queue");
    scheduler.handle_all_pending(&tx);

    let count = 50;
    let mut receivers = Vec::with_capacity(count);
    let mut expected_ids = Vec::with_capacity(count);

    for i in 0..count {
        let mut msg = test_message("coalesce-queue");
        msg.enqueued_at = i as u64;
        expected_ids.push(msg.id);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
        receivers.push(reply_rx);
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // All callers should receive their msg_id
    let result_ids: Vec<uuid::Uuid> = receivers
        .into_iter()
        .map(|rx| rx.blocking_recv().unwrap().unwrap())
        .collect();
    assert_eq!(result_ids, expected_ids);

    // All messages should be persisted
    let prefix = crate::storage::keys::message_prefix("coalesce-queue");
    let stored = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(
        stored.len(),
        count,
        "all coalesced messages should be persisted"
    );
}

/// Error isolation: an enqueue to a nonexistent queue fails independently
/// without affecting other enqueues in the same batch.
#[test]
fn error_isolation_in_batch() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "good-queue");
    scheduler.handle_all_pending(&tx);

    // Queue up: good enqueue, bad enqueue (nonexistent queue), good enqueue
    let good_msg1 = test_message("good-queue");
    let good_id1 = good_msg1.id;
    let (reply_tx1, reply_rx1) = tokio::sync::oneshot::channel();

    let bad_msg = test_message("nonexistent-queue");
    let (reply_tx_bad, reply_rx_bad) = tokio::sync::oneshot::channel();

    let mut good_msg2 = test_message("good-queue");
    good_msg2.enqueued_at = 2_000_000_000;
    let good_id2 = good_msg2.id;
    let (reply_tx2, reply_rx2) = tokio::sync::oneshot::channel();

    tx.send(SchedulerCommand::Enqueue {
        message: good_msg1,
        reply: reply_tx1,
    })
    .unwrap();
    tx.send(SchedulerCommand::Enqueue {
        message: bad_msg,
        reply: reply_tx_bad,
    })
    .unwrap();
    tx.send(SchedulerCommand::Enqueue {
        message: good_msg2,
        reply: reply_tx2,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    // Good messages succeed
    let result1 = reply_rx1.blocking_recv().unwrap();
    assert!(result1.is_ok(), "first good message should succeed");
    assert_eq!(result1.unwrap(), good_id1);

    let result2 = reply_rx2.blocking_recv().unwrap();
    assert!(result2.is_ok(), "second good message should succeed");
    assert_eq!(result2.unwrap(), good_id2);

    // Bad message gets QueueNotFound error
    let result_bad = reply_rx_bad.blocking_recv().unwrap();
    assert!(result_bad.is_err(), "bad message should fail");
    assert!(
        matches!(
            result_bad.unwrap_err(),
            crate::error::EnqueueError::QueueNotFound(_)
        ),
        "should be QueueNotFound error"
    );

    // Only 2 messages persisted (the good ones)
    let prefix = crate::storage::keys::message_prefix("good-queue");
    let stored = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(stored.len(), 2, "only valid messages should be persisted");
}

/// Non-enqueue commands interleaved with enqueues are processed correctly.
/// The coalescing flushes pending enqueues before processing non-enqueue commands.
#[test]
fn non_enqueue_commands_interleaved() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "interleaved-queue");
    scheduler.handle_all_pending(&tx);

    // Enqueue 1
    let msg1 = test_message("interleaved-queue");
    let id1 = msg1.id;
    let (reply_tx1, reply_rx1) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg1,
        reply: reply_tx1,
    })
    .unwrap();

    // CreateQueue command in the middle (non-enqueue)
    let (create_reply_tx, create_reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: "new-queue".to_string(),
        config: crate::queue::QueueConfig::new("new-queue".to_string()),
        reply: create_reply_tx,
    })
    .unwrap();

    // Enqueue 2 (after the interleaved command)
    let mut msg2 = test_message("interleaved-queue");
    msg2.enqueued_at = 2_000_000_000;
    let id2 = msg2.id;
    let (reply_tx2, reply_rx2) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg2,
        reply: reply_tx2,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // All operations succeed
    assert_eq!(reply_rx1.blocking_recv().unwrap().unwrap(), id1);
    assert_eq!(reply_rx2.blocking_recv().unwrap().unwrap(), id2);
    assert!(create_reply_rx.blocking_recv().unwrap().is_ok());

    // Both messages persisted
    let prefix = crate::storage::keys::message_prefix("interleaved-queue");
    let stored = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(stored.len(), 2);

    // New queue was created
    assert!(scheduler
        .storage()
        .get_queue("new-queue")
        .unwrap()
        .is_some());
}

/// Coalesced enqueues across multiple queues work correctly.
#[test]
fn coalescing_across_queues() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "queue-a");
    send_create_queue(&tx, "queue-b");
    scheduler.handle_all_pending(&tx);

    let msg_a = test_message("queue-a");
    let id_a = msg_a.id;
    let (reply_tx_a, reply_rx_a) = tokio::sync::oneshot::channel();

    let msg_b = test_message("queue-b");
    let id_b = msg_b.id;
    let (reply_tx_b, reply_rx_b) = tokio::sync::oneshot::channel();

    tx.send(SchedulerCommand::Enqueue {
        message: msg_a,
        reply: reply_tx_a,
    })
    .unwrap();
    tx.send(SchedulerCommand::Enqueue {
        message: msg_b,
        reply: reply_tx_b,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    assert_eq!(reply_rx_a.blocking_recv().unwrap().unwrap(), id_a);
    assert_eq!(reply_rx_b.blocking_recv().unwrap().unwrap(), id_b);

    // Messages in their respective queues
    let prefix_a = crate::storage::keys::message_prefix("queue-a");
    let stored_a = scheduler.storage().list_messages(&prefix_a).unwrap();
    assert_eq!(stored_a.len(), 1);
    assert_eq!(stored_a[0].1.id, id_a);

    let prefix_b = crate::storage::keys::message_prefix("queue-b");
    let stored_b = scheduler.storage().list_messages(&prefix_b).unwrap();
    assert_eq!(stored_b.len(), 1);
    assert_eq!(stored_b[0].1.id, id_b);
}

/// Write coalesce max batch is respected: the scheduler drains at most
/// write_coalesce_max_batch commands per iteration.
#[test]
fn max_batch_size_respected() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RocksDbEngine::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 1000,
        write_coalesce_max_batch: 5,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
    let lua_config = LuaConfig::default();
    let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config, 0);

    send_create_queue(&tx, "batch-limit-queue");
    scheduler.handle_all_pending(&tx);

    // Queue up 10 messages, with max_batch=5
    let mut receivers = Vec::with_capacity(10);
    for i in 0..10u64 {
        let mut msg = test_message("batch-limit-queue");
        msg.enqueued_at = i;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
        receivers.push(reply_rx);
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // All 10 should still be processed (across multiple iterations)
    let ids: Vec<uuid::Uuid> = receivers
        .into_iter()
        .map(|rx| rx.blocking_recv().unwrap().unwrap())
        .collect();
    assert_eq!(ids.len(), 10);

    let prefix = crate::storage::keys::message_prefix("batch-limit-queue");
    let stored = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(
        stored.len(),
        10,
        "all 10 messages should be persisted across batches"
    );
}
