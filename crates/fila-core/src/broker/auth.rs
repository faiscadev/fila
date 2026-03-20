use sha2::{Digest, Sha256};

/// The action being performed on a queue — used for ACL checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Produce,
    Consume,
    Admin,
}

impl Permission {
    fn as_str(self) -> &'static str {
        match self {
            Permission::Produce => "produce",
            Permission::Consume => "consume",
            Permission::Admin => "admin",
        }
    }
}

/// An API key entry stored in the broker's state store.
///
/// The plaintext key is never stored — only its SHA-256 hash.
/// Permissions are stored as `(kind, pattern)` pairs where `kind` is one of
/// `"produce"`, `"consume"`, or `"admin"`, and `pattern` is a queue name or
/// wildcard (`"*"` for any queue, `"foo.*"` for any queue starting with `"foo."`).
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
    /// ACL: list of `(kind, pattern)` permission grants.
    /// Empty by default — no access until explicitly granted.
    #[serde(default)]
    pub permissions: Vec<(String, String)>,
    /// Superadmin keys bypass all ACL checks.
    #[serde(default)]
    pub is_superadmin: bool,
}

/// Returns `true` if `pattern` matches `queue`.
///
/// Pattern rules:
/// - `"*"` matches any queue name.
/// - `"foo.*"` matches any queue whose name starts with `"foo."`.
/// - Any other string matches exactly.
pub fn pattern_matches(pattern: &str, queue: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return queue.starts_with(&format!("{prefix}."));
    }
    pattern == queue
}

/// Returns `true` if the key entry grants the requested permission on `queue`.
///
/// Superadmin keys always return `true`. Other keys require a matching
/// `(kind, pattern)` grant in `permissions`.
pub fn has_permission(entry: &ApiKeyEntry, perm: Permission, queue: &str) -> bool {
    if entry.is_superadmin {
        return true;
    }
    let kind = perm.as_str();
    entry
        .permissions
        .iter()
        .any(|(k, p)| k == kind && pattern_matches(p, queue))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_star_matches_anything() {
        assert!(pattern_matches("*", "orders"));
        assert!(pattern_matches("*", "orders.us"));
        assert!(pattern_matches("*", ""));
    }

    #[test]
    fn pattern_prefix_wildcard() {
        assert!(pattern_matches("orders.*", "orders.us"));
        assert!(pattern_matches("orders.*", "orders.eu"));
        assert!(!pattern_matches("orders.*", "orders"));
        assert!(!pattern_matches("orders.*", "payments.us"));
    }

    #[test]
    fn pattern_exact_match() {
        assert!(pattern_matches("orders", "orders"));
        assert!(!pattern_matches("orders", "orders.us"));
        assert!(!pattern_matches("orders", "payments"));
    }

    #[test]
    fn superadmin_bypasses_acl() {
        let entry = ApiKeyEntry {
            key_id: "k1".into(),
            name: "admin".into(),
            hashed_key: "hash".into(),
            created_at_ms: 0,
            expires_at_ms: None,
            permissions: vec![],
            is_superadmin: true,
        };
        assert!(has_permission(&entry, Permission::Produce, "any-queue"));
        assert!(has_permission(&entry, Permission::Admin, "*"));
    }

    #[test]
    fn no_permissions_denied() {
        let entry = ApiKeyEntry {
            key_id: "k1".into(),
            name: "limited".into(),
            hashed_key: "hash".into(),
            created_at_ms: 0,
            expires_at_ms: None,
            permissions: vec![],
            is_superadmin: false,
        };
        assert!(!has_permission(&entry, Permission::Produce, "orders"));
        assert!(!has_permission(&entry, Permission::Consume, "orders"));
        assert!(!has_permission(&entry, Permission::Admin, "*"));
    }

    #[test]
    fn produce_only_key() {
        let entry = ApiKeyEntry {
            key_id: "k1".into(),
            name: "producer".into(),
            hashed_key: "hash".into(),
            created_at_ms: 0,
            expires_at_ms: None,
            permissions: vec![("produce".into(), "orders.*".into())],
            is_superadmin: false,
        };
        assert!(has_permission(&entry, Permission::Produce, "orders.us"));
        assert!(!has_permission(&entry, Permission::Consume, "orders.us"));
        assert!(!has_permission(&entry, Permission::Produce, "payments"));
    }
}
