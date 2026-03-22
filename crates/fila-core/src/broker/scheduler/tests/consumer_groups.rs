use super::*;

#[test]
fn consumer_group_distributes_messages_round_robin() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "cg-queue");

    // Register 3 consumers in the same group
    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "cg-queue".to_string(),
        consumer_id: "c1".to_string(),
        consumer_group: Some("group-a".to_string()),
        tx: c1_tx,
    })
    .unwrap();

    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "cg-queue".to_string(),
        consumer_id: "c2".to_string(),
        consumer_group: Some("group-a".to_string()),
        tx: c2_tx,
    })
    .unwrap();

    let (c3_tx, mut c3_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "cg-queue".to_string(),
        consumer_id: "c3".to_string(),
        consumer_group: Some("group-a".to_string()),
        tx: c3_tx,
    })
    .unwrap();

    // Enqueue 9 messages (should be 3 per consumer)
    let mut msg_ids = Vec::new();
    for i in 0u64..9 {
        let mut msg = test_message("cg-queue");
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

    // Collect messages from each consumer
    let mut c1_msgs = Vec::new();
    while let Ok(ready) = c1_rx.try_recv() {
        c1_msgs.push(ready.msg_id);
    }
    let mut c2_msgs = Vec::new();
    while let Ok(ready) = c2_rx.try_recv() {
        c2_msgs.push(ready.msg_id);
    }
    let mut c3_msgs = Vec::new();
    while let Ok(ready) = c3_rx.try_recv() {
        c3_msgs.push(ready.msg_id);
    }

    let total = c1_msgs.len() + c2_msgs.len() + c3_msgs.len();
    assert_eq!(total, 9, "all 9 messages should be delivered");

    // Each consumer should get exactly 3 messages (round-robin in a group)
    assert_eq!(c1_msgs.len(), 3, "c1 should get 3 messages");
    assert_eq!(c2_msgs.len(), 3, "c2 should get 3 messages");
    assert_eq!(c3_msgs.len(), 3, "c3 should get 3 messages");

    // No duplicates
    let mut all_ids: Vec<_> = c1_msgs
        .iter()
        .chain(c2_msgs.iter())
        .chain(c3_msgs.iter())
        .copied()
        .collect();
    all_ids.sort();
    all_ids.dedup();
    assert_eq!(all_ids.len(), 9, "no message duplicates");
}

#[test]
fn consumer_group_rebalances_on_leave() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "rebal-queue");

    // Register 3 consumers in the same group
    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "rebal-queue".to_string(),
        consumer_id: "c1".to_string(),
        consumer_group: Some("grp".to_string()),
        tx: c1_tx,
    })
    .unwrap();

    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "rebal-queue".to_string(),
        consumer_id: "c2".to_string(),
        consumer_group: Some("grp".to_string()),
        tx: c2_tx,
    })
    .unwrap();

    let (c3_tx, mut c3_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "rebal-queue".to_string(),
        consumer_id: "c3".to_string(),
        consumer_group: Some("grp".to_string()),
        tx: c3_tx,
    })
    .unwrap();

    // Enqueue 3 messages — one to each consumer
    for i in 0u64..3 {
        let mut msg = test_message("rebal-queue");
        msg.enqueued_at = i;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
    }

    // Process all pending commands to deliver the first 3 messages
    scheduler.handle_all_pending(&tx);

    // Drain first batch
    let mut phase1_c1 = 0;
    while c1_rx.try_recv().is_ok() {
        phase1_c1 += 1;
    }
    let mut phase1_c2 = 0;
    while c2_rx.try_recv().is_ok() {
        phase1_c2 += 1;
    }
    let mut phase1_c3 = 0;
    while c3_rx.try_recv().is_ok() {
        phase1_c3 += 1;
    }
    assert_eq!(
        phase1_c1 + phase1_c2 + phase1_c3,
        3,
        "3 messages delivered in phase 1"
    );

    // Now c3 disconnects
    tx.send(SchedulerCommand::UnregisterConsumer {
        consumer_id: "c3".to_string(),
    })
    .unwrap();
    drop(c3_rx);

    // Enqueue 6 more messages — should be distributed across c1 and c2 only
    for i in 10u64..16 {
        let mut msg = test_message("rebal-queue");
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

    // Collect phase 2 messages
    let mut phase2_c1 = 0;
    while c1_rx.try_recv().is_ok() {
        phase2_c1 += 1;
    }
    let mut phase2_c2 = 0;
    while c2_rx.try_recv().is_ok() {
        phase2_c2 += 1;
    }

    let phase2_total = phase2_c1 + phase2_c2;
    assert_eq!(phase2_total, 6, "all 6 messages delivered after rebalance");
    // Each remaining consumer should get ~50% (3 each)
    assert_eq!(phase2_c1, 3, "c1 should get 3 messages after rebalance");
    assert_eq!(phase2_c2, 3, "c2 should get 3 messages after rebalance");
}

#[test]
fn no_consumer_group_works_as_independent() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "indep-queue");

    // Register 2 consumers without groups (classic behavior)
    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "indep-queue".to_string(),
        consumer_id: "c1".to_string(),
        consumer_group: None,
        tx: c1_tx,
    })
    .unwrap();

    let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "indep-queue".to_string(),
        consumer_id: "c2".to_string(),
        consumer_group: None,
        tx: c2_tx,
    })
    .unwrap();

    // Enqueue 4 messages
    for i in 0u64..4 {
        let mut msg = test_message("indep-queue");
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

    let mut c1_msgs = 0;
    while c1_rx.try_recv().is_ok() {
        c1_msgs += 1;
    }
    let mut c2_msgs = 0;
    while c2_rx.try_recv().is_ok() {
        c2_msgs += 1;
    }

    // Independent consumers get round-robin delivery (same as before)
    assert_eq!(c1_msgs + c2_msgs, 4, "all messages delivered");
    assert_eq!(c1_msgs, 2, "c1 gets 2 messages");
    assert_eq!(c2_msgs, 2, "c2 gets 2 messages");
}

#[test]
fn mixed_group_and_independent_consumers() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "mix-queue");

    // Register a group with 2 members
    let (g1_tx, mut g1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "mix-queue".to_string(),
        consumer_id: "g1".to_string(),
        consumer_group: Some("group-a".to_string()),
        tx: g1_tx,
    })
    .unwrap();

    let (g2_tx, mut g2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "mix-queue".to_string(),
        consumer_id: "g2".to_string(),
        consumer_group: Some("group-a".to_string()),
        tx: g2_tx,
    })
    .unwrap();

    // Register an independent consumer
    let (ind_tx, mut ind_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "mix-queue".to_string(),
        consumer_id: "independent".to_string(),
        consumer_group: None,
        tx: ind_tx,
    })
    .unwrap();

    // Enqueue 6 messages
    for i in 0u64..6 {
        let mut msg = test_message("mix-queue");
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

    let mut g1_count = 0;
    while g1_rx.try_recv().is_ok() {
        g1_count += 1;
    }
    let mut g2_count = 0;
    while g2_rx.try_recv().is_ok() {
        g2_count += 1;
    }
    let mut ind_count = 0;
    while ind_rx.try_recv().is_ok() {
        ind_count += 1;
    }

    // There are 2 delivery targets: group-a and independent.
    // Round-robin across targets: 3 messages to group-a, 3 to independent.
    // Within group-a, round-robin: ~1-2 each.
    let group_total = g1_count + g2_count;
    assert_eq!(group_total + ind_count, 6, "all 6 messages delivered");
    // Group-a gets half, independent gets half
    assert_eq!(group_total, 3, "group-a gets 3 messages total");
    assert_eq!(ind_count, 3, "independent gets 3 messages");
}

#[test]
fn get_consumer_groups_returns_group_info() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "info-queue");

    // Register consumers in groups
    let (c1_tx, _c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "info-queue".to_string(),
        consumer_id: "c1".to_string(),
        consumer_group: Some("grp1".to_string()),
        tx: c1_tx,
    })
    .unwrap();

    let (c2_tx, _c2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "info-queue".to_string(),
        consumer_id: "c2".to_string(),
        consumer_group: Some("grp1".to_string()),
        tx: c2_tx,
    })
    .unwrap();

    let (c3_tx, _c3_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "info-queue".to_string(),
        consumer_id: "c3".to_string(),
        consumer_group: Some("grp2".to_string()),
        tx: c3_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(&tx);

    // Query consumer groups via GetConsumerGroups command
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetConsumerGroups {
        queue_filter: None,
        reply: reply_tx,
    });
    let groups = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(groups.len(), 2, "should have 2 groups");

    let grp1 = groups.iter().find(|g| g.group_name == "grp1").unwrap();
    assert_eq!(grp1.member_count, 2);
    assert_eq!(grp1.queue, "info-queue");

    let grp2 = groups.iter().find(|g| g.group_name == "grp2").unwrap();
    assert_eq!(grp2.member_count, 1);

    // Filter by queue
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetConsumerGroups {
        queue_filter: Some("info-queue".to_string()),
        reply: reply_tx,
    });
    let filtered = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(filtered.len(), 2);

    // Filter by non-existent queue
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::GetConsumerGroups {
        queue_filter: Some("other-queue".to_string()),
        reply: reply_tx,
    });
    let empty = reply_rx.blocking_recv().unwrap().unwrap();
    assert!(empty.is_empty());
}

#[test]
fn consumer_group_single_member_gets_all() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "solo-queue");

    // Register 1 consumer in a group
    let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "solo-queue".to_string(),
        consumer_id: "c1".to_string(),
        consumer_group: Some("solo-grp".to_string()),
        tx: c1_tx,
    })
    .unwrap();

    // Enqueue 5 messages
    for i in 0u64..5 {
        let mut msg = test_message("solo-queue");
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

    let mut count = 0;
    while c1_rx.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, 5, "solo consumer in group gets all messages");
}

#[test]
fn multiple_groups_on_same_queue() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "multi-grp-queue");

    // Group A: 2 consumers
    let (ga1_tx, mut ga1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "multi-grp-queue".to_string(),
        consumer_id: "ga1".to_string(),
        consumer_group: Some("group-a".to_string()),
        tx: ga1_tx,
    })
    .unwrap();

    let (ga2_tx, mut ga2_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "multi-grp-queue".to_string(),
        consumer_id: "ga2".to_string(),
        consumer_group: Some("group-a".to_string()),
        tx: ga2_tx,
    })
    .unwrap();

    // Group B: 1 consumer
    let (gb1_tx, mut gb1_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: "multi-grp-queue".to_string(),
        consumer_id: "gb1".to_string(),
        consumer_group: Some("group-b".to_string()),
        tx: gb1_tx,
    })
    .unwrap();

    // Enqueue 6 messages
    for i in 0u64..6 {
        let mut msg = test_message("multi-grp-queue");
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

    let mut ga1_count = 0;
    while ga1_rx.try_recv().is_ok() {
        ga1_count += 1;
    }
    let mut ga2_count = 0;
    while ga2_rx.try_recv().is_ok() {
        ga2_count += 1;
    }
    let mut gb1_count = 0;
    while gb1_rx.try_recv().is_ok() {
        gb1_count += 1;
    }

    let total = ga1_count + ga2_count + gb1_count;
    assert_eq!(total, 6, "all 6 messages delivered");

    // 2 delivery targets: group-a and group-b
    // Round-robin: 3 to group-a, 3 to group-b
    let group_a_total = ga1_count + ga2_count;
    assert_eq!(group_a_total, 3, "group-a gets 3 messages");
    assert_eq!(gb1_count, 3, "group-b gets 3 messages");
}
