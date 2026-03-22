# SDK Quick Start

Get up and running with Fila in your language of choice. Each section covers installation, connection, and the core enqueue/consume/ack flow.

## Prerequisites

```sh
# Start a Fila broker
fila-server &

# Create a queue
fila queue create demo
```

## Rust

```sh
cargo add fila-sdk tokio tokio-stream
```

```rust
use fila_sdk::FilaClient;
use std::collections::HashMap;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FilaClient::connect("http://localhost:5555").await?;

    // Enqueue a message
    let mut headers = HashMap::new();
    headers.insert("tenant".to_string(), "acme".to_string());
    let id = client.enqueue("demo", headers, b"hello".to_vec()).await?;
    println!("enqueued: {id}");

    // Consume messages
    let mut stream = client.consume("demo").await?;
    if let Some(msg) = stream.next().await {
        let msg = msg?;
        println!("received: {}", String::from_utf8_lossy(&msg.payload));

        // Acknowledge
        client.ack("demo", &msg.id).await?;
    }

    Ok(())
}
```

## Go

```sh
go get github.com/faiscadev/fila-go
```

```go
package main

import (
    "context"
    "fmt"
    "log"

    fila "github.com/faiscadev/fila-go"
)

func main() {
    client, err := fila.Connect("localhost:5555")
    if err != nil {
        log.Fatal(err)
    }
    defer client.Close()

    ctx := context.Background()

    // Enqueue
    id, err := client.Enqueue(ctx, "demo",
        map[string]string{"tenant": "acme"}, []byte("hello"))
    if err != nil {
        log.Fatal(err)
    }
    fmt.Println("enqueued:", id)

    // Consume
    stream, err := client.Consume(ctx, "demo")
    if err != nil {
        log.Fatal(err)
    }

    msg, err := stream.Recv()
    if err != nil {
        log.Fatal(err)
    }
    fmt.Printf("received: %s\n", msg.Payload)

    // Ack
    if err := client.Ack(ctx, "demo", msg.ID); err != nil {
        log.Fatal(err)
    }
}
```

## Python

```sh
pip install fila
```

```python
import asyncio
from fila import FilaClient

async def main():
    client = await FilaClient.connect("localhost:5555")

    # Enqueue
    msg_id = await client.enqueue("demo",
        headers={"tenant": "acme"}, payload=b"hello")
    print(f"enqueued: {msg_id}")

    # Consume
    async for msg in client.consume("demo"):
        print(f"received: {msg.payload}")
        await client.ack("demo", msg.id)
        break

asyncio.run(main())
```

## JavaScript / Node.js

```sh
npm install fila-client
```

```javascript
const { FilaClient } = require('fila-client');

async function main() {
    const client = await FilaClient.connect('localhost:5555');

    // Enqueue
    const id = await client.enqueue('demo',
        { tenant: 'acme' }, Buffer.from('hello'));
    console.log('enqueued:', id);

    // Consume
    const stream = client.consume('demo');
    for await (const msg of stream) {
        console.log('received:', msg.payload.toString());
        await client.ack('demo', msg.id);
        break;
    }
}

main().catch(console.error);
```

## Ruby

```sh
gem install fila-client
```

```ruby
require 'fila'

client = Fila::Client.new('localhost:5555')

# Enqueue
id = client.enqueue('demo',
  headers: { 'tenant' => 'acme' }, payload: 'hello')
puts "enqueued: #{id}"

# Consume
client.consume('demo') do |msg|
  puts "received: #{msg.payload}"
  client.ack('demo', msg.id)
  break
end
```

## Java

```xml
<dependency>
    <groupId>dev.faisca</groupId>
    <artifactId>fila-client</artifactId>
    <version>0.1.0</version>
</dependency>
```

```java
import dev.faisca.fila.FilaClient;
import java.util.Map;

public class Demo {
    public static void main(String[] args) throws Exception {
        var client = FilaClient.connect("localhost:5555");

        // Enqueue
        var id = client.enqueue("demo",
            Map.of("tenant", "acme"), "hello".getBytes());
        System.out.println("enqueued: " + id);

        // Consume
        var stream = client.consume("demo");
        var msg = stream.next();
        System.out.println("received: " + new String(msg.getPayload()));

        // Ack
        client.ack("demo", msg.getId());
    }
}
```

---

## Security

### TLS

Connect to a TLS-enabled broker by specifying the CA certificate or using system trust store.

| SDK | TLS Connection |
|-----|---------------|
| Rust | `FilaClient::builder("https://host:5555").with_tls().connect().await?` |
| Go | `fila.Connect("host:5555", fila.WithTLS())` |
| Python | `FilaClient.connect("host:5555", tls=True)` |
| JavaScript | `FilaClient.connect('host:5555', { tls: true })` |
| Ruby | `Fila::Client.new('host:5555', tls: true)` |
| Java | `FilaClient.connect("host:5555", FilaClient.withTLS())` |

For custom CA certificates:

| SDK | Custom CA |
|-----|-----------|
| Rust | `.with_tls_ca("path/to/ca.crt")` |
| Go | `fila.WithTLSCA("path/to/ca.crt")` |
| Python | `tls_ca="path/to/ca.crt"` |
| JavaScript | `{ tlsCa: 'path/to/ca.crt' }` |
| Ruby | `tls_ca: 'path/to/ca.crt'` |
| Java | `FilaClient.withTLSCA("path/to/ca.crt")` |

### API Key Authentication

| SDK | API Key |
|-----|---------|
| Rust | `.with_api_key("your-key")` |
| Go | `fila.WithAPIKey("your-key")` |
| Python | `api_key="your-key"` |
| JavaScript | `{ apiKey: 'your-key' }` |
| Ruby | `api_key: 'your-key'` |
| Java | `FilaClient.withAPIKey("your-key")` |
