use std::path::PathBuf;

use serde::Deserialize;

/// Top-level broker configuration, deserializable from TOML.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BrokerConfig {
    pub server: ServerConfig,
    pub scheduler: SchedulerConfig,
    pub lua: LuaConfig,
    pub telemetry: TelemetryConfig,
    pub storage: StorageConfig,
}

/// Storage engine selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageEngine {
    Fila,
    Rocksdb,
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::Fila
    }
}

/// Storage configuration section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Which storage engine to use: "fila" (default) or "rocksdb" (legacy).
    pub engine: StorageEngine,
    /// Data directory for storage files. Overridden by FILA_DATA_DIR env var.
    pub data_dir: String,
    /// Maximum WAL segment size in bytes (Fila engine only).
    pub segment_size_bytes: u64,
    /// Whether background compaction is enabled (Fila engine only).
    pub compaction_enabled: bool,
    /// Interval between compaction passes in seconds (Fila engine only).
    pub compaction_interval_secs: u64,
    /// Maximum I/O rate for compaction writes in bytes/sec (Fila engine only).
    pub compaction_io_rate_bytes_per_sec: u64,
    /// Message TTL in milliseconds (Fila engine only). 0 = no TTL.
    pub message_ttl_ms: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            engine: StorageEngine::default(),
            data_dir: "data".to_string(),
            segment_size_bytes: 64 * 1024 * 1024,
            compaction_enabled: true,
            compaction_interval_secs: 60,
            compaction_io_rate_bytes_per_sec: 10 * 1024 * 1024,
            message_ttl_ms: 0,
        }
    }
}

impl StorageConfig {
    /// Resolved data directory, preferring FILA_DATA_DIR env var over config.
    pub fn data_dir(&self) -> PathBuf {
        std::env::var("FILA_DATA_DIR")
            .unwrap_or_else(|_| self.data_dir.clone())
            .into()
    }

    /// Resolved storage engine, preferring FILA_STORAGE_ENGINE env var over config.
    pub fn engine(&self) -> StorageEngine {
        if let Ok(val) = std::env::var("FILA_STORAGE_ENGINE") {
            match val.to_lowercase().as_str() {
                "rocksdb" => return StorageEngine::Rocksdb,
                "fila" => return StorageEngine::Fila,
                _ => {}
            }
        }
        self.engine
    }

    /// Build a `FilaStorageConfig` from this storage config.
    pub fn to_fila_config(&self) -> crate::storage::FilaStorageConfig {
        let mut fila_config = crate::storage::FilaStorageConfig::new(self.data_dir());
        fila_config.segment_size_bytes = self.segment_size_bytes;
        fila_config.compaction_enabled = self.compaction_enabled;
        fila_config.compaction_interval_secs = self.compaction_interval_secs;
        fila_config.compaction_io_rate_bytes_per_sec = self.compaction_io_rate_bytes_per_sec;
        fila_config.message_ttl_ms = if self.message_ttl_ms == 0 {
            None
        } else {
            Some(self.message_ttl_ms)
        };
        fila_config
    }
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

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            command_channel_capacity: 10_000,
            idle_timeout_ms: 100,
            quantum: 1000,
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
        assert_eq!(config.lua.default_timeout_ms, 10);
        assert_eq!(config.lua.default_memory_limit_bytes, 1_048_576);
        assert_eq!(config.lua.circuit_breaker_threshold, 3);
        assert_eq!(config.lua.circuit_breaker_cooldown_ms, 10_000);
        assert_eq!(config.storage.engine, StorageEngine::Fila);
        assert_eq!(config.storage.data_dir, "data");
        assert_eq!(config.storage.segment_size_bytes, 64 * 1024 * 1024);
        assert!(config.storage.compaction_enabled);
        assert_eq!(config.storage.compaction_interval_secs, 60);
        assert_eq!(
            config.storage.compaction_io_rate_bytes_per_sec,
            10 * 1024 * 1024
        );
        assert_eq!(config.storage.message_ttl_ms, 0);
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
    fn telemetry_absent_section_uses_defaults() {
        let toml_str = r#"
            [server]
            listen_addr = "0.0.0.0:8080"
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert!(config.telemetry.otlp_endpoint.is_none());
    }

    #[test]
    fn storage_toml_parsing() {
        let toml_str = r#"
            [storage]
            engine = "rocksdb"
            data_dir = "/var/lib/fila"
            segment_size_bytes = 134217728
            compaction_enabled = false
            compaction_interval_secs = 120
            compaction_io_rate_bytes_per_sec = 5242880
            message_ttl_ms = 3600000
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.storage.engine, StorageEngine::Rocksdb);
        assert_eq!(config.storage.data_dir, "/var/lib/fila");
        assert_eq!(config.storage.segment_size_bytes, 128 * 1024 * 1024);
        assert!(!config.storage.compaction_enabled);
        assert_eq!(config.storage.compaction_interval_secs, 120);
        assert_eq!(
            config.storage.compaction_io_rate_bytes_per_sec,
            5 * 1024 * 1024
        );
        assert_eq!(config.storage.message_ttl_ms, 3_600_000);
    }

    #[test]
    fn storage_absent_section_uses_defaults() {
        let config: BrokerConfig = toml::from_str("").unwrap();
        assert_eq!(config.storage.engine, StorageEngine::Fila);
        assert!(config.storage.compaction_enabled);
    }

    #[test]
    fn to_fila_config_converts_correctly() {
        let mut storage = StorageConfig::default();
        storage.segment_size_bytes = 32 * 1024 * 1024;
        storage.compaction_enabled = false;
        storage.compaction_interval_secs = 30;
        storage.message_ttl_ms = 5000;

        let fila = storage.to_fila_config();
        assert_eq!(fila.segment_size_bytes, 32 * 1024 * 1024);
        assert!(!fila.compaction_enabled);
        assert_eq!(fila.compaction_interval_secs, 30);
        assert_eq!(fila.message_ttl_ms, Some(5000));
    }

    #[test]
    fn to_fila_config_zero_ttl_maps_to_none() {
        let storage = StorageConfig::default();
        assert_eq!(storage.message_ttl_ms, 0);
        let fila = storage.to_fila_config();
        assert_eq!(fila.message_ttl_ms, None);
    }
}
