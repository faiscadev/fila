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
    use crate::cluster::network::FilaNetworkFactory;
    use crate::cluster::store::FilaRaftStore;
    use crate::cluster::types::TypeConfig;
    use crate::storage::RocksDbEngine;
    use fila_proto::fila_cluster_server::FilaClusterServer;

    struct TestNode {
        raft: Arc<Raft<TypeConfig>>,
        _grpc_handle: tokio::task::JoinHandle<()>,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        _dir: tempfile::TempDir,
    }

    impl TestNode {
        async fn start(node_id: u64, port: u16) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Arc::new(RocksDbEngine::open(dir.path().to_str().unwrap()).unwrap());

            let raft_config = Config {
                cluster_name: "fila-test".to_string(),
                heartbeat_interval: 100,
                election_timeout_min: 200,
                election_timeout_max: 400,
                ..Default::default()
            };
            let raft_config = Arc::new(raft_config.validate().unwrap());

            let store = FilaRaftStore::new(db);
            let (log_store, state_machine) = Adaptor::new(store);
            let network = FilaNetworkFactory;

            let raft = Raft::new(node_id, raft_config, network, log_store, state_machine)
                .await
                .unwrap();
            let raft = Arc::new(raft);

            let service = ClusterGrpcService::new(Arc::clone(&raft));
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
                _grpc_handle: grpc_handle,
                shutdown_tx: Some(shutdown_tx),
                _dir: dir,
            }
        }

        async fn shutdown(mut self) {
            let _ = self.raft.shutdown().await;
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    /// Bootstrap a 3-node cluster and verify leader election.
    #[tokio::test]
    async fn test_three_node_bootstrap_and_leader_election() {
        let base_port = 15600;
        let node1 = TestNode::start(1, base_port).await;
        let node2 = TestNode::start(2, base_port + 1).await;
        let node3 = TestNode::start(3, base_port + 2).await;

        // Initialize cluster with all 3 nodes.
        let mut members = BTreeMap::new();
        members.insert(
            1,
            BasicNode {
                addr: format!("127.0.0.1:{base_port}"),
            },
        );
        members.insert(
            2,
            BasicNode {
                addr: format!("127.0.0.1:{}", base_port + 1),
            },
        );
        members.insert(
            3,
            BasicNode {
                addr: format!("127.0.0.1:{}", base_port + 2),
            },
        );

        // Initialize on all nodes (openraft docs say this is safe with same config).
        node1.raft.initialize(members.clone()).await.unwrap();
        node2.raft.initialize(members.clone()).await.unwrap();
        node3.raft.initialize(members.clone()).await.unwrap();

        // Wait for leader to be elected (within 2 seconds).
        let leader = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(leader_id) = node1.raft.current_leader().await {
                    return leader_id;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("leader should be elected within timeout");

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

        let mut members = BTreeMap::new();
        members.insert(
            1,
            BasicNode {
                addr: format!("127.0.0.1:{base_port}"),
            },
        );
        members.insert(
            2,
            BasicNode {
                addr: format!("127.0.0.1:{}", base_port + 1),
            },
        );
        members.insert(
            3,
            BasicNode {
                addr: format!("127.0.0.1:{}", base_port + 2),
            },
        );

        node1.raft.initialize(members.clone()).await.unwrap();
        node2.raft.initialize(members.clone()).await.unwrap();
        node3.raft.initialize(members.clone()).await.unwrap();

        // Wait for leader.
        let leader_id = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(id) = node1.raft.current_leader().await {
                    return id;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("leader elected");

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
        let node4_leader = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(id) = node4.raft.current_leader().await {
                    return id;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("node 4 should see leader");

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

        let mut members = BTreeMap::new();
        members.insert(
            1,
            BasicNode {
                addr: format!("127.0.0.1:{base_port}"),
            },
        );
        members.insert(
            2,
            BasicNode {
                addr: format!("127.0.0.1:{}", base_port + 1),
            },
        );
        members.insert(
            3,
            BasicNode {
                addr: format!("127.0.0.1:{}", base_port + 2),
            },
        );

        node1.raft.initialize(members.clone()).await.unwrap();
        node2.raft.initialize(members.clone()).await.unwrap();
        node3.raft.initialize(members.clone()).await.unwrap();

        // Wait for leader.
        let leader_id = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(id) = node1.raft.current_leader().await {
                    return id;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("leader elected");

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

        // Verify the remaining 2-node cluster still has a leader.
        let still_leader = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(id) = node1.raft.current_leader().await {
                    return id;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("cluster should still have leader after removing a node");

        assert!(
            still_leader == 1 || still_leader == 2 || still_leader == 3,
            "leader should exist"
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

        // Single node should elect itself as leader immediately.
        let leader = timeout(Duration::from_secs(3), async {
            loop {
                if let Some(id) = node.raft.current_leader().await {
                    return id;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("single node should become leader");

        assert_eq!(leader, 1);

        node.shutdown().await;
    }
}
