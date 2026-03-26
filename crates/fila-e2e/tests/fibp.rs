mod helpers;

use std::time::Duration;

use bytes::{Buf, BufMut, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// FIBP magic bytes: "FIBP" + major 1 + minor 0.
const MAGIC: &[u8; 6] = b"FIBP\x01\x00";

/// Start a fila-server, returning (TestServer, fibp_addr).
/// FIBP is the sole transport, so the server address IS the FIBP address.
fn start_fibp_server() -> (helpers::TestServer, String) {
    let server = helpers::TestServer::start();
    let addr = server.addr().to_string();
    (server, addr)
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

/// Read a full FIBP frame from a TCP stream with a timeout.
async fn read_frame(stream: &mut TcpStream) -> (u8, u8, u32, Vec<u8>) {
    let mut buf = vec![0u8; 64 * 1024];
    let mut offset = 0;

    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf[offset..]))
            .await
            .expect("timeout reading frame")
            .expect("read error");
        assert!(n > 0, "connection closed while reading frame");
        offset += n;

        // Check if we have a complete frame.
        if offset >= 4 {
            let body_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            if offset >= 4 + body_len {
                return decode_frame(&buf[..4 + body_len]);
            }
        }
    }
}

const OP_HEARTBEAT: u8 = 0x21;
const OP_ENQUEUE: u8 = 0x01;
const OP_CONSUME: u8 = 0x02;
const OP_ACK: u8 = 0x03;
const OP_NACK: u8 = 0x04;
const OP_FLOW: u8 = 0x20;
const OP_AUTH: u8 = 0x30;
const OP_ERROR: u8 = 0xFE;
const OP_CREATE_QUEUE: u8 = 0x10;
const OP_LIST_QUEUES: u8 = 0x13;
const FLAG_STREAM: u8 = 0x04;

// ---------------------------------------------------------------------------
// Wire encoding helpers
// ---------------------------------------------------------------------------

fn write_string16(buf: &mut BytesMut, s: &str) {
    buf.put_u16(s.len() as u16);
    buf.extend_from_slice(s.as_bytes());
}

/// Build an enqueue request payload for a single queue with `count` messages.
fn build_enqueue_payload(queue: &str, count: usize) -> Vec<u8> {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, queue);
    buf.put_u16(count as u16);
    for i in 0..count {
        buf.put_u8(0); // no headers
        let payload = format!("message-{i}");
        buf.put_u32(payload.len() as u32);
        buf.extend_from_slice(payload.as_bytes());
    }
    buf.to_vec()
}

/// Build a consume request payload.
fn build_consume_payload(queue: &str, initial_credits: u32) -> Vec<u8> {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, queue);
    buf.put_u32(initial_credits);
    buf.to_vec()
}

/// Build an ack request payload for the given queue + msg_id pairs.
fn build_ack_payload(items: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = BytesMut::new();
    buf.put_u16(items.len() as u16);
    for (queue, msg_id) in items {
        write_string16(&mut buf, queue);
        write_string16(&mut buf, msg_id);
    }
    buf.to_vec()
}

/// Build a nack request payload.
fn build_nack_payload(items: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut buf = BytesMut::new();
    buf.put_u16(items.len() as u16);
    for (queue, msg_id, error) in items {
        write_string16(&mut buf, queue);
        write_string16(&mut buf, msg_id);
        write_string16(&mut buf, error);
    }
    buf.to_vec()
}

/// Build a flow (credit) payload.
fn build_flow_payload(credits: u32) -> Vec<u8> {
    let mut buf = BytesMut::new();
    buf.put_u32(credits);
    buf.to_vec()
}

/// Parse an enqueue response payload, returning (ok_msg_ids, err_count).
fn parse_enqueue_response(payload: &[u8]) -> Vec<Result<String, (u16, String)>> {
    let mut buf = &payload[..];
    let count = buf.get_u16() as usize;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let ok = buf.get_u8();
        if ok == 1 {
            let id_len = buf.get_u16() as usize;
            let id = std::str::from_utf8(&buf[..id_len]).unwrap().to_string();
            buf.advance(id_len);
            results.push(Ok(id));
        } else {
            let code = buf.get_u16();
            let msg_len = buf.get_u16() as usize;
            let msg = std::str::from_utf8(&buf[..msg_len]).unwrap().to_string();
            buf.advance(msg_len);
            results.push(Err((code, msg)));
        }
    }
    results
}

/// Parse a consume push frame payload, returning message IDs.
fn parse_consume_push(payload: &[u8]) -> Vec<String> {
    let mut buf = &payload[..];
    let count = buf.get_u16() as usize;
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        // msg_id
        let id_len = buf.get_u16() as usize;
        let id = std::str::from_utf8(&buf[..id_len]).unwrap().to_string();
        buf.advance(id_len);
        // fairness_key
        let fk_len = buf.get_u16() as usize;
        buf.advance(fk_len);
        // attempt_count
        let _ = buf.get_u32();
        // headers
        let hcount = buf.get_u8() as usize;
        for _ in 0..hcount {
            let klen = buf.get_u16() as usize;
            buf.advance(klen);
            let vlen = buf.get_u16() as usize;
            buf.advance(vlen);
        }
        // payload
        let plen = buf.get_u32() as usize;
        buf.advance(plen);
        ids.push(id);
    }
    ids
}

/// Parse ack/nack response payload.
fn parse_ack_nack_response(payload: &[u8]) -> Vec<Result<(), (u16, String)>> {
    let mut buf = &payload[..];
    let count = buf.get_u16() as usize;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let ok = buf.get_u8();
        if ok == 1 {
            results.push(Ok(()));
        } else {
            let code = buf.get_u16();
            let msg_len = buf.get_u16() as usize;
            let msg = std::str::from_utf8(&buf[..msg_len]).unwrap().to_string();
            buf.advance(msg_len);
            results.push(Err((code, msg)));
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    let (flags, op, corr_id, payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_HEARTBEAT);
    assert_eq!(corr_id, 42);
    assert_eq!(flags, 0);
    assert!(payload.is_empty());
}

/// E2E: enqueue 100 messages over FIBP and verify all succeed.
#[tokio::test]
async fn e2e_fibp_enqueue_batch_100() {
    let (server, fibp_addr) = start_fibp_server();

    // Create the queue via CLI.
    helpers::create_queue_cli(server.addr(), "fibp-batch-queue");

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Enqueue 100 messages in a single FIBP request.
    let payload = build_enqueue_payload("fibp-batch-queue", 100);
    let req = encode_frame(0, OP_ENQUEUE, 1, &payload);
    stream.write_all(&req).await.unwrap();

    // Read the response.
    let (_, op, corr_id, resp_payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_ENQUEUE);
    assert_eq!(corr_id, 1);

    let results = parse_enqueue_response(&resp_payload);
    assert_eq!(
        results.len(),
        100,
        "expected 100 results, got {}",
        results.len()
    );

    // All should be success.
    let mut success_count = 0;
    for result in &results {
        match result {
            Ok(msg_id) => {
                assert!(!msg_id.is_empty(), "msg_id should not be empty");
                success_count += 1;
            }
            Err((code, msg)) => panic!("unexpected error: code={code}, msg={msg}"),
        }
    }
    assert_eq!(success_count, 100);

    // Verify all IDs are unique.
    let unique_ids: std::collections::HashSet<_> = results
        .iter()
        .map(|r| r.as_ref().unwrap().clone())
        .collect();
    assert_eq!(unique_ids.len(), 100, "all message IDs should be unique");
}

/// E2E: enqueue -> consume with credit flow -> ack lifecycle over FIBP.
#[tokio::test]
async fn e2e_fibp_enqueue_consume_ack() {
    let (server, fibp_addr) = start_fibp_server();

    // Create the queue via CLI.
    helpers::create_queue_cli(server.addr(), "fibp-lifecycle-queue");

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Enqueue 3 messages.
    let payload = build_enqueue_payload("fibp-lifecycle-queue", 3);
    let req = encode_frame(0, OP_ENQUEUE, 1, &payload);
    stream.write_all(&req).await.unwrap();

    let (_, op, corr_id, resp_payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_ENQUEUE);
    assert_eq!(corr_id, 1);
    let enqueue_results = parse_enqueue_response(&resp_payload);
    assert_eq!(enqueue_results.len(), 3);
    for r in &enqueue_results {
        assert!(r.is_ok(), "enqueue should succeed");
    }

    // Open a consume session with 10 credits.
    let consume_payload = build_consume_payload("fibp-lifecycle-queue", 10);
    let consume_req = encode_frame(0, OP_CONSUME, 2, &consume_payload);
    stream.write_all(&consume_req).await.unwrap();

    // Read the consume ack (empty payload = success).
    let (_, op, corr_id, ack_payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_CONSUME);
    assert_eq!(corr_id, 2);
    assert!(
        ack_payload.is_empty(),
        "consume ack should have empty payload"
    );

    // Give the scheduler a moment to deliver, then read pushed messages.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Read pushed messages (may arrive as one or more push frames).
    let mut received_ids: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while received_ids.len() < 3 {
        if tokio::time::Instant::now() > deadline {
            panic!(
                "timed out waiting for consume push frames; got {} of 3",
                received_ids.len()
            );
        }
        let (flags, op, _, push_payload) = read_frame(&mut stream).await;
        assert_eq!(op, OP_CONSUME, "expected consume push frame");
        assert_ne!(flags & FLAG_STREAM, 0, "push frame should have FLAG_STREAM");
        let ids = parse_consume_push(&push_payload);
        received_ids.extend(ids);
    }
    assert_eq!(received_ids.len(), 3, "should receive exactly 3 messages");

    // Ack all 3 messages.
    let ack_items: Vec<(&str, &str)> = received_ids
        .iter()
        .map(|id| ("fibp-lifecycle-queue", id.as_str()))
        .collect();
    let ack_payload = build_ack_payload(&ack_items);
    let ack_req = encode_frame(0, OP_ACK, 3, &ack_payload);
    stream.write_all(&ack_req).await.unwrap();

    let (_, op, corr_id, resp_payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_ACK);
    assert_eq!(corr_id, 3);

    let ack_results = parse_ack_nack_response(&resp_payload);
    assert_eq!(ack_results.len(), 3);
    for (i, r) in ack_results.iter().enumerate() {
        assert!(r.is_ok(), "ack {i} should succeed");
    }
}

/// E2E: nack with error message over FIBP.
#[tokio::test]
async fn e2e_fibp_nack_with_error() {
    let (server, fibp_addr) = start_fibp_server();

    helpers::create_queue_cli(server.addr(), "fibp-nack-queue");

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Enqueue 1 message.
    let payload = build_enqueue_payload("fibp-nack-queue", 1);
    let req = encode_frame(0, OP_ENQUEUE, 1, &payload);
    stream.write_all(&req).await.unwrap();

    let (_, op, _, resp_payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_ENQUEUE);
    let enqueue_results = parse_enqueue_response(&resp_payload);
    assert_eq!(enqueue_results.len(), 1);
    assert!(enqueue_results[0].is_ok());

    // Consume with credits.
    let consume_payload = build_consume_payload("fibp-nack-queue", 10);
    let consume_req = encode_frame(0, OP_CONSUME, 2, &consume_payload);
    stream.write_all(&consume_req).await.unwrap();

    // Read consume ack.
    let (_, op, _, _) = read_frame(&mut stream).await;
    assert_eq!(op, OP_CONSUME);

    // Give the scheduler a moment to deliver.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Read pushed message.
    let (flags, op, _, push_payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_CONSUME);
    assert_ne!(flags & FLAG_STREAM, 0);
    let ids = parse_consume_push(&push_payload);
    assert_eq!(ids.len(), 1);
    let msg_id = &ids[0];

    // Nack the message with an error string.
    let nack_items: Vec<(&str, &str, &str)> = vec![(
        "fibp-nack-queue",
        msg_id.as_str(),
        "processing failed: timeout",
    )];
    let nack_payload = build_nack_payload(&nack_items);
    let nack_req = encode_frame(0, OP_NACK, 3, &nack_payload);
    stream.write_all(&nack_req).await.unwrap();

    let (_, op, corr_id, resp_payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_NACK);
    assert_eq!(corr_id, 3);

    let nack_results = parse_ack_nack_response(&resp_payload);
    assert_eq!(nack_results.len(), 1);
    assert!(nack_results[0].is_ok(), "nack should succeed");

    // The message should be re-delivered (nack puts it back in the queue).
    // Read the re-delivered message with a generous timeout.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut redelivered = false;

    // Send more credits since we used some.
    let flow_payload = build_flow_payload(10);
    let flow_req = encode_frame(0, OP_FLOW, 0, &flow_payload);
    stream.write_all(&flow_req).await.unwrap();

    while tokio::time::Instant::now() < deadline {
        let read_result =
            tokio::time::timeout(Duration::from_secs(3), read_frame(&mut stream)).await;

        match read_result {
            Ok((flags, op, _, push_payload)) if op == OP_CONSUME && (flags & FLAG_STREAM) != 0 => {
                let ids = parse_consume_push(&push_payload);
                if !ids.is_empty() {
                    redelivered = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(redelivered, "nacked message should be re-delivered");
}

// ---------------------------------------------------------------------------
// Auth + Admin tests
// ---------------------------------------------------------------------------

/// Start a fila-server with auth enabled.
fn start_fibp_auth_server() -> (helpers::TestServer, String, String) {
    let data_dir = tempfile::tempdir().expect("temp dir");
    let port_file = data_dir.path().join("port");
    let bootstrap_key = "e2e-bootstrap-key-12345";

    let config_content = format!(
        concat!(
            "[fibp]\n",
            "listen_addr = \"127.0.0.1:0\"\n",
            "\n",
            "[telemetry]\n",
            "otlp_endpoint = \"\"\n",
            "\n",
            "[auth]\n",
            "bootstrap_apikey = \"{}\"\n",
        ),
        bootstrap_key,
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
        .current_dir(data_dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start fila-server");

    use std::io::BufRead;
    let stdout = child.stdout.take().expect("stdout");
    std::thread::spawn(move || for _ in std::io::BufReader::new(stdout).lines() {});
    let stderr = child.stderr.take().expect("stderr");
    std::thread::spawn(move || for _ in std::io::BufReader::new(stderr).lines() {});

    let start = std::time::Instant::now();
    let addr = loop {
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

    let server = helpers::TestServer::from_parts(child, addr.clone(), data_dir);
    (server, addr, bootstrap_key.to_string())
}

/// E2E: auth success over FIBP — send OP_AUTH with valid key, get success response.
#[tokio::test]
async fn e2e_fibp_auth_success() {
    let (_server, fibp_addr, bootstrap_key) = start_fibp_auth_server();

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Send auth frame with valid bootstrap key.
    let auth_frame = encode_frame(0, OP_AUTH, 1, bootstrap_key.as_bytes());
    stream.write_all(&auth_frame).await.unwrap();

    let (_, op, corr_id, payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_AUTH, "expected OP_AUTH response");
    assert_eq!(corr_id, 1);
    assert!(payload.is_empty(), "auth success has empty payload");

    // Heartbeat should work after auth.
    let ping = encode_frame(0, OP_HEARTBEAT, 42, b"");
    stream.write_all(&ping).await.unwrap();

    let (_, op, corr_id, _) = read_frame(&mut stream).await;
    assert_eq!(op, OP_HEARTBEAT);
    assert_eq!(corr_id, 42);
}

/// E2E: auth failure over FIBP — send OP_AUTH with wrong key, get error and close.
#[tokio::test]
async fn e2e_fibp_auth_failure() {
    let (_server, fibp_addr, _bootstrap_key) = start_fibp_auth_server();

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Send auth frame with wrong key.
    let auth_frame = encode_frame(0, OP_AUTH, 1, b"totally-wrong-key");
    stream.write_all(&auth_frame).await.unwrap();

    let (_, op, corr_id, payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_ERROR, "expected error response");
    assert_eq!(corr_id, 1);
    let msg = String::from_utf8_lossy(&payload);
    assert!(msg.contains("invalid"), "expected auth failure, got: {msg}");
}

/// E2E: create queue + list queues over FIBP admin operations.
#[tokio::test]
async fn e2e_fibp_admin_create_and_list_queues() {
    let (_server, fibp_addr) = start_fibp_server();

    let mut stream = TcpStream::connect(&fibp_addr).await.unwrap();
    handshake(&mut stream).await;

    // Create a queue via FIBP admin op (protobuf payload).
    use prost::Message;
    let create_req = fila_proto::CreateQueueRequest {
        name: "fibp-admin-test".to_string(),
        config: None,
    };
    let mut proto_buf = Vec::new();
    create_req.encode(&mut proto_buf).unwrap();

    let create_frame = encode_frame(0, OP_CREATE_QUEUE, 10, &proto_buf);
    stream.write_all(&create_frame).await.unwrap();

    let (_, op, corr_id, payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_CREATE_QUEUE);
    assert_eq!(corr_id, 10);
    let create_resp = fila_proto::CreateQueueResponse::decode(&payload[..]).unwrap();
    assert_eq!(create_resp.queue_id, "fibp-admin-test");

    // List queues via FIBP admin op.
    let list_req = fila_proto::ListQueuesRequest {};
    let mut list_buf = Vec::new();
    list_req.encode(&mut list_buf).unwrap();

    let list_frame = encode_frame(0, OP_LIST_QUEUES, 11, &list_buf);
    stream.write_all(&list_frame).await.unwrap();

    let (_, op, corr_id, payload) = read_frame(&mut stream).await;
    assert_eq!(op, OP_LIST_QUEUES);
    assert_eq!(corr_id, 11);
    let list_resp = fila_proto::ListQueuesResponse::decode(&payload[..]).unwrap();
    let names: Vec<&str> = list_resp.queues.iter().map(|q| q.name.as_str()).collect();
    assert!(
        names.contains(&"fibp-admin-test"),
        "expected fibp-admin-test in {names:?}"
    );
    // Also check that the auto-created DLQ is present.
    assert!(
        names.contains(&"fibp-admin-test.dlq"),
        "expected DLQ in {names:?}"
    );
}
