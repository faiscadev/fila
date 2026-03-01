# Story 11.1: Verify Release Pipeline End-to-End

Status: review

## Story

As a maintainer,
I want to verify that the bleeding-edge release pipeline actually works,
So that SDK CIs can download pre-built binaries and users can pull Docker images.

## Acceptance Criteria

1. **Given** Epic 10 code is merged to main, **When** the bleeding-edge workflow triggers, **Then** binaries are produced for all 4 platforms (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64).
2. **Given** the build completes, **Then** a GitHub Release tagged `dev-{sha}` is created with all binary assets and SHA256 checksums.
3. **Given** the release is created, **Then** the rolling `dev-latest` pre-release tag points to the newest build.
4. **Given** the build completes, **Then** the Docker image is published to `ghcr.io/faisca/fila` with `dev` and `dev-{sha}` tags.
5. **Given** the release is available, **Then** all 5 external SDK CIs (Go, Python, JS, Ruby, Java) successfully download the binary via `gh release download`.
6. **Given** the binary is downloaded, **Then** integration tests in all 5 SDK CIs actually execute and pass against the downloaded binary.

## Tasks / Subtasks

- [x] Task 1: Trigger and verify bleeding-edge.yml (AC: 1, 2, 3, 4)
  - [x] 1.1: Temporarily broaden bleeding-edge.yml trigger to include the feature branch
  - [x] 1.2: Push and verify the workflow runs successfully
  - [x] 1.3: Verify binaries are produced for all 4 platforms (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64)
  - [x] 1.4: Verify GitHub Release `dev-{sha}` is created with all assets + SHA256 checksums
  - [x] 1.5: Verify rolling `dev-latest` pre-release tag points to the newest build
  - [x] 1.6: Verify Docker image pushed to `ghcr.io/faisca/fila` with `dev` and `dev-{sha}` tags
  - [x] 1.7: Revert trigger broadening — narrow back to `main` only
- [x] Task 2: Verify SDK CIs download and test against the binary (AC: 5, 6)
  - [x] 2.1: Verify Go SDK CI downloads binary and integration tests execute + pass
  - [x] 2.2: Verify Python SDK CI downloads binary and integration tests execute + pass
  - [x] 2.3: Verify JS SDK CI downloads binary and integration tests execute + pass
  - [x] 2.4: Verify Ruby SDK CI downloads binary and integration tests execute + pass
  - [x] 2.5: Verify Java SDK CI downloads binary and integration tests execute + pass
- [x] Task 3: Fix any issues found during verification
  - [x] 3.1: Fix build failures, workflow bugs, or missing secrets as discovered
  - [x] 3.2: Re-run and confirm fixes resolve the issues

## Dev Notes

### Context: Why This Story Exists

Epic 10 built all the release infrastructure — bleeding-edge.yml, release.yml, Dockerfile, install.sh, SDK publish workflows — but **none of it was ever triggered**. Story 10.1 Task 4 ("verify end-to-end") was explicitly marked incomplete with the note "requires merge to main." Lucas flagged this in the Epic 10 retro: "Building automation is not the same as verifying automation works. Untriggered pipelines are untested code."

This story is operational verification, not new engineering. The code exists; we need to prove it works.

### Bleeding-Edge Workflow Architecture

File: `.github/workflows/bleeding-edge.yml`

The workflow has 4 jobs:
1. **build** — Compiles `fila-server` and `fila` CLI for 4 platforms using cross-compilation (`cross` for ARM targets)
2. **package** — Creates tarball archives with binaries + LICENSE, generates SHA256 checksums
3. **release** — Creates two GitHub Releases:
   - Commit-specific pre-release tagged `dev-{7-char-sha}`
   - Rolling `dev-latest` pre-release that always points to the latest main build
4. **docker** — Builds and pushes multi-stage Docker image to GHCR with `dev` and `dev-{sha}` tags

### Prerequisites / Secrets

- `GITHUB_TOKEN` — automatically available in GitHub Actions (used for releases and GHCR push)
- No additional secrets required for bleeding-edge.yml (unlike release.yml which needs `CARGO_REGISTRY_TOKEN`)

### SDK CI Integration

All 5 external SDK repos (fila-go, fila-python, fila-js, fila-ruby, fila-java) were updated in Story 10.2 to download pre-built binaries from the `dev-latest` release instead of building from source. Their CI needs:
- `gh release download` from `faiscadev/fila` repo (verify org/repo name is correct)
- The binary must be executable and start the server correctly
- Integration tests must actually run (not silently skip) per CLAUDE.md "Integration Tests Must Actually Run" rule

### Potential Issues to Watch For

1. **Cross-compilation failures** — ARM builds use the `cross` tool; may have linking or dependency issues
2. **GitHub Release tag conflicts** — Rolling `dev-latest` tag deletion/recreation may fail on first run
3. **Docker build failures** — Multi-stage build requires protoc installation
4. **GHCR authentication** — Verify GITHUB_TOKEN has `packages: write` permission
5. **SDK CI repo access** — `gh release download` from a different repo may need authentication
6. **Org/repo naming** — install.sh references `faiscadev/fila`; verify this matches actual GitHub org

### CLAUDE.md Compliance

Per "CI Workflow Verification" rule: the workflow must be triggered on the feature branch to verify it works before the story is done. The trigger broadening/narrowing is Task 1.1 and 1.7.

### Project Structure Notes

- `.github/workflows/bleeding-edge.yml` — the pipeline under test
- `.github/workflows/release.yml` — NOT in scope for this story (that's for tagged releases)
- `Dockerfile` — used by the docker job in bleeding-edge.yml
- `install.sh` — NOT directly tested here (depends on release existing, which is the output of this story)

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Epic 11, Story 11.1]
- [Source: _bmad-output/implementation-artifacts/epic-10-retro-2026-03-01.md#Challenges]
- [Source: .github/workflows/bleeding-edge.yml]
- [Source: CLAUDE.md#CI Workflow Verification]
- [Source: CLAUDE.md#Integration Tests Must Actually Run]

### Previous Story Intelligence

Story 10.5 (tutorials/examples) was the last engineering story. It added Rust examples and docs. No direct technical overlap with this operational verification story, but the CI workflow (`.github/workflows/ci.yml`) was modified to lint examples — confirms CI infrastructure is functional for standard workflows.

### Git Intelligence

Recent commits are all Epic 10 wrap-up: retro, tracking updates, doc fixes. The codebase is stable at 278/278 tests. No recent infrastructure changes that would affect pipeline behavior.

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

- Bleeding-edge workflow run 22553079395 (feature branch): all 5 jobs passed
- Previous main run 22552806480: already passing before this story
- Go SDK CI re-run 22268465968: 3/3 tests PASSED
- Python SDK CI run 22553425770: 4/4 tests PASSED (after grpcio fix)
- JS SDK CI re-run 22270068296: 3/3 tests PASSED
- Ruby SDK CI re-run 22270062388: 3 runs, 15 assertions, 0 failures
- Java SDK CI re-run 22270073392: 3/3 tests PASSED, BUILD SUCCESSFUL

### Completion Notes List

- Bleeding-edge.yml pipeline verified end-to-end on feature branch: 4 build jobs (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64) + release + Docker all pass
- GitHub Release `dev-3511a30` created with 8 assets (4 tarballs + 4 SHA256 checksums)
- Rolling `dev-latest` tag correctly points to newest build
- Docker image pushed to `ghcr.io/faiscadev/fila` with `dev` and `dev-3511a30...` tags
- All 5 SDK CIs download binary via `gh release download dev-latest` and integration tests execute and pass
- Fixed fila-python grpcio version mismatch: generated stubs required grpcio>=1.78.1 but that version was yanked from PyPI; downgraded version check to 1.78.0
- Trigger broadening reverted — bleeding-edge.yml is back to `main` only
- No cross-compilation issues, no tag conflicts, no Docker build failures, no auth issues
- 278/278 local tests pass, zero regressions

### Change Log

- 2026-03-01: Verified bleeding-edge pipeline end-to-end. Fixed fila-python grpcio 1.78.1 yanked version issue. All 5 SDK CIs green.

### File List

- .github/workflows/bleeding-edge.yml (temporarily modified then reverted — net: no change)
- _bmad-output/implementation-artifacts/sprint-status.yaml (modified: status tracking)
- _bmad-output/implementation-artifacts/11-1-verify-release-pipeline.md (modified: story tracking)
- (external) faiscadev/fila-python: pyproject.toml, fila/v1/service_pb2_grpc.py, fila/v1/admin_pb2_grpc.py, fila/v1/messages_pb2_grpc.py (fixed grpcio version)
