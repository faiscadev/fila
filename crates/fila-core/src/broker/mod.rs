pub mod auth;
pub mod command;
pub mod config;
pub mod drr;
pub mod metrics;
pub mod router;
mod scheduler;
pub mod stats;
pub mod throttle;

use std::sync::Arc;
use std::thread;

use tracing::info;

use crate::error::{BrokerError, BrokerResult};
use crate::storage::StorageEngine;

pub use command::{QueueSummary, ReadyMessage, SchedulerCommand};
pub use config::{AuthConfig, BrokerConfig, TlsParams};

use scheduler::Scheduler;

/// The broker owns the scheduler thread and the inbound command channel.
/// IO threads (gRPC handlers) send commands through `send_command()`,
/// and the single-threaded scheduler processes them sequentially.
pub struct Broker {
    command_tx: crossbeam_channel::Sender<SchedulerCommand>,
    scheduler_thread: Option<thread::JoinHandle<()>>,
    storage: Arc<dyn StorageEngine>,
    /// Whether API key authentication is enabled. Set from `BrokerConfig.auth`.
    pub auth_enabled: bool,
}

impl Broker {
    /// Create a new broker, spawning the scheduler on a dedicated OS thread.
    #[tracing::instrument(skip_all, fields(listen_addr = %config.server.listen_addr))]
    pub fn new(config: BrokerConfig, storage: Arc<dyn StorageEngine>) -> BrokerResult<Self> {
        let (tx, rx) = crossbeam_channel::bounded::<SchedulerCommand>(
            config.scheduler.command_channel_capacity,
        );

        let scheduler_config = config.scheduler.clone();
        let lua_config = config.lua.clone();
        let cluster_node_id = if config.cluster.enabled {
            config.cluster.node_id
        } else {
            0
        };

        let storage_for_scheduler = Arc::clone(&storage);
        let handle = thread::Builder::new()
            .name("fila-scheduler".to_string())
            .spawn(move || {
                let mut scheduler = Scheduler::new(
                    storage_for_scheduler,
                    rx,
                    &scheduler_config,
                    &lua_config,
                    cluster_node_id,
                );
                scheduler.run();
            })
            .map_err(|e| BrokerError::SchedulerSpawn(e.to_string()))?;

        info!("broker started");

        Ok(Self {
            command_tx: tx,
            scheduler_thread: Some(handle),
            storage,
            auth_enabled: config.auth.is_some(),
        })
    }

    /// Send a command to the scheduler. Returns an error if the channel is full
    /// or disconnected.
    #[tracing::instrument(skip_all)]
    pub fn send_command(&self, cmd: SchedulerCommand) -> BrokerResult<()> {
        self.command_tx.try_send(cmd).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => BrokerError::ChannelFull,
            crossbeam_channel::TrySendError::Disconnected(_) => BrokerError::ChannelDisconnected,
        })
    }

    /// Initiate graceful shutdown: send the shutdown command and wait for the
    /// scheduler thread to finish.
    #[tracing::instrument(skip_all)]
    pub fn shutdown(mut self) -> BrokerResult<()> {
        info!("initiating broker shutdown");

        // Send shutdown command (ignore error if channel already closed)
        let _ = self.command_tx.send(SchedulerCommand::Shutdown);

        // Wait for the scheduler thread to finish
        if let Some(handle) = self.scheduler_thread.take() {
            handle.join().map_err(|_| BrokerError::SchedulerPanicked)?;
        }

        info!("broker shutdown complete");
        Ok(())
    }

    // --- API key management ---

    /// Create a new API key. Returns `(key_id, plaintext_token)`.
    ///
    /// The plaintext token is returned exactly once and never stored.
    /// Only the SHA-256 hash is persisted in the state store.
    pub fn create_api_key(
        &self,
        name: &str,
        expires_at_ms: Option<u64>,
    ) -> crate::error::StorageResult<(String, String)> {
        use auth::{hash_key, now_ms, storage_key, ApiKeyEntry};
        use uuid::Uuid;

        let key_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().simple().to_string(); // 32-char hex, opaque to clients
        let hashed = hash_key(&token);

        let entry = ApiKeyEntry {
            key_id: key_id.clone(),
            name: name.to_string(),
            hashed_key: hashed,
            created_at_ms: now_ms(),
            expires_at_ms,
        };
        let value = serde_json::to_vec(&entry)
            .map_err(|e| crate::error::StorageError::Serialization(e.to_string()))?;
        self.storage.put_state(&storage_key(&key_id), &value)?;

        tracing::info!(key_id = %key_id, name = %name, "api key created");
        Ok((key_id, token))
    }

    /// Revoke an API key by its key ID.
    ///
    /// Returns `true` if the key was found and deleted, `false` if not found.
    pub fn revoke_api_key(&self, key_id: &str) -> crate::error::StorageResult<bool> {
        use auth::storage_key;

        let key = storage_key(key_id);
        if self.storage.get_state(&key)?.is_none() {
            return Ok(false);
        }
        self.storage.delete_state(&key)?;
        tracing::info!(key_id = %key_id, "api key revoked");
        Ok(true)
    }

    /// List all API keys (metadata only — no plaintext tokens).
    ///
    /// Expired keys are included so operators can identify and clean them up.
    pub fn list_api_keys(&self) -> crate::error::StorageResult<Vec<auth::ApiKeyEntry>> {
        use auth::{ApiKeyEntry, API_KEY_PREFIX};

        let entries = self
            .storage
            .list_state_by_prefix(API_KEY_PREFIX, usize::MAX)?;
        let mut result = Vec::with_capacity(entries.len());
        for (_, value) in entries {
            match serde_json::from_slice::<ApiKeyEntry>(&value) {
                Ok(entry) => result.push(entry),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to deserialize api key entry — skipping");
                }
            }
        }
        Ok(result)
    }

    /// Validate a plaintext API key token.
    ///
    /// Returns `true` if the token matches a stored, non-expired key.
    /// This is called on every authenticated RPC — designed to be fast
    /// (SHA-256 hash + prefix scan over a small number of keys).
    pub fn validate_api_key(&self, token: &str) -> crate::error::StorageResult<bool> {
        use auth::{hash_key, now_ms, ApiKeyEntry, API_KEY_PREFIX};

        let hashed = hash_key(token);
        let now = now_ms();
        let entries = self
            .storage
            .list_state_by_prefix(API_KEY_PREFIX, usize::MAX)?;
        for (_, value) in entries {
            if let Ok(entry) = serde_json::from_slice::<ApiKeyEntry>(&value) {
                if entry.hashed_key == hashed {
                    // Check expiry.
                    if let Some(exp) = entry.expires_at_ms {
                        if now > exp {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        // If shutdown wasn't called explicitly, attempt to stop the scheduler
        if self.scheduler_thread.is_some() {
            let _ = self.command_tx.send(SchedulerCommand::Shutdown);
            if let Some(handle) = self.scheduler_thread.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::storage::RocksDbEngine;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_broker() -> (Broker, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbEngine::open(dir.path()).unwrap());
        let config = BrokerConfig {
            scheduler: config::SchedulerConfig {
                command_channel_capacity: 100,
                idle_timeout_ms: 10,
                quantum: 1000,
            },
            ..Default::default()
        };
        let broker = Broker::new(config, storage).unwrap();
        (broker, dir)
    }

    #[test]
    fn broker_starts_and_shuts_down() {
        let (broker, _dir) = test_broker();
        broker.shutdown().unwrap();
    }

    #[test]
    fn broker_processes_enqueue_command() {
        let (broker, _dir) = test_broker();

        // Create the queue first
        let (create_tx, create_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::CreateQueue {
                name: "test-queue".to_string(),
                config: crate::queue::QueueConfig::new("test-queue".to_string()),
                reply: create_tx,
            })
            .unwrap();
        create_rx.blocking_recv().unwrap().unwrap();

        let msg = Message {
            id: Uuid::now_v7(),
            queue_id: "test-queue".to_string(),
            headers: HashMap::new(),
            payload: vec![42],
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 1_000_000_000,
            leased_at: None,
        };
        let msg_id = msg.id;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();

        // Give the scheduler thread time to process
        let result = reply_rx.blocking_recv().unwrap().unwrap();
        assert_eq!(result, msg_id);

        broker.shutdown().unwrap();
    }

    #[test]
    fn broker_drop_stops_scheduler() {
        let (broker, _dir) = test_broker();
        drop(broker);
        // If we get here without hanging, the Drop impl worked
    }
}
