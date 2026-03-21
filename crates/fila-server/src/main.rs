mod admin_service;
mod auth;
mod error;
mod service;
mod trace_context;

use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use fila_core::{Broker, BrokerConfig, RocksDbEngine, TlsParams};
use fila_proto::fila_admin_server::FilaAdminServer;
use fila_proto::fila_service_server::FilaServiceServer;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tracing::info;

#[derive(Parser)]
#[command(name = "fila-server", version, about = "Fila message broker server")]
struct Args {}

use admin_service::AdminService;
use service::HotPathService;

/// Build a `ServerTlsConfig` from `TlsParams`.
/// `ca_file` present → mTLS (verify client certs); absent → server-TLS only.
async fn load_server_tls(tls: &TlsParams) -> Result<ServerTlsConfig, Box<dyn std::error::Error>> {
    let cert_pem = tokio::fs::read(&tls.cert_file).await?;
    let key_pem = tokio::fs::read(&tls.key_file).await?;
    let identity = Identity::from_pem(cert_pem, key_pem);
    let mut server_tls = ServerTlsConfig::new().identity(identity);
    if let Some(ref ca_file) = tls.ca_file {
        let ca_pem = tokio::fs::read(ca_file).await?;
        server_tls = server_tls.client_ca_root(Certificate::from_pem(ca_pem));
    }
    Ok(server_tls)
}

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
    // Parse CLI args (handles --version, --help).
    let _args = Args::parse();

    let mut config = load_config();

    // Initialize telemetry (logging + optional OTel export).
    // Must happen after config is loaded but before anything else.
    let telemetry_guard = fila_core::telemetry::init_telemetry(&config.telemetry);

    let listen_addr = config.server.listen_addr.clone();

    // FILA_BOOTSTRAP_APIKEY overrides (or sets) the bootstrap key from config.
    // Setting the env var also implicitly enables auth when no [auth] section exists.
    if let Ok(key) = std::env::var("FILA_BOOTSTRAP_APIKEY") {
        match &mut config.auth {
            Some(auth) => auth.bootstrap_apikey = key,
            auth @ None => {
                *auth = Some(fila_core::AuthConfig {
                    bootstrap_apikey: key,
                })
            }
        }
    }

    let data_dir = std::env::var("FILA_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let rocksdb = Arc::new(RocksDbEngine::open(&data_dir)?);
    let storage: Arc<dyn fila_core::StorageEngine> = Arc::clone(&rocksdb) as _;

    // Conditionally start cluster manager (Raft consensus).
    let cluster_config = config.cluster.clone();
    let tls_params = config.tls.clone();
    let (meta_event_tx, meta_event_rx) = tokio::sync::mpsc::unbounded_channel();
    let cluster_manager = if cluster_config.enabled {
        Some(
            fila_core::cluster::ClusterManager::start(
                &cluster_config,
                Arc::clone(&rocksdb) as _,
                Arc::clone(&storage),
                Some(meta_event_tx),
                tls_params.as_ref(),
            )
            .await?,
        )
    } else {
        None
    };

    let cluster_handle = cluster_manager.as_ref().map(|cm| cm.handle());

    let broker = Arc::new(Broker::new(config, Arc::clone(&storage))?);

    // Wire cluster ↔ broker integration:
    // 1. Give the cluster gRPC service access to the Broker so forwarded
    //    writes can be applied to the leader's local scheduler.
    // 2. Start meta event handler for queue group lifecycle.
    // 3. Start leader change watcher for failover (recover queue / drop consumers).
    let (leader_watch_shutdown_tx, leader_watch_shutdown_rx) = tokio::sync::watch::channel(false);
    if let Some(ref cm) = cluster_manager {
        cm.set_broker(Arc::clone(&broker));
        let broker_for_events = Arc::clone(&broker);
        let multi_raft = Arc::clone(cm.multi_raft());
        tokio::spawn(fila_core::cluster::process_meta_events(
            meta_event_rx,
            broker_for_events,
            multi_raft,
        ));

        let broker_for_watcher = Arc::clone(&broker);
        let multi_raft_for_watcher = Arc::clone(cm.multi_raft());
        let node_id = cluster_config.node_id;
        tokio::spawn(fila_core::cluster::watch_leader_changes(
            node_id,
            multi_raft_for_watcher,
            broker_for_watcher,
            std::time::Duration::from_millis(200),
            leader_watch_shutdown_rx,
        ));
    }

    let admin_service = AdminService::new(Arc::clone(&broker), cluster_handle.clone());
    let hot_path_service = HotPathService::new(Arc::clone(&broker), cluster_handle);

    let addr = listen_addr.parse()?;
    info!(%addr, tls = tls_params.is_some(), "starting gRPC server");

    let mut server_builder = Server::builder();
    if let Some(ref tls) = tls_params {
        let server_tls = load_server_tls(tls).await?;
        server_builder = server_builder.tls_config(server_tls)?;
    }

    // Layer order: last `.layer()` becomes outermost (first to receive requests).
    // AuthLayer must be inner so auth runs within the trace context span.
    let serve_result = server_builder
        .layer(auth::AuthLayer::new(Arc::clone(&broker)))
        .layer(trace_context::TraceContextLayer)
        .add_service(FilaAdminServer::new(admin_service))
        .add_service(FilaServiceServer::new(hot_path_service))
        .serve_with_shutdown(addr, shutdown_signal())
        .await;

    info!("gRPC server stopped, shutting down broker");

    // Graceful broker shutdown — Drop impl will handle it since Arc may have refs
    drop(broker);

    // Shut down leader change watcher and Raft before exiting.
    let _ = leader_watch_shutdown_tx.send(true);
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
