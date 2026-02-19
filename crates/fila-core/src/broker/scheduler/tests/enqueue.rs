use super::*;

#[test]
fn enqueue_to_nonexistent_queue_returns_error() {
    let (tx, mut scheduler, _dir) = test_setup();

    let msg = test_message("no-such-queue");
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    let err = reply_rx.try_recv().unwrap().unwrap_err();
    assert!(
        matches!(err, crate::error::EnqueueError::QueueNotFound(_)),
        "expected QueueNotFound, got {err:?}"
    );
}

#[test]
fn enqueue_persists_message_to_storage() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "persist-queue");

    let msg = test_message("persist-queue");
    let msg_id = msg.id;
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

    tx.send(SchedulerCommand::Enqueue {
        message: msg,
        reply: reply_tx,
    })
    .unwrap();
    tx.send(SchedulerCommand::Shutdown).unwrap();

    scheduler.run();

    let result = reply_rx.try_recv().unwrap().unwrap();
    assert_eq!(result, msg_id);

    // Verify the message was persisted by reading it back from storage
    let key = crate::storage::keys::message_key("persist-queue", "default", 1_000_000_000, &msg_id);
    let stored = scheduler.storage().get_message(&key).unwrap();
    assert!(stored.is_some(), "message should be persisted in storage");

    let stored_msg = stored.unwrap();
    assert_eq!(stored_msg.id, msg_id);
    assert_eq!(stored_msg.queue_id, "persist-queue");
    assert_eq!(stored_msg.payload, vec![1, 2, 3]);
}

#[test]
fn enqueue_100_messages_unique_time_ordered_ids() {
    let (tx, mut scheduler, _dir) = test_setup();

    send_create_queue(&tx, "bulk-queue");

    let mut receivers = Vec::with_capacity(100);
    for i in 0u64..100 {
        let mut msg = test_message("bulk-queue");
        msg.enqueued_at = i;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
        receivers.push(reply_rx);
    }

    tx.send(SchedulerCommand::Shutdown).unwrap();
    scheduler.run();

    // Collect all returned IDs
    let ids: Vec<Uuid> = receivers
        .into_iter()
        .map(|mut rx| rx.try_recv().unwrap().unwrap())
        .collect();

    // All IDs must be unique
    let unique_count = {
        let mut set = std::collections::HashSet::new();
        ids.iter().for_each(|id| {
            set.insert(*id);
        });
        set.len()
    };
    assert_eq!(unique_count, 100, "all 100 message IDs must be unique");

    // UUIDv7 IDs are time-ordered, so sorted order should match insertion order
    let mut sorted_ids = ids.clone();
    sorted_ids.sort();
    assert_eq!(ids, sorted_ids, "UUIDv7 IDs should be time-ordered");

    // Verify all 100 messages are persisted
    let prefix = crate::storage::keys::message_prefix("bulk-queue");
    let stored = scheduler.storage().list_messages(&prefix).unwrap();
    assert_eq!(stored.len(), 100, "all 100 messages should be persisted");
}
