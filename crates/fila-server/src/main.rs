mod admin_service;
mod error;
mod service;
mod trace_context;

use std::path::Path;
use std::sync::Arc;

use fila_core::{Broker, BrokerConfig, RocksDbEngine};
use fila_proto::fila_admin_server::FilaAdminServer;
use fila_proto::fila_service_server::FilaServiceServer;
use tonic::transport::Server;
use tracing::info;

use admin_service::AdminService;
use service::HotPathService;

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
    let config = load_config();

    // Initialize telemetry (logging + optional OTel export).
    // Must happen after config is loaded but before anything else.
    let telemetry_guard = fila_core::telemetry::init_telemetry(&config.telemetry);

    let listen_addr = config.server.listen_addr.clone();

    let data_dir = std::env::var("FILA_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let storage = Arc::new(RocksDbEngine::open(&data_dir)?);

    // Conditionally start cluster manager (Raft consensus).
    let cluster_config = config.cluster.clone();
    let (meta_event_tx, meta_event_rx) = tokio::sync::mpsc::unbounded_channel();
    let cluster_manager = if cluster_config.enabled {
        Some(
            fila_core::cluster::ClusterManager::start(
                &cluster_config,
                Arc::clone(&storage) as _,
                Some(meta_event_tx),
            )
            .await?,
        )
    } else {
        None
    };

    let cluster_handle = cluster_manager.as_ref().map(|cm| cm.handle());

    let broker = Arc::new(Broker::new(config, storage)?);

    // Wire cluster ↔ broker integration:
    // 1. Give the cluster gRPC service access to the Broker so forwarded
    //    writes can be applied to the leader's local scheduler.
    // 2. Start meta event handler for queue group lifecycle.
    if let Some(ref cm) = cluster_manager {
        cm.set_broker(Arc::clone(&broker));
        let broker_for_events = Arc::clone(&broker);
        let multi_raft = Arc::clone(cm.multi_raft());
        tokio::spawn(fila_core::cluster::process_meta_events(
            meta_event_rx,
            broker_for_events,
            multi_raft,
        ));
    }

    let admin_service = AdminService::new(Arc::clone(&broker), cluster_handle.clone());
    let hot_path_service = HotPathService::new(Arc::clone(&broker), cluster_handle);

    let addr = listen_addr.parse()?;
    info!(%addr, "starting gRPC server");

    let serve_result = Server::builder()
        .layer(trace_context::TraceContextLayer)
        .add_service(FilaAdminServer::new(admin_service))
        .add_service(FilaServiceServer::new(hot_path_service))
        .serve_with_shutdown(addr, shutdown_signal())
        .await;

    info!("gRPC server stopped, shutting down broker");

    // Graceful broker shutdown — Drop impl will handle it since Arc may have refs
    drop(broker);

    // Shut down Raft before exiting (both success and error paths).
    if let Some(cm) = cluster_manager {
        cm.shutdown().await;
    }

    serve_result?;

    // Flush OTel pipeline (spans + metrics) before exit
    if let Some(guard) = telemetry_guard {
        guard.shutdown();
    }

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
