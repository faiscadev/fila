# Story 10.1: Server & CLI Release Pipeline + Docker Image

Status: done

## Story

As a developer and user,
I want bleeding-edge releases of fila-server and fila CLI from every push to main, plus a Docker image,
So that SDK CIs can download pre-built binaries, and users can try Fila with a single Docker command.

## Acceptance Criteria

1. **Given** a commit is pushed to main, **When** the bleeding-edge release workflow runs, **Then** cross-compiled binaries are built for: linux-amd64, linux-arm64, darwin-amd64, darwin-arm64.
2. **Given** the workflow runs, **Then** both `fila-server` and `fila` (CLI) binaries are included for each platform, packaged as `tar.gz` with checksums.
3. **Given** the workflow succeeds, **Then** a GitHub Release is created tagged with the short commit hash (e.g., `dev-abc1234`), marked as pre-release.
4. **Given** the workflow succeeds, **Then** a `latest` pre-release tag is updated to always point to the most recent build (overwrite previous).
5. **Given** the bleeding-edge release exists, **Then** SDK CI workflows can download the latest pre-built binary via `gh release download latest` or direct URL.
6. **Given** the release workflow produces binaries, **Then** a multi-stage Dockerfile builds from `rust:latest` and produces a `debian:bookworm-slim` runtime image.
7. **Given** the Docker image is built, **Then** the image contains both `fila-server` and `fila` (CLI) binaries.
8. **Given** the Docker image is built, **Then** the image exposes port 5555 and `docker run ghcr.io/faisca/fila` starts the broker with default config.
9. **Given** the Docker image is built, **Then** it is published to `ghcr.io/faisca/fila` with `dev` and commit-hash tags on every push to main.
10. **Given** the Docker image is running, **Then** data directory is configurable via volume mount (`-v /data:/var/lib/fila`) and env var overrides work (`-e FILA_SERVER__LISTEN_ADDR=0.0.0.0:6666`).
11. **Given** the Docker image is built, **Then** the image size is minimized (no build tools in runtime layer, only `ca-certificates` and binaries).

## Tasks / Subtasks

- [x] Task 1: Create bleeding-edge release workflow (AC: 1-5)
  - [x] 1.1: Create `.github/workflows/bleeding-edge.yml` triggered on push to main
  - [x] 1.2: Reuse the existing release.yml build matrix (4 platforms, `cross` for linux-arm64)
  - [x] 1.3: Package binaries as `fila-dev-{sha}-{platform}.tar.gz` with SHA256 checksums
  - [x] 1.4: Create/update GitHub Release tagged `dev-{short_sha}` marked as pre-release
  - [x] 1.5: Create/update a rolling `latest` pre-release tag pointing to the newest build
- [x] Task 2: Update Docker build to publish on push to main (AC: 6-11)
  - [x] 2.1: Docker build job in bleeding-edge.yml (push to main trigger)
  - [x] 2.2: Keep existing Dockerfile as-is (already correct: multi-stage, bookworm-slim, both binaries)
  - [x] 2.3: Tag image with `dev` and `dev-{full_sha}` on every push to main
  - [x] 2.4: Verify image exposes port 5555, entrypoint is `fila-server` (Dockerfile unchanged)
- [x] Task 3: Keep existing release.yml for tagged releases (no changes needed)
  - [x] 3.1: Verify release.yml still triggers on `v*` tags for stable releases — confirmed, unchanged
  - [x] 3.2: Ensure no conflicts between bleeding-edge and stable release workflows — different triggers, no overlap
- [ ] Task 4: Verify end-to-end (requires merge to main)
  - [ ] 4.1: Push to main produces bleeding-edge release with binaries for all 4 platforms
  - [ ] 4.2: `latest` pre-release tag is accessible for SDK CI binary download

## Dev Notes

### Existing Infrastructure

The project already has substantial release infrastructure:

- **`.github/workflows/release.yml`** — Triggered on `v*` tags. 4-platform build matrix (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64). Uses `cross` for linux-arm64 cross-compilation, native macOS runners for darwin targets. Produces `tar.gz` with checksums. Already builds Docker image on tag.
- **`.github/workflows/ci.yml`** — PR checks: fmt, clippy, nextest, bench.
- **`.github/workflows/e2e.yml`** — E2E tests on PR/push to main.
- **`Dockerfile`** — Multi-stage: `rust:latest` builder → `debian:bookworm-slim` runtime. Already contains both `fila-server` and `fila` binaries.

### What Needs to Change

The key delta is: **release.yml triggers on tags only**. We need a new workflow that triggers on **push to main** and produces bleeding-edge releases.

1. **New workflow: `.github/workflows/bleeding-edge.yml`**
   - Trigger: `push` to `main` branch
   - Reuse the same 4-platform build matrix from `release.yml`
   - Tag format: `dev-{short_sha}` (e.g., `dev-abc1234`)
   - Additionally maintain a rolling `latest` pre-release tag
   - Build and push Docker image with `dev` and `dev-{short_sha}` tags

2. **Do NOT modify `release.yml`** — it works correctly for stable tagged releases.

3. **Do NOT modify the `Dockerfile`** — it already produces the correct image.

### Build Matrix Reference (from release.yml)

```yaml
matrix:
  include:
    - target: x86_64-unknown-linux-gnu
      os: ubuntu-latest
      platform: linux-amd64
      use_cross: false
    - target: aarch64-unknown-linux-gnu
      os: ubuntu-latest
      platform: linux-arm64
      use_cross: true
    - target: x86_64-apple-darwin
      os: macos-13
      platform: darwin-amd64
      use_cross: false
    - target: aarch64-apple-darwin
      os: macos-14
      platform: darwin-arm64
      use_cross: false
```

### Rolling `latest` Tag Strategy

Use `gh release` or GitHub API to:
1. Delete the existing `latest` release (if exists)
2. Delete the `latest` git tag (if exists)
3. Create new `latest` release pointing to current commit, upload all binaries

Alternatively: use `softprops/action-gh-release` with `tag_name: latest` and `prerelease: true` to overwrite.

### SDK CI Download Pattern

After this story, SDK CIs will download binaries like:
```bash
gh release download latest --repo faisca/fila --pattern "fila-dev-*-linux-amd64.tar.gz"
tar xzf fila-dev-*-linux-amd64.tar.gz
```

### Docker Tags

- On push to main: `ghcr.io/faisca/fila:dev`, `ghcr.io/faisca/fila:dev-{sha}`
- On tagged release (existing): `ghcr.io/faisca/fila:{version}`, `ghcr.io/faisca/fila:{major.minor}`

### Binary Names

- Server: `fila-server` (package name, no `[[bin]]` override)
- CLI: `fila` (explicit `[[bin]]` in `fila-cli/Cargo.toml`)

### Project Structure Notes

- Workspace: 6 crates (fila-proto, fila-core, fila-server, fila-cli, fila-sdk, fila-e2e)
- Repository: `https://github.com/faisca/fila`
- Docker registry: `ghcr.io/faisca/fila`
- License: AGPL-3.0-or-later (included in release tarballs)

### References

- [Source: .github/workflows/release.yml] — Existing release workflow with build matrix
- [Source: .github/workflows/ci.yml] — CI checks
- [Source: Dockerfile] — Multi-stage Docker build
- [Source: _bmad-output/planning-artifacts/architecture.md#Infrastructure & Deployment] — Distribution channels and CI/CD specs
- [Source: _bmad-output/implementation-artifacts/epic-9-retro-2026-02-21.md] — Epic 10 reshaping, bleeding-edge release decision

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

None — clean implementation, no issues encountered.

### Completion Notes List

- Created `.github/workflows/bleeding-edge.yml` with 3 jobs: build (4-platform matrix), release (dev tag + latest rolling tag), docker (dev + sha tags)
- Build matrix reused from existing `release.yml` with identical platform/cross-compilation config
- Rolling `latest` tag strategy: delete old release+tag, create new ones pointing to current commit
- Docker tags: `dev` (rolling) and `dev-{full_sha}` (immutable) on every push to main
- Existing `release.yml` and `Dockerfile` unchanged — no conflicts with stable release path
- 278/278 tests pass, no regressions

### Change Log

- **Added** `.github/workflows/bleeding-edge.yml` — new workflow for bleeding-edge releases on push to main
- **Updated** `_bmad-output/implementation-artifacts/sprint-status.yaml` — reshaped Epic 10 stories, marked 10.1 ready-for-dev
- **Updated** `_bmad-output/epic-execution-state.yaml` — Epic 10 initialized

### File List

- `.github/workflows/bleeding-edge.yml` (new)
- `_bmad-output/implementation-artifacts/10-1-release-pipeline-docker.md` (new)
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (modified)
- `_bmad-output/epic-execution-state.yaml` (modified)
