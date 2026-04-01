use bytes::Bytes;

use crate::error::FrameError;
use crate::error_code::ErrorCode;
use crate::frame::{PayloadReader, PayloadWriter, RawFrame};
use crate::opcode::Opcode;

use super::check_count;
use super::ProtocolMessage;

// ---------------------------------------------------------------------------
// Admin frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateQueueRequest {
    pub name: String,
    pub on_enqueue_script: Option<String>,
    pub on_failure_script: Option<String>,
    pub visibility_timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct CreateQueueResponse {
    pub error_code: ErrorCode,
    pub queue_id: String,
}

#[derive(Debug, Clone)]
pub struct DeleteQueueRequest {
    pub queue: String,
}

#[derive(Debug, Clone)]
pub struct DeleteQueueResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct GetStatsRequest {
    pub queue: String,
}

#[derive(Debug, Clone)]
pub struct FairnessKeyStat {
    pub key: String,
    pub pending_count: u64,
    pub current_deficit: i64,
    pub weight: u32,
}

#[derive(Debug, Clone)]
pub struct ThrottleKeyStat {
    pub key: String,
    pub tokens: f64,
    pub rate_per_second: f64,
    pub burst: f64,
}

#[derive(Debug, Clone)]
pub struct GetStatsResponse {
    pub error_code: ErrorCode,
    pub depth: u64,
    pub in_flight: u64,
    pub active_fairness_keys: u64,
    pub active_consumers: u32,
    pub quantum: u32,
    pub leader_node_id: u64,
    pub replication_count: u32,
    pub per_key_stats: Vec<FairnessKeyStat>,
    pub per_throttle_stats: Vec<ThrottleKeyStat>,
}

#[derive(Debug, Clone)]
pub struct ListQueuesRequest;

#[derive(Debug, Clone)]
pub struct QueueInfo {
    pub name: String,
    pub depth: u64,
    pub in_flight: u64,
    pub active_consumers: u32,
    pub leader_node_id: u64,
}

#[derive(Debug, Clone)]
pub struct ListQueuesResponse {
    pub error_code: ErrorCode,
    pub cluster_node_count: u32,
    pub queues: Vec<QueueInfo>,
}

#[derive(Debug, Clone)]
pub struct SetConfigRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct SetConfigResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct GetConfigRequest {
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct GetConfigResponse {
    pub error_code: ErrorCode,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ListConfigRequest {
    pub prefix: String,
}

#[derive(Debug, Clone)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ListConfigResponse {
    pub error_code: ErrorCode,
    pub entries: Vec<ConfigEntry>,
}

#[derive(Debug, Clone)]
pub struct RedriveRequest {
    pub dlq_queue: String,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct RedriveResponse {
    pub error_code: ErrorCode,
    pub redriven: u64,
}

// ---------------------------------------------------------------------------
// Encode/decode implementations
// ---------------------------------------------------------------------------

impl ProtocolMessage for CreateQueueRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.name);
        w.put_optional_string(&self.on_enqueue_script);
        w.put_optional_string(&self.on_failure_script);
        w.put_u64(self.visibility_timeout_ms);
        RawFrame {
            opcode: Opcode::CreateQueue as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let name = r.read_string()?;
        let on_enqueue_script = r.read_optional_string()?;
        let on_failure_script = r.read_optional_string()?;
        let visibility_timeout_ms = r.read_u64()?;
        Ok(Self {
            name,
            on_enqueue_script,
            on_failure_script,
            visibility_timeout_ms,
        })
    }
}

impl ProtocolMessage for CreateQueueResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.queue_id);
        RawFrame {
            opcode: Opcode::CreateQueueResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let queue_id = r.read_string()?;
        Ok(Self {
            error_code,
            queue_id,
        })
    }
}

impl ProtocolMessage for DeleteQueueRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.queue);
        RawFrame {
            opcode: Opcode::DeleteQueue as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let queue = r.read_string()?;
        Ok(Self { queue })
    }
}

impl ProtocolMessage for DeleteQueueResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::DeleteQueueResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl ProtocolMessage for GetStatsRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.queue);
        RawFrame {
            opcode: Opcode::GetStats as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let queue = r.read_string()?;
        Ok(Self { queue })
    }
}

impl ProtocolMessage for GetStatsResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u64(self.depth);
        w.put_u64(self.in_flight);
        w.put_u64(self.active_fairness_keys);
        w.put_u32(self.active_consumers);
        w.put_u32(self.quantum);
        w.put_u64(self.leader_node_id);
        w.put_u32(self.replication_count);
        let key_count = self.per_key_stats.len().min(u16::MAX as usize);
        if key_count < self.per_key_stats.len() {
            tracing::warn!(
                total = self.per_key_stats.len(),
                included = key_count,
                "per_key_stats truncated: count exceeds u16::MAX (see #164)"
            );
        }
        w.put_u16(key_count as u16);
        for s in self.per_key_stats.iter().take(key_count) {
            w.put_string(&s.key);
            w.put_u64(s.pending_count);
            w.put_i64(s.current_deficit);
            w.put_u32(s.weight);
        }
        let throttle_count = self.per_throttle_stats.len().min(u16::MAX as usize);
        if throttle_count < self.per_throttle_stats.len() {
            tracing::warn!(
                total = self.per_throttle_stats.len(),
                included = throttle_count,
                "per_throttle_stats truncated: count exceeds u16::MAX (see #164)"
            );
        }
        w.put_u16(throttle_count as u16);
        for s in self.per_throttle_stats.iter().take(throttle_count) {
            w.put_string(&s.key);
            w.put_f64(s.tokens);
            w.put_f64(s.rate_per_second);
            w.put_f64(s.burst);
        }
        RawFrame {
            opcode: Opcode::GetStatsResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let depth = r.read_u64()?;
        let in_flight = r.read_u64()?;
        let active_fairness_keys = r.read_u64()?;
        let active_consumers = r.read_u32()?;
        let quantum = r.read_u32()?;
        let leader_node_id = r.read_u64()?;
        let replication_count = r.read_u32()?;
        let per_key_count = r.read_u16()? as usize;
        check_count(per_key_count)?;
        let mut per_key_stats = Vec::with_capacity(per_key_count);
        for _ in 0..per_key_count {
            let key = r.read_string()?;
            let pending_count = r.read_u64()?;
            let current_deficit = r.read_i64()?;
            let weight = r.read_u32()?;
            per_key_stats.push(FairnessKeyStat {
                key,
                pending_count,
                current_deficit,
                weight,
            });
        }
        let per_throttle_count = r.read_u16()? as usize;
        check_count(per_throttle_count)?;
        let mut per_throttle_stats = Vec::with_capacity(per_throttle_count);
        for _ in 0..per_throttle_count {
            let key = r.read_string()?;
            let tokens = r.read_f64()?;
            let rate_per_second = r.read_f64()?;
            let burst = r.read_f64()?;
            per_throttle_stats.push(ThrottleKeyStat {
                key,
                tokens,
                rate_per_second,
                burst,
            });
        }
        Ok(Self {
            error_code,
            depth,
            in_flight,
            active_fairness_keys,
            active_consumers,
            quantum,
            leader_node_id,
            replication_count,
            per_key_stats,
            per_throttle_stats,
        })
    }
}

impl ProtocolMessage for ListQueuesRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        RawFrame {
            opcode: Opcode::ListQueues as u8,
            flags: 0,
            request_id,
            payload: Bytes::new(),
        }
    }

    fn decode(_payload: Bytes) -> Result<Self, FrameError> {
        Ok(Self)
    }
}

impl ProtocolMessage for ListQueuesResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u32(self.cluster_node_count);
        let queue_count = self.queues.len().min(u16::MAX as usize);
        if queue_count < self.queues.len() {
            tracing::warn!(
                total = self.queues.len(),
                included = queue_count,
                "queues list truncated: count exceeds u16::MAX (see #164)"
            );
        }
        w.put_u16(queue_count as u16);
        for q in self.queues.iter().take(queue_count) {
            w.put_string(&q.name);
            w.put_u64(q.depth);
            w.put_u64(q.in_flight);
            w.put_u32(q.active_consumers);
            w.put_u64(q.leader_node_id);
        }
        RawFrame {
            opcode: Opcode::ListQueuesResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let cluster_node_count = r.read_u32()?;
        let queue_count = r.read_u16()? as usize;
        check_count(queue_count)?;
        let mut queues = Vec::with_capacity(queue_count);
        for _ in 0..queue_count {
            let name = r.read_string()?;
            let depth = r.read_u64()?;
            let in_flight = r.read_u64()?;
            let active_consumers = r.read_u32()?;
            let leader_node_id = r.read_u64()?;
            queues.push(QueueInfo {
                name,
                depth,
                in_flight,
                active_consumers,
                leader_node_id,
            });
        }
        Ok(Self {
            error_code,
            cluster_node_count,
            queues,
        })
    }
}

impl ProtocolMessage for SetConfigRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key);
        w.put_string(&self.value);
        RawFrame {
            opcode: Opcode::SetConfig as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key = r.read_string()?;
        let value = r.read_string()?;
        Ok(Self { key, value })
    }
}

impl ProtocolMessage for SetConfigResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::SetConfigResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl ProtocolMessage for GetConfigRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key);
        RawFrame {
            opcode: Opcode::GetConfig as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key = r.read_string()?;
        Ok(Self { key })
    }
}

impl ProtocolMessage for GetConfigResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.value);
        RawFrame {
            opcode: Opcode::GetConfigResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let value = r.read_string()?;
        Ok(Self { error_code, value })
    }
}

impl ProtocolMessage for ListConfigRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.prefix);
        RawFrame {
            opcode: Opcode::ListConfig as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let prefix = r.read_string()?;
        Ok(Self { prefix })
    }
}

impl ProtocolMessage for ListConfigResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        let entry_count = self.entries.len().min(u16::MAX as usize);
        if entry_count < self.entries.len() {
            tracing::warn!(
                total = self.entries.len(),
                included = entry_count,
                "config entries truncated: count exceeds u16::MAX (see #164)"
            );
        }
        w.put_u16(entry_count as u16);
        for entry in self.entries.iter().take(entry_count) {
            w.put_string(&entry.key);
            w.put_string(&entry.value);
        }
        RawFrame {
            opcode: Opcode::ListConfigResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let key = r.read_string()?;
            let value = r.read_string()?;
            entries.push(ConfigEntry { key, value });
        }
        Ok(Self {
            error_code,
            entries,
        })
    }
}

impl ProtocolMessage for RedriveRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.dlq_queue);
        w.put_u64(self.count);
        RawFrame {
            opcode: Opcode::Redrive as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let dlq_queue = r.read_string()?;
        let count = r.read_u64()?;
        Ok(Self { dlq_queue, count })
    }
}

impl ProtocolMessage for RedriveResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u64(self.redriven);
        RawFrame {
            opcode: Opcode::RedriveResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let redriven = r.read_u64()?;
        Ok(Self {
            error_code,
            redriven,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_queue_round_trip() {
        let req = CreateQueueRequest {
            name: "my-queue".to_string(),
            on_enqueue_script: Some("lua code".to_string()),
            on_failure_script: None,
            visibility_timeout_ms: 60000,
        };
        let frame = req.encode(1);
        assert_eq!(frame.opcode, Opcode::CreateQueue as u8);
        let decoded = CreateQueueRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.name, "my-queue");
        assert_eq!(decoded.on_enqueue_script, Some("lua code".to_string()));
        assert_eq!(decoded.on_failure_script, None);
        assert_eq!(decoded.visibility_timeout_ms, 60000);
    }

    #[test]
    fn create_queue_response_round_trip() {
        let resp = CreateQueueResponse {
            error_code: ErrorCode::Ok,
            queue_id: "my-queue".to_string(),
        };
        let frame = resp.encode(1);
        let decoded = CreateQueueResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
        assert_eq!(decoded.queue_id, "my-queue");
    }

    #[test]
    fn delete_queue_round_trip() {
        let req = DeleteQueueRequest {
            queue: "my-queue".to_string(),
        };
        let frame = req.encode(2);
        let decoded = DeleteQueueRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.queue, "my-queue");

        let resp = DeleteQueueResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(2);
        let decoded = DeleteQueueResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
    }

    #[test]
    fn get_stats_round_trip() {
        let req = GetStatsRequest {
            queue: "q1".to_string(),
        };
        let frame = req.encode(3);
        let decoded = GetStatsRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.queue, "q1");

        let resp = GetStatsResponse {
            error_code: ErrorCode::Ok,
            depth: 100,
            in_flight: 5,
            active_fairness_keys: 3,
            active_consumers: 2,
            quantum: 10,
            leader_node_id: 1,
            replication_count: 3,
            per_key_stats: vec![FairnessKeyStat {
                key: "tenant-a".to_string(),
                pending_count: 50,
                current_deficit: -10,
                weight: 2,
            }],
            per_throttle_stats: vec![ThrottleKeyStat {
                key: "provider".to_string(),
                tokens: 9.5,
                rate_per_second: 10.0,
                burst: 20.0,
            }],
        };
        let frame = resp.encode(3);
        let decoded = GetStatsResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.depth, 100);
        assert_eq!(decoded.in_flight, 5);
        assert_eq!(decoded.per_key_stats.len(), 1);
        assert_eq!(decoded.per_key_stats[0].key, "tenant-a");
        assert_eq!(decoded.per_key_stats[0].current_deficit, -10);
        assert_eq!(decoded.per_throttle_stats.len(), 1);
        assert!((decoded.per_throttle_stats[0].tokens - 9.5).abs() < f64::EPSILON);
    }

    #[test]
    fn list_queues_round_trip() {
        let req = ListQueuesRequest;
        let frame = req.encode(4);
        assert_eq!(frame.opcode, Opcode::ListQueues as u8);
        ListQueuesRequest::decode(frame.payload).unwrap();

        let resp = ListQueuesResponse {
            error_code: ErrorCode::Ok,
            cluster_node_count: 3,
            queues: vec![QueueInfo {
                name: "q1".to_string(),
                depth: 10,
                in_flight: 2,
                active_consumers: 1,
                leader_node_id: 1,
            }],
        };
        let frame = resp.encode(4);
        let decoded = ListQueuesResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.cluster_node_count, 3);
        assert_eq!(decoded.queues.len(), 1);
        assert_eq!(decoded.queues[0].name, "q1");
    }

    #[test]
    fn set_get_config_round_trip() {
        let req = SetConfigRequest {
            key: "throttle.x".to_string(),
            value: "10.0,100.0".to_string(),
        };
        let frame = req.encode(5);
        let decoded = SetConfigRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key, "throttle.x");
        assert_eq!(decoded.value, "10.0,100.0");

        let resp = SetConfigResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(5);
        let decoded = SetConfigResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);

        let req = GetConfigRequest {
            key: "throttle.x".to_string(),
        };
        let frame = req.encode(6);
        let decoded = GetConfigRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key, "throttle.x");

        let resp = GetConfigResponse {
            error_code: ErrorCode::Ok,
            value: "10.0,100.0".to_string(),
        };
        let frame = resp.encode(6);
        let decoded = GetConfigResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.value, "10.0,100.0");
    }

    #[test]
    fn list_config_round_trip() {
        let req = ListConfigRequest {
            prefix: "throttle.".to_string(),
        };
        let frame = req.encode(7);
        let decoded = ListConfigRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.prefix, "throttle.");

        let resp = ListConfigResponse {
            error_code: ErrorCode::Ok,
            entries: vec![
                ConfigEntry {
                    key: "throttle.a".to_string(),
                    value: "1.0".to_string(),
                },
                ConfigEntry {
                    key: "throttle.b".to_string(),
                    value: "2.0".to_string(),
                },
            ],
        };
        let frame = resp.encode(7);
        let decoded = ListConfigResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.entries.len(), 2);
        assert_eq!(decoded.entries[0].key, "throttle.a");
    }

    #[test]
    fn redrive_round_trip() {
        let req = RedriveRequest {
            dlq_queue: "q.dlq".to_string(),
            count: 50,
        };
        let frame = req.encode(8);
        let decoded = RedriveRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.dlq_queue, "q.dlq");
        assert_eq!(decoded.count, 50);

        let resp = RedriveResponse {
            error_code: ErrorCode::Ok,
            redriven: 42,
        };
        let frame = resp.encode(8);
        let decoded = RedriveResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.redriven, 42);
    }
}
