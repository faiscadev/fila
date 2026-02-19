use super::*;

#[test]
fn ack_removes_message_lease_and_expiry() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "ack-queue");

    // Register consumer to get a lease
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "ack-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue a message (will be delivered and leased)
    let msg = test_message("ack-queue");
    let msg_id = msg.id;
    let (enq_tx, _enq_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Ack the message
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Ack {
        queue_id: "ack-queue".to_string(),
        msg_id,
        reply: ack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Consumer should have received the message
    assert!(consumer_rx.try_recv().is_ok());

    // Ack should succeed
    assert!(ack_rx.try_recv().unwrap().is_ok());

    // Message should be gone from messages CF
    let msg_key = crate::storage::keys::message_key("ack-queue", "default", 1_000_000_000, &msg_id);
    assert!(
        scheduler.storage().get_message(&msg_key).unwrap().is_none(),
        "message should be deleted after ack"
    );

    // Lease should be gone
    let lease_key = crate::storage::keys::lease_key("ack-queue", &msg_id);
    assert!(
        scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
        "lease should be deleted after ack"
    );
}

#[test]
fn ack_unknown_message_returns_not_found() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "ack-unknown-queue");

    let msg_id = Uuid::now_v7();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Ack {
        queue_id: "ack-unknown-queue".to_string(),
        msg_id,
        reply: ack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let err = ack_rx.try_recv().unwrap().unwrap_err();
    assert!(
        matches!(err, crate::error::AckError::MessageNotFound(_)),
        "expected MessageNotFound, got {err:?}"
    );
}

#[test]
fn ack_same_message_twice_returns_not_found() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "double-ack-queue");

    // Register consumer
    let (consumer_tx, mut _consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "double-ack-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue and let it be leased
    let msg = test_message("double-ack-queue");
    let msg_id = msg.id;
    let (enq_tx, _enq_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // First ack
    let (ack1_tx, mut ack1_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Ack {
        queue_id: "double-ack-queue".to_string(),
        msg_id,
        reply: ack1_tx,
    })
    .unwrap();

    // Second ack (same message)
    let (ack2_tx, mut ack2_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Ack {
        queue_id: "double-ack-queue".to_string(),
        msg_id,
        reply: ack2_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // First ack should succeed
    assert!(
        ack1_rx.try_recv().unwrap().is_ok(),
        "first ack should succeed"
    );

    // Second ack should return NOT_FOUND
    let err = ack2_rx.try_recv().unwrap().unwrap_err();
    assert!(
        matches!(err, crate::error::AckError::MessageNotFound(_)),
        "second ack should return MessageNotFound, got {err:?}"
    );
}

#[test]
fn nack_requeues_message_with_incremented_attempt_count() {
    // AC#8: enqueue → lease → nack → lease again → verify attempt count incremented
    let (tx, mut scheduler, _dir) = test_setup();
    send_create_queue(&tx, "nack-queue");

    // Register consumer
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "nack-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue a message (attempt_count starts at 0)
    let msg = test_message("nack-queue");
    let msg_id = msg.id;
    let (enq_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Nack the message
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "nack-queue".to_string(),
        msg_id,
        error: "processing failed".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // First delivery: attempt_count = 0
    let first = consumer_rx
        .try_recv()
        .expect("should receive first delivery");
    assert_eq!(first.msg_id, msg_id);
    assert_eq!(first.attempt_count, 0);

    // Nack should succeed
    assert!(nack_rx.try_recv().unwrap().is_ok(), "nack should succeed");

    // Second delivery: attempt_count = 1 (incremented by nack)
    let second = consumer_rx
        .try_recv()
        .expect("should receive second delivery after nack");
    assert_eq!(second.msg_id, msg_id);
    assert_eq!(
        second.attempt_count, 1,
        "attempt count should be incremented after nack"
    );

    // Message should still exist in storage (not deleted — only ack deletes)
    let msg_key =
        crate::storage::keys::message_key("nack-queue", "default", 1_000_000_000, &msg_id);
    assert!(
        scheduler.storage().get_message(&msg_key).unwrap().is_some(),
        "message should still exist after nack (not deleted)"
    );
}

#[test]
fn nack_removes_lease_and_lease_expiry() {
    let (tx, mut scheduler, _dir) = test_setup();
    send_create_queue(&tx, "nack-lease-queue");

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "nack-lease-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("nack-lease-queue");
    let msg_id = msg.id;
    let (enq_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Unregister consumer so nack doesn't immediately re-deliver
    tx.send(SchedulerCommand::UnregisterConsumer {
        consumer_id: "c1".to_string(),
    })
    .unwrap();

    // Nack the message to release the lease
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "nack-lease-queue".to_string(),
        msg_id,
        error: "retry please".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Consume first delivery
    let _ = consumer_rx.try_recv();
    assert!(nack_rx.try_recv().unwrap().is_ok());

    // Lease should be gone after nack (no re-delivery since consumer unregistered)
    let lease_key = crate::storage::keys::lease_key("nack-lease-queue", &msg_id);
    assert!(
        scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
        "lease should be deleted after nack"
    );

    // Lease expiry entry should also be gone
    let far_future = crate::storage::keys::lease_expiry_key(u64::MAX, "", &Uuid::nil());
    let expired = scheduler
        .storage()
        .list_expired_leases(&far_future)
        .unwrap();
    assert!(
        expired.is_empty(),
        "lease_expiry CF should be empty after nack, found {} entries",
        expired.len()
    );
}

#[test]
fn nack_unknown_message_returns_not_found() {
    let (tx, mut scheduler, _dir) = test_setup();
    send_create_queue(&tx, "nack-unknown-queue");

    let msg_id = Uuid::now_v7();
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "nack-unknown-queue".to_string(),
        msg_id,
        error: "test error".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let err = nack_rx.try_recv().unwrap().unwrap_err();
    assert!(
        matches!(err, crate::error::NackError::MessageNotFound(_)),
        "expected MessageNotFound, got {err:?}"
    );
}

#[test]
fn double_nack_returns_not_found() {
    // First nack succeeds, second nack returns NOT_FOUND because lease is gone
    let (tx, mut scheduler, _dir) = test_setup();
    send_create_queue(&tx, "double-nack-queue");

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "double-nack-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("double-nack-queue");
    let msg_id = msg.id;
    let (enq_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Unregister consumer so nack doesn't immediately re-deliver (which would create a new lease)
    tx.send(SchedulerCommand::UnregisterConsumer {
        consumer_id: "c1".to_string(),
    })
    .unwrap();

    // First nack — should succeed
    let (nack1_tx, mut nack1_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "double-nack-queue".to_string(),
        msg_id,
        error: "first nack".to_string(),
        reply: nack1_tx,
    })
    .unwrap();

    // Second nack — lease is already gone, should return NOT_FOUND
    let (nack2_tx, mut nack2_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "double-nack-queue".to_string(),
        msg_id,
        error: "second nack".to_string(),
        reply: nack2_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Consume the initial delivery
    let _ = consumer_rx.try_recv();

    assert!(
        nack1_rx.try_recv().unwrap().is_ok(),
        "first nack should succeed"
    );
    let err = nack2_rx.try_recv().unwrap().unwrap_err();
    assert!(
        matches!(err, crate::error::NackError::MessageNotFound(_)),
        "second nack should return MessageNotFound, got {err:?}"
    );
}

#[test]
fn nack_then_ack_completes_message_lifecycle() {
    // Full lifecycle: enqueue → lease → nack → lease again → ack
    let (tx, mut scheduler, _dir) = test_setup();
    send_create_queue(&tx, "nack-ack-queue");

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "nack-ack-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("nack-ack-queue");
    let msg_id = msg.id;
    let (enq_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Nack the message
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "nack-ack-queue".to_string(),
        msg_id,
        error: "first attempt failed".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    // Ack the redelivered message
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Ack {
        queue_id: "nack-ack-queue".to_string(),
        msg_id,
        reply: ack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // First delivery
    let first = consumer_rx.try_recv().expect("first delivery");
    assert_eq!(first.attempt_count, 0);

    // Nack succeeds
    assert!(nack_rx.try_recv().unwrap().is_ok());

    // Second delivery with incremented count
    let second = consumer_rx.try_recv().expect("second delivery after nack");
    assert_eq!(second.attempt_count, 1);

    // Ack succeeds
    assert!(ack_rx.try_recv().unwrap().is_ok());

    // Message should be deleted after ack
    let msg_key =
        crate::storage::keys::message_key("nack-ack-queue", "default", 1_000_000_000, &msg_id);
    assert!(
        scheduler.storage().get_message(&msg_key).unwrap().is_none(),
        "message should be deleted after ack"
    );
}

#[test]
fn lease_expiry_redelivers_message_with_incremented_attempt_count() {
    // Use a short visibility timeout (50ms) and idle_timeout (10ms)
    // so the scheduler wakes up frequently and reclaims quickly.
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 1000,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

    send_create_queue_with_timeout(&tx, "expiry-queue", 50);

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "expiry-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("expiry-queue");
    let msg_id = msg.id;
    let (enq_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Run scheduler on background thread — Scheduler contains Lua (not Send),
    // so it must be created inside the thread.
    let handle = std::thread::spawn(move || {
        let lua_config = LuaConfig::default();
        let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);
        scheduler.run();
    });

    // First delivery — should happen immediately
    let first = consumer_rx.blocking_recv().expect("first delivery");
    assert_eq!(first.msg_id, msg_id);
    assert_eq!(
        first.attempt_count, 0,
        "first delivery should have attempt_count=0"
    );

    // Wait for the visibility timeout to expire (50ms) plus some buffer
    std::thread::sleep(Duration::from_millis(100));

    // Second delivery — after lease expiry, the message should be redelivered
    let second = consumer_rx
        .blocking_recv()
        .expect("second delivery after expiry");
    assert_eq!(second.msg_id, msg_id);
    assert_eq!(
        second.attempt_count, 1,
        "redelivery after expiry should have attempt_count=1"
    );

    tx.send(SchedulerCommand::Shutdown).unwrap();
    handle.join().unwrap();
}

#[test]
fn lease_expiry_clears_lease_and_expiry_entries() {
    // Verify that lease and lease_expiry CFs are cleaned up after expiry reclaim
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 1000,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
    let storage_for_thread = Arc::clone(&storage);

    send_create_queue_with_timeout(&tx, "expiry-clean-queue", 50);

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "expiry-clean-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("expiry-clean-queue");
    let msg_id = msg.id;
    let (enq_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Run scheduler on background thread — Scheduler contains Lua (not Send),
    // so it must be created inside the thread.
    let handle = std::thread::spawn(move || {
        let lua_config = LuaConfig::default();
        let mut scheduler = Scheduler::new(storage_for_thread, rx, &config, &lua_config);
        scheduler.run();
    });

    // First delivery
    let _ = consumer_rx.blocking_recv().expect("first delivery");

    // Unregister consumer so expiry reclaim doesn't immediately redeliver
    tx.send(SchedulerCommand::UnregisterConsumer {
        consumer_id: "c1".to_string(),
    })
    .unwrap();

    // Wait for expiry
    std::thread::sleep(Duration::from_millis(100));

    tx.send(SchedulerCommand::Shutdown).unwrap();
    handle.join().unwrap();

    // After reclaim, lease should be gone
    let lease_key = crate::storage::keys::lease_key("expiry-clean-queue", &msg_id);
    assert!(
        storage.get_lease(&lease_key).unwrap().is_none(),
        "lease should be deleted after expiry reclaim"
    );

    // lease_expiry CF should be empty
    let far_future = crate::storage::keys::lease_expiry_key(u64::MAX, "", &Uuid::nil());
    let expired = storage.list_expired_leases(&far_future).unwrap();
    assert!(
        expired.is_empty(),
        "lease_expiry CF should be empty after reclaim, found {} entries",
        expired.len()
    );

    // Message should still exist with leased_at cleared (AC#4)
    let prefix = crate::storage::keys::message_prefix("expiry-clean-queue");
    let messages = storage.list_messages(&prefix).unwrap();
    assert_eq!(
        messages.len(),
        1,
        "message should still exist after expiry reclaim"
    );
    let (_, msg) = &messages[0];
    assert_eq!(msg.id, msg_id);
    assert!(
        msg.leased_at.is_none(),
        "leased_at should be cleared after expiry reclaim"
    );
    assert_eq!(msg.attempt_count, 1, "attempt_count should be incremented");
}

#[test]
fn lease_expiry_multiple_messages_different_timeouts() {
    // Two queues with different visibility timeouts: 50ms and 200ms.
    // After 100ms, only the first message should be redelivered.
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 1000,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

    send_create_queue_with_timeout(&tx, "fast-queue", 50);
    send_create_queue_with_timeout(&tx, "slow-queue", 200);

    let (consumer_tx_fast, mut consumer_rx_fast) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "fast-queue".to_string(),
        consumer_id: "c-fast".to_string(),
        tx: consumer_tx_fast,
    })
    .unwrap();

    let (consumer_tx_slow, mut consumer_rx_slow) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "slow-queue".to_string(),
        consumer_id: "c-slow".to_string(),
        tx: consumer_tx_slow,
    })
    .unwrap();

    let msg_fast = test_message("fast-queue");
    let msg_fast_id = msg_fast.id;
    let (enq_tx1, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg_fast,
        reply: enq_tx1,
    })
    .unwrap();

    let msg_slow = test_message("slow-queue");
    let (enq_tx2, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg_slow,
        reply: enq_tx2,
    })
    .unwrap();

    // Run scheduler on background thread — Scheduler contains Lua (not Send),
    // so it must be created inside the thread.
    let handle = std::thread::spawn(move || {
        let lua_config = LuaConfig::default();
        let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);
        scheduler.run();
    });

    // Both messages should be delivered immediately
    let first_fast = consumer_rx_fast
        .blocking_recv()
        .expect("fast first delivery");
    assert_eq!(first_fast.attempt_count, 0);
    let first_slow = consumer_rx_slow
        .blocking_recv()
        .expect("slow first delivery");
    assert_eq!(first_slow.attempt_count, 0);

    // Wait 100ms — fast (50ms) should expire, slow (200ms) should not
    std::thread::sleep(Duration::from_millis(100));

    // Fast message should be redelivered
    let second_fast = consumer_rx_fast
        .blocking_recv()
        .expect("fast second delivery");
    assert_eq!(second_fast.msg_id, msg_fast_id);
    assert_eq!(second_fast.attempt_count, 1);

    // Slow message should NOT have been redelivered yet
    assert!(
        consumer_rx_slow.try_recv().is_err(),
        "slow message should not have expired yet"
    );

    tx.send(SchedulerCommand::Shutdown).unwrap();
    handle.join().unwrap();
}

#[test]
fn ack_before_expiry_prevents_redelivery() {
    // Ack within the visibility timeout prevents the message from being redelivered
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 1000,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

    send_create_queue_with_timeout(&tx, "ack-before-expiry-queue", 100);

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "ack-before-expiry-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("ack-before-expiry-queue");
    let msg_id = msg.id;
    let (enq_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: enq_tx,
    })
    .unwrap();

    // Run scheduler on background thread — Scheduler contains Lua (not Send),
    // so it must be created inside the thread.
    let handle = std::thread::spawn(move || {
        let lua_config = LuaConfig::default();
        let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);
        scheduler.run();
    });

    // First delivery
    let first = consumer_rx.blocking_recv().expect("first delivery");
    assert_eq!(first.msg_id, msg_id);

    // Ack immediately (before the 100ms visibility timeout)
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Ack {
        queue_id: "ack-before-expiry-queue".to_string(),
        msg_id,
        reply: ack_tx,
    })
    .unwrap();

    // Wait to ensure ack is processed
    std::thread::sleep(Duration::from_millis(10));
    assert!(
        ack_rx.blocking_recv().unwrap().is_ok(),
        "ack should succeed"
    );

    // Wait past the visibility timeout
    std::thread::sleep(Duration::from_millis(150));

    // No redelivery should happen — the message was acked
    assert!(
        consumer_rx.try_recv().is_err(),
        "acked message should not be redelivered after visibility timeout"
    );

    tx.send(SchedulerCommand::Shutdown).unwrap();
    handle.join().unwrap();
}
