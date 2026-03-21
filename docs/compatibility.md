# SDK-Server Compatibility Matrix

## Versioning Policy

Fila uses [semantic versioning](https://semver.org/) (semver) for all releases:

- **MAJOR** — Breaking proto/API changes. Existing SDK versions may not work.
- **MINOR** — New features, backward compatible. Existing SDKs continue to work; new features require SDK update.
- **PATCH** — Bug fixes only. No behavior changes.

## Proto Backward Compatibility

The `fila.v1` proto package follows additive-only changes within a MAJOR version:

- New RPCs may be added
- New fields may be added to existing messages
- Existing fields are never removed, renamed, or retyped
- Field numbers are never reused

A MAJOR version bump (e.g., v1 → v2) would introduce a new proto package (`fila.v2`) and may remove deprecated RPCs or fields from the previous package.

## Deprecation Policy

- Features are deprecated with at least 1 MINOR version warning before removal
- Deprecated RPCs/fields are documented in release notes and proto comments
- Removal only occurs in a MAJOR version bump

## SDK-Server Compatibility Matrix

All components are currently at version **0.1.0** (pre-1.0 development).

| SDK | Version | Min Server | TLS | API Key Auth | Proto |
|-----|---------|-----------|-----|-------------|-------|
| **fila-sdk** (Rust) | 0.1.0 | 0.1.0 | Yes | Yes | v1 |
| **fila-go** | 0.1.0 | 0.1.0 | No | No | v1 |
| **fila-python** | 0.1.0 | 0.1.0 | No | No | v1 |
| **fila-js** (Node.js) | 0.1.0 | 0.1.0 | No | No | v1 |
| **fila-ruby** | 0.1.0 | 0.1.0 | No | No | v1 |
| **fila-java** | 0.1.0 | 0.1.0 | No | No | v1 |

> **Note:** External SDKs (Go, Python, JS, Ruby, Java) do not yet support TLS or API key authentication. When connecting to a server with auth enabled, use the Rust SDK or CLI. Auth support for external SDKs is planned.

## Feature Discovery

Clients can query the server's capabilities at runtime using the `GetServerInfo` RPC:

```
GetServerInfo() → { server_version, proto_version, features[] }
```

This RPC does not require authentication and can be called before establishing credentials.

**Current features:** `fair_scheduling`, `throttling`, `lua_scripting`, `tls`, `api_key_auth`, `acl`, `clustering`, `dlq`, `redrive`

## Server Version Detection

- **Server binary:** `fila-server --version`
- **CLI:** `fila --version`
- **Runtime:** `GetServerInfo` RPC returns the server version
- **SDK:** `FilaClient::sdk_version()` returns the SDK version (Rust SDK)
