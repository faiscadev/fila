# Story 10.3: Binary Distribution & Installation

Status: done

## Story

As a user,
I want to install Fila via cargo or a shell script,
So that I can run it natively on my machine without Docker.

## Acceptance Criteria

1. **Given** the Fila crates are published, **When** a user runs `cargo install fila-server`, **Then** the broker binary is installed.
2. **Given** the Fila crates are published, **When** a user runs `cargo install fila-cli`, **Then** the CLI binary is installed as `fila`.
3. **Given** release binaries are available, **When** a user runs `curl -fsSL https://get.fila.dev | bash`, **Then** the correct binary for their platform is downloaded and installed.
4. **Given** the install script runs, **Then** it detects OS (linux/darwin) and architecture (amd64/arm64).
5. **Given** the install script runs, **Then** binaries are placed in `/usr/local/bin` (with sudo) or `~/.local/bin` (without sudo).
6. **Given** a semver version is tagged, **When** the release workflow runs, **Then** a stable GitHub Release is created (not pre-release) with the version tag.
7. **Given** release artifacts exist, **Then** checksums are generated for verification.

## Tasks / Subtasks

- [x] Task 1: Create install shell script (AC: 3-5, 7)
  - [x] 1.1: Create `install.sh` that detects platform and arch
  - [x] 1.2: Download binary from GitHub Releases (latest stable or specified version)
  - [x] 1.3: Verify checksum after download (sha256sum or shasum)
  - [x] 1.4: Install to `/usr/local/bin` (with sudo) or custom dir via `--install-dir`
- [x] Task 2: Prepare crates for cargo install (AC: 1-2)
  - [x] 2.1: fila-server and fila-cli already have correct metadata
  - [x] 2.2: Added publish steps for fila-core, fila-server, fila-cli to release workflow
- [x] Task 3: Verify stable release workflow (AC: 6-7)
  - [x] 3.1: release.yml creates stable release on v* tags — confirmed, includes checksums

## Dev Notes

### Install Script Design

The script should:
1. Detect OS: `uname -s` → linux/darwin
2. Detect arch: `uname -m` → x86_64/aarch64/arm64 → amd64/arm64
3. Construct download URL from GitHub Releases
4. Download tarball + checksum
5. Verify checksum with `shasum -a 256 -c`
6. Extract and install binaries
7. Print success message

### cargo install Preparation

fila-server and fila-cli need workspace path deps to have version keys (already done in Story 10.2). They also need `publish = true` (default) and proper metadata.

For cargo install to work, fila-core must also be published (fila-server depends on it). The publish order in release.yml needs to be: fila-proto → fila-core → fila-sdk → fila-server → fila-cli.

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Infrastructure & Deployment] — Distribution channels
- [Source: .github/workflows/release.yml] — Existing release workflow
- [Source: Cargo.toml] — Workspace with version keys (Story 10.2)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Completion Notes List

- Created `install.sh` — POSIX-compatible shell script for binary installation
- Script detects OS (linux/darwin) and arch (amd64/arm64), downloads from GitHub Releases, verifies checksum
- Supports `--version` and `--install-dir` flags
- Updated release.yml to publish all 5 crates in dependency order
- 278/278 tests pass, no regressions

### Change Log

- **Added** `install.sh` — platform-aware binary installer script
- **Modified** `.github/workflows/release.yml` — added fila-core, fila-server, fila-cli to publish-crates job

### File List

- `install.sh` (new)
- `.github/workflows/release.yml` (modified)
- `_bmad-output/implementation-artifacts/10-3-binary-distribution.md` (new)
