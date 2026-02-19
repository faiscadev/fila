use super::*;

pub(super) fn test_setup() -> (
    crossbeam_channel::Sender<SchedulerCommand>,
    Scheduler,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 1000,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
    let lua_config = LuaConfig::default();
    let scheduler = Scheduler::new(storage, rx, &config, &lua_config);
    (tx, scheduler, dir)
}

pub(super) fn test_message(queue_id: &str) -> Message {
    Message {
        id: Uuid::now_v7(),
        queue_id: queue_id.to_string(),
        headers: HashMap::new(),
        payload: vec![1, 2, 3],
        fairness_key: "default".to_string(),
        weight: 1,
        throttle_keys: vec![],
        attempt_count: 0,
        enqueued_at: 1_000_000_000,
        leased_at: None,
    }
}

/// Helper: send a CreateQueue command so enqueue tests have an existing queue.
pub(super) fn send_create_queue(tx: &crossbeam_channel::Sender<SchedulerCommand>, name: &str) {
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::CreateQueue {
        name: name.to_string(),
        config: crate::queue::QueueConfig::new(name.to_string()),
        reply: reply_tx,
    })
    .unwrap();
}

/// Helper: create a scheduler sharing an existing storage (for restart tests).
pub(super) fn test_setup_with_storage(
    storage: Arc<dyn Storage>,
) -> (crossbeam_channel::Sender<SchedulerCommand>, Scheduler) {
    let config = SchedulerConfig {
        command_channel_capacity: 256,
        idle_timeout_ms: 10,
        quantum: 1000,
    };
    let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
    let lua_config = LuaConfig::default();
    let scheduler = Scheduler::new(storage, rx, &config, &lua_config);
    (tx, scheduler)
}

/// Helper: create a message with a specific fairness key and weight.
pub(super) fn test_message_with_key_and_weight(
    queue_id: &str,
    fairness_key: &str,
    weight: u32,
    enqueued_at: u64,
) -> Message {
    Message {
        id: Uuid::now_v7(),
        queue_id: queue_id.to_string(),
        headers: HashMap::new(),
        payload: vec![1, 2, 3],
        fairness_key: fairness_key.to_string(),
        weight,
        throttle_keys: vec![],
        attempt_count: 0,
        enqueued_at,
        leased_at: None,
    }
}

/// Helper: create a message with a specific fairness key.
pub(super) fn test_message_with_key(
    queue_id: &str,
    fairness_key: &str,
    enqueued_at: u64,
) -> Message {
    Message {
        id: Uuid::now_v7(),
        queue_id: queue_id.to_string(),
        headers: HashMap::new(),
        payload: vec![1, 2, 3],
        fairness_key: fairness_key.to_string(),
        weight: 1,
        throttle_keys: vec![],
        attempt_count: 0,
        enqueued_at,
        leased_at: None,
    }
}

/// Helper: create a queue with a custom visibility timeout (in ms).
pub(super) fn send_create_queue_with_timeout(
    tx: &crossbeam_channel::Sender<SchedulerCommand>,
    name: &str,
    visibility_timeout_ms: u64,
) {
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    let mut config = crate::queue::QueueConfig::new(name.to_string());
    config.visibility_timeout_ms = visibility_timeout_ms;
    tx.send(SchedulerCommand::CreateQueue {
        name: name.to_string(),
        config,
        reply: reply_tx,
    })
    .unwrap();
}

/// Helper: create a queue with an on_enqueue Lua script.
pub(super) fn send_create_queue_with_script(
    tx: &crossbeam_channel::Sender<SchedulerCommand>,
    name: &str,
    script: &str,
) -> tokio::sync::oneshot::Receiver<Result<String, crate::error::CreateQueueError>> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let mut config = crate::queue::QueueConfig::new(name.to_string());
    config.on_enqueue_script = Some(script.to_string());
    tx.send(SchedulerCommand::CreateQueue {
        name: name.to_string(),
        config,
        reply: reply_tx,
    })
    .unwrap();
    reply_rx
}

/// Helper: create a message with specific headers.
pub(super) fn test_message_with_headers(
    queue_id: &str,
    headers: HashMap<String, String>,
) -> Message {
    Message {
        id: Uuid::now_v7(),
        queue_id: queue_id.to_string(),
        headers,
        payload: vec![1, 2, 3],
        fairness_key: "default".to_string(),
        weight: 1,
        throttle_keys: vec![],
        attempt_count: 0,
        enqueued_at: 1_000_000_000,
        leased_at: None,
    }
}

/// Helper: create a queue with an on_failure script and optional DLQ configuration.
pub(super) fn send_create_queue_with_on_failure(
    tx: &crossbeam_channel::Sender<SchedulerCommand>,
    name: &str,
    on_failure_script: &str,
    dlq_queue_id: Option<&str>,
) {
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    let mut config = crate::queue::QueueConfig::new(name.to_string());
    config.on_failure_script = Some(on_failure_script.to_string());
    config.dlq_queue_id = dlq_queue_id.map(|s| s.to_string());
    tx.send(SchedulerCommand::CreateQueue {
        name: name.to_string(),
        config,
        reply: reply_tx,
    })
    .unwrap();
}

/// Helper: create a message with throttle keys.
pub(super) fn test_message_with_throttle_keys(
    queue_id: &str,
    throttle_keys: Vec<String>,
    enqueued_at: u64,
) -> Message {
    Message {
        id: Uuid::now_v7(),
        queue_id: queue_id.to_string(),
        headers: HashMap::new(),
        payload: vec![1, 2, 3],
        fairness_key: "default".to_string(),
        weight: 1,
        throttle_keys,
        attempt_count: 0,
        enqueued_at,
        leased_at: None,
    }
}

/// Helper: enqueue a message, lease it, then nack it to trigger DLQ routing.
/// Returns the msg_id of the dead-lettered message.
pub(super) fn dlq_one_message(
    tx: &crossbeam_channel::Sender<SchedulerCommand>,
    scheduler: &mut Scheduler,
    queue_name: &str,
) -> uuid::Uuid {
    let msg = test_message(queue_name);
    let msg_id = msg.id;
    let (reply_tx, _) = tokio::sync::oneshot::channel();
    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();

    // Register consumer to trigger delivery
    let (consumer_tx, _consumer_rx) = tokio::sync::mpsc::channel(64);
    tx.send(SchedulerCommand::RegisterConsumer {
        queue_id: queue_name.to_string(),
        consumer_id: format!("dlq-helper-{msg_id}"),
        tx: consumer_tx,
    })
    .unwrap();

    scheduler.handle_all_pending(tx);

    // Nack to trigger on_failure → DLQ
    let (nack_tx, nack_rx) = tokio::sync::oneshot::channel();
    scheduler.handle_command(SchedulerCommand::Nack {
        queue_id: queue_name.to_string(),
        msg_id,
        error: "test failure".to_string(),
        reply: nack_tx,
    });
    nack_rx.blocking_recv().unwrap().unwrap();

    // Unregister consumer
    scheduler.handle_command(SchedulerCommand::UnregisterConsumer {
        consumer_id: format!("dlq-helper-{msg_id}"),
    });

    msg_id
}

/// Helper: drain all buffered commands and run a delivery round.
impl Scheduler {
    #[cfg(test)]
    pub(super) fn handle_all_pending(&mut self, _tx: &crossbeam_channel::Sender<SchedulerCommand>) {
        // Drain all pending commands from the inbound channel
        while let Ok(cmd) = self.inbound.try_recv() {
            self.handle_command(cmd);
        }
        // Refill token buckets and run delivery
        self.throttle.refill_all(Instant::now());
        self.drr_deliver();
    }
}
