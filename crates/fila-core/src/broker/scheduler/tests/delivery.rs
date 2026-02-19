use super::*;

#[test]
fn consumer_receives_enqueued_messages() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "lease-queue");

    // Register a consumer
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "lease-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue a message — should be delivered to the consumer
    let msg = test_message("lease-queue");
    let msg_id = msg.id;
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Consumer should have received the message
    let ready = consumer_rx.try_recv().unwrap();
    assert_eq!(ready.msg_id, msg_id);
    assert_eq!(ready.queue_id, "lease-queue");
    assert_eq!(ready.payload, vec![1, 2, 3]);
}

#[test]
fn consumer_receives_pending_messages_on_register() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "pending-queue");

    // Enqueue messages first (no consumer yet)
    let mut msg_ids = Vec::new();
    for i in 0u64..5 {
        let mut msg = test_message("pending-queue");
        msg.enqueued_at = i;
        msg_ids.push(msg.id);
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // Now register a consumer — should receive all pending messages
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "pending-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // All 5 messages should be delivered
    let mut received_ids = Vec::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        received_ids.push(ready.msg_id);
    }
    assert_eq!(
        received_ids.len(),
        5,
        "all pending messages should be delivered"
    );
    assert_eq!(received_ids, msg_ids, "messages delivered in FIFO order");
}

#[test]
fn lease_creates_entries_in_storage() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "lease-cf-queue");

    // Register consumer first
    let (consumer_tx, _consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "lease-cf-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue a message
    let msg = test_message("lease-cf-queue");
    let msg_id = msg.id;
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Verify a lease was created
    let lease_key = crate::storage::keys::lease_key("lease-cf-queue", &msg_id);
    let lease = scheduler.storage().get_lease(&lease_key).unwrap();
    assert!(lease.is_some(), "lease entry should exist after delivery");
}

#[test]
fn multiple_consumers_get_different_messages() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "multi-queue");

    // Register two consumers
    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "multi-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: c1_tx,
    })
    .unwrap();

    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "multi-queue".to_string(),
        consumer_id: "c2".to_string(),
        tx: c2_tx,
    })
    .unwrap();

    // Enqueue 4 messages
    let mut msg_ids = Vec::new();
    for i in 0u64..4 {
        let mut msg = test_message("multi-queue");
        msg.enqueued_at = i;
        msg_ids.push(msg.id);
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Collect messages from both consumers
    let mut c1_msgs = Vec::new();
    while let Ok(ready) = c1_rx.try_recv() {
        c1_msgs.push(ready.msg_id);
    }
    let mut c2_msgs = Vec::new();
    while let Ok(ready) = c2_rx.try_recv() {
        c2_msgs.push(ready.msg_id);
    }

    // Both consumers should have received messages
    let total = c1_msgs.len() + c2_msgs.len();
    assert_eq!(
        total, 4,
        "all 4 messages should be delivered across consumers"
    );

    // No message should be delivered to both consumers
    let mut all_ids: Vec<_> = c1_msgs.iter().chain(c2_msgs.iter()).copied().collect();
    all_ids.sort();
    all_ids.dedup();
    assert_eq!(
        all_ids.len(),
        4,
        "each message delivered to exactly one consumer"
    );
}

#[test]
fn unregister_consumer_stops_delivery() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "unreg-queue");

    // Register then immediately unregister
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "unreg-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::UnregisterConsumer {
        consumer_id: "c1".to_string(),
    })
    .unwrap();

    // Enqueue a message after unregistration
    let msg = test_message("unreg-queue");
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Consumer should NOT have received any messages
    assert!(
        consumer_rx.try_recv().is_err(),
        "unregistered consumer should not receive messages"
    );
}

#[test]
fn enqueue_10_messages_lease_receives_all() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "ten-queue");

    // Register consumer
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "ten-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 10 messages
    let mut msg_ids = Vec::new();
    for i in 0u64..10 {
        let mut msg = test_message("ten-queue");
        msg.enqueued_at = i;
        msg_ids.push(msg.id);
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // All 10 messages should be delivered
    let mut received = Vec::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        received.push(ready.msg_id);
    }
    assert_eq!(received.len(), 10, "all 10 messages should be received");
    assert_eq!(received, msg_ids, "messages received in FIFO order");
}

#[test]
fn delivery_skips_closed_consumer_and_delivers_to_next() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "closed-queue");

    // Register c1 with a channel we immediately close (drop receiver)
    let (c1_tx, c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "closed-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: c1_tx,
    })
    .unwrap();
    drop(c1_rx);

    // Register c2 with a healthy channel
    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "closed-queue".to_string(),
        consumer_id: "c2".to_string(),
        tx: c2_tx,
    })
    .unwrap();

    // Enqueue a message
    let msg = test_message("closed-queue");
    let msg_id = msg.id;
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // c2 should have received the message (c1 was closed)
    let ready = c2_rx.try_recv().unwrap();
    assert_eq!(ready.msg_id, msg_id);

    // A lease should exist for the delivered message
    let lease_key = crate::storage::keys::lease_key("closed-queue", &msg_id);
    assert!(
        scheduler.storage().get_lease(&lease_key).unwrap().is_some(),
        "lease should exist for the delivered message"
    );
}

#[test]
fn delivery_rolls_back_lease_when_all_consumers_closed() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "all-closed-queue");

    // Register two consumers, close both channels
    let (c1_tx, c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "all-closed-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: c1_tx,
    })
    .unwrap();
    drop(c1_rx);

    let (c2_tx, c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "all-closed-queue".to_string(),
        consumer_id: "c2".to_string(),
        tx: c2_tx,
    })
    .unwrap();
    drop(c2_rx);

    // Enqueue a message
    let msg = test_message("all-closed-queue");
    let msg_id = msg.id;
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // No lease should remain — both were rolled back
    let lease_key = crate::storage::keys::lease_key("all-closed-queue", &msg_id);
    assert!(
        scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
        "lease should be rolled back when all consumers are closed"
    );

    // Message should still exist in storage (not lost)
    let prefix = crate::storage::keys::message_prefix("all-closed-queue");
    let messages = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(
        messages.len(),
        1,
        "message should still be in storage for retry"
    );
}

#[test]
fn delivery_skips_full_consumer_and_delivers_to_next() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "full-queue");

    // Register c1 with capacity 1, then pre-fill it
    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(1);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "full-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: c1_tx,
    })
    .unwrap();

    // Register c2 with plenty of capacity
    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "full-queue".to_string(),
        consumer_id: "c2".to_string(),
        tx: c2_tx,
    })
    .unwrap();

    // Enqueue 2 messages — they fill c1 (capacity 1) and overflow to c2
    let mut msg_ids = Vec::new();
    for i in 0u64..2 {
        let mut msg = test_message("full-queue");
        msg.enqueued_at = i;
        msg_ids.push(msg.id);
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Collect from both consumers
    let mut c1_msgs = Vec::new();
    while let Ok(ready) = c1_rx.try_recv() {
        c1_msgs.push(ready.msg_id);
    }
    let mut c2_msgs = Vec::new();
    while let Ok(ready) = c2_rx.try_recv() {
        c2_msgs.push(ready.msg_id);
    }

    // Both messages should be delivered across the two consumers
    let total = c1_msgs.len() + c2_msgs.len();
    assert_eq!(total, 2, "both messages should be delivered");

    // c2 should have received at least one message (overflow from c1)
    assert!(
        !c2_msgs.is_empty(),
        "c2 should receive messages when c1 is full"
    );

    // Each delivered message should have a lease
    for msg_id in msg_ids {
        let lease_key = crate::storage::keys::lease_key("full-queue", &msg_id);
        assert!(
            scheduler.storage().get_lease(&lease_key).unwrap().is_some(),
            "lease should exist for msg {msg_id}"
        );
    }
}

#[test]
fn delivery_rolls_back_lease_when_all_consumers_full() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "all-full-queue");

    // Register two consumers, both with capacity 1
    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(1);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "all-full-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: c1_tx,
    })
    .unwrap();

    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(1);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "all-full-queue".to_string(),
        consumer_id: "c2".to_string(),
        tx: c2_tx,
    })
    .unwrap();

    // Enqueue 3 messages — first 2 fill both consumers, 3rd has nowhere to go
    let mut msg_ids = Vec::new();
    for i in 0u64..3 {
        let mut msg = test_message("all-full-queue");
        msg.enqueued_at = i;
        msg_ids.push(msg.id);
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // First 2 messages should have been delivered
    let mut c1_msgs = Vec::new();
    while let Ok(ready) = c1_rx.try_recv() {
        c1_msgs.push(ready.msg_id);
    }
    let mut c2_msgs = Vec::new();
    while let Ok(ready) = c2_rx.try_recv() {
        c2_msgs.push(ready.msg_id);
    }
    let delivered: Vec<Uuid> = c1_msgs.iter().chain(c2_msgs.iter()).copied().collect();
    assert_eq!(delivered.len(), 2, "only 2 messages should be delivered");

    // The 3rd message (not delivered) should have no lease
    let undelivered_id = msg_ids
        .iter()
        .find(|id| !delivered.contains(id))
        .expect("one message should be undelivered");
    let lease_key = crate::storage::keys::lease_key("all-full-queue", undelivered_id);
    assert!(
        scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
        "undelivered message should have no lease (rolled back)"
    );

    // The undelivered message should still be in storage
    let prefix = crate::storage::keys::message_prefix("all-full-queue");
    let messages = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(
        messages.len(),
        3,
        "all 3 messages should still be in storage"
    );
}
