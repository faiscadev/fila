pub mod auth;
pub mod command;
pub mod config;
pub mod drr;
pub mod metrics;
pub mod router;
mod scheduler;
mod shard_router;
pub mod stats;
pub mod throttle;

use std::sync::Arc;
use std::thread;

use tracing::info;

use crate::error::{BrokerError, BrokerResult};
use crate::storage::StorageEngine;

pub use command::{QueueSummary, ReadyMessage, SchedulerCommand};
pub use config::{
    AuthConfig, BrokerConfig, FibpConfig, GuiConfig, RocksDbConfig, ServerConfig, StorageConfig,
    TlsParams,
};

use scheduler::Scheduler;
use shard_router::ShardRouter;

/// Internal routing decision for a scheduler command.
enum RouteKey {
    /// Route to the shard owning this queue.
    Queue(String),
    /// Route to shard 0 (global commands).
    Shard0,
    /// Broadcast to all shards (fire-and-forget).
    Broadcast,
    /// Broadcast ListQueues and aggregate results.
    ListQueues,
    /// Broadcast shutdown to all shards.
    Shutdown,
}

/// The broker owns the scheduler threads and routes commands through
/// the `ShardRouter`. IO threads (gRPC handlers) send commands through
/// `send_command()`, which routes each command to the correct shard
/// based on queue name.
///
/// When `shard_count` is 1 (the default), behavior is identical to the
/// pre-sharding single-scheduler architecture.
pub struct Broker {
    router: ShardRouter,
    scheduler_threads: Vec<thread::JoinHandle<()>>,
    storage: Arc<dyn StorageEngine>,
    /// Whether API key authentication is enabled. Set from `BrokerConfig.auth`.
    pub auth_enabled: bool,
    /// Bootstrap API key accepted without a storage lookup.
    /// `None` when not configured. Matched by plain string equality.
    bootstrap_api_key: Option<String>,
}

impl Broker {
    /// Create a new broker, spawning scheduler threads on dedicated OS threads.
    ///
    /// When `shard_count` is 1 (default), a single scheduler thread is spawned.
    /// For `shard_count > 1`, N scheduler threads are spawned, each processing
    /// a subset of queues determined by consistent hashing.
    #[tracing::instrument(skip_all, fields(listen_addr = %config.fibp.listen_addr))]
    pub fn new(config: BrokerConfig, storage: Arc<dyn StorageEngine>) -> BrokerResult<Self> {
        let shard_count = config.scheduler.shard_count.max(1);
        let scheduler_config = config.scheduler.clone();
        let lua_config = config.lua.clone();
        let cluster_node_id = if config.cluster.enabled {
            config.cluster.node_id
        } else {
            0
        };

        let mut senders = Vec::with_capacity(shard_count);
        let mut threads = Vec::with_capacity(shard_count);

        for shard_idx in 0..shard_count {
            let (tx, rx) = crossbeam_channel::bounded::<SchedulerCommand>(
                scheduler_config.command_channel_capacity,
            );

            let sc = scheduler_config.clone();
            let lc = lua_config.clone();
            let storage_clone = Arc::clone(&storage);
            let thread_name = if shard_count == 1 {
                "fila-scheduler".to_string()
            } else {
                format!("fila-scheduler-{shard_idx}")
            };

            let handle = thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    let mut scheduler =
                        Scheduler::new(storage_clone, rx, &sc, &lc, cluster_node_id);
                    scheduler.run();
                })
                .map_err(|e| BrokerError::SchedulerSpawn(e.to_string()))?;

            senders.push(tx);
            threads.push(handle);
        }

        info!(shard_count, "broker started");

        Ok(Self {
            router: ShardRouter::new(senders),
            scheduler_threads: threads,
            storage,
            auth_enabled: config.auth.is_some(),
            bootstrap_api_key: config.auth.map(|a| a.bootstrap_apikey),
        })
    }

    /// Send a command to the appropriate scheduler shard.
    ///
    /// Commands are routed based on their queue affinity:
    /// - Queue-targeted commands (Enqueue, Ack, Nack, CreateQueue, etc.)
    ///   are hashed by queue name to a specific shard.
    /// - Global commands (SetConfig, GetConfig, ListConfig, SetThrottleRate,
    ///   RemoveThrottleRate) are sent to shard 0.
    /// - ListQueues is broadcast to all shards and results are aggregated.
    /// - Shutdown is broadcast to all shards.
    ///
    /// Returns an error if the target channel is full or disconnected.
    #[tracing::instrument(skip_all)]
    pub fn send_command(&self, cmd: SchedulerCommand) -> BrokerResult<()> {
        // Extract the routing queue name before moving the command.
        let route_key = Self::extract_route_key(&cmd);
        match route_key {
            RouteKey::Queue(qid) => self.router.send_to_queue(&qid, cmd),
            RouteKey::Shard0 => self.router.send_to_shard0(cmd),
            RouteKey::Broadcast => self.router.broadcast_fire_and_forget(cmd),
            RouteKey::ListQueues => {
                // Destructure to extract the reply channel.
                if let SchedulerCommand::ListQueues { reply } = cmd {
                    self.router.send_list_queues(reply)
                } else {
                    unreachable!()
                }
            }
            RouteKey::Shutdown => {
                self.router.broadcast_shutdown();
                Ok(())
            }
        }
    }

    /// Determine the routing key for a command without consuming it.
    fn extract_route_key(cmd: &SchedulerCommand) -> RouteKey {
        match cmd {
            SchedulerCommand::Enqueue { messages, .. } => {
                // Route by the first message's queue. In practice, batches target
                // one queue. Multi-queue batches are split at the gRPC layer.
                RouteKey::Queue(
                    messages
                        .first()
                        .map(|m| m.queue_id.clone())
                        .unwrap_or_default(),
                )
            }
            SchedulerCommand::Ack { queue_id, .. }
            | SchedulerCommand::Nack { queue_id, .. }
            | SchedulerCommand::RegisterConsumer { queue_id, .. }
            | SchedulerCommand::DeleteQueue { queue_id, .. }
            | SchedulerCommand::GetStats { queue_id, .. }
            | SchedulerCommand::RecoverQueue { queue_id, .. }
            | SchedulerCommand::DropQueueConsumers { queue_id, .. } => {
                RouteKey::Queue(queue_id.clone())
            }
            SchedulerCommand::CreateQueue { name, .. } => RouteKey::Queue(name.clone()),
            SchedulerCommand::Redrive { dlq_queue_id, .. } => RouteKey::Queue(dlq_queue_id.clone()),

            // UnregisterConsumer has no queue_id — broadcast to all shards.
            SchedulerCommand::UnregisterConsumer { .. } => RouteKey::Broadcast,

            // Cross-shard aggregation
            SchedulerCommand::ListQueues { .. } => RouteKey::ListQueues,

            // Global commands: route to shard 0
            SchedulerCommand::SetThrottleRate { .. }
            | SchedulerCommand::RemoveThrottleRate { .. }
            | SchedulerCommand::SetConfig { .. }
            | SchedulerCommand::GetConfig { .. }
            | SchedulerCommand::ListConfig { .. } => RouteKey::Shard0,

            SchedulerCommand::Shutdown => RouteKey::Shutdown,
        }
    }

    /// Initiate graceful shutdown: send the shutdown command to all shards
    /// and wait for all scheduler threads to finish.
    #[tracing::instrument(skip_all)]
    pub fn shutdown(mut self) -> BrokerResult<()> {
        info!("initiating broker shutdown");

        // Send shutdown command to all shards (ignore error if channels already closed)
        self.router.broadcast_shutdown();

        // Wait for all scheduler threads to finish
        for handle in self.scheduler_threads.drain(..) {
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
    /// When `is_superadmin` is true, the key bypasses all ACL checks.
    pub fn create_api_key(
        &self,
        name: &str,
        expires_at_ms: Option<u64>,
        is_superadmin: bool,
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
            permissions: vec![],
            is_superadmin,
        };
        let value = serde_json::to_vec(&entry)
            .map_err(|e| crate::error::StorageError::Serialization(e.to_string()))?;
        self.storage.put_state(&storage_key(&key_id), &value)?;

        tracing::info!(key_id = %key_id, name = %name, is_superadmin, "api key created");
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
    /// Returns `Some(CallerKey::Bootstrap)` for the bootstrap credential,
    /// `Some(CallerKey::Key(key_id))` for a valid stored key, or `None` if
    /// the token is invalid, not found, or expired.
    pub fn validate_api_key(
        &self,
        token: &str,
    ) -> crate::error::StorageResult<Option<auth::CallerKey>> {
        use auth::{hash_key, now_ms, ApiKeyEntry, CallerKey, API_KEY_PREFIX};

        // Bootstrap key short-circuits the storage lookup.
        if let Some(ref bootstrap) = self.bootstrap_api_key {
            if token == bootstrap.as_str() {
                return Ok(Some(CallerKey::Bootstrap));
            }
        }

        let hashed = hash_key(token);
        let now = now_ms();
        let entries = self
            .storage
            .list_state_by_prefix(API_KEY_PREFIX, usize::MAX)?;
        for (storage_key, value) in entries {
            let entry = match serde_json::from_slice::<ApiKeyEntry>(&value) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(storage_key = %storage_key, error = %e, "failed to deserialize api key entry during validation — skipping");
                    continue;
                }
            };
            if entry.hashed_key == hashed {
                if let Some(exp) = entry.expires_at_ms {
                    if now > exp {
                        return Ok(None);
                    }
                }
                return Ok(Some(CallerKey::Key(entry.key_id)));
            }
        }
        Ok(None)
    }

    /// Check whether `caller` has the requested permission on `queue`.
    ///
    /// Returns `Ok(true)` if permitted, `Ok(false)` if not found or not granted,
    /// `Err` only on storage failure. When auth is disabled this method is never called.
    pub fn check_permission(
        &self,
        caller: &auth::CallerKey,
        perm: auth::Permission,
        queue: &str,
    ) -> crate::error::StorageResult<bool> {
        use auth::{has_permission, ApiKeyEntry, CallerKey};

        match caller {
            CallerKey::Bootstrap => Ok(true),
            CallerKey::Key(key_id) => {
                let storage_key = auth::storage_key(key_id);
                let raw = match self.storage.get_state(&storage_key)? {
                    Some(v) => v,
                    None => return Ok(false),
                };
                let entry = match serde_json::from_slice::<ApiKeyEntry>(&raw) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(key_id = %key_id, error = %e, "failed to deserialize api key entry for permission check");
                        return Ok(false);
                    }
                };
                Ok(has_permission(&entry, perm, queue))
            }
        }
    }

    /// Set ACL permissions for a key. Replaces any existing permissions.
    pub fn set_acl(
        &self,
        key_id: &str,
        permissions: Vec<(String, String)>,
    ) -> crate::error::StorageResult<bool> {
        use auth::ApiKeyEntry;

        let sk = auth::storage_key(key_id);
        let raw = match self.storage.get_state(&sk)? {
            Some(v) => v,
            None => return Ok(false),
        };
        let mut entry = match serde_json::from_slice::<ApiKeyEntry>(&raw) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(key_id = %key_id, error = %e, "failed to deserialize api key entry for set_acl");
                return Ok(false);
            }
        };
        entry.permissions = permissions;
        let value = serde_json::to_vec(&entry)
            .map_err(|e| crate::error::StorageError::Serialization(e.to_string()))?;
        self.storage.put_state(&sk, &value)?;
        tracing::info!(key_id = %key_id, "api key acl updated");
        Ok(true)
    }

    /// Returns `true` if `caller` has any admin permission (is_superadmin or admin:<queue>).
    pub fn has_any_admin(&self, caller: &auth::CallerKey) -> crate::error::StorageResult<bool> {
        use auth::{has_any_admin_permission, ApiKeyEntry, CallerKey};

        match caller {
            CallerKey::Bootstrap => Ok(true),
            CallerKey::Key(key_id) => {
                let sk = auth::storage_key(key_id);
                let raw = match self.storage.get_state(&sk)? {
                    Some(v) => v,
                    None => return Ok(false),
                };
                match serde_json::from_slice::<ApiKeyEntry>(&raw) {
                    Ok(entry) => Ok(has_any_admin_permission(&entry)),
                    Err(e) => {
                        tracing::warn!(key_id = %key_id, error = %e, "failed to deserialize api key entry for has_any_admin");
                        Ok(false)
                    }
                }
            }
        }
    }

    /// Returns `true` if `caller` is a superadmin.
    pub fn is_superadmin(&self, caller: &auth::CallerKey) -> crate::error::StorageResult<bool> {
        use auth::{ApiKeyEntry, CallerKey};

        match caller {
            CallerKey::Bootstrap => Ok(true),
            CallerKey::Key(key_id) => {
                let sk = auth::storage_key(key_id);
                let raw = match self.storage.get_state(&sk)? {
                    Some(v) => v,
                    None => return Ok(false),
                };
                match serde_json::from_slice::<ApiKeyEntry>(&raw) {
                    Ok(entry) => Ok(entry.is_superadmin),
                    Err(e) => {
                        tracing::warn!(key_id = %key_id, error = %e, "failed to deserialize api key entry for is_superadmin");
                        Ok(false)
                    }
                }
            }
        }
    }

    /// Returns `true` if `caller` has admin permission covering `queue`.
    pub fn caller_has_queue_admin(
        &self,
        caller: &auth::CallerKey,
        queue: &str,
    ) -> crate::error::StorageResult<bool> {
        use auth::{caller_has_queue_admin, ApiKeyEntry, CallerKey};

        match caller {
            CallerKey::Bootstrap => Ok(true),
            CallerKey::Key(key_id) => {
                let sk = auth::storage_key(key_id);
                let raw = match self.storage.get_state(&sk)? {
                    Some(v) => v,
                    None => return Ok(false),
                };
                match serde_json::from_slice::<ApiKeyEntry>(&raw) {
                    Ok(entry) => Ok(caller_has_queue_admin(&entry, queue)),
                    Err(e) => {
                        tracing::warn!(key_id = %key_id, error = %e, "failed to deserialize api key entry for caller_has_queue_admin");
                        Ok(false)
                    }
                }
            }
        }
    }

    /// Returns `true` if `caller` can grant all of the given `permissions`.
    ///
    /// Each `(kind, pattern)` in `permissions` must be within the caller's admin scope.
    pub fn caller_can_grant_all(
        &self,
        caller: &auth::CallerKey,
        permissions: &[(String, String)],
    ) -> crate::error::StorageResult<bool> {
        use auth::{caller_can_grant, ApiKeyEntry, CallerKey};

        match caller {
            CallerKey::Bootstrap => Ok(true),
            CallerKey::Key(key_id) => {
                let sk = auth::storage_key(key_id);
                let raw = match self.storage.get_state(&sk)? {
                    Some(v) => v,
                    None => return Ok(false),
                };
                let entry = match serde_json::from_slice::<ApiKeyEntry>(&raw) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(key_id = %key_id, error = %e, "failed to deserialize api key entry for caller_can_grant_all");
                        return Ok(false);
                    }
                };
                Ok(permissions
                    .iter()
                    .all(|(kind, pattern)| caller_can_grant(&entry, kind, pattern)))
            }
        }
    }

    /// Get the ACL permissions for a key.
    pub fn get_acl(&self, key_id: &str) -> crate::error::StorageResult<Option<auth::ApiKeyEntry>> {
        use auth::ApiKeyEntry;

        let sk = auth::storage_key(key_id);
        let raw = match self.storage.get_state(&sk)? {
            Some(v) => v,
            None => return Ok(None),
        };
        match serde_json::from_slice::<ApiKeyEntry>(&raw) {
            Ok(e) => Ok(Some(e)),
            Err(e) => {
                tracing::warn!(key_id = %key_id, error = %e, "failed to deserialize api key entry for get_acl");
                Ok(None)
            }
        }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        // If shutdown wasn't called explicitly, attempt to stop all schedulers
        if !self.scheduler_threads.is_empty() {
            self.router.broadcast_shutdown();
            for handle in self.scheduler_threads.drain(..) {
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
                ..Default::default()
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
            payload: vec![42].into(),
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
                messages: vec![msg],
                reply: reply_tx,
            })
            .unwrap();

        // Give the scheduler thread time to process
        let result = reply_rx
            .blocking_recv()
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(result, msg_id);

        broker.shutdown().unwrap();
    }

    #[test]
    fn broker_drop_stops_scheduler() {
        let (broker, _dir) = test_broker();
        drop(broker);
        // If we get here without hanging, the Drop impl worked
    }

    fn broker_with_auth(bootstrap_key: &str) -> (Broker, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbEngine::open(dir.path()).unwrap());
        let config = BrokerConfig {
            scheduler: config::SchedulerConfig {
                command_channel_capacity: 100,
                idle_timeout_ms: 10,
                quantum: 1000,
                ..Default::default()
            },
            auth: Some(config::AuthConfig {
                bootstrap_apikey: bootstrap_key.to_string(),
            }),
            ..Default::default()
        };
        let broker = Broker::new(config, storage).unwrap();
        (broker, dir)
    }

    #[test]
    fn bootstrap_api_key_is_accepted() {
        let (broker, _dir) = broker_with_auth("my-bootstrap-key");
        assert!(broker
            .validate_api_key("my-bootstrap-key")
            .unwrap()
            .is_some());
    }

    #[test]
    fn bootstrap_api_key_returns_bootstrap_variant() {
        let (broker, _dir) = broker_with_auth("my-bootstrap-key");
        assert_eq!(
            broker.validate_api_key("my-bootstrap-key").unwrap(),
            Some(auth::CallerKey::Bootstrap)
        );
    }

    #[test]
    fn bootstrap_api_key_wrong_value_rejected() {
        let (broker, _dir) = broker_with_auth("my-bootstrap-key");
        assert!(broker.validate_api_key("wrong-key").unwrap().is_none());
    }

    #[test]
    fn non_bootstrap_key_falls_through_to_storage() {
        let (broker, _dir) = broker_with_auth("my-bootstrap-key");
        // "other-key" is not the bootstrap key and no stored keys exist → rejected
        assert!(broker.validate_api_key("other-key").unwrap().is_none());
    }

    // ── has_any_admin / is_superadmin / caller_has_queue_admin / caller_can_grant_all ──

    #[test]
    fn bootstrap_has_any_admin() {
        let (broker, _dir) = broker_with_auth("boot");
        assert!(broker.has_any_admin(&auth::CallerKey::Bootstrap).unwrap());
    }

    #[test]
    fn bootstrap_is_superadmin() {
        let (broker, _dir) = broker_with_auth("boot");
        assert!(broker.is_superadmin(&auth::CallerKey::Bootstrap).unwrap());
    }

    #[test]
    fn superadmin_key_has_any_admin_and_queue_admin() {
        let (broker, _dir) = broker_with_auth("boot");
        let (key_id, _) = broker.create_api_key("sa", None, true).unwrap();
        let caller = auth::CallerKey::Key(key_id);
        assert!(broker.has_any_admin(&caller).unwrap());
        assert!(broker.is_superadmin(&caller).unwrap());
        assert!(broker.caller_has_queue_admin(&caller, "any-queue").unwrap());
    }

    #[test]
    fn admin_star_key_has_any_admin_and_covers_all_queues() {
        let (broker, _dir) = broker_with_auth("boot");
        let (key_id, _) = broker.create_api_key("admin-star", None, false).unwrap();
        broker
            .set_acl(&key_id, vec![("admin".into(), "*".into())])
            .unwrap();
        let caller = auth::CallerKey::Key(key_id);
        assert!(broker.has_any_admin(&caller).unwrap());
        assert!(!broker.is_superadmin(&caller).unwrap());
        assert!(broker.caller_has_queue_admin(&caller, "orders").unwrap());
        assert!(broker.caller_has_queue_admin(&caller, "payments").unwrap());
    }

    #[test]
    fn admin_queue_key_covers_only_matching_queue() {
        let (broker, _dir) = broker_with_auth("boot");
        let (key_id, _) = broker.create_api_key("admin-orders", None, false).unwrap();
        broker
            .set_acl(&key_id, vec![("admin".into(), "orders.*".into())])
            .unwrap();
        let caller = auth::CallerKey::Key(key_id);
        assert!(broker.has_any_admin(&caller).unwrap());
        assert!(broker.caller_has_queue_admin(&caller, "orders.us").unwrap());
        assert!(!broker.caller_has_queue_admin(&caller, "payments").unwrap());
        assert!(!broker.caller_has_queue_admin(&caller, "orders").unwrap());
    }

    #[test]
    fn produce_only_key_has_no_admin() {
        let (broker, _dir) = broker_with_auth("boot");
        let (key_id, _) = broker.create_api_key("producer", None, false).unwrap();
        broker
            .set_acl(&key_id, vec![("produce".into(), "*".into())])
            .unwrap();
        let caller = auth::CallerKey::Key(key_id);
        assert!(!broker.has_any_admin(&caller).unwrap());
        assert!(!broker.caller_has_queue_admin(&caller, "orders").unwrap());
    }

    #[test]
    fn caller_can_grant_all_within_scope() {
        let (broker, _dir) = broker_with_auth("boot");
        let (key_id, _) = broker.create_api_key("admin-orders", None, false).unwrap();
        broker
            .set_acl(&key_id, vec![("admin".into(), "orders.*".into())])
            .unwrap();
        let caller = auth::CallerKey::Key(key_id);
        let in_scope = vec![
            ("produce".into(), "orders.us".into()),
            ("consume".into(), "orders.eu".into()),
            ("admin".into(), "orders.us".into()),
        ];
        assert!(broker.caller_can_grant_all(&caller, &in_scope).unwrap());
        let out_of_scope = vec![("produce".into(), "payments".into())];
        assert!(!broker.caller_can_grant_all(&caller, &out_of_scope).unwrap());
    }

    #[test]
    fn admin_star_can_grant_any_permission_pattern() {
        let (broker, _dir) = broker_with_auth("boot");
        let (key_id, _) = broker.create_api_key("admin-star", None, false).unwrap();
        broker
            .set_acl(&key_id, vec![("admin".into(), "*".into())])
            .unwrap();
        let caller = auth::CallerKey::Key(key_id);
        // admin:* can grant any permission pattern; superadmin creation is a
        // separate check at the service layer (is_superadmin flag, not a permission)
        let perms = vec![("produce".into(), "*".into())];
        assert!(broker.caller_can_grant_all(&caller, &perms).unwrap());
    }

    // ── Scheduler sharding tests ──

    fn sharded_broker(shard_count: usize) -> (Broker, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbEngine::open(dir.path()).unwrap());
        let config = BrokerConfig {
            scheduler: config::SchedulerConfig {
                command_channel_capacity: 100,
                idle_timeout_ms: 10,
                quantum: 1000,
                shard_count,
                ..Default::default()
            },
            ..Default::default()
        };
        let broker = Broker::new(config, storage).unwrap();
        (broker, dir)
    }

    #[test]
    fn sharded_broker_starts_and_shuts_down() {
        let (broker, _dir) = sharded_broker(4);
        broker.shutdown().unwrap();
    }

    #[test]
    fn sharded_broker_drop_stops_all_schedulers() {
        let (broker, _dir) = sharded_broker(4);
        drop(broker);
        // If we get here without hanging, all scheduler threads stopped
    }

    #[test]
    fn sharded_broker_create_and_enqueue() {
        let (broker, _dir) = sharded_broker(2);

        // Create 4 queues (will be distributed across 2 shards)
        for name in &["alpha", "beta", "gamma", "delta"] {
            let (tx, rx) = tokio::sync::oneshot::channel();
            broker
                .send_command(SchedulerCommand::CreateQueue {
                    name: name.to_string(),
                    config: crate::queue::QueueConfig::new(name.to_string()),
                    reply: tx,
                })
                .unwrap();
            rx.blocking_recv().unwrap().unwrap();
        }

        // Enqueue a message to each queue
        for name in &["alpha", "beta", "gamma", "delta"] {
            let msg = Message {
                id: Uuid::now_v7(),
                queue_id: name.to_string(),
                headers: HashMap::new(),
                payload: vec![42].into(),
                fairness_key: "default".to_string(),
                weight: 1,
                throttle_keys: vec![],
                attempt_count: 0,
                enqueued_at: 1_000_000_000,
                leased_at: None,
            };
            let (tx, rx) = tokio::sync::oneshot::channel();
            broker
                .send_command(SchedulerCommand::Enqueue {
                    messages: vec![msg],
                    reply: tx,
                })
                .unwrap();
            rx.blocking_recv()
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
                .unwrap();
        }

        // ListQueues should aggregate from both shards
        let (tx, rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::ListQueues { reply: tx })
            .unwrap();
        let summaries = rx.blocking_recv().unwrap().unwrap();
        // 4 queues + 4 auto-created DLQs = 8
        assert_eq!(summaries.len(), 8);

        // Verify each original queue has depth=1
        for name in &["alpha", "beta", "gamma", "delta"] {
            let s = summaries.iter().find(|s| s.name == *name).unwrap();
            assert_eq!(s.depth, 1, "queue {name} should have depth 1");
        }

        broker.shutdown().unwrap();
    }

    #[test]
    fn sharded_broker_list_queues_deduplicates() {
        let (broker, _dir) = sharded_broker(4);

        // Create 2 queues
        for name in &["q1", "q2"] {
            let (tx, rx) = tokio::sync::oneshot::channel();
            broker
                .send_command(SchedulerCommand::CreateQueue {
                    name: name.to_string(),
                    config: crate::queue::QueueConfig::new(name.to_string()),
                    reply: tx,
                })
                .unwrap();
            rx.blocking_recv().unwrap().unwrap();
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::ListQueues { reply: tx })
            .unwrap();
        let summaries = rx.blocking_recv().unwrap().unwrap();
        // 2 queues + 2 DLQs = 4, no duplicates
        assert_eq!(summaries.len(), 4);
        let names: Vec<&str> = summaries.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"q1"));
        assert!(names.contains(&"q2"));
        assert!(names.contains(&"q1.dlq"));
        assert!(names.contains(&"q2.dlq"));
    }

    #[test]
    fn sharded_broker_get_stats_routes_to_correct_shard() {
        let (broker, _dir) = sharded_broker(2);

        let (tx, rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::CreateQueue {
                name: "stats-q".to_string(),
                config: crate::queue::QueueConfig::new("stats-q".to_string()),
                reply: tx,
            })
            .unwrap();
        rx.blocking_recv().unwrap().unwrap();

        // Enqueue 3 messages
        for _ in 0..3 {
            let msg = Message {
                id: Uuid::now_v7(),
                queue_id: "stats-q".to_string(),
                headers: HashMap::new(),
                payload: vec![1].into(),
                fairness_key: "default".to_string(),
                weight: 1,
                throttle_keys: vec![],
                attempt_count: 0,
                enqueued_at: 1_000_000_000,
                leased_at: None,
            };
            let (tx, rx) = tokio::sync::oneshot::channel();
            broker
                .send_command(SchedulerCommand::Enqueue {
                    messages: vec![msg],
                    reply: tx,
                })
                .unwrap();
            rx.blocking_recv()
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
                .unwrap();
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::GetStats {
                queue_id: "stats-q".to_string(),
                reply: tx,
            })
            .unwrap();
        let stats = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(stats.depth, 3);
    }
}
