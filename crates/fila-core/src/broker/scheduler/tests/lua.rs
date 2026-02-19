use super::*;

#[test]
fn on_enqueue_assigns_fairness_key_from_header() {
    let (tx, mut scheduler, _dir) = test_setup();

    let script = r#"
        function on_enqueue(msg)
            return { fairness_key = msg.headers["tenant_id"] or "unknown" }
        end
    "#;
    send_create_queue_with_script(&tx, "lua-fk-queue", script);

    let mut headers = HashMap::new();
    headers.insert("tenant_id".to_string(), "acme".to_string());
    let msg = test_message_with_headers("lua-fk-queue", headers);
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // The message should be stored with fairness_key="acme" from the Lua script
    let key = crate::storage::keys::message_key("lua-fk-queue", "acme", 1_000_000_000, &msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(
        stored.is_some(),
        "message should be stored with fairness_key='acme' from Lua script"
    );
    let stored_msg = stored.unwrap();
    assert_eq!(stored_msg.fairness_key, "acme");
}

#[test]
fn on_enqueue_assigns_weight_and_throttle_keys() {
    let (tx, mut scheduler, _dir) = test_setup();

    let script = r#"
        function on_enqueue(msg)
            return {
                fairness_key = "tenant_x",
                weight = 5,
                throttle_keys = { "rate:global", "rate:tenant_x" },
            }
        end
    "#;
    send_create_queue_with_script(&tx, "lua-wt-queue", script);

    let msg = test_message("lua-wt-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let key = crate::storage::keys::message_key("lua-wt-queue", "tenant_x", 1_000_000_000, &msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(
        stored.is_some(),
        "message should exist with fairness_key='tenant_x'"
    );
    let stored_msg = stored.unwrap();
    assert_eq!(stored_msg.weight, 5);
    assert_eq!(
        stored_msg.throttle_keys,
        vec!["rate:global", "rate:tenant_x"]
    );
}

#[test]
fn queue_without_script_uses_defaults() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "no-script-queue");

    let msg = test_message("no-script-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let key =
        crate::storage::keys::message_key("no-script-queue", "default", 1_000_000_000, &msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(
        stored.is_some(),
        "message should exist with default fairness_key"
    );
    let stored_msg = stored.unwrap();
    assert_eq!(stored_msg.fairness_key, "default");
    assert_eq!(stored_msg.weight, 1);
    assert!(stored_msg.throttle_keys.is_empty());
}

#[test]
fn on_enqueue_reads_config_via_fila_get() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Write a config value to the state CF that the Lua script will read
    scheduler
        .storage()
        .put_state("default_tenant", b"megacorp")
        .unwrap();

    let script = r#"
        function on_enqueue(msg)
            local tenant = msg.headers["tenant_id"]
            if tenant == nil or tenant == "" then
                tenant = fila.get("default_tenant") or "fallback"
            end
            return { fairness_key = tenant }
        end
    "#;
    send_create_queue_with_script(&tx, "lua-fila-get-queue", script);

    // Message without tenant_id header — script should fall back to fila.get()
    let msg = test_message("lua-fila-get-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let key =
        crate::storage::keys::message_key("lua-fila-get-queue", "megacorp", 1_000_000_000, &msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(
        stored.is_some(),
        "message should have fairness_key='megacorp' from fila.get()"
    );
    assert_eq!(stored.unwrap().fairness_key, "megacorp");
}

#[test]
fn create_queue_with_invalid_script_returns_error() {
    let (tx, mut scheduler, _dir) = test_setup();

    let mut reply_rx = send_create_queue_with_script(&tx, "bad-script-queue", "not valid lua %%%");

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let result = reply_rx.try_recv().unwrap();
    assert!(result.is_err(), "invalid script should return an error");
    assert!(
        matches!(
            result.unwrap_err(),
            crate::error::CreateQueueError::LuaCompilation(_)
        ),
        "error should be LuaCompilation"
    );
}

// --- Lua safety integration tests (Story 3.2) ---

#[test]
fn on_enqueue_infinite_loop_falls_back_to_defaults() {
    // A script that loops forever should be killed by the instruction hook
    // and the message should get safe defaults.
    let (tx, mut scheduler, _dir) = test_setup();

    let script = r#"
        function on_enqueue(msg)
            while true do end
            return { fairness_key = "unreachable" }
        end
    "#;
    send_create_queue_with_script(&tx, "infinite-loop-queue", script);

    let msg = test_message("infinite-loop-queue");
    let msg_id = msg.id;
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Enqueue should succeed (Lua failure falls back to defaults)
    assert!(reply_rx.try_recv().unwrap().is_ok());

    // Message should have default fairness_key
    let key =
        crate::storage::keys::message_key("infinite-loop-queue", "default", 1_000_000_000, &msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(
        stored.is_some(),
        "message should be stored with default fairness_key after script timeout"
    );
}

#[test]
fn on_enqueue_memory_bomb_falls_back_to_defaults() {
    // A script that tries to allocate too much memory should be killed
    // and the message should get safe defaults.
    let (tx, mut scheduler, _dir) = test_setup();

    let script = r#"
        function on_enqueue(msg)
            local t = {}
            for i = 1, 10000000 do
                t[i] = string.rep("x", 1000)
            end
            return { fairness_key = "unreachable" }
        end
    "#;
    send_create_queue_with_script(&tx, "memory-bomb-queue", script);

    let msg = test_message("memory-bomb-queue");
    let msg_id = msg.id;
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Enqueue should succeed (Lua failure falls back to defaults)
    assert!(reply_rx.try_recv().unwrap().is_ok());

    // Message should have default fairness_key
    let key =
        crate::storage::keys::message_key("memory-bomb-queue", "default", 1_000_000_000, &msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(
        stored.is_some(),
        "message should be stored with default fairness_key after memory limit"
    );
}

#[test]
fn circuit_breaker_trips_and_bypasses_lua() {
    // After enough consecutive failures, the circuit breaker should trip
    // and bypass Lua entirely (returning defaults without running the script).
    // Default threshold is 3 failures.
    let (tx, mut scheduler, _dir) = test_setup();

    // Script that always fails
    let script = r#"
        function on_enqueue(msg)
            error("always fails")
        end
    "#;
    send_create_queue_with_script(&tx, "cb-queue", script);

    // Enqueue 5 messages — first 3 trip the breaker, last 2 bypass Lua
    let mut msg_ids = Vec::new();
    for i in 0u64..5 {
        let mut msg = test_message("cb-queue");
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

    // All 5 messages should be stored with default fairness_key
    for (i, msg_id) in msg_ids.iter().enumerate() {
        let key = crate::storage::keys::message_key("cb-queue", "default", i as u64, msg_id);
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "message {i} should be stored with default fairness_key"
        );
    }
}

#[test]
fn failed_script_does_not_break_subsequent_good_scripts() {
    // A failing script for one queue should not affect another queue's scripts.
    // Also verifies the VM is usable after safety hooks fire.
    let (tx, mut scheduler, _dir) = test_setup();

    // Queue A: script always errors
    let bad_script = r#"
        function on_enqueue(msg)
            error("queue A failure")
        end
    "#;
    send_create_queue_with_script(&tx, "bad-queue", bad_script);

    // Queue B: normal script
    let good_script = r#"
        function on_enqueue(msg)
            return { fairness_key = "good" }
        end
    "#;
    send_create_queue_with_script(&tx, "good-queue", good_script);

    // Enqueue to bad queue first, then good queue
    let bad_msg = test_message("bad-queue");
    let (reply_tx1, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: bad_msg,
        reply: reply_tx1,
    })
    .unwrap();

    let good_msg = test_message("good-queue");
    let good_msg_id = good_msg.id;
    let (reply_tx2, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: good_msg,
        reply: reply_tx2,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Good queue's message should have the correct fairness_key from Lua
    let key = crate::storage::keys::message_key("good-queue", "good", 1_000_000_000, &good_msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(
        stored.is_some(),
        "good queue's Lua script should work even after bad queue's script failed"
    );
    assert_eq!(stored.unwrap().fairness_key, "good");
}

// --- on_failure integration tests ---

#[test]
fn on_failure_retry_requeues_message() {
    let (tx, mut scheduler, _dir) = test_setup();

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "retry" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "retry-queue", on_failure_script, None);

    // Register consumer to receive messages
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "retry-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue a message
    let msg = test_message("retry-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Nack the message — on_failure returns retry
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "retry-queue".to_string(),
        msg_id,
        error: "transient error".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Nack should succeed
    assert!(nack_rx.try_recv().unwrap().is_ok(), "nack should succeed");

    // Should receive the message twice: initial delivery + retry after nack
    let first = consumer_rx.try_recv().expect("first delivery");
    assert_eq!(first.msg_id, msg_id);
    assert_eq!(first.attempt_count, 0);

    let second = consumer_rx.try_recv().expect("second delivery after retry");
    assert_eq!(second.msg_id, msg_id);
    assert_eq!(second.attempt_count, 1);
}

#[test]
fn on_failure_dlq_moves_message_to_dead_letter_queue() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create the main queue with on_failure script — DLQ is auto-created as main-queue.dlq
    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "main-queue", on_failure_script, None);

    // Register consumers for both queues
    let (main_tx, mut main_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "main-queue".to_string(),
        consumer_id: "main-consumer".to_string(),
        tx: main_tx,
    })
    .unwrap();

    let (dlq_tx, mut dlq_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "main-queue.dlq".to_string(),
        consumer_id: "dlq-consumer".to_string(),
        tx: dlq_tx,
    })
    .unwrap();

    // Enqueue a message to the main queue
    let msg = test_message("main-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Nack the message — on_failure returns dlq
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "main-queue".to_string(),
        msg_id,
        error: "permanent error".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(nack_rx.try_recv().unwrap().is_ok(), "nack should succeed");

    // First delivery on main queue (before nack)
    let first = main_rx.try_recv().expect("initial delivery on main queue");
    assert_eq!(first.msg_id, msg_id);

    // No second delivery on main queue (message was DLQ'd, not retried)
    assert!(
        main_rx.try_recv().is_err(),
        "message should not be retried on main queue"
    );

    // Message should appear on DLQ
    let dlq_msg = dlq_rx
        .try_recv()
        .expect("message should be delivered to DLQ");
    assert_eq!(dlq_msg.msg_id, msg_id);
    assert_eq!(
        dlq_msg.attempt_count, 1,
        "attempt_count should be incremented"
    );

    // Verify message is gone from main queue storage
    let main_prefix = crate::storage::keys::message_prefix("main-queue");
    let main_msgs = scheduler.storage().list_messages(&main_prefix).unwrap();
    assert!(
        main_msgs.is_empty(),
        "message should be removed from main queue"
    );

    // Verify message exists in DLQ storage
    let dlq_prefix = crate::storage::keys::message_prefix("main-queue.dlq");
    let dlq_msgs = scheduler.storage().list_messages(&dlq_prefix).unwrap();
    assert_eq!(dlq_msgs.len(), 1, "message should exist in DLQ");
    assert_eq!(dlq_msgs[0].1.id, msg_id);
}

#[test]
fn on_failure_dlq_without_dlq_configured_falls_back_to_retry() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Use a .dlq queue name — DLQ queues don't get auto-created DLQs,
    // so on_failure returning "dlq" should fall back to retry.
    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "orphan.dlq", on_failure_script, None);

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "orphan.dlq".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("orphan.dlq");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Nack — on_failure says DLQ but no DLQ configured (it's a .dlq queue), should retry
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "orphan.dlq".to_string(),
        msg_id,
        error: "some error".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(nack_rx.try_recv().unwrap().is_ok());

    // Should receive message twice (initial + retry fallback)
    let first = consumer_rx.try_recv().expect("first delivery");
    assert_eq!(first.msg_id, msg_id);
    assert_eq!(first.attempt_count, 0);

    let second = consumer_rx.try_recv().expect("retry after DLQ fallback");
    assert_eq!(second.msg_id, msg_id);
    assert_eq!(second.attempt_count, 1);
}

#[test]
fn on_failure_receives_attempt_count_and_error() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Script that DLQs only when attempts >= 3 and error is "fatal"
    // DLQ is auto-created as attempts-queue.dlq
    let on_failure_script = r#"
        function on_failure(msg)
            if msg.attempts >= 3 and msg.error == "fatal" then
                return { action = "dlq" }
            end
            return { action = "retry" }
        end
    "#;
    send_create_queue_with_on_failure(&tx, "attempts-queue", on_failure_script, None);

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "attempts-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let (dlq_consumer_tx, mut dlq_consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "attempts-queue.dlq".to_string(),
        consumer_id: "dlq-c1".to_string(),
        tx: dlq_consumer_tx,
    })
    .unwrap();

    let msg = test_message("attempts-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Nack 1: attempt_count becomes 1, error is "transient" → retry
    let (nack1_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "attempts-queue".to_string(),
        msg_id,
        error: "transient".to_string(),
        reply: nack1_tx,
    })
    .unwrap();

    // Nack 2: attempt_count becomes 2, error is "fatal" → retry (attempts < 3)
    let (nack2_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "attempts-queue".to_string(),
        msg_id,
        error: "fatal".to_string(),
        reply: nack2_tx,
    })
    .unwrap();

    // Nack 3: attempt_count becomes 3, error is "fatal" → DLQ (attempts >= 3 AND error == "fatal")
    let (nack3_tx, mut nack3_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "attempts-queue".to_string(),
        msg_id,
        error: "fatal".to_string(),
        reply: nack3_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(nack3_rx.try_recv().unwrap().is_ok());

    // Main queue: initial delivery (0) + retry after nack 1 (1) + retry after nack 2 (2) = 3 deliveries
    let d0 = consumer_rx.try_recv().expect("delivery 0");
    assert_eq!(d0.attempt_count, 0);
    let d1 = consumer_rx.try_recv().expect("delivery 1");
    assert_eq!(d1.attempt_count, 1);
    let d2 = consumer_rx.try_recv().expect("delivery 2");
    assert_eq!(d2.attempt_count, 2);

    // No more deliveries on main queue (nack 3 sent to DLQ)
    assert!(
        consumer_rx.try_recv().is_err(),
        "no more deliveries after DLQ"
    );

    // DLQ should have the message with attempt_count = 3
    let dlq_msg = dlq_consumer_rx
        .try_recv()
        .expect("message should be in DLQ");
    assert_eq!(dlq_msg.msg_id, msg_id);
    assert_eq!(dlq_msg.attempt_count, 3);
}

#[test]
fn on_failure_no_script_uses_default_retry() {
    // Backward compatibility: no on_failure script → default retry behavior
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "no-script-queue");

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "no-script-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    let msg = test_message("no-script-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Nack {
        queue_id: "no-script-queue".to_string(),
        msg_id,
        error: "some error".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    assert!(nack_rx.try_recv().unwrap().is_ok());

    // Should get 2 deliveries: initial + retry (default behavior)
    let first = consumer_rx.try_recv().expect("first delivery");
    assert_eq!(first.attempt_count, 0);
    let second = consumer_rx.try_recv().expect("second delivery after nack");
    assert_eq!(second.attempt_count, 1);
}

#[test]
fn recovery_restores_on_failure_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

    // Phase 1: create queue with on_failure script, enqueue a message, shut down
    let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));

    let on_failure_script = r#"
        function on_failure(msg)
            return { action = "dlq" }
        end
    "#;
    // DLQ is auto-created as recovery-queue.dlq
    send_create_queue_with_on_failure(&tx, "recovery-queue", on_failure_script, None);

    let msg = test_message("recovery-queue");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Register consumer so message gets delivered and leased
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "recovery-queue".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();
    drop(tx);

    // Consume the initial delivery
    let delivered = consumer_rx.try_recv().expect("initial delivery");
    assert_eq!(delivered.msg_id, msg_id);

    // Phase 2: start new scheduler on same storage — on_failure script should be recovered
    let (tx2, mut scheduler2) = test_setup_with_storage(Arc::clone(&storage));

    // Register consumers for both queues
    let (main_consumer_tx, _main_consumer_rx) = tokio::sync::mpsc::channel(64);
    tx2.send(SchedulerCommand::RegisterConsumer {
        queue_id: "recovery-queue".to_string(),
        consumer_id: "c2".to_string(),
        tx: main_consumer_tx,
    })
    .unwrap();

    let (dlq_consumer_tx, mut dlq_consumer_rx) = tokio::sync::mpsc::channel(64);
    tx2.send(SchedulerCommand::RegisterConsumer {
        queue_id: "recovery-queue.dlq".to_string(),
        consumer_id: "dlq-c2".to_string(),
        tx: dlq_consumer_tx,
    })
    .unwrap();

    // Nack the message — should trigger on_failure script (recovered from storage)
    let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
    tx2.send(SchedulerCommand::Nack {
        queue_id: "recovery-queue".to_string(),
        msg_id,
        error: "error after recovery".to_string(),
        reply: nack_tx,
    })
    .unwrap();

    tx2.send(SchedulerCommand::Shutdown).unwrap();
    scheduler2.run();

    assert!(
        nack_rx.try_recv().unwrap().is_ok(),
        "nack should succeed after recovery"
    );

    // Message should be in DLQ (on_failure script was recovered and returned "dlq")
    let dlq_msg = dlq_consumer_rx
        .try_recv()
        .expect("on_failure script should be restored — message should be in DLQ");
    assert_eq!(dlq_msg.msg_id, msg_id);
}
