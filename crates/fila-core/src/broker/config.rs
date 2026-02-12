use serde::Deserialize;

/// Top-level broker configuration, deserializable from TOML.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BrokerConfig {
    pub server: ServerConfig,
    pub scheduler: SchedulerConfig,
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
    }

    #[test]
    fn toml_parsing_with_overrides() {
        let toml_str = r#"
            [server]
            listen_addr = "127.0.0.1:9999"

            [scheduler]
            command_channel_capacity = 500
            idle_timeout_ms = 50
        "#;
        let config: BrokerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.listen_addr, "127.0.0.1:9999");
        assert_eq!(config.scheduler.command_channel_capacity, 500);
        assert_eq!(config.scheduler.idle_timeout_ms, 50);
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
}
