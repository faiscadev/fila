use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use tracing::warn;

use super::config::SyncMode;

/// Magic bytes at the start of every segment file.
const SEGMENT_MAGIC: &[u8; 4] = b"FILA";
/// Current segment format version.
const SEGMENT_VERSION: u16 = 1;
/// Total segment header size in bytes.
pub(super) const SEGMENT_HEADER_SIZE: u64 = 32;

/// WAL operation tag values. These identify the operation type in each WAL
/// entry and must remain stable across versions for replay compatibility.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpTag {
    PutMessage = 0x01,
    DeleteMessage = 0x02,
    PutLease = 0x03,
    DeleteLease = 0x04,
    PutLeaseExpiry = 0x05,
    DeleteLeaseExpiry = 0x06,
    PutQueue = 0x07,
    DeleteQueue = 0x08,
    PutState = 0x09,
    DeleteState = 0x0A,
}

impl OpTag {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::PutMessage),
            0x02 => Some(Self::DeleteMessage),
            0x03 => Some(Self::PutLease),
            0x04 => Some(Self::DeleteLease),
            0x05 => Some(Self::PutLeaseExpiry),
            0x06 => Some(Self::DeleteLeaseExpiry),
            0x07 => Some(Self::PutQueue),
            0x08 => Some(Self::DeleteQueue),
            0x09 => Some(Self::PutState),
            0x0A => Some(Self::DeleteState),
            _ => None,
        }
    }
}

/// A single deserialized WAL operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalOp {
    pub tag: OpTag,
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
}

/// A WAL entry consisting of one or more operations (a batch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalEntry {
    pub ops: Vec<WalOp>,
}

// ---------------------------------------------------------------------------
// Serialization helpers for WAL entries
// ---------------------------------------------------------------------------

/// Serialize a batch of operations into the WAL payload format.
/// Public within the crate for use by the compaction module.
///   op_count (u32 LE) + for each op: tag(1) + key_len(u32 LE) + key +
///   [value_len(u32 LE) + value] (only for Put variants)
pub(super) fn serialize_entry(entry: &WalEntry) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(entry.ops.len() as u32).to_le_bytes());
    for op in &entry.ops {
        buf.push(op.tag as u8);
        buf.extend_from_slice(&(op.key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&op.key);
        if let Some(ref value) = op.value {
            buf.extend_from_slice(&(value.len() as u32).to_le_bytes());
            buf.extend_from_slice(value);
        }
    }
    buf
}

/// Deserialize a WAL payload back into a WalEntry.
fn deserialize_entry(data: &[u8]) -> Option<WalEntry> {
    let mut pos = 0;

    if data.len() < 4 {
        return None;
    }
    let op_count = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
    pos += 4;

    let mut ops = Vec::with_capacity(op_count);
    for _ in 0..op_count {
        if pos >= data.len() {
            return None;
        }
        let tag = OpTag::from_u8(data[pos])?;
        pos += 1;

        if pos + 4 > data.len() {
            return None;
        }
        let key_len = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        pos += 4;

        if pos + key_len > data.len() {
            return None;
        }
        let key = data[pos..pos + key_len].to_vec();
        pos += key_len;

        let has_value = matches!(
            tag,
            OpTag::PutMessage | OpTag::PutLease | OpTag::PutQueue | OpTag::PutState
        );

        let value = if has_value {
            if pos + 4 > data.len() {
                return None;
            }
            let value_len = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
            pos += 4;
            if pos + value_len > data.len() {
                return None;
            }
            let v = data[pos..pos + value_len].to_vec();
            pos += value_len;
            Some(v)
        } else {
            None
        };

        ops.push(WalOp { tag, key, value });
    }

    Some(WalEntry { ops })
}

// ---------------------------------------------------------------------------
// Segment file naming
// ---------------------------------------------------------------------------

fn segment_filename(seq: u64) -> String {
    format!("segment-{seq:010}.wal")
}

fn parse_segment_seq(filename: &str) -> Option<u64> {
    let name = filename.strip_prefix("segment-")?.strip_suffix(".wal")?;
    name.parse().ok()
}

/// List segment files in a directory sorted by sequence number.
fn list_segments(dir: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut segments = Vec::new();
    if !dir.exists() {
        return Ok(segments);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let fname = entry.file_name();
        let fname_str = fname.to_string_lossy();
        if let Some(seq) = parse_segment_seq(&fname_str) {
            segments.push((seq, entry.path()));
        }
    }
    segments.sort_by_key(|(seq, _)| *seq);
    Ok(segments)
}

// ---------------------------------------------------------------------------
// Segment header
// ---------------------------------------------------------------------------

pub(super) fn write_segment_header(writer: &mut BufWriter<File>) -> io::Result<()> {
    let mut header = [0u8; SEGMENT_HEADER_SIZE as usize];
    header[0..4].copy_from_slice(SEGMENT_MAGIC);
    header[4..6].copy_from_slice(&SEGMENT_VERSION.to_le_bytes());
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    header[6..14].copy_from_slice(&now_ns.to_le_bytes());
    // bytes 14..32 are reserved (zeroes)
    writer.write_all(&header)?;
    Ok(())
}

pub(super) fn validate_segment_header(reader: &mut BufReader<File>) -> io::Result<bool> {
    let mut header = [0u8; SEGMENT_HEADER_SIZE as usize];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(e) => return Err(e),
    }
    if &header[0..4] != SEGMENT_MAGIC {
        return Ok(false);
    }
    let version = u16::from_le_bytes(header[4..6].try_into().unwrap());
    if version != SEGMENT_VERSION {
        return Ok(false);
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// WalWriter
// ---------------------------------------------------------------------------

/// Manages writing WAL entries to the current active segment file.
pub struct WalWriter {
    dir: PathBuf,
    sync_mode: SyncMode,
    segment_size_bytes: u64,
    current_seq: u64,
    writer: BufWriter<File>,
    current_size: u64,
}

impl WalWriter {
    /// Open or create a WAL writer in the given directory.
    pub fn open(dir: &Path, sync_mode: SyncMode, segment_size_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;

        let segments = list_segments(dir)?;
        let (current_seq, current_size, writer) = if let Some((seq, path)) = segments.last() {
            // Reopen the last segment for appending
            let file = OpenOptions::new().append(true).open(path)?;
            let size = file.metadata()?.len();
            (*seq, size, BufWriter::new(file))
        } else {
            // No segments exist — create the first one
            let seq = 1;
            let path = dir.join(segment_filename(seq));
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)?;
            let mut writer = BufWriter::new(file);
            write_segment_header(&mut writer)?;
            writer.flush()?;
            (seq, SEGMENT_HEADER_SIZE, writer)
        };

        Ok(Self {
            dir: dir.to_path_buf(),
            sync_mode,
            segment_size_bytes,
            current_seq,
            writer,
            current_size,
        })
    }

    /// Append a WAL entry (batch of operations) to the current segment.
    pub fn append(&mut self, entry: &WalEntry) -> io::Result<()> {
        let payload = serialize_entry(entry);
        let crc = crc32fast::hash(&payload);

        let len = payload.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&crc.to_le_bytes())?;
        self.writer.write_all(&payload)?;
        self.writer.flush()?;

        self.current_size += 4 + 4 + payload.len() as u64;

        if matches!(self.sync_mode, SyncMode::EveryBatch) {
            self.fsync()?;
        }

        // Rotate if segment exceeds configured size
        if self.current_size >= self.segment_size_bytes {
            self.rotate()?;
        }

        Ok(())
    }

    /// Force fsync of the current segment file.
    pub fn fsync(&mut self) -> io::Result<()> {
        self.writer.flush()?;
        self.writer.get_ref().sync_all()
    }

    /// Seal the current segment and create a new one.
    fn rotate(&mut self) -> io::Result<()> {
        // Flush and sync the current segment before sealing
        self.fsync()?;

        let next_seq = self.current_seq + 1;
        let path = self.dir.join(segment_filename(next_seq));
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        let mut new_writer = BufWriter::new(file);
        write_segment_header(&mut new_writer)?;
        new_writer.flush()?;

        self.writer = new_writer;
        self.current_seq = next_seq;
        self.current_size = SEGMENT_HEADER_SIZE;

        Ok(())
    }

    /// Returns the current (active) segment sequence number.
    pub fn current_seq(&self) -> u64 {
        self.current_seq
    }

    /// Returns paths to all sealed (non-active) segment files, sorted by
    /// sequence number.
    pub fn sealed_segment_paths(&self) -> io::Result<Vec<PathBuf>> {
        let segments = list_segments(&self.dir)?;
        Ok(segments
            .into_iter()
            .filter(|(seq, _)| *seq != self.current_seq)
            .map(|(_, path)| path)
            .collect())
    }

    /// Returns the data directory.
    pub fn data_dir(&self) -> &Path {
        &self.dir
    }
}

// ---------------------------------------------------------------------------
// WalReader
// ---------------------------------------------------------------------------

/// Replays WAL entries from all segments in sequence order.
pub struct WalReader {
    dir: PathBuf,
}

impl WalReader {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// Replay all valid WAL entries from all segments in order.
    pub fn replay(&self) -> io::Result<Vec<WalEntry>> {
        let segments = list_segments(&self.dir)?;
        let mut entries = Vec::new();

        for (seq, path) in &segments {
            let file = File::open(path)?;
            let mut reader = BufReader::new(file);

            if !validate_segment_header(&mut reader)? {
                warn!(segment = %seq, "invalid segment header, skipping segment");
                continue;
            }

            loop {
                match read_one_entry(&mut reader) {
                    Ok(Some(entry)) => entries.push(entry),
                    Ok(None) => break, // EOF
                    Err(EntryError::Truncated) => {
                        warn!(segment = %seq, "truncated WAL entry at end of segment, skipping");
                        break;
                    }
                    Err(EntryError::CrcMismatch) => {
                        warn!(segment = %seq, "CRC mismatch in WAL entry, treating as end of log");
                        break;
                    }
                    Err(EntryError::Io(e)) => return Err(e),
                }
            }
        }

        Ok(entries)
    }
}

pub(super) enum EntryError {
    Truncated,
    CrcMismatch,
    Io(io::Error),
}

/// Read a single WAL entry from the reader. Returns None at clean EOF.
pub(super) fn read_one_entry(reader: &mut BufReader<File>) -> Result<Option<WalEntry>, EntryError> {
    // Read length (4 bytes)
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(EntryError::Io(e)),
    }
    let len = u32::from_le_bytes(len_buf) as usize;

    // Read CRC (4 bytes)
    let mut crc_buf = [0u8; 4];
    reader
        .read_exact(&mut crc_buf)
        .map_err(|_| EntryError::Truncated)?;
    let expected_crc = u32::from_le_bytes(crc_buf);

    // Read payload
    if len > 256 * 1024 * 1024 {
        // Sanity check: reject entries > 256MB as corrupt
        return Err(EntryError::CrcMismatch);
    }
    let mut payload = vec![0u8; len];
    reader
        .read_exact(&mut payload)
        .map_err(|_| EntryError::Truncated)?;

    // Verify CRC
    let actual_crc = crc32fast::hash(&payload);
    if actual_crc != expected_crc {
        return Err(EntryError::CrcMismatch);
    }

    // Deserialize
    match deserialize_entry(&payload) {
        Some(entry) => Ok(Some(entry)),
        None => Err(EntryError::CrcMismatch), // treat deserialization failure as corruption
    }
}

/// Truncate the last N bytes from the last segment file in the directory.
/// Used by tests to simulate a crash mid-write.
#[cfg(test)]
pub fn truncate_last_segment(dir: &Path, bytes_to_remove: u64) -> io::Result<()> {
    let segments = list_segments(dir)?;
    if let Some((_, path)) = segments.last() {
        let file = OpenOptions::new().write(true).open(path)?;
        let current_len = file.metadata()?.len();
        let new_len = current_len.saturating_sub(bytes_to_remove);
        file.set_len(new_len)?;
        file.sync_all()?;
    }
    Ok(())
}

/// Corrupt a byte at the given offset in the last segment file.
/// Used by tests to verify CRC checking.
#[cfg(test)]
pub fn corrupt_byte_in_last_segment(dir: &Path, offset_from_end: u64) -> io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let segments = list_segments(dir)?;
    if let Some((_, path)) = segments.last() {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len();
        let pos = len.saturating_sub(offset_from_end);
        file.seek(SeekFrom::Start(pos))?;
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte)?;
        byte[0] ^= 0xFF; // flip all bits
        file.seek(SeekFrom::Start(pos))?;
        file.write_all(&byte)?;
        file.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(ops: Vec<WalOp>) -> WalEntry {
        WalEntry { ops }
    }

    fn put_msg_op(key: &[u8], value: &[u8]) -> WalOp {
        WalOp {
            tag: OpTag::PutMessage,
            key: key.to_vec(),
            value: Some(value.to_vec()),
        }
    }

    fn del_msg_op(key: &[u8]) -> WalOp {
        WalOp {
            tag: OpTag::DeleteMessage,
            key: key.to_vec(),
            value: None,
        }
    }

    #[test]
    fn wal_append_and_replay_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let entries: Vec<WalEntry> = (0..10)
            .map(|i| {
                make_entry(vec![put_msg_op(
                    format!("key-{i}").as_bytes(),
                    format!("value-{i}").as_bytes(),
                )])
            })
            .collect();

        {
            let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 64 * 1024 * 1024).unwrap();
            for entry in &entries {
                writer.append(entry).unwrap();
            }
        }

        let reader = WalReader::new(path);
        let replayed = reader.replay().unwrap();
        assert_eq!(replayed.len(), 10);
        assert_eq!(replayed, entries);
    }

    #[test]
    fn segment_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // Use a very small segment size to force rotation
        let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 100).unwrap();
        assert_eq!(writer.current_seq(), 1);

        // Write entries until rotation happens
        for i in 0..20 {
            writer
                .append(&make_entry(vec![put_msg_op(
                    format!("key-{i}").as_bytes(),
                    b"some-value-that-takes-space",
                )]))
                .unwrap();
        }

        // Should have rotated to multiple segments
        assert!(writer.current_seq() > 1, "should have rotated segments");

        // Verify multiple segment files exist
        let segments = list_segments(path).unwrap();
        assert!(segments.len() > 1, "multiple segment files should exist");

        // Drop writer, replay all
        drop(writer);
        let reader = WalReader::new(path);
        let replayed = reader.replay().unwrap();
        assert_eq!(replayed.len(), 20);
    }

    #[test]
    fn crash_recovery_truncated_last_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // Write 5 entries
        {
            let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 64 * 1024 * 1024).unwrap();
            for i in 0..5 {
                writer
                    .append(&make_entry(vec![put_msg_op(
                        format!("key-{i}").as_bytes(),
                        format!("value-{i}").as_bytes(),
                    )]))
                    .unwrap();
            }
        }

        // Truncate the last few bytes (simulating crash mid-write)
        truncate_last_segment(path, 5).unwrap();

        // Replay should recover 4 entries (the 5th is truncated)
        let reader = WalReader::new(path);
        let replayed = reader.replay().unwrap();
        assert_eq!(replayed.len(), 4);
    }

    #[test]
    fn crc_integrity_corrupt_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        {
            let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 64 * 1024 * 1024).unwrap();
            for i in 0..3 {
                writer
                    .append(&make_entry(vec![put_msg_op(
                        format!("key-{i}").as_bytes(),
                        format!("value-{i}").as_bytes(),
                    )]))
                    .unwrap();
            }
        }

        // Corrupt a byte in the last entry's payload
        corrupt_byte_in_last_segment(path, 3).unwrap();

        // Replay should skip the corrupted entry
        let reader = WalReader::new(path);
        let replayed = reader.replay().unwrap();
        assert_eq!(replayed.len(), 2, "corrupted entry should be skipped");
    }

    #[test]
    fn batch_atomicity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let batch = make_entry(vec![
            put_msg_op(b"key-1", b"val-1"),
            del_msg_op(b"key-2"),
            WalOp {
                tag: OpTag::PutLease,
                key: b"lease-1".to_vec(),
                value: Some(b"consumer-1:12345".to_vec()),
            },
        ]);

        {
            let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 64 * 1024 * 1024).unwrap();
            writer.append(&batch).unwrap();
        }

        let reader = WalReader::new(path);
        let replayed = reader.replay().unwrap();
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0], batch);
        assert_eq!(replayed[0].ops.len(), 3);
    }

    #[test]
    fn segment_ordering_across_multiple_segments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // Very small segments to force multiple rotations
        let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 80).unwrap();

        let expected_keys: Vec<String> = (0..30).map(|i| format!("key-{i:04}")).collect();
        for key in &expected_keys {
            writer
                .append(&make_entry(vec![put_msg_op(key.as_bytes(), b"v")]))
                .unwrap();
        }
        drop(writer);

        let segments = list_segments(path).unwrap();
        assert!(segments.len() >= 3, "need multiple segments for this test");

        let reader = WalReader::new(path);
        let replayed = reader.replay().unwrap();
        assert_eq!(replayed.len(), expected_keys.len());

        // Verify order is preserved across segments
        for (i, entry) in replayed.iter().enumerate() {
            assert_eq!(entry.ops[0].key, expected_keys[i].as_bytes());
        }
    }

    #[test]
    fn serialization_roundtrip_all_op_types() {
        let entry = make_entry(vec![
            WalOp {
                tag: OpTag::PutMessage,
                key: b"k1".to_vec(),
                value: Some(b"v1".to_vec()),
            },
            WalOp {
                tag: OpTag::DeleteMessage,
                key: b"k2".to_vec(),
                value: None,
            },
            WalOp {
                tag: OpTag::PutLease,
                key: b"k3".to_vec(),
                value: Some(b"v3".to_vec()),
            },
            WalOp {
                tag: OpTag::DeleteLease,
                key: b"k4".to_vec(),
                value: None,
            },
            WalOp {
                tag: OpTag::PutLeaseExpiry,
                key: b"k5".to_vec(),
                value: None,
            },
            WalOp {
                tag: OpTag::DeleteLeaseExpiry,
                key: b"k6".to_vec(),
                value: None,
            },
            WalOp {
                tag: OpTag::PutQueue,
                key: b"k7".to_vec(),
                value: Some(b"v7".to_vec()),
            },
            WalOp {
                tag: OpTag::DeleteQueue,
                key: b"k8".to_vec(),
                value: None,
            },
            WalOp {
                tag: OpTag::PutState,
                key: b"k9".to_vec(),
                value: Some(b"v9".to_vec()),
            },
            WalOp {
                tag: OpTag::DeleteState,
                key: b"k10".to_vec(),
                value: None,
            },
        ]);

        let serialized = serialize_entry(&entry);
        let deserialized = deserialize_entry(&serialized).unwrap();
        assert_eq!(deserialized, entry);
    }

    #[test]
    fn empty_directory_replay_returns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reader = WalReader::new(dir.path());
        let replayed = reader.replay().unwrap();
        assert!(replayed.is_empty());
    }

    #[test]
    fn writer_reopens_existing_segment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // Write 3 entries, close
        {
            let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 64 * 1024 * 1024).unwrap();
            for i in 0..3 {
                writer
                    .append(&make_entry(vec![put_msg_op(
                        format!("key-{i}").as_bytes(),
                        b"val",
                    )]))
                    .unwrap();
            }
        }

        // Reopen and write 2 more
        {
            let mut writer = WalWriter::open(path, SyncMode::EveryBatch, 64 * 1024 * 1024).unwrap();
            for i in 3..5 {
                writer
                    .append(&make_entry(vec![put_msg_op(
                        format!("key-{i}").as_bytes(),
                        b"val",
                    )]))
                    .unwrap();
            }
        }

        // All 5 should be recoverable
        let reader = WalReader::new(path);
        let replayed = reader.replay().unwrap();
        assert_eq!(replayed.len(), 5);
    }
}
