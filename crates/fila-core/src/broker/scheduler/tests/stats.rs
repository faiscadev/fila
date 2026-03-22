use super::*;

#[test]
fn get_stats_returns_depth_and_in_flight() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "stats-q");

    // Enqueue 3 messages
    for i in 0..3 {
        let msg = test_message_with_key("stats-q", "key-a", i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // Register consumer with capacity 1 so only 1 message gets leased
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(1);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "stats-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(&tx);

    // 1 message should be delivered (leased); channel capacity=1 blocks more
    assert!(consumer_rx.try_recv().is_ok());

    // GetStats: depth=3 (2 pending + 1 in-flight), in_flight=1
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(stats.depth, 3);
    assert_eq!(stats.in_flight, 1);
    assert_eq!(stats.active_consumers, 1);
}

#[test]
fn get_stats_returns_per_fairness_key_stats() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "stats-fk-q");

    // Enqueue 2 messages with key-a and 1 with key-b
    for i in 0..2 {
        let msg = test_message_with_key("stats-fk-q", "key-a", i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    let msg = test_message_with_key("stats-fk-q", "key-b", 10);
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(&tx);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-fk-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();

    assert_eq!(stats.active_fairness_keys, 2);
    assert_eq!(stats.per_key_stats.len(), 2);

    // Find key-a and key-b stats
    let key_a = stats
        .per_key_stats
        .iter()
        .find(|s| s.key == "key-a")
        .unwrap();
    let key_b = stats
        .per_key_stats
        .iter()
        .find(|s| s.key == "key-b")
        .unwrap();
    assert_eq!(key_a.pending_count, 2);
    assert_eq!(key_a.weight, 1); // default weight from test_message_with_key
    assert_eq!(key_b.pending_count, 1);
    assert_eq!(key_b.weight, 1);

    // Deficits should be non-negative (keys were just added, no delivery yet)
    assert!(key_a.current_deficit >= 0);
    assert!(key_b.current_deficit >= 0);
}

#[test]
fn get_stats_returns_weighted_fairness_key_stats() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "stats-wt-q");

    // Enqueue messages with different weights
    let msg = test_message_with_key_and_weight("stats-wt-q", "heavy", 5, 0);
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    let msg = test_message_with_key_and_weight("stats-wt-q", "light", 1, 1);
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(&tx);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-wt-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();

    let heavy = stats
        .per_key_stats
        .iter()
        .find(|s| s.key == "heavy")
        .unwrap();
    let light = stats
        .per_key_stats
        .iter()
        .find(|s| s.key == "light")
        .unwrap();
    assert_eq!(heavy.weight, 5);
    assert_eq!(light.weight, 1);
}

#[test]
fn get_stats_returns_throttle_state() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "stats-throttle-q");
    // Process the queue creation
    scheduler.handle_all_pending(&tx);

    // Set a throttle rate via SetConfig
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.rate:global".to_string(),
        value: "10.0,100.0".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-throttle-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();

    assert_eq!(stats.per_throttle_stats.len(), 1);
    let throttle = &stats.per_throttle_stats[0];
    assert_eq!(throttle.key, "rate:global");
    assert!((throttle.rate_per_second - 10.0).abs() < f64::EPSILON);
    assert!((throttle.burst - 100.0).abs() < f64::EPSILON);
}

#[test]
fn get_stats_nonexistent_queue_returns_not_found() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "nonexistent".to_string(),
        reply: reply_tx,
    });
    let result = reply_rx.blocking_recv().unwrap();
    assert!(result.is_err());
}

#[test]
fn get_stats_integration_multi_key_with_leases_and_throttle() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "stats-int-q");
    scheduler.handle_all_pending(&tx);

    // Set throttle rate
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.rate:api".to_string(),
        value: "5.0,50.0".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Enqueue messages across 3 fairness keys, all with throttle key "rate:api"
    for i in 0..3 {
        let mut msg = test_message_with_key("stats-int-q", "tenant-a", i);
        msg.throttle_keys = vec!["rate:api".to_string()];
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    for i in 10..12 {
        let mut msg = test_message_with_key("stats-int-q", "tenant-b", i);
        msg.throttle_keys = vec!["rate:api".to_string()];
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }
    let mut msg = test_message_with_key("stats-int-q", "tenant-c", 20);
    msg.throttle_keys = vec!["rate:api".to_string()];
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Register consumer (capacity 2: only 2 messages will be leased)
    let (consumer_tx, _consumer_rx) = tokio::sync::mpsc::channel(2);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "stats-int-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(&tx);

    // GetStats
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-int-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();

    // Total depth = 6 (3 + 2 + 1)
    assert_eq!(stats.depth, 6);
    assert_eq!(stats.in_flight, 2);
    assert_eq!(stats.active_consumers, 1);
    assert_eq!(stats.active_fairness_keys, 3);
    assert_eq!(stats.per_key_stats.len(), 3);

    // Throttle state — 2 messages delivered consumed 2 tokens from burst of 50
    assert_eq!(stats.per_throttle_stats.len(), 1);
    assert_eq!(stats.per_throttle_stats[0].key, "rate:api");
    assert!((stats.per_throttle_stats[0].rate_per_second - 5.0).abs() < f64::EPSILON);
    assert!((stats.per_throttle_stats[0].burst - 50.0).abs() < f64::EPSILON);
    assert!(
        stats.per_throttle_stats[0].tokens < 50.0,
        "tokens should be less than burst after delivery consumed some"
    );

    // Quantum should be the configured value
    assert!(stats.quantum > 0);
}

#[test]
fn get_stats_empty_queue_returns_zero_counts() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "stats-empty-q");
    scheduler.handle_all_pending(&tx);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-empty-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(stats.depth, 0);
    assert_eq!(stats.in_flight, 0);
    assert_eq!(stats.active_fairness_keys, 0);
    assert_eq!(stats.active_consumers, 0);
    assert!(stats.per_key_stats.is_empty());
}

#[test]
fn get_stats_after_ack_decreases_in_flight_and_depth() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "stats-ack-q");

    // Enqueue 3 messages
    for i in 0..3 {
        let msg = test_message_with_key("stats-ack-q", "key-a", i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // Register consumer with capacity 1
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(1);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "stats-ack-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(&tx);

    // 1 message leased
    let delivered = consumer_rx.try_recv().unwrap();

    // Before ack: depth=3, in_flight=1
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-ack-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(stats.depth, 3);
    assert_eq!(stats.in_flight, 1);

    // Ack the delivered message
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Ack {
        queue_id: "stats-ack-q".to_string(),
        msg_id: delivered.msg_id,
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // After ack: depth=2 (only pending), in_flight=0
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetStats {
        queue_id: "stats-ack-q".to_string(),
        reply: reply_tx,
    });
    let stats = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(stats.depth, 2);
    assert_eq!(stats.in_flight, 0);
}
