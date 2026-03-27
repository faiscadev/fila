//! FIBP binary wire encoding and decoding for all operation payloads.
//!
//! Each operation (data and admin) has a compact binary representation
//! defined here. All multi-byte integers are big-endian. Strings are
//! UTF-8 with a `u16` length prefix.

use std::collections::HashMap;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use super::error::FibpCodecError;

// ---------------------------------------------------------------------------
// Error codes used in response payloads
// ---------------------------------------------------------------------------

/// Enqueue error codes sent in FIBP enqueue responses.
pub mod enqueue_err {
    pub const QUEUE_NOT_FOUND: u16 = 1;
    pub const STORAGE: u16 = 2;
}

/// Ack/Nack error codes sent in FIBP ack/nack responses.
pub mod ack_nack_err {
    pub const MESSAGE_NOT_FOUND: u16 = 1;
    pub const STORAGE: u16 = 2;
}

// ---------------------------------------------------------------------------
// Enqueue request / response
// ---------------------------------------------------------------------------

/// A single message in an enqueue request.
#[derive(Debug, Clone)]
pub struct WireEnqueueMessage {
    pub headers: HashMap<String, String>,
    pub payload: Bytes,
}

/// Parsed enqueue request payload.
#[derive(Debug)]
pub struct EnqueueRequest {
    pub queue: String,
    pub messages: Vec<WireEnqueueMessage>,
}

/// Decode an enqueue request from the frame payload.
pub fn decode_enqueue_request(mut buf: Bytes) -> Result<EnqueueRequest, FibpCodecError> {
    let queue = read_string16(&mut buf)?;
    let msg_count = read_u16(&mut buf)? as usize;

    let mut messages = Vec::with_capacity(msg_count);
    for _ in 0..msg_count {
        let header_count = read_u8(&mut buf)? as usize;
        let mut headers = HashMap::with_capacity(header_count);
        for _ in 0..header_count {
            let key = read_string16(&mut buf)?;
            let val = read_string16(&mut buf)?;
            headers.insert(key, val);
        }
        let payload_len = read_u32(&mut buf)? as usize;
        if buf.remaining() < payload_len {
            return Err(FibpCodecError::InvalidPayload {
                reason: format!(
                    "payload length {payload_len} exceeds remaining {} bytes",
                    buf.remaining()
                ),
            });
        }
        let payload = buf.split_to(payload_len);
        messages.push(WireEnqueueMessage { headers, payload });
    }

    Ok(EnqueueRequest { queue, messages })
}

/// Encode an enqueue request payload (client-side).
pub fn encode_enqueue_request(queue: &str, messages: &[EnqueueWireMessage]) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, queue);
    buf.put_u16(messages.len() as u16);
    for msg in messages {
        buf.put_u8(msg.headers.len() as u8);
        for (k, v) in &msg.headers {
            write_string16(&mut buf, k);
            write_string16(&mut buf, v);
        }
        buf.put_u32(msg.payload.len() as u32);
        buf.extend_from_slice(&msg.payload);
    }
    buf.freeze()
}

/// A single message for client-side wire encoding.
pub struct EnqueueWireMessage {
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

/// A single result in the enqueue response.
pub enum EnqueueResultItem {
    Ok { msg_id: String },
    Err { code: u16, message: String },
}

/// Encode an enqueue response payload.
pub fn encode_enqueue_response(results: &[EnqueueResultItem]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(results.len() as u16);
    for item in results {
        match item {
            EnqueueResultItem::Ok { msg_id } => {
                buf.put_u8(1); // success
                write_string16(&mut buf, msg_id);
            }
            EnqueueResultItem::Err { code, message } => {
                buf.put_u8(0); // error
                buf.put_u16(*code);
                write_string16(&mut buf, message);
            }
        }
    }
    buf.freeze()
}

/// Decode an enqueue response payload (client-side).
pub fn decode_enqueue_response(mut buf: Bytes) -> Result<Vec<EnqueueResultItem>, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = read_u8(&mut buf)?;
        if tag == 1 {
            let msg_id = read_string16(&mut buf)?;
            results.push(EnqueueResultItem::Ok { msg_id });
        } else {
            let code = read_u16(&mut buf)?;
            let message = read_string16(&mut buf)?;
            results.push(EnqueueResultItem::Err { code, message });
        }
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Consume request
// ---------------------------------------------------------------------------

/// Parsed consume request payload.
#[derive(Debug)]
pub struct ConsumeRequest {
    pub queue: String,
    pub initial_credits: u32,
}

/// Decode a consume request from the frame payload.
pub fn decode_consume_request(mut buf: Bytes) -> Result<ConsumeRequest, FibpCodecError> {
    let queue = read_string16(&mut buf)?;
    let initial_credits = read_u32(&mut buf)?;
    Ok(ConsumeRequest {
        queue,
        initial_credits,
    })
}

/// Encode a consume request payload (client-side).
pub fn encode_consume_request(queue: &str, initial_credits: u32) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, queue);
    buf.put_u32(initial_credits);
    buf.freeze()
}

// ---------------------------------------------------------------------------
// Consume push frame (server -> client)
// ---------------------------------------------------------------------------

/// A message prepared for the consume push frame (server-side).
pub struct ConsumePushMessage {
    pub msg_id: String,
    pub fairness_key: String,
    pub attempt_count: u32,
    pub headers: HashMap<String, String>,
    pub payload: Bytes,
}

/// Encode a batch of ready messages as a consume push payload.
pub fn encode_consume_push(messages: &[ConsumePushMessage]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(messages.len() as u16);
    for msg in messages {
        write_string16(&mut buf, &msg.msg_id);
        write_string16(&mut buf, &msg.fairness_key);
        buf.put_u32(msg.attempt_count);
        // headers
        buf.put_u8(msg.headers.len() as u8);
        for (k, v) in &msg.headers {
            write_string16(&mut buf, k);
            write_string16(&mut buf, v);
        }
        // payload
        buf.put_u32(msg.payload.len() as u32);
        buf.extend_from_slice(&msg.payload);
    }
    buf.freeze()
}

/// A consumed message from a push frame (client-side).
pub struct ClientConsumePushMessage {
    pub msg_id: String,
    pub fairness_key: String,
    pub attempt_count: u32,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

/// Decode a consume push payload (client-side, server -> client).
pub fn decode_consume_push(
    mut buf: Bytes,
) -> Result<Vec<ClientConsumePushMessage>, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut messages = Vec::with_capacity(count);
    for _ in 0..count {
        let msg_id = read_string16(&mut buf)?;
        let fairness_key = read_string16(&mut buf)?;
        let attempt_count = read_u32(&mut buf)?;
        let header_count = read_u8(&mut buf)? as usize;
        let mut headers = HashMap::with_capacity(header_count);
        for _ in 0..header_count {
            let k = read_string16(&mut buf)?;
            let v = read_string16(&mut buf)?;
            headers.insert(k, v);
        }
        let payload_len = read_u32(&mut buf)? as usize;
        if buf.remaining() < payload_len {
            return Err(FibpCodecError::InvalidPayload {
                reason: format!(
                    "payload length {payload_len} exceeds remaining {} bytes",
                    buf.remaining()
                ),
            });
        }
        let payload = buf.split_to(payload_len).to_vec();
        messages.push(ClientConsumePushMessage {
            msg_id,
            fairness_key,
            attempt_count,
            headers,
            payload,
        });
    }
    Ok(messages)
}

// ---------------------------------------------------------------------------
// Flow (credit update)
// ---------------------------------------------------------------------------

/// Decode a flow (credit) frame payload.
pub fn decode_flow(mut buf: Bytes) -> Result<u32, FibpCodecError> {
    read_u32(&mut buf)
}

/// Encode a flow (credit update) payload.
pub fn encode_flow(credits: u32) -> Bytes {
    let mut buf = BytesMut::with_capacity(4);
    buf.put_u32(credits);
    buf.freeze()
}

// ---------------------------------------------------------------------------
// Ack request / response
// ---------------------------------------------------------------------------

/// A single item in an ack request.
#[derive(Debug)]
pub struct AckItem {
    pub queue: String,
    pub msg_id: String,
}

/// Decode an ack request payload.
pub fn decode_ack_request(mut buf: Bytes) -> Result<Vec<AckItem>, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let queue = read_string16(&mut buf)?;
        let msg_id = read_string16(&mut buf)?;
        items.push(AckItem { queue, msg_id });
    }
    Ok(items)
}

/// Encode an ack request payload (client-side).
pub fn encode_ack_request(items: &[(&str, &str)]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(items.len() as u16);
    for (queue, msg_id) in items {
        write_string16(&mut buf, queue);
        write_string16(&mut buf, msg_id);
    }
    buf.freeze()
}

/// A single item in a nack request.
#[derive(Debug)]
pub struct NackItem {
    pub queue: String,
    pub msg_id: String,
    pub error: String,
}

/// Decode a nack request payload.
pub fn decode_nack_request(mut buf: Bytes) -> Result<Vec<NackItem>, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let queue = read_string16(&mut buf)?;
        let msg_id = read_string16(&mut buf)?;
        let error = read_string16(&mut buf)?;
        items.push(NackItem {
            queue,
            msg_id,
            error,
        });
    }
    Ok(items)
}

/// Encode a nack request payload (client-side).
pub fn encode_nack_request(items: &[(&str, &str, &str)]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(items.len() as u16);
    for (queue, msg_id, error) in items {
        write_string16(&mut buf, queue);
        write_string16(&mut buf, msg_id);
        write_string16(&mut buf, error);
    }
    buf.freeze()
}

/// A single result in the ack/nack response.
pub enum AckNackResultItem {
    Ok,
    Err { code: u16, message: String },
}

/// Encode an ack or nack response payload.
pub fn encode_ack_nack_response(results: &[AckNackResultItem]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(results.len() as u16);
    for item in results {
        match item {
            AckNackResultItem::Ok => {
                buf.put_u8(1);
            }
            AckNackResultItem::Err { code, message } => {
                buf.put_u8(0);
                buf.put_u16(*code);
                write_string16(&mut buf, message);
            }
        }
    }
    buf.freeze()
}

/// Decode an ack/nack response payload (client-side).
pub fn decode_ack_nack_response(mut buf: Bytes) -> Result<Vec<AckNackResultItem>, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = read_u8(&mut buf)?;
        if tag == 1 {
            results.push(AckNackResultItem::Ok);
        } else {
            let code = read_u16(&mut buf)?;
            let message = read_string16(&mut buf)?;
            results.push(AckNackResultItem::Err { code, message });
        }
    }
    Ok(results)
}

// ===========================================================================
// Admin operations — binary wire encoding
// ===========================================================================

// ---------------------------------------------------------------------------
// CreateQueue
// ---------------------------------------------------------------------------

/// Parsed CreateQueue request.
#[derive(Debug)]
pub struct CreateQueueRequest {
    pub name: String,
    pub on_enqueue_script: String,
    pub on_failure_script: String,
    pub visibility_timeout_ms: u64,
}

/// Encode a CreateQueue request payload.
pub fn encode_create_queue_request(req: &CreateQueueRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.name);
    write_string16(&mut buf, &req.on_enqueue_script);
    write_string16(&mut buf, &req.on_failure_script);
    buf.put_u64(req.visibility_timeout_ms);
    buf.freeze()
}

/// Decode a CreateQueue request payload.
pub fn decode_create_queue_request(mut buf: Bytes) -> Result<CreateQueueRequest, FibpCodecError> {
    let name = read_string16(&mut buf)?;
    let on_enqueue_script = read_string16(&mut buf)?;
    let on_failure_script = read_string16(&mut buf)?;
    let visibility_timeout_ms = read_u64(&mut buf)?;
    Ok(CreateQueueRequest {
        name,
        on_enqueue_script,
        on_failure_script,
        visibility_timeout_ms,
    })
}

/// Parsed CreateQueue response.
#[derive(Debug)]
pub struct CreateQueueResponse {
    pub queue_id: String,
}

/// Encode a CreateQueue response payload.
pub fn encode_create_queue_response(resp: &CreateQueueResponse) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &resp.queue_id);
    buf.freeze()
}

/// Decode a CreateQueue response payload.
pub fn decode_create_queue_response(mut buf: Bytes) -> Result<CreateQueueResponse, FibpCodecError> {
    let queue_id = read_string16(&mut buf)?;
    Ok(CreateQueueResponse { queue_id })
}

// ---------------------------------------------------------------------------
// DeleteQueue
// ---------------------------------------------------------------------------

/// Parsed DeleteQueue request.
#[derive(Debug)]
pub struct DeleteQueueRequest {
    pub queue: String,
}

/// Encode a DeleteQueue request payload.
pub fn encode_delete_queue_request(req: &DeleteQueueRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.queue);
    buf.freeze()
}

/// Decode a DeleteQueue request payload.
pub fn decode_delete_queue_request(mut buf: Bytes) -> Result<DeleteQueueRequest, FibpCodecError> {
    let queue = read_string16(&mut buf)?;
    Ok(DeleteQueueRequest { queue })
}

// ---------------------------------------------------------------------------
// QueueStats
// ---------------------------------------------------------------------------

/// Parsed GetStats request.
#[derive(Debug)]
pub struct GetStatsRequest {
    pub queue: String,
}

/// Encode a GetStats request.
pub fn encode_get_stats_request(req: &GetStatsRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.queue);
    buf.freeze()
}

/// Decode a GetStats request.
pub fn decode_get_stats_request(mut buf: Bytes) -> Result<GetStatsRequest, FibpCodecError> {
    let queue = read_string16(&mut buf)?;
    Ok(GetStatsRequest { queue })
}

/// Per-fairness-key stats in a GetStats response.
#[derive(Debug, Clone)]
pub struct PerFairnessKeyStats {
    pub key: String,
    pub pending_count: u64,
    pub current_deficit: i64,
    pub weight: u32,
}

/// Per-throttle-key stats in a GetStats response.
#[derive(Debug, Clone)]
pub struct PerThrottleKeyStats {
    pub key: String,
    pub tokens: f64,
    pub rate_per_second: f64,
    pub burst: f64,
}

/// Parsed GetStats response.
#[derive(Debug)]
pub struct GetStatsResponse {
    pub depth: u64,
    pub in_flight: u64,
    pub active_fairness_keys: u64,
    pub active_consumers: u32,
    pub quantum: u32,
    pub per_key_stats: Vec<PerFairnessKeyStats>,
    pub per_throttle_stats: Vec<PerThrottleKeyStats>,
    pub leader_node_id: u64,
    pub replication_count: u32,
}

/// Encode a GetStats response.
pub fn encode_get_stats_response(resp: &GetStatsResponse) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u64(resp.depth);
    buf.put_u64(resp.in_flight);
    buf.put_u64(resp.active_fairness_keys);
    buf.put_u32(resp.active_consumers);
    buf.put_u32(resp.quantum);
    buf.put_u16(resp.per_key_stats.len() as u16);
    for s in &resp.per_key_stats {
        write_string16(&mut buf, &s.key);
        buf.put_u64(s.pending_count);
        buf.put_i64(s.current_deficit);
        buf.put_u32(s.weight);
    }
    buf.put_u16(resp.per_throttle_stats.len() as u16);
    for s in &resp.per_throttle_stats {
        write_string16(&mut buf, &s.key);
        buf.put_f64(s.tokens);
        buf.put_f64(s.rate_per_second);
        buf.put_f64(s.burst);
    }
    buf.put_u64(resp.leader_node_id);
    buf.put_u32(resp.replication_count);
    buf.freeze()
}

/// Decode a GetStats response.
pub fn decode_get_stats_response(mut buf: Bytes) -> Result<GetStatsResponse, FibpCodecError> {
    let depth = read_u64(&mut buf)?;
    let in_flight = read_u64(&mut buf)?;
    let active_fairness_keys = read_u64(&mut buf)?;
    let active_consumers = read_u32(&mut buf)?;
    let quantum = read_u32(&mut buf)?;
    let key_count = read_u16(&mut buf)? as usize;
    let mut per_key_stats = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        let key = read_string16(&mut buf)?;
        let pending_count = read_u64(&mut buf)?;
        let current_deficit = read_i64(&mut buf)?;
        let weight = read_u32(&mut buf)?;
        per_key_stats.push(PerFairnessKeyStats {
            key,
            pending_count,
            current_deficit,
            weight,
        });
    }
    let throttle_count = read_u16(&mut buf)? as usize;
    let mut per_throttle_stats = Vec::with_capacity(throttle_count);
    for _ in 0..throttle_count {
        let key = read_string16(&mut buf)?;
        let tokens = read_f64(&mut buf)?;
        let rate_per_second = read_f64(&mut buf)?;
        let burst = read_f64(&mut buf)?;
        per_throttle_stats.push(PerThrottleKeyStats {
            key,
            tokens,
            rate_per_second,
            burst,
        });
    }
    let leader_node_id = read_u64(&mut buf)?;
    let replication_count = read_u32(&mut buf)?;
    Ok(GetStatsResponse {
        depth,
        in_flight,
        active_fairness_keys,
        active_consumers,
        quantum,
        per_key_stats,
        per_throttle_stats,
        leader_node_id,
        replication_count,
    })
}

// ---------------------------------------------------------------------------
// ListQueues
// ---------------------------------------------------------------------------

/// Queue info in a ListQueues response.
#[derive(Debug, Clone)]
pub struct QueueInfo {
    pub name: String,
    pub depth: u64,
    pub in_flight: u64,
    pub active_consumers: u32,
    pub leader_node_id: u64,
}

/// Parsed ListQueues response.
#[derive(Debug)]
pub struct ListQueuesResponse {
    pub queues: Vec<QueueInfo>,
    pub cluster_node_count: u32,
}

/// Encode a ListQueues request (empty payload, but explicit for consistency).
pub fn encode_list_queues_request() -> Bytes {
    Bytes::new()
}

/// Encode a ListQueues response.
pub fn encode_list_queues_response(resp: &ListQueuesResponse) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(resp.queues.len() as u16);
    for q in &resp.queues {
        write_string16(&mut buf, &q.name);
        buf.put_u64(q.depth);
        buf.put_u64(q.in_flight);
        buf.put_u32(q.active_consumers);
        buf.put_u64(q.leader_node_id);
    }
    buf.put_u32(resp.cluster_node_count);
    buf.freeze()
}

/// Decode a ListQueues response.
pub fn decode_list_queues_response(mut buf: Bytes) -> Result<ListQueuesResponse, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut queues = Vec::with_capacity(count);
    for _ in 0..count {
        let name = read_string16(&mut buf)?;
        let depth = read_u64(&mut buf)?;
        let in_flight = read_u64(&mut buf)?;
        let active_consumers = read_u32(&mut buf)?;
        let leader_node_id = read_u64(&mut buf)?;
        queues.push(QueueInfo {
            name,
            depth,
            in_flight,
            active_consumers,
            leader_node_id,
        });
    }
    let cluster_node_count = read_u32(&mut buf)?;
    Ok(ListQueuesResponse {
        queues,
        cluster_node_count,
    })
}

// ---------------------------------------------------------------------------
// Redrive
// ---------------------------------------------------------------------------

/// Parsed Redrive request.
#[derive(Debug)]
pub struct RedriveRequest {
    pub dlq_queue: String,
    pub count: u64,
}

/// Encode a Redrive request.
pub fn encode_redrive_request(req: &RedriveRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.dlq_queue);
    buf.put_u64(req.count);
    buf.freeze()
}

/// Decode a Redrive request.
pub fn decode_redrive_request(mut buf: Bytes) -> Result<RedriveRequest, FibpCodecError> {
    let dlq_queue = read_string16(&mut buf)?;
    let count = read_u64(&mut buf)?;
    Ok(RedriveRequest { dlq_queue, count })
}

/// Parsed Redrive response.
#[derive(Debug)]
pub struct RedriveResponse {
    pub redriven: u64,
}

/// Encode a Redrive response.
pub fn encode_redrive_response(resp: &RedriveResponse) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u64(resp.redriven);
    buf.freeze()
}

/// Decode a Redrive response.
pub fn decode_redrive_response(mut buf: Bytes) -> Result<RedriveResponse, FibpCodecError> {
    let redriven = read_u64(&mut buf)?;
    Ok(RedriveResponse { redriven })
}

// ---------------------------------------------------------------------------
// Config operations
// ---------------------------------------------------------------------------

/// Parsed SetConfig request.
#[derive(Debug)]
pub struct SetConfigRequest {
    pub key: String,
    pub value: String,
}

/// Encode a SetConfig request.
pub fn encode_set_config_request(req: &SetConfigRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.key);
    write_string16(&mut buf, &req.value);
    buf.freeze()
}

/// Decode a SetConfig request.
pub fn decode_set_config_request(mut buf: Bytes) -> Result<SetConfigRequest, FibpCodecError> {
    let key = read_string16(&mut buf)?;
    let value = read_string16(&mut buf)?;
    Ok(SetConfigRequest { key, value })
}

/// Parsed GetConfig request.
#[derive(Debug)]
pub struct GetConfigRequest {
    pub key: String,
}

/// Encode a GetConfig request.
pub fn encode_get_config_request(req: &GetConfigRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.key);
    buf.freeze()
}

/// Decode a GetConfig request.
pub fn decode_get_config_request(mut buf: Bytes) -> Result<GetConfigRequest, FibpCodecError> {
    let key = read_string16(&mut buf)?;
    Ok(GetConfigRequest { key })
}

/// Parsed GetConfig response.
#[derive(Debug)]
pub struct GetConfigResponse {
    pub value: String,
}

/// Encode a GetConfig response.
pub fn encode_get_config_response(resp: &GetConfigResponse) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &resp.value);
    buf.freeze()
}

/// Decode a GetConfig response.
pub fn decode_get_config_response(mut buf: Bytes) -> Result<GetConfigResponse, FibpCodecError> {
    let value = read_string16(&mut buf)?;
    Ok(GetConfigResponse { value })
}

/// A config entry in a ListConfig response.
#[derive(Debug, Clone)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
}

/// Parsed ListConfig request.
#[derive(Debug)]
pub struct ListConfigRequest {
    pub prefix: String,
}

/// Encode a ListConfig request.
pub fn encode_list_config_request(req: &ListConfigRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.prefix);
    buf.freeze()
}

/// Decode a ListConfig request.
pub fn decode_list_config_request(mut buf: Bytes) -> Result<ListConfigRequest, FibpCodecError> {
    let prefix = read_string16(&mut buf)?;
    Ok(ListConfigRequest { prefix })
}

/// Parsed ListConfig response.
#[derive(Debug)]
pub struct ListConfigResponse {
    pub entries: Vec<ConfigEntry>,
    pub total_count: u32,
}

/// Encode a ListConfig response.
pub fn encode_list_config_response(resp: &ListConfigResponse) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(resp.entries.len() as u16);
    for entry in &resp.entries {
        write_string16(&mut buf, &entry.key);
        write_string16(&mut buf, &entry.value);
    }
    buf.put_u32(resp.total_count);
    buf.freeze()
}

/// Decode a ListConfig response.
pub fn decode_list_config_response(mut buf: Bytes) -> Result<ListConfigResponse, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let key = read_string16(&mut buf)?;
        let value = read_string16(&mut buf)?;
        entries.push(ConfigEntry { key, value });
    }
    let total_count = read_u32(&mut buf)?;
    Ok(ListConfigResponse {
        entries,
        total_count,
    })
}

// ---------------------------------------------------------------------------
// Auth CRUD operations
// ---------------------------------------------------------------------------

/// Parsed CreateApiKey request.
#[derive(Debug)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub expires_at_ms: u64,
    pub is_superadmin: bool,
}

/// Encode a CreateApiKey request.
pub fn encode_create_api_key_request(req: &CreateApiKeyRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.name);
    buf.put_u64(req.expires_at_ms);
    buf.put_u8(u8::from(req.is_superadmin));
    buf.freeze()
}

/// Decode a CreateApiKey request.
pub fn decode_create_api_key_request(
    mut buf: Bytes,
) -> Result<CreateApiKeyRequest, FibpCodecError> {
    let name = read_string16(&mut buf)?;
    let expires_at_ms = read_u64(&mut buf)?;
    let is_superadmin = read_u8(&mut buf)? != 0;
    Ok(CreateApiKeyRequest {
        name,
        expires_at_ms,
        is_superadmin,
    })
}

/// Parsed CreateApiKey response.
#[derive(Debug)]
pub struct CreateApiKeyResponse {
    pub key_id: String,
    pub key: String,
    pub is_superadmin: bool,
}

/// Encode a CreateApiKey response.
pub fn encode_create_api_key_response(resp: &CreateApiKeyResponse) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &resp.key_id);
    write_string16(&mut buf, &resp.key);
    buf.put_u8(u8::from(resp.is_superadmin));
    buf.freeze()
}

/// Decode a CreateApiKey response.
pub fn decode_create_api_key_response(
    mut buf: Bytes,
) -> Result<CreateApiKeyResponse, FibpCodecError> {
    let key_id = read_string16(&mut buf)?;
    let key = read_string16(&mut buf)?;
    let is_superadmin = read_u8(&mut buf)? != 0;
    Ok(CreateApiKeyResponse {
        key_id,
        key,
        is_superadmin,
    })
}

/// Parsed RevokeApiKey request.
#[derive(Debug)]
pub struct RevokeApiKeyRequest {
    pub key_id: String,
}

/// Encode a RevokeApiKey request.
pub fn encode_revoke_api_key_request(req: &RevokeApiKeyRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.key_id);
    buf.freeze()
}

/// Decode a RevokeApiKey request.
pub fn decode_revoke_api_key_request(
    mut buf: Bytes,
) -> Result<RevokeApiKeyRequest, FibpCodecError> {
    let key_id = read_string16(&mut buf)?;
    Ok(RevokeApiKeyRequest { key_id })
}

/// A single API key info entry.
#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub key_id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub is_superadmin: bool,
}

/// Parsed ListApiKeys response.
#[derive(Debug)]
pub struct ListApiKeysResponse {
    pub keys: Vec<ApiKeyInfo>,
}

/// Encode a ListApiKeys response.
pub fn encode_list_api_keys_response(resp: &ListApiKeysResponse) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(resp.keys.len() as u16);
    for k in &resp.keys {
        write_string16(&mut buf, &k.key_id);
        write_string16(&mut buf, &k.name);
        buf.put_u64(k.created_at_ms);
        buf.put_u64(k.expires_at_ms);
        buf.put_u8(u8::from(k.is_superadmin));
    }
    buf.freeze()
}

/// Decode a ListApiKeys response.
pub fn decode_list_api_keys_response(
    mut buf: Bytes,
) -> Result<ListApiKeysResponse, FibpCodecError> {
    let count = read_u16(&mut buf)? as usize;
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let key_id = read_string16(&mut buf)?;
        let name = read_string16(&mut buf)?;
        let created_at_ms = read_u64(&mut buf)?;
        let expires_at_ms = read_u64(&mut buf)?;
        let is_superadmin = read_u8(&mut buf)? != 0;
        keys.push(ApiKeyInfo {
            key_id,
            name,
            created_at_ms,
            expires_at_ms,
            is_superadmin,
        });
    }
    Ok(ListApiKeysResponse { keys })
}

/// A single ACL permission.
#[derive(Debug, Clone)]
pub struct AclPermission {
    pub kind: String,
    pub pattern: String,
}

/// Parsed SetAcl request.
#[derive(Debug)]
pub struct SetAclRequest {
    pub key_id: String,
    pub permissions: Vec<AclPermission>,
}

/// Encode a SetAcl request.
pub fn encode_set_acl_request(req: &SetAclRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.key_id);
    buf.put_u16(req.permissions.len() as u16);
    for p in &req.permissions {
        write_string16(&mut buf, &p.kind);
        write_string16(&mut buf, &p.pattern);
    }
    buf.freeze()
}

/// Decode a SetAcl request.
pub fn decode_set_acl_request(mut buf: Bytes) -> Result<SetAclRequest, FibpCodecError> {
    let key_id = read_string16(&mut buf)?;
    let count = read_u16(&mut buf)? as usize;
    let mut permissions = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = read_string16(&mut buf)?;
        let pattern = read_string16(&mut buf)?;
        permissions.push(AclPermission { kind, pattern });
    }
    Ok(SetAclRequest {
        key_id,
        permissions,
    })
}

/// Parsed GetAcl request.
#[derive(Debug)]
pub struct GetAclRequest {
    pub key_id: String,
}

/// Encode a GetAcl request.
pub fn encode_get_acl_request(req: &GetAclRequest) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &req.key_id);
    buf.freeze()
}

/// Decode a GetAcl request.
pub fn decode_get_acl_request(mut buf: Bytes) -> Result<GetAclRequest, FibpCodecError> {
    let key_id = read_string16(&mut buf)?;
    Ok(GetAclRequest { key_id })
}

/// Parsed GetAcl response.
#[derive(Debug)]
pub struct GetAclResponse {
    pub key_id: String,
    pub permissions: Vec<AclPermission>,
    pub is_superadmin: bool,
}

/// Encode a GetAcl response.
pub fn encode_get_acl_response(resp: &GetAclResponse) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, &resp.key_id);
    buf.put_u16(resp.permissions.len() as u16);
    for p in &resp.permissions {
        write_string16(&mut buf, &p.kind);
        write_string16(&mut buf, &p.pattern);
    }
    buf.put_u8(u8::from(resp.is_superadmin));
    buf.freeze()
}

/// Decode a GetAcl response.
pub fn decode_get_acl_response(mut buf: Bytes) -> Result<GetAclResponse, FibpCodecError> {
    let key_id = read_string16(&mut buf)?;
    let count = read_u16(&mut buf)? as usize;
    let mut permissions = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = read_string16(&mut buf)?;
        let pattern = read_string16(&mut buf)?;
        permissions.push(AclPermission { kind, pattern });
    }
    let is_superadmin = read_u8(&mut buf)? != 0;
    Ok(GetAclResponse {
        key_id,
        permissions,
        is_superadmin,
    })
}

// ---------------------------------------------------------------------------
// Primitive readers / writers (public so both server and client can use them)
// ---------------------------------------------------------------------------

pub fn read_u8(buf: &mut Bytes) -> Result<u8, FibpCodecError> {
    if buf.remaining() < 1 {
        return Err(FibpCodecError::InvalidPayload {
            reason: "unexpected end of payload reading u8".into(),
        });
    }
    Ok(buf.get_u8())
}

pub fn read_u16(buf: &mut Bytes) -> Result<u16, FibpCodecError> {
    if buf.remaining() < 2 {
        return Err(FibpCodecError::InvalidPayload {
            reason: "unexpected end of payload reading u16".into(),
        });
    }
    Ok(buf.get_u16())
}

pub fn read_u32(buf: &mut Bytes) -> Result<u32, FibpCodecError> {
    if buf.remaining() < 4 {
        return Err(FibpCodecError::InvalidPayload {
            reason: "unexpected end of payload reading u32".into(),
        });
    }
    Ok(buf.get_u32())
}

pub fn read_u64(buf: &mut Bytes) -> Result<u64, FibpCodecError> {
    if buf.remaining() < 8 {
        return Err(FibpCodecError::InvalidPayload {
            reason: "unexpected end of payload reading u64".into(),
        });
    }
    Ok(buf.get_u64())
}

pub fn read_i64(buf: &mut Bytes) -> Result<i64, FibpCodecError> {
    if buf.remaining() < 8 {
        return Err(FibpCodecError::InvalidPayload {
            reason: "unexpected end of payload reading i64".into(),
        });
    }
    Ok(buf.get_i64())
}

pub fn read_f64(buf: &mut Bytes) -> Result<f64, FibpCodecError> {
    if buf.remaining() < 8 {
        return Err(FibpCodecError::InvalidPayload {
            reason: "unexpected end of payload reading f64".into(),
        });
    }
    Ok(buf.get_f64())
}

pub fn read_string16(buf: &mut Bytes) -> Result<String, FibpCodecError> {
    let len = read_u16(buf)? as usize;
    if buf.remaining() < len {
        return Err(FibpCodecError::InvalidPayload {
            reason: format!(
                "string length {len} exceeds remaining {} bytes",
                buf.remaining()
            ),
        });
    }
    let raw = buf.split_to(len);
    String::from_utf8(raw.to_vec()).map_err(|e| FibpCodecError::InvalidPayload {
        reason: format!("invalid utf8: {e}"),
    })
}

pub fn write_string16(buf: &mut BytesMut, s: &str) {
    buf.put_u16(s.len() as u16);
    buf.extend_from_slice(s.as_bytes());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_enqueue_request() {
        let mut buf = BytesMut::new();
        let queue = "test-queue";
        write_string16(&mut buf, queue);
        buf.put_u16(2); // 2 messages

        // Message 1: one header, small payload
        buf.put_u8(1); // header_count
        write_string16(&mut buf, "key1");
        write_string16(&mut buf, "val1");
        buf.put_u32(5);
        buf.extend_from_slice(b"hello");

        // Message 2: no headers, empty payload
        buf.put_u8(0);
        buf.put_u32(0);

        let req = decode_enqueue_request(buf.freeze()).unwrap();
        assert_eq!(req.queue, "test-queue");
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].headers.get("key1").unwrap(), "val1");
        assert_eq!(&req.messages[0].payload[..], b"hello");
        assert!(req.messages[1].headers.is_empty());
        assert!(req.messages[1].payload.is_empty());
    }

    #[test]
    fn round_trip_enqueue_response() {
        let results = vec![
            EnqueueResultItem::Ok {
                msg_id: "abc-123".to_string(),
            },
            EnqueueResultItem::Err {
                code: enqueue_err::QUEUE_NOT_FOUND,
                message: "queue not found".to_string(),
            },
        ];
        let encoded = encode_enqueue_response(&results);
        let decoded = decode_enqueue_response(encoded).unwrap();
        assert_eq!(decoded.len(), 2);
        match &decoded[0] {
            EnqueueResultItem::Ok { msg_id } => assert_eq!(msg_id, "abc-123"),
            _ => panic!("expected Ok"),
        }
        match &decoded[1] {
            EnqueueResultItem::Err { code, message } => {
                assert_eq!(*code, enqueue_err::QUEUE_NOT_FOUND);
                assert_eq!(message, "queue not found");
            }
            _ => panic!("expected Err"),
        }
    }

    #[test]
    fn round_trip_ack_request() {
        let mut buf = BytesMut::new();
        buf.put_u16(1); // 1 item
        write_string16(&mut buf, "q1");
        write_string16(&mut buf, "msg-1");

        let items = decode_ack_request(buf.freeze()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].queue, "q1");
        assert_eq!(items[0].msg_id, "msg-1");
    }

    #[test]
    fn round_trip_nack_request() {
        let mut buf = BytesMut::new();
        buf.put_u16(1);
        write_string16(&mut buf, "q1");
        write_string16(&mut buf, "msg-1");
        write_string16(&mut buf, "processing failed");

        let items = decode_nack_request(buf.freeze()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].queue, "q1");
        assert_eq!(items[0].msg_id, "msg-1");
        assert_eq!(items[0].error, "processing failed");
    }

    #[test]
    fn round_trip_consume_request() {
        let mut buf = BytesMut::new();
        write_string16(&mut buf, "my-queue");
        buf.put_u32(100);

        let req = decode_consume_request(buf.freeze()).unwrap();
        assert_eq!(req.queue, "my-queue");
        assert_eq!(req.initial_credits, 100);
    }

    #[test]
    fn round_trip_flow() {
        let mut buf = BytesMut::new();
        buf.put_u32(42);
        let credits = decode_flow(buf.freeze()).unwrap();
        assert_eq!(credits, 42);
    }

    #[test]
    fn round_trip_create_queue() {
        let req = CreateQueueRequest {
            name: "orders".to_string(),
            on_enqueue_script: "return msg".to_string(),
            on_failure_script: String::new(),
            visibility_timeout_ms: 30_000,
        };
        let encoded = encode_create_queue_request(&req);
        let decoded = decode_create_queue_request(encoded).unwrap();
        assert_eq!(decoded.name, "orders");
        assert_eq!(decoded.on_enqueue_script, "return msg");
        assert!(decoded.on_failure_script.is_empty());
        assert_eq!(decoded.visibility_timeout_ms, 30_000);
    }

    #[test]
    fn round_trip_get_stats_response() {
        let resp = GetStatsResponse {
            depth: 100,
            in_flight: 5,
            active_fairness_keys: 3,
            active_consumers: 2,
            quantum: 10,
            per_key_stats: vec![PerFairnessKeyStats {
                key: "default".to_string(),
                pending_count: 90,
                current_deficit: -5,
                weight: 1,
            }],
            per_throttle_stats: vec![PerThrottleKeyStats {
                key: "api".to_string(),
                tokens: 9.5,
                rate_per_second: 10.0,
                burst: 20.0,
            }],
            leader_node_id: 1,
            replication_count: 3,
        };
        let encoded = encode_get_stats_response(&resp);
        let decoded = decode_get_stats_response(encoded).unwrap();
        assert_eq!(decoded.depth, 100);
        assert_eq!(decoded.in_flight, 5);
        assert_eq!(decoded.per_key_stats.len(), 1);
        assert_eq!(decoded.per_key_stats[0].current_deficit, -5);
        assert_eq!(decoded.per_throttle_stats.len(), 1);
        assert!((decoded.per_throttle_stats[0].tokens - 9.5).abs() < f64::EPSILON);
        assert_eq!(decoded.leader_node_id, 1);
        assert_eq!(decoded.replication_count, 3);
    }

    #[test]
    fn round_trip_list_queues_response() {
        let resp = ListQueuesResponse {
            queues: vec![QueueInfo {
                name: "orders".to_string(),
                depth: 50,
                in_flight: 3,
                active_consumers: 1,
                leader_node_id: 2,
            }],
            cluster_node_count: 3,
        };
        let encoded = encode_list_queues_response(&resp);
        let decoded = decode_list_queues_response(encoded).unwrap();
        assert_eq!(decoded.queues.len(), 1);
        assert_eq!(decoded.queues[0].name, "orders");
        assert_eq!(decoded.cluster_node_count, 3);
    }

    #[test]
    fn round_trip_create_api_key() {
        let req = CreateApiKeyRequest {
            name: "test-key".to_string(),
            expires_at_ms: 0,
            is_superadmin: true,
        };
        let encoded = encode_create_api_key_request(&req);
        let decoded = decode_create_api_key_request(encoded).unwrap();
        assert_eq!(decoded.name, "test-key");
        assert_eq!(decoded.expires_at_ms, 0);
        assert!(decoded.is_superadmin);
    }

    #[test]
    fn round_trip_get_acl_response() {
        let resp = GetAclResponse {
            key_id: "k1".to_string(),
            permissions: vec![AclPermission {
                kind: "produce".to_string(),
                pattern: "orders.*".to_string(),
            }],
            is_superadmin: false,
        };
        let encoded = encode_get_acl_response(&resp);
        let decoded = decode_get_acl_response(encoded).unwrap();
        assert_eq!(decoded.key_id, "k1");
        assert_eq!(decoded.permissions.len(), 1);
        assert_eq!(decoded.permissions[0].kind, "produce");
        assert!(!decoded.is_superadmin);
    }

    #[test]
    fn decode_truncated_enqueue_request_fails() {
        let mut buf = BytesMut::new();
        write_string16(&mut buf, "q");
        buf.put_u16(5);
        let err = decode_enqueue_request(buf.freeze()).unwrap_err();
        assert!(
            matches!(err, FibpCodecError::InvalidPayload { .. }),
            "expected InvalidPayload, got: {err:?}"
        );
    }
}
