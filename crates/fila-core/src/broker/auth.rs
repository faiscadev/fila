use sha2::{Digest, Sha256};

/// An API key entry stored in the broker's state store.
///
/// The plaintext key is never stored — only its SHA-256 hash.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiKeyEntry {
    pub key_id: String,
    pub name: String,
    /// Hex-encoded SHA-256 hash of the plaintext key.
    pub hashed_key: String,
    /// Unix timestamp in milliseconds when the key was created.
    pub created_at_ms: u64,
    /// Unix timestamp in milliseconds when the key expires, or `None` if it never expires.
    pub expires_at_ms: Option<u64>,
}

/// Hash a plaintext API key token using SHA-256.
/// Returns the lowercase hex encoding of the hash.
pub fn hash_key(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Storage key prefix for API key entries.
pub const API_KEY_PREFIX: &str = "auth:key:";

/// Build the storage key for a given key ID.
pub fn storage_key(key_id: &str) -> String {
    format!("{API_KEY_PREFIX}{key_id}")
}

/// Current Unix timestamp in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
