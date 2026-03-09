use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use opentelemetry::metrics::{Counter, Histogram};
use tracing::{info, warn};

use crate::message::Message;
use crate::queue::QueueConfig;

use super::config::FilaStorageConfig;
use super::wal::{
    read_one_entry, serialize_entry, validate_segment_header, write_segment_header, EntryError,
    OpTag, WalEntry, WalOp, WalWriter,
};

/// Metrics for the compaction subsystem.
struct CompactionMetrics {
    segments_compacted: Counter<u64>,
    bytes_reclaimed: Counter<u64>,
    duration_seconds: Histogram<f64>,
}

impl CompactionMetrics {
    fn new() -> Self {
        let meter = opentelemetry::global::meter("fila");
        Self {
            segments_compacted: meter
                .u64_counter("fila.storage.compaction.segments_compacted")
                .with_description("Total WAL segments compacted")
                .build(),
            bytes_reclaimed: meter
                .u64_counter("fila.storage.compaction.bytes_reclaimed")
                .with_description("Total bytes reclaimed by compaction")
                .build(),
            duration_seconds: meter
                .f64_histogram("fila.storage.compaction.duration_seconds")
                .with_description("Duration of compaction passes")
                .build(),
        }
    }
}

/// In-memory snapshot of what's live, used for liveness checks during compaction.
/// We only need to know which keys exist — not the values.
struct LivenessSnapshot {
    message_keys: std::collections::HashSet<Vec<u8>>,
    lease_keys: std::collections::HashSet<Vec<u8>>,
    lease_expiry_keys: std::collections::HashSet<Vec<u8>>,
    queue_keys: std::collections::HashSet<String>,
    state_keys: std::collections::HashSet<String>,
}

impl LivenessSnapshot {
    /// Build a liveness snapshot from the current indexes.
    fn from_indexes(
        messages: &BTreeMap<Vec<u8>, Message>,
        leases: &HashMap<Vec<u8>, Vec<u8>>,
        lease_expiries: &BTreeMap<Vec<u8>, ()>,
        queues: &HashMap<String, QueueConfig>,
        state: &BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            message_keys: messages.keys().cloned().collect(),
            lease_keys: leases.keys().cloned().collect(),
            lease_expiry_keys: lease_expiries.keys().cloned().collect(),
            queue_keys: queues.keys().cloned().collect(),
            state_keys: state.keys().cloned().collect(),
        }
    }

    /// Check if a WAL operation references a live key.
    fn is_live(&self, op: &WalOp) -> bool {
        match op.tag {
            OpTag::PutMessage => self.message_keys.contains(&op.key),
            OpTag::PutLease => self.lease_keys.contains(&op.key),
            OpTag::PutLeaseExpiry => self.lease_expiry_keys.contains(&op.key),
            OpTag::PutQueue => {
                let queue_id = String::from_utf8(op.key.clone()).unwrap_or_default();
                self.queue_keys.contains(&queue_id)
            }
            OpTag::PutState => {
                let key = String::from_utf8(op.key.clone()).unwrap_or_default();
                self.state_keys.contains(&key)
            }
            // Delete operations are always dead after compaction
            OpTag::DeleteMessage
            | OpTag::DeleteLease
            | OpTag::DeleteLeaseExpiry
            | OpTag::DeleteQueue
            | OpTag::DeleteState => false,
        }
    }
}

/// Handle for the background compaction thread.
pub(super) struct CompactionHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CompactionHandle {
    /// Signal the compaction thread to stop and wait for it to finish.
    pub(super) fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CompactionHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawn a background compaction thread.
pub(super) fn spawn_compaction_thread(
    config: &FilaStorageConfig,
    writer: Arc<Mutex<WalWriter>>,
    indexes: Arc<RwLock<super::Indexes>>,
) -> CompactionHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let interval = Duration::from_secs(config.compaction_interval_secs);
    let io_rate = config.compaction_io_rate_bytes_per_sec;
    let message_ttl_ms = config.message_ttl_ms;

    let thread = thread::spawn(move || {
        let metrics = CompactionMetrics::new();

        loop {
            // Sleep in small increments so we can stop quickly
            let sleep_until = Instant::now() + interval;
            while Instant::now() < sleep_until {
                if stop_clone.load(Ordering::Acquire) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }

            if stop_clone.load(Ordering::Acquire) {
                return;
            }

            // Run one compaction pass
            let start = Instant::now();
            match run_compaction_pass(&writer, &indexes, io_rate, message_ttl_ms) {
                Ok(stats) => {
                    let duration = start.elapsed().as_secs_f64();
                    if stats.segments_compacted > 0 {
                        info!(
                            segments = stats.segments_compacted,
                            bytes_reclaimed = stats.bytes_reclaimed,
                            duration_secs = format!("{duration:.3}"),
                            "compaction pass complete"
                        );
                    }
                    metrics
                        .segments_compacted
                        .add(stats.segments_compacted, &[]);
                    metrics.bytes_reclaimed.add(stats.bytes_reclaimed, &[]);
                    metrics.duration_seconds.record(duration, &[]);
                }
                Err(e) => {
                    warn!(error = %e, "compaction pass failed");
                }
            }
        }
    });

    CompactionHandle {
        stop,
        thread: Some(thread),
    }
}

struct CompactionStats {
    segments_compacted: u64,
    bytes_reclaimed: u64,
}

/// Run one compaction pass over all sealed segments.
fn run_compaction_pass(
    writer: &Mutex<WalWriter>,
    indexes: &RwLock<super::Indexes>,
    io_rate: u64,
    message_ttl_ms: Option<u64>,
) -> io::Result<CompactionStats> {
    // Get sealed segment paths while holding the writer lock briefly
    let sealed_paths = {
        let w = writer
            .lock()
            .map_err(|e| io::Error::other(format!("writer lock poisoned: {e}")))?;
        w.sealed_segment_paths()?
    };

    if sealed_paths.is_empty() {
        return Ok(CompactionStats {
            segments_compacted: 0,
            bytes_reclaimed: 0,
        });
    }

    let mut total_stats = CompactionStats {
        segments_compacted: 0,
        bytes_reclaimed: 0,
    };

    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let ttl_ns = message_ttl_ms.map(|ms| ms * 1_000_000);

    for segment_path in &sealed_paths {
        // Take a liveness snapshot from the indexes (read lock)
        let snapshot = {
            let idx = indexes
                .read()
                .map_err(|e| io::Error::other(format!("index lock poisoned: {e}")))?;
            LivenessSnapshot::from_indexes(
                &idx.messages,
                &idx.leases,
                &idx.lease_expiries,
                &idx.queues,
                &idx.state,
            )
        };

        match compact_segment(segment_path, &snapshot, io_rate, ttl_ns, now_ns, indexes)? {
            Some(bytes_reclaimed) => {
                total_stats.segments_compacted += 1;
                total_stats.bytes_reclaimed += bytes_reclaimed;
            }
            None => {
                // Segment had no dead entries, skip
            }
        }
    }

    Ok(total_stats)
}

/// Compact a single sealed segment file. Returns the bytes reclaimed, or None
/// if the segment had no dead entries and was left unchanged.
fn compact_segment(
    segment_path: &Path,
    snapshot: &LivenessSnapshot,
    io_rate: u64,
    ttl_ns: Option<u64>,
    now_ns: u64,
    indexes: &RwLock<super::Indexes>,
) -> io::Result<Option<u64>> {
    let original_size = fs::metadata(segment_path)?.len();

    // Read all entries from the segment
    let file = File::open(segment_path)?;
    let mut reader = BufReader::new(file);

    if !validate_segment_header(&mut reader)? {
        warn!(path = %segment_path.display(), "invalid segment header during compaction, skipping");
        return Ok(None);
    }

    let mut live_entries: Vec<WalEntry> = Vec::new();
    let mut has_dead_entries = false;
    let mut ttl_expired_keys: Vec<(OpTag, Vec<u8>)> = Vec::new();

    loop {
        match read_one_entry(&mut reader) {
            Ok(Some(entry)) => {
                let mut live_ops = Vec::new();
                for op in &entry.ops {
                    // Check TTL expiry for PutMessage entries
                    if op.tag == OpTag::PutMessage {
                        if let Some(ttl) = ttl_ns {
                            if let Some(ref value) = op.value {
                                if let Ok(msg) = serde_json::from_slice::<Message>(value) {
                                    if now_ns.saturating_sub(msg.enqueued_at) > ttl {
                                        // Message is TTL-expired
                                        has_dead_entries = true;
                                        ttl_expired_keys.push((OpTag::PutMessage, op.key.clone()));
                                        continue;
                                    }
                                }
                            }
                        }
                    }

                    if snapshot.is_live(op) {
                        live_ops.push(op.clone());
                    } else {
                        has_dead_entries = true;
                    }
                }
                if !live_ops.is_empty() {
                    live_entries.push(WalEntry { ops: live_ops });
                }
            }
            Ok(None) => break,
            Err(EntryError::Truncated) | Err(EntryError::CrcMismatch) => {
                has_dead_entries = true;
                break;
            }
            Err(EntryError::Io(e)) => return Err(e),
        }
    }

    if !has_dead_entries {
        return Ok(None);
    }

    // Remove TTL-expired messages from the in-memory index
    if !ttl_expired_keys.is_empty() {
        let mut idx = indexes
            .write()
            .map_err(|e| io::Error::other(format!("index lock poisoned: {e}")))?;
        for (_, key) in &ttl_expired_keys {
            idx.messages.remove(key);
        }
    }

    if live_entries.is_empty() {
        // All entries were dead — delete the segment entirely
        fs::remove_file(segment_path)?;
        return Ok(Some(original_size));
    }

    // Write compacted segment to a temp file
    let tmp_path = segment_path.with_extension("wal.tmp");
    {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)?;
        let mut writer = BufWriter::new(file);
        write_segment_header(&mut writer)?;

        let mut bytes_since_last_sleep: u64 = 0;
        for entry in &live_entries {
            let payload = serialize_entry(entry);
            let crc = crc32fast::hash(&payload);
            let len = payload.len() as u32;

            writer.write_all(&len.to_le_bytes())?;
            writer.write_all(&crc.to_le_bytes())?;
            writer.write_all(&payload)?;

            let entry_bytes = 4 + 4 + payload.len() as u64;
            bytes_since_last_sleep += entry_bytes;

            // Rate-limit: sleep proportionally to bytes written since last sleep
            if io_rate > 0 && bytes_since_last_sleep > 0 {
                let sleep_us =
                    (bytes_since_last_sleep as f64 / io_rate as f64 * 1_000_000.0) as u64;
                if sleep_us > 0 {
                    thread::sleep(Duration::from_micros(sleep_us.min(10_000)));
                    bytes_since_last_sleep = 0;
                }
            }
        }

        writer.flush()?;
        writer.get_ref().sync_all()?;
    }

    // Atomic rename
    fs::rename(&tmp_path, segment_path)?;

    let new_size = fs::metadata(segment_path)?.len();
    let reclaimed = original_size.saturating_sub(new_size);

    Ok(Some(reclaimed))
}

/// Run a single compaction pass (for testing). This is the same logic as the
/// background thread but callable directly.
#[cfg(test)]
pub(super) fn run_compaction_pass_for_test(
    writer: &Mutex<WalWriter>,
    indexes: &RwLock<super::Indexes>,
    io_rate: u64,
    message_ttl_ms: Option<u64>,
) -> io::Result<(u64, u64)> {
    let stats = run_compaction_pass(writer, indexes, io_rate, message_ttl_ms)?;
    Ok((stats.segments_compacted, stats.bytes_reclaimed))
}
