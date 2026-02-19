use super::*;

#[test]
fn drr_three_equal_weight_keys_get_equal_delivery() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "drr-equal");

    // Register a consumer with enough capacity
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "drr-equal".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 30 messages: 10 for each of 3 fairness keys
    let mut ts = 0u64;
    for key in &["tenant_a", "tenant_b", "tenant_c"] {
        for _ in 0..10 {
            let msg = test_message_with_key("drr-equal", key, ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Collect delivered messages and count per fairness key
    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
    }

    // All 30 messages should be delivered
    let total: usize = counts.values().sum();
    assert_eq!(total, 30, "all 30 messages should be delivered");

    // Each key should get exactly 10 (equal weight, 10 messages each)
    assert_eq!(
        counts.get("tenant_a"),
        Some(&10),
        "tenant_a should get 10 messages"
    );
    assert_eq!(
        counts.get("tenant_b"),
        Some(&10),
        "tenant_b should get 10 messages"
    );
    assert_eq!(
        counts.get("tenant_c"),
        Some(&10),
        "tenant_c should get 10 messages"
    );
}

#[test]
fn drr_single_key_backward_compatible() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "drr-single");

    // Register consumer
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "drr-single".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 10 messages with default fairness key (Epic 1 behavior)
    let mut msg_ids = Vec::new();
    for i in 0u64..10 {
        let msg = test_message_with_key("drr-single", "default", i);
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

    // All 10 messages should be delivered in FIFO order
    let mut received = Vec::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        received.push(ready.msg_id);
    }
    assert_eq!(received.len(), 10, "all 10 messages should be delivered");
    assert_eq!(received, msg_ids, "single-key DRR delivers in FIFO order");
}

#[test]
fn drr_key_exhaustion_continues_other_keys() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "drr-exhaust");

    // Register consumer
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "drr-exhaust".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue: tenant_a has 2 messages, tenant_b has 10 messages
    // tenant_a will exhaust after 2, tenant_b should continue to 10
    let mut ts = 0u64;
    for _ in 0..2 {
        let msg = test_message_with_key("drr-exhaust", "tenant_a", ts);
        ts += 1;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    for _ in 0..10 {
        let msg = test_message_with_key("drr-exhaust", "tenant_b", ts);
        ts += 1;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Collect delivered messages
    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
    }

    let total: usize = counts.values().sum();
    assert_eq!(total, 12, "all 12 messages should be delivered");
    assert_eq!(
        counts.get("tenant_a"),
        Some(&2),
        "tenant_a exhausted after 2"
    );
    assert_eq!(
        counts.get("tenant_b"),
        Some(&10),
        "tenant_b should get all 10 messages"
    );
}

#[test]
fn drr_weighted_keys_proportional_delivery() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "drr-weighted");

    // Register consumer
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "drr-weighted".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue: tenant_a (weight=3) gets 20 messages, tenant_b (weight=1) gets 20 messages
    // With quantum=1000: tenant_a gets 3000 deficit, tenant_b gets 1000 deficit per round
    // Both have 20 messages each, so both should deliver all 20 within one round
    let mut ts = 0u64;
    for _ in 0..20 {
        let msg = test_message_with_key_and_weight("drr-weighted", "tenant_a", 3, ts);
        ts += 1;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    for _ in 0..20 {
        let msg = test_message_with_key_and_weight("drr-weighted", "tenant_b", 1, ts);
        ts += 1;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Collect delivered messages
    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
    }

    let total: usize = counts.values().sum();
    assert_eq!(total, 40, "all 40 messages should be delivered");
    assert_eq!(
        counts.get("tenant_a"),
        Some(&20),
        "tenant_a (weight=3) should get all 20 messages"
    );
    assert_eq!(
        counts.get("tenant_b"),
        Some(&20),
        "tenant_b (weight=1) should get all 20 messages"
    );
}

/// Full-path fairness accuracy test with 10k+ messages (AC#6).
///
/// Messages are enqueued BEFORE registering the consumer, so the DRR
/// scheduler exercises fairness under contention (all keys have pending
/// messages simultaneously). The scheduler runs on a background thread;
/// the main thread collects deliveries and sends Shutdown after all
/// messages are received.
///
/// With the in-memory pending index (Story 4.2), this test runs in O(n)
/// time instead of the previous O(n²) storage scan approach.
#[test]
fn drr_fairness_accuracy_10k_messages_6_keys() {
    // 6 keys with weights 1,2,3,4,5,6 → total_weight = 21
    // quantum=100: per round delivers 21*100 = 2100 messages
    // Each key gets weight * 500 messages → total = 21*500 = 10,500 messages (≥ 10k AC)
    // With quantum=100, each key exhausts after 5 rounds: 5*2100 = 10,500 deliveries
    let weights: Vec<(&str, u32)> = vec![
        ("tenant_a", 1),
        ("tenant_b", 2),
        ("tenant_c", 3),
        ("tenant_d", 4),
        ("tenant_e", 5),
        ("tenant_f", 6),
    ];
    let total_weight: u32 = weights.iter().map(|(_, w)| *w).sum();
    let msgs_per_weight_unit = 500usize;
    let total_msgs: usize = weights
        .iter()
        .map(|(_, w)| *w as usize * msgs_per_weight_unit)
        .sum();

    // Custom setup: larger channel (all commands fit), smaller quantum
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: total_msgs + 100, // room for all commands
        idle_timeout_ms: 50,
        quantum: 100,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

    send_create_queue(&tx, "drr-accuracy");

    // Enqueue ALL messages BEFORE registering consumer — no delivery
    // happens during enqueue because has_consumers check returns false.
    let mut ts = 0u64;
    for (key, weight) in &weights {
        let count = (*weight as usize) * msgs_per_weight_unit;
        for _ in 0..count {
            let msg = test_message_with_key_and_weight("drr-accuracy", key, *weight, ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
    }

    // Register consumer AFTER enqueues — DRR sees all messages at once
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(total_msgs + 100);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "drr-accuracy".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Spawn scheduler on background thread — Scheduler contains Lua (not Send),
    // so it must be created inside the thread.
    let lua_config = LuaConfig::default();
    let handle = std::thread::spawn(move || {
        let mut s = Scheduler::new(storage, rx, &config, &lua_config);
        s.run();
    });

    // Collect delivered messages from consumer channel
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total_delivered = 0usize;
    while total_delivered < total_msgs {
        match consumer_rx.blocking_recv() {
            Some(ready) => {
                *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
                total_delivered += 1;
            }
            None => break, // scheduler dropped consumer_tx
        }
    }

    // Shutdown and join
    let _ = tx.send(SchedulerCommand::Shutdown);
    drop(tx);
    handle.join().unwrap();

    assert!(
        total_delivered >= 10_000,
        "should deliver at least 10,000 messages, got {total_delivered}"
    );

    // Verify each key's share is within 5% of its fair share
    for (key, weight) in &weights {
        let expected_share = *weight as f64 / total_weight as f64;
        let actual = counts.get(*key).copied().unwrap_or(0);
        let actual_share = actual as f64 / total_delivered as f64;
        let diff = (actual_share - expected_share).abs();

        assert!(
            diff <= 0.05,
            "Key {key} (weight={weight}): expected share {expected_share:.4}, actual {actual_share:.4}, diff {diff:.4} > 0.05. Delivered {actual}/{total_delivered}"
        );
    }
}

#[test]
fn drr_default_weight_is_one() {
    // Messages enqueued via test_message_with_key use weight=1 (default).
    // Two keys with default weight should get equal delivery.
    let (tx, mut scheduler, _dir) = test_setup();
    send_create_queue(&tx, "default-weight");

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "default-weight".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 20 messages per key, using the default-weight helper
    for i in 0u64..20 {
        let msg = test_message_with_key("default-weight", "key_a", i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    for i in 20u64..40 {
        let msg = test_message_with_key("default-weight", "key_b", i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
    }

    let a = counts.get("key_a").copied().unwrap_or(0);
    let b = counts.get("key_b").copied().unwrap_or(0);
    assert_eq!(a, 20, "key_a should get all 20 messages");
    assert_eq!(b, 20, "key_b should get all 20 messages");
}

#[test]
fn drr_weight_zero_treated_as_one() {
    // AC#3: messages with weight=0 should be clamped to weight=1.
    // Two keys with weight=0 behave the same as weight=1 (equal delivery).
    let (tx, mut scheduler, _dir) = test_setup();
    send_create_queue(&tx, "weight-zero");

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "weight-zero".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    for i in 0u64..10 {
        let msg = test_message_with_key_and_weight("weight-zero", "key_a", 0, i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    for i in 10u64..20 {
        let msg = test_message_with_key_and_weight("weight-zero", "key_b", 0, i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
    }

    let a = counts.get("key_a").copied().unwrap_or(0);
    let b = counts.get("key_b").copied().unwrap_or(0);
    assert_eq!(a, 10, "key_a (weight=0→1) should get all 10 messages");
    assert_eq!(b, 10, "key_b (weight=0→1) should get all 10 messages");
}

#[test]
fn drr_weight_update_changes_proportions() {
    // Verify AC#4: weight changes take effect on the next scheduling round.
    //
    // Strategy: enqueue all messages BEFORE registering the consumer.
    // During enqueue (no consumer), drr_deliver_queue returns immediately,
    // so no delivery happens. After RegisterConsumer, the DRR runs with
    // final weights. Shutdown follows immediately, so only ~2 DRR rounds
    // execute, delivering a subset of messages proportional to weights.
    //
    // quantum=5, key_a weight starts at 1 then updates to 3, key_b stays 1.
    // Per round: key_a gets 3*5=15 msgs, key_b gets 1*5=5 msgs.
    // ~2 rounds → key_a≈30, key_b≈10 → 3:1 ratio.
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 5,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
    let lua_config = LuaConfig::default();
    let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);

    send_create_queue(&tx, "weight-update");

    // Establish key_a with weight=1
    let msg = test_message_with_key_and_weight("weight-update", "key_a", 1, 0);
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Update key_a's weight to 3 via subsequent enqueues
    let mut ts = 1u64;
    for _ in 0..49 {
        let msg = test_message_with_key_and_weight("weight-update", "key_a", 3, ts);
        ts += 1;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // key_b: 50 messages with weight=1
    for _ in 0..50 {
        let msg = test_message_with_key_and_weight("weight-update", "key_b", 1, ts);
        ts += 1;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // Register consumer AFTER all enqueues — DRR sees all messages at once
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "weight-update".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(ready) = consumer_rx.try_recv() {
        *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
    }

    let total: usize = counts.values().sum();
    let a = counts.get("key_a").copied().unwrap_or(0);

    // With weight 3:1 and quantum=5, each round delivers 15+5=20.
    // 2 rounds → key_a=30, key_b=10 → exactly 75%:25%.
    assert!(total > 0, "should deliver some messages");
    let a_share = a as f64 / total as f64;
    assert!(
        (a_share - 0.75).abs() <= 0.05,
        "key_a (weight=3) expected ~75% share, got {:.1}% ({a}/{total})",
        a_share * 100.0
    );
}
