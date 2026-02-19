use super::*;

#[test]
fn recovery_preserves_messages_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

    // Phase 1: enqueue messages, then shut down the scheduler
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

    // Phase 2: create a brand-new scheduler on the same storage
    let (tx2, mut scheduler2) = test_setup_with_storage(Arc::clone(&storage));

    // Register consumer — should receive all 5 messages (they had no leases)
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
fn recovery_reclaims_expired_leases() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

    // Phase 1: set up a queue, enqueue a message, and manually create an
    // expired lease (simulating a crash while a message was in-flight)
    let config = crate::queue::QueueConfig::new("reclaim-queue".to_string());
    storage.put_queue("reclaim-queue", &config).unwrap();

    let msg = test_message("reclaim-queue");
    let msg_id = msg.id;
    let msg_key = crate::storage::keys::message_key(
        "reclaim-queue",
        &msg.fairness_key,
        msg.enqueued_at,
        &msg_id,
    );
    storage.put_message(&msg_key, &msg).unwrap();

    // Create a lease with an expiry in the past (1 nanosecond)
    let lease_key = crate::storage::keys::lease_key("reclaim-queue", &msg_id);
    let lease_val = crate::storage::keys::lease_value("old-consumer", 1);
    let expiry_key = crate::storage::keys::lease_expiry_key(1, "reclaim-queue", &msg_id);
    storage
        .write_batch(vec![
            WriteBatchOp::PutLease {
                key: lease_key.clone(),
                value: lease_val,
            },
            WriteBatchOp::PutLeaseExpiry {
                key: expiry_key.clone(),
            },
        ])
        .unwrap();

    // Verify the lease exists before recovery
    assert!(storage.get_lease(&lease_key).unwrap().is_some());

    // Phase 2: start a new scheduler — recovery should reclaim the expired lease
    let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));

    // Register consumer — the message should be delivered since the expired
    // lease was reclaimed during recovery
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "reclaim-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // The message should have been delivered to the consumer — this proves
    // the expired lease was reclaimed during recovery, since try_deliver_pending
    // skips messages that have an active lease.
    let ready = consumer_rx
        .try_recv()
        .expect("consumer should receive the reclaimed message");
    assert_eq!(ready.msg_id, msg_id);
    assert_eq!(
        ready.attempt_count, 1,
        "attempt_count should be incremented after lease expiry reclaim"
    );

    // The old lease_expiry entry should be gone (recovery deleted it)
    let old_up_to = crate::storage::keys::lease_expiry_key(2, "reclaim-queue", &uuid::Uuid::max());
    let old_expired = storage.list_expired_leases(&old_up_to).unwrap();
    assert!(
        old_expired.is_empty(),
        "expired lease_expiry entry at ts=1 should be removed by recovery"
    );
}

/// Regression: reclaim_expired_leases + recovery scan must not produce
/// duplicate pending entries (Cubic P1 finding on PR #21).
#[test]
fn recovery_does_not_duplicate_reclaimed_messages() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

    let config = crate::queue::QueueConfig::new("dup-queue".to_string());
    storage.put_queue("dup-queue", &config).unwrap();

    // Create 3 messages: 2 with expired leases, 1 unleased (ready)
    let mut msg_ids = Vec::new();
    for i in 0u64..3 {
        let mut msg = test_message("dup-queue");
        msg.enqueued_at = i;
        msg_ids.push(msg.id);
        let msg_key = crate::storage::keys::message_key(
            "dup-queue",
            &msg.fairness_key,
            msg.enqueued_at,
            &msg.id,
        );
        storage.put_message(&msg_key, &msg).unwrap();

        // Add expired leases for first 2 messages
        if i < 2 {
            let lease_key = crate::storage::keys::lease_key("dup-queue", &msg.id);
            let lease_val = crate::storage::keys::lease_value("old-consumer", 1);
            let expiry_key = crate::storage::keys::lease_expiry_key(1 + i, "dup-queue", &msg.id);
            storage
                .write_batch(vec![
                    WriteBatchOp::PutLease {
                        key: lease_key,
                        value: lease_val,
                    },
                    WriteBatchOp::PutLeaseExpiry { key: expiry_key },
                ])
                .unwrap();
        }
    }

    // Run recovery: should deliver exactly 3 messages, not 5
    let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "dup-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let mut received = Vec::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        received.push(ready.msg_id);
    }
    assert_eq!(
        received.len(),
        3,
        "should deliver exactly 3 messages (not duplicated by reclaim + scan)"
    );
}

#[test]
fn recovery_preserves_queue_definitions() {
    let dir = tempfile::tempdir().unwrap();

    // Phase 1: create queues, shut down, drop all handles
    {
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
        for name in &["q1", "q2", "q3"] {
            send_create_queue(&tx, name);
        }
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();
    }

    // Phase 2: reopen storage from disk — queues should survive the restart
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let queues = storage.list_queues().unwrap();
    assert_eq!(
        queues.len(),
        6,
        "all 3 queue definitions + 3 DLQs should survive restart"
    );

    let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
    assert!(names.contains(&"q1"));
    assert!(names.contains(&"q2"));
    assert!(names.contains(&"q3"));
    assert!(names.contains(&"q1.dlq"));
    assert!(names.contains(&"q2.dlq"));
    assert!(names.contains(&"q3.dlq"));
}

#[test]
fn shutdown_flushes_wal() {
    let dir = tempfile::tempdir().unwrap();
    let msg_id;

    // Phase 1: enqueue a message and shut down gracefully (which flushes WAL)
    {
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
        send_create_queue(&tx, "flush-queue");

        let msg = test_message("flush-queue");
        msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();
        // storage + scheduler dropped here, releasing RocksDB lock
    }

    // Phase 2: reopen the database and verify data survived
    let storage2: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let prefix = crate::storage::keys::message_prefix("flush-queue");
    let messages = storage2.list_messages(&prefix).unwrap();
    assert_eq!(
        messages.len(),
        1,
        "message should survive WAL flush + reopen"
    );
    assert_eq!(messages[0].1.id, msg_id);
}

#[test]
fn recovery_skips_corrupt_lease_expiry_key() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

    // Set up a queue with a message
    let config = crate::queue::QueueConfig::new("corrupt-queue".to_string());
    storage.put_queue("corrupt-queue", &config).unwrap();

    let msg = test_message("corrupt-queue");
    let msg_id = msg.id;
    let msg_key = crate::storage::keys::message_key(
        "corrupt-queue",
        &msg.fairness_key,
        msg.enqueued_at,
        &msg_id,
    );
    storage.put_message(&msg_key, &msg).unwrap();

    // Create a valid lease pointing at this message
    let lease_key = crate::storage::keys::lease_key("corrupt-queue", &msg_id);
    let lease_val = crate::storage::keys::lease_value("c1", 1);
    storage
        .write_batch(vec![WriteBatchOp::PutLease {
            key: lease_key.clone(),
            value: lease_val,
        }])
        .unwrap();

    // Write a corrupt lease_expiry key (expired, but unparseable)
    // Use a valid timestamp prefix so the scanner finds it, but garbage after
    let mut corrupt_expiry_key = Vec::new();
    corrupt_expiry_key.extend_from_slice(&1u64.to_be_bytes()); // expired timestamp
    corrupt_expiry_key.push(b':');
    corrupt_expiry_key.extend_from_slice(&[0xFF; 4]); // garbage
    storage
        .write_batch(vec![WriteBatchOp::PutLeaseExpiry {
            key: corrupt_expiry_key.clone(),
        }])
        .unwrap();

    // Start scheduler — recovery should skip the corrupt key without panicking
    let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // The corrupt lease_expiry entry should still be there (skipped, not deleted)
    let up_to = crate::storage::keys::lease_expiry_key(2, "z", &uuid::Uuid::max());
    let remaining = storage.list_expired_leases(&up_to).unwrap();
    assert!(
        remaining.contains(&corrupt_expiry_key),
        "corrupt expiry key should be skipped, not deleted"
    );

    // The valid lease should also still be there (recovery couldn't match it
    // to the corrupt expiry key, so it wasn't reclaimed)
    assert!(
        storage.get_lease(&lease_key).unwrap().is_some(),
        "lease should survive when expiry key is corrupt"
    );
}

#[test]
fn recovery_preserves_non_expired_leases() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

    // Set up a queue with a message
    let config = crate::queue::QueueConfig::new("active-lease-queue".to_string());
    storage.put_queue("active-lease-queue", &config).unwrap();

    let msg = test_message("active-lease-queue");
    let msg_id = msg.id;
    let msg_key = crate::storage::keys::message_key(
        "active-lease-queue",
        &msg.fairness_key,
        msg.enqueued_at,
        &msg_id,
    );
    storage.put_message(&msg_key, &msg).unwrap();

    // Create a lease with an expiry far in the future
    let future_expiry = u64::MAX;
    let lease_key = crate::storage::keys::lease_key("active-lease-queue", &msg_id);
    let lease_val = crate::storage::keys::lease_value("active-consumer", future_expiry);
    let expiry_key =
        crate::storage::keys::lease_expiry_key(future_expiry, "active-lease-queue", &msg_id);
    storage
        .write_batch(vec![
            WriteBatchOp::PutLease {
                key: lease_key.clone(),
                value: lease_val,
            },
            WriteBatchOp::PutLeaseExpiry {
                key: expiry_key.clone(),
            },
        ])
        .unwrap();

    // Start scheduler — recovery should NOT reclaim this lease
    let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));

    // Register a consumer — the message should NOT be delivered because
    // it still has an active (non-expired) lease
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "active-lease-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Consumer should NOT have received the message
    assert!(
        consumer_rx.try_recv().is_err(),
        "message with active lease should not be delivered"
    );

    // Lease and expiry should still exist
    assert!(
        storage.get_lease(&lease_key).unwrap().is_some(),
        "non-expired lease should survive recovery"
    );
}
