use std::sync::Arc;

use fila_core::{
    Broker, ClusterHandle, ClusterRequest, ClusterResponse, ClusterWriteError, QueueConfig,
    SchedulerCommand,
};
use fila_proto::fila_admin_server::FilaAdmin;
use fila_proto::{
    AclPermission, ApiKeyInfo, ConfigEntry, CreateApiKeyRequest, CreateApiKeyResponse,
    CreateQueueRequest, CreateQueueResponse, DeleteQueueRequest, DeleteQueueResponse,
    GetAclRequest, GetAclResponse, GetConfigRequest, GetConfigResponse, GetStatsRequest,
    GetStatsResponse, ListApiKeysRequest, ListApiKeysResponse, ListConfigRequest,
    ListConfigResponse, ListQueuesRequest, ListQueuesResponse, PerFairnessKeyStats,
    PerThrottleKeyStats, QueueInfo, RedriveRequest, RedriveResponse, RevokeApiKeyRequest,
    RevokeApiKeyResponse, SetAclRequest, SetAclResponse, SetConfigRequest, SetConfigResponse,
};
use tonic::{Request, Response, Status};
use tracing::instrument;

use crate::auth::ValidatedKeyId;
use crate::error::IntoStatus;

/// Map cluster write errors to appropriate gRPC status codes.
fn cluster_write_err_to_status(err: ClusterWriteError) -> Status {
    match err {
        ClusterWriteError::QueueGroupNotFound => Status::not_found("queue raft group not found"),
        ClusterWriteError::NoLeader => Status::unavailable("no leader available"),
        ClusterWriteError::Raft(e) => Status::internal(format!("raft error: {e}")),
        ClusterWriteError::Forward(e) => Status::unavailable(format!("forward error: {e}")),
    }
}

/// gRPC admin service implementation. Wraps a Broker to send commands
/// to the scheduler thread.
pub struct AdminService {
    broker: Arc<Broker>,
    cluster: Option<Arc<ClusterHandle>>,
}

impl AdminService {
    const MAX_CONFIG_KEY_LEN: usize = 256;
    const MAX_CONFIG_VALUE_LEN: usize = 1024;

    pub fn new(broker: Arc<Broker>, cluster: Option<Arc<ClusterHandle>>) -> Self {
        Self { broker, cluster }
    }

    /// Check that the request's validated key has `admin:*` permission.
    ///
    /// Returns `Ok(())` when auth is disabled (no `ValidatedKeyId` extension) or
    /// when the key has admin access. Returns `PERMISSION_DENIED` otherwise.
    fn check_admin<T>(&self, request: &tonic::Request<T>) -> Result<(), Status> {
        let key_id = match request.extensions().get::<ValidatedKeyId>() {
            Some(k) => &k.0,
            None => return Ok(()), // auth disabled
        };
        let permitted = self
            .broker
            .check_permission(key_id, fila_core::broker::auth::Permission::Admin, "*")
            .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
        if permitted {
            Ok(())
        } else {
            Err(Status::permission_denied(
                "key does not have admin permission",
            ))
        }
    }
}

#[tonic::async_trait]
impl FilaAdmin for AdminService {
    #[instrument(skip(self), fields(queue_id))]
    async fn create_queue(
        &self,
        request: Request<CreateQueueRequest>,
    ) -> Result<Response<CreateQueueResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();

        if req.name.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }
        tracing::Span::current().record("queue_id", req.name.as_str());

        let proto_config = req.config.unwrap_or_default();

        let visibility_timeout_ms = match proto_config.visibility_timeout_ms {
            0 => QueueConfig::DEFAULT_VISIBILITY_TIMEOUT_MS, // proto3: 0 means unset
            v if v < 1_000 => {
                return Err(Status::invalid_argument(
                    "visibility_timeout_ms must be at least 1000 (1 second)",
                ));
            }
            v => v,
        };

        let config = QueueConfig {
            name: req.name.clone(),
            on_enqueue_script: if proto_config.on_enqueue_script.is_empty() {
                None
            } else {
                Some(proto_config.on_enqueue_script)
            },
            on_failure_script: if proto_config.on_failure_script.is_empty() {
                None
            } else {
                Some(proto_config.on_failure_script)
            },
            visibility_timeout_ms,
            dlq_queue_id: None,
            lua_timeout_ms: None,
            lua_memory_limit_bytes: None,
        };

        if let Some(ref cluster) = self.cluster {
            // Cluster mode: submit CreateQueueGroup to meta Raft.
            // The meta state machine event handler will create the queue
            // in the local scheduler and start the queue's Raft group on
            // all nodes.
            let (members, _member_addrs) = cluster.meta_members();
            let resp = cluster
                .write_to_meta(ClusterRequest::CreateQueueGroup {
                    queue_id: req.name.clone(),
                    members,
                    config,
                })
                .await
                .map_err(cluster_write_err_to_status)?;

            match resp {
                ClusterResponse::CreateQueueGroup { queue_id } => {
                    Ok(Response::new(CreateQueueResponse { queue_id }))
                }
                ClusterResponse::Error { message } => Err(Status::internal(message)),
                _ => Err(Status::internal("unexpected cluster response")),
            }
        } else {
            // Single-node mode: direct to scheduler.
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            self.broker
                .send_command(SchedulerCommand::CreateQueue {
                    name: req.name,
                    config,
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            let queue_id = reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?
                .map_err(IntoStatus::into_status)?;

            Ok(Response::new(CreateQueueResponse { queue_id }))
        }
    }

    #[instrument(skip(self), fields(queue_id))]
    async fn delete_queue(
        &self,
        request: Request<DeleteQueueRequest>,
    ) -> Result<Response<DeleteQueueResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }
        tracing::Span::current().record("queue_id", req.queue.as_str());

        if let Some(ref cluster) = self.cluster {
            // Cluster mode: submit DeleteQueueGroup to meta Raft.
            let resp = cluster
                .write_to_meta(ClusterRequest::DeleteQueueGroup {
                    queue_id: req.queue,
                })
                .await
                .map_err(cluster_write_err_to_status)?;

            match resp {
                ClusterResponse::DeleteQueueGroup => Ok(Response::new(DeleteQueueResponse {})),
                ClusterResponse::Error { message } => Err(Status::internal(message)),
                _ => Err(Status::internal("unexpected cluster response")),
            }
        } else {
            // Single-node mode: direct to scheduler.
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            self.broker
                .send_command(SchedulerCommand::DeleteQueue {
                    queue_id: req.queue,
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?
                .map_err(IntoStatus::into_status)?;

            Ok(Response::new(DeleteQueueResponse {}))
        }
    }

    #[instrument(skip(self))]
    async fn set_config(
        &self,
        request: Request<SetConfigRequest>,
    ) -> Result<Response<SetConfigResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();

        if req.key.is_empty() {
            return Err(Status::invalid_argument("config key must not be empty"));
        }
        if req.key.len() > AdminService::MAX_CONFIG_KEY_LEN {
            return Err(Status::invalid_argument(format!(
                "config key must not exceed {} bytes",
                AdminService::MAX_CONFIG_KEY_LEN
            )));
        }
        if req.value.len() > AdminService::MAX_CONFIG_VALUE_LEN {
            return Err(Status::invalid_argument(format!(
                "config value must not exceed {} bytes",
                AdminService::MAX_CONFIG_VALUE_LEN
            )));
        }

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::SetConfig {
                key: req.key,
                value: req.value,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(Response::new(SetConfigResponse {}))
    }

    #[instrument(skip(self))]
    async fn get_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();

        if req.key.is_empty() {
            return Err(Status::invalid_argument("config key must not be empty"));
        }
        if req.key.len() > AdminService::MAX_CONFIG_KEY_LEN {
            return Err(Status::invalid_argument(format!(
                "config key must not exceed {} bytes",
                AdminService::MAX_CONFIG_KEY_LEN
            )));
        }

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::GetConfig {
                key: req.key,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let value = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(Response::new(GetConfigResponse {
            value: value.unwrap_or_default(),
        }))
    }

    #[instrument(skip(self))]
    async fn list_config(
        &self,
        request: Request<ListConfigRequest>,
    ) -> Result<Response<ListConfigResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();

        if req.prefix.len() > AdminService::MAX_CONFIG_KEY_LEN {
            return Err(Status::invalid_argument(format!(
                "config prefix must not exceed {} bytes",
                AdminService::MAX_CONFIG_KEY_LEN
            )));
        }

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::ListConfig {
                prefix: req.prefix,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let entries = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        let total_count = u32::try_from(entries.len()).unwrap_or(u32::MAX);
        let config_entries = entries
            .into_iter()
            .map(|(key, value)| ConfigEntry { key, value })
            .collect();

        Ok(Response::new(ListConfigResponse {
            entries: config_entries,
            total_count,
        }))
    }

    #[instrument(skip(self))]
    async fn get_stats(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<GetStatsResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        let queue_name = req.queue;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::GetStats {
                queue_id: queue_name.clone(),
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let stats = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        let per_key_stats = stats
            .per_key_stats
            .into_iter()
            .map(|s| PerFairnessKeyStats {
                key: s.key,
                pending_count: s.pending_count,
                current_deficit: s.current_deficit,
                weight: s.weight,
            })
            .collect();

        let per_throttle_stats = stats
            .per_throttle_stats
            .into_iter()
            .map(|s| PerThrottleKeyStats {
                key: s.key,
                tokens: s.tokens,
                rate_per_second: s.rate_per_second,
                burst: s.burst,
            })
            .collect();

        // Enrich with cluster info if in cluster mode.
        let (leader_node_id, replication_count) = if let Some(cluster) = &self.cluster {
            let leader = cluster
                .multi_raft
                .get_raft(&queue_name)
                .await
                .map(|raft| {
                    let metrics = raft.metrics().borrow().clone();
                    let leader_id = metrics.current_leader.unwrap_or(0);
                    let voters = metrics.membership_config.membership().voter_ids().count() as u32;
                    (leader_id, voters)
                })
                .unwrap_or((0, 0));
            leader
        } else {
            (0, 0)
        };

        Ok(Response::new(GetStatsResponse {
            depth: stats.depth,
            in_flight: stats.in_flight,
            active_fairness_keys: stats.active_fairness_keys,
            active_consumers: stats.active_consumers,
            quantum: stats.quantum,
            per_key_stats,
            per_throttle_stats,
            leader_node_id,
            replication_count,
        }))
    }

    #[instrument(skip(self))]
    async fn redrive(
        &self,
        request: Request<RedriveRequest>,
    ) -> Result<Response<RedriveResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();

        if req.dlq_queue.is_empty() {
            return Err(Status::invalid_argument("dlq_queue name must not be empty"));
        }

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::Redrive {
                dlq_queue_id: req.dlq_queue,
                count: req.count,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let redriven = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(Response::new(RedriveResponse { redriven }))
    }

    #[instrument(skip(self))]
    async fn list_queues(
        &self,
        request: Request<ListQueuesRequest>,
    ) -> Result<Response<ListQueuesResponse>, Status> {
        self.check_admin(&request)?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::ListQueues { reply: reply_tx })
            .map_err(IntoStatus::into_status)?;

        let summaries = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        let mut queues = Vec::with_capacity(summaries.len());
        for s in summaries {
            let leader_node_id = if let Some(cluster) = &self.cluster {
                cluster
                    .multi_raft
                    .get_raft(&s.name)
                    .await
                    .map(|raft| raft.metrics().borrow().current_leader.unwrap_or(0))
                    .unwrap_or(0)
            } else {
                0
            };
            queues.push(QueueInfo {
                name: s.name,
                depth: s.depth,
                in_flight: s.in_flight,
                active_consumers: s.active_consumers,
                leader_node_id,
            });
        }

        let cluster_node_count = self
            .cluster
            .as_ref()
            .map(|c| {
                let (members, _) = c.meta_members();
                members.len() as u32
            })
            .unwrap_or(0);

        Ok(Response::new(ListQueuesResponse {
            queues,
            cluster_node_count,
        }))
    }

    #[instrument(skip(self))]
    async fn create_api_key(
        &self,
        request: Request<CreateApiKeyRequest>,
    ) -> Result<Response<CreateApiKeyResponse>, Status> {
        let req = request.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("name must not be empty"));
        }
        // `CreateApiKey` bypasses authentication (bootstrap), so we cannot rely on the
        // auth middleware to inject a `ValidatedKeyId`.  To prevent privilege escalation,
        // we block `is_superadmin: true` once an admin-capable key (superadmin or a key
        // with `admin:*` permission) already exists.  Non-admin keys do not block bootstrap,
        // so a deployment that only has produce/consume keys can still create its first
        // superadmin key without being permanently locked out.
        //
        // TOCTOU note: the list-then-write pattern has an inherent race window.  Concurrent
        // bootstrap requests arriving in the same nanosecond could each see an empty admin
        // set and both create superadmin keys.  This is an operator-level operation (CLI /
        // one-time setup), so the practical risk is negligible and does not warrant the
        // complexity of distributed compare-and-swap locking.
        if req.is_superadmin && self.broker.auth_enabled {
            let existing = self
                .broker
                .list_api_keys()
                .map_err(|e| Status::internal(format!("storage error: {e}")))?;
            let has_admin_key = existing.iter().any(|k| {
                k.is_superadmin
                    || k.permissions
                        .iter()
                        .any(|(kind, pattern)| kind == "admin" && pattern == "*")
            });
            if has_admin_key {
                return Err(Status::permission_denied(
                    "superadmin key creation is only permitted when no admin-capable key exists; \
                     use an existing key with admin permission to manage access",
                ));
            }
        }
        let expires_at = if req.expires_at_ms == 0 {
            None
        } else {
            Some(req.expires_at_ms)
        };
        let (key_id, token) = self
            .broker
            .create_api_key(&req.name, expires_at, req.is_superadmin)
            .map_err(|e| Status::internal(format!("storage error: {e}")))?;
        Ok(Response::new(CreateApiKeyResponse {
            key_id,
            key: token,
            is_superadmin: req.is_superadmin,
        }))
    }

    #[instrument(skip(self))]
    async fn revoke_api_key(
        &self,
        request: Request<RevokeApiKeyRequest>,
    ) -> Result<Response<RevokeApiKeyResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();
        if req.key_id.is_empty() {
            return Err(Status::invalid_argument("key_id must not be empty"));
        }
        let found = self
            .broker
            .revoke_api_key(&req.key_id)
            .map_err(|e| Status::internal(format!("storage error: {e}")))?;
        if found {
            Ok(Response::new(RevokeApiKeyResponse {}))
        } else {
            Err(Status::not_found("api key not found"))
        }
    }

    #[instrument(skip(self))]
    async fn list_api_keys(
        &self,
        request: Request<ListApiKeysRequest>,
    ) -> Result<Response<ListApiKeysResponse>, Status> {
        self.check_admin(&request)?;
        let entries = self
            .broker
            .list_api_keys()
            .map_err(|e| Status::internal(format!("storage error: {e}")))?;
        let keys = entries
            .into_iter()
            .map(|e| ApiKeyInfo {
                key_id: e.key_id,
                name: e.name,
                created_at_ms: e.created_at_ms,
                expires_at_ms: e.expires_at_ms.unwrap_or(0),
                is_superadmin: e.is_superadmin,
            })
            .collect();
        Ok(Response::new(ListApiKeysResponse { keys }))
    }

    #[instrument(skip(self))]
    async fn set_acl(
        &self,
        request: Request<SetAclRequest>,
    ) -> Result<Response<SetAclResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();
        if req.key_id.is_empty() {
            return Err(Status::invalid_argument("key_id must not be empty"));
        }
        let permissions = req
            .permissions
            .into_iter()
            .map(|p| (p.kind, p.pattern))
            .collect();
        let found = self
            .broker
            .set_acl(&req.key_id, permissions)
            .map_err(|e| Status::internal(format!("storage error: {e}")))?;
        if found {
            Ok(Response::new(SetAclResponse {}))
        } else {
            Err(Status::not_found("api key not found"))
        }
    }

    #[instrument(skip(self))]
    async fn get_acl(
        &self,
        request: Request<GetAclRequest>,
    ) -> Result<Response<GetAclResponse>, Status> {
        self.check_admin(&request)?;
        let req = request.into_inner();
        if req.key_id.is_empty() {
            return Err(Status::invalid_argument("key_id must not be empty"));
        }
        match self
            .broker
            .get_acl(&req.key_id)
            .map_err(|e| Status::internal(format!("storage error: {e}")))?
        {
            Some(entry) => Ok(Response::new(GetAclResponse {
                key_id: entry.key_id,
                permissions: entry
                    .permissions
                    .into_iter()
                    .map(|(kind, pattern)| AclPermission { kind, pattern })
                    .collect(),
                is_superadmin: entry.is_superadmin,
            })),
            None => Err(Status::not_found("api key not found")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fila_core::{Broker, BrokerConfig, RocksDbEngine};
    use fila_proto::QueueConfig as ProtoQueueConfig;

    fn test_admin_service() -> (AdminService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbEngine::open(dir.path()).unwrap());
        let broker = Arc::new(Broker::new(BrokerConfig::default(), storage).unwrap());
        (AdminService::new(broker, None), dir)
    }

    #[tokio::test]
    async fn create_queue_rejects_invalid_visibility_timeout() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(CreateQueueRequest {
            name: "test-queue".to_string(),
            config: Some(ProtoQueueConfig {
                visibility_timeout_ms: 500, // too low
                ..Default::default()
            }),
        });

        let err = svc.create_queue(request).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("visibility_timeout_ms"));
    }

    #[tokio::test]
    async fn create_queue_uses_default_when_timeout_is_zero() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(CreateQueueRequest {
            name: "test-queue".to_string(),
            config: Some(ProtoQueueConfig {
                visibility_timeout_ms: 0, // proto3 unset
                ..Default::default()
            }),
        });

        let resp = svc.create_queue(request).await.unwrap();
        assert_eq!(resp.into_inner().queue_id, "test-queue");
    }

    #[tokio::test]
    async fn create_queue_accepts_valid_visibility_timeout() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(CreateQueueRequest {
            name: "test-queue".to_string(),
            config: Some(ProtoQueueConfig {
                visibility_timeout_ms: 60_000,
                ..Default::default()
            }),
        });

        let resp = svc.create_queue(request).await.unwrap();
        assert_eq!(resp.into_inner().queue_id, "test-queue");
    }

    #[tokio::test]
    async fn set_config_with_valid_throttle_key() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(SetConfigRequest {
            key: "throttle.provider_a".to_string(),
            value: "10.0,100.0".to_string(),
        });

        svc.set_config(request).await.unwrap();

        // Verify via get_config
        let request = Request::new(GetConfigRequest {
            key: "throttle.provider_a".to_string(),
        });
        let resp = svc.get_config(request).await.unwrap();
        assert_eq!(resp.into_inner().value, "10.0,100.0");
    }

    #[tokio::test]
    async fn set_config_empty_key_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(SetConfigRequest {
            key: String::new(),
            value: "10.0,100.0".to_string(),
        });

        let err = svc.set_config(request).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_config_empty_key_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(GetConfigRequest { key: String::new() });

        let err = svc.get_config(request).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_config_missing_key_returns_empty_value() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(GetConfigRequest {
            key: "nonexistent".to_string(),
        });

        let resp = svc.get_config(request).await.unwrap();
        assert_eq!(resp.into_inner().value, "");
    }

    #[tokio::test]
    async fn list_config_returns_all_entries() {
        let (svc, _dir) = test_admin_service();

        // Set a few config entries
        for (key, value) in &[("throttle.provider_a", "10.0,100.0"), ("app.flag", "on")] {
            svc.set_config(Request::new(SetConfigRequest {
                key: key.to_string(),
                value: value.to_string(),
            }))
            .await
            .unwrap();
        }

        let resp = svc
            .list_config(Request::new(ListConfigRequest {
                prefix: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.total_count, 2);
        assert_eq!(resp.entries.len(), 2);
        // RocksDB lexicographic order: app.flag < throttle.provider_a
        assert_eq!(resp.entries[0].key, "app.flag");
        assert_eq!(resp.entries[0].value, "on");
        assert_eq!(resp.entries[1].key, "throttle.provider_a");
        assert_eq!(resp.entries[1].value, "10.0,100.0");
    }

    #[tokio::test]
    async fn list_config_filters_by_prefix() {
        let (svc, _dir) = test_admin_service();

        for (key, value) in &[
            ("throttle.a", "1.0,10.0"),
            ("throttle.b", "2.0,20.0"),
            ("app.flag", "on"),
        ] {
            svc.set_config(Request::new(SetConfigRequest {
                key: key.to_string(),
                value: value.to_string(),
            }))
            .await
            .unwrap();
        }

        let resp = svc
            .list_config(Request::new(ListConfigRequest {
                prefix: "throttle.".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.total_count, 2);
        assert_eq!(resp.entries[0].key, "throttle.a");
        assert_eq!(resp.entries[0].value, "1.0,10.0");
        assert_eq!(resp.entries[1].key, "throttle.b");
        assert_eq!(resp.entries[1].value, "2.0,20.0");
    }

    #[tokio::test]
    async fn list_config_empty_result_is_not_error() {
        let (svc, _dir) = test_admin_service();

        let resp = svc
            .list_config(Request::new(ListConfigRequest {
                prefix: "nonexistent.".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.total_count, 0);
        assert!(resp.entries.is_empty());
    }

    #[tokio::test]
    async fn list_config_oversized_prefix_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        let long_prefix = "x".repeat(AdminService::MAX_CONFIG_KEY_LEN + 1);
        let err = svc
            .list_config(Request::new(ListConfigRequest {
                prefix: long_prefix,
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_stats_returns_populated_response() {
        let (svc, _dir) = test_admin_service();

        // Create a queue first
        svc.create_queue(Request::new(CreateQueueRequest {
            name: "stats-q".to_string(),
            config: None,
        }))
        .await
        .unwrap();

        let resp = svc
            .get_stats(Request::new(GetStatsRequest {
                queue: "stats-q".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.depth, 0);
        assert_eq!(resp.in_flight, 0);
        assert_eq!(resp.active_fairness_keys, 0);
        assert_eq!(resp.active_consumers, 0);
    }

    #[tokio::test]
    async fn get_stats_empty_queue_id_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        let err = svc
            .get_stats(Request::new(GetStatsRequest {
                queue: String::new(),
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_stats_nonexistent_queue_returns_not_found() {
        let (svc, _dir) = test_admin_service();

        let err = svc
            .get_stats(Request::new(GetStatsRequest {
                queue: "nonexistent".to_string(),
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn redrive_returns_redriven_count() {
        let (svc, _dir) = test_admin_service();

        // Create a DLQ-named queue and its parent
        svc.create_queue(Request::new(CreateQueueRequest {
            name: "redrive-test".to_string(),
            config: None,
        }))
        .await
        .unwrap();

        // Redrive from the auto-created DLQ (which is empty)
        let resp = svc
            .redrive(Request::new(RedriveRequest {
                dlq_queue: "redrive-test.dlq".to_string(),
                count: 0,
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.redriven, 0);
    }

    #[tokio::test]
    async fn redrive_empty_dlq_queue_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        let err = svc
            .redrive(Request::new(RedriveRequest {
                dlq_queue: String::new(),
                count: 0,
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn list_queues_returns_created_queues() {
        let (svc, _dir) = test_admin_service();

        // No queues initially
        let resp = svc
            .list_queues(Request::new(ListQueuesRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.queues.is_empty());

        // Create two queues
        svc.create_queue(Request::new(CreateQueueRequest {
            name: "alpha".to_string(),
            config: None,
        }))
        .await
        .unwrap();
        svc.create_queue(Request::new(CreateQueueRequest {
            name: "beta".to_string(),
            config: None,
        }))
        .await
        .unwrap();

        let resp = svc
            .list_queues(Request::new(ListQueuesRequest {}))
            .await
            .unwrap()
            .into_inner();

        // Should have 4 queues: alpha, alpha.dlq, beta, beta.dlq
        let names: Vec<&str> = resp.queues.iter().map(|q| q.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"alpha.dlq"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"beta.dlq"));
        assert_eq!(resp.queues.len(), 4);

        // All should have zero depth/in_flight/consumers
        for q in &resp.queues {
            assert_eq!(q.depth, 0);
            assert_eq!(q.in_flight, 0);
            assert_eq!(q.active_consumers, 0);
        }
    }

    #[tokio::test]
    async fn get_stats_single_node_returns_zero_cluster_fields() {
        let (svc, _dir) = test_admin_service();

        svc.create_queue(Request::new(CreateQueueRequest {
            name: "single-q".to_string(),
            config: None,
        }))
        .await
        .unwrap();

        let resp = svc
            .get_stats(Request::new(GetStatsRequest {
                queue: "single-q".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            resp.leader_node_id, 0,
            "single-node should have leader_node_id=0"
        );
        assert_eq!(
            resp.replication_count, 0,
            "single-node should have replication_count=0"
        );
    }

    #[tokio::test]
    async fn list_queues_single_node_returns_zero_cluster_fields() {
        let (svc, _dir) = test_admin_service();

        svc.create_queue(Request::new(CreateQueueRequest {
            name: "single-q".to_string(),
            config: None,
        }))
        .await
        .unwrap();

        let resp = svc
            .list_queues(Request::new(ListQueuesRequest {}))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            resp.cluster_node_count, 0,
            "single-node should have cluster_node_count=0"
        );
        for q in &resp.queues {
            assert_eq!(
                q.leader_node_id, 0,
                "single-node queue should have leader_node_id=0"
            );
        }
    }

    #[tokio::test]
    async fn redrive_non_dlq_queue_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        svc.create_queue(Request::new(CreateQueueRequest {
            name: "not-a-dlq".to_string(),
            config: None,
        }))
        .await
        .unwrap();

        let err = svc
            .redrive(Request::new(RedriveRequest {
                dlq_queue: "not-a-dlq".to_string(),
                count: 0,
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }
}
