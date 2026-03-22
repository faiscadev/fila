# Troubleshooting

Common issues and their solutions.

## Connection refused

**Symptom:** `transport error: connection refused` or `failed to connect`

**Causes:**
- Broker not running. Start it: `fila-server` or `docker run -p 5555:5555 ghcr.io/faiscadev/fila:latest`
- Wrong address. Default is `localhost:5555`. Check your connection string.
- Firewall blocking port 5555. Verify: `nc -zv localhost 5555`
- In Docker: use `host.docker.internal:5555` from containers, not `localhost`

## TLS errors

**Symptom:** `tls handshake error`, `certificate verify failed`, or `unknown ca`

**Solutions:**
- **Self-signed cert:** Provide the CA certificate to the SDK (`with_tls_ca("ca.crt")`)
- **System trust store:** Use `with_tls()` (no CA path) if the broker's cert is issued by a public CA
- **Expired cert:** Check cert expiry: `openssl x509 -in server.crt -noout -enddate`
- **Wrong hostname:** The cert's CN/SAN must match the hostname you're connecting to
- **mTLS required:** If the broker requires client certs, provide both client cert and key

## Authentication failures

**Symptom:** `UNAUTHENTICATED` gRPC status

**Solutions:**
- **Missing API key:** Set `api_key` / `with_api_key()` on your client connection
- **Wrong key:** Verify the key matches what's configured on the broker
- **Key revoked:** Check if the key was deleted. List keys: `fila auth list`
- **Permission denied (PERMISSION_DENIED):** The key exists but lacks permission for the queue. Check ACLs: `fila auth acl list`

## Queue not found

**Symptom:** `NOT_FOUND` gRPC status on enqueue or consume

**Solutions:**
- Queue doesn't exist. Create it: `fila queue create <name>`
- Typo in queue name. List queues: `fila queue list`
- In clustered mode: if you get `UNAVAILABLE` instead of `NOT_FOUND`, the node is still starting up — retry after a moment

## Consumer timeout

**Symptom:** Consumer stream hangs with no messages delivered

**Causes:**
- Queue is empty. Check depth: `fila queue inspect <name>`
- All messages are leased to other consumers. Check pending count in queue stats.
- Visibility timeout expired and messages were re-queued. Increase timeout or process faster.
- Throttle rate limit hit. Check throttle config: `fila config list`

## Message redelivery

**Symptom:** Same message delivered multiple times

**Causes:**
- **Visibility timeout expiry:** If a consumer takes too long to ack, the message is re-delivered. Solution: ack promptly, or increase the visibility timeout.
- **Nack without DLQ:** If on_failure returns `{ action = "retry" }`, the message goes back to the queue. After enough retries, consider dead-lettering.
- **Consumer crash:** Leased messages are re-delivered after visibility timeout. This is by design — at-least-once delivery.

**Best practice:** Make consumers idempotent. Use `msg.id` as a deduplication key.

## Lua script errors

**Symptom:** Warning logs about script failures, circuit breaker tripping

**Solutions:**
- **Syntax error:** Validate before setting: `fila queue create test --on-enqueue 'function on_enqueue(msg) return {} end'`
- **Runtime error:** Check logs for the specific error. Common: accessing nil headers, type mismatches.
- **Timeout:** Script took too long. Default limit is 10ms. Simplify the script or increase `lua.default_timeout_ms`.
- **Memory limit:** Script uses too much memory. Default is 1MB. Check for unbounded table growth.
- **Circuit breaker:** After 3 consecutive failures, scripts are bypassed for 10 seconds. Fix the script to reset the breaker.

## High memory usage

**Solutions:**
- Check queue depths: `fila queue inspect <name>`. Large queues consume memory.
- RocksDB block cache can be tuned via configuration.
- In clustered mode, each node replicates data for its assigned queues.

## Cluster issues

**Symptom:** Node not joining cluster, split-brain, leader election failures

**Solutions:**
- **Node not joining:** Check `cluster.peers` config — all nodes must list each other's cluster addresses (port 5556, not 5555).
- **Bootstrap:** Exactly one node should have `bootstrap = true`. Start it first.
- **Network:** Verify nodes can reach each other on the cluster port: `nc -zv node2 5556`
- **Node ID:** Each node must have a unique `node_id`. Duplicates cause undefined behavior.
