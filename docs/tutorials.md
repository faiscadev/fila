# Tutorials

Step-by-step guides for common Fila use cases. Each tutorial assumes you have a running broker (see [quickstart](../README.md#quickstart)).

## Multi-tenant fair scheduling

**Goal:** Prevent a noisy tenant from starving other tenants in a shared queue.

### 1. Create a queue with tenant-aware fairness

```sh
fila queue create orders \
  --on-enqueue 'function on_enqueue(msg)
    return { fairness_key = msg.headers["tenant_id"] or "default" }
  end'
```

The `on_enqueue` hook extracts a `tenant_id` header and uses it as the fairness key. Each unique tenant gets its own DRR scheduling group.

### 2. Produce messages from multiple tenants

```python
# Python SDK
from fila import FilaClient

client = FilaClient("localhost:5555")

# Noisy tenant sends 1000 messages
for i in range(1000):
    client.enqueue("orders", {"tenant_id": "noisy-corp"}, f"order-{i}")

# Other tenants send a few each
for tenant in ["acme", "globex", "initech"]:
    for i in range(10):
        client.enqueue("orders", {"tenant_id": tenant}, f"{tenant}-order-{i}")
```

### 3. Consume and observe fairness

```python
stream = client.consume("orders")
for msg in stream:
    print(f"tenant={msg.metadata.fairness_key} id={msg.id}")
    client.ack("orders", msg.id)
```

Without Fila, all 1000 noisy-corp messages would be delivered first. With DRR scheduling, each tenant gets interleaved delivery — acme, globex, and initech messages arrive alongside noisy-corp's, not after.

### 4. Verify with stats

```sh
fila queue inspect orders
```

The per-key breakdown shows each tenant's pending count and current DRR deficit.

### Weighted fairness

Give premium tenants more bandwidth by setting weights:

```sh
fila queue create orders \
  --on-enqueue 'function on_enqueue(msg)
    local weights = { premium = 3, standard = 1 }
    local tier = msg.headers["tier"] or "standard"
    return {
      fairness_key = msg.headers["tenant_id"] or "default",
      weight = weights[tier] or 1
    }
  end'
```

A premium tenant with weight=3 gets 3x the delivery bandwidth of a standard tenant with weight=1.

---

## Per-provider throttling

**Goal:** Rate-limit outgoing API calls per external provider without wasting consumer resources.

### 1. Create a queue with throttle keys

```sh
fila queue create api-calls \
  --on-enqueue 'function on_enqueue(msg)
    local keys = {}
    if msg.headers["provider"] then
      table.insert(keys, "provider:" .. msg.headers["provider"])
    end
    return {
      fairness_key = msg.headers["tenant"] or "default",
      throttle_keys = keys
    }
  end'
```

### 2. Set throttle rates

```sh
# Stripe: 100 requests/second, burst up to 150
fila config set throttle.provider:stripe 100,150

# SendGrid: 10 requests/second, burst up to 20
fila config set throttle.provider:sendgrid 10,20
```

The format is `rate,burst`. Rate is tokens per second; burst is the maximum bucket capacity.

### 3. Produce messages

```go
// Go SDK
client, _ := fila.Connect("localhost:5555")

// These will be throttled to 100/s
for i := 0; i < 500; i++ {
    client.Enqueue(ctx, "api-calls", map[string]string{
        "tenant":   "acme",
        "provider": "stripe",
    }, []byte(fmt.Sprintf("charge-%d", i)))
}
```

### 4. Consume — the broker does the throttling

```go
stream, _ := client.Consume(ctx, "api-calls")
for msg := range stream {
    // Every message received is within the rate limit.
    // No need to check limits client-side.
    callExternalAPI(msg.Payload)
    client.Ack(ctx, "api-calls", msg.ID)
}
```

Consumers receive messages at the provider's rate limit. No consumer-side rate checking, no wasted fetches, no re-enqueue loops.

### Adjusting rates at runtime

Change rates without restarting the broker:

```sh
# Double Stripe's rate
fila config set throttle.provider:stripe 200,300
```

The token bucket updates immediately.

---

## Exponential backoff retry

**Goal:** Retry failed messages with increasing delays, then dead-letter after max attempts.

### 1. Create a queue with retry logic

```sh
fila queue create jobs \
  --on-enqueue 'function on_enqueue(msg)
    return { fairness_key = msg.headers["job_type"] or "default" }
  end' \
  --on-failure 'function on_failure(msg)
    local max_attempts = tonumber(fila.get("max_retries") or "5")
    if msg.attempts >= max_attempts then
      return { action = "dlq" }
    end
    -- Exponential backoff: 1s, 2s, 4s, 8s, 16s...
    local delay = math.min(1000 * (2 ^ (msg.attempts - 1)), 60000)
    return { action = "retry", delay_ms = delay }
  end' \
  --visibility-timeout 30000
```

### 2. Configure max retries at runtime

```sh
fila config set max_retries 3
```

The `on_failure` hook reads this with `fila.get("max_retries")`. Change it without redeploying.

### 3. Process messages with failure handling

```javascript
// JavaScript SDK
const { FilaClient } = require('@anthropic/fila');

const client = await FilaClient.connect('localhost:5555');
const stream = client.consume('jobs');

for await (const msg of stream) {
  try {
    await processJob(msg.payload);
    await client.ack('jobs', msg.id);
  } catch (err) {
    // Nack triggers on_failure hook — broker handles retry/DLQ
    await client.nack('jobs', msg.id, err.message);
  }
}
```

### 4. Monitor and redrive

```sh
# Check how many messages ended up in the DLQ
fila queue inspect jobs.dlq

# After fixing the root cause, redrive them
fila redrive jobs.dlq --count 0  # 0 = all messages
```

### Customizing backoff per job type

Read config to customize behavior per job type:

```lua
function on_failure(msg)
  -- Different retry strategies per job type
  local job_type = msg.headers["job_type"] or "default"
  local max = tonumber(fila.get("max_retries:" .. job_type) or "5")

  if msg.attempts >= max then
    return { action = "dlq" }
  end

  -- Critical jobs: shorter delays, more retries
  -- Batch jobs: longer delays, fewer retries
  local base_ms = tonumber(fila.get("retry_base_ms:" .. job_type) or "1000")
  local delay = math.min(base_ms * (2 ^ (msg.attempts - 1)), 300000)
  return { action = "retry", delay_ms = delay }
end
```

```sh
fila config set max_retries:payment 10
fila config set retry_base_ms:payment 500

fila config set max_retries:report 3
fila config set retry_base_ms:report 5000
```
