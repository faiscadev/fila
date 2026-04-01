//! Integration tests for the FIBP binary protocol server.
//!
//! These tests start a full Broker + binary protocol listener and exercise
//! hot-path operations (enqueue, consume, ack, nack) over raw TCP using the
//! fila-fibp codec crate.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use fila_core::{Broker, BrokerConfig, RocksDbEngine};
use fila_fibp::{
    AckRequest, AckResponse, ConsumeRequest, DeliveryBatch, EnqueueMessage, EnqueueRequest,
    EnqueueResponse, ErrorCode, ErrorFrame, Handshake, HandshakeOk, NackRequest, NackResponse,
    Opcode, RawFrame, FLAG_CONTINUATION,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Start a broker + binary server on a random port. Returns the TCP address
/// and a shutdown sender (must be kept alive or the server exits).
async fn start_test_server() -> (
    String,
    Arc<Broker>,
    tempfile::TempDir,
    tokio::sync::watch::Sender<bool>,
) {
    let data_dir = tempfile::tempdir().expect("create temp dir");
    let rocksdb =
        Arc::new(RocksDbEngine::open(data_dir.path().join("data").to_str().unwrap()).unwrap());
    let storage: Arc<dyn fila_core::StorageEngine> = Arc::clone(&rocksdb) as _;

    let config = BrokerConfig::default();
    let broker = Arc::new(Broker::new(config, storage).unwrap());

    // Bind to a random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let binary_server = Arc::new(fila_server::binary_server::BinaryServer::new(
        Arc::clone(&broker),
        None,
        0,
    ));

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        fila_server::binary_server::run(binary_server, listener, None, shutdown_rx).await;
    });

    // Give the server a moment to start accepting connections
    tokio::time::sleep(Duration::from_millis(200)).await;

    (addr, broker, data_dir, shutdown_tx)
}

/// Connect to the server and perform the protocol handshake.
async fn connect_and_handshake(addr: &str) -> TcpStream {
    // Retry connection in case the server hasn't started accepting yet.
    let mut stream = None;
    for _ in 0..50 {
        match TcpStream::connect(addr).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let mut stream = stream.expect("failed to connect to binary server");

    // Send handshake
    let hs = Handshake {
        protocol_version: 1,
        api_key: None,
    };
    let frame = hs.encode(0);
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    stream.write_all(&buf).await.unwrap();

    // Read handshake ok
    let mut read_buf = BytesMut::with_capacity(4096);
    loop {
        stream.read_buf(&mut read_buf).await.unwrap();
        if let Some(frame) = RawFrame::decode(&mut read_buf).unwrap() {
            assert_eq!(frame.opcode, Opcode::HandshakeOk as u8);
            let ok = HandshakeOk::decode(frame.payload).unwrap();
            assert_eq!(ok.negotiated_version, 1);
            break;
        }
    }

    stream
}

/// Send a frame and read a response frame matching the same request_id.
async fn send_and_recv(stream: &mut TcpStream, frame: &RawFrame) -> RawFrame {
    let mut write_buf = BytesMut::new();
    frame.encode(&mut write_buf);
    stream.write_all(&write_buf).await.unwrap();

    let mut read_buf = BytesMut::with_capacity(4096);
    loop {
        let n = stream.read_buf(&mut read_buf).await.unwrap();
        if n == 0 {
            panic!(
                "server closed connection (EOF) while waiting for response to request_id={}",
                frame.request_id
            );
        }
        while let Some(resp) = RawFrame::decode(&mut read_buf).unwrap() {
            if resp.request_id == frame.request_id {
                return resp;
            }
            // Skip frames that don't match (e.g. delivery frames from consume subscriptions)
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_succeeds() {
    let (addr, _broker, _dir, _shutdown) = start_test_server().await;
    let _stream = connect_and_handshake(&addr).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_wrong_version_returns_error() {
    let (addr, _broker, _dir, _shutdown) = start_test_server().await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();

    let hs = Handshake {
        protocol_version: 99,
        api_key: None,
    };
    let frame = hs.encode(0);
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    stream.write_all(&buf).await.unwrap();

    let mut read_buf = BytesMut::with_capacity(4096);
    loop {
        stream.read_buf(&mut read_buf).await.unwrap();
        if let Some(frame) = RawFrame::decode(&mut read_buf).unwrap() {
            assert_eq!(frame.opcode, Opcode::Error as u8);
            let err = ErrorFrame::decode(frame.payload).unwrap();
            assert_eq!(err.error_code, ErrorCode::UnsupportedVersion);
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enqueue_to_nonexistent_queue_returns_queue_not_found() {
    let (addr, _broker, _dir, _shutdown) = start_test_server().await;
    let mut stream = connect_and_handshake(&addr).await;

    let req = EnqueueRequest {
        messages: vec![EnqueueMessage {
            queue: "no-such-queue".to_string(),
            headers: HashMap::new(),
            payload: vec![1, 2, 3],
        }],
    };
    let resp_frame = send_and_recv(&mut stream, &req.encode(1)).await;
    assert_eq!(resp_frame.opcode, Opcode::EnqueueResult as u8);

    let resp = EnqueueResponse::decode(resp_frame.payload).unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].error_code, ErrorCode::QueueNotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enqueue_and_consume_round_trip() {
    let (addr, broker, _dir, _shutdown) = start_test_server().await;

    // Create a queue first
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::CreateQueue {
            name: "test-queue".to_string(),
            config: fila_core::queue::QueueConfig::new("".to_string()),
            reply: reply_tx,
        })
        .unwrap();
    reply_rx.await.unwrap().unwrap();

    let mut stream = connect_and_handshake(&addr).await;

    // Subscribe to consume (request_id = 10)
    let consume_req = ConsumeRequest {
        queue: "test-queue".to_string(),
    };
    let mut write_buf = BytesMut::new();
    consume_req.encode(10).encode(&mut write_buf);
    stream.write_all(&write_buf).await.unwrap();

    // Give consumer registration a moment
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Enqueue a message (request_id = 1)
    let enqueue_req = EnqueueRequest {
        messages: vec![EnqueueMessage {
            queue: "test-queue".to_string(),
            headers: HashMap::new(),
            payload: b"hello binary protocol".to_vec(),
        }],
    };
    let resp_frame = send_and_recv(&mut stream, &enqueue_req.encode(1)).await;
    assert_eq!(resp_frame.opcode, Opcode::EnqueueResult as u8);
    let enqueue_resp = EnqueueResponse::decode(resp_frame.payload).unwrap();
    assert_eq!(enqueue_resp.results.len(), 1);
    assert_eq!(enqueue_resp.results[0].error_code, ErrorCode::Ok);
    let msg_id = enqueue_resp.results[0].message_id.clone();
    assert!(!msg_id.is_empty());

    // Read delivery frame (should come through from consume subscription)
    let mut read_buf = BytesMut::with_capacity(4096);
    let delivery_frame = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            stream.read_buf(&mut read_buf).await.unwrap();
            if let Some(frame) = RawFrame::decode(&mut read_buf).unwrap() {
                return frame;
            }
        }
    })
    .await
    .expect("delivery frame should arrive within 5s");

    assert_eq!(delivery_frame.opcode, Opcode::Delivery as u8);
    assert_eq!(delivery_frame.request_id, 10); // matches consume request_id
    let delivery = DeliveryBatch::decode(delivery_frame.payload).unwrap();
    assert_eq!(delivery.messages.len(), 1);
    assert_eq!(delivery.messages[0].message_id, msg_id);
    assert_eq!(delivery.messages[0].payload, b"hello binary protocol");

    // Ack the message
    let ack_req = AckRequest {
        items: vec![fila_fibp::AckItem {
            queue: "test-queue".to_string(),
            message_id: msg_id,
        }],
    };
    let ack_resp_frame = send_and_recv(&mut stream, &ack_req.encode(2)).await;
    assert_eq!(ack_resp_frame.opcode, Opcode::AckResult as u8);
    let ack_resp = AckResponse::decode(ack_resp_frame.payload).unwrap();
    assert_eq!(ack_resp.results.len(), 1);
    assert_eq!(ack_resp.results[0].error_code, ErrorCode::Ok);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_enqueue_100_messages() {
    let (addr, broker, _dir, _shutdown) = start_test_server().await;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::CreateQueue {
            name: "batch-queue".to_string(),
            config: fila_core::queue::QueueConfig::new("".to_string()),
            reply: reply_tx,
        })
        .unwrap();
    reply_rx.await.unwrap().unwrap();

    let mut stream = connect_and_handshake(&addr).await;

    // Batch enqueue 100 messages
    let messages: Vec<EnqueueMessage> = (0..100)
        .map(|i| EnqueueMessage {
            queue: "batch-queue".to_string(),
            headers: HashMap::new(),
            payload: format!("message-{i}").into_bytes(),
        })
        .collect();

    let req = EnqueueRequest { messages };
    let resp_frame = send_and_recv(&mut stream, &req.encode(1)).await;
    assert_eq!(resp_frame.opcode, Opcode::EnqueueResult as u8);

    let resp = EnqueueResponse::decode(resp_frame.payload).unwrap();
    assert_eq!(resp.results.len(), 100);
    for item in &resp.results {
        assert_eq!(item.error_code, ErrorCode::Ok);
        assert!(!item.message_id.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_ack_and_nack() {
    let (addr, broker, _dir, _shutdown) = start_test_server().await;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::CreateQueue {
            name: "ack-queue".to_string(),
            config: fila_core::queue::QueueConfig::new("".to_string()),
            reply: reply_tx,
        })
        .unwrap();
    reply_rx.await.unwrap().unwrap();

    let mut stream = connect_and_handshake(&addr).await;

    // Subscribe to consume
    let consume_req = ConsumeRequest {
        queue: "ack-queue".to_string(),
    };
    let mut write_buf = BytesMut::new();
    consume_req.encode(10).encode(&mut write_buf);
    stream.write_all(&write_buf).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Enqueue 2 messages
    let req = EnqueueRequest {
        messages: vec![
            EnqueueMessage {
                queue: "ack-queue".to_string(),
                headers: HashMap::new(),
                payload: b"msg1".to_vec(),
            },
            EnqueueMessage {
                queue: "ack-queue".to_string(),
                headers: HashMap::new(),
                payload: b"msg2".to_vec(),
            },
        ],
    };
    // Send enqueue and collect both the enqueue response and delivery frames
    let mut write_buf = BytesMut::new();
    req.encode(1).encode(&mut write_buf);
    stream.write_all(&write_buf).await.unwrap();

    let mut msg_ids = Vec::new();
    let mut read_buf = BytesMut::with_capacity(4096);
    let mut got_enqueue_response = false;

    // Read frames until we have the enqueue response and 2 deliveries
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        while !got_enqueue_response || msg_ids.len() < 2 {
            stream.read_buf(&mut read_buf).await.unwrap();
            while let Some(frame) = RawFrame::decode(&mut read_buf).unwrap() {
                if frame.opcode == Opcode::EnqueueResult as u8 {
                    got_enqueue_response = true;
                } else if frame.opcode == Opcode::Delivery as u8 {
                    let batch = DeliveryBatch::decode(frame.payload).unwrap();
                    for msg in &batch.messages {
                        msg_ids.push(msg.message_id.clone());
                    }
                }
            }
        }
    })
    .await;
    result.expect("timed out waiting for enqueue response + 2 deliveries");

    assert!(got_enqueue_response);
    assert_eq!(msg_ids.len(), 2);

    // Cancel consume to prevent redelivery interference
    let cancel = RawFrame {
        opcode: Opcode::CancelConsume as u8,
        flags: 0,
        request_id: 10,
        payload: bytes::Bytes::new(),
    };
    let mut cancel_buf = BytesMut::new();
    cancel.encode(&mut cancel_buf);
    stream.write_all(&cancel_buf).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Ack first message
    let ack_req = AckRequest {
        items: vec![fila_fibp::AckItem {
            queue: "ack-queue".to_string(),
            message_id: msg_ids[0].clone(),
        }],
    };
    let ack_frame = send_and_recv(&mut stream, &ack_req.encode(2)).await;
    let ack_resp = AckResponse::decode(ack_frame.payload).unwrap();
    assert_eq!(ack_resp.results[0].error_code, ErrorCode::Ok);

    // Nack second message
    let nack_req = NackRequest {
        items: vec![fila_fibp::NackItem {
            queue: "ack-queue".to_string(),
            message_id: msg_ids[1].clone(),
            error: "test failure".to_string(),
        }],
    };
    let nack_frame = send_and_recv(&mut stream, &nack_req.encode(3)).await;
    let nack_resp = NackResponse::decode(nack_frame.payload).unwrap();
    assert_eq!(nack_resp.results[0].error_code, ErrorCode::Ok);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_pong() {
    let (addr, _broker, _dir, _shutdown) = start_test_server().await;
    let mut stream = connect_and_handshake(&addr).await;

    let ping = RawFrame {
        opcode: Opcode::Ping as u8,
        flags: 0,
        request_id: 42,
        payload: bytes::Bytes::new(),
    };
    let pong = send_and_recv(&mut stream, &ping).await;
    assert_eq!(pong.opcode, Opcode::Pong as u8);
    assert_eq!(pong.request_id, 42);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_opcode_returns_error() {
    let (addr, _broker, _dir, _shutdown) = start_test_server().await;
    let mut stream = connect_and_handshake(&addr).await;

    let unknown = RawFrame {
        opcode: 0xAA,
        flags: 0,
        request_id: 1,
        payload: bytes::Bytes::new(),
    };
    let resp = send_and_recv(&mut stream, &unknown).await;
    assert_eq!(resp.opcode, Opcode::Error as u8);
    let err = ErrorFrame::decode(resp.payload).unwrap();
    assert_eq!(err.error_code, ErrorCode::InvalidFrame);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuation_frame_reassembly() {
    let (addr, broker, _dir, _shutdown) = start_test_server().await;

    // Create a queue first
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::CreateQueue {
            name: "cont-queue".to_string(),
            config: fila_core::queue::QueueConfig::new("".to_string()),
            reply: reply_tx,
        })
        .unwrap();
    reply_rx.await.unwrap().unwrap();

    let mut stream = connect_and_handshake(&addr).await;

    // Build a normal enqueue request and get its full payload.
    let req = EnqueueRequest {
        messages: vec![EnqueueMessage {
            queue: "cont-queue".to_string(),
            headers: HashMap::new(),
            payload: b"continuation-test-payload".to_vec(),
        }],
    };
    let full_frame = req.encode(1);
    let full_payload = full_frame.payload;

    // Split the payload roughly in half into two continuation frames.
    let mid = full_payload.len() / 2;
    let chunk1 = full_payload.slice(..mid);
    let chunk2 = full_payload.slice(mid..);

    // First frame: CONTINUATION=1 (more to come)
    let frame1 = RawFrame {
        opcode: Opcode::Enqueue as u8,
        flags: FLAG_CONTINUATION,
        request_id: 1,
        payload: chunk1,
    };
    let mut buf = BytesMut::new();
    frame1.encode(&mut buf);
    stream.write_all(&buf).await.unwrap();

    // Second frame: CONTINUATION=0 (final)
    let frame2 = RawFrame {
        opcode: Opcode::Enqueue as u8,
        flags: 0,
        request_id: 1,
        payload: chunk2,
    };
    let mut buf = BytesMut::new();
    frame2.encode(&mut buf);
    stream.write_all(&buf).await.unwrap();

    // Read the enqueue result — if reassembly works, the server decoded
    // the concatenated payload as a valid EnqueueRequest.
    let mut read_buf = BytesMut::with_capacity(4096);
    let resp_frame = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let n = stream.read_buf(&mut read_buf).await.unwrap();
            if n == 0 {
                panic!("connection closed unexpectedly (EOF) before receiving enqueue result");
            }
            if let Some(frame) = RawFrame::decode(&mut read_buf).unwrap() {
                return frame;
            }
        }
    })
    .await
    .expect("should receive enqueue result within 5s");

    assert_eq!(resp_frame.opcode, Opcode::EnqueueResult as u8);
    assert_eq!(resp_frame.request_id, 1);
    let resp = EnqueueResponse::decode(resp_frame.payload).unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].error_code, ErrorCode::Ok);
    assert!(!resp.results[0].message_id.is_empty());
}
