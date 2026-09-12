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

**Goal:** Keep calls to an external API within its rate limit, without the worker
fetching jobs it can't perform yet.

### 1. Create a queue

```sh
fila queue create charges \
  --on-enqueue 'function on_enqueue(msg)
    return { fairness_key = msg.headers["tenant"] or "default" }
  end'
```

The queue knows nothing about Stripe. Rate limits belong to the worker that calls it.

### 2. Produce messages

```rust
let producer = client.producer();

let charges: Vec<Message> = (0..500)
    .map(|i| {
        Message::new("charges", format!("charge-{i}"))
            .header("tenant", "acme")
            .header("customer", format!("cus_{}", i % 20))
    })
    .collect();

producer.send_batch(charges).await?;
```

Producers don't mention Stripe or any limit.

### 3. Consume, declaring the limits you're bound by

```rust
let mut charges = client
    .consumer()
    .subscribe("charges")
    // Stripe: 100 requests/second, burst up to 150
    .throttle(Throttle::named("stripe").rate(100, Duration::from_secs(1)).burst(150))
    // and no customer above 10/s
    .throttle(
        Throttle::named("stripe-per-customer")
            .partition_by_header("customer")
            .rate(10, Duration::from_secs(1)),
    )
    .await?;

while let Some(delivery) = charges.next().await {
    let delivery = delivery?;
    // Already within both limits. No client-side rate checking, no re-enqueue loop.
    stripe.charge(delivery.payload()).await?;
    delivery.ack().await?;
}
```

A message is delivered only when both `stripe` and its customer's
`stripe-per-customer` bucket have a token. Until then it stays in the broker — no lease,
no attempt counted.

### 4. Share the limit with another service

A refunds worker on a different queue also calls Stripe. It declares the same name:

```rust
let mut refunds = client
    .consumer()
    .subscribe("refunds")
    .throttle(Throttle::named("stripe").rate(100, Duration::from_secs(1)).burst(150))
    .await?;
```

Charges and refunds together stay within 100/s, on one node or across a cluster.

### Changing a rate

A rate lives in the worker's code, so changing it is a deploy. When declarations with
the same name disagree, the strictest wins:

- **Lowering** a rate takes effect as soon as the first updated worker subscribes.
- **Raising** a rate takes effect once no worker declares the old, lower one.

Both directions are safe during a rolling deploy: the limit never rises above what some
running worker asked for.

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
