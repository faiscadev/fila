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
pub use config::{
    AuthConfig, BrokerConfig, GrpcConfig, GuiConfig, RocksDbConfig, StorageConfig, TlsParams,
};

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
    /// Bootstrap API key accepted without a storage lookup.
    /// `None` when not configured. Matched by plain string equality.
    bootstrap_api_key: Option<String>,
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
            bootstrap_api_key: config.auth.map(|a| a.bootstrap_apikey),
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
}
