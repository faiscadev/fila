use tracing_subscriber::EnvFilter;

/// Initialize the tracing subscriber for structured logging.
///
/// - Debug builds: pretty-printed human-readable output
/// - Release builds: JSON-formatted output for log aggregation
///
/// The log level is controlled by the `RUST_LOG` environment variable,
/// defaulting to `info`.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if cfg!(debug_assertions) {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    }
}
