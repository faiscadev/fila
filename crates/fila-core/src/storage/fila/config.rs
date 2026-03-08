use std::path::PathBuf;

/// Sync mode for WAL writes.
#[derive(Debug, Clone)]
pub enum SyncMode {
    /// Fsync after every write_batch() call. Safest but slower.
    EveryBatch,
    /// Fsync at the given interval in milliseconds. Faster but slight
    /// durability risk on crash.
    Interval(u64),
}

impl Default for SyncMode {
    fn default() -> Self {
        Self::EveryBatch
    }
}

/// Configuration for the Fila storage engine.
#[derive(Debug, Clone)]
pub struct FilaStorageConfig {
    /// Directory where WAL segment files are stored.
    pub data_dir: PathBuf,
    /// Maximum size of a single WAL segment in bytes. When exceeded, the
    /// current segment is sealed and a new one is created.
    pub segment_size_bytes: u64,
    /// How and when WAL writes are fsynced to disk.
    pub sync_mode: SyncMode,
}

impl FilaStorageConfig {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            segment_size_bytes: 64 * 1024 * 1024, // 64 MB
            sync_mode: SyncMode::default(),
        }
    }
}
