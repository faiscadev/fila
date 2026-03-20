use sha2::{Digest, Sha256};

/// The identity of a validated API key caller.
///
/// Two structurally distinct cases: the bootstrap credential (accepted without
/// a storage lookup, always superadmin) and a stored key identified by its UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerKey {
    /// Accepted without a storage lookup when `bootstrap_apikey` is configured.
    /// Has full superadmin privileges.
    Bootstrap,
    /// A stored API key, identified by its UUID key_id.
    Key(String),
}

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

/// Returns `true` if every queue matched by `granted` is also matched by `admin`.
///
/// Used to enforce the delegation invariant: a caller can only grant a permission
/// whose pattern scope is a subset of their own admin scope.
///
/// | granted \ admin | `"foo"` | `"foo.*"` | `"*"` |
/// |-----------------|---------|-----------|-------|
/// | `"foo"`         | true    | false     | true  |
/// | `"foo.bar"`     | false   | true      | true  |
/// | `"foo.*"`       | false   | true      | true  |
/// | `"foo.bar.*"`   | false   | true      | true  |
/// | `"bar"`         | false   | false     | true  |
/// | `"*"`           | false   | false     | true  |
pub fn pattern_subsumed_by(granted: &str, admin: &str) -> bool {
    if admin == "*" {
        return true;
    }
    if granted == "*" {
        return false;
    }
    if granted == admin {
        return true;
    }
    if let Some(admin_prefix) = admin.strip_suffix(".*") {
        match granted.strip_suffix(".*") {
            Some(granted_prefix) => {
                // e.g. admin:"foo.*", granted:"foo.bar.*" → granted_prefix starts with "foo."
                granted_prefix.starts_with(&format!("{admin_prefix}."))
            }
            None => {
                // e.g. admin:"foo.*", granted:"foo.bar" → "foo.bar" starts with "foo."
                granted.starts_with(&format!("{admin_prefix}."))
            }
        }
    } else {
        // admin is an exact name and granted != admin → not subsumed
        false
    }
}

/// Returns `true` if the entry has any `admin` permission on any queue
/// (including `is_superadmin`).
pub fn has_any_admin_permission(entry: &ApiKeyEntry) -> bool {
    entry.is_superadmin || entry.permissions.iter().any(|(k, _)| k == "admin")
}

/// Returns `true` if the caller can grant the given `(kind, pattern)` permission.
///
/// Superadmins can grant anything. Other callers can only grant a permission
/// whose pattern is subsumed by one of their own `admin` grants.
pub fn caller_can_grant(caller: &ApiKeyEntry, _kind: &str, pattern: &str) -> bool {
    if caller.is_superadmin {
        return true;
    }
    caller
        .permissions
        .iter()
        .any(|(k, p)| k == "admin" && pattern_subsumed_by(pattern, p))
}

/// Returns `true` if the caller has `admin` permission covering `queue`.
pub fn caller_has_queue_admin(caller: &ApiKeyEntry, queue: &str) -> bool {
    if caller.is_superadmin {
        return true;
    }
    caller
        .permissions
        .iter()
        .any(|(k, p)| k == "admin" && pattern_matches(p, queue))
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

    // ── pattern_subsumed_by ──────────────────────────────────────────────────

    #[test]
    fn subsumed_by_star_always_true() {
        assert!(pattern_subsumed_by("*", "*"));
        assert!(pattern_subsumed_by("foo", "*"));
        assert!(pattern_subsumed_by("foo.*", "*"));
        assert!(pattern_subsumed_by("foo.bar", "*"));
    }

    #[test]
    fn wildcard_granted_only_subsumed_by_star() {
        assert!(!pattern_subsumed_by("*", "foo"));
        assert!(!pattern_subsumed_by("*", "foo.*"));
    }

    #[test]
    fn exact_subsumed_by_matching_prefix_wildcard() {
        assert!(pattern_subsumed_by("foo.bar", "foo.*"));
        assert!(pattern_subsumed_by("foo.bar.baz", "foo.*"));
        assert!(!pattern_subsumed_by("foo", "foo.*")); // "foo.*" does not cover "foo"
        assert!(!pattern_subsumed_by("bar", "foo.*"));
    }

    #[test]
    fn prefix_wildcard_subsumed_by_broader_prefix_wildcard() {
        assert!(pattern_subsumed_by("foo.bar.*", "foo.*"));
        assert!(pattern_subsumed_by("foo.*", "foo.*")); // same
        assert!(!pattern_subsumed_by("foo.*", "foo.bar.*")); // broader, not subsumed
        assert!(!pattern_subsumed_by("bar.*", "foo.*"));
    }

    #[test]
    fn exact_subsumed_by_same_exact() {
        assert!(pattern_subsumed_by("orders", "orders"));
        assert!(!pattern_subsumed_by("orders", "payments"));
        assert!(!pattern_subsumed_by("orders.us", "orders")); // exact admin doesn't cover sub-names
    }

    // ── caller_can_grant ─────────────────────────────────────────────────────

    fn make_entry(perms: Vec<(&str, &str)>, is_superadmin: bool) -> ApiKeyEntry {
        ApiKeyEntry {
            key_id: "k".into(),
            name: "test".into(),
            hashed_key: "h".into(),
            created_at_ms: 0,
            expires_at_ms: None,
            permissions: perms
                .into_iter()
                .map(|(k, p)| (k.into(), p.into()))
                .collect(),
            is_superadmin,
        }
    }

    #[test]
    fn superadmin_can_grant_anything() {
        let caller = make_entry(vec![], true);
        assert!(caller_can_grant(&caller, "produce", "*"));
        assert!(caller_can_grant(&caller, "admin", "*"));
        assert!(caller_can_grant(&caller, "consume", "orders"));
    }

    #[test]
    fn admin_star_can_grant_any_pattern() {
        let caller = make_entry(vec![("admin", "*")], false);
        assert!(caller_can_grant(&caller, "produce", "orders"));
        assert!(caller_can_grant(&caller, "consume", "payments.*"));
        assert!(caller_can_grant(&caller, "admin", "orders"));
        assert!(caller_can_grant(&caller, "produce", "*"));
    }

    #[test]
    fn admin_queue_can_grant_within_scope() {
        let caller = make_entry(vec![("admin", "orders.*")], false);
        assert!(caller_can_grant(&caller, "produce", "orders.us"));
        assert!(caller_can_grant(&caller, "consume", "orders.eu"));
        assert!(caller_can_grant(&caller, "admin", "orders.us"));
        assert!(!caller_can_grant(&caller, "produce", "payments"));
        assert!(!caller_can_grant(&caller, "produce", "*"));
        assert!(!caller_can_grant(&caller, "produce", "orders")); // "orders" not in "orders.*"
    }

    #[test]
    fn admin_exact_queue_can_grant_exact_only() {
        let caller = make_entry(vec![("admin", "orders")], false);
        assert!(caller_can_grant(&caller, "produce", "orders"));
        assert!(caller_can_grant(&caller, "consume", "orders"));
        assert!(caller_can_grant(&caller, "admin", "orders"));
        assert!(!caller_can_grant(&caller, "produce", "orders.us"));
        assert!(!caller_can_grant(&caller, "produce", "payments"));
    }

    // ── caller_has_queue_admin ───────────────────────────────────────────────

    #[test]
    fn superadmin_has_admin_on_any_queue() {
        let caller = make_entry(vec![], true);
        assert!(caller_has_queue_admin(&caller, "orders"));
        assert!(caller_has_queue_admin(&caller, "anything"));
    }

    #[test]
    fn admin_star_has_admin_on_any_queue() {
        let caller = make_entry(vec![("admin", "*")], false);
        assert!(caller_has_queue_admin(&caller, "orders"));
        assert!(caller_has_queue_admin(&caller, "payments"));
    }

    #[test]
    fn admin_prefix_has_admin_on_matching_queues() {
        let caller = make_entry(vec![("admin", "orders.*")], false);
        assert!(caller_has_queue_admin(&caller, "orders.us"));
        assert!(caller_has_queue_admin(&caller, "orders.eu"));
        assert!(!caller_has_queue_admin(&caller, "orders")); // not matched by "orders.*"
        assert!(!caller_has_queue_admin(&caller, "payments"));
    }

    #[test]
    fn produce_permission_does_not_grant_queue_admin() {
        let caller = make_entry(vec![("produce", "orders")], false);
        assert!(!caller_has_queue_admin(&caller, "orders"));
    }

    // ── pattern_star_matches_anything (existing) ─────────────────────────────

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
