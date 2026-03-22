# Integration Patterns

Common messaging patterns with Fila.

## Producer / Consumer

The most basic pattern: one service produces messages, another consumes them.

```
┌──────────┐    enqueue    ┌──────┐    consume    ┌──────────┐
│ Producer │ ────────────► │ Fila │ ────────────► │ Consumer │
└──────────┘               └──────┘               └──────────┘
```

**Use when:** You want to decouple a producer from a consumer — the producer doesn't need to wait for processing to complete.

```python
# producer.py
import asyncio
from fila import FilaClient

async def produce():
    client = await FilaClient.connect("localhost:5555")
    for i in range(100):
        await client.enqueue("orders",
            headers={"tenant": "acme"},
            payload=f"order-{i}".encode())

asyncio.run(produce())
```

```python
# consumer.py
import asyncio
from fila import FilaClient

async def consume():
    client = await FilaClient.connect("localhost:5555")
    async for msg in client.consume("orders"):
        print(f"processing: {msg.payload}")
        # ... do work ...
        await client.ack("orders", msg.id)

asyncio.run(consume())
```

## Fan-out

Multiple consumers process different types of work from a shared queue. Fila's fairness keys ensure each workload type gets its fair share.

```
                              ┌──────────────┐
                         ┌──► │ Consumer (A)  │
┌──────────┐  enqueue   │    └──────────────┘
│ Producer │ ──────► ┌──┴──┐
└──────────┘         │ Fila │  fairness_key routing
                     └──┬──┘
                         └──► ┌──────────────┐
                              │ Consumer (B)  │
                              └──────────────┘
```

**Use when:** Different message types need fair scheduling. Without fairness keys, a burst of type-A messages would starve type-B consumers.

```python
# Producer assigns fairness keys via Lua on_enqueue
# Queue created with:
# fila queue create work --on-enqueue '
#   function on_enqueue(msg)
#     return { fairness_key = msg.headers["type"] or "default" }
#   end
# '

# producer.py
async def produce():
    client = await FilaClient.connect("localhost:5555")
    # Type A messages (high volume)
    for i in range(1000):
        await client.enqueue("work",
            headers={"type": "email"}, payload=f"email-{i}".encode())
    # Type B messages (low volume but equally important)
    for i in range(10):
        await client.enqueue("work",
            headers={"type": "sms"}, payload=f"sms-{i}".encode())
    # Fila's DRR scheduler ensures SMS messages aren't starved by emails
```

## Request-Reply

Implement synchronous-style request/reply over async messaging using a correlation ID and a reply queue.

```
┌─────────┐  enqueue(request)  ┌──────┐  consume   ┌─────────┐
│ Client  │ ─────────────────► │ Fila │ ────────► │ Service │
│         │ ◄───────────────── │      │ ◄──────── │         │
└─────────┘  consume(reply)    └──────┘  enqueue   └─────────┘
                                         (reply)
```

**Use when:** A service needs a response but you still want the decoupling and reliability benefits of message queues.

```go
// client.go — sends request, waits for reply
package main

import (
    "context"
    "fmt"
    "log"

    "github.com/google/uuid"
    fila "github.com/faiscadev/fila-go"
)

func main() {
    client, _ := fila.Connect("localhost:5555")
    ctx := context.Background()

    correlationID := uuid.New().String()
    replyQueue := "replies-" + correlationID

    // Create a temporary reply queue
    client.CreateQueue(ctx, replyQueue)
    defer client.DeleteQueue(ctx, replyQueue)

    // Send request with correlation ID
    client.Enqueue(ctx, "requests",
        map[string]string{
            "correlation_id": correlationID,
            "reply_to":       replyQueue,
        },
        []byte("what is 2+2?"))

    // Wait for reply
    stream, _ := client.Consume(ctx, replyQueue)
    reply, _ := stream.Recv()
    fmt.Printf("reply: %s\n", reply.Payload)
    client.Ack(ctx, replyQueue, reply.ID)
}
```

```go
// service.go — processes requests, sends replies
package main

import (
    "context"
    "fmt"
    "log"

    fila "github.com/faiscadev/fila-go"
)

func main() {
    client, _ := fila.Connect("localhost:5555")
    ctx := context.Background()

    stream, _ := client.Consume(ctx, "requests")
    for {
        msg, err := stream.Recv()
        if err != nil {
            log.Fatal(err)
        }

        // Process request
        answer := fmt.Sprintf("answer: 4 (to: %s)", string(msg.Payload))

        // Send reply to the caller's reply queue
        replyTo := msg.Headers["reply_to"]
        client.Enqueue(ctx, replyTo,
            map[string]string{
                "correlation_id": msg.Headers["correlation_id"],
            },
            []byte(answer))

        client.Ack(ctx, "requests", msg.ID)
    }
}
```
