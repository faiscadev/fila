//! Per-key throttling example.
//!
//! Demonstrates how Fila enforces rate limits at the broker level. Messages
//! assigned to a throttled key are held until tokens are available — consumers
//! only receive messages they can actually process.
//!
//! ## Prerequisites
//!
//! Start a fila-server and set up the queue:
//!
//! ```sh
//! fila-server &
//!
//! # Create a queue with a Lua hook that extracts throttle keys
//! fila queue create throttle-demo \
//!   --on-enqueue 'function on_enqueue(msg)
//!     local keys = {}
//!     if msg.headers["provider"] then
//!       table.insert(keys, "provider:" .. msg.headers["provider"])
//!     end
//!     return {
//!       fairness_key = msg.headers["tenant"] or "default",
//!       throttle_keys = keys
//!     }
//!   end'
//!
//! # Set rate limit: 2 messages/second for the "slow-api" provider
//! fila config set throttle.provider:slow-api 2,5
//! ```
//!
//! Then run:
//!
//! ```sh
//! cargo run --example throttling
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

use fila_sdk::FilaClient;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FilaClient::connect("http://localhost:5555").await?;
    let queue = "throttle-demo";

    // Enqueue messages for the throttled provider
    println!("Enqueuing 10 messages for throttled provider 'slow-api'...");
    for i in 0..10 {
        let mut headers = HashMap::new();
        headers.insert("tenant".to_string(), "acme".to_string());
        headers.insert("provider".to_string(), "slow-api".to_string());
        client
            .enqueue(queue, headers, format!("throttled-{i}").into_bytes())
            .await?;
    }

    // Enqueue messages for an unthrottled path
    println!("Enqueuing 5 messages with no throttle key...");
    for i in 0..5 {
        let mut headers = HashMap::new();
        headers.insert("tenant".to_string(), "acme".to_string());
        // No "provider" header = no throttle keys
        client
            .enqueue(queue, headers, format!("fast-{i}").into_bytes())
            .await?;
    }

    // Consume and observe throttling
    println!("\nConsuming messages:\n");
    let mut stream = client.consume(queue).await?;

    let start = Instant::now();
    for i in 0..10 {
        let msg = tokio::time::timeout(Duration::from_secs(15), stream.next())
            .await?
            .expect("stream ended")
            .expect("stream error");

        let payload = String::from_utf8_lossy(&msg.payload);
        let elapsed = start.elapsed();
        println!(
            "  [{:>2}] +{:.1}s  key={:<8} payload={}",
            i + 1,
            elapsed.as_secs_f64(),
            msg.fairness_key,
            payload
        );

        client.ack(queue, &msg.id).await?;
    }

    println!("\nUnthrottled messages arrive immediately.");
    println!("Throttled messages are rate-limited by the broker — consumers never see excess.");

    Ok(())
}
