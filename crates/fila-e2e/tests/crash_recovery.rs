mod helpers;

use std::collections::HashMap;
use std::time::Duration;

use tokio_stream::StreamExt;

/// AC 5j: Crash recovery — enqueue → kill → restart → verify messages available.
#[tokio::test]
async fn e2e_crash_recovery() {
    let server = helpers::TestServer::start();
    helpers::create_queue_cli(server.addr(), "recovery");

    let client = helpers::sdk_client(server.addr()).await;

    // Enqueue several messages
    let mut msg_ids = Vec::new();
    for i in 0..5 {
        let id = client
            .enqueue("recovery", HashMap::new(), format!("msg-{i}").into_bytes())
            .await
            .unwrap();
        msg_ids.push(id);
    }

    // Kill the server (simulates crash — SIGKILL, not graceful)
    let (data_dir, port) = server.kill_and_take_data();

    // Restart on the same data directory and port
    let server2 = helpers::TestServer::restart_on(data_dir, port);
    let client2 = helpers::sdk_client(server2.addr()).await;

    // Consume from the queue — all messages should be available
    let mut stream = client2.consume("recovery").await.unwrap();
    let mut recovered_ids = Vec::new();

    for _ in 0..5 {
        let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("timeout waiting for recovered message")
            .expect("stream ended")
            .expect("consume error");
        recovered_ids.push(msg.id.clone());
        client2.ack("recovery", &msg.id).await.unwrap();
    }

    // Verify all original messages were recovered
    for id in &msg_ids {
        assert!(
            recovered_ids.contains(id),
            "message {id} was not recovered after crash"
        );
    }
}
