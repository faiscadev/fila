# Story 1.2: CI/CD Pipeline

Status: done

## Story

As a developer,
I want automated CI for every PR from the start of the project,
So that code quality is enforced continuously and regressions are caught immediately.

## Acceptance Criteria

1. `.github/workflows/ci.yml` runs on every pull request and push to main
2. The workflow runs `cargo fmt --check` to verify code formatting
3. The workflow runs `cargo clippy` with warnings as errors
4. The workflow runs `cargo nextest run` to execute all tests
5. The workflow runs `cargo bench` (informational, no regression gate)
6. The CI pipeline passes on the initial workspace with all four crates
7. A release workflow runs all PR checks on version tag push
8. Release binaries are built for linux-amd64, linux-arm64, darwin-amd64, darwin-arm64
9. A GitHub Release is created with the binaries
10. Docker image is tagged with the version and pushed to ghcr.io

## Tasks / Subtasks

- [x] Task 1: Create CI workflow (AC: #1, #2, #3, #4, #5, #6)
  - [x] 1.1 Create `.github/workflows/ci.yml` triggered on PR and push to main
  - [x] 1.2 Add `fmt` job: `cargo fmt --check`
  - [x] 1.3 Add `clippy` job: `cargo clippy -- -D warnings`
  - [x] 1.4 Add `test` job: install `cargo-nextest`, run `cargo nextest run`
  - [x] 1.5 Add `bench` job: `cargo bench` (informational, `continue-on-error: true`)
  - [x] 1.6 Use `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `actions/checkout@v4`
  - [x] 1.7 Install `protoc` in CI (required by `tonic-prost-build`)
- [x] Task 2: Create release workflow (AC: #7, #8, #9, #10)
  - [x] 2.1 Create `.github/workflows/release.yml` triggered on `v*` tag push
  - [x] 2.2 Add check job that runs all CI checks (fmt, clippy, test, bench)
  - [x] 2.3 Add build matrix for 4 targets: linux-amd64, linux-arm64, darwin-amd64, darwin-arm64
  - [x] 2.4 Use `cross` for linux-arm64 cross-compilation
  - [x] 2.5 Build both `fila-server` and `fila` (CLI) binaries per target
  - [x] 2.6 Create GitHub Release with binaries and checksums via `softprops/action-gh-release`
  - [x] 2.7 Add Docker build+push job: multi-stage Dockerfile, push to `ghcr.io/faiscadev/fila`
- [x] Task 3: Create Dockerfile (AC: #10)
  - [x] 3.1 Create multi-stage `Dockerfile`: `rust:latest` builder → `debian:bookworm-slim` runtime
  - [x] 3.2 Include both `fila-server` and `fila` binaries in runtime image
  - [x] 3.3 Expose port 5555, set `ENTRYPOINT ["fila-server"]`
- [x] Task 4: Verify CI passes (AC: #6)
  - [x] 4.1 Ensure CI workflow passes on current workspace

## Dev Notes

### GitHub Actions Versions (February 2026)

| Action | Version | Notes |
|--------|---------|-------|
| `actions/checkout` | `v4` | Stable, widely used |
| `dtolnay/rust-toolchain` | `@stable` | Use tag not version |
| `Swatinem/rust-cache` | `v2` | Caches ~/.cargo and ./target |
| `taiki-e/install-action` | `v2` | For cargo-nextest and cross |
| `docker/build-push-action` | `v6` | Multi-platform Docker builds |
| `docker/setup-buildx-action` | `v3` | Required for multi-platform |
| `docker/login-action` | `v3` | GHCR authentication |
| `docker/metadata-action` | `v5` | Tag extraction |
| `softprops/action-gh-release` | `v2` | Create GitHub Releases |

### CI Protoc Requirement

`tonic-prost-build` requires `protoc` to be installed. In CI, install via:
```yaml
- name: Install protoc
  uses: arduino/setup-protoc@v3
  with:
    repo-token: ${{ secrets.GITHUB_TOKEN }}
```

### Cross-Compilation Targets

| Target | Runner | Tool | Binary Names |
|--------|--------|------|-------------|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `cargo` | `fila-server`, `fila` |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` | `cross` | `fila-server`, `fila` |
| `x86_64-apple-darwin` | `macos-13` | `cargo` | `fila-server`, `fila` |
| `aarch64-apple-darwin` | `macos-14` | `cargo` | `fila-server`, `fila` |

### Dockerfile Pattern

```dockerfile
FROM rust:latest AS builder
WORKDIR /build
COPY . .
RUN apt-get update && apt-get install -y protobuf-compiler
RUN cargo build --release --bin fila-server --bin fila

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/fila-server /usr/local/bin/
COPY --from=builder /build/target/release/fila /usr/local/bin/
EXPOSE 5555
ENTRYPOINT ["fila-server"]
```

### Anti-Patterns

- Do NOT use `actions/cache` directly — `Swatinem/rust-cache` handles Rust caching properly
- Do NOT use nightly Rust in CI — stable only
- Do NOT gate on benchmark regressions (informational only at this stage)
- Do NOT skip clippy warnings — use `-D warnings` to fail on any warning

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Infrastructure & Deployment]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.2: CI/CD Pipeline]
- [Source: _bmad-output/planning-artifacts/architecture.md#Testing Strategy]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

No issues encountered.

### Completion Notes List

- CI workflow: 4 parallel jobs (fmt, clippy, test, bench) with protoc installed via arduino/setup-protoc
- Release workflow: check → build (4-target matrix) → release + docker (parallel)
- Cross-compilation uses `cross` tool for linux-arm64; native cargo for all other targets
- Dockerfile: multi-stage build, bookworm-slim runtime, ca-certificates for TLS
- Release packages include LICENSE and SHA256 checksums

### File List

- .github/workflows/ci.yml (new)
- .github/workflows/release.yml (new)
- Dockerfile (new)
