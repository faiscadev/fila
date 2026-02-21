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

## Provider throttling

Rate-limit outbound calls per external API provider:

```lua
function on_enqueue(msg)
  local keys = {}
  if msg.headers["provider"] then
    table.insert(keys, "provider:" .. msg.headers["provider"])
  end

  return {
    fairness_key = msg.headers["tenant"] or "default",
    throttle_keys = keys
  }
end
```

Set rates: `fila config set throttle.provider:stripe 100,200`

### Multi-dimensional throttling

Throttle by both provider and tenant (composite key):

```lua
function on_enqueue(msg)
  local tenant = msg.headers["tenant"] or "default"
  local provider = msg.headers["provider"]

  local keys = {}
  if provider then
    -- Global provider limit
    table.insert(keys, "provider:" .. provider)
    -- Per-tenant-per-provider limit
    table.insert(keys, "tenant-provider:" .. tenant .. ":" .. provider)
  end

  return {
    fairness_key = tenant,
    throttle_keys = keys
  }
end
```

```sh
# Global: Stripe allows 1000 req/s total
fila config set throttle.provider:stripe 1000,1500

# Per-tenant: each tenant gets at most 100 req/s to Stripe
fila config set throttle.tenant-provider:acme:stripe 100,150
fila config set throttle.tenant-provider:globex:stripe 100,150
```

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
    fairness_key = "region:" .. region,
    throttle_keys = { "region:" .. region }
  }
end
```

```sh
# Rate limit per region
fila config set throttle.region:us-east 500,750
fila config set throttle.region:eu-west 300,450
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
