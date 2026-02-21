# SDK Examples

Working code for every SDK showing the core enqueue -> consume -> ack flow.

## Setup

All examples assume a running broker with a queue:

```sh
fila-server &
fila queue create demo
```

## Rust

```rust
use std::collections::HashMap;
use std::time::Duration;

use fila_sdk::FilaClient;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FilaClient::connect("http://localhost:5555").await?;

    // Enqueue
    let mut headers = HashMap::new();
    headers.insert("tenant".to_string(), "acme".to_string());
    let id = client.enqueue("demo", headers, b"hello".to_vec()).await?;
    println!("enqueued: {id}");

    // Consume
    let mut stream = client.consume("demo").await?;
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await??
        .expect("stream error");

    println!("received: {} ({})", msg.id, String::from_utf8_lossy(&msg.payload));

    // Ack
    client.ack("demo", &msg.id).await?;
    println!("acked: {}", msg.id);

    Ok(())
}
```

Add to `Cargo.toml`:
```toml
[dependencies]
fila-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
```

## Go

```go
package main

import (
	"context"
	"fmt"
	"log"

	fila "github.com/faiscadev/fila-go"
)

func main() {
	ctx := context.Background()
	client, err := fila.Connect("localhost:5555")
	if err != nil {
		log.Fatal(err)
	}
	defer client.Close()

	// Enqueue
	id, err := client.Enqueue(ctx, "demo", map[string]string{
		"tenant": "acme",
	}, []byte("hello"))
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("enqueued:", id)

	// Consume
	stream, err := client.Consume(ctx, "demo")
	if err != nil {
		log.Fatal(err)
	}

	msg, err := stream.Next()
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("received: %s (%s)\n", msg.ID, string(msg.Payload))

	// Ack
	if err := client.Ack(ctx, "demo", msg.ID); err != nil {
		log.Fatal(err)
	}
	fmt.Println("acked:", msg.ID)
}
```

## Python

```python
from fila import FilaClient

client = FilaClient("localhost:5555")

# Enqueue
msg_id = client.enqueue("demo", {"tenant": "acme"}, b"hello")
print(f"enqueued: {msg_id}")

# Consume
stream = client.consume("demo")
msg = next(stream)
print(f"received: {msg.id} ({msg.payload.decode()})")

# Ack
client.ack("demo", msg.id)
print(f"acked: {msg.id}")
```

## JavaScript / Node.js

```javascript
const { FilaClient } = require('@anthropic/fila');

async function main() {
  const client = await FilaClient.connect('localhost:5555');

  // Enqueue
  const id = await client.enqueue('demo', { tenant: 'acme' }, Buffer.from('hello'));
  console.log('enqueued:', id);

  // Consume
  const stream = client.consume('demo');
  for await (const msg of stream) {
    console.log(`received: ${msg.id} (${msg.payload.toString()})`);

    // Ack
    await client.ack('demo', msg.id);
    console.log('acked:', msg.id);
    break; // just one message for the demo
  }
}

main().catch(console.error);
```

## Ruby

```ruby
require 'fila'

client = Fila::Client.new('localhost:5555')

# Enqueue
id = client.enqueue('demo', { 'tenant' => 'acme' }, 'hello')
puts "enqueued: #{id}"

# Consume
client.consume('demo') do |msg|
  puts "received: #{msg.id} (#{msg.payload})"

  # Ack
  client.ack('demo', msg.id)
  puts "acked: #{msg.id}"
  break
end
```

## Java

```java
import dev.fila.client.FilaClient;
import dev.fila.client.Message;

public class Demo {
    public static void main(String[] args) throws Exception {
        FilaClient client = FilaClient.connect("localhost:5555");

        // Enqueue
        String id = client.enqueue("demo",
            java.util.Map.of("tenant", "acme"),
            "hello".getBytes());
        System.out.println("enqueued: " + id);

        // Consume
        var stream = client.consume("demo");
        Message msg = stream.next();
        System.out.printf("received: %s (%s)%n", msg.getId(),
            new String(msg.getPayload()));

        // Ack
        client.ack("demo", msg.getId());
        System.out.println("acked: " + msg.getId());

        client.close();
    }
}
```
