//! Competitive benchmark suite: Fila vs Kafka vs RabbitMQ vs NATS.
//!
//! Runs identical workloads against each broker using native Rust clients,
//! producing JSON reports compatible with BenchReport schema.
//!
//! Usage:
//!   bench-competitive fila <output-dir>
//!   bench-competitive kafka <output-dir>
//!   bench-competitive rabbitmq <output-dir>
//!   bench-competitive nats <output-dir>
//!   bench-competitive all <output-dir>

use fila_bench::measurement::{LatencySampler, ThroughputMeter};
use fila_bench::report::{BenchReport, BenchResult};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const WARMUP_SECS: u64 = 1;
const MEASURE_SECS: u64 = 3;
const LATENCY_SAMPLES: usize = 100;
const LIFECYCLE_MESSAGES: usize = 1000;
const MULTI_PRODUCERS: usize = 3;

const PAYLOAD_64B: usize = 64;
const PAYLOAD_1KB: usize = 1024;
const PAYLOAD_64KB: usize = 65536;

// --- Kafka benchmarks ---

mod kafka {
    use super::*;
    use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
    use rdkafka::client::DefaultClientContext;
    use rdkafka::config::ClientConfig;
    use rdkafka::consumer::{BaseConsumer, Consumer, DefaultConsumerContext};
    use rdkafka::message::BorrowedMessage;
    use rdkafka::producer::{BaseRecord, DefaultProducerContext, Producer, ThreadedProducer};

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
            let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
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
                metadata: HashMap::new(),
            });
            cleanup_topic(&adm, &topic).await;
        }

        // Latency benchmark (1KB)
        println!("[kafka] Latency benchmark...");
        {
            let topic = "bench-latency";
            create_topic(&adm, topic).await;

            let producer = throughput_producer(BROKER, "0", "1");

            let consumer: BaseConsumer<DefaultConsumerContext> = ClientConfig::new()
                .set("bootstrap.servers", BROKER)
                .set("group.id", "bench-latency-group")
                .set("auto.offset.reset", "latest")
                .set("enable.auto.commit", "true")
                .create()
                .expect("kafka consumer");
            consumer.subscribe(&[topic]).expect("subscribe");
            // Prime consumer
            let _ = consumer.poll(Duration::from_secs(2));

            let payload = vec![0u8; PAYLOAD_1KB];
            let mut sampler = LatencySampler::with_capacity(LATENCY_SAMPLES);

            for _ in 0..LATENCY_SAMPLES {
                let start = Instant::now();
                let _ = producer.send(BaseRecord::<(), [u8]>::to(topic).payload(&payload));
                producer.flush(Duration::from_secs(5)).ok();
                // Poll until we get a message
                let deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() < deadline {
                    if let Some(Ok(_msg)) = consumer
                        .poll(Duration::from_millis(100))
                        .map(|r| r.map(|_m: BorrowedMessage<'_>| ()))
                    {
                        sampler.record(start.elapsed());
                        break;
                    }
                }
            }

            if let Some((p50, p95, p99)) = sampler.percentiles() {
                report.add(BenchResult {
                    name: "kafka_latency_p50".to_string(),
                    value: p50.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "kafka_latency_p95".to_string(),
                    value: p95.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "kafka_latency_p99".to_string(),
                    value: p99.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
            }
            cleanup_topic(&adm, topic).await;
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
                metadata: HashMap::new(),
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
                        let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
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
                value: total as f64 / MEASURE_SECS as f64,
                unit: "msg/s".to_string(),
                metadata: HashMap::new(),
            });
            cleanup_topic(&adm, topic).await;
        }

        // Resource stats
        if let Some(stats) = container_stats("competitive-kafka-1") {
            report.add(BenchResult {
                name: "kafka_cpu_pct".to_string(),
                value: stats.0,
                unit: "%".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "kafka_memory_mb".to_string(),
                value: stats.1,
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
            let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
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
                metadata: HashMap::new(),
            });
            channel
                .queue_delete(&queue, QueueDeleteOptions::default())
                .await
                .ok();
            conn.close(200, "done").await.ok();
        }

        // Latency benchmark (1KB)
        println!("[rabbitmq] Latency benchmark...");
        {
            let conn = connect().await;
            let channel = conn.create_channel().await.expect("channel");
            let queue = "bench-latency";
            declare_queue(&channel, queue).await;
            let payload = vec![0u8; PAYLOAD_1KB];

            let mut sampler = LatencySampler::with_capacity(LATENCY_SAMPLES);
            for _ in 0..LATENCY_SAMPLES {
                let start = Instant::now();
                let _ = channel
                    .basic_publish(
                        "",
                        queue,
                        BasicPublishOptions::default(),
                        &payload,
                        BasicProperties::default(),
                    )
                    .await;
                if let Ok(Some(delivery)) = channel
                    .basic_get(queue, BasicGetOptions { no_ack: false })
                    .await
                {
                    delivery.acker.ack(BasicAckOptions::default()).await.ok();
                    sampler.record(start.elapsed());
                }
            }

            if let Some((p50, p95, p99)) = sampler.percentiles() {
                report.add(BenchResult {
                    name: "rabbitmq_latency_p50".to_string(),
                    value: p50.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "rabbitmq_latency_p95".to_string(),
                    value: p95.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "rabbitmq_latency_p99".to_string(),
                    value: p99.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
            }
            channel
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
                metadata: HashMap::new(),
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
                        let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
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
                value: total as f64 / MEASURE_SECS as f64,
                unit: "msg/s".to_string(),
                metadata: HashMap::new(),
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
                value: stats.0,
                unit: "%".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "rabbitmq_memory_mb".to_string(),
                value: stats.1,
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
            let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
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
                metadata: HashMap::new(),
            });
            let _ = js.delete_stream(&stream).await;
        }

        // Latency benchmark (1KB)
        println!("[nats] Latency benchmark...");
        {
            let stream = "bench-latency";
            let subject = "bench.latency";
            ensure_stream(&js, stream, vec![subject.to_string()]).await;
            let payload = vec![0u8; PAYLOAD_1KB];

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

            let mut sampler = LatencySampler::with_capacity(LATENCY_SAMPLES);
            for _ in 0..LATENCY_SAMPLES {
                let start = Instant::now();
                let _ = js.publish(subject, payload.clone().into()).await;
                if let Ok(mut batch) = consumer
                    .fetch()
                    .max_messages(1)
                    .expires(Duration::from_secs(5))
                    .messages()
                    .await
                {
                    if let Some(Ok(msg)) = batch.next().await {
                        let _ = msg.ack().await;
                        sampler.record(start.elapsed());
                    }
                }
            }

            if let Some((p50, p95, p99)) = sampler.percentiles() {
                report.add(BenchResult {
                    name: "nats_latency_p50".to_string(),
                    value: p50.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "nats_latency_p95".to_string(),
                    value: p95.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "nats_latency_p99".to_string(),
                    value: p99.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
            }
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
                metadata: HashMap::new(),
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
                        let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
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
                value: total as f64 / MEASURE_SECS as f64,
                unit: "msg/s".to_string(),
                metadata: HashMap::new(),
            });
            let _ = js.delete_stream(stream).await;
        }

        // Resource stats
        if let Some(stats) = container_stats("competitive-nats-1") {
            report.add(BenchResult {
                name: "nats_cpu_pct".to_string(),
                value: stats.0,
                unit: "%".to_string(),
                metadata: HashMap::new(),
            });
            report.add(BenchResult {
                name: "nats_memory_mb".to_string(),
                value: stats.1,
                unit: "MB".to_string(),
                metadata: HashMap::new(),
            });
        }
    }
}

// --- Fila benchmarks ---

mod fila {
    use super::*;
    use fila_bench::measurement::process_rss_bytes;
    use fila_bench::server::{create_queue_cli, BenchServer};
    use fila_sdk::FilaClient;
    use tokio_stream::StreamExt;

    pub async fn bench(report: &mut BenchReport) {
        let server = BenchServer::start();
        let addr = server.addr();

        // Throughput benchmarks
        for (size_name, payload_size) in [
            ("64b", PAYLOAD_64B),
            ("1kb", PAYLOAD_1KB),
            ("64kb", PAYLOAD_64KB),
        ] {
            let queue = format!("bench-throughput-{size_name}");
            create_queue_cli(addr, &queue);
            let client = FilaClient::connect(addr).await.expect("fila connect");
            let payload = vec![0u8; payload_size];
            let headers = HashMap::new();

            // Warmup
            println!("[fila] Throughput {size_name} warmup...");
            let warmup_deadline = Instant::now() + Duration::from_secs(WARMUP_SECS);
            while Instant::now() < warmup_deadline {
                let _ = client
                    .enqueue(&queue, headers.clone(), payload.clone())
                    .await;
            }

            // Measure
            println!("[fila] Throughput {size_name} measuring...");
            let mut meter = ThroughputMeter::start();
            let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
            while Instant::now() < measure_deadline {
                if client
                    .enqueue(&queue, headers.clone(), payload.clone())
                    .await
                    .is_ok()
                {
                    meter.increment();
                }
            }

            report.add(BenchResult {
                name: format!("fila_throughput_{size_name}"),
                value: meter.msg_per_sec(),
                unit: "msg/s".to_string(),
                metadata: HashMap::new(),
            });
        }

        // Latency benchmark (1KB)
        println!("[fila] Latency benchmark...");
        {
            let queue = "bench-latency";
            create_queue_cli(addr, queue);
            let client = FilaClient::connect(addr).await.expect("fila connect");
            let payload = vec![0u8; PAYLOAD_1KB];
            let headers = HashMap::new();

            let mut stream = client.consume(queue).await.expect("fila consume");
            let mut sampler = LatencySampler::with_capacity(LATENCY_SAMPLES);

            for _ in 0..LATENCY_SAMPLES {
                let start = Instant::now();
                let enqueued_id = client
                    .enqueue(queue, headers.clone(), payload.clone())
                    .await
                    .expect("enqueue");
                if let Ok(Some(Ok(msg))) =
                    tokio::time::timeout(Duration::from_secs(5), stream.next()).await
                {
                    if msg.id == enqueued_id {
                        sampler.record(start.elapsed());
                        client.ack(queue, &msg.id).await.ok();
                    }
                }
            }

            if let Some((p50, p95, p99)) = sampler.percentiles() {
                report.add(BenchResult {
                    name: "fila_latency_p50".to_string(),
                    value: p50.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "fila_latency_p95".to_string(),
                    value: p95.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
                report.add(BenchResult {
                    name: "fila_latency_p99".to_string(),
                    value: p99.as_secs_f64() * 1000.0,
                    unit: "ms".to_string(),
                    metadata: HashMap::new(),
                });
            }
        }

        // Lifecycle throughput
        println!("[fila] Lifecycle throughput...");
        {
            let queue = "bench-lifecycle";
            create_queue_cli(addr, queue);
            let client = FilaClient::connect(addr).await.expect("fila connect");
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
                metadata: HashMap::new(),
            });
        }

        // Multi-producer throughput
        println!("[fila] Multi-producer throughput...");
        {
            let queue = "bench-multi-producer";
            create_queue_cli(addr, queue);
            let payload = vec![0u8; PAYLOAD_1KB];
            let addr = addr.to_string();

            let counts: Vec<std::sync::Arc<std::sync::atomic::AtomicU64>> = (0..MULTI_PRODUCERS)
                .map(|_| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)))
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
                        let measure_deadline = Instant::now() + Duration::from_secs(MEASURE_SECS);
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
                value: total as f64 / MEASURE_SECS as f64,
                unit: "msg/s".to_string(),
                metadata: HashMap::new(),
            });
        }

        // Resource stats (process-level, not container)
        if let Some(pid) = server.pid() {
            if let Some(rss) = process_rss_bytes(pid) {
                report.add(BenchResult {
                    name: "fila_memory_mb".to_string(),
                    value: rss as f64 / (1024.0 * 1024.0),
                    unit: "MB".to_string(),
                    metadata: HashMap::new(),
                });
            }
        }
    }
}

// --- Shared utilities ---

/// Get CPU% and memory MB from a Docker container via `docker stats`.
fn container_stats(container_name: &str) -> Option<(f64, f64)> {
    let output = std::process::Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.CPUPerc}}\t{{.MemUsage}}",
        ])
        .arg(container_name)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = text.trim().split('\t').collect();
    if parts.len() < 2 {
        return None;
    }
    let cpu_pct: f64 = parts[0].trim_end_matches('%').parse().ok()?;
    let mem_str = parts[1].split('/').next()?.trim();
    let mem_mb = if mem_str.contains("GiB") {
        mem_str.replace("GiB", "").trim().parse::<f64>().ok()? * 1024.0
    } else if mem_str.contains("MiB") {
        mem_str.replace("MiB", "").trim().parse::<f64>().ok()?
    } else if mem_str.contains("KiB") {
        mem_str.replace("KiB", "").trim().parse::<f64>().ok()? / 1024.0
    } else {
        0.0
    };
    Some((cpu_pct, mem_mb))
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
