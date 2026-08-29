# SDK Examples

Working code for the Rust SDK. The signature-level reference is rustdoc, generated
from the source; this page is the worked examples. For the wire format underneath,
see [protocol.md](protocol.md).

> **Status:** design, not shipped code. Fila ships one client, in Rust.

## Setup

```toml
[dependencies]
fila-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
```

All examples assume a running broker:

```sh
fila-server &
fila queue create demo
```

## The core flow

Enqueue, consume, ack.

```rust
use std::time::Duration;

use fila_sdk::FilaClient;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FilaClient::connect("localhost:5555").await?;

    // Produce
    let id = client.producer().enqueue("demo", b"hello").await?;
    println!("enqueued: {id}");

    // Consume
    let consumer = client.consumer();
    let mut demo = consumer.subscribe("demo").await?;

    let delivery = tokio::time::timeout(Duration::from_secs(5), demo.next())
        .await?
        .expect("stream ended")?;

    println!(
        "received {} attempt {}: {}",
        delivery.id(),
        delivery.attempt(),
        String::from_utf8_lossy(delivery.payload())
    );

    delivery.ack().await?;
    Ok(())
}
```

## Producing with scheduling metadata

Fairness key, weight and throttle keys are values, not something you must reach
through a Lua script to set.

```rust
use fila_sdk::Message;

let producer = client.producer();

producer.send(
    Message::new("orders", payload)
        .header("tenant", "acme")
        .fairness_key("acme")
        .weight(3)
        .throttle_key("provider:stripe")
).await?;
```

Delayed delivery:

```rust
producer.send(
    Message::new("orders", payload).delay(Duration::from_secs(30))
).await?;
```

## Batching

The protocol is batch-native. One round trip, one scheduler pass.

```rust
let messages: Vec<Message> = orders
    .iter()
    .map(|o| Message::new("orders", o.encode()).fairness_key(&o.tenant))
    .collect();

for result in producer.send_batch(messages).await? {
    match result {
        Ok(id) => tracing::debug!(%id, "enqueued"),
        Err(e) => tracing::warn!(error = %e, "message rejected"),
    }
}
```

A batch can partially succeed — the outer `Result` fails only if nothing was sent.

## A worker loop

The realistic shape: long-running work, lease extension, explicit failure handling.

```rust
let consumer = client.consumer();
let mut jobs = consumer.subscribe("jobs").await?;

while let Some(delivery) = jobs.next().await {
    let delivery = delivery?;

    match process(delivery.payload()).await {
        Ok(()) => {
            delivery.ack().await?;
        }
        Err(e) if e.is_transient() => {
            // Back off rather than hot-looping the retry
            let backoff = Duration::from_secs(2u64.pow(delivery.attempt()));
            delivery.retry_after(backoff).await?;
        }
        Err(e) => {
            // on_failure decides whether this dead-letters
            delivery.nack(&e.to_string()).await?;
        }
    }
}
```

For work that outlives the queue's visibility timeout, extend the lease instead of
letting it expire and be redelivered:

```rust
let delivery = /* ... */;
let extending = delivery.clone();

let heartbeat = tokio::spawn(async move {
    let mut tick = tokio::time::interval(Duration::from_secs(20));
    loop {
        tick.tick().await;
        if extending.extend_lease(Duration::from_secs(30)).await.is_err() {
            break;
        }
    }
});

let outcome = long_running_work(delivery.payload()).await;
heartbeat.abort();

match outcome {
    Ok(()) => delivery.ack().await?,
    Err(e) => delivery.nack(&e.to_string()).await?,
}
```

## Consuming several queues

One connection, concurrent subscriptions.

```rust
let consumer = client.consumer();

let mut orders  = consumer.subscribe("orders").await?;
let mut billing = consumer.subscribe("billing").await?;

loop {
    tokio::select! {
        Some(d) = orders.next()  => { let d = d?; handle_order(&d).await?;  d.ack().await?; }
        Some(d) = billing.next() => { let d = d?; handle_billing(&d).await?; d.ack().await?; }
        else => break,
    }
}
```

## Handling errors by kind

Per-operation error types mean the compiler shows you what can actually go wrong.

```rust
use fila_sdk::{EnqueueError, StatusError};

match producer.enqueue("orders", payload).await {
    Ok(id) => Ok(id),

    // Domain failure — the queue does not exist
    Err(EnqueueError::QueueNotFound(q)) => {
        admin.create_queue(QueueSpec::new(&q)).await?;
        producer.enqueue(&q, payload).await
    }

    // Retriable — not leader, node starting, scheduler saturated
    Err(EnqueueError::Status(StatusError::Unavailable(_))) => {
        backoff_and_retry().await
    }

    // Not retriable — the ACL denies this
    Err(EnqueueError::Status(StatusError::Forbidden(msg))) => {
        Err(FatalError::Permissions(msg))
    }

    Err(e) => Err(e.into()),
}
```

## Authentication and TLS

```rust
use fila_sdk::{ConnectOptions, FilaClient};

let client = FilaClient::connect_with(
    ConnectOptions::new("fila.internal:5555")
        .api_key(std::env::var("FILA_API_KEY")?)
        .tls_ca_cert(std::fs::read("ca.pem")?)
).await?;
```

Mutual TLS:

```rust
let client = FilaClient::connect_with(
    ConnectOptions::new("fila.internal:5555")
        .tls_ca_cert(std::fs::read("ca.pem")?)
        .tls_identity(
            std::fs::read("client.pem")?,
            std::fs::read("client-key.pem")?,
        )
).await?;
```

## Administration

```rust
use fila_sdk::{ApiKeySpec, Permission, QueueSpec};

let admin = client.admin();

admin.create_queue(
    QueueSpec::new("orders")
        .visibility_timeout(Duration::from_secs(30))
        .on_enqueue(ON_ENQUEUE)
).await?;

// Throttle rates live in runtime config, keyed by throttle key
admin.set_config("throttle.provider:stripe", "100,150").await?;

// Mint a narrowly-scoped key for a producer service
let key = admin.create_api_key(ApiKeySpec::new("checkout-svc")).await?;
admin.set_acl(&key.key_id, &[Permission::produce("orders.*")]).await?;
// key.secret is shown exactly once

let stats = admin.queue_stats("orders").await?;
for k in &stats.per_key_stats {
    println!("{}: {} pending, deficit {}", k.key, k.pending_count, k.current_deficit);
}
```
