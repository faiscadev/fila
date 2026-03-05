#!/usr/bin/env python3
"""Competitive benchmark suite for Fila vs Kafka, RabbitMQ, and NATS.

Runs identical workloads against each broker and produces JSON reports
compatible with Fila's BenchReport schema.

Usage:
    python bench.py kafka          # benchmark Kafka only
    python bench.py rabbitmq       # benchmark RabbitMQ only
    python bench.py nats           # benchmark NATS only
    python bench.py all            # benchmark all competitors
"""

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone


# --- Configuration ---

WARMUP_SECS = 1
MEASURE_SECS = 3
SAMPLES_PER_LATENCY_LEVEL = 100
MESSAGE_SIZES = {
    "64B": b"x" * 64,
    "1KB": b"x" * 1024,
    "64KB": b"x" * 65536,
}
FAN_OUT_CONSUMERS = 3
MULTI_PRODUCERS = 3


# --- Report types ---


@dataclass
class BenchResult:
    name: str
    value: float
    unit: str
    metadata: dict = field(default_factory=dict)


@dataclass
class BenchReport:
    version: str = "1.0"
    timestamp: str = ""
    commit: str = ""
    benchmarks: list = field(default_factory=list)

    def __post_init__(self):
        if not self.timestamp:
            self.timestamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        if not self.commit:
            try:
                self.commit = (
                    subprocess.check_output(
                        ["git", "rev-parse", "--short", "HEAD"], stderr=subprocess.DEVNULL
                    )
                    .decode()
                    .strip()
                )
            except Exception:
                self.commit = "unknown"

    def add(self, result: BenchResult):
        self.benchmarks.append(result)

    def to_dict(self):
        return {
            "version": self.version,
            "timestamp": self.timestamp,
            "commit": self.commit,
            "benchmarks": [
                {"name": b.name, "value": b.value, "unit": b.unit, **({"metadata": b.metadata} if b.metadata else {})}
                for b in self.benchmarks
            ],
        }

    def write(self, path: str):
        with open(path, "w") as f:
            json.dump(self.to_dict(), f, indent=2)
        print(f"\nResults written to: {path}")


# --- Resource monitoring ---


def get_container_stats(container_name: str) -> dict:
    """Get CPU and memory stats for a Docker container."""
    try:
        result = subprocess.run(
            ["docker", "stats", "--no-stream", "--format", "{{.CPUPerc}}\t{{.MemUsage}}", container_name],
            capture_output=True,
            text=True,
            timeout=10,
        )
        if result.returncode == 0 and result.stdout.strip():
            parts = result.stdout.strip().split("\t")
            cpu_pct = float(parts[0].replace("%", ""))
            mem_str = parts[1].split("/")[0].strip()
            # Parse memory (e.g., "123.4MiB" or "1.2GiB")
            if "GiB" in mem_str:
                mem_mb = float(mem_str.replace("GiB", "")) * 1024
            elif "MiB" in mem_str:
                mem_mb = float(mem_str.replace("MiB", ""))
            elif "KiB" in mem_str:
                mem_mb = float(mem_str.replace("KiB", "")) / 1024
            else:
                mem_mb = 0.0
            return {"cpu_pct": cpu_pct, "mem_mb": mem_mb}
    except Exception:
        pass
    return {"cpu_pct": 0.0, "mem_mb": 0.0}


# --- Kafka benchmarks ---


def bench_kafka(report: BenchReport):
    """Run benchmarks against Kafka."""
    from confluent_kafka import Consumer, Producer
    from confluent_kafka.admin import AdminClient, NewTopic

    broker = "localhost:9092"
    admin = AdminClient({"bootstrap.servers": broker})

    def create_topic(name: str, partitions: int = 1):
        """Create a topic, deleting it first if it exists."""
        # Delete if exists
        try:
            admin.delete_topics([name])
            time.sleep(1)
        except Exception:
            pass
        fs = admin.create_topics([NewTopic(name, num_partitions=partitions)])
        for _, f in fs.items():
            f.result(timeout=10)
        time.sleep(0.5)

    def cleanup_topic(name: str):
        try:
            admin.delete_topics([name])
        except Exception:
            pass

    print("[kafka] Throughput benchmarks...")
    for size_name, payload in MESSAGE_SIZES.items():
        topic = f"bench-throughput-{size_name.lower()}"
        create_topic(topic)

        producer = Producer({"bootstrap.servers": broker, "linger.ms": 5, "batch.num.messages": 1000})

        # Warmup
        end_warmup = time.time() + WARMUP_SECS
        while time.time() < end_warmup:
            producer.produce(topic, payload)
            producer.poll(0)
        producer.flush()

        # Measure
        count = 0
        start = time.time()
        end_time = start + MEASURE_SECS
        while time.time() < end_time:
            producer.produce(topic, payload)
            producer.poll(0)
            count += 1
        producer.flush()
        elapsed = time.time() - start

        throughput = count / elapsed
        report.add(BenchResult(f"kafka_throughput_{size_name.lower()}", throughput, "msg/s"))
        cleanup_topic(topic)

    # Latency benchmark (1KB)
    print("[kafka] Latency benchmark...")
    topic = "bench-latency"
    create_topic(topic)
    producer = Producer({"bootstrap.servers": broker, "linger.ms": 0})
    consumer = Consumer({
        "bootstrap.servers": broker,
        "group.id": "bench-latency-group",
        "auto.offset.reset": "latest",
        "enable.auto.commit": True,
    })
    consumer.subscribe([topic])
    # Prime the consumer
    consumer.poll(2.0)

    latencies = []
    payload = MESSAGE_SIZES["1KB"]
    for _ in range(SAMPLES_PER_LATENCY_LEVEL):
        start = time.time()
        producer.produce(topic, payload)
        producer.flush()
        msg = consumer.poll(5.0)
        if msg and not msg.error():
            latencies.append((time.time() - start) * 1000)  # ms

    consumer.close()
    cleanup_topic(topic)

    if latencies:
        latencies.sort()
        report.add(BenchResult("kafka_latency_p50", latencies[len(latencies) // 2], "ms"))
        report.add(BenchResult("kafka_latency_p95", latencies[int(len(latencies) * 0.95)], "ms"))
        report.add(BenchResult("kafka_latency_p99", latencies[int(len(latencies) * 0.99)], "ms"))

    # E2E lifecycle: produce → consume → commit
    print("[kafka] Lifecycle throughput...")
    topic = "bench-lifecycle"
    create_topic(topic)
    producer = Producer({"bootstrap.servers": broker, "linger.ms": 0})

    # Pre-load
    for _ in range(1000):
        producer.produce(topic, MESSAGE_SIZES["1KB"])
    producer.flush()

    consumer = Consumer({
        "bootstrap.servers": broker,
        "group.id": "bench-lifecycle-group",
        "auto.offset.reset": "earliest",
        "enable.auto.commit": False,
    })
    consumer.subscribe([topic])
    time.sleep(1)

    count = 0
    start = time.time()
    while count < 1000:
        msg = consumer.poll(5.0)
        if msg and not msg.error():
            consumer.commit(msg)
            count += 1
    elapsed = time.time() - start
    consumer.close()
    cleanup_topic(topic)

    report.add(BenchResult("kafka_lifecycle_throughput", count / elapsed, "msg/s"))

    # Multi-producer throughput (3 producers, 1KB)
    print("[kafka] Multi-producer throughput...")
    topic = "bench-multi-producer"
    create_topic(topic)
    payload = MESSAGE_SIZES["1KB"]
    counts = [0] * MULTI_PRODUCERS

    def kafka_producer_worker(idx):
        p = Producer({"bootstrap.servers": broker, "linger.ms": 5, "batch.num.messages": 1000})
        end_time = time.time() + MEASURE_SECS
        while time.time() < end_time:
            p.produce(topic, payload)
            p.poll(0)
            counts[idx] += 1
        p.flush()

    threads = [threading.Thread(target=kafka_producer_worker, args=(i,)) for i in range(MULTI_PRODUCERS)]
    start = time.time()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    elapsed = time.time() - start
    report.add(BenchResult("kafka_multi_producer_throughput", sum(counts) / elapsed, "msg/s"))
    cleanup_topic(topic)

    # Fan-out (1 producer, 3 consumer groups)
    print("[kafka] Fan-out throughput...")
    topic = "bench-fanout"
    create_topic(topic)
    payload = MESSAGE_SIZES["1KB"]
    fanout_counts = [0] * FAN_OUT_CONSUMERS

    def kafka_fanout_consumer(idx):
        c = Consumer({
            "bootstrap.servers": broker,
            "group.id": f"bench-fanout-group-{idx}",
            "auto.offset.reset": "earliest",
            "enable.auto.commit": True,
        })
        c.subscribe([topic])
        while fanout_counts[idx] < 500:
            msg = c.poll(5.0)
            if msg and not msg.error():
                fanout_counts[idx] += 1
        c.close()

    # Pre-load messages
    p = Producer({"bootstrap.servers": broker, "linger.ms": 5})
    for _ in range(500):
        p.produce(topic, payload)
    p.flush()
    time.sleep(0.5)

    threads = [threading.Thread(target=kafka_fanout_consumer, args=(i,)) for i in range(FAN_OUT_CONSUMERS)]
    start = time.time()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    elapsed = time.time() - start
    total_delivered = sum(fanout_counts)
    report.add(BenchResult("kafka_fanout_throughput", total_delivered / elapsed, "msg/s"))
    cleanup_topic(topic)

    # Resource stats
    stats = get_container_stats("competitive-kafka-1")
    report.add(BenchResult("kafka_cpu_pct", stats["cpu_pct"], "%"))
    report.add(BenchResult("kafka_memory_mb", stats["mem_mb"], "MB"))


# --- RabbitMQ benchmarks ---


def bench_rabbitmq(report: BenchReport):
    """Run benchmarks against RabbitMQ."""
    import pika

    conn_params = pika.ConnectionParameters(
        host="localhost",
        port=5672,
        credentials=pika.PlainCredentials("bench", "bench"),
    )

    print("[rabbitmq] Throughput benchmarks...")
    for size_name, payload in MESSAGE_SIZES.items():
        queue = f"bench-throughput-{size_name.lower()}"
        connection = pika.BlockingConnection(conn_params)
        channel = connection.channel()
        channel.queue_declare(queue=queue, durable=True, arguments={"x-queue-type": "quorum"})
        channel.queue_purge(queue)

        # Warmup
        end_warmup = time.time() + WARMUP_SECS
        while time.time() < end_warmup:
            channel.basic_publish(exchange="", routing_key=queue, body=payload)

        # Measure
        count = 0
        start = time.time()
        end_time = start + MEASURE_SECS
        while time.time() < end_time:
            channel.basic_publish(exchange="", routing_key=queue, body=payload)
            count += 1
        elapsed = time.time() - start

        throughput = count / elapsed
        report.add(BenchResult(f"rabbitmq_throughput_{size_name.lower()}", throughput, "msg/s"))
        channel.queue_delete(queue)
        connection.close()

    # Latency benchmark (1KB)
    print("[rabbitmq] Latency benchmark...")
    queue = "bench-latency"
    connection = pika.BlockingConnection(conn_params)
    channel = connection.channel()
    channel.queue_declare(queue=queue, durable=True, arguments={"x-queue-type": "quorum"})
    channel.queue_purge(queue)

    latencies = []
    payload = MESSAGE_SIZES["1KB"]
    for _ in range(SAMPLES_PER_LATENCY_LEVEL):
        start_t = time.time()
        channel.basic_publish(exchange="", routing_key=queue, body=payload)
        method, _, body = channel.basic_get(queue=queue, auto_ack=False)
        if method:
            channel.basic_ack(method.delivery_tag)
            latencies.append((time.time() - start_t) * 1000)

    channel.queue_delete(queue)
    connection.close()

    if latencies:
        latencies.sort()
        report.add(BenchResult("rabbitmq_latency_p50", latencies[len(latencies) // 2], "ms"))
        report.add(BenchResult("rabbitmq_latency_p95", latencies[int(len(latencies) * 0.95)], "ms"))
        report.add(BenchResult("rabbitmq_latency_p99", latencies[int(len(latencies) * 0.99)], "ms"))

    # E2E lifecycle: publish → get → ack
    print("[rabbitmq] Lifecycle throughput...")
    queue = "bench-lifecycle"
    connection = pika.BlockingConnection(conn_params)
    channel = connection.channel()
    channel.queue_declare(queue=queue, durable=True, arguments={"x-queue-type": "quorum"})
    channel.queue_purge(queue)

    payload = MESSAGE_SIZES["1KB"]
    for _ in range(1000):
        channel.basic_publish(exchange="", routing_key=queue, body=payload)

    count = 0
    start = time.time()
    while count < 1000:
        method, _, body = channel.basic_get(queue=queue, auto_ack=False)
        if method:
            channel.basic_ack(method.delivery_tag)
            count += 1
        else:
            time.sleep(0.01)
    elapsed = time.time() - start

    channel.queue_delete(queue)
    connection.close()

    report.add(BenchResult("rabbitmq_lifecycle_throughput", count / elapsed, "msg/s"))

    # Multi-producer throughput (3 producers, 1KB)
    print("[rabbitmq] Multi-producer throughput...")
    queue = "bench-multi-producer"
    connection = pika.BlockingConnection(conn_params)
    channel = connection.channel()
    channel.queue_declare(queue=queue, durable=True, arguments={"x-queue-type": "quorum"})
    channel.queue_purge(queue)
    connection.close()
    payload = MESSAGE_SIZES["1KB"]
    counts = [0] * MULTI_PRODUCERS

    def rmq_producer_worker(idx):
        conn = pika.BlockingConnection(conn_params)
        ch = conn.channel()
        end_time = time.time() + MEASURE_SECS
        while time.time() < end_time:
            ch.basic_publish(exchange="", routing_key=queue, body=payload)
            counts[idx] += 1
        conn.close()

    threads = [threading.Thread(target=rmq_producer_worker, args=(i,)) for i in range(MULTI_PRODUCERS)]
    start = time.time()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    elapsed = time.time() - start
    report.add(BenchResult("rabbitmq_multi_producer_throughput", sum(counts) / elapsed, "msg/s"))

    connection = pika.BlockingConnection(conn_params)
    channel = connection.channel()
    channel.queue_delete(queue)
    connection.close()

    # Fan-out (1 producer, 3 consumers via fanout exchange)
    print("[rabbitmq] Fan-out throughput...")
    exchange = "bench-fanout"
    connection = pika.BlockingConnection(conn_params)
    channel = connection.channel()
    channel.exchange_declare(exchange=exchange, exchange_type="fanout", durable=True)
    fanout_queues = []
    for i in range(FAN_OUT_CONSUMERS):
        q = f"bench-fanout-{i}"
        channel.queue_declare(queue=q, durable=True, arguments={"x-queue-type": "quorum"})
        channel.queue_purge(q)
        channel.queue_bind(queue=q, exchange=exchange)
        fanout_queues.append(q)

    # Pre-load via exchange
    payload = MESSAGE_SIZES["1KB"]
    for _ in range(500):
        channel.basic_publish(exchange=exchange, routing_key="", body=payload)
    connection.close()
    time.sleep(0.5)

    fanout_counts = [0] * FAN_OUT_CONSUMERS

    def rmq_fanout_consumer(idx):
        conn = pika.BlockingConnection(conn_params)
        ch = conn.channel()
        q = fanout_queues[idx]
        while fanout_counts[idx] < 500:
            method, _, body = ch.basic_get(queue=q, auto_ack=True)
            if method:
                fanout_counts[idx] += 1
            else:
                time.sleep(0.01)
        conn.close()

    threads = [threading.Thread(target=rmq_fanout_consumer, args=(i,)) for i in range(FAN_OUT_CONSUMERS)]
    start = time.time()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    elapsed = time.time() - start
    total_delivered = sum(fanout_counts)
    report.add(BenchResult("rabbitmq_fanout_throughput", total_delivered / elapsed, "msg/s"))

    connection = pika.BlockingConnection(conn_params)
    channel = connection.channel()
    channel.exchange_delete(exchange)
    for q in fanout_queues:
        channel.queue_delete(q)
    connection.close()

    # Resource stats
    stats = get_container_stats("competitive-rabbitmq-1")
    report.add(BenchResult("rabbitmq_cpu_pct", stats["cpu_pct"], "%"))
    report.add(BenchResult("rabbitmq_memory_mb", stats["mem_mb"], "MB"))


# --- NATS benchmarks ---


def bench_nats(report: BenchReport):
    """Run benchmarks against NATS JetStream."""
    import asyncio

    async def _bench_nats():
        import nats
        from nats.js.api import StreamConfig, ConsumerConfig, AckPolicy

        nc = await nats.connect("nats://localhost:4222")
        js = nc.jetstream()

        async def ensure_stream(name: str, subjects: list[str]):
            try:
                await js.delete_stream(name)
            except Exception:
                pass
            await js.add_stream(StreamConfig(name=name, subjects=subjects))

        print("[nats] Throughput benchmarks...")
        for size_name, payload in MESSAGE_SIZES.items():
            stream = f"bench-tp-{size_name.lower()}"
            subject = f"bench.tp.{size_name.lower()}"
            await ensure_stream(stream, [subject])

            # Warmup
            end_warmup = time.time() + WARMUP_SECS
            while time.time() < end_warmup:
                await js.publish(subject, payload)

            # Measure
            count = 0
            start = time.time()
            end_time = start + MEASURE_SECS
            while time.time() < end_time:
                await js.publish(subject, payload)
                count += 1
            elapsed = time.time() - start

            throughput = count / elapsed
            report.add(BenchResult(f"nats_throughput_{size_name.lower()}", throughput, "msg/s"))
            await js.delete_stream(stream)

        # Latency benchmark (1KB)
        print("[nats] Latency benchmark...")
        stream = "bench-latency"
        subject = "bench.latency"
        await ensure_stream(stream, [subject])

        sub = await js.pull_subscribe(subject, durable="bench-lat-consumer")

        latencies = []
        payload = MESSAGE_SIZES["1KB"]
        for _ in range(SAMPLES_PER_LATENCY_LEVEL):
            start_t = time.time()
            await js.publish(subject, payload)
            try:
                msgs = await sub.fetch(1, timeout=5)
                if msgs:
                    await msgs[0].ack()
                    latencies.append((time.time() - start_t) * 1000)
            except Exception:
                pass

        await js.delete_stream(stream)

        if latencies:
            latencies.sort()
            report.add(BenchResult("nats_latency_p50", latencies[len(latencies) // 2], "ms"))
            report.add(BenchResult("nats_latency_p95", latencies[int(len(latencies) * 0.95)], "ms"))
            report.add(BenchResult("nats_latency_p99", latencies[int(len(latencies) * 0.99)], "ms"))

        # E2E lifecycle: publish → consume → ack
        print("[nats] Lifecycle throughput...")
        stream = "bench-lifecycle"
        subject = "bench.lifecycle"
        await ensure_stream(stream, [subject])

        payload = MESSAGE_SIZES["1KB"]
        for _ in range(1000):
            await js.publish(subject, payload)

        sub = await js.pull_subscribe(subject, durable="bench-lc-consumer")

        count = 0
        start = time.time()
        while count < 1000:
            try:
                msgs = await sub.fetch(10, timeout=5)
                for msg in msgs:
                    await msg.ack()
                    count += 1
            except Exception:
                pass
        elapsed = time.time() - start

        await js.delete_stream(stream)
        await nc.close()

        report.add(BenchResult("nats_lifecycle_throughput", count / elapsed, "msg/s"))

        # Multi-producer throughput (3 producers, 1KB)
        print("[nats] Multi-producer throughput...")
        stream = "bench-multi-producer"
        subject = "bench.mp"
        await ensure_stream(stream, [subject])
        payload = MESSAGE_SIZES["1KB"]

        async def nats_producer_coro():
            nc2 = await nats.connect("nats://localhost:4222")
            js2 = nc2.jetstream()
            c = 0
            end_time = time.time() + MEASURE_SECS
            while time.time() < end_time:
                await js2.publish(subject, payload)
                c += 1
            await nc2.close()
            return c

        import asyncio
        start = time.time()
        results = await asyncio.gather(*[nats_producer_coro() for _ in range(MULTI_PRODUCERS)])
        elapsed = time.time() - start
        report.add(BenchResult("nats_multi_producer_throughput", sum(results) / elapsed, "msg/s"))
        await js.delete_stream(stream)

        # Fan-out (1 producer, 3 consumers on same stream)
        print("[nats] Fan-out throughput...")
        stream = "bench-fanout"
        subject = "bench.fanout"
        await ensure_stream(stream, [subject])
        payload = MESSAGE_SIZES["1KB"]

        # Pre-load
        for _ in range(500):
            await js.publish(subject, payload)

        # Each consumer reads all 500 messages from their own durable
        async def nats_fanout_consumer(idx):
            nc2 = await nats.connect("nats://localhost:4222")
            js2 = nc2.jetstream()
            sub = await js2.pull_subscribe(subject, durable=f"bench-fo-{idx}")
            c = 0
            while c < 500:
                try:
                    msgs = await sub.fetch(10, timeout=5)
                    for msg in msgs:
                        await msg.ack()
                        c += 1
                except Exception:
                    pass
            await nc2.close()
            return c

        start = time.time()
        results = await asyncio.gather(*[nats_fanout_consumer(i) for i in range(FAN_OUT_CONSUMERS)])
        elapsed = time.time() - start
        total_delivered = sum(results)
        report.add(BenchResult("nats_fanout_throughput", total_delivered / elapsed, "msg/s"))
        await js.delete_stream(stream)

        # Resource stats
        stats = get_container_stats("competitive-nats-1")
        report.add(BenchResult("nats_cpu_pct", stats["cpu_pct"], "%"))
        report.add(BenchResult("nats_memory_mb", stats["mem_mb"], "MB"))

    asyncio.run(_bench_nats())


# --- Print summary ---


def print_summary(report: BenchReport, broker: str):
    """Print a human-readable summary of benchmark results."""
    print(f"\n{'=' * 60}")
    print(f"  {broker.upper()} Benchmark Results")
    print(f"  Time: {report.timestamp}")
    print(f"{'=' * 60}\n")

    for b in report.benchmarks:
        if b.name.startswith(broker):
            print(f"  {b.name:<50} {b.value:>12.2f} {b.unit}")

    print(f"\n{'=' * 60}\n")


# --- Main ---


def main():
    parser = argparse.ArgumentParser(description="Competitive benchmark suite")
    parser.add_argument(
        "broker",
        choices=["kafka", "rabbitmq", "nats", "all"],
        help="Which broker(s) to benchmark",
    )
    parser.add_argument(
        "--output-dir",
        default=".",
        help="Directory to write result JSON files (default: current dir)",
    )
    args = parser.parse_args()

    brokers = ["kafka", "rabbitmq", "nats"] if args.broker == "all" else [args.broker]

    for broker in brokers:
        print(f"\n{'#' * 60}")
        print(f"  Benchmarking {broker.upper()}")
        print(f"{'#' * 60}\n")

        report = BenchReport()

        try:
            if broker == "kafka":
                bench_kafka(report)
            elif broker == "rabbitmq":
                bench_rabbitmq(report)
            elif broker == "nats":
                bench_nats(report)
        except Exception as e:
            print(f"\nERROR benchmarking {broker}: {e}", file=sys.stderr)
            print("Is the broker running? Try: docker compose up -d", file=sys.stderr)
            sys.exit(1)

        print_summary(report, broker)

        output_path = os.path.join(args.output_dir, f"bench-{broker}.json")
        report.write(output_path)


if __name__ == "__main__":
    main()
