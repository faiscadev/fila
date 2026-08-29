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

```rust
let client = FilaClient::connect("localhost:5555").await?;
let producer = client.producer();

// Noisy tenant sends 1000 messages
for i in 0..1000 {
    producer.send(
        Message::new("orders", format!("order-{i}"))
            .header("tenant_id", "noisy-corp")
    ).await?;
}

// Other tenants send a few each
for tenant in ["acme", "globex", "initech"] {
    for i in 0..10 {
        producer.send(
            Message::new("orders", format!("{tenant}-order-{i}"))
                .header("tenant_id", tenant)
        ).await?;
    }
}
```

### 3. Consume and observe fairness

```rust
let mut orders = client.consumer().subscribe("orders").await?;

while let Some(delivery) = orders.next().await {
    let delivery = delivery?;
    println!("tenant={} id={}", delivery.fairness_key(), delivery.id());
    delivery.ack().await?;
}
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

```rust
let producer = client.producer();

// These will be throttled to 100/s
let charges: Vec<Message> = (0..500)
    .map(|i| {
        Message::new("api-calls", format!("charge-{i}"))
            .header("tenant", "acme")
            .header("provider", "stripe")
    })
    .collect();

producer.send_batch(charges).await?;
```

### 4. Consume — the broker does the throttling

```rust
let mut calls = client.consumer().subscribe("api-calls").await?;

while let Some(delivery) = calls.next().await {
    let delivery = delivery?;
    // Every message received is already within the rate limit.
    // No client-side limit checking, no re-enqueue loop.
    call_external_api(delivery.payload()).await?;
    delivery.ack().await?;
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

```rust
let mut jobs = client.consumer().subscribe("jobs").await?;

while let Some(delivery) = jobs.next().await {
    let delivery = delivery?;

    match process_job(delivery.payload()).await {
        Ok(()) => delivery.ack().await?,
        // Nack runs the on_failure hook — the broker decides retry vs. DLQ
        Err(e) => delivery.nack(&e.to_string()).await?,
    }
}
```

If the client already knows how long to wait, it can say so directly instead of
deferring to the hook's delay decision:

```rust
Err(e) if e.is_rate_limited() => {
    delivery.retry_after(e.retry_after()).await?;
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
