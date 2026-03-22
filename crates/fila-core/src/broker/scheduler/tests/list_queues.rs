use super::*;

#[test]
fn list_queues_returns_all_queues_with_summary() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Empty initially
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListQueues { reply: reply_tx });
    let queues = reply_rx.blocking_recv().unwrap().unwrap();
    assert!(queues.is_empty());

    // Create a queue (auto-creates .dlq)
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::CreateQueue {
        name: "list-q".to_string(),
        config: crate::queue::QueueConfig::new("list-q".to_string()),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // List should return 2 queues (list-q + list-q.dlq)
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListQueues { reply: reply_tx });
    let queues = reply_rx.blocking_recv().unwrap().unwrap();
    assert_eq!(queues.len(), 2);

    let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
    assert!(names.contains(&"list-q"));
    assert!(names.contains(&"list-q.dlq"));

    for q in &queues {
        assert_eq!(q.depth, 0);
        assert_eq!(q.in_flight, 0);
        assert_eq!(q.active_consumers, 0);
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
}

#[test]
fn list_queues_reports_nonzero_depth_and_consumers() {
    let (tx, mut scheduler, _dir) = test_setup();

    // Create a queue
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::CreateQueue {
        name: "stats-q".to_string(),
        config: crate::queue::QueueConfig::new("stats-q".to_string()),
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Enqueue a message
    let msg = Message {
        id: Uuid::now_v7(),
        queue_id: "stats-q".to_string(),
        headers: HashMap::new(),
        payload: vec![1],
        fairness_key: "fk".to_string(),
        weight: 1,
        throttle_keys: vec![],
        attempt_count: 0,
        enqueued_at: 1_000_000_000,
        leased_at: None,
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    });
    reply_rx.blocking_recv().unwrap().unwrap();

    // Register a consumer
    let (consumer_tx, _consumer_rx) = tokio::sync::mpsc::channel(10);
    scheduler.handle_command(SchedulerCommand::RegisterConsumer {
        queue_id: "stats-q".to_string(),
        consumer_id: "c1".to_string(),
        tx: consumer_tx,
    });

    // Deliver (makes the message in-flight)
    scheduler.handle_all_pending(&tx);

    // ListQueues should report 1 depth, 1 in_flight, 1 consumer for stats-q
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::ListQueues { reply: reply_tx });
    let queues = reply_rx.blocking_recv().unwrap().unwrap();
    let q = queues.iter().find(|q| q.name == "stats-q").unwrap();
    assert_eq!(q.depth, 1, "depth should include in-flight");
    assert_eq!(q.in_flight, 1);
    assert_eq!(q.active_consumers, 1);

    tx.send(SchedulerCommand::Shutdown).unwrap();
}
