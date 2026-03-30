# Epic 19 Review Notes

## PR #156: Wire Format Design & Protocol Specification

### Gaps in Dev Process
- Error frame lacked structured metadata — only had a human-readable string. Lucas caught that programmable error handling needs a metadata map for machine-readable context (leader hints, retry-after, etc.)
- Protocol spec didn't address payloads/headers exceeding the 16 MiB frame limit. Lucas flagged that the protocol should support unlimited sizes. Continuation frames were added during review.
- Table alignment in frame header ASCII art was misaligned — minor formatting gap

### Incorrect Decisions During Development
- None — the spec was solid, review additions were genuine enhancements not corrections

### Deferred Work
- None

### Patterns for Future Stories
- Error metadata map pattern: when designing error responses, always include a structured metadata map alongside human-readable messages. Error codes alone aren't sufficient for programmatic handling.
- Continuation frame mechanism will need implementation in Stories 20.1 (server) and 20.3+ (SDKs). The spec is complete; implementation should follow it exactly.
- Cubic caught that HandshakeOk didn't communicate max_frame_size — when adding configurable limits, always ensure the negotiation path communicates the limit to the other side.

## PR #157: Batch-Native Scheduler Internals

### Gaps in Dev Process
- Used "batch" terminology throughout the code despite multi-item being the only path. Lucas flagged: if there's no single-message alternative, calling it "batch" is redundant. Code should read as if multi-item was the design from day 1.
- Included backward-compat legacy fields (`message: Option<Message>`, `queue_id: Option<String>`, etc.) with `#[serde(default)]` in ClusterRequest. Fila is pre-alpha with no users — legacy compat is unnecessary weight.

### Incorrect Decisions During Development
- Naming: `handle_enqueue_batch`, `handle_ack_batch`, `handle_nack_batch` — the `_batch` suffix implied an alternative exists. Renamed to `handle_enqueue`, `handle_ack`, `handle_nack`.
- Keeping backward compat fields "just in case" — adds code complexity with zero value pre-launch.

### Deferred Work
- None

### Patterns for Future Stories
- When a capability is the only path (not an alternative), don't name it after the pattern. Multi-item enqueue is just "enqueue", not "batch enqueue".
- Pre-alpha projects should not carry backward compat code. Add it when there are actual users to protect.
- Cubic P1s on proto_convert.rs were false positives — the single-item proto limitation is intentional and guarded by debug_assert. Cubic doesn't understand design-intent constraints.
