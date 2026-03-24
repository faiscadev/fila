//! Competitive benchmark suite: Fila vs Kafka vs RabbitMQ vs NATS.
//!
//! Runs identical workloads against each broker using native Rust clients,
//! producing JSON reports compatible with BenchReport schema.
//!
//! Latency benchmarks use concurrent produce/consume with embedded timestamps.
//! Throughput and latency durations are configurable via `FILA_BENCH_DURATION_SECS`.
//!
//! Usage:
//!   bench-competitive fila <output-dir>
//!   bench-competitive kafka <output-dir>
//!   bench-competitive rabbitmq <output-dir>
//!   bench-competitive nats <output-dir>
//!   bench-competitive all <output-dir>

use fila_bench::measurement::{LatencyHistogram, ThroughputMeter};
use fila_bench::report::{BenchReport, BenchResult};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WARMUP_SECS: u64 = 5;
const LIFECYCLE_MESSAGES: usize = 1000;
const MULTI_PRODUCERS: usize = 3;

const PAYLOAD_64B: usize = 64;
const PAYLOAD_1KB: usize = 1024;
const PAYLOAD_64KB: usize = 65536;

/// Build metadata with a `batching` field indicating the strategy used.
fn batching_meta(strategy: &str) -> HashMap<String, serde_json::Value> {
    let mut m = HashMap::new();
    m.insert(
        "batching".to_string(),
        serde_json::Value::String(strategy.to_string()),
    );
    m
}

/// Returns the measurement duration in seconds, configurable via `FILA_BENCH_DURATION_SECS`.
fn measure_secs() -> u64 {
    std::env::var("FILA_BENCH_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30)
}

/// Build a payload with an embedded nanosecond timestamp in the first 8 bytes.
/// The remaining bytes are zero-filled to reach `total_size`.
fn payload_with_timestamp(elapsed_nanos: u64, total_size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; total_size.max(8)];
    buf[..8].copy_from_slice(&elapsed_nanos.to_le_bytes());
    buf
}

/// Extract the embedded nanosecond timestamp from a payload's first 8 bytes.
fn extract_timestamp(payload: &[u8]) -> Option<u64> {
    if payload.len() < 8 {
        return None;
    }
    Some(u64::from_le_bytes(payload[..8].try_into().ok()?))
}

/// Emit latency percentile results from a histogram into the report.
fn emit_latency_results(report: &mut BenchReport, broker: &str, sampler: &LatencyHistogram) {
    if let Some(pcts) = sampler.percentiles() {
        let meta: HashMap<String, serde_json::Value> = [(
            "histogram".to_string(),
            serde_json::json!(sampler.serialize_base64()),
        )]
        .into_iter()
        .collect();
        for (label, value_us) in [
            ("p50", pcts.p50),
            ("p95", pcts.p95),
            ("p99", pcts.p99),
            ("p99_9", pcts.p99_9),
            ("p99_99", pcts.p99_99),
            ("max", pcts.max),
        ] {
            report.add(BenchResult {
                name: format!("{broker}_latency_{label}"),
                value: value_us / 1000.0,
                unit: "ms".to_string(),
                metadata: meta.clone(),
            });
        }
    }
}

// --- Kafka benchmarks ---

mod kafka {
    use super::*;
    use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
    use rdkafka::client::DefaultClientContext;
    use rdkafka::config::ClientConfig;
    use rdkafka::consumer::{BaseConsumer, Consumer, DefaultConsumerContext};
    use rdkafka::message::BorrowedMessage;
    use rdkafka::producer::{BaseRecord, DefaultProducerContext, Producer, ThreadedProducer};
    use rdkafka::Message;

    const BROKER: &str = "localhost:9092";

    fn admin() -> AdminClient<DefaultClientContext> {
        ClientConfig::new()
            .set("bootstrap.servers", BROKER)
            .create()
            .expect("kafka admin client")
    }

    async fn create_topic(admin: &AdminClient<DefaultClientContext>, name: &str) {
        let opts = AdminOptions::new();
        // Delete if exists (ignore errors)
        let _ = admin.delete_topics(&[name], &opts).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let topic = NewTopic::new(name, 1, TopicReplication::Fixed(1));
        admin
            .create_topics(&[topic], &opts)
            .await
            .expect("create topic");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    async fn cleanup_topic(admin: &AdminClient<DefaultClientContext>, name: &str) {
        let _ = admin.delete_topics(&[name], &AdminOptions::new()).await;
    }

    fn throughput_producer(
        broker: &str,
        linger_ms: &str,
        batch_size: &str,
    ) -> ThreadedProducer<DefaultProducerContext> {
        ClientConfig::new()
            .set("bootstrap.servers", broker)
            .set("linger.ms", linger_ms)
            .set("batch.num.messages", batch_size)
            .create()
            .expect("kafka producer")
    }

    pub async fn bench(report: &mut BenchReport) {
        let adm = admin();
        let measure_duration = measure_secs();

        // Throughput benchmarks
        for (size_name, payload_size) in [
            ("64b", PAYLOAD_64B),
            ("1kb", PAYLOAD_1KB),
            ("64kb", PAYLOAD_64KB),
        ] {
            let topic = format!("bench-throughput-{size_name}");
            create_topic(&adm, &topic).await;
            let payload = vec![0u8; payload_size];

            let producer = throughput_producer(BROKER, "5", "1000");

            // Warmup
            println!("[kafka] Throughput {size_name} warmup...");
            let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
            while Instant::now() < warmup_deadline {
                let _ = producer.send(BaseRecord::<(), [u8]>::to(&topic).payload(&payload));
            }
            producer.flush(Duration::from_secs(5)).ok();

            // Measure
            println!("[kafka] Throughput {size_name} measuring...");
            let mut meter = ThroughputMeter::start();
            let measure_deadline = Instant::now() + Duration::from_secs(measure_duration);
            while Instant::now() < measure_deadline {
                if producer
                    .send(BaseRecord::<(), [u8]>::to(&topic).payload(&payload))
                    .is_ok()
                {
                    meter.increment();
                }
            }
            producer.flush(Duration::from_secs(5)).ok();

            report.add(BenchResult {
                name: format!("kafka_throughput_{size_name}"),
                value: meter.msg_per_sec(),
                unit: "msg/s".to_string(),
                metadata: batching_meta("linger_ms=5"),
            });
            cleanup_topic(&adm, &topic).await;
        }

        // Latency benchmark (1KB) — concurrent produce/consume
        println!("[kafka] Latency benchmark...");
        {
            let topic_name = "bench-latency";
            create_topic(&adm, topic_name).await;

            let sampler = Arc::new(Mutex::new(LatencyHistogram::new()));
            let start_time = Instant::now();
            let warmup_dur = Duration::from_secs(WARMUP_SECS);
            let total_dur = Duration::from_secs(WARMUP_SECS + measure_duration);

            // Kafka producer/consumer are sync — use std::thread
            let producer_topic = topic_name.to_string();
            let producer_sampler = sampler.clone();
            let producer_handle = std::thread::spawn(move || {
                let producer = throughput_producer(BROKER, "0", "1");
                let start = start_time;
                let total = total_dur;
                while start.elapsed() < total {
                    let elapsed_nanos = start.elapsed().as_nanos() as u64;
                    let payload = payload_with_timestamp(elapsed_nanos, PAYLOAD_1KB);
                    let _ = producer
                        .send(BaseRecord::<(), [u8]>::to(&producer_topic).payload(&payload));
                    producer.flush(Duration::from_secs(1)).ok();
                    std::thread::sleep(Duration::from_millis(1));
                }
                let _ = producer_sampler; // keep alive
            });

            let consumer_topic = topic_name.to_string();
            let consumer_sampler = sampler.clone();
            let consumer_handle = std::thread::spawn(move || {
                let consumer: BaseConsumer<DefaultConsumerContext> = ClientConfig::new()
                    .set("bootstrap.servers", BROKER)
                    .set("group.id", "bench-latency-group")
                    .set("auto.offset.reset", "latest")
                    .set("enable.auto.commit", "true")
                    .create()
                    .expect("kafka consumer");
                consumer.subscribe(&[&consumer_topic]).expect("subscribe");
                // Prime consumer
                let _ = consumer.poll(Duration::from_secs(2));

                let start = start_time;
                let warmup = warmup_dur;
                let total = total_dur;
                while start.elapsed() < total {
                    if let Some(Ok(msg)) = consumer
                        .poll(Duration::from_millis(100))
                        .map(|r| r.map(|m: BorrowedMessage<'_>| m.payload().map(|p| p.to_vec())))
                    {
                        if let Some(ref payload) = msg {
                            if let Some(send_nanos) = extract_timestamp(payload) {
                                let now_nanos = start.elapsed().as_nanos() as u64;
                                if now_nanos > send_nanos {
                                    let latency = Duration::from_nanos(now_nanos - send_nanos);
                                    // Only record after warmup
                                    if start.elapsed() >= warmup {
                                        consumer_sampler.lock().unwrap().record(latency);
                                    }
                                }
                            }
                        }
                    }
                }
            });

            producer_handle.join().expect("kafka producer thread");
            consumer_handle.join().expect("kafka consumer thread");

            let locked = sampler.lock().unwrap();
            emit_latency_results(report, "kafka", &locked);
            cleanup_topic(&adm, topic_name).await;
        }

        // Lifecycle throughput
        println!("[kafka] Lifecycle throughput...");
        {
            let topic = "bench-lifecycle";
            create_topic(&adm, topic).await;
            let producer = throughput_producer(BROKER, "0", "1");
            let payload = vec![0u8; PAYLOAD_1KB];

            // Pre-load
            for _ in 0..LIFECYCLE_MESSAGES {
                let _ = producer.send(BaseRecord::<(), [u8]>::to(topic).payload(&payload));
            }
            producer.flush(Duration::from_secs(10)).ok();

            let consumer: BaseConsumer<DefaultConsumerContext> = ClientConfig::new()
                .set("bootstrap.servers", BROKER)
                .set("group.id", "bench-lifecycle-group")
                .set("auto.offset.reset", "earliest")
                .set("enable.auto.commit", "false")
                .create()
                .expect("kafka consumer");
            consumer.subscribe(&[topic]).expect("subscribe");
            std::thread::sleep(Duration::from_secs(1));

            let mut count = 0u64;
            let start = Instant::now();
            while count < LIFECYCLE_MESSAGES as u64 {
                if let Some(Ok(_)) = consumer
                    .poll(Duration::from_secs(5))
                    .map(|r| r.map(|_m: BorrowedMessage<'_>| ()))
                {
                    consumer
                        .commit_consumer_state(rdkafka::consumer::CommitMode::Sync)
                        .ok();
                    count += 1;
                }
            }
            let elapsed = start.elapsed().as_secs_f64();

            report.add(BenchResult {
                name: "kafka_lifecycle_throughput".to_string(),
                value: count as f64 / elapsed,
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });
            cleanup_topic(&adm, topic).await;
        }

        // Multi-producer throughput
        println!("[kafka] Multi-producer throughput...");
        {
            let topic = "bench-multi-producer";
            create_topic(&adm, topic).await;
            let payload = vec![0u8; PAYLOAD_1KB];

            let counts: Vec<std::sync::Arc<std::sync::atomic::AtomicU64>> = (0..MULTI_PRODUCERS)
                .map(|_| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)))
                .collect();

            let handles: Vec<_> = (0..MULTI_PRODUCERS)
                .map(|i| {
                    let topic = topic.to_string();
                    let payload = payload.clone();
                    let count = counts[i].clone();
                    std::thread::spawn(move || {
                        let producer = throughput_producer(BROKER, "5", "1000");
                        // Warmup
                        let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
                        while Instant::now() < warmup_deadline {
                            let _ =
                                producer.send(BaseRecord::<(), [u8]>::to(&topic).payload(&payload));
                        }
                        producer.flush(Duration::from_secs(5)).ok();
                        // Measure
                        let measure_deadline = Instant::now() + Duration::from_secs(measure_secs());
                        while Instant::now() < measure_deadline {
                            if producer
                                .send(BaseRecord::<(), [u8]>::to(&topic).payload(&payload))
                                .is_ok()
                            {
                                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        producer.flush(Duration::from_secs(5)).ok();
                    })
                })
                .collect();

            for h in handles {
                h.join().unwrap();
            }

            let total: u64 = counts
                .iter()
                .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                .sum();
            report.add(BenchResult {
                name: "kafka_multi_producer_throughput".to_string(),
                value: total as f64 / measure_duration as f64,
                unit: "msg/s".to_string(),
                metadata: batching_meta("linger_ms=5"),
            });
            cleanup_topic(&adm, topic).await;
        }

        // Resource stats
        if let Some(stats) = container_stats("competitive-kafka-1") {
            report.add(BenchResult {
                name: "kafka_cpu_pct".to_string(),
                value: stats.cpu_pct,
                unit: "%".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "kafka_memory_mb".to_string(),
                value: stats.memory_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "kafka_disk_io_read_mb".to_string(),
                value: stats.disk_read_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "kafka_disk_io_write_mb".to_string(),
                value: stats.disk_write_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
        }
    }
}

// --- RabbitMQ benchmarks ---

mod rabbitmq {
    use super::*;
    use lapin::{
        options::*, types::FieldTable, BasicProperties, Channel, Connection, ConnectionProperties,
    };

    const ADDR: &str = "amqp://bench:bench@localhost:5672";

    async fn connect() -> Connection {
        Connection::connect(ADDR, ConnectionProperties::default())
            .await
            .expect("rabbitmq connection")
    }

    async fn declare_queue(channel: &Channel, name: &str) {
        let mut args = FieldTable::default();
        args.insert(
            "x-queue-type".into(),
            lapin::types::AMQPValue::LongString("quorum".into()),
        );
        channel
            .queue_declare(
                name,
                QueueDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                args,
            )
            .await
            .expect("declare queue");
        channel
            .queue_purge(name, QueuePurgeOptions::default())
            .await
            .ok();
    }

    pub async fn bench(report: &mut BenchReport) {
        let measure_duration = measure_secs();

        // Throughput benchmarks
        for (size_name, payload_size) in [
            ("64b", PAYLOAD_64B),
            ("1kb", PAYLOAD_1KB),
            ("64kb", PAYLOAD_64KB),
        ] {
            let queue = format!("bench-throughput-{size_name}");
            let conn = connect().await;
            let channel = conn.create_channel().await.expect("channel");
            declare_queue(&channel, &queue).await;
            let payload = vec![0u8; payload_size];

            // Warmup
            println!("[rabbitmq] Throughput {size_name} warmup...");
            let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
            while Instant::now() < warmup_deadline {
                let _ = channel
                    .basic_publish(
                        "",
                        &queue,
                        BasicPublishOptions::default(),
                        &payload,
                        BasicProperties::default(),
                    )
                    .await;
            }

            // Measure
            println!("[rabbitmq] Throughput {size_name} measuring...");
            let mut meter = ThroughputMeter::start();
            let measure_deadline = Instant::now() + Duration::from_secs(measure_duration);
            while Instant::now() < measure_deadline {
                if channel
                    .basic_publish(
                        "",
                        &queue,
                        BasicPublishOptions::default(),
                        &payload,
                        BasicProperties::default(),
                    )
                    .await
                    .is_ok()
                {
                    meter.increment();
                }
            }

            report.add(BenchResult {
                name: format!("rabbitmq_throughput_{size_name}"),
                value: meter.msg_per_sec(),
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });
            channel
                .queue_delete(&queue, QueueDeleteOptions::default())
                .await
                .ok();
            conn.close(200, "done").await.ok();
        }

        // Latency benchmark (1KB) — concurrent produce/consume
        println!("[rabbitmq] Latency benchmark...");
        {
            let conn = connect().await;
            let pub_channel = conn.create_channel().await.expect("channel");
            let queue = "bench-latency";
            declare_queue(&pub_channel, queue).await;

            let sampler = Arc::new(Mutex::new(LatencyHistogram::new()));
            let start_time = Instant::now();
            let warmup_dur = Duration::from_secs(WARMUP_SECS);
            let total_dur = Duration::from_secs(WARMUP_SECS + measure_duration);

            // Producer task
            let producer_handle = {
                let queue = queue.to_string();
                tokio::spawn(async move {
                    let conn = connect().await;
                    let channel = conn.create_channel().await.expect("channel");
                    let start = start_time;
                    let total = total_dur;
                    while start.elapsed() < total {
                        let elapsed_nanos = start.elapsed().as_nanos() as u64;
                        let payload = payload_with_timestamp(elapsed_nanos, PAYLOAD_1KB);
                        let _ = channel
                            .basic_publish(
                                "",
                                &queue,
                                BasicPublishOptions::default(),
                                &payload,
                                BasicProperties::default(),
                            )
                            .await;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    conn.close(200, "done").await.ok();
                })
            };

            // Consumer task
            let consumer_handle = {
                let queue = queue.to_string();
                let consumer_sampler = sampler.clone();
                tokio::spawn(async move {
                    let conn = connect().await;
                    let channel = conn.create_channel().await.expect("channel");
                    let start = start_time;
                    let warmup = warmup_dur;
                    let total = total_dur;
                    while start.elapsed() < total {
                        match channel
                            .basic_get(&queue, BasicGetOptions { no_ack: false })
                            .await
                        {
                            Ok(Some(delivery)) => {
                                if let Some(send_nanos) = extract_timestamp(&delivery.data) {
                                    let now_nanos = start.elapsed().as_nanos() as u64;
                                    if now_nanos > send_nanos && start.elapsed() >= warmup {
                                        let latency = Duration::from_nanos(now_nanos - send_nanos);
                                        consumer_sampler.lock().unwrap().record(latency);
                                    }
                                }
                                delivery.acker.ack(BasicAckOptions::default()).await.ok();
                            }
                            _ => {
                                tokio::time::sleep(Duration::from_millis(1)).await;
                            }
                        }
                    }
                    conn.close(200, "done").await.ok();
                })
            };

            producer_handle.await.unwrap();
            consumer_handle.await.unwrap();

            let locked = sampler.lock().unwrap();
            emit_latency_results(report, "rabbitmq", &locked);

            pub_channel
                .queue_delete(queue, QueueDeleteOptions::default())
                .await
                .ok();
            conn.close(200, "done").await.ok();
        }

        // Lifecycle throughput
        println!("[rabbitmq] Lifecycle throughput...");
        {
            let conn = connect().await;
            let channel = conn.create_channel().await.expect("channel");
            let queue = "bench-lifecycle";
            declare_queue(&channel, queue).await;
            let payload = vec![0u8; PAYLOAD_1KB];

            for _ in 0..LIFECYCLE_MESSAGES {
                let _ = channel
                    .basic_publish(
                        "",
                        queue,
                        BasicPublishOptions::default(),
                        &payload,
                        BasicProperties::default(),
                    )
                    .await;
            }

            let mut count = 0u64;
            let start = Instant::now();
            while count < LIFECYCLE_MESSAGES as u64 {
                match channel
                    .basic_get(queue, BasicGetOptions { no_ack: false })
                    .await
                {
                    Ok(Some(delivery)) => {
                        delivery.acker.ack(BasicAckOptions::default()).await.ok();
                        count += 1;
                    }
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
            let elapsed = start.elapsed().as_secs_f64();

            report.add(BenchResult {
                name: "rabbitmq_lifecycle_throughput".to_string(),
                value: count as f64 / elapsed,
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });
            channel
                .queue_delete(queue, QueueDeleteOptions::default())
                .await
                .ok();
            conn.close(200, "done").await.ok();
        }

        // Multi-producer throughput
        println!("[rabbitmq] Multi-producer throughput...");
        {
            let conn = connect().await;
            let channel = conn.create_channel().await.expect("channel");
            let queue = "bench-multi-producer";
            declare_queue(&channel, queue).await;
            conn.close(200, "setup done").await.ok();

            let payload = vec![0u8; PAYLOAD_1KB];
            let counts: Vec<std::sync::Arc<std::sync::atomic::AtomicU64>> = (0..MULTI_PRODUCERS)
                .map(|_| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)))
                .collect();

            let handles: Vec<_> = (0..MULTI_PRODUCERS)
                .map(|i| {
                    let queue = queue.to_string();
                    let payload = payload.clone();
                    let count = counts[i].clone();
                    tokio::spawn(async move {
                        let conn = connect().await;
                        let ch = conn.create_channel().await.expect("channel");
                        // Warmup
                        let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
                        while Instant::now() < warmup_deadline {
                            let _ = ch
                                .basic_publish(
                                    "",
                                    &queue,
                                    BasicPublishOptions::default(),
                                    &payload,
                                    BasicProperties::default(),
                                )
                                .await;
                        }
                        // Measure
                        let measure_deadline = Instant::now() + Duration::from_secs(measure_secs());
                        while Instant::now() < measure_deadline {
                            if ch
                                .basic_publish(
                                    "",
                                    &queue,
                                    BasicPublishOptions::default(),
                                    &payload,
                                    BasicProperties::default(),
                                )
                                .await
                                .is_ok()
                            {
                                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        conn.close(200, "done").await.ok();
                    })
                })
                .collect();

            for h in handles {
                h.await.unwrap();
            }

            let total: u64 = counts
                .iter()
                .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                .sum();
            report.add(BenchResult {
                name: "rabbitmq_multi_producer_throughput".to_string(),
                value: total as f64 / measure_duration as f64,
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });

            let conn = connect().await;
            let channel = conn.create_channel().await.expect("channel");
            channel
                .queue_delete(queue, QueueDeleteOptions::default())
                .await
                .ok();
            conn.close(200, "done").await.ok();
        }

        // Resource stats
        if let Some(stats) = container_stats("competitive-rabbitmq-1") {
            report.add(BenchResult {
                name: "rabbitmq_cpu_pct".to_string(),
                value: stats.cpu_pct,
                unit: "%".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "rabbitmq_memory_mb".to_string(),
                value: stats.memory_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "rabbitmq_disk_io_read_mb".to_string(),
                value: stats.disk_read_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "rabbitmq_disk_io_write_mb".to_string(),
                value: stats.disk_write_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
        }
    }
}

// --- NATS benchmarks ---

mod nats {
    use super::*;
    use async_nats::jetstream;
    use futures_util::StreamExt;

    const URL: &str = "nats://localhost:4222";

    async fn connect() -> async_nats::Client {
        async_nats::connect(URL).await.expect("nats connection")
    }

    async fn ensure_stream(js: &jetstream::Context, name: &str, subjects: Vec<String>) {
        let _ = js.delete_stream(name).await;
        js.create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects,
            ..Default::default()
        })
        .await
        .expect("create stream");
    }

    pub async fn bench(report: &mut BenchReport) {
        let client = connect().await;
        let js = jetstream::new(client.clone());
        let measure_duration = measure_secs();

        // Throughput benchmarks
        for (size_name, payload_size) in [
            ("64b", PAYLOAD_64B),
            ("1kb", PAYLOAD_1KB),
            ("64kb", PAYLOAD_64KB),
        ] {
            let stream = format!("bench-tp-{size_name}");
            let subject = format!("bench.tp.{size_name}");
            ensure_stream(&js, &stream, vec![subject.clone()]).await;
            let payload = vec![0u8; payload_size];

            // Warmup
            println!("[nats] Throughput {size_name} warmup...");
            let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
            while Instant::now() < warmup_deadline {
                let _ = js.publish(subject.clone(), payload.clone().into()).await;
            }

            // Measure
            println!("[nats] Throughput {size_name} measuring...");
            let mut meter = ThroughputMeter::start();
            let measure_deadline = Instant::now() + Duration::from_secs(measure_duration);
            while Instant::now() < measure_deadline {
                if js
                    .publish(subject.clone(), payload.clone().into())
                    .await
                    .is_ok()
                {
                    meter.increment();
                }
            }

            report.add(BenchResult {
                name: format!("nats_throughput_{size_name}"),
                value: meter.msg_per_sec(),
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });
            let _ = js.delete_stream(&stream).await;
        }

        // Latency benchmark (1KB) — concurrent produce/consume
        println!("[nats] Latency benchmark...");
        {
            let stream = "bench-latency";
            let subject = "bench.latency";
            ensure_stream(&js, stream, vec![subject.to_string()]).await;

            let sampler = Arc::new(Mutex::new(LatencyHistogram::new()));
            let start_time = Instant::now();
            let warmup_dur = Duration::from_secs(WARMUP_SECS);
            let total_dur = Duration::from_secs(WARMUP_SECS + measure_duration);

            // Producer task
            let producer_handle = {
                let subject = subject.to_string();
                let js = js.clone();
                tokio::spawn(async move {
                    let start = start_time;
                    let total = total_dur;
                    while start.elapsed() < total {
                        let elapsed_nanos = start.elapsed().as_nanos() as u64;
                        let payload = payload_with_timestamp(elapsed_nanos, PAYLOAD_1KB);
                        let _ = js.publish(subject.clone(), payload.into()).await;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
            };

            // Consumer task
            let consumer_handle = {
                let consumer_sampler = sampler.clone();
                let consumer: jetstream::consumer::PullConsumer = js
                    .create_consumer_on_stream(
                        jetstream::consumer::pull::Config {
                            durable_name: Some("bench-lat-consumer".to_string()),
                            ack_policy: jetstream::consumer::AckPolicy::Explicit,
                            ..Default::default()
                        },
                        stream,
                    )
                    .await
                    .expect("create consumer");

                tokio::spawn(async move {
                    let start = start_time;
                    let warmup = warmup_dur;
                    let total = total_dur;
                    while start.elapsed() < total {
                        if let Ok(mut batch) = consumer
                            .fetch()
                            .max_messages(10)
                            .expires(Duration::from_secs(1))
                            .messages()
                            .await
                        {
                            while let Some(Ok(msg)) = batch.next().await {
                                if let Some(send_nanos) = extract_timestamp(&msg.payload) {
                                    let now_nanos = start.elapsed().as_nanos() as u64;
                                    if now_nanos > send_nanos && start.elapsed() >= warmup {
                                        let latency = Duration::from_nanos(now_nanos - send_nanos);
                                        consumer_sampler.lock().unwrap().record(latency);
                                    }
                                }
                                let _ = msg.ack().await;
                            }
                        }
                    }
                })
            };

            producer_handle.await.unwrap();
            consumer_handle.await.unwrap();

            let locked = sampler.lock().unwrap();
            emit_latency_results(report, "nats", &locked);
            let _ = js.delete_stream(stream).await;
        }

        // Lifecycle throughput
        println!("[nats] Lifecycle throughput...");
        {
            let stream = "bench-lifecycle";
            let subject = "bench.lifecycle";
            ensure_stream(&js, stream, vec![subject.to_string()]).await;
            let payload = vec![0u8; PAYLOAD_1KB];

            for _ in 0..LIFECYCLE_MESSAGES {
                let _ = js.publish(subject, payload.clone().into()).await;
            }

            let consumer: jetstream::consumer::PullConsumer = js
                .create_consumer_on_stream(
                    jetstream::consumer::pull::Config {
                        durable_name: Some("bench-lc-consumer".to_string()),
                        ack_policy: jetstream::consumer::AckPolicy::Explicit,
                        ..Default::default()
                    },
                    stream,
                )
                .await
                .expect("create consumer");

            let mut count = 0u64;
            let start = Instant::now();
            while count < LIFECYCLE_MESSAGES as u64 {
                if let Ok(mut batch) = consumer
                    .fetch()
                    .max_messages(10)
                    .expires(Duration::from_secs(5))
                    .messages()
                    .await
                {
                    while let Some(Ok(msg)) = batch.next().await {
                        let _ = msg.ack().await;
                        count += 1;
                    }
                }
            }
            let elapsed = start.elapsed().as_secs_f64();

            report.add(BenchResult {
                name: "nats_lifecycle_throughput".to_string(),
                value: count as f64 / elapsed,
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });
            let _ = js.delete_stream(stream).await;
        }

        // Multi-producer throughput
        println!("[nats] Multi-producer throughput...");
        {
            let stream = "bench-multi-producer";
            let subject = "bench.mp";
            ensure_stream(&js, stream, vec![subject.to_string()]).await;
            let payload = vec![0u8; PAYLOAD_1KB];

            let counts: Vec<std::sync::Arc<std::sync::atomic::AtomicU64>> = (0..MULTI_PRODUCERS)
                .map(|_| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)))
                .collect();

            let handles: Vec<_> = (0..MULTI_PRODUCERS)
                .map(|i| {
                    let payload = payload.clone();
                    let count = counts[i].clone();
                    let subject = subject.to_string();
                    tokio::spawn(async move {
                        let client = connect().await;
                        let js = jetstream::new(client);
                        // Warmup
                        let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
                        while Instant::now() < warmup_deadline {
                            let _ = js.publish(subject.clone(), payload.clone().into()).await;
                        }
                        // Measure
                        let measure_deadline = Instant::now() + Duration::from_secs(measure_secs());
                        while Instant::now() < measure_deadline {
                            if js
                                .publish(subject.clone(), payload.clone().into())
                                .await
                                .is_ok()
                            {
                                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    })
                })
                .collect();

            for h in handles {
                h.await.unwrap();
            }

            let total: u64 = counts
                .iter()
                .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                .sum();
            report.add(BenchResult {
                name: "nats_multi_producer_throughput".to_string(),
                value: total as f64 / measure_duration as f64,
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });
            let _ = js.delete_stream(stream).await;
        }

        // Resource stats
        if let Some(stats) = container_stats("competitive-nats-1") {
            report.add(BenchResult {
                name: "nats_cpu_pct".to_string(),
                value: stats.cpu_pct,
                unit: "%".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "nats_memory_mb".to_string(),
                value: stats.memory_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "nats_disk_io_read_mb".to_string(),
                value: stats.disk_read_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "nats_disk_io_write_mb".to_string(),
                value: stats.disk_write_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
        }
    }
}

// --- Fila benchmarks ---

mod fila {
    use super::*;
    use fila_bench::server::create_queue_cli;
    use fila_sdk::{BatchMode, ConnectOptions, FilaClient};
    use tokio_stream::StreamExt;

    const ADDR: &str = "http://localhost:5555";
    /// Number of concurrent producers for throughput scenarios.
    /// Concurrent producers are needed to trigger auto-batching: the Nagle-style
    /// batcher sends immediately when idle, so a single serial producer never
    /// accumulates messages. Multiple producers ensure messages queue while RPCs
    /// are in flight, matching how Kafka's linger.ms amortizes network calls.
    const THROUGHPUT_PRODUCERS: usize = 4;

    pub async fn bench(report: &mut BenchReport) {
        let addr = ADDR;
        let measure_duration = measure_secs();

        // Throughput benchmarks — concurrent producers with auto-batching
        for (size_name, payload_size) in [
            ("64b", PAYLOAD_64B),
            ("1kb", PAYLOAD_1KB),
            ("64kb", PAYLOAD_64KB),
        ] {
            let queue = format!("bench-throughput-{size_name}");
            create_queue_cli(addr, &queue);

            println!("[fila] Throughput {size_name} warmup ({THROUGHPUT_PRODUCERS} producers, auto-batching)...");
            let counts: Vec<Arc<std::sync::atomic::AtomicU64>> = (0..THROUGHPUT_PRODUCERS)
                .map(|_| Arc::new(std::sync::atomic::AtomicU64::new(0)))
                .collect();

            let handles: Vec<_> = (0..THROUGHPUT_PRODUCERS)
                .map(|i| {
                    let queue = queue.clone();
                    let count = counts[i].clone();
                    let addr = addr.to_string();
                    tokio::spawn(async move {
                        let client = FilaClient::connect(&addr).await.expect("fila connect");
                        let payload = vec![0u8; payload_size];
                        let headers = HashMap::new();
                        // Warmup
                        let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
                        while Instant::now() < warmup_deadline {
                            let _ = client
                                .enqueue(&queue, headers.clone(), payload.clone())
                                .await;
                        }
                        // Measure
                        let measure_deadline =
                            Instant::now() + Duration::from_secs(measure_secs());
                        while Instant::now() < measure_deadline {
                            if client
                                .enqueue(&queue, headers.clone(), payload.clone())
                                .await
                                .is_ok()
                            {
                                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    })
                })
                .collect();

            println!("[fila] Throughput {size_name} measuring...");
            for h in handles {
                h.await.unwrap();
            }

            let total: u64 = counts
                .iter()
                .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                .sum();
            report.add(BenchResult {
                name: format!("fila_throughput_{size_name}"),
                value: total as f64 / measure_duration as f64,
                unit: "msg/s".to_string(),
                metadata: batching_meta("auto"),
            });
        }

        // Latency benchmark (1KB) — concurrent produce/consume, unbatched
        println!("[fila] Latency benchmark (unbatched)...");
        {
            let queue = "bench-latency";
            create_queue_cli(addr, queue);

            let sampler = Arc::new(Mutex::new(LatencyHistogram::new()));
            let start_time = Instant::now();
            let warmup_dur = Duration::from_secs(WARMUP_SECS);
            let total_dur = Duration::from_secs(WARMUP_SECS + measure_duration);

            // Producer task — use Disabled batching for accurate per-message latency
            let producer_handle = {
                let queue = queue.to_string();
                let opts = ConnectOptions::new(addr)
                    .with_batch_mode(BatchMode::Disabled);
                let client = FilaClient::connect_with_options(opts)
                    .await
                    .expect("fila connect");
                tokio::spawn(async move {
                    let start = start_time;
                    let total = total_dur;
                    let headers = HashMap::new();
                    while start.elapsed() < total {
                        let elapsed_nanos = start.elapsed().as_nanos() as u64;
                        let payload = payload_with_timestamp(elapsed_nanos, PAYLOAD_1KB);
                        let _ = client.enqueue(&queue, headers.clone(), payload).await;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
            };

            // Consumer task
            let consumer_handle = {
                let consumer_sampler = sampler.clone();
                let queue = queue.to_string();
                let client_c = FilaClient::connect(addr).await.expect("fila connect");
                let mut stream = client_c.consume(&queue).await.expect("fila consume");

                tokio::spawn(async move {
                    let start = start_time;
                    let warmup = warmup_dur;
                    let total = total_dur;
                    while start.elapsed() < total {
                        match tokio::time::timeout(Duration::from_secs(1), stream.next()).await {
                            Ok(Some(Ok(msg))) => {
                                if let Some(send_nanos) = extract_timestamp(&msg.payload) {
                                    let now_nanos = start.elapsed().as_nanos() as u64;
                                    if now_nanos > send_nanos && start.elapsed() >= warmup {
                                        let latency = Duration::from_nanos(now_nanos - send_nanos);
                                        consumer_sampler.lock().unwrap().record(latency);
                                    }
                                }
                                client_c.ack(&queue, &msg.id).await.ok();
                            }
                            _ => {}
                        }
                    }
                })
            };

            producer_handle.await.unwrap();
            consumer_handle.await.unwrap();

            let locked = sampler.lock().unwrap();
            emit_latency_results(report, "fila", &locked);
        }

        // Lifecycle throughput — unbatched serial enqueue→consume→ack
        println!("[fila] Lifecycle throughput (unbatched)...");
        {
            let queue = "bench-lifecycle";
            create_queue_cli(addr, queue);
            let opts =
                ConnectOptions::new(addr).with_batch_mode(BatchMode::Disabled);
            let client = FilaClient::connect_with_options(opts)
                .await
                .expect("fila connect");
            let payload = vec![0u8; PAYLOAD_1KB];
            let headers = HashMap::new();

            // Pre-load
            for _ in 0..LIFECYCLE_MESSAGES {
                let _ = client
                    .enqueue(queue, headers.clone(), payload.clone())
                    .await;
            }

            let mut stream = client.consume(queue).await.expect("fila consume");
            let mut count = 0u64;
            let start = Instant::now();
            while count < LIFECYCLE_MESSAGES as u64 {
                if let Some(Ok(msg)) = stream.next().await {
                    client.ack(queue, &msg.id).await.ok();
                    count += 1;
                }
            }
            let elapsed = start.elapsed().as_secs_f64();

            report.add(BenchResult {
                name: "fila_lifecycle_throughput".to_string(),
                value: count as f64 / elapsed,
                unit: "msg/s".to_string(),
                metadata: batching_meta("none"),
            });
        }

        // Multi-producer throughput — concurrent producers with auto-batching
        println!("[fila] Multi-producer throughput ({MULTI_PRODUCERS} producers, auto-batching)...");
        {
            let queue = "bench-multi-producer";
            create_queue_cli(addr, queue);
            let payload = vec![0u8; PAYLOAD_1KB];
            let addr = addr.to_string();

            let counts: Vec<Arc<std::sync::atomic::AtomicU64>> = (0..MULTI_PRODUCERS)
                .map(|_| Arc::new(std::sync::atomic::AtomicU64::new(0)))
                .collect();

            let handles: Vec<_> = (0..MULTI_PRODUCERS)
                .map(|i| {
                    let queue = queue.to_string();
                    let payload = payload.clone();
                    let count = counts[i].clone();
                    let addr = addr.clone();
                    tokio::spawn(async move {
                        let client = FilaClient::connect(&addr).await.expect("fila connect");
                        let headers = HashMap::new();
                        // Warmup
                        let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
                        while Instant::now() < warmup_deadline {
                            let _ = client
                                .enqueue(&queue, headers.clone(), payload.clone())
                                .await;
                        }
                        // Measure
                        let measure_deadline = Instant::now() + Duration::from_secs(measure_secs());
                        while Instant::now() < measure_deadline {
                            if client
                                .enqueue(&queue, headers.clone(), payload.clone())
                                .await
                                .is_ok()
                            {
                                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    })
                })
                .collect();

            for h in handles {
                h.await.unwrap();
            }

            let total: u64 = counts
                .iter()
                .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                .sum();
            report.add(BenchResult {
                name: "fila_multi_producer_throughput".to_string(),
                value: total as f64 / measure_duration as f64,
                unit: "msg/s".to_string(),
                metadata: batching_meta("auto"),
            });
        }

        // Resource stats (container-level, same as other brokers)
        if let Some(stats) = container_stats("competitive-fila-1") {
            report.add(BenchResult {
                name: "fila_cpu_pct".to_string(),
                value: stats.cpu_pct,
                unit: "%".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "fila_memory_mb".to_string(),
                value: stats.memory_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "fila_disk_io_read_mb".to_string(),
                value: stats.disk_read_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "fila_disk_io_write_mb".to_string(),
                value: stats.disk_write_mb,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
        }
    }
}

// --- Shared utilities ---

/// Container resource stats from `docker stats`.
struct ContainerStats {
    cpu_pct: f64,
    memory_mb: f64,
    disk_read_mb: f64,
    disk_write_mb: f64,
}

/// Parse a BlockIO size string (e.g., "1.23GB", "456MB", "789kB", "0B") into megabytes.
fn parse_block_io_mb(s: &str) -> f64 {
    let s = s.trim();
    if s.ends_with("TB") {
        s.trim_end_matches("TB")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0)
            * 1_000_000.0
    } else if s.ends_with("GB") {
        s.trim_end_matches("GB")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0)
            * 1000.0
    } else if s.ends_with("MB") {
        s.trim_end_matches("MB")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0)
    } else if s.ends_with("kB") {
        s.trim_end_matches("kB")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0)
            / 1000.0
    } else if s.ends_with('B') {
        s.trim_end_matches('B').trim().parse::<f64>().unwrap_or(0.0) / 1_000_000.0
    } else {
        0.0
    }
}

/// Get CPU%, memory MB, and disk I/O from a Docker container via `docker stats`.
fn container_stats(container_name: &str) -> Option<ContainerStats> {
    let output = std::process::Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.CPUPerc}}\t{{.MemUsage}}\t{{.BlockIO}}",
        ])
        .arg(container_name)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = text.trim().split('\t').collect();
    if parts.len() < 3 {
        return None;
    }
    let cpu_pct: f64 = parts[0].trim_end_matches('%').parse().ok()?;
    let mem_str = parts[1].split('/').next()?.trim();
    let memory_mb = if mem_str.contains("GiB") {
        mem_str.replace("GiB", "").trim().parse::<f64>().ok()? * 1024.0
    } else if mem_str.contains("MiB") {
        mem_str.replace("MiB", "").trim().parse::<f64>().ok()?
    } else if mem_str.contains("KiB") {
        mem_str.replace("KiB", "").trim().parse::<f64>().ok()? / 1024.0
    } else {
        0.0
    };

    // BlockIO format: "read / write" e.g. "1.23GB / 456MB"
    let block_parts: Vec<&str> = parts[2].split('/').collect();
    let (disk_read_mb, disk_write_mb) = if block_parts.len() == 2 {
        (
            parse_block_io_mb(block_parts[0]),
            parse_block_io_mb(block_parts[1]),
        )
    } else {
        (0.0, 0.0)
    };

    Some(ContainerStats {
        cpu_pct,
        memory_mb,
        disk_read_mb,
        disk_write_mb,
    })
}

fn print_summary(report: &BenchReport, broker: &str) {
    println!("\n========================================");
    println!("  {broker} Benchmark Results");
    println!("  Time: {}", report.timestamp);
    println!("========================================\n");

    for b in &report.benchmarks {
        if b.name.starts_with(broker) {
            println!("  {:<50} {:>12.2} {}", b.name, b.value, b.unit);
        }
    }
    println!("\n========================================\n");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: bench-competitive <fila|kafka|rabbitmq|nats|all> <output-dir>");
        std::process::exit(1);
    }

    let broker_arg = &args[1];
    let output_dir = &args[2];
    std::fs::create_dir_all(output_dir).expect("create output dir");

    let brokers: Vec<&str> = if broker_arg == "all" {
        vec!["fila", "kafka", "rabbitmq", "nats"]
    } else {
        vec![broker_arg.as_str()]
    };

    for broker in brokers {
        println!("\n############################################################");
        println!("  Benchmarking {}", broker.to_uppercase());
        println!("############################################################\n");

        let mut report = BenchReport::new();

        match broker {
            "fila" => fila::bench(&mut report).await,
            "kafka" => kafka::bench(&mut report).await,
            "rabbitmq" => rabbitmq::bench(&mut report).await,
            "nats" => nats::bench(&mut report).await,
            _ => {
                eprintln!("Unknown broker: {broker}. Choose fila, kafka, rabbitmq, nats, or all.");
                std::process::exit(1);
            }
        }

        print_summary(&report, broker);

        let output_path = format!("{output_dir}/bench-{broker}.json");
        std::fs::write(&output_path, report.to_json()).expect("write report");
        println!("Results written to: {output_path}");
    }
}
