//! Multi-tenant fair scheduling example.
//!
//! Demonstrates how Fila's DRR scheduler prevents a noisy tenant from starving
//! a quiet tenant. Both tenants share the same queue, but each gets its fair
//! share of delivery bandwidth.
//!
//! ## Prerequisites
//!
//! Start a fila-server and create a queue with a Lua hook:
//!
//! ```sh
//! fila-server &
//! fila queue create fair-demo \
//!   --on-enqueue 'function on_enqueue(msg)
//!     return { fairness_key = msg.headers["tenant"] or "default" }
//!   end'
//! ```
//!
//! Then run this example:
//!
//! ```sh
//! cargo run --example fair_scheduling
//! ```

use std::collections::HashMap;
use std::time::Duration;

use fila_sdk::FilaClient;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FilaClient::connect("http://localhost:5555").await?;
    let queue = "fair-demo";

    // Noisy tenant: 50 messages
    println!("Enqueuing 50 messages from tenant 'noisy'...");
    for i in 0..50 {
        let mut headers = HashMap::new();
        headers.insert("tenant".to_string(), "noisy".to_string());
        client
            .enqueue(queue, headers, format!("noisy-{i}").into_bytes())
            .await?;
    }

    // Quiet tenant: 5 messages
    println!("Enqueuing 5 messages from tenant 'quiet'...");
    for i in 0..5 {
        let mut headers = HashMap::new();
        headers.insert("tenant".to_string(), "quiet".to_string());
        client
            .enqueue(queue, headers, format!("quiet-{i}").into_bytes())
            .await?;
    }

    // Consume and observe fairness
    println!("\nConsuming messages (watch the interleaving):\n");
    let mut stream = client.consume(queue).await?;

    let mut noisy_count = 0u32;
    let mut quiet_count = 0u32;

    for i in 0..20 {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await?
            .expect("stream ended")
            .expect("stream error");

        let payload = String::from_utf8_lossy(&msg.payload);
        println!(
            "  [{:>2}] tenant={:<6} payload={}",
            i + 1,
            msg.fairness_key,
            payload
        );

        match msg.fairness_key.as_str() {
            "noisy" => noisy_count += 1,
            "quiet" => quiet_count += 1,
            _ => {}
        }

        client.ack(queue, &msg.id).await?;
    }

    println!("\nFirst 20 messages delivered:");
    println!("  noisy: {noisy_count}");
    println!("  quiet: {quiet_count}");
    println!("\nWithout fair scheduling, all 20 would be from 'noisy'.");
    println!("With DRR, 'quiet' gets its fair share despite being outnumbered 10:1.");

    Ok(())
}
