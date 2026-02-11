mod admin_service;
mod error;

use std::path::Path;
use std::sync::Arc;

use fila_core::{Broker, BrokerConfig, RocksDbStorage};
use fila_proto::fila_admin_server::FilaAdminServer;
use tonic::transport::Server;
use tracing::info;

use admin_service::AdminService;

fn load_config() -> BrokerConfig {
    let paths = ["fila.toml", "/etc/fila/fila.toml"];

    for path in &paths {
        if Path::new(path).exists() {
            match std::fs::read_to_string(path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(config) => {
                        info!(path, "loaded configuration");
                        return config;
                    }
                    Err(e) => {
                        eprintln!("error parsing {path}: {e}");
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("error reading {path}: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    info!("no config file found, using defaults");
    BrokerConfig::default()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fila_core::telemetry::init_tracing();

    let config = load_config();
    let listen_addr = config.server.listen_addr.clone();

    let data_dir = std::env::var("FILA_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let storage = Arc::new(RocksDbStorage::open(&data_dir)?);

    let broker = Arc::new(Broker::new(config, storage)?);

    let admin_service = AdminService::new(Arc::clone(&broker));

    let addr = listen_addr.parse()?;
    info!(%addr, "starting gRPC server");

    Server::builder()
        .add_service(FilaAdminServer::new(admin_service))
        .serve_with_shutdown(addr, shutdown_signal())
        .await?;

    info!("gRPC server stopped, shutting down broker");

    // Graceful broker shutdown — Drop impl will handle it since Arc may have refs
    drop(broker);

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to install CTRL+C handler");
    }

    info!("received shutdown signal");
}
