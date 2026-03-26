//! FIBP TCP listener.
//!
//! Binds a TCP socket on the configured address and spawns a
//! `FibpConnection` task for each accepted connection.

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::connection::FibpConnection;
use super::error::FibpError;
use crate::broker::config::FibpConfig;

/// TCP listener for the FIBP transport.
///
/// Call [`start()`](FibpListener::start) to bind and begin accepting
/// connections. Call [`shutdown()`](FibpListener::shutdown) to stop accepting
/// new connections and signal existing ones to drain.
pub struct FibpListener {
    shutdown_tx: watch::Sender<bool>,
    local_addr: SocketAddr,
}

impl FibpListener {
    /// Bind on the configured address and begin accepting FIBP connections.
    ///
    /// Returns immediately after binding; the accept loop runs in a spawned
    /// tokio task. Use the returned handle to query the local address and to
    /// trigger a graceful shutdown.
    pub async fn start(config: &FibpConfig) -> Result<Self, FibpError> {
        let listener = TcpListener::bind(&config.listen_addr).await?;
        let local_addr = listener.local_addr()?;
        let max_frame_size = config.max_frame_size;

        info!(addr = %local_addr, "starting FIBP listener");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(accept_loop(listener, max_frame_size, shutdown_rx));

        Ok(Self {
            shutdown_tx,
            local_addr,
        })
    }

    /// The address the listener actually bound to (useful when port 0 is used).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Signal the listener to stop accepting new connections.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Accept loop: runs until `shutdown_rx` signals `true`.
async fn accept_loop(
    listener: TcpListener,
    max_frame_size: u32,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer)) => {
                        debug!(peer = %peer, "accepted FIBP connection");
                        tokio::spawn(async move {
                            match FibpConnection::accept(stream, max_frame_size).await {
                                Ok(conn) => conn.run().await,
                                Err(e) => {
                                    warn!(peer = %peer, error = %e, "FIBP handshake failed");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "FIBP accept error");
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("FIBP listener shutting down");
                    break;
                }
            }
        }
    }
}
