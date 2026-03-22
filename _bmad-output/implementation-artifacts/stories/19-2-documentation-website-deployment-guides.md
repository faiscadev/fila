# Story 19.2: Documentation Website & Deployment Guides

Status: review

## Story

As an evaluator or operator,
I want a polished documentation website with deployment guides,
so that I can evaluate Fila quickly and deploy it confidently in production.

## Acceptance Criteria

1. **Given** the existing docs/ markdown files
   **When** the docs website is built
   **Then** an mdBook site is generated from `docs/` with a logical navigation structure
   **And** `book.toml` configures the site with title, theme, and search enabled
   **And** `docs/SUMMARY.md` defines the table of contents linking all existing docs

2. **Given** an operator wants to deploy Fila on a Linux server
   **When** they follow the systemd deployment guide
   **Then** `deploy/fila.service` provides a production-ready systemd unit file
   **And** `docs/deployment.md` includes systemd setup instructions (install binary, create config, enable service)

3. **Given** an operator wants to deploy Fila on Kubernetes
   **When** they follow the Kubernetes deployment guide
   **Then** a Helm chart at `deploy/helm/fila/` provides single-node and clustered deployments
   **And** `values.yaml` supports: `replicaCount`, `cluster.enabled`, `cluster.replicationFactor`, `tls.enabled`, `auth.bootstrapApiKey`, `persistence.enabled`, `persistence.size`
   **And** the chart includes: Deployment (single-node) or StatefulSet (clustered), Service, ConfigMap, optional PersistentVolumeClaim
   **And** `docs/deployment.md` includes Helm install instructions

4. **Given** an operator wants to run a multi-node cluster locally
   **When** they use docker-compose
   **Then** `deploy/docker-compose.cluster.yml` starts a 3-node Fila cluster with shared network
   **And** `docs/deployment.md` includes docker-compose cluster instructions

5. **Given** the deployment guide exists
   **Then** `docs/deployment.md` covers: systemd, Docker single-node, Docker Compose cluster, Kubernetes/Helm
   **And** each section includes configuration examples with comments
   **And** a production checklist covers: data directory persistence, TLS, API key auth, OTel export, log levels

6. **Given** mdBook is configured
   **Then** a GitHub Actions workflow `docs.yml` builds and deploys the docs site to GitHub Pages on push to main

## Tasks / Subtasks

- [x] Task 1: Configure mdBook (AC: 1)
  - [x] 1.1: Create `book.toml` in project root
  - [x] 1.2: Create `docs/SUMMARY.md` with table of contents linking all existing docs
  - [x] 1.3: Verify `mdbook build` produces a working site

- [x] Task 2: Create systemd deployment (AC: 2)
  - [x] 2.1: Create `deploy/fila.service` systemd unit file
  - [x] 2.2: Create `deploy/fila.toml` example production config

- [x] Task 3: Create Helm chart (AC: 3)
  - [x] 3.1: Create `deploy/helm/fila/Chart.yaml`
  - [x] 3.2: Create `deploy/helm/fila/values.yaml` with configurable options
  - [x] 3.3: Create deployment template (Deployment for single-node, StatefulSet for clustered)
  - [x] 3.4: Create Service, ConfigMap templates
  - [x] 3.5: Create optional PVC template for persistent storage

- [x] Task 4: Create docker-compose cluster config (AC: 4)
  - [x] 4.1: Create `deploy/docker-compose.cluster.yml` with 3 Fila nodes
  - [x] 4.2: Create node config files for the 3-node cluster

- [x] Task 5: Write deployment guide (AC: 5)
  - [x] 5.1: Create `docs/deployment.md` with all deployment methods
  - [x] 5.2: Add production checklist section

- [x] Task 6: Create GitHub Pages workflow (AC: 6)
  - [x] 6.1: Create `.github/workflows/docs.yml` to build mdBook and deploy to GitHub Pages

- [x] Task 7: Update sprint-status.yaml
  - [x] 7.1: Mark story 19-2 as in-progress

## Dev Notes

### Architecture Context

All documentation exists in `docs/` as standalone markdown files. The story adds a static site generator (mdBook) and deployment infrastructure.

### Key Design Decision: mdBook

mdBook is chosen because:
- Rust-native (fits the project ecosystem)
- Zero runtime dependencies (single binary)
- Built-in search
- GitHub Pages deployment is trivial
- Existing markdown files work as-is (just need SUMMARY.md)

### Key Design Decision: Helm Chart Structure

The Helm chart uses a conditional template: `Deployment` for single-node mode, `StatefulSet` for clustered mode. The `cluster.enabled` value controls which is used. ConfigMap holds the TOML config generated from values.

### Existing Documentation Files

Files already in `docs/`:
- `concepts.md` — Core concepts
- `api-reference.md` — gRPC API
- `configuration.md` — Config reference
- `tutorials.md` — Guided walkthroughs
- `sdk-examples.md` — SDK code examples
- `lua-patterns.md` — Lua hook patterns
- `compatibility.md` — Versioning policy
- `benchmarks.md` — Performance results
- `cluster-scaling.md` — Cluster setup guide

### Docker Reference

Existing `Dockerfile` builds both `fila-server` and `fila` CLI. Exposes port 5555. Uses `debian:trixie-slim` runtime.

### Cluster Config Reference

From `docs/cluster-scaling.md`, cluster config uses:
```toml
[cluster]
node_id = 1
bind_addr = "0.0.0.0:5556"
bootstrap = true
peers = ["node2:5556", "node3:5556"]
replication_factor = 3
```

### Existing Patterns to Follow

- `.github/workflows/ci.yml` — existing CI workflow pattern
- `Dockerfile` — existing Docker build pattern
- `docs/cluster-scaling.md` — existing cluster config examples

### Testing Standards

This is primarily an infrastructure/docs story. Validation:
- `mdbook build` succeeds
- `helm lint deploy/helm/fila/` passes
- `helm template deploy/helm/fila/` renders valid YAML
- docker-compose config validates

### References

- [Source: docs/] — existing documentation files
- [Source: Dockerfile] — existing Docker setup
- [Source: docs/cluster-scaling.md] — cluster config examples
- [Source: docs/configuration.md] — full config reference
- [Source: .github/workflows/ci.yml] — CI workflow pattern

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- mdBook chosen for Rust-native simplicity; builds in <1s from existing markdown
- Helm chart uses conditional Deployment vs StatefulSet based on cluster.enabled
- Docker Compose cluster uses Docker's internal DNS for peer discovery (node1, node2, node3)
- GitHub Pages workflow triggers only on docs/ or book.toml changes
- All existing markdown docs work as-is with mdBook, just needed SUMMARY.md navigation

### File List

- `book.toml` — NEW: mdBook configuration
- `.gitignore` — MODIFIED: added /book/ build output
- `docs/SUMMARY.md` — NEW: mdBook table of contents
- `docs/introduction.md` — NEW: landing page (from README)
- `docs/deployment.md` — NEW: comprehensive deployment guide
- `deploy/fila.service` — NEW: systemd unit file
- `deploy/fila.toml` — NEW: example production config
- `deploy/helm/fila/Chart.yaml` — NEW: Helm chart metadata
- `deploy/helm/fila/values.yaml` — NEW: configurable chart values
- `deploy/helm/fila/templates/_helpers.tpl` — NEW: Helm template helpers
- `deploy/helm/fila/templates/configmap.yaml` — NEW: ConfigMap with TOML config
- `deploy/helm/fila/templates/deployment.yaml` — NEW: single-node Deployment
- `deploy/helm/fila/templates/statefulset.yaml` — NEW: clustered StatefulSet
- `deploy/helm/fila/templates/service.yaml` — NEW: Service + headless Service
- `deploy/helm/fila/templates/pvc.yaml` — NEW: optional PVC for single-node
- `deploy/docker-compose.cluster.yml` — NEW: 3-node cluster compose
- `deploy/cluster/node1.toml` — NEW: node1 cluster config
- `deploy/cluster/node2.toml` — NEW: node2 cluster config
- `deploy/cluster/node3.toml` — NEW: node3 cluster config
- `.github/workflows/docs.yml` — NEW: GitHub Pages deployment workflow
