use super::*;

// ── SetConfig / GetConfig tests ──────────────────────────────────

#[test]
fn set_config_throttle_key_sets_rate_in_throttle_manager() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.provider_a".to_string(),
        value: "10.0,100.0".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Verify the throttle manager has the rate
    assert!(scheduler.throttle.has_key("provider_a"));
}

#[test]
fn set_config_empty_value_removes_throttle_rate() {
    let (_tx, mut scheduler, _dir) = test_setup();

    // Set a rate first
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.provider_a".to_string(),
        value: "10.0,100.0".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();
    assert!(scheduler.throttle.has_key("provider_a"));

    // Remove it with empty value
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.provider_a".to_string(),
        value: String::new(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    assert!(!scheduler.throttle.has_key("provider_a"));
}

#[test]
fn set_config_persists_to_state_cf() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.provider_a".to_string(),
        value: "10.0,100.0".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Verify persisted in storage
    let stored = scheduler
        .storage()
        .get_state("throttle.provider_a")
        .unwrap()
        .unwrap();
    assert_eq!(stored, b"10.0,100.0");
}

#[test]
fn get_config_returns_stored_value() {
    let (_tx, mut scheduler, _dir) = test_setup();

    // Set a value
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.provider_a".to_string(),
        value: "10.0,100.0".to_string(),
        reply: reply_tx,
    });

    // Get it back
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetConfig {
        key: "throttle.provider_a".to_string(),
        reply: reply_tx,
    });
    let value = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(value, Some("10.0,100.0".to_string()));
}

#[test]
fn get_config_returns_none_for_missing_key() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetConfig {
        key: "nonexistent".to_string(),
        reply: reply_tx,
    });
    let value = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(value, None);
}

#[test]
fn set_config_non_throttle_key_persists_without_affecting_throttle_manager() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "app.feature_flag".to_string(),
        value: "enabled".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Should be persisted
    let stored = scheduler
        .storage()
        .get_state("app.feature_flag")
        .unwrap()
        .unwrap();
    assert_eq!(stored, b"enabled");

    // ThrottleManager should be unaffected
    assert!(scheduler.throttle.is_empty());
}

#[test]
fn set_config_invalid_throttle_value_returns_error() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.provider_a".to_string(),
        value: "not_a_number".to_string(),
        reply: reply_tx,
    });
    let result = reply_rx.blocking_recv().unwrap();
    assert!(result.is_err());

    // Verify the invalid value was NOT persisted to storage
    assert!(scheduler
        .storage()
        .get_state("throttle.provider_a")
        .unwrap()
        .is_none());
}

#[test]
fn set_config_rejects_nan_and_infinity() {
    let (_tx, mut scheduler, _dir) = test_setup();

    for bad_value in &["NaN,1.0", "1.0,inf", "-1.0,10.0", "10.0,-5.0"] {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        scheduler.handle_command(SchedulerCommand::SetConfig {
            key: "throttle.bad".to_string(),
            value: bad_value.to_string(),
            reply: reply_tx,
        });
        let result = reply_rx.blocking_recv().unwrap();
        assert!(result.is_err(), "should reject {bad_value}");
        assert!(
            scheduler
                .storage()
                .get_state("throttle.bad")
                .unwrap()
                .is_none(),
            "rejected value {bad_value} should not be persisted"
        );
    }
}

#[test]
fn set_config_rejects_empty_throttle_key_name() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.".to_string(), // just prefix, empty key name
        value: "10.0,100.0".to_string(),
        reply: reply_tx,
    });
    let result = reply_rx.blocking_recv().unwrap();
    assert!(result.is_err());
}

#[test]
fn recovery_restores_throttle_rates_from_state_cf() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

    // Pre-populate throttle rates in state CF
    storage
        .put_state("throttle.provider_a", b"10.0,100.0")
        .unwrap();
    storage
        .put_state("throttle.region:us-east-1", b"50.0,200.0")
        .unwrap();
    // Also a non-throttle key to verify it's ignored
    storage.put_state("app.flag", b"true").unwrap();

    // Create a fresh scheduler — recovery happens inside run()
    let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));

    // Send a SetConfig for a non-throttle key so we can verify recovery
    // happened by checking the throttle manager after run().
    // We also register a consumer so we can assert throttle state from
    // the delivery side later if needed.
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Verify throttle rates were restored during recovery
    assert!(scheduler.throttle.has_key("provider_a"));
    assert!(scheduler.throttle.has_key("region:us-east-1"));
    assert_eq!(scheduler.throttle.len(), 2);
}

#[test]
fn set_config_throttle_rate_enforced_on_delivery() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "config-throttle-q");

    // Set throttle rate via SetConfig: 0 rate, 1 burst (only 1 delivery)
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.rate:global".to_string(),
        value: "0.0,1.0".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(10);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "config-throttle-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();

    // Enqueue 3 messages with throttle key "rate:global"
    for i in 0..3 {
        let msg =
            test_message_with_throttle_keys("config-throttle-q", vec!["rate:global".into()], i);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    scheduler.handle_all_pending(&tx);

    // Only 1 message should be delivered (bucket starts with 1 token, no refill)
    assert!(consumer_rx.try_recv().is_ok(), "first message delivered");
    assert!(
        consumer_rx.try_recv().is_err(),
        "second message throttled (bucket exhausted)"
    );

    // Phase 2: Update rate to allow remaining messages (high rate refills bucket)
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "throttle.rate:global".to_string(),
        value: "1000000.0,10.0".to_string(), // high rate + burst refills instantly
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // handle_all_pending calls refill_all then drr_deliver
    scheduler.handle_all_pending(&tx);

    // The remaining 2 messages should now be delivered after rate update + refill
    assert!(
        consumer_rx.try_recv().is_ok(),
        "second message delivered after rate update"
    );
    assert!(
        consumer_rx.try_recv().is_ok(),
        "third message delivered after rate update"
    );
}

#[test]
fn list_config_returns_all_entries_with_empty_prefix() {
    let (_tx, mut scheduler, _dir) = test_setup();

    // Set multiple config entries
    for (key, value) in &[
        ("throttle.provider_a", "10.0,100.0"),
        ("app.feature_flag", "enabled"),
        ("app.routing", "tenant"),
    ] {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        scheduler.handle_command(SchedulerCommand::SetConfig {
            key: key.to_string(),
            value: value.to_string(),
            reply: reply_tx,
        });
        reply_rx.blocking_recv().unwrap().unwrap();
    }

    // List all with empty prefix
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListConfig {
        prefix: String::new(),
        reply: reply_tx,
    });
    let entries = reply_rx.blocking_recv().unwrap().unwrap();

    assert_eq!(entries.len(), 3);
    // Entries should be sorted by key (RocksDB lexicographic order)
    assert_eq!(entries[0], ("app.feature_flag".into(), "enabled".into()));
    assert_eq!(entries[1], ("app.routing".into(), "tenant".into()));
    assert_eq!(
        entries[2],
        ("throttle.provider_a".into(), "10.0,100.0".into())
    );
}

#[test]
fn list_config_filters_by_prefix() {
    let (_tx, mut scheduler, _dir) = test_setup();

    for (key, value) in &[
        ("throttle.provider_a", "10.0,100.0"),
        ("throttle.region:us", "50.0,200.0"),
        ("app.feature_flag", "enabled"),
    ] {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        scheduler.handle_command(SchedulerCommand::SetConfig {
            key: key.to_string(),
            value: value.to_string(),
            reply: reply_tx,
        });
        reply_rx.blocking_recv().unwrap().unwrap();
    }

    // List with "throttle." prefix
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListConfig {
        prefix: "throttle.".to_string(),
        reply: reply_tx,
    });
    let entries = reply_rx.blocking_recv().unwrap().unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0],
        ("throttle.provider_a".into(), "10.0,100.0".into())
    );
    assert_eq!(
        entries[1],
        ("throttle.region:us".into(), "50.0,200.0".into())
    );
}

#[test]
fn list_config_returns_empty_vec_when_no_entries() {
    let (_tx, mut scheduler, _dir) = test_setup();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListConfig {
        prefix: "nonexistent.".to_string(),
        reply: reply_tx,
    });
    let entries = reply_rx.blocking_recv().unwrap().unwrap();

    assert!(entries.is_empty());
}

#[test]
fn list_config_integration_set_list_filter() {
    let (_tx, mut scheduler, _dir) = test_setup();

    // Set throttle and non-throttle config values via SetConfig command
    for (key, value) in &[
        ("throttle.provider_a", "10.0,100.0"),
        ("throttle.region:us", "50.0,200.0"),
        ("app.routing_key", "tenant-priority"),
        ("app.feature_flag", "enabled"),
    ] {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        scheduler.handle_command(SchedulerCommand::SetConfig {
            key: key.to_string(),
            value: value.to_string(),
            reply: reply_tx,
        });
        reply_rx.blocking_recv().unwrap().unwrap();
    }

    // List all — should return all 4
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListConfig {
        prefix: String::new(),
        reply: reply_tx,
    });
    let all = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(all.len(), 4);

    // List throttle prefix — should return 2
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListConfig {
        prefix: "throttle.".to_string(),
        reply: reply_tx,
    });
    let throttle = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(throttle.len(), 2);
    assert_eq!(
        throttle[0],
        ("throttle.provider_a".into(), "10.0,100.0".into())
    );
    assert_eq!(
        throttle[1],
        ("throttle.region:us".into(), "50.0,200.0".into())
    );

    // List app prefix — should return 2
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListConfig {
        prefix: "app.".to_string(),
        reply: reply_tx,
    });
    let app = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(app.len(), 2);
    assert_eq!(app[0], ("app.feature_flag".into(), "enabled".into()));
    assert_eq!(app[1], ("app.routing_key".into(), "tenant-priority".into()));

    // List non-matching prefix — empty
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListConfig {
        prefix: "zzz.".to_string(),
        reply: reply_tx,
    });
    let empty = reply_rx.blocking_recv().unwrap().unwrap();
    assert!(empty.is_empty());
}

#[test]
fn lua_e2e_non_throttle_config_via_set_config() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Set a non-throttle config value via SetConfig command (not direct storage)
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::SetConfig {
        key: "app.routing_key".to_string(),
        value: "tenant-priority".to_string(),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Create a queue with a Lua on_enqueue script that reads the config
    let script = r#"
        function on_enqueue(msg)
            local routing = fila.get("app.routing_key")
            return { fairness_key = routing or "default", weight = 1 }
        end
    "#;
    send_create_queue_with_script(&tx, "lua-config-e2e", script);

    // Enqueue a message — the Lua script should read the config and use it
    let msg = test_message("lua-config-e2e");
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Register consumer and run to completion
    let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(10);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "lua-config-e2e".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Verify the delivered message has the fairness_key from the config
    let delivered = consumer_rx
        .try_recv()
        .expect("should have received a message");
    assert_eq!(delivered.msg_id, msg_id);
    assert_eq!(
        delivered.fairness_key, "tenant-priority",
        "fairness_key should come from fila.get('app.routing_key')"
    );
}
