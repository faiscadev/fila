use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::broker::command::{QueueSummary, SchedulerCommand};
use crate::error::{BrokerError, BrokerResult, ListQueuesError};

/// Routes scheduler commands to the correct shard based on queue name.
///
/// When `shard_count` is 1 (the default), all commands go to shard 0 and
/// behavior is identical to the pre-sharding single-scheduler architecture.
///
/// For `shard_count > 1`, each queue name is consistently hashed to a shard
/// index, ensuring all operations for a given queue are processed by the
/// same scheduler thread. This preserves per-queue message ordering while
/// enabling parallel processing of different queues.
pub struct ShardRouter {
    senders: Vec<crossbeam_channel::Sender<SchedulerCommand>>,
}

impl ShardRouter {
    /// Create a new shard router with the given per-shard senders.
    pub fn new(senders: Vec<crossbeam_channel::Sender<SchedulerCommand>>) -> Self {
        assert!(!senders.is_empty(), "shard_count must be >= 1");
        Self { senders }
    }

    /// Determine which shard owns a queue.
    fn shard_for(&self, queue_id: &str) -> usize {
        if self.senders.len() == 1 {
            return 0;
        }
        let mut hasher = DefaultHasher::new();
        queue_id.hash(&mut hasher);
        (hasher.finish() as usize) % self.senders.len()
    }

    /// Send a queue-targeted command to the correct shard.
    pub fn send_to_queue(&self, queue_id: &str, cmd: SchedulerCommand) -> BrokerResult<()> {
        let shard = self.shard_for(queue_id);
        self.senders[shard].try_send(cmd).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => BrokerError::ChannelFull,
            crossbeam_channel::TrySendError::Disconnected(_) => BrokerError::ChannelDisconnected,
        })
    }

    /// Send a command to shard 0. Used for non-queue-targeted commands
    /// (config, throttle) that don't need routing.
    pub fn send_to_shard0(&self, cmd: SchedulerCommand) -> BrokerResult<()> {
        self.senders[0].try_send(cmd).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => BrokerError::ChannelFull,
            crossbeam_channel::TrySendError::Disconnected(_) => BrokerError::ChannelDisconnected,
        })
    }

    /// Broadcast a fire-and-forget command to all shards.
    ///
    /// Used for `UnregisterConsumer` which has no queue_id — only the shard
    /// that owns the consumer will find and remove it. The others are no-ops.
    ///
    /// For single-shard, this is equivalent to a direct send.
    pub fn broadcast_fire_and_forget(&self, cmd: SchedulerCommand) -> BrokerResult<()> {
        if self.senders.len() == 1 {
            return self.senders[0].try_send(cmd).map_err(|e| match e {
                crossbeam_channel::TrySendError::Full(_) => BrokerError::ChannelFull,
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    BrokerError::ChannelDisconnected
                }
            });
        }

        // For multi-shard, reconstruct the command for each shard.
        // Only UnregisterConsumer uses this path.
        match cmd {
            SchedulerCommand::UnregisterConsumer { consumer_id } => {
                for sender in &self.senders {
                    let _ = sender.try_send(SchedulerCommand::UnregisterConsumer {
                        consumer_id: consumer_id.clone(),
                    });
                }
                Ok(())
            }
            _ => {
                // Unexpected command type for broadcast — fall back to shard 0
                self.send_to_shard0(cmd)
            }
        }
    }

    /// Broadcast a shutdown command to all shards.
    pub fn broadcast_shutdown(&self) {
        for sender in &self.senders {
            let _ = sender.send(SchedulerCommand::Shutdown);
        }
    }

    /// Broadcast ListQueues to all shards, aggregate results.
    ///
    /// Creates N oneshot channels, sends ListQueues to each shard, collects
    /// all N replies, and sends the merged result through the original reply
    /// channel. For shard_count=1, this is equivalent to direct send.
    pub fn send_list_queues(
        &self,
        reply: tokio::sync::oneshot::Sender<Result<Vec<QueueSummary>, ListQueuesError>>,
    ) -> BrokerResult<()> {
        if self.senders.len() == 1 {
            // Single shard: direct send, no aggregation needed.
            return self.senders[0]
                .try_send(SchedulerCommand::ListQueues { reply })
                .map_err(|e| match e {
                    crossbeam_channel::TrySendError::Full(_) => BrokerError::ChannelFull,
                    crossbeam_channel::TrySendError::Disconnected(_) => {
                        BrokerError::ChannelDisconnected
                    }
                });
        }

        // Multi-shard: send to each shard and aggregate.
        let mut shard_replies = Vec::with_capacity(self.senders.len());
        for sender in &self.senders {
            let (shard_tx, shard_rx) = tokio::sync::oneshot::channel();
            sender
                .try_send(SchedulerCommand::ListQueues { reply: shard_tx })
                .map_err(|e| match e {
                    crossbeam_channel::TrySendError::Full(_) => BrokerError::ChannelFull,
                    crossbeam_channel::TrySendError::Disconnected(_) => {
                        BrokerError::ChannelDisconnected
                    }
                })?;
            shard_replies.push(shard_rx);
        }

        // Spawn a thread to aggregate results and send through the original reply.
        // We use a thread rather than a tokio task because the Broker API is sync.
        std::thread::Builder::new()
            .name("fila-list-queues-agg".to_string())
            .spawn(move || {
                let mut merged = Vec::new();
                for rx in shard_replies {
                    match rx.blocking_recv() {
                        Ok(Ok(summaries)) => merged.extend(summaries),
                        Ok(Err(e)) => {
                            let _ = reply.send(Err(e));
                            return;
                        }
                        Err(_) => {
                            let _ = reply.send(Err(ListQueuesError::Storage(
                                crate::error::StorageError::Engine(
                                    "shard reply channel dropped".to_string(),
                                ),
                            )));
                            return;
                        }
                    }
                }
                // Deduplicate: storage is shared, so each shard sees all queues.
                // Each shard's in-memory counts (pending, in_flight, consumers)
                // only cover queues assigned to that shard. Merge by summing
                // in-memory counts from the shard that owns each queue, taking
                // storage-derived data from any shard (they're identical).
                //
                // Since each queue is owned by exactly one shard, and other shards
                // report zero in-memory counts, we can just deduplicate by name
                // keeping the entry with the highest depth (the owning shard).
                let mut by_name: std::collections::HashMap<String, QueueSummary> =
                    std::collections::HashMap::new();
                for summary in merged {
                    by_name
                        .entry(summary.name.clone())
                        .and_modify(|existing| {
                            // Sum in-memory stats across shards. The owning shard
                            // has the real counts; non-owning shards report 0.
                            existing.depth += summary.depth;
                            existing.in_flight += summary.in_flight;
                            existing.active_consumers += summary.active_consumers;
                        })
                        .or_insert(summary);
                }
                let mut result: Vec<QueueSummary> = by_name.into_values().collect();
                result.sort_by(|a, b| a.name.cmp(&b.name));
                let _ = reply.send(Ok(result));
            })
            .map_err(|e| BrokerError::SchedulerSpawn(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_for_consistent() {
        let (senders, _receivers): (Vec<_>, Vec<_>) = (0..4)
            .map(|_| crossbeam_channel::bounded::<SchedulerCommand>(1))
            .unzip();
        let router = ShardRouter::new(senders);

        // Same queue always maps to same shard.
        let shard_a = router.shard_for("orders");
        let shard_b = router.shard_for("orders");
        assert_eq!(shard_a, shard_b);

        // Different queues can map to different shards.
        // (Not guaranteed, but with 4 shards and these specific names it's very likely.)
        let shards: std::collections::HashSet<usize> = ["q1", "q2", "q3", "q4", "q5", "q6"]
            .iter()
            .map(|q| router.shard_for(q))
            .collect();
        assert!(shards.len() > 1, "expected multiple shards used, got 1");
    }

    #[test]
    fn shard_for_single_shard() {
        let (senders, _receivers): (Vec<_>, Vec<_>) = (0..1)
            .map(|_| crossbeam_channel::bounded::<SchedulerCommand>(1))
            .unzip();
        let router = ShardRouter::new(senders);

        assert_eq!(router.shard_for("anything"), 0);
        assert_eq!(router.shard_for("other"), 0);
    }

    #[test]
    fn shard_for_in_range() {
        let shard_count = 8;
        let (senders, _receivers): (Vec<_>, Vec<_>) = (0..shard_count)
            .map(|_| crossbeam_channel::bounded::<SchedulerCommand>(1))
            .unzip();
        let router = ShardRouter::new(senders);

        for name in ["a", "b", "c", "orders", "payments", "notifications"] {
            let shard = router.shard_for(name);
            assert!(shard < shard_count, "shard {shard} out of range for {name}");
        }
    }
}
