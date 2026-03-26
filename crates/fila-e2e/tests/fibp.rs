mod helpers;

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// FIBP magic bytes: "FIBP" + major 1 + minor 0.
const MAGIC: &[u8; 6] = b"FIBP\x01\x00";

/// Start a fila-server with FIBP enabled, returning (TestServer, fibp_addr).
fn start_fibp_server() -> (helpers::TestServer, String) {
    let data_dir = tempfile::tempdir().expect("temp dir");
    let port_file = data_dir.path().join("port");
    let fibp_port_file = data_dir.path().join("fibp_port");

    let config_content = concat!(
        "[server]\n",
        "listen_addr = \"127.0.0.1:0\"\n",
        "\n",
        "[telemetry]\n",
        "otlp_endpoint = \"\"\n",
        "\n",
        "[fibp]\n",
        "listen_addr = \"127.0.0.1:0\"\n",
    );
    let config_path = data_dir.path().join("fila.toml");
    std::fs::write(&config_path, config_content).expect("write config");

    let binary = workspace_binary("fila-server");
    assert!(
        binary.exists(),
        "fila-server binary not found at {binary:?}. Run `cargo build` first."
    );

    let mut child = std::process::Command::new(&binary)
        .env(
            "FILA_DATA_DIR",
            data_dir.path().join("data").to_str().unwrap(),
        )
        .env("FILA_PORT_FILE", port_file.to_str().unwrap())
        .env("FILA_FIBP_PORT_FILE", fibp_port_file.to_str().unwrap())
        .current_dir(data_dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start fila-server");

    // Drain stdout/stderr.
    use std::io::BufRead;
    let stdout = child.stdout.take().expect("stdout");
    std::thread::spawn(move || for _ in std::io::BufReader::new(stdout).lines() {});
    let stderr = child.stderr.take().expect("stderr");
    std::thread::spawn(move || for _ in std::io::BufReader::new(stderr).lines() {});

    // Wait for the gRPC port file.
    let start = std::time::Instant::now();
    let grpc_addr = loop {
        if start.elapsed() > Duration::from_secs(10) {
            match child.try_wait() {
                Ok(Some(status)) => {
                    panic!("fila-server exited with {status} before writing port file")
                }
                _ => panic!("fila-server did not write port file within 10s"),
            }
        }
        if let Ok(contents) = std::fs::read_to_string(&port_file) {
            let contents = contents.trim();
            if !contents.is_empty() {
                break contents.to_string();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // Wait for the FIBP port file.
    let fibp_addr = loop {
        if start.elapsed() > Duration::from_secs(10) {
            panic!("fila-server did not write FIBP port file within 10s");
        }
        if let Ok(contents) = std::fs::read_to_string(&fibp_port_file) {
            let contents = contents.trim();
            if !contents.is_empty() {
                break contents.to_string();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let server = helpers::TestServer::from_parts(child, format!("http://{grpc_addr}"), data_dir);
    (server, fibp_addr)
}

fn workspace_binary(name: &str) -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push(name);
    path
}

/// Perform FIBP handshake on a raw TCP stream.
async fn handshake(stream: &mut TcpStream) {
    stream.write_all(MAGIC).await.unwrap();
    let mut server_magic = [0u8; 6];
    stream.read_exact(&mut server_magic).await.unwrap();
    assert_eq!(&server_magic, MAGIC, "server handshake magic mismatch");
}

/// Encode a FIBP frame into bytes (length-prefixed).
fn encode_frame(flags: u8, op: u8, correlation_id: u32, payload: &[u8]) -> Vec<u8> {
    let body_len = 6 + payload.len(); // flags(1) + op(1) + corr_id(4) + payload
    let mut buf = Vec::with_capacity(4 + body_len);
    buf.extend_from_slice(&(body_len as u32).to_be_bytes());
    buf.push(flags);
    buf.push(op);
    buf.extend_from_slice(&correlation_id.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Decode a FIBP frame from a buffer, returning (flags, op, correlation_id, payload).
/// Panics if the buffer is too short.
fn decode_frame(buf: &[u8]) -> (u8, u8, u32, Vec<u8>) {
    assert!(buf.len() >= 4, "need at least 4 bytes for length prefix");
    let body_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    assert!(buf.len() >= 4 + body_len, "incomplete frame");
    let flags = buf[4];
    let op = buf[5];
    let corr_id = u32::from_be_bytes([buf[6], buf[7], buf[8], buf[9]]);
    let payload = buf[10..4 + body_len].to_vec();
    (flags, op, corr_id, payload)
}

const OP_HEARTBEAT: u8 = 0x21;
const OP_ENQUEUE: u8 = 0x01;
const OP_ERROR: u8 = 0xFE;

/// E2E: connect via raw TCP, complete FIBP handshake, send heartbeat, verify echo.
#[tokio::test]
async fn e2e_fibp_handshake_and_heartbeat() {
    let (_server, fibp_addr) = start_fibp_server();

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Send a heartbeat frame.
    let ping = encode_frame(0, OP_HEARTBEAT, 42, b"");
    stream.write_all(&ping).await.unwrap();

    // Read the echoed heartbeat.
    let mut buf = vec![0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("timeout reading heartbeat response")
        .expect("read error");

    let (flags, op, corr_id, payload) = decode_frame(&buf[..n]);
    assert_eq!(op, OP_HEARTBEAT);
    assert_eq!(corr_id, 42);
    assert_eq!(flags, 0);
    assert!(payload.is_empty());
}

/// E2E: verify that unimplemented operations return an error frame.
#[tokio::test]
async fn e2e_fibp_not_implemented() {
    let (_server, fibp_addr) = start_fibp_server();

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Send an enqueue frame (not yet implemented).
    let req = encode_frame(0, OP_ENQUEUE, 7, b"some payload");
    stream.write_all(&req).await.unwrap();

    // Read the error response.
    let mut buf = vec![0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("timeout reading error response")
        .expect("read error");

    let (_flags, op, corr_id, payload) = decode_frame(&buf[..n]);
    assert_eq!(op, OP_ERROR);
    assert_eq!(corr_id, 7);
    let msg = String::from_utf8_lossy(&payload);
    assert!(
        msg.contains("not implemented"),
        "expected 'not implemented' in: {msg}"
    );
}
