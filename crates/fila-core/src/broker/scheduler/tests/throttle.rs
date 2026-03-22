use super::*;

#[test]
fn throttle_skips_throttled_message() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "throttle-q");

    // Set a very restrictive throttle: 1 token, burst 1
    tx.send(SchedulerCommand::SetThrottleRate {
        key: "rate:global".to_string(),
        rate_per_second: 0.0, // no refill
        burst: 1.0,
    })
    .unwrap();

    // Register consumer
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "throttle-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 3 messages with throttle key
    for i in 0..3 {
        let msg = test_message_with_throttle_keys("throttle-q", vec!["rate:global".to_string()], i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // Process all commands
    scheduler.handle_all_pending(&tx);

    // Only 1 message should be delivered (1 token available)
    let msg1 = consumer_rx.try_recv();
    assert!(msg1.is_ok(), "first message should be delivered");

    let msg2 = consumer_rx.try_recv();
    assert!(msg2.is_err(), "second message should be throttled");
}

#[test]
fn throttle_multi_key_all_must_pass() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "multi-key-q");

    // Set two throttle keys: one with tokens, one exhausted
    tx.send(SchedulerCommand::SetThrottleRate {
        key: "rate:a".to_string(),
        rate_per_second: 0.0,
        burst: 10.0, // plenty of tokens
    })
    .unwrap();
    tx.send(SchedulerCommand::SetThrottleRate {
        key: "rate:b".to_string(),
        rate_per_second: 0.0,
        burst: 1.0, // only 1 token
    })
    .unwrap();

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "multi-key-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 3 messages with BOTH throttle keys
    for i in 0..3 {
        let msg = test_message_with_throttle_keys(
            "multi-key-q",
            vec!["rate:a".to_string(), "rate:b".to_string()],
            i,
        );
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    scheduler.handle_all_pending(&tx);

    // Only 1 message should be delivered (rate:b has 1 token)
    assert!(consumer_rx.try_recv().is_ok(), "first message delivered");
    assert!(
        consumer_rx.try_recv().is_err(),
        "second message throttled by rate:b"
    );
}

#[test]
fn throttle_refill_allows_delivery() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "refill-q");

    // Rate of 1000/s, burst 1 — drains after 1 delivery, refills quickly
    tx.send(SchedulerCommand::SetThrottleRate {
        key: "rate:fast".to_string(),
        rate_per_second: 1_000_000.0, // very fast refill
        burst: 1.0,
    })
    .unwrap();

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "refill-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 2 messages
    for i in 0..2 {
        let msg = test_message_with_throttle_keys("refill-q", vec!["rate:fast".to_string()], i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    scheduler.handle_all_pending(&tx);

    // First delivery consumes the 1 token
    assert!(consumer_rx.try_recv().is_ok());

    // Refill and deliver again — the fast rate should have refilled
    std::thread::sleep(std::time::Duration::from_millis(1));
    scheduler.throttle.refill_all(Instant::now());
    scheduler.drr_deliver();

    assert!(
        consumer_rx.try_recv().is_ok(),
        "second message should be delivered after refill"
    );
}

#[test]
fn throttle_skipped_key_stays_active() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "active-q");

    // Exhausted throttle
    tx.send(SchedulerCommand::SetThrottleRate {
        key: "rate:exhausted".to_string(),
        rate_per_second: 0.0,
        burst: 0.0, // no tokens at all
    })
    .unwrap();

    let (consumer_tx, mut _consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "active-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue a message with exhausted throttle key
    let msg = test_message_with_throttle_keys("active-q", vec!["rate:exhausted".to_string()], 0);
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(&tx);

    // Key should still be active in DRR (not removed)
    assert!(
        scheduler.drr.has_active_keys("active-q"),
        "throttled key should remain in active set"
    );
}

#[test]
fn throttle_empty_keys_unthrottled() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "unthrottled-q");

    // Set a throttle rate (but messages won't use it)
    tx.send(SchedulerCommand::SetThrottleRate {
        key: "rate:global".to_string(),
        rate_per_second: 0.0,
        burst: 0.0, // exhausted
    })
    .unwrap();

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "unthrottled-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue messages with NO throttle keys (backward compatible)
    for i in 0..3 {
        let msg = test_message_with_throttle_keys("unthrottled-q", vec![], i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    scheduler.handle_all_pending(&tx);

    // All 3 messages should be delivered (empty throttle_keys = unthrottled)
    for i in 0..3 {
        assert!(
            consumer_rx.try_recv().is_ok(),
            "message {i} should be delivered (no throttle keys)"
        );
    }
}
