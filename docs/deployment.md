# Deployment Guide

This guide covers deploying Fila in production environments.

## Docker (single node)

The quickest way to run Fila:

```sh
docker run -d \
  --name fila \
  -p 5555:5555 \
  -v fila-data:/var/lib/fila \
  ghcr.io/faiscadev/fila:latest
```

With a custom config file:

```sh
docker run -d \
  --name fila \
  -p 5555:5555 \
  -v fila-data:/var/lib/fila \
  -v ./fila.toml:/etc/fila/fila.toml:ro \
  ghcr.io/faiscadev/fila:latest \
  fila-server --config /etc/fila/fila.toml
```

## systemd

For bare-metal or VM deployments on Linux.

### 1. Install the binary

```sh
curl -fsSL https://raw.githubusercontent.com/faiscadev/fila/main/install.sh -o install.sh
bash install.sh
```

### 2. Create system user and directories

```sh
sudo useradd --system --no-create-home --shell /usr/sbin/nologin fila
sudo mkdir -p /etc/fila /var/lib/fila
sudo chown fila:fila /var/lib/fila
```

### 3. Create configuration

Copy the example config and customize:

```sh
sudo cp deploy/fila.toml /etc/fila/fila.toml
sudo editor /etc/fila/fila.toml
```

See [Configuration Reference](configuration.md) for all options.

### 4. Install and start the service

```sh
sudo cp deploy/fila.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now fila
sudo systemctl status fila
```

### 5. Verify

```sh
fila queue create test-queue
fila queue inspect test-queue
```

## Docker Compose cluster

Run a 3-node cluster locally using Docker Compose.

### 1. Start the cluster

```sh
docker compose -f deploy/docker-compose.cluster.yml up -d
```

This starts three Fila nodes:
- **node1** — bootstrap node, port 5555
- **node2** — port 5565
- **node3** — port 5575

### 2. Create a queue and test

```sh
# Connect to any node
fila --addr localhost:5555 queue create orders

# Enqueue via node1, consume via node2 (transparent routing)
fila --addr localhost:5555 enqueue orders --payload "hello"
fila --addr localhost:5565 queue inspect orders
```

### 3. Stop the cluster

```sh
docker compose -f deploy/docker-compose.cluster.yml down
# Add -v to also remove data volumes
```

## Kubernetes with Helm

### Single-node deployment

```sh
helm install fila deploy/helm/fila/
```

### Clustered deployment

```sh
helm install fila deploy/helm/fila/ \
  --set replicaCount=3 \
  --set cluster.enabled=true \
  --set cluster.replicationFactor=3 \
  --set persistence.enabled=true \
  --set persistence.size=20Gi
```

### With TLS and authentication

```sh
# Create TLS secret first
kubectl create secret tls fila-tls \
  --cert=server.crt --key=server.key

helm install fila deploy/helm/fila/ \
  --set tls.enabled=true \
  --set tls.certSecret=fila-tls \
  --set auth.bootstrapApiKey=your-secret-key
```

### With OpenTelemetry

```sh
helm install fila deploy/helm/fila/ \
  --set telemetry.enabled=true \
  --set telemetry.otlpEndpoint=http://otel-collector:4317
```

### Custom values file

Create a `my-values.yaml`:

```yaml
replicaCount: 3

cluster:
  enabled: true
  replicationFactor: 3

persistence:
  enabled: true
  size: 50Gi
  storageClass: gp3

tls:
  enabled: true
  certSecret: fila-tls

auth:
  bootstrapApiKey: "change-me-in-production"

telemetry:
  enabled: true
  otlpEndpoint: "http://otel-collector.monitoring:4317"

resources:
  requests:
    cpu: 500m
    memory: 512Mi
  limits:
    cpu: 2000m
    memory: 2Gi
```

```sh
helm install fila deploy/helm/fila/ -f my-values.yaml
```

## Production checklist

Before going to production, verify these items:

- [ ] **Data persistence** — data directory is on a persistent volume (not ephemeral container storage)
- [ ] **TLS enabled** — all client connections encrypted; consider mTLS for service-to-service
- [ ] **API key auth** — bootstrap API key set; per-queue ACLs configured for multi-tenant
- [ ] **Backups** — data directory backup strategy in place (RocksDB is crash-consistent, point-in-time snapshots work)
- [ ] **Monitoring** — OTel exporter configured, scraping endpoint or collector receiving metrics
- [ ] **Log level** — set `RUST_LOG=info` (default); use `debug` only for troubleshooting
- [ ] **File descriptors** — `LimitNOFILE=65536` or higher for high-connection workloads
- [ ] **Cluster sizing** — for HA, run 3+ nodes with `replication_factor = 3`
- [ ] **Resource limits** — set CPU/memory limits appropriate for workload
- [ ] **Lua safety** — review script timeout and memory limits per queue
