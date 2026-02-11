# Project Settings

## Git
- Use conventional commit prefixes: feat, fix, chore, test, docs, refactor, perf, ci
- All commit messages in lowercase (including the prefix and description)

## Error Handling

### Pattern: Explicit Error Mapping

When converting or wrapping errors, always follow this pattern:

```rust
// GOOD: match on all variants explicitly, map to our own types
.map_err(|err| match err {
    upstream::Error::VariantA(inner) => OurError::Relevant(inner),
    upstream::Error::VariantB(inner) => OurError::Relevant(inner),
    upstream::Error::VariantC(msg) => OurError::Other(msg),
})
```

```rust
// BAD: catch-all string conversion — loses type info, no compiler protection
.map_err(|e| OurError::Generic(e.to_string()))
.map_err(|e| OurError::Generic(format!("something failed: {e}")))
```

**Rules:**

1. **Always match on variants explicitly.** When the upstream error adds a new variant, the compiler must force us to handle it. Never use catch-all `_` or `.to_string()` on the whole error.

2. **It's OK to group multiple upstream variants into one of ours.** We are also consumers of our own API — only split variants when callers need to distinguish them.

3. **Always preserve the original error.** Use `#[source]`, `#[from]`, or include the original error/message so logs and stack traces show what actually happened. Never silently discard error context.

4. **Use specific error variants, not string-formatted catch-alls.** If a failure mode is distinct (e.g., "column family not found" vs "rocksdb I/O error"), give it its own variant rather than stuffing a format string into a generic one.

5. **For `Option` → `Result` conversions** (`.ok_or()`), use a dedicated error variant that describes the specific absence, not a generic error with a format string.

### Per-Command Error Types

Each command/operation returns only the errors it can actually produce. Never use a "god enum" where `put_queue` could return `MessageNotFound`. See `crates/fila-core/src/error.rs` for the pattern.
