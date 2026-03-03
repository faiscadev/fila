# Story 11.2: Package Registry Publishing

Status: review

## Story

As a maintainer,
I want all SDKs published to their respective package registries,
So that users can install Fila and its SDKs via standard package managers.

## Acceptance Criteria

1. **Given** the publish workflows exist in each SDK repo, **When** secrets are configured and a publish is triggered, **Then** `fila-proto` and `fila-sdk` are published to crates.io.
2. **Given** the Go SDK repo exists, **Then** `fila-go` has a dev version tag accessible via `go get`.
3. **Given** the Python SDK repo exists, **When** the publish workflow is triggered, **Then** `fila-python` is published to PyPI (OIDC trusted publisher configured).
4. **Given** the JS SDK repo exists, **When** the publish workflow is triggered, **Then** `@fila/client` is published to npm.
5. **Given** the Ruby SDK repo exists, **When** the publish workflow is triggered, **Then** `fila-client` gem is published to RubyGems.
6. **Given** the Java SDK repo exists, **When** the publish workflow is triggered, **Then** `dev.faisca:fila-client` is published to Maven Central.
7. **Given** the Rust crates are published, **Then** `fila-server` and `fila-cli` are installable via `cargo install`.
8. **Given** `install.sh` exists, **Then** DNS for `get.fila.dev` is configured (or install.sh updated to use raw GitHub URL), and `curl -fsSL <install-url> | bash` successfully installs the correct binary.
9. **Given** each publish workflow must be verified, **Then** each workflow is triggered at least once (on feature branch or via manual dispatch) and produces a successful publish before marking complete.

## Tasks / Subtasks

- [x] Task 1: Fix release.yml and crate metadata (AC: 1) — **dev-agent**
  - [x] 1.1: Remove the second (shorter) `publish-crates` block at lines 145-166 — it shadows the first complete block
  - [x] 1.2: Verify the remaining block publishes the full chain: fila-proto → fila-core → fila-sdk → fila-server → fila-cli
  - [x] 1.3: Fix `workspace.package.repository` in root `Cargo.toml`: change `https://github.com/faisca/fila` → `https://github.com/faiscadev/fila`
  - [x] 1.4: Add `homepage` and `keywords` to all 5 publishable crates (fila-proto, fila-core, fila-sdk, fila-server, fila-cli)
  - [x] 1.5: Run `cargo publish --dry-run -p fila-proto`, then fila-core, fila-sdk, fila-server, fila-cli — fix any issues
- [x] Task 2: Configure secrets and publish Rust crates to crates.io (AC: 1, 7) — **human (Lucas)**
  - [x] 2.1: Create crates.io API token and add as `CARGO_REGISTRY_TOKEN` repo secret on faiscadev/fila
  - [x] 2.2: Trigger the `publish-crates` job (either via release.yml on a v* tag, or run manually) and verify fila-proto + fila-sdk + fila-cli are on crates.io (fila-core and fila-server set to publish=false)
  - [x] 2.3: Verify `cargo install fila-cli` works (fila-server set to publish=false)
- [x] Task 3: Verify Go SDK dev tags (AC: 2) — **human (Lucas)**
  - [x] 3.1: Trigger the fila-go publish workflow (push to main or manual dispatch)
  - [x] 3.2: Verify a `v0.1.0-dev.{N}` tag is created — v0.1.0-dev.1, v0.1.0-dev.2 exist
  - [x] 3.3: Verify `go get github.com/faiscadev/fila-go@v0.1.0-dev.{N}` works
- [x] Task 4: Configure and publish Python SDK to PyPI (AC: 3) — **human (Lucas)**
  - [x] 4.1: Configure OIDC trusted publisher on PyPI for the faiscadev/fila-python repo
  - [x] 4.2: Trigger the dev-publish workflow (push to main) and verify package appears on TestPyPI
  - [x] 4.3: Create a v* tag to trigger the publish workflow and verify `fila-python` appears on PyPI
  - [x] 4.4: Verify `pip install fila-python` installs the published version (requires Python >=3.10)
- [x] Task 5: Configure and publish JS SDK to npm (AC: 4) — **human (Lucas)**
  - [x] 5.1: Create npm org/scope `@fila` if not exists, generate NPM_TOKEN, add as repo secret on faiscadev/fila-js
  - [x] 5.2: Trigger the publish workflow (v* tag) and verify `fila-client` appears on npm
  - [x] 5.3: Verify `npm install fila-client` installs the published version
- [x] Task 6: Configure and publish Ruby SDK to RubyGems (AC: 5) — **human (Lucas)**
  - [x] 6.1: Create RubyGems API key and add as `RUBYGEMS_API_KEY` repo secret on faiscadev/fila-ruby
  - [x] 6.2: Trigger the publish workflow (v* tag) and verify `fila-client` gem appears on RubyGems
  - [x] 6.3: Verify `gem install fila-client` installs the published version
- [x] Task 7: Configure and publish Java SDK to Maven Central (AC: 6) — **human (Lucas)** + **dev-agent**
  - [x] 7.1: Set up Sonatype Central Portal account, verify `dev.faisca` namespace
  - [x] 7.2: Generate GPG key pair for artifact signing (RSA 4096, uploaded to keyserver.ubuntu.com)
  - [x] 7.3: Add secrets: `OSSRH_USERNAME`, `OSSRH_PASSWORD`, `GPG_PRIVATE_KEY`, `GPG_PASSPHRASE` to faiscadev/fila-java
  - [x] 7.4: Trigger the publish workflow (v0.1.0 tag) — publish succeeded via Central Portal OSSRH Staging API
  - [x] 7.5: Verify the artifact is resolvable via Gradle/Maven dependency declaration
- [x] Task 8: Configure DNS / install URL (AC: 8) — **human (Lucas)** / **dev-agent** for install.sh update
  - [x] 8.1: Updated install.sh to use `https://raw.githubusercontent.com/faiscadev/fila/main/install.sh`
  - [x] 8.2: Verify `curl -fsSL <install-url> | bash` downloads and installs the correct binary for the host platform

## Dev Notes

### Context: Why This Story Exists

Story 10.2 built all the publish workflows and configured the infrastructure, but none of them were ever actually triggered. Story 11.1 verified the release pipeline (binary builds, GitHub Releases, Docker images, SDK CI binary downloads), proving the first half of the distribution chain works. This story completes the second half: getting packages into registries so users can `cargo install`, `pip install`, `npm install`, etc.

This is operational verification with configuration work — configuring secrets, setting up registry accounts, triggering workflows, and confirming packages land in registries.

### Dev-Agent vs Human Task Separation

This story is ~80% operational (account creation, secret configuration, workflow triggering). Clear ownership:

**Dev-agent tasks** (Tasks 1, parts of 8):
- Fix duplicate `publish-crates` job in release.yml
- Fix repository URL mismatch in workspace Cargo.toml
- Add `homepage` and `keywords` metadata to all publishable crates
- Run `cargo publish --dry-run` to validate
- Update install.sh URL if Lucas chooses the raw GitHub URL option

**Human tasks** (Tasks 2–7, parts of 8):
- Create registry accounts (crates.io, PyPI, npm, RubyGems, Sonatype OSSRH)
- Generate API tokens and GPG keys
- Configure GitHub repo secrets
- Trigger publish workflows and verify packages land in registries
- Configure DNS (if choosing `get.fila.dev` route)
- Verify `cargo install`, `pip install`, `npm install`, `gem install` work

The dev agent should NOT attempt to create accounts, configure secrets, or trigger external workflows. Those require Lucas's credentials and manual GitHub UI interaction.

### Known Bug: Duplicate `publish-crates` Job

`release.yml` has **two** `publish-crates` job definitions (lines 108–135 and 145–166). In YAML, duplicate keys are resolved by last-write-wins, so the second (shorter) block silently shadows the first. The first block has the correct full publish chain (fila-proto → fila-core → fila-sdk → fila-server → fila-cli). The second block only publishes fila-proto and fila-sdk. **Fix the duplicate before triggering the workflow.**

### Secrets Required Per Repo

| Repo | Secret | Source |
|------|--------|--------|
| faiscadev/fila | `CARGO_REGISTRY_TOKEN` | crates.io API token |
| faiscadev/fila-js | `NPM_TOKEN` | npm access token (automation type) |
| faiscadev/fila-ruby | `RUBYGEMS_API_KEY` | RubyGems API key |
| faiscadev/fila-java | `OSSRH_USERNAME` | Sonatype OSSRH username |
| faiscadev/fila-java | `OSSRH_PASSWORD` | Sonatype OSSRH password |
| faiscadev/fila-java | `GPG_PRIVATE_KEY` | GPG private key (armor exported) |
| faiscadev/fila-java | `GPG_PASSPHRASE` | GPG key passphrase |
| faiscadev/fila-python | (none — OIDC) | PyPI trusted publisher (GitHub OIDC) |
| faiscadev/fila-go | (none) | Go modules use git tags, no registry |

### Publishing Order Matters

Rust crates must be published in dependency order with index propagation delays:
1. `fila-proto` (no workspace deps)
2. `fila-core` (depends on fila-proto) — wait ~30s for crates.io index
3. `fila-sdk` (depends on fila-core) — wait ~30s
4. `fila-server` (depends on fila-core) — wait ~30s
5. `fila-cli` (depends on fila-core) — wait ~30s

The existing `publish-crates` job already has `sleep 30` between steps — just needs the duplicate removed.

### Known Bug: Repository URL Mismatch

`workspace.package.repository` in root `Cargo.toml` is set to `https://github.com/faisca/fila` but the actual GitHub org is `faiscadev`. All crates published to crates.io will display the wrong repository link. **Must fix to `https://github.com/faiscadev/fila` before publishing.**

### Crate Metadata — Current State and Gaps

All 5 publishable crates already have `description`, `license` (AGPL-3.0-or-later via workspace), and `repository` (via workspace, but wrong URL — see bug above).

**Missing from all crates:**
- `homepage` — add `"https://github.com/faiscadev/fila"` (or a docs URL if one exists)
- `keywords` — e.g., `["message-broker", "queue", "fair-scheduling", "grpc"]`
- `categories` — e.g., `["network-programming", "asynchronous"]`

**Already present:** `description` (each crate has a unique one-liner), `license`, `repository`, `include` (fila-proto only).

`fila-e2e` already has `publish = false` — correct.

### `cargo install` Build Prerequisites

`cargo install fila-server` and `cargo install fila-cli` will require users to have:
- **Rust toolchain** (obviously)
- **protoc** (Protocol Buffers compiler) — fila-proto build script uses `tonic-prost-build`
- **C/C++ compiler** — fila-core depends on `rocksdb` (C++ bindings) and `mlua` with `vendored` feature (builds Lua from C source)

This makes `cargo install` suitable for **Rust developers** who already have a build environment. Casual users should use `install.sh` (pre-built binaries) or Docker. Consider adding a note to crate descriptions or README about these prerequisites.

### install.sh DNS Decision

Current `install.sh` header says `curl -fsSL https://get.fila.dev | bash`. Options:
1. **Configure DNS**: `get.fila.dev` CNAME → raw GitHub URL. Professional but requires domain ownership.
2. **Use raw GitHub URL directly**: `https://raw.githubusercontent.com/faiscadev/fila/main/install.sh`. Works immediately, no DNS needed.
3. **Use GitHub Pages**: Deploy from main branch, gives `faiscadev.github.io/fila/install.sh`.

This is Lucas's call — the story AC says "configured OR install.sh updated to use raw GitHub URL."

### Previous Story Intelligence (11.1)

Story 11.1 verified:
- Bleeding-edge pipeline: all 5 jobs pass (4 builds + release + Docker)
- Binary downloads work for all 5 SDK CIs
- Integration tests run (not silently skipped)
- Docker images at `ghcr.io/faiscadev/fila`
- Fixed fila-python grpcio version mismatch (1.78.1 yanked → 1.78.0)

Key insight: the release pipeline infrastructure is solid. This story is about the **publish** side, not the build/release side.

### Git Intelligence

Recent commits (last 5) are all Epic 10/11 tracking updates. Codebase is stable at 278/278 tests, zero regressions. No code changes needed beyond the release.yml duplicate fix — this story is almost entirely operational (secret configuration, workflow triggering, registry verification).

### CLAUDE.md Compliance Notes

- **CI Workflow Verification**: Each publish workflow must be triggered at least once before marking done. Use feature branch trigger broadening or manual dispatch.
- **Integration Tests Must Actually Run**: Already verified in Story 11.1, but confirm after any workflow changes.

### Project Structure Notes

- Main repo workflows: `.github/workflows/release.yml` (publish-crates + Docker), `.github/workflows/bleeding-edge.yml` (verified in 11.1)
- External SDK repos: `faiscadev/fila-{go,python,js,ruby,java}` — each has `.github/workflows/publish.yml`
- fila-python also has `.github/workflows/dev-publish.yml` for TestPyPI
- `install.sh` at repo root
- Workspace crate publish config in root `Cargo.toml` + per-crate `Cargo.toml`

### References

- [Source: .github/workflows/release.yml] — Rust crate publish + release workflow
- [Source: _bmad-output/implementation-artifacts/10-2-sdk-package-publishing.md] — Original implementation of publish workflows
- [Source: _bmad-output/implementation-artifacts/11-1-verify-release-pipeline.md] — Pipeline verification results
- [Source: _bmad-output/implementation-artifacts/epic-10-retro-2026-03-01.md] — Why operational verification is needed
- [Source: install.sh] — Install script with `get.fila.dev` reference
- [Source: CLAUDE.md#CI Workflow Verification] — Must trigger workflows before marking done
- [Source: _bmad-output/planning-artifacts/epics.md#Epic 11] — Story ACs

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

- Task 1 complete: Fixed release.yml duplicate `publish-crates` job (second shorter block removed, first complete chain retained). Verified publish chain: fila-proto → fila-core → fila-sdk → fila-server → fila-cli with correct sleep intervals. Fixed workspace repository URL from `faisca/fila` to `faiscadev/fila`. Added `homepage` (workspace-level), `keywords`, and `categories` to all 5 publishable crates. `cargo publish --dry-run` passes for fila-proto; others fail only because dependencies aren't on crates.io yet (expected). 278/278 tests pass, zero regressions.
- Tasks 2–6, 8 completed by Lucas in remote session (2026-03-02): Rust crates published (fila-proto, fila-sdk, fila-cli — fila-core and fila-server set to publish=false). Go dev tags created. Python published to PyPI. JS published to npm as fila-client. Ruby published to RubyGems. install.sh updated to raw GitHub URL.
- Task 7 completed (2026-03-03): Fixed Java SDK publish — added signing plugin, POM developers/scm sections, migrated from dead OSSRH endpoint to Central Portal OSSRH Staging API via gradle-nexus/publish-plugin. Generated GPG key (RSA 4096), uploaded to keyserver, configured 4 secrets via gh CLI. Publish workflow succeeded on v0.1.0 tag.

### Change Log

- 2026-03-01: Task 1 — Fixed release.yml duplicate publish-crates job, fixed repository URL, added crate metadata (homepage, keywords, categories)
- 2026-03-02: Tasks 2–6, 8 — Lucas published all SDKs except Java, updated install.sh, set fila-core/fila-server to publish=false
- 2026-03-03: Task 7 — Fixed Java SDK: added signing plugin, POM metadata, migrated to Central Portal, generated GPG key, published dev.faisca:fila-client to Maven Central

### File List

- .github/workflows/release.yml (modified — removed duplicate publish-crates block, removed fila-core/fila-server from chain)
- Cargo.toml (modified — fixed repository URL, added homepage)
- crates/fila-proto/Cargo.toml (modified — added homepage, keywords, categories)
- crates/fila-core/Cargo.toml (modified — added homepage, keywords, categories, publish=false)
- crates/fila-sdk/Cargo.toml (modified — added homepage, keywords, categories)
- crates/fila-server/Cargo.toml (modified — added homepage, keywords, categories, publish=false)
- crates/fila-cli/Cargo.toml (modified — added homepage, keywords, categories)
- install.sh (modified — updated URL to raw GitHub URL)
- [external] faiscadev/fila-java: build.gradle (added signing plugin, nexus-publish-plugin, POM developers/scm)
- [external] faiscadev/fila-java: .github/workflows/publish.yml (migrated to Central Portal staging API)
