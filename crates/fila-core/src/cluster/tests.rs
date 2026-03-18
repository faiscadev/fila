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

            let store = FilaRaftStore::new(Arc::clone(&db));
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

            let service = ClusterGrpcService::new(Arc::clone(&raft), Arc::clone(&multi_raft));
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
