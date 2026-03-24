use serde::Deserialize;

/// Top-level broker configuration, deserializable from TOML.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BrokerConfig {
    pub server: ServerConfig,
    pub scheduler: SchedulerConfig,
    pub lua: LuaConfig,
    pub telemetry: TelemetryConfig,
    pub cluster: ClusterConfig,
    pub storage: StorageConfig,
    pub grpc: GrpcConfig,
    /// TLS configuration. `None` (the default) disables TLS entirely.
    /// Presence of this section in `fila.toml` enables TLS.
    pub tls: Option<TlsParams>,
    /// Authentication configuration. `None` (the default) disables authentication.
    /// Presence of this section in `fila.toml` enables API key authentication.
    pub auth: Option<AuthConfig>,
    /// Web GUI configuration. `None` (the default) disables the web GUI.
    /// Presence of this section in `fila.toml` enables the read-only management dashboard.
    pub gui: Option<GuiConfig>,
}

/// Web management GUI configuration. Presence in `BrokerConfig.gui` (i.e. a `[gui]` section
/// in `fila.toml`) enables the read-only web dashboard; absence disables it with zero overhead.
#[derive(Debug, Clone, Deserialize)]
pub struct GuiConfig {
    /// HTTP listen address for the web GUI (default: "0.0.0.0:8080").
    #[serde(default = "default_gui_listen_addr")]
    pub listen_addr: String,
}

fn default_gui_listen_addr() -> String {
    "0.0.0.0:8080".to_string()
}

/// Cluster configuration for Raft-based horizontal scaling.
///
/// When `enabled` is false (the default), Fila runs as a standalone
/// single-node broker with zero Raft overhead.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ClusterConfig {
    /// Enable cluster mode. When false, no Raft infrastructure is created.
    pub enabled: bool,
    /// Unique numeric node ID within the cluster.
    pub node_id: u64,
    /// Addresses of seed peers for cluster discovery (e.g., `["node1:5556", "node2:5556"]`).
    /// When empty, this node bootstraps a new single-node cluster.
    /// When non-empty, the node joins an existing cluster by contacting these peers.
    pub peers: Vec<String>,
    /// Listen address for intra-cluster gRPC communication (separate from client port).
    pub bind_addr: String,
    /// Raft election timeout in milliseconds.
    pub election_timeout_ms: u64,
    /// Raft heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,
    /// Number of applied log entries before triggering a snapshot.
    pub snapshot_threshold: u64,
    /// Number of nodes that participate in each queue's Raft group.
    /// When the cluster has more nodes than this value, only a subset of
    /// nodes are selected for each queue (the least-loaded ones).
    /// When the cluster has fewer or equal nodes, all nodes participate.
    /// Default: 3.
    pub replication_factor: usize,
}

/// TLS parameters. Presence in `BrokerConfig.tls` (i.e. a `[tls]` section in
/// `fila.toml`) enables TLS; absence disables it entirely.
///
/// `cert_file` and `key_file` are always required — they are the server
/// identity. `ca_file` is optional: when set, client certificates are
/// verified against it (mTLS); when absent, the server uses one-way TLS.
#[derive(Debug, Clone, Deserialize)]
pub struct TlsParams {
    /// Path to the server's PEM-encoded certificate chain.
    pub cert_file: String,
    /// Path to the server's PEM-encoded private key.
    pub key_file: String,
    /// Path to the CA certificate used to verify client certificates (mTLS).
    /// When absent, client certificates are not required (server-TLS only).
    pub ca_file: Option<String>,
}

/// Authentication configuration. Presence in `BrokerConfig.auth` (i.e. an `[auth]` section
/// in `fila.toml`) enables API key authentication; absence disables it entirely.
///
/// When enabled, every gRPC RPC must include `authorization: Bearer <key>` metadata.
/// `bootstrap_apikey` is required: it is a permanent operator key accepted without a storage
/// lookup, enabling operators to provision the first real API key. It can also be set (or
/// overridden) via the `FILA_BOOTSTRAP_APIKEY` environment variable.
///
/// It is impossible to enable auth without a `bootstrap_apikey` — the type enforces this.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// A permanent operator key accepted without storage lookup.
    /// Required when auth is enabled. Use it to provision the first real API key,
    /// then keep it as a recovery backdoor or rotate it to a long-lived secret.
    /// Can be overridden via the `FILA_BOOTSTRAP_APIKEY` environment variable.
    pub bootstrap_apikey: String,
}

/// Server configuration (gRPC listen address).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub listen_addr: String,
}

/// Scheduler configuration (channel capacity, idle timeout, DRR quantum).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    pub command_channel_capacity: usize,
    pub idle_timeout_ms: u64,
    /// DRR quantum — each fairness key receives `weight * quantum` deficit
    /// per scheduling round. Higher values mean fewer round resets but
    /// coarser fairness granularity.
    pub quantum: u32,
    /// Maximum number of commands to drain from the channel per coalescing
    /// batch. When multiple enqueue commands arrive concurrently, their
    /// storage writes are coalesced into a single RocksDB WriteBatch.
    /// Default: 100.
    pub write_coalesce_max_batch: usize,
}

/// Lua scripting safety configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LuaConfig {
    /// Default script execution timeout in milliseconds (per queue, overridable).
    /// Enforced via instruction count hook (approximate).
    pub default_timeout_ms: u64,
    /// Default script memory limit in bytes (per queue, overridable).
    pub default_memory_limit_bytes: usize,
    /// Number of consecutive Lua failures before the circuit breaker trips.
    pub circuit_breaker_threshold: u32,
    /// Cooldown period in milliseconds after circuit breaker trips before
    /// retrying Lua execution.
    pub circuit_breaker_cooldown_ms: u64,
}

/// Telemetry configuration (OpenTelemetry export).
/// When `otlp_endpoint` is `None`, telemetry export is disabled and the broker
/// uses plain tracing-subscriber logging only.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// OTLP gRPC endpoint (e.g., "http://localhost:4317").
    /// When absent, no metrics or traces are exported.
    pub otlp_endpoint: Option<String>,
    /// OTel service name reported in traces and metrics.
    pub service_name: Option<String>,
    /// Metrics export interval in milliseconds.
    pub metrics_interval_ms: Option<u64>,
}

impl TelemetryConfig {
    /// Resolved service name, defaulting to "fila".
    pub fn service_name(&self) -> &str {
        self.service_name.as_deref().unwrap_or("fila")
    }

    /// Resolved metrics interval, defaulting to 10 seconds.
    pub fn metrics_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.metrics_interval_ms.unwrap_or(10_000))
    }
}

/// Storage engine configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub rocksdb: RocksDbConfig,
}

/// RocksDB tuning parameters. All defaults are optimized for queue workloads
/// (write-heavy, sequential reads, frequent deletes).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RocksDbConfig {
    /// Shared LRU block cache size in megabytes (default: 256).
    pub block_cache_mb: usize,
    /// Messages CF write buffer size in megabytes (default: 128).
    pub messages_write_buffer_mb: usize,
    /// Max write buffers for messages CF (default: 4).
    pub messages_max_write_buffers: i32,
    /// Min write buffers to merge for messages CF (default: 2).
    pub messages_min_write_buffers_to_merge: i32,
    /// Leases CF write buffer size in megabytes (default: 64).
    pub leases_write_buffer_mb: usize,
    /// Enable pipelined writes (default: true).
    pub pipelined_write: bool,
    /// Enable manual WAL flush with buffering (default: false).
    /// When true, WAL entries are buffered in memory and a crash may lose
    /// unflushed writes. Only enable in cluster mode where Raft provides
    /// durability.
    pub manual_wal_flush: bool,
    /// WAL bytes per sync in bytes (default: 524288 = 512KB).
    pub wal_bytes_per_sync: u64,
    /// Enable CompactOnDeletionCollector (default: true).
    pub compact_on_deletion: bool,
    /// Bloom filter bits per key (default: 10). Set to 0 to disable.
    pub bloom_filter_bits: i32,
}

/// gRPC transport tuning parameters.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GrpcConfig {
    /// Initial HTTP/2 stream window size in bytes (default: 2MB).
    pub initial_stream_window_size: u32,
    /// Initial HTTP/2 connection window size in bytes (default: 4MB).
    pub initial_connection_window_size: u32,
    /// Enable TCP_NODELAY (default: true).
    pub tcp_nodelay: bool,
    /// HTTP/2 keepalive interval in seconds (default: 15).
    pub keepalive_interval_secs: u64,
    /// HTTP/2 keepalive timeout in seconds (default: 10).
    pub keepalive_timeout_secs: u64,
}

impl Default for RocksDbConfig {
    fn default() -> Self {
        Self {
            block_cache_mb: 256,
            messages_write_buffer_mb: 128,
            messages_max_write_buffers: 4,
            messages_min_write_buffers_to_merge: 2,
            leases_write_buffer_mb: 64,
            pipelined_write: true,
            manual_wal_flush: false,
            wal_bytes_per_sync: 524_288,
            compact_on_deletion: true,
            bloom_filter_bits: 10,
        }
    }
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            initial_stream_window_size: 2 * 1024 * 1024,
            initial_connection_window_size: 4 * 1024 * 1024,
            tcp_nodelay: true,
            keepalive_interval_secs: 15,
            keepalive_timeout_secs: 10,
        }
    }
}

impl Default for LuaConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 10,
            default_memory_limit_bytes: 1_048_576, // 1 MB
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown_ms: 10_000,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:5555".to_string(),
        }
    }
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_id: 1,
            peers: Vec::new(),
            bind_addr: "0.0.0.0:5556".to_string(),
            election_timeout_ms: 1000,
            heartbeat_interval_ms: 300,
            snapshot_threshold: 10_000,
            replication_factor: 3,
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            command_channel_capacity: 10_000,
            idle_timeout_ms: 100,
            quantum: 1000,
            write_coalesce_max_batch: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = BrokerConfig::default();
        assert_eq!(config.server.listen_addr, "0.0.0.0:5555");
        assert_eq!(config.scheduler.command_channel_capacity, 10_000);
        assert_eq!(config.scheduler.idle_timeout_ms, 100);
        assert_eq!(config.scheduler.quantum, 1000);
        assert_eq!(config.scheduler.write_coalesce_max_batch, 100);
        assert_eq!(config.lua.default_timeout_ms, 10);
        assert_eq!(config.lua.default_memory_limit_bytes, 1_048_576);
        assert_eq!(config.lua.circuit_breaker_threshold, 3);
        assert_eq!(config.lua.circuit_breaker_cooldown_ms, 10_000);
    }

    #[test]
    fn toml_parsing_with_overrides() {
        let toml_str = r#"
            [server]
            listen_addr = "127.0.0.1:9999"

            [scheduler]
            command_channel_capacity = 500
            idle_timeout_ms = 50
            quantum = 500
            write_coalesce_max_batch = 200

            [lua]
            default_timeout_ms = 20
            default_memory_limit_bytes = 524288
            circuit_breaker_threshold = 5
            circuit_breaker_cooldown_ms = 30000
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.listen_addr, "127.0.0.1:9999");
        assert_eq!(config.scheduler.command_channel_capacity, 500);
        assert_eq!(config.scheduler.idle_timeout_ms, 50);
        assert_eq!(config.scheduler.quantum, 500);
        assert_eq!(config.scheduler.write_coalesce_max_batch, 200);
        assert_eq!(config.lua.default_timeout_ms, 20);
        assert_eq!(config.lua.default_memory_limit_bytes, 524288);
        assert_eq!(config.lua.circuit_breaker_threshold, 5);
        assert_eq!(config.lua.circuit_breaker_cooldown_ms, 30000);
    }

    #[test]
    fn toml_parsing_empty_uses_defaults() {
        let config: BrokerConfig = toml::from_str("").unwrap();
        assert_eq!(config.server.listen_addr, "0.0.0.0:5555");
        assert_eq!(config.scheduler.command_channel_capacity, 10_000);
        assert_eq!(config.scheduler.idle_timeout_ms, 100);
    }

    #[test]
    fn toml_parsing_partial_config() {
        let toml_str = r#"
            [server]
            listen_addr = "0.0.0.0:8080"
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.listen_addr, "0.0.0.0:8080");
        // Scheduler defaults preserved
        assert_eq!(config.scheduler.command_channel_capacity, 10_000);
    }

    #[test]
    fn telemetry_defaults_to_disabled() {
        let config = BrokerConfig::default();
        assert!(config.telemetry.otlp_endpoint.is_none());
        assert!(config.telemetry.service_name.is_none());
        assert!(config.telemetry.metrics_interval_ms.is_none());
        assert_eq!(config.telemetry.service_name(), "fila");
        assert_eq!(
            config.telemetry.metrics_interval(),
            std::time::Duration::from_millis(10_000)
        );
    }

    #[test]
    fn telemetry_toml_parsing() {
        let toml_str = r#"
            [telemetry]
            otlp_endpoint = "http://localhost:4317"
            service_name = "fila-prod"
            metrics_interval_ms = 5000
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.telemetry.otlp_endpoint.as_deref(),
            Some("http://localhost:4317")
        );
        assert_eq!(config.telemetry.service_name(), "fila-prod");
        assert_eq!(
            config.telemetry.metrics_interval(),
            std::time::Duration::from_millis(5_000)
        );
    }

    #[test]
    fn cluster_defaults() {
        let config = BrokerConfig::default();
        assert!(!config.cluster.enabled);
        assert_eq!(config.cluster.node_id, 1);
        assert!(config.cluster.peers.is_empty());
        assert_eq!(config.cluster.bind_addr, "0.0.0.0:5556");
        assert_eq!(config.cluster.election_timeout_ms, 1000);
        assert_eq!(config.cluster.heartbeat_interval_ms, 300);
        assert_eq!(config.cluster.snapshot_threshold, 10_000);
    }

    #[test]
    fn cluster_toml_parsing() {
        let toml_str = r#"
            [cluster]
            enabled = true
            node_id = 3
            peers = ["node1:5556", "node2:5556"]
            bind_addr = "0.0.0.0:6000"
            election_timeout_ms = 2000
            heartbeat_interval_ms = 500
            snapshot_threshold = 5000
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert!(config.cluster.enabled);
        assert_eq!(config.cluster.node_id, 3);
        assert_eq!(config.cluster.peers, vec!["node1:5556", "node2:5556"]);
        assert_eq!(config.cluster.bind_addr, "0.0.0.0:6000");
        assert_eq!(config.cluster.election_timeout_ms, 2000);
        assert_eq!(config.cluster.heartbeat_interval_ms, 500);
        assert_eq!(config.cluster.snapshot_threshold, 5000);
    }

    #[test]
    fn cluster_absent_section_uses_defaults() {
        let config: BrokerConfig = toml::from_str("").unwrap();
        assert!(!config.cluster.enabled);
        assert_eq!(config.cluster.node_id, 1);
    }

    #[test]
    fn telemetry_absent_section_uses_defaults() {
        let toml_str = r#"
            [server]
            listen_addr = "0.0.0.0:8080"
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert!(config.telemetry.otlp_endpoint.is_none());
    }

    #[test]
    fn tls_absent_means_disabled() {
        let config: BrokerConfig = toml::from_str("").unwrap();
        assert!(config.tls.is_none());
    }

    #[test]
    fn tls_server_only_parsing() {
        let toml_str = r#"
            [tls]
            cert_file = "server.crt"
            key_file = "server.key"
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        let tls = config.tls.as_ref().unwrap();
        assert_eq!(tls.cert_file, "server.crt");
        assert_eq!(tls.key_file, "server.key");
        assert!(tls.ca_file.is_none());
    }

    #[test]
    fn tls_mtls_parsing() {
        let toml_str = r#"
            [tls]
            cert_file = "server.crt"
            key_file = "server.key"
            ca_file = "ca.crt"
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        let tls = config.tls.as_ref().unwrap();
        assert_eq!(tls.cert_file, "server.crt");
        assert_eq!(tls.key_file, "server.key");
        assert_eq!(tls.ca_file.as_deref(), Some("ca.crt"));
    }

    #[test]
    fn storage_defaults() {
        let config = BrokerConfig::default();
        assert_eq!(config.storage.rocksdb.block_cache_mb, 256);
        assert_eq!(config.storage.rocksdb.messages_write_buffer_mb, 128);
        assert_eq!(config.storage.rocksdb.messages_max_write_buffers, 4);
        assert_eq!(
            config.storage.rocksdb.messages_min_write_buffers_to_merge,
            2
        );
        assert_eq!(config.storage.rocksdb.leases_write_buffer_mb, 64);
        assert!(config.storage.rocksdb.pipelined_write);
        assert!(!config.storage.rocksdb.manual_wal_flush);
        assert_eq!(config.storage.rocksdb.wal_bytes_per_sync, 524_288);
        assert!(config.storage.rocksdb.compact_on_deletion);
        assert_eq!(config.storage.rocksdb.bloom_filter_bits, 10);
    }

    #[test]
    fn storage_toml_parsing() {
        let toml_str = r#"
            [storage.rocksdb]
            block_cache_mb = 512
            messages_write_buffer_mb = 256
            messages_max_write_buffers = 8
            messages_min_write_buffers_to_merge = 4
            leases_write_buffer_mb = 128
            pipelined_write = false
            manual_wal_flush = false
            wal_bytes_per_sync = 1048576
            compact_on_deletion = false
            bloom_filter_bits = 16
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.storage.rocksdb.block_cache_mb, 512);
        assert_eq!(config.storage.rocksdb.messages_write_buffer_mb, 256);
        assert_eq!(config.storage.rocksdb.messages_max_write_buffers, 8);
        assert_eq!(
            config.storage.rocksdb.messages_min_write_buffers_to_merge,
            4
        );
        assert_eq!(config.storage.rocksdb.leases_write_buffer_mb, 128);
        assert!(!config.storage.rocksdb.pipelined_write);
        assert!(!config.storage.rocksdb.manual_wal_flush);
        assert_eq!(config.storage.rocksdb.wal_bytes_per_sync, 1_048_576);
        assert!(!config.storage.rocksdb.compact_on_deletion);
        assert_eq!(config.storage.rocksdb.bloom_filter_bits, 16);
    }

    #[test]
    fn storage_absent_section_uses_defaults() {
        let config: BrokerConfig = toml::from_str("").unwrap();
        assert_eq!(config.storage.rocksdb.block_cache_mb, 256);
        assert!(config.storage.rocksdb.pipelined_write);
    }

    #[test]
    fn grpc_defaults() {
        let config = BrokerConfig::default();
        assert_eq!(config.grpc.initial_stream_window_size, 2 * 1024 * 1024);
        assert_eq!(config.grpc.initial_connection_window_size, 4 * 1024 * 1024);
        assert!(config.grpc.tcp_nodelay);
        assert_eq!(config.grpc.keepalive_interval_secs, 15);
        assert_eq!(config.grpc.keepalive_timeout_secs, 10);
    }

    #[test]
    fn grpc_toml_parsing() {
        let toml_str = r#"
            [grpc]
            initial_stream_window_size = 4194304
            initial_connection_window_size = 8388608
            tcp_nodelay = false
            keepalive_interval_secs = 30
            keepalive_timeout_secs = 20
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.grpc.initial_stream_window_size, 4 * 1024 * 1024);
        assert_eq!(config.grpc.initial_connection_window_size, 8 * 1024 * 1024);
        assert!(!config.grpc.tcp_nodelay);
        assert_eq!(config.grpc.keepalive_interval_secs, 30);
        assert_eq!(config.grpc.keepalive_timeout_secs, 20);
    }

    #[test]
    fn grpc_absent_section_uses_defaults() {
        let config: BrokerConfig = toml::from_str("").unwrap();
        assert!(config.grpc.tcp_nodelay);
        assert_eq!(config.grpc.keepalive_interval_secs, 15);
    }
}
