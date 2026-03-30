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
