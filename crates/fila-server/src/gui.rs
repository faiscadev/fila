//! Web management GUI — optional HTTP server serving a read-only dashboard.
//!
//! When `[gui]` is present in `fila.toml`, an axum HTTP server starts alongside
//! the gRPC server, serving a single-page dashboard and JSON API endpoints.
//!
//! The GUI is read-only: no create/delete/config operations. It queries the
//! broker's scheduler via the same command channel used by the gRPC admin service.

use std::sync::Arc;

use axum::{
    extract::Path,
    extract::State,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use fila_core::{Broker, QueueSummary, SchedulerCommand};
use serde::Serialize;
use tracing::info;

/// Shared state for GUI HTTP handlers.
#[derive(Clone)]
pub struct GuiState {
    broker: Arc<Broker>,
}

/// JSON response for the queue list.
#[derive(Serialize)]
struct QueueListResponse {
    queues: Vec<QueueSummaryJson>,
}

#[derive(Serialize)]
struct QueueSummaryJson {
    name: String,
    depth: u64,
    in_flight: u64,
    active_consumers: u32,
    leader_node_id: u64,
}

impl From<QueueSummary> for QueueSummaryJson {
    fn from(q: QueueSummary) -> Self {
        Self {
            name: q.name,
            depth: q.depth,
            in_flight: q.in_flight,
            active_consumers: q.active_consumers,
            leader_node_id: q.leader_node_id,
        }
    }
}

/// JSON response for per-queue stats.
#[derive(Serialize)]
struct QueueStatsResponse {
    depth: u64,
    in_flight: u64,
    active_fairness_keys: u64,
    active_consumers: u32,
    quantum: u32,
    leader_node_id: u64,
    replication_count: u32,
    per_key_stats: Vec<FairnessKeyStatsJson>,
    per_throttle_stats: Vec<ThrottleKeyStatsJson>,
}

#[derive(Serialize)]
struct FairnessKeyStatsJson {
    key: String,
    pending_count: u64,
    current_deficit: i64,
    weight: u32,
}

#[derive(Serialize)]
struct ThrottleKeyStatsJson {
    key: String,
    tokens: f64,
    rate_per_second: f64,
    burst: f64,
}

/// Build the axum router for the GUI.
pub fn router(broker: Arc<Broker>) -> Router {
    let state = GuiState { broker };

    Router::new()
        .route("/", get(index_handler))
        .route("/api/queues", get(list_queues_handler))
        .route("/api/queues/{name}/stats", get(get_stats_handler))
        .with_state(state)
}

/// Start the GUI HTTP server. Runs until the provided shutdown future completes.
pub async fn start(
    broker: Arc<Broker>,
    listen_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = router(broker);
    let addr: std::net::SocketAddr = listen_addr.parse()?;
    info!(%addr, "starting GUI HTTP server");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
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
}

/// Serve the embedded HTML dashboard.
async fn index_handler() -> impl IntoResponse {
    Html(include_str!("gui_dashboard.html"))
}

/// GET /api/queues — list all queues with summary stats.
async fn list_queues_handler(
    State(state): State<GuiState>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .broker
        .send_command(SchedulerCommand::ListQueues { reply: reply_tx })
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("scheduler error: {e}"),
            )
        })?;

    let queues = reply_rx
        .await
        .map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "scheduler dropped".to_string(),
            )
        })?
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("list queues error: {e}"),
            )
        })?;

    let response = QueueListResponse {
        queues: queues.into_iter().map(QueueSummaryJson::from).collect(),
    };

    Ok((
        [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        Json(response),
    ))
}

/// GET /api/queues/:name/stats — get detailed stats for a queue.
async fn get_stats_handler(
    State(state): State<GuiState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .broker
        .send_command(SchedulerCommand::GetStats {
            queue_id: name.clone(),
            reply: reply_tx,
        })
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("scheduler error: {e}"),
            )
        })?;

    let stats = reply_rx
        .await
        .map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "scheduler dropped".to_string(),
            )
        })?
        .map_err(|e| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("queue not found: {e}"),
            )
        })?;

    let response = QueueStatsResponse {
        depth: stats.depth,
        in_flight: stats.in_flight,
        active_fairness_keys: stats.active_fairness_keys,
        active_consumers: stats.active_consumers,
        quantum: stats.quantum,
        leader_node_id: stats.leader_node_id,
        replication_count: stats.replication_count,
        per_key_stats: stats
            .per_key_stats
            .into_iter()
            .map(|s| FairnessKeyStatsJson {
                key: s.key,
                pending_count: s.pending_count,
                current_deficit: s.current_deficit,
                weight: s.weight,
            })
            .collect(),
        per_throttle_stats: stats
            .per_throttle_stats
            .into_iter()
            .map(|s| ThrottleKeyStatsJson {
                key: s.key,
                tokens: s.tokens,
                rate_per_second: s.rate_per_second,
                burst: s.burst,
            })
            .collect(),
    };

    Ok((
        [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        Json(response),
    ))
}
