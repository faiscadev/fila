#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    use openraft::storage::Adaptor;
    use openraft::{BasicNode, Config, Raft};
    use tokio::time::timeout;
    use tonic::transport::Server;

    use crate::cluster::grpc_service::ClusterGrpcService;
    use crate::cluster::multi_raft::MultiRaftManager;
    use crate::cluster::network::FilaNetworkFactory;
    use crate::cluster::store::FilaRaftStore;
    use crate::cluster::types::TypeConfig;
    use crate::storage::RocksDbEngine;
    use fila_proto::fila_cluster_server::FilaClusterServer;

    struct TestNode {
        raft: Arc<Raft<TypeConfig>>,
        multi_raft: Arc<MultiRaftManager>,
        _grpc_handle: tokio::task::JoinHandle<()>,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        _dir: tempfile::TempDir,
    }

    impl TestNode {
        async fn start(node_id: u64, port: u16) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db: Arc<dyn crate::storage::RaftKeyValueStore> =
                Arc::new(RocksDbEngine::open(dir.path().to_str().unwrap()).unwrap());

            let raft_config = Config {
                cluster_name: "fila-test".to_string(),
                heartbeat_interval: 100,
                election_timeout_min: 200,
                election_timeout_max: 400,
                ..Default::default()
            };
            let raft_config = Arc::new(raft_config.validate().unwrap());

            let store = FilaRaftStore::new(Arc::clone(&db), None);
            let (log_store, state_machine) = Adaptor::new(store);
            let network = FilaNetworkFactory::meta();

            let raft = Raft::new(
                node_id,
                Arc::clone(&raft_config),
                network,
                log_store,
                state_machine,
            )
            .await
            .unwrap();
            let raft = Arc::new(raft);

            let multi_raft = Arc::new(MultiRaftManager::new(
                node_id,
                Arc::clone(&db),
                Arc::clone(&raft_config),
            ));

            let broker_slot = Arc::new(std::sync::OnceLock::new());
            let service = ClusterGrpcService::new(
                Arc::clone(&raft),
                Arc::clone(&multi_raft),
                Arc::clone(&broker_slot),
            );
            let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

            let grpc_handle = tokio::spawn(async move {
                Server::builder()
                    .add_service(FilaClusterServer::new(service))
                    .serve_with_shutdown(addr, async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .unwrap();
            });

            // Give the gRPC server a moment to bind.
            tokio::time::sleep(Duration::from_millis(50)).await;

            Self {
                raft,
                multi_raft,
                _grpc_handle: grpc_handle,
                shutdown_tx: Some(shutdown_tx),
                _dir: dir,
            }
        }

        async fn shutdown(mut self) {
            self.multi_raft.shutdown_all().await;
            let _ = self.raft.shutdown().await;
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    async fn bootstrap_cluster(nodes: &[&TestNode], base_port: u16) -> BTreeMap<u64, BasicNode> {
        let mut members = BTreeMap::new();
        for (i, _) in nodes.iter().enumerate() {
            let id = (i + 1) as u64;
            members.insert(
                id,
                BasicNode {
                    addr: format!("127.0.0.1:{}", base_port + i as u16),
                },
            );
        }

        for node in nodes {
            node.raft.initialize(members.clone()).await.unwrap();
        }

        members
    }

    async fn wait_for_leader(raft: &Raft<TypeConfig>, timeout_secs: u64) -> u64 {
        timeout(Duration::from_secs(timeout_secs), async {
            loop {
                if let Some(leader_id) = raft.current_leader().await {
                    return leader_id;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("leader should be elected within timeout")
    }

    /// Bootstrap a 3-node cluster and verify leader election.
    #[tokio::test]
    async fn test_three_node_bootstrap_and_leader_election() {
        let base_port = 15600;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        bootstrap_cluster(&[&node1, &node2, &node3], base_port).await;

        let leader = wait_for_leader(&node1.raft, 5).await;
        assert!(
            leader == 1 || leader == 2 || leader == 3,
            "leader should be one of the 3 nodes"
        );

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Test adding a 4th node to an existing 3-node cluster.
    #[tokio::test]
    async fn test_add_node_to_cluster() {
        let base_port = 15610;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        bootstrap_cluster(&[&node1, &node2, &node3], base_port).await;

        let leader_id = wait_for_leader(&node1.raft, 5).await;
        let leader_raft = match leader_id {
            1 => &node1.raft,
            2 => &node2.raft,
            3 => &node3.raft,
            _ => unreachable!(),
        };

        // Start node 4 and add it to the cluster.
        let node4 = TestNode::start(4, base_port + 3).await;

        leader_raft
            .add_learner(
                4,
                BasicNode {
                    addr: format!("127.0.0.1:{}", base_port + 3),
                },
                true,
            )
            .await
            .unwrap();

        // Promote to voter.
        let mut add_voters = std::collections::BTreeSet::new();
        add_voters.insert(4);
        leader_raft
            .change_membership(openraft::ChangeMembers::AddVoterIds(add_voters), false)
            .await
            .unwrap();

        // Verify node 4 sees the leader.
        let node4_leader = wait_for_leader(&node4.raft, 5).await;
        assert_eq!(node4_leader, leader_id, "node 4 should see the same leader");

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
        node4.shutdown().await;
    }

    /// Test removing a node from the cluster.
    #[tokio::test]
    async fn test_remove_node_from_cluster() {
        let base_port = 15620;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        bootstrap_cluster(&[&node1, &node2, &node3], base_port).await;

        let leader_id = wait_for_leader(&node1.raft, 5).await;
        let leader_raft = match leader_id {
            1 => &node1.raft,
            2 => &node2.raft,
            3 => &node3.raft,
            _ => unreachable!(),
        };

        // Remove a non-leader node.
        let remove_id = if leader_id == 3 { 2 } else { 3 };
        let mut remove_set = std::collections::BTreeSet::new();
        remove_set.insert(remove_id);
        leader_raft
            .change_membership(openraft::ChangeMembers::RemoveVoters(remove_set), false)
            .await
            .unwrap();

        // Verify the remaining 2-node cluster still has a leader
        // that is NOT the removed node.
        let remaining: Vec<u64> = vec![1, 2, 3]
            .into_iter()
            .filter(|&id| id != remove_id)
            .collect();
        let still_leader = wait_for_leader(&node1.raft, 5).await;

        assert!(
            remaining.contains(&still_leader),
            "leader {still_leader} must be one of the remaining nodes {remaining:?}, not the removed node {remove_id}"
        );

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Single-node bootstrap test.
    #[tokio::test]
    async fn test_single_node_bootstrap() {
        let base_port = 15630;
        let node = TestNode::start(1, base_port).await;

        let mut members = BTreeMap::new();
        members.insert(
            1,
            BasicNode {
                addr: format!("127.0.0.1:{base_port}"),
            },
        );

        node.raft.initialize(members).await.unwrap();

        let leader = wait_for_leader(&node.raft, 3).await;
        assert_eq!(leader, 1);

        node.shutdown().await;
    }

    // --- Multi-Raft (per-queue group) tests ---

    /// Test creating a queue Raft group on a 3-node cluster.
    #[tokio::test]
    async fn test_queue_raft_group_creation() {
        let base_port = 15640;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        let members = bootstrap_cluster(&[&node1, &node2, &node3], base_port).await;
        wait_for_leader(&node1.raft, 5).await;

        let member_ids = nonempty::NonEmpty::from_vec(
            members
                .iter()
                .map(|(&id, n)| (id, n.addr.clone()))
                .collect(),
        )
        .unwrap();

        // Create a queue Raft group on all nodes.
        for node in [&node1, &node2, &node3] {
            node.multi_raft
                .create_group("orders", &member_ids)
                .await
                .unwrap();
        }

        // Verify each node has the queue group.
        assert!(node1.multi_raft.get_raft("orders").await.is_some());
        assert!(node2.multi_raft.get_raft("orders").await.is_some());
        assert!(node3.multi_raft.get_raft("orders").await.is_some());

        // Wait for queue group leader election.
        let queue_raft = node1.multi_raft.get_raft("orders").await.unwrap();
        let queue_leader = wait_for_leader(&queue_raft, 5).await;
        assert!(
            (1..=3).contains(&queue_leader),
            "queue leader should be one of the 3 nodes"
        );

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Test writing through a queue's Raft group.
    #[tokio::test]
    async fn test_queue_raft_group_write() {
        let base_port = 15650;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        let members = bootstrap_cluster(&[&node1, &node2, &node3], base_port).await;
        wait_for_leader(&node1.raft, 5).await;

        let member_ids = nonempty::NonEmpty::from_vec(
            members
                .iter()
                .map(|(&id, n)| (id, n.addr.clone()))
                .collect(),
        )
        .unwrap();

        // Create queue group on all nodes.
        for node in [&node1, &node2, &node3] {
            node.multi_raft
                .create_group("payments", &member_ids)
                .await
                .unwrap();
        }

        // Wait for queue leader.
        let nodes = [&node1, &node2, &node3];
        let queue_leader_id = timeout(Duration::from_secs(5), async {
            loop {
                for node in &nodes {
                    if let Some(raft) = node.multi_raft.get_raft("payments").await {
                        if let Some(id) = raft.current_leader().await {
                            return id;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("queue leader should be elected");

        // Submit a write to the queue leader.
        let leader_node = match queue_leader_id {
            1 => &node1,
            2 => &node2,
            3 => &node3,
            _ => unreachable!(),
        };
        let leader_raft = leader_node.multi_raft.get_raft("payments").await.unwrap();

        let msg = crate::message::Message {
            id: uuid::Uuid::now_v7(),
            queue_id: "payments".to_string(),
            headers: std::collections::HashMap::new(),
            payload: vec![1, 2, 3],
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 1_000_000_000,
            leased_at: None,
        };

        let resp = leader_raft
            .client_write(crate::cluster::ClusterRequest::Enqueue {
                message: msg.clone(),
            })
            .await
            .unwrap();

        match resp.data {
            crate::cluster::ClusterResponse::Enqueue { msg_id } => {
                assert_eq!(msg_id, msg.id);
            }
            other => panic!("expected Enqueue response, got {other:?}"),
        }

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Test deleting a queue Raft group.
    #[tokio::test]
    async fn test_queue_raft_group_deletion() {
        let base_port = 15660;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        let members = bootstrap_cluster(&[&node1, &node2, &node3], base_port).await;
        wait_for_leader(&node1.raft, 5).await;

        let member_ids = nonempty::NonEmpty::from_vec(
            members
                .iter()
                .map(|(&id, n)| (id, n.addr.clone()))
                .collect(),
        )
        .unwrap();

        // Create and then delete a queue group.
        for node in [&node1, &node2, &node3] {
            node.multi_raft
                .create_group("temp-queue", &member_ids)
                .await
                .unwrap();
        }

        // Verify it exists.
        assert!(node1.multi_raft.get_raft("temp-queue").await.is_some());

        // Delete on all nodes.
        for node in [&node1, &node2, &node3] {
            node.multi_raft.remove_group("temp-queue").await;
        }

        // Verify it's gone.
        assert!(node1.multi_raft.get_raft("temp-queue").await.is_none());
        assert!(node2.multi_raft.get_raft("temp-queue").await.is_none());
        assert!(node3.multi_raft.get_raft("temp-queue").await.is_none());

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    // --- Story 14.3: Request Routing & Transparent Delivery tests ---

    /// Full test node with broker, cluster handle, and meta event processing.
    /// Used for Story 14.3 integration tests that verify end-to-end routing.
    struct FullTestNode {
        raft: Arc<Raft<TypeConfig>>,
        multi_raft: Arc<MultiRaftManager>,
        broker: Arc<crate::Broker>,
        cluster_handle: Arc<crate::ClusterHandle>,
        _grpc_handle: tokio::task::JoinHandle<()>,
        _event_handle: tokio::task::JoinHandle<()>,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        _dir: tempfile::TempDir,
        _broker_dir: tempfile::TempDir,
    }

    impl FullTestNode {
        async fn start(node_id: u64, port: u16) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db: Arc<dyn crate::storage::RaftKeyValueStore> =
                Arc::new(RocksDbEngine::open(dir.path().to_str().unwrap()).unwrap());

            let raft_config = Config {
                cluster_name: "fila-test".to_string(),
                heartbeat_interval: 100,
                election_timeout_min: 200,
                election_timeout_max: 400,
                ..Default::default()
            };
            let raft_config = Arc::new(raft_config.validate().unwrap());

            let (meta_event_tx, meta_event_rx) = tokio::sync::mpsc::unbounded_channel();
            let store = FilaRaftStore::new(Arc::clone(&db), Some(meta_event_tx));
            let (log_store, state_machine) = Adaptor::new(store);
            let network = FilaNetworkFactory::meta();

            let raft = Raft::new(
                node_id,
                Arc::clone(&raft_config),
                network,
                log_store,
                state_machine,
            )
            .await
            .unwrap();
            let raft = Arc::new(raft);

            let multi_raft = Arc::new(MultiRaftManager::new(
                node_id,
                Arc::clone(&db),
                Arc::clone(&raft_config),
            ));

            let broker_slot = Arc::new(std::sync::OnceLock::new());
            let service = ClusterGrpcService::new(
                Arc::clone(&raft),
                Arc::clone(&multi_raft),
                Arc::clone(&broker_slot),
            );
            let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

            let grpc_handle = tokio::spawn(async move {
                Server::builder()
                    .add_service(FilaClusterServer::new(service))
                    .serve_with_shutdown(addr, async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .unwrap();
            });

            tokio::time::sleep(Duration::from_millis(50)).await;

            // Create broker with a separate RocksDB for message storage.
            let broker_dir = tempfile::tempdir().unwrap();
            let broker_db =
                Arc::new(RocksDbEngine::open(broker_dir.path().to_str().unwrap()).unwrap());
            let broker =
                Arc::new(crate::Broker::new(crate::BrokerConfig::default(), broker_db).unwrap());

            // Wire broker to cluster gRPC service for forwarded write handling.
            let _ = broker_slot.set(Arc::clone(&broker));

            // Start meta event handler.
            let event_broker = Arc::clone(&broker);
            let event_multi_raft = Arc::clone(&multi_raft);
            let event_handle = tokio::spawn(crate::cluster::process_meta_events(
                meta_event_rx,
                event_broker,
                event_multi_raft,
            ));

            let cluster_handle = Arc::new(crate::ClusterHandle {
                meta_raft: Arc::clone(&raft),
                multi_raft: Arc::clone(&multi_raft),
                node_id,
                client_cache: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            });

            Self {
                raft,
                multi_raft,
                broker,
                cluster_handle,
                _grpc_handle: grpc_handle,
                _event_handle: event_handle,
                shutdown_tx: Some(shutdown_tx),
                _dir: dir,
                _broker_dir: broker_dir,
            }
        }

        async fn shutdown(mut self) {
            self.multi_raft.shutdown_all().await;
            let _ = self.raft.shutdown().await;
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    /// Create a queue via the meta Raft (simulating admin service in cluster mode).
    async fn create_queue_cluster(
        node: &FullTestNode,
        queue_name: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let (members, _addrs) = node.cluster_handle.meta_members();
        let config = crate::QueueConfig {
            name: queue_name.to_string(),
            on_enqueue_script: None,
            on_failure_script: None,
            visibility_timeout_ms: crate::QueueConfig::DEFAULT_VISIBILITY_TIMEOUT_MS,
            dlq_queue_id: None,
            lua_timeout_ms: None,
            lua_memory_limit_bytes: None,
        };

        let resp = node
            .cluster_handle
            .write_to_meta(crate::ClusterRequest::CreateQueueGroup {
                queue_id: queue_name.to_string(),
                members,
                config,
            })
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        match resp {
            crate::ClusterResponse::CreateQueueGroup { queue_id } => Ok(queue_id),
            other => Err(format!("unexpected response: {other:?}").into()),
        }
    }

    /// Enqueue a message via cluster routing (simulating hot-path service in cluster mode).
    async fn enqueue_cluster(
        node: &FullTestNode,
        queue_id: &str,
        payload: Vec<u8>,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let msg = crate::Message {
            id: uuid::Uuid::now_v7(),
            queue_id: queue_id.to_string(),
            headers: std::collections::HashMap::new(),
            payload: payload.clone(),
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 1_000_000_000,
            leased_at: None,
        };

        let result = node
            .cluster_handle
            .write_to_queue(
                queue_id,
                crate::ClusterRequest::Enqueue {
                    message: msg.clone(),
                },
            )
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        let msg_id = match result.response {
            crate::ClusterResponse::Enqueue { msg_id } => msg_id,
            other => return Err(format!("unexpected response: {other:?}").into()),
        };

        // Apply to local scheduler only if handled locally (this node is leader).
        // Forwarded writes are applied by the leader's ClientWrite handler.
        if result.handled_locally {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            node.broker
                .send_command(crate::SchedulerCommand::Enqueue {
                    message: msg,
                    reply: reply_tx,
                })
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("{e}").into()
                })?;
            let _ = reply_rx.await??;
        }

        Ok(msg_id)
    }

    /// Wait for a queue Raft group to exist and have a leader on all nodes.
    async fn wait_for_queue_ready(
        nodes: &[&FullTestNode],
        queue_id: &str,
        timeout_secs: u64,
    ) -> u64 {
        timeout(Duration::from_secs(timeout_secs), async {
            loop {
                // Check all nodes have the queue group.
                let mut all_have_group = true;
                for node in nodes {
                    if node.multi_raft.get_raft(queue_id).await.is_none() {
                        all_have_group = false;
                        break;
                    }
                }
                if !all_have_group {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }

                // Check for a leader.
                for node in nodes {
                    if let Some(raft) = node.multi_raft.get_raft(queue_id).await {
                        if let Some(leader) = raft.current_leader().await {
                            return leader;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("queue should be ready within timeout")
    }

    /// Get the FullTestNode that is the leader for a queue.
    fn get_leader_node<'a>(nodes: &'a [&'a FullTestNode], leader_id: u64) -> &'a FullTestNode {
        nodes
            .iter()
            .find(|n| n.cluster_handle.node_id == leader_id)
            .expect("leader node should exist")
    }

    async fn bootstrap_full_cluster(
        nodes: &[&FullTestNode],
        base_port: u16,
    ) -> BTreeMap<u64, BasicNode> {
        let mut members = BTreeMap::new();
        for (i, _) in nodes.iter().enumerate() {
            let id = (i + 1) as u64;
            members.insert(
                id,
                BasicNode {
                    addr: format!("127.0.0.1:{}", base_port + i as u16),
                },
            );
        }
        for node in nodes {
            node.raft.initialize(members.clone()).await.unwrap();
        }
        members
    }

    /// Test: enqueue on non-leader node, consume on leader — message delivered.
    #[tokio::test]
    async fn test_cluster_enqueue_and_consume_across_nodes() {
        let base_port = 15700;
        let node1 = FullTestNode::start(1, base_port).await;
        let node2 = FullTestNode::start(2, base_port + 1).await;
        let node3 = FullTestNode::start(3, base_port + 2).await;

        let nodes = [&node1, &node2, &node3];
        bootstrap_full_cluster(&nodes, base_port).await;
        wait_for_leader(&node1.raft, 5).await;

        // Create queue via meta Raft (from any node).
        create_queue_cluster(&node1, "orders").await.unwrap();

        // Wait for queue Raft group to be ready on all nodes.
        let queue_leader_id = wait_for_queue_ready(&nodes, "orders", 10).await;

        // Find a non-leader node for enqueue.
        let non_leader_id = if queue_leader_id == 1 { 2 } else { 1 };
        let non_leader = get_leader_node(&nodes, non_leader_id);
        let leader = get_leader_node(&nodes, queue_leader_id);

        // Enqueue on non-leader → should be forwarded to leader via Raft.
        let msg_id = enqueue_cluster(non_leader, "orders", b"hello cluster".to_vec())
            .await
            .unwrap();

        // Consume from the leader's broker — message should be there.
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(1);
        let consumer_id = "test-consumer".to_string();
        leader
            .broker
            .send_command(crate::SchedulerCommand::RegisterConsumer {
                queue_id: "orders".to_string(),
                consumer_id: consumer_id.clone(),
                tx: ready_tx,
            })
            .unwrap();

        let ready_msg = timeout(Duration::from_secs(5), ready_rx.recv())
            .await
            .expect("should receive message within timeout")
            .expect("channel should not be closed");

        assert_eq!(ready_msg.msg_id, msg_id);
        assert_eq!(ready_msg.payload, b"hello cluster");

        // Cleanup.
        let _ = leader
            .broker
            .send_command(crate::SchedulerCommand::UnregisterConsumer { consumer_id });

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Test: ack on a different node completes the message lifecycle.
    #[tokio::test]
    async fn test_cluster_ack_across_nodes() {
        let base_port = 15710;
        let node1 = FullTestNode::start(1, base_port).await;
        let node2 = FullTestNode::start(2, base_port + 1).await;
        let node3 = FullTestNode::start(3, base_port + 2).await;

        let nodes = [&node1, &node2, &node3];
        bootstrap_full_cluster(&nodes, base_port).await;
        wait_for_leader(&node1.raft, 5).await;

        create_queue_cluster(&node1, "ack-test").await.unwrap();
        let queue_leader_id = wait_for_queue_ready(&nodes, "ack-test", 10).await;
        let leader = get_leader_node(&nodes, queue_leader_id);

        // Enqueue on leader.
        let msg_id = enqueue_cluster(leader, "ack-test", b"ack me".to_vec())
            .await
            .unwrap();

        // Consume from leader.
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(1);
        leader
            .broker
            .send_command(crate::SchedulerCommand::RegisterConsumer {
                queue_id: "ack-test".to_string(),
                consumer_id: "c1".to_string(),
                tx: ready_tx,
            })
            .unwrap();

        let ready_msg = timeout(Duration::from_secs(5), ready_rx.recv())
            .await
            .expect("message should be received")
            .unwrap();
        assert_eq!(ready_msg.msg_id, msg_id);

        // Ack from a different node (non-leader) via Raft.
        let ack_node_id = if queue_leader_id == 3 { 2 } else { 3 };
        let ack_node = get_leader_node(&nodes, ack_node_id);

        let result = ack_node
            .cluster_handle
            .write_to_queue(
                "ack-test",
                crate::ClusterRequest::Ack {
                    queue_id: "ack-test".to_string(),
                    msg_id,
                },
            )
            .await
            .unwrap();

        assert!(
            matches!(result.response, crate::ClusterResponse::Ack),
            "expected Ack response, got {:?}",
            result.response
        );

        // Apply ack to leader's scheduler (forwarded writes are applied
        // by the leader's ClientWrite handler, but for local writes we
        // need to apply manually like the service handler does).
        if result.handled_locally {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            ack_node
                .broker
                .send_command(crate::SchedulerCommand::Ack {
                    queue_id: "ack-test".to_string(),
                    msg_id,
                    reply: reply_tx,
                })
                .unwrap();
            reply_rx.await.unwrap().unwrap();
        }

        // Cleanup.
        let _ = leader
            .broker
            .send_command(crate::SchedulerCommand::UnregisterConsumer {
                consumer_id: "c1".to_string(),
            });

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Test: create queue on non-meta-leader node — propagates to all nodes.
    #[tokio::test]
    async fn test_cluster_create_queue_on_non_leader() {
        let base_port = 15720;
        let node1 = FullTestNode::start(1, base_port).await;
        let node2 = FullTestNode::start(2, base_port + 1).await;
        let node3 = FullTestNode::start(3, base_port + 2).await;

        let nodes = [&node1, &node2, &node3];
        bootstrap_full_cluster(&nodes, base_port).await;
        let meta_leader_id = wait_for_leader(&node1.raft, 5).await;

        // Find a non-meta-leader node.
        let non_leader_id = if meta_leader_id == 1 { 2 } else { 1 };
        let non_leader = get_leader_node(&nodes, non_leader_id);

        // Create queue from the non-meta-leader node.
        let queue_id = create_queue_cluster(non_leader, "routed-queue")
            .await
            .unwrap();
        assert_eq!(queue_id, "routed-queue");

        // Wait for the queue Raft group to be created on all nodes.
        let _queue_leader = wait_for_queue_ready(&nodes, "routed-queue", 10).await;

        // Verify all nodes have the queue group.
        for node in &nodes {
            assert!(
                node.multi_raft.get_raft("routed-queue").await.is_some(),
                "node {} should have routed-queue group",
                node.cluster_handle.node_id
            );
        }

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Test: delete queue on non-leader node — cleaned up on all nodes.
    #[tokio::test]
    async fn test_cluster_delete_queue_propagates() {
        let base_port = 15730;
        let node1 = FullTestNode::start(1, base_port).await;
        let node2 = FullTestNode::start(2, base_port + 1).await;
        let node3 = FullTestNode::start(3, base_port + 2).await;

        let nodes = [&node1, &node2, &node3];
        bootstrap_full_cluster(&nodes, base_port).await;
        wait_for_leader(&node1.raft, 5).await;

        // Create queue.
        create_queue_cluster(&node1, "temp-q").await.unwrap();
        wait_for_queue_ready(&nodes, "temp-q", 10).await;

        // Delete from a non-meta-leader node.
        let meta_leader_id = wait_for_leader(&node1.raft, 5).await;
        let delete_node_id = if meta_leader_id == 3 { 2 } else { 3 };
        let delete_node = get_leader_node(&nodes, delete_node_id);

        let resp = delete_node
            .cluster_handle
            .write_to_meta(crate::ClusterRequest::DeleteQueueGroup {
                queue_id: "temp-q".to_string(),
            })
            .await
            .unwrap();

        assert!(
            matches!(resp, crate::ClusterResponse::DeleteQueueGroup),
            "expected DeleteQueueGroup response"
        );

        // Wait for cleanup on all nodes.
        timeout(Duration::from_secs(5), async {
            loop {
                let mut all_removed = true;
                for node in &nodes {
                    if node.multi_raft.get_raft("temp-q").await.is_some() {
                        all_removed = false;
                        break;
                    }
                }
                if all_removed {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("queue group should be cleaned up on all nodes");

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }

    /// Test multiple queues have potentially different leaders (leadership distribution).
    #[tokio::test]
    async fn test_multiple_queue_groups_leadership() {
        let base_port = 15670;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        let members = bootstrap_cluster(&[&node1, &node2, &node3], base_port).await;
        wait_for_leader(&node1.raft, 5).await;

        let member_ids = nonempty::NonEmpty::from_vec(
            members
                .iter()
                .map(|(&id, n)| (id, n.addr.clone()))
                .collect(),
        )
        .unwrap();

        // Create multiple queue groups.
        let queue_ids = ["q1", "q2", "q3", "q4", "q5"];
        for queue_id in &queue_ids {
            for node in [&node1, &node2, &node3] {
                node.multi_raft
                    .create_group(queue_id, &member_ids)
                    .await
                    .unwrap();
            }
        }

        // Wait for leaders to be elected for all queues and collect leader IDs.
        let mut leaders = Vec::new();
        for queue_id in &queue_ids {
            let raft = node1.multi_raft.get_raft(queue_id).await.unwrap();
            let leader = wait_for_leader(&raft, 5).await;
            leaders.push(leader);
        }

        // All leaders should be valid node IDs.
        for &leader in &leaders {
            assert!(
                (1..=3).contains(&leader),
                "leader should be a valid node ID"
            );
        }

        // Verify each node lists all queue groups.
        let groups = node1.multi_raft.list_groups().await;
        assert_eq!(groups.len(), 5, "should have 5 queue groups");

        node1.shutdown().await;
        node2.shutdown().await;
        node3.shutdown().await;
    }
}
