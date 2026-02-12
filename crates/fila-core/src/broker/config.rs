use serde::Deserialize;

/// Top-level broker configuration, deserializable from TOML.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BrokerConfig {
    pub server: ServerConfig,
    pub scheduler: SchedulerConfig,
    pub lua: LuaConfig,
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
}
