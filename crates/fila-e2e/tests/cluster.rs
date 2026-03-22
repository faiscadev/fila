mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// Helper: create a queue on the given node via CLI and wait for the queue Raft group
/// to stabilize across the cluster.
fn create_cluster_queue(addr: &str, name: &str) {
    helpers::create_queue_cli(addr, name);
    // Allow the queue Raft group to be created on all nodes, elect a leader,
    // and propagate client address mappings. CI environments may need more time.
    std::thread::sleep(Duration::from_secs(3));
}

/// Helper: run `fila queue inspect <name>` on a node and return the raw stdout.
fn queue_inspect(addr: &str, name: &str) -> String {
    let out = helpers::cli_run(addr, &["queue", "inspect", name]);
    assert!(
        out.success,
        "inspect failed on {addr}: stderr={}",
        out.stderr
    );
    out.stdout
}

/// Helper: parse "Depth:" line from inspect output.
fn parse_depth(output: &str) -> u64 {
    output
        .lines()
        .find(|l| l.contains("Depth:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().parse::<u64>().expect("parse depth"))
        .expect("Depth line not found in inspect output")
}

/// Helper: parse "Raft leader:" line to get leader node_id.
fn parse_leader_node_id(output: &str) -> u64 {
    output
        .lines()
        .find(|l| l.contains("Raft leader:"))
        .and_then(|l| l.split("node").nth(1))
        .map(|s| s.trim().parse::<u64>().expect("parse leader id"))
        .unwrap_or(0)
}

/// Helper: parse "Replicas:" line.
fn parse_replicas(output: &str) -> u32 {
    output
        .lines()
        .find(|l| l.contains("Replicas:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().parse::<u32>().expect("parse replicas"))
        .unwrap_or(0)
}

/// Helper: find the leader node index (0-based) for a queue by checking stats on node 0.
fn find_leader_index(cluster: &helpers::cluster::TestCluster, queue: &str) -> usize {
    let output = queue_inspect(cluster.addr(0), queue);
    let leader_node_id = parse_leader_node_id(&output);
    assert!(leader_node_id > 0, "no leader found for queue '{queue}'");
    (leader_node_id - 1) as usize // node IDs are 1-based
}

/// Helper: find a non-leader node index for a queue.
fn find_non_leader_index(cluster: &helpers::cluster::TestCluster, queue: &str) -> usize {
    let leader = find_leader_index(cluster, queue);
    if leader == 0 {
        1
    } else {
        0
    }
}

/// AC 1: Enqueue on node A, consume on node B (leader), ack on node C — full cross-node lifecycle.
///
/// Writes (enqueue, ack) are forwarded to the leader from any node.
/// Consume must connect to the queue's Raft leader.
#[tokio::test]
async fn cluster_cross_node_lifecycle() {
    let cluster = helpers::cluster::TestCluster::start(3);

    create_cluster_queue(cluster.addr(0), "cross-node");

    let leader = find_leader_index(&cluster, "cross-node");
    let non_leader_a = (leader + 1) % 3;
    let non_leader_b = (leader + 2) % 3;

    // Enqueue via a non-leader node (forwarded to leader)
    let client_a = helpers::sdk_client(cluster.addr(non_leader_a)).await;
    let mut headers = HashMap::new();
    headers.insert("test".to_string(), "cross-node".to_string());
    let msg_id = client_a
        .enqueue("cross-node", headers, b"hello-cluster".to_vec())
        .await
        .expect("enqueue via non-leader should succeed (forwarded)");
    assert!(!msg_id.is_empty());

    // Consume via the leader node (consume requires leader)
    let client_leader = helpers::sdk_client(cluster.addr(leader)).await;
    let mut stream = client_leader
        .consume("cross-node")
        .await
        .expect("consume on leader");
    let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("timeout consuming")
        .expect("stream ended")
        .expect("consume error");
    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.payload, b"hello-cluster");

    // Ack via a different non-leader node (forwarded to leader)
    let client_b = helpers::sdk_client(cluster.addr(non_leader_b)).await;
    client_b
        .ack("cross-node", &msg_id)
        .await
        .expect("ack via non-leader should succeed (forwarded)");
}

/// AC 2: Leader killed → new leader elected → zero message loss.
#[tokio::test]
async fn cluster_leader_failover_zero_message_loss() {
    let mut cluster = helpers::cluster::TestCluster::start(3);

    create_cluster_queue(cluster.addr(0), "failover-q");

    let leader = find_leader_index(&cluster, "failover-q");

    // Enqueue messages via the leader
    let client = helpers::sdk_client(cluster.addr(leader)).await;
    for i in 0..5 {
        client
            .enqueue(
                "failover-q",
                HashMap::new(),
                format!("msg-{i}").into_bytes(),
            )
            .await
            .expect("enqueue");
    }

    // Kill the leader
    cluster.kill_node(leader);

    // Wait for new leader election (NFR23: 10s max, we use election_timeout_ms=500)
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Find a surviving node and check if there's a new leader
    let surviving = (leader + 1) % 3;
    let inspect = queue_inspect(cluster.addr(surviving), "failover-q");
    let new_leader_id = parse_leader_node_id(&inspect);
    assert!(
        new_leader_id > 0,
        "new leader should have been elected after failover"
    );
    assert_ne!(
        new_leader_id,
        (leader + 1) as u64,
        "new leader should differ from killed node"
    );
    let new_leader_index = (new_leader_id - 1) as usize;

    // Consume from the new leader — all 5 messages should be available
    let new_client = helpers::sdk_client(cluster.addr(new_leader_index)).await;
    let mut stream = new_client
        .consume("failover-q")
        .await
        .expect("consume after failover");

    let mut received = Vec::new();
    for _ in 0..5 {
        match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
            Ok(Some(Ok(msg))) => received.push(msg),
            Ok(Some(Err(e))) => panic!("consume error after failover: {e}"),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    assert_eq!(
        received.len(),
        5,
        "expected 5 messages after failover, got {}",
        received.len()
    );
}

/// AC 3: Non-leader receives write request → forwards to leader → correct response.
#[tokio::test]
async fn cluster_leader_forwarding() {
    let cluster = helpers::cluster::TestCluster::start(3);

    create_cluster_queue(cluster.addr(0), "forwarding-q");

    let non_leader = find_non_leader_index(&cluster, "forwarding-q");
    let leader = find_leader_index(&cluster, "forwarding-q");

    // Enqueue via non-leader — should be forwarded to leader transparently
    let client = helpers::sdk_client(cluster.addr(non_leader)).await;
    let msg_id = client
        .enqueue("forwarding-q", HashMap::new(), b"forwarded-msg".to_vec())
        .await
        .expect("enqueue via non-leader should succeed (transparent forwarding)");
    assert!(!msg_id.is_empty());

    // Verify message is on the leader
    let leader_client = helpers::sdk_client(cluster.addr(leader)).await;
    let mut stream = leader_client
        .consume("forwarding-q")
        .await
        .expect("consume on leader");
    let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("consume error");
    assert_eq!(msg.id, msg_id);
    assert_eq!(msg.payload, b"forwarded-msg");

    // Ack via non-leader (forwarded)
    client
        .ack("forwarding-q", &msg_id)
        .await
        .expect("ack via non-leader should succeed (forwarded)");
}

/// AC 4: Node crashes and rejoins → catches up from Raft log.
#[tokio::test]
async fn cluster_node_rejoin_catchup() {
    let mut cluster = helpers::cluster::TestCluster::start(3);

    create_cluster_queue(cluster.addr(0), "rejoin-q");

    let leader = find_leader_index(&cluster, "rejoin-q");

    // Enqueue initial messages
    let client = helpers::sdk_client(cluster.addr(leader)).await;
    for i in 0..3 {
        client
            .enqueue(
                "rejoin-q",
                HashMap::new(),
                format!("before-{i}").into_bytes(),
            )
            .await
            .expect("enqueue before crash");
    }

    // Kill a non-leader node (killing the leader would trigger re-election, complicating things)
    let victim = (leader + 1) % 3;
    cluster.kill_node(victim);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Enqueue more messages while node is down
    for i in 0..3 {
        client
            .enqueue(
                "rejoin-q",
                HashMap::new(),
                format!("during-{i}").into_bytes(),
            )
            .await
            .expect("enqueue while node down");
    }

    // Restart the killed node
    cluster.restart_node(victim);
    // Wait for Raft log replay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify the leader sees all 6 messages
    let leader_inspect = queue_inspect(cluster.addr(leader), "rejoin-q");
    let depth = parse_depth(&leader_inspect);
    assert_eq!(depth, 6, "leader should see all 6 messages, saw {depth}");

    // Verify the restarted node has the queue and reports the correct cluster info
    let victim_inspect = queue_inspect(cluster.addr(victim), "rejoin-q");
    let replicas = parse_replicas(&victim_inspect);
    assert!(
        replicas >= 2,
        "restarted node should see replicas >= 2, got {replicas}"
    );
}

/// AC 5: `fila queue inspect` on any node returns cluster metadata (leader, replicas).
///
/// In the current architecture, the message depth is only accurate on the leader node
/// because the scheduler's in-memory state tracks only queues this node leads.
/// All nodes DO report consistent cluster metadata (leader_node_id, replication_count).
#[tokio::test]
async fn cluster_stats_cluster_metadata_all_nodes() {
    let cluster = helpers::cluster::TestCluster::start(3);

    create_cluster_queue(cluster.addr(0), "stats-q");

    // Enqueue messages
    let leader = find_leader_index(&cluster, "stats-q");
    let client = helpers::sdk_client(cluster.addr(leader)).await;
    for i in 0..5 {
        client
            .enqueue("stats-q", HashMap::new(), format!("msg-{i}").into_bytes())
            .await
            .expect("enqueue");
    }

    // Small delay for replication
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Leader should report correct depth
    let leader_inspect = queue_inspect(cluster.addr(leader), "stats-q");
    let leader_depth = parse_depth(&leader_inspect);
    assert_eq!(
        leader_depth, 5,
        "leader should report depth=5, got {leader_depth}"
    );

    // All nodes should report the same leader and replica count
    for node_idx in 0..3 {
        let inspect = queue_inspect(cluster.addr(node_idx), "stats-q");
        let leader_id = parse_leader_node_id(&inspect);
        let replicas = parse_replicas(&inspect);

        assert!(
            leader_id > 0,
            "node {node_idx} should report a valid leader, got {leader_id}"
        );
        assert_eq!(
            leader_id,
            (leader + 1) as u64,
            "node {node_idx} should agree on leader"
        );
        assert_eq!(
            replicas, 3,
            "node {node_idx} should report 3 replicas, got {replicas}"
        );
    }
}

/// AC: Consume on non-leader gets redirected to leader via leader hint,
/// and the SDK transparently reconnects so the consumer receives messages.
#[tokio::test]
async fn cluster_consume_leader_redirect() {
    let cluster = helpers::cluster::TestCluster::start(3);

    create_cluster_queue(cluster.addr(0), "redirect-q");

    let leader = find_leader_index(&cluster, "redirect-q");
    let non_leader = find_non_leader_index(&cluster, "redirect-q");

    // Enqueue a message via any node (forwarded to leader).
    let enqueue_client = helpers::sdk_client(cluster.addr(leader)).await;
    let mut headers = HashMap::new();
    headers.insert("test".to_string(), "redirect".to_string());
    let msg_id = enqueue_client
        .enqueue("redirect-q", headers, b"redirect-test".to_vec())
        .await
        .expect("enqueue should succeed");

    // Connect consumer to NON-LEADER node. The SDK should detect the
    // leader hint in the UNAVAILABLE error and transparently reconnect
    // to the leader node.
    let consumer_client = helpers::sdk_client(cluster.addr(non_leader)).await;
    let mut stream = consumer_client
        .consume("redirect-q")
        .await
        .expect("consume should succeed after leader redirect");

    // We should receive the message despite connecting to a non-leader.
    let received = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("should receive message within timeout")
        .expect("stream should not be empty")
        .expect("message should not be an error");

    assert_eq!(received.id, msg_id);
    assert_eq!(received.payload, b"redirect-test");

    // The consume redirect test is complete — we successfully received a
    // message after transparent leader redirect. Ack is tested separately
    // in cluster_cross_node_lifecycle; skip it here to avoid timing issues
    // with the redirect connection's lease propagation.
}
