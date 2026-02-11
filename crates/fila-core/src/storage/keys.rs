//! Key encoding for RocksDB column families.
//!
//! All numeric values use big-endian encoding for correct lexicographic ordering.
//! Composite keys use `:` (0x3A) as separator.
//! Variable-length strings are length-prefixed with a big-endian u16.

const SEPARATOR: u8 = b':';

/// Encode a u64 as 8 big-endian bytes.
fn encode_u64(val: u64) -> [u8; 8] {
    val.to_be_bytes()
}

/// Encode a variable-length string with a 2-byte big-endian length prefix.
fn encode_string(s: &str) -> Vec<u8> {
    let len = u16::try_from(s.len()).expect("key string exceeds 64 KiB");
    let mut buf = Vec::with_capacity(2 + s.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
    buf
}

/// Build a message key: `{queue_id}:{fairness_key}:{enqueue_ts_ns}:{msg_id}`
///
/// Key layout (binary):
/// - length-prefixed queue_id
/// - separator
/// - length-prefixed fairness_key
/// - separator
/// - 8-byte big-endian enqueue timestamp (nanos)
/// - separator
/// - 16-byte UUID (raw bytes, already lexicographically sortable for UUIDv7)
pub fn message_key(
    queue_id: &str,
    fairness_key: &str,
    enqueue_ts_ns: u64,
    msg_id: &uuid::Uuid,
) -> Vec<u8> {
    let mut key = Vec::with_capacity(64);
    key.extend_from_slice(&encode_string(queue_id));
    key.push(SEPARATOR);
    key.extend_from_slice(&encode_string(fairness_key));
    key.push(SEPARATOR);
    key.extend_from_slice(&encode_u64(enqueue_ts_ns));
    key.push(SEPARATOR);
    key.extend_from_slice(msg_id.as_bytes());
    key
}

/// Build a prefix for iterating all messages in a queue.
pub fn message_prefix(queue_id: &str) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(32);
    prefix.extend_from_slice(&encode_string(queue_id));
    prefix.push(SEPARATOR);
    prefix
}

/// Build a prefix for iterating messages with a specific fairness key in a queue.
#[cfg(test)]
pub fn message_prefix_with_key(queue_id: &str, fairness_key: &str) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(48);
    prefix.extend_from_slice(&encode_string(queue_id));
    prefix.push(SEPARATOR);
    prefix.extend_from_slice(&encode_string(fairness_key));
    prefix.push(SEPARATOR);
    prefix
}

/// Build a lease key: `{queue_id}:{msg_id}`
pub fn lease_key(queue_id: &str, msg_id: &uuid::Uuid) -> Vec<u8> {
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(&encode_string(queue_id));
    key.push(SEPARATOR);
    key.extend_from_slice(msg_id.as_bytes());
    key
}

/// Build a lease expiry key: `{expiry_ts_ns}:{queue_id}:{msg_id}`
///
/// Timestamp-first layout enables efficient "scan from earliest expiry" iteration.
pub fn lease_expiry_key(expiry_ts_ns: u64, queue_id: &str, msg_id: &uuid::Uuid) -> Vec<u8> {
    let mut key = Vec::with_capacity(40);
    key.extend_from_slice(&encode_u64(expiry_ts_ns));
    key.push(SEPARATOR);
    key.extend_from_slice(&encode_string(queue_id));
    key.push(SEPARATOR);
    key.extend_from_slice(msg_id.as_bytes());
    key
}

/// Encode a lease value: `{consumer_id}:{expiry_ts_ns}`
pub fn lease_value(consumer_id: &str, expiry_ts_ns: u64) -> Vec<u8> {
    let mut val = Vec::with_capacity(32);
    val.extend_from_slice(&encode_string(consumer_id));
    val.push(SEPARATOR);
    val.extend_from_slice(&encode_u64(expiry_ts_ns));
    val
}

/// Extract the expiry timestamp from a lease value.
/// The expiry is stored as the last 8 bytes (big-endian u64).
pub fn parse_expiry_from_lease_value(value: &[u8]) -> Option<u64> {
    if value.len() < 8 {
        return None;
    }
    let bytes: [u8; 8] = value[value.len() - 8..].try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn big_endian_u64_lexicographic_order() {
        let small = encode_u64(100);
        let large = encode_u64(200);
        assert!(small < large, "100 should sort before 200 in big-endian");

        let zero = encode_u64(0);
        let max = encode_u64(u64::MAX);
        assert!(zero < max, "0 should sort before MAX");

        let a = encode_u64(1_000_000_000);
        let b = encode_u64(1_000_000_001);
        assert!(a < b, "adjacent values should sort correctly");
    }

    #[test]
    fn message_keys_sort_by_queue_then_key_then_time() {
        let id1 = Uuid::now_v7();
        let id2 = Uuid::now_v7();

        // Same queue, same fairness_key, different timestamps
        let k1 = message_key("q1", "tenant_a", 1000, &id1);
        let k2 = message_key("q1", "tenant_a", 2000, &id2);
        assert!(k1 < k2, "earlier timestamp should sort first");

        // Same queue, different fairness_key
        let ka = message_key("q1", "a", 1000, &id1);
        let kb = message_key("q1", "b", 1000, &id1);
        assert!(ka < kb, "fairness key 'a' should sort before 'b'");

        // Different queues
        let kq1 = message_key("q1", "a", 1000, &id1);
        let kq2 = message_key("q2", "a", 1000, &id1);
        assert!(kq1 < kq2, "queue 'q1' should sort before 'q2'");
    }

    #[test]
    fn lease_expiry_keys_sort_by_timestamp() {
        let id = Uuid::now_v7();

        let early = lease_expiry_key(1000, "q1", &id);
        let late = lease_expiry_key(2000, "q1", &id);
        assert!(early < late, "earlier expiry should sort first");
    }

    #[test]
    fn message_prefix_is_prefix_of_message_key() {
        let id = Uuid::now_v7();
        let key = message_key("my-queue", "tenant_a", 12345, &id);
        let prefix = message_prefix("my-queue");
        assert!(
            key.starts_with(&prefix),
            "message key should start with queue prefix"
        );
    }

    #[test]
    fn message_prefix_with_key_is_prefix_of_message_key() {
        let id = Uuid::now_v7();
        let key = message_key("my-queue", "tenant_a", 12345, &id);
        let prefix = message_prefix_with_key("my-queue", "tenant_a");
        assert!(
            key.starts_with(&prefix),
            "message key should start with queue+fairness prefix"
        );
    }

    #[test]
    fn different_length_strings_dont_collide() {
        let id = Uuid::now_v7();
        // "a" and "ab" should not produce overlapping key prefixes
        let k1 = message_key("q", "a", 1000, &id);
        let k2 = message_key("q", "ab", 1000, &id);
        // They should be different (length-prefix prevents collision)
        assert_ne!(k1, k2);
    }
}
