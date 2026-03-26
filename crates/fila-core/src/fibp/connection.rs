//! FIBP connection handler.
//!
//! `FibpConnection` wraps a TCP stream, performs the protocol handshake, and
//! dispatches incoming frames. Data operations (enqueue, consume, etc.) are
//! stubs that return "not implemented" until Story 36.2.

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_stream::StreamExt;
use tokio_util::codec::{Encoder, Framed};
use tracing::{debug, warn};

use super::codec::{FibpCodec, Frame, OP_GOAWAY, OP_HEARTBEAT};
use super::error::FibpError;
use super::MAGIC;

/// A single FIBP client connection.
///
/// Performs the handshake and then enters a frame read/dispatch loop.
/// The connection does **not** hold references to the scheduler or storage;
/// command dispatch will be injected via a channel in Story 36.2.
#[derive(Debug)]
pub struct FibpConnection {
    framed: Framed<TcpStream, FibpCodec>,
    peer_addr: std::net::SocketAddr,
}

impl FibpConnection {
    /// Accept a new FIBP connection on `stream`.
    ///
    /// Performs the 6-byte handshake before returning. Returns `Err` if the
    /// handshake fails (version mismatch, I/O error, etc.).
    pub async fn accept(mut stream: TcpStream, max_frame_size: u32) -> Result<Self, FibpError> {
        let peer_addr = stream.peer_addr()?;
        debug!(peer = %peer_addr, "fibp handshake starting");

        // --- Handshake: read client magic, validate, echo back. ---
        let mut client_magic = [0u8; 6];
        stream.read_exact(&mut client_magic).await?;

        if client_magic[..4] != MAGIC[..4] {
            // Not FIBP at all — write GoAway and close.
            let goaway = Frame::goaway("invalid magic");
            write_frame_raw(&mut stream, &goaway, max_frame_size)
                .await
                .ok();
            return Err(FibpError::HandshakeFailed {
                reason: "invalid magic bytes".into(),
            });
        }

        let client_major = client_magic[4];
        if client_major != MAGIC[4] {
            // Major version mismatch — send GoAway.
            let goaway = Frame::goaway(&format!(
                "unsupported protocol version {client_major}.x (server supports {}.x)",
                MAGIC[4]
            ));
            write_frame_raw(&mut stream, &goaway, max_frame_size)
                .await
                .ok();
            return Err(FibpError::HandshakeFailed {
                reason: format!(
                    "major version mismatch: client={client_major}, server={}",
                    MAGIC[4]
                ),
            });
        }

        // Handshake accepted — echo the server's magic.
        stream.write_all(MAGIC).await?;

        let codec = FibpCodec::new(max_frame_size);
        let framed = Framed::new(stream, codec);

        debug!(peer = %peer_addr, "fibp handshake complete");
        Ok(Self { framed, peer_addr })
    }

    /// Run the connection loop until the peer disconnects or an error occurs.
    ///
    /// Currently dispatches heartbeats and returns "not implemented" for all
    /// data/admin operations. Story 36.2 will inject a command dispatcher.
    pub async fn run(mut self) {
        loop {
            let frame = match self.framed.next().await {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    warn!(peer = %self.peer_addr, error = %e, "fibp read error");
                    return;
                }
                None => {
                    debug!(peer = %self.peer_addr, "fibp connection closed by peer");
                    return;
                }
            };

            if let Err(e) = self.dispatch(frame).await {
                warn!(peer = %self.peer_addr, error = %e, "fibp dispatch error");
                return;
            }
        }
    }

    /// Dispatch a single incoming frame.
    async fn dispatch(&mut self, frame: Frame) -> Result<(), FibpError> {
        match frame.op {
            OP_HEARTBEAT => {
                // Echo back the heartbeat with the same correlation id.
                let pong = Frame::new(0, OP_HEARTBEAT, frame.correlation_id, Bytes::new());
                self.write_frame(pong).await
            }
            OP_GOAWAY => {
                debug!(peer = %self.peer_addr, "received goaway from peer");
                Err(FibpError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "peer sent goaway",
                )))
            }
            op => {
                // All data/admin operations are stubs for now.
                let err_frame = Frame::error(
                    frame.correlation_id,
                    &format!("operation 0x{op:02X} not implemented"),
                );
                self.write_frame(err_frame).await
            }
        }
    }

    /// Encode and write a frame to the underlying TCP stream.
    async fn write_frame(&mut self, frame: Frame) -> Result<(), FibpError> {
        use bytes::BytesMut;

        let mut buf = BytesMut::new();
        // Use a temporary codec for encoding only (the framed codec owns the read half).
        let mut enc = FibpCodec::new(u32::MAX);
        enc.encode(frame, &mut buf)?;

        let stream = self.framed.get_mut();
        stream.write_all(&buf).await?;
        stream.flush().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write a frame directly to a raw `TcpStream` (used during handshake, before
/// the `Framed` wrapper is created).
async fn write_frame_raw(
    stream: &mut TcpStream,
    frame: &Frame,
    max_frame_size: u32,
) -> Result<(), FibpError> {
    use bytes::BytesMut;

    let mut buf = BytesMut::new();
    let mut enc = FibpCodec::new(max_frame_size);
    enc.encode(frame.clone(), &mut buf)?;
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::codec::{OP_ENQUEUE, OP_ERROR};
    use super::*;
    use tokio::net::TcpListener;
    use tokio_util::codec::Decoder;

    /// Helper: connect a raw TCP client, perform handshake, return the stream.
    async fn handshake_client(addr: std::net::SocketAddr) -> TcpStream {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(MAGIC).await.unwrap();
        let mut server_magic = [0u8; 6];
        stream.read_exact(&mut server_magic).await.unwrap();
        assert_eq!(&server_magic, MAGIC);
        stream
    }

    #[tokio::test]
    async fn handshake_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            FibpConnection::accept(stream, 16_777_216).await.unwrap();
        });

        let _client = handshake_client(addr).await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_bad_magic() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let err = FibpConnection::accept(stream, 16_777_216)
                .await
                .unwrap_err();
            assert!(
                matches!(err, FibpError::HandshakeFailed { .. }),
                "expected HandshakeFailed, got: {err:?}"
            );
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"HTTP/1").await.unwrap();
        // Read whatever the server sends back (GoAway frame).
        let mut buf = vec![0u8; 256];
        let _ = client.read(&mut buf).await;

        server.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_version_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let err = FibpConnection::accept(stream, 16_777_216)
                .await
                .unwrap_err();
            assert!(
                matches!(err, FibpError::HandshakeFailed { .. }),
                "expected HandshakeFailed, got: {err:?}"
            );
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        // Correct magic, wrong major version (99).
        client.write_all(b"FIBP\x63\x00").await.unwrap();
        let mut buf = vec![0u8; 256];
        let _ = client.read(&mut buf).await;

        server.await.unwrap();
    }

    #[tokio::test]
    async fn heartbeat_echo() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216).await.unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        // Encode a heartbeat frame.
        let ping = Frame::new(0, OP_HEARTBEAT, 123, Bytes::new());
        let mut send_buf = bytes::BytesMut::new();
        codec.encode(ping, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        // Read the echoed heartbeat.
        let mut recv_buf = bytes::BytesMut::new();
        loop {
            let mut tmp = [0u8; 256];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_HEARTBEAT);
                assert_eq!(frame.correlation_id, 123);
                break;
            }
        }

        // Close the client to end the server loop.
        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn not_implemented_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216).await.unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        // Send an enqueue frame (not implemented yet).
        let req = Frame::new(0, OP_ENQUEUE, 7, Bytes::from_static(b"test"));
        let mut send_buf = bytes::BytesMut::new();
        codec.encode(req, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        // Read the error response.
        let mut recv_buf = bytes::BytesMut::new();
        loop {
            let mut tmp = [0u8; 256];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_ERROR);
                assert_eq!(frame.correlation_id, 7);
                let msg = String::from_utf8_lossy(&frame.payload);
                assert!(msg.contains("not implemented"), "got: {msg}");
                break;
            }
        }

        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }
}
