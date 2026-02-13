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

/// Extract the queue_id from a message key.
///
/// Message key format starts with a length-prefixed queue_id string.
/// Returns `None` if the key is too short or malformed.
pub fn extract_queue_id(key: &[u8]) -> Option<String> {
    if key.len() < 3 {
        return None;
    }
    let queue_id_len = u16::from_be_bytes([key[0], key[1]]) as usize;
    if key.len() < 2 + queue_id_len {
        return None;
    }
    std::str::from_utf8(&key[2..2 + queue_id_len])
        .ok()
        .map(|s| s.to_string())
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
///
/// Validates the encoded structure (`{len_prefix}{consumer_id}:{expiry_ts_ns}`)
/// before parsing. Returns `None` if the value is corrupted.
pub fn parse_expiry_from_lease_value(value: &[u8]) -> Option<u64> {
    // Minimum: 2 (length prefix) + 0 (empty consumer_id) + 1 (separator) + 8 (expiry) = 11
    if value.len() < 11 {
        return None;
    }
    let consumer_len = u16::from_be_bytes([value[0], value[1]]) as usize;
    let expected_len = 2 + consumer_len + 1 + 8;
    if value.len() != expected_len {
        return None;
    }
    if value[2 + consumer_len] != SEPARATOR {
        return None;
    }
    let bytes: [u8; 8] = value[2 + consumer_len + 1..].try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

/// Parse a lease_expiry key to extract (queue_id, msg_id).
///
/// Key format: `{expiry_ts_ns(8B)}:{queue_id_len(2B)}{queue_id}:{msg_id(16B)}`
pub fn parse_lease_expiry_key(key: &[u8]) -> Option<(String, uuid::Uuid)> {
    // Minimum: 8 (ts) + 1 (:) + 2 (len) + 0 (queue_id) + 1 (:) + 16 (uuid) = 28
    if key.len() < 28 {
        return None;
    }
    // Skip 8 bytes timestamp + 1 byte separator
    let rest = &key[9..];
    let queue_id_len = u16::from_be_bytes(rest[0..2].try_into().ok()?) as usize;
    if rest.len() < 2 + queue_id_len + 1 + 16 {
        return None;
    }
    let queue_id = std::str::from_utf8(&rest[2..2 + queue_id_len])
        .ok()?
        .to_string();
    // Skip separator
    let uuid_start = 2 + queue_id_len + 1;
    let uuid_bytes: [u8; 16] = rest[uuid_start..uuid_start + 16].try_into().ok()?;
    Some((queue_id, uuid::Uuid::from_bytes(uuid_bytes)))
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

    #[test]
    fn extract_queue_id_roundtrip() {
        let id = Uuid::now_v7();
        let key = message_key("my-queue", "fk", 1000, &id);
        assert_eq!(extract_queue_id(&key), Some("my-queue".to_string()));
    }

    #[test]
    fn extract_queue_id_rejects_corrupt_input() {
        assert!(extract_queue_id(&[]).is_none());
        assert!(extract_queue_id(&[0]).is_none());
        // Length prefix says 5 bytes but only 1 byte available
        assert!(extract_queue_id(&[0, 5, b'a']).is_none());
    }

    #[test]
    fn parse_lease_expiry_key_roundtrip() {
        let id = Uuid::now_v7();
        let key = lease_expiry_key(42_000_000_000, "my-queue", &id);
        let (queue_id, msg_id) = parse_lease_expiry_key(&key).expect("should parse valid key");
        assert_eq!(queue_id, "my-queue");
        assert_eq!(msg_id, id);
    }

    #[test]
    fn parse_lease_expiry_key_rejects_corrupt_input() {
        // Too short
        assert!(parse_lease_expiry_key(&[0; 10]).is_none());

        // Exactly minimum length but with wrong length prefix (claims 255 bytes of queue_id)
        let mut bad = vec![0u8; 28];
        bad[9] = 0x00;
        bad[10] = 0xFF; // length prefix says 255, but only 17 bytes remain
        assert!(parse_lease_expiry_key(&bad).is_none());

        // Empty input
        assert!(parse_lease_expiry_key(&[]).is_none());
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating valid queue/key strings (1-100 ASCII alphanumeric chars).
        fn key_string() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9_-]{1,100}"
        }

        proptest! {
            #[test]
            fn message_key_starts_with_queue_prefix(
                queue_id in key_string(),
                fairness_key in key_string(),
                ts in any::<u64>(),
                uuid_bytes in any::<[u8; 16]>(),
            ) {
                let id = Uuid::from_bytes(uuid_bytes);
                let key = message_key(&queue_id, &fairness_key, ts, &id);
                let prefix = message_prefix(&queue_id);
                prop_assert!(
                    key.starts_with(&prefix),
                    "message key must start with queue prefix"
                );
            }

            #[test]
            fn extract_queue_id_from_message_key(
                queue_id in key_string(),
                fairness_key in key_string(),
                ts in any::<u64>(),
                uuid_bytes in any::<[u8; 16]>(),
            ) {
                let id = Uuid::from_bytes(uuid_bytes);
                let key = message_key(&queue_id, &fairness_key, ts, &id);
                let extracted = extract_queue_id(&key);
                prop_assert_eq!(extracted, Some(queue_id));
            }

            #[test]
            fn message_key_starts_with_fairness_prefix(
                queue_id in key_string(),
                fairness_key in key_string(),
                ts in any::<u64>(),
                uuid_bytes in any::<[u8; 16]>(),
            ) {
                let id = Uuid::from_bytes(uuid_bytes);
                let key = message_key(&queue_id, &fairness_key, ts, &id);
                let prefix = message_prefix_with_key(&queue_id, &fairness_key);
                prop_assert!(
                    key.starts_with(&prefix),
                    "message key must start with queue+fairness prefix"
                );
            }

            #[test]
            fn lease_expiry_key_roundtrips(
                ts in any::<u64>(),
                queue_id in key_string(),
                uuid_bytes in any::<[u8; 16]>(),
            ) {
                let id = Uuid::from_bytes(uuid_bytes);
                let key = lease_expiry_key(ts, &queue_id, &id);
                let (parsed_queue, parsed_id) = parse_lease_expiry_key(&key)
                    .expect("valid key should parse");
                prop_assert_eq!(parsed_queue, queue_id);
                prop_assert_eq!(parsed_id, id);
            }

            #[test]
            fn lease_value_expiry_roundtrips(
                consumer_id in key_string(),
                expiry in any::<u64>(),
            ) {
                let val = lease_value(&consumer_id, expiry);
                let parsed = parse_expiry_from_lease_value(&val)
                    .expect("valid lease value should parse");
                prop_assert_eq!(parsed, expiry);
            }

            #[test]
            fn message_keys_preserve_timestamp_ordering(
                queue_id in key_string(),
                fairness_key in key_string(),
                ts1 in any::<u64>(),
                ts2 in any::<u64>(),
                uuid_bytes1 in any::<[u8; 16]>(),
                uuid_bytes2 in any::<[u8; 16]>(),
            ) {
                let id1 = Uuid::from_bytes(uuid_bytes1);
                let id2 = Uuid::from_bytes(uuid_bytes2);
                let k1 = message_key(&queue_id, &fairness_key, ts1, &id1);
                let k2 = message_key(&queue_id, &fairness_key, ts2, &id2);

                // Same queue and fairness key: timestamp determines ordering
                // (with UUID as tiebreaker)
                if ts1 < ts2 {
                    prop_assert!(k1 < k2, "smaller ts should sort first");
                } else if ts1 > ts2 {
                    prop_assert!(k1 > k2, "larger ts should sort later");
                }
                // When ts1 == ts2, UUID bytes determine order — not testing that case
            }
        }
    }
}
