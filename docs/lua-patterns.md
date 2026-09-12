# Lua Hook Patterns

Copy-paste patterns for common scheduling scenarios. See [concepts](concepts.md#lua-hooks) for hook API details.

## Tenant fairness

Assign each tenant its own fairness group so the DRR scheduler gives equal delivery bandwidth:

```lua
function on_enqueue(msg)
  return {
    fairness_key = msg.headers["tenant_id"] or "default"
  }
end
```

### With weighted tiers

Premium tenants get more bandwidth:

```lua
function on_enqueue(msg)
  local tier = msg.headers["tier"] or "standard"
  local weight = 1
  if tier == "premium" then weight = 3 end
  if tier == "enterprise" then weight = 5 end

  return {
    fairness_key = msg.headers["tenant_id"] or "default",
    weight = weight
  }
end
```

### With dynamic weights from config

```lua
function on_enqueue(msg)
  local tenant = msg.headers["tenant_id"] or "default"
  local weight = tonumber(fila.get("weight:" .. tenant) or "1")

  return {
    fairness_key = tenant,
    weight = weight
  }
end
```

Set weights at runtime: `fila config set weight:acme 5`

---

## Throttle partitions

Throttles are declared by consumers (see [throttling.md](throttling.md)). Lua's role is
computing the values a throttle partitions by, in the queue's `attributes` hook, when
they cannot simply be read from a header or the fairness key.

`attributes` runs the first time the scheduler considers a message, and always
reflects the current script.

### Derived customer account

Several customer IDs map to one billing account, and the downstream limit is per
account:

```lua
function attributes(msg)
  local customer = msg.headers["customer"]
  if not customer then
    return {}   -- attribute absent: the throttle's missing-value policy applies
  end
  return { account = fila.get("account:" .. customer) or customer }
end
```

```rust
consumer
    .subscribe("charges")
    .throttle(
        Throttle::named("stripe-per-account")
            .partition_by_attribute("account")
            .rate(10, Duration::from_secs(1)),
    )
    .await?;
```

### Region from a composite header

```lua
function attributes(msg)
  -- "eu-west-1:acme" -> "eu-west-1"
  local target = msg.headers["target"] or ""
  return { region = target:match("^([^:]+)") }
end
```

A nil value means the attribute is absent, and the throttle's missing-value policy
applies.

---

## Exponential backoff retry

Retry with increasing delays, dead-letter after max attempts:

```lua
function on_failure(msg)
  if msg.attempts >= 5 then
    return { action = "dlq" }
  end

  -- 1s, 2s, 4s, 8s, 16s
  local delay = math.min(1000 * (2 ^ (msg.attempts - 1)), 60000)
  return { action = "retry", delay_ms = delay }
end
```

### With configurable max retries

```lua
function on_failure(msg)
  local max = tonumber(fila.get("max_retries") or "5")
  if msg.attempts >= max then
    return { action = "dlq" }
  end

  local delay = math.min(1000 * (2 ^ (msg.attempts - 1)), 60000)
  return { action = "retry", delay_ms = delay }
end
```

Change at runtime: `fila config set max_retries 10`

### Linear backoff

```lua
function on_failure(msg)
  if msg.attempts >= 5 then
    return { action = "dlq" }
  end

  -- 5s, 10s, 15s, 20s, 25s
  return { action = "retry", delay_ms = 5000 * msg.attempts }
end
```

### Immediate retry (no delay)

```lua
function on_failure(msg)
  if msg.attempts >= 3 then
    return { action = "dlq" }
  end
  return { action = "retry", delay_ms = 0 }
end
```

---

## Header-based routing

Use headers to make dynamic scheduling decisions.

### Route by priority

```lua
function on_enqueue(msg)
  local priority = msg.headers["priority"] or "normal"
  local weights = {
    critical = 10,
    high = 5,
    normal = 2,
    low = 1
  }

  return {
    fairness_key = "priority:" .. priority,
    weight = weights[priority] or 2
  }
end
```

### Route by region

```lua
function on_enqueue(msg)
  local region = msg.headers["region"] or "default"

  return {
    fairness_key = "region:" .. region
  }
end
```

### Conditional dead-letter by error type

```lua
function on_failure(msg)
  -- Permanent errors: dead-letter immediately
  if msg.error:find("4%d%d") then  -- HTTP 4xx
    return { action = "dlq" }
  end

  -- Transient errors: retry with backoff
  if msg.attempts >= 5 then
    return { action = "dlq" }
  end

  local delay = 1000 * (2 ^ (msg.attempts - 1))
  return { action = "retry", delay_ms = delay }
end
```

### Feature flag gating

```lua
function on_enqueue(msg)
  local tenant = msg.headers["tenant"] or "default"
  local new_flow = fila.get("feature:new_flow:" .. tenant)

  if new_flow == "enabled" then
    return { fairness_key = tenant .. ":v2", weight = 1 }
  end

  return { fairness_key = tenant, weight = 1 }
end
```

```sh
# Enable new flow for one tenant
fila config set feature:new_flow:acme enabled
```
