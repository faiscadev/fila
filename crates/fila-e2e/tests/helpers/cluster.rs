#![allow(dead_code)]

use std::io::BufReader;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use std::io::BufRead;
use std::path::PathBuf;

/// A running 3-node fila-server cluster for e2e testing.
///
/// Spawns multiple `fila-server` processes with cluster configuration.
/// Each node has distinct client and cluster gRPC ports.
/// Node 0 bootstraps the cluster (`peers = []`); the rest join via node 0's cluster port.
pub struct TestCluster {
    nodes: Vec<Option<ClusterNode>>,
    client_ports: Vec<u16>,
    cluster_ports: Vec<u16>,
}

struct ClusterNode {
    child: Child,
    addr: String,
    data_dir: tempfile::TempDir,
}

impl TestCluster {
    /// Start an N-node cluster. Node 0 bootstraps; others join via node 0.
    pub fn start(n: usize) -> Self {
        assert!(n >= 1, "cluster must have at least 1 node");

        let client_ports: Vec<u16> = (0..n).map(|_| free_port()).collect();
        let cluster_ports: Vec<u16> = (0..n).map(|_| free_port()).collect();

        let mut nodes = Vec::with_capacity(n);

        // Start node 0 first (bootstrap). Give it a moment to initialize Raft.
        let node0 = start_cluster_node(0, client_ports[0], cluster_ports[0], &[]);
        nodes.push(Some(node0));

        // Brief delay to let node 0 bootstrap its single-node Raft cluster
        std::thread::sleep(Duration::from_millis(500));

        // Start remaining nodes that join via node 0
        let seed = format!("127.0.0.1:{}", cluster_ports[0]);
        for i in 1..n {
            let node = start_cluster_node(i, client_ports[i], cluster_ports[i], &[&seed]);
            nodes.push(Some(node));
        }

        // Allow cluster to stabilize (nodes discover each other, elect leaders)
        std::thread::sleep(Duration::from_secs(2));

        TestCluster {
            nodes,
            client_ports,
            cluster_ports,
        }
    }

    /// Get the HTTP address of node `i` (e.g., "http://127.0.0.1:12345").
    pub fn addr(&self, i: usize) -> &str {
        &self.nodes[i].as_ref().expect("node not running").addr
    }

    /// Get the raw host:port of node `i`.
    pub fn host_port(&self, i: usize) -> String {
        format!("127.0.0.1:{}", self.client_ports[i])
    }

    /// Kill node `i` (simulates a crash).
    pub fn kill_node(&mut self, i: usize) {
        if let Some(mut node) = self.nodes[i].take() {
            let _ = node.child.kill();
            let _ = node.child.wait();
            // Keep data_dir alive in a temporary holding spot for restart
            // We stash it back so restart_node can use it
            self.nodes[i] = Some(ClusterNode {
                child: node.child,
                addr: node.addr,
                data_dir: node.data_dir,
            });
        }
    }

    /// Restart a previously killed node on the same data directory and ports.
    pub fn restart_node(&mut self, i: usize) {
        let old = self.nodes[i]
            .take()
            .expect("node must have been started before restart");
        let data_dir = old.data_dir;

        let seed = if i == 0 {
            vec![]
        } else {
            vec![format!("127.0.0.1:{}", self.cluster_ports[0])]
        };
        let seed_refs: Vec<&str> = seed.iter().map(|s| s.as_str()).collect();

        let node = restart_cluster_node(
            i,
            self.client_ports[i],
            self.cluster_ports[i],
            &seed_refs,
            data_dir,
        );
        self.nodes[i] = Some(node);
    }

    /// Number of nodes in the cluster.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
}

impl Drop for TestCluster {
    fn drop(&mut self) {
        for node in &mut self.nodes {
            if let Some(ref mut n) = node {
                let _ = n.child.kill();
                let _ = n.child.wait();
            }
        }
    }
}

fn start_cluster_node(
    index: usize,
    client_port: u16,
    cluster_port: u16,
    seeds: &[&str],
) -> ClusterNode {
    let data_dir = tempfile::tempdir().expect("create temp dir");
    write_cluster_config(&data_dir, index, client_port, cluster_port, seeds);
    spawn_and_wait(data_dir, client_port)
}

fn restart_cluster_node(
    index: usize,
    client_port: u16,
    cluster_port: u16,
    seeds: &[&str],
    data_dir: tempfile::TempDir,
) -> ClusterNode {
    // Rewrite config (it's already there but let's be safe)
    write_cluster_config(&data_dir, index, client_port, cluster_port, seeds);
    spawn_and_wait(data_dir, client_port)
}

fn write_cluster_config(
    data_dir: &tempfile::TempDir,
    index: usize,
    client_port: u16,
    cluster_port: u16,
    seeds: &[&str],
) {
    let node_id = index as u64 + 1; // node IDs are 1-based
    let addr = format!("127.0.0.1:{client_port}");
    let bind_addr = format!("127.0.0.1:{cluster_port}");

    let peers_toml = if seeds.is_empty() {
        "peers = []".to_string()
    } else {
        let items: Vec<String> = seeds.iter().map(|s| format!("\"{}\"", s)).collect();
        format!("peers = [{}]", items.join(", "))
    };

    let config = format!(
        r#"[server]
listen_addr = "{addr}"

[telemetry]
otlp_endpoint = ""

[cluster]
enabled = true
node_id = {node_id}
bind_addr = "{bind_addr}"
{peers_toml}
election_timeout_ms = 500
heartbeat_interval_ms = 150
"#
    );

    let config_path = data_dir.path().join("fila.toml");
    std::fs::write(&config_path, config).expect("write cluster config");
}

fn spawn_and_wait(data_dir: tempfile::TempDir, client_port: u16) -> ClusterNode {
    let addr_str = format!("127.0.0.1:{client_port}");
    let binary = server_binary();
    assert!(
        binary.exists(),
        "fila-server binary not found at {binary:?}. Run `cargo build` first."
    );

    let data_path = data_dir.path().join("data");
    let mut child = Command::new(&binary)
        .env("FILA_DATA_DIR", &data_path)
        .current_dir(data_dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start fila-server cluster node");

    // Drain stdout and stderr
    let stdout = child.stdout.take().expect("stdout");
    std::thread::spawn(move || for _ in BufReader::new(stdout).lines() {});
    let stderr = child.stderr.take().expect("stderr");
    std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});

    // Poll TCP until reachable
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < Duration::from_secs(15) {
        if std::net::TcpStream::connect(&addr_str).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        connected,
        "cluster node did not become reachable at {addr_str} within 15s"
    );

    ClusterNode {
        child,
        addr: format!("http://{addr_str}"),
        data_dir,
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

fn server_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push("fila-server");
    path
}
