# Competitive benchmark orchestration.
#
# Prerequisites:
#   - Docker (with compose plugin)
#   - Python 3.10+ with: pip install -r requirements.txt
#   - Rust toolchain (for Fila benchmarks)
#
# Usage:
#   make bench-competitive    # run all brokers + Fila
#   make bench-kafka          # benchmark Kafka only
#   make bench-rabbitmq       # benchmark RabbitMQ only
#   make bench-nats           # benchmark NATS only
#   make bench-fila           # benchmark Fila only
#   make bench-clean          # teardown containers

RESULTS_DIR := results
COMPOSE := docker compose
READY_TIMEOUT := 60

.PHONY: bench-competitive bench-kafka bench-rabbitmq bench-nats bench-fila bench-clean setup

setup:
	@mkdir -p $(RESULTS_DIR)

bench-competitive: setup bench-fila bench-kafka bench-rabbitmq bench-nats
	@echo ""
	@echo "============================================================"
	@echo "  All competitive benchmarks complete."
	@echo "  Results in $(RESULTS_DIR)/"
	@echo "============================================================"
	@ls -la $(RESULTS_DIR)/bench-*.json

bench-kafka: setup
	$(COMPOSE) up -d kafka
	@echo "Waiting for Kafka to be ready (timeout $(READY_TIMEOUT)s)..."
	@i=0; while [ $$i -lt $(READY_TIMEOUT) ]; do \
		if $(COMPOSE) exec kafka /opt/kafka/bin/kafka-broker-api-versions.sh --bootstrap-server localhost:9092 > /dev/null 2>&1; then break; fi; \
		sleep 1; i=$$((i+1)); \
	done; \
	if [ $$i -ge $(READY_TIMEOUT) ]; then echo "ERROR: Kafka not ready after $(READY_TIMEOUT)s"; exit 1; fi
	python3 bench.py kafka --output-dir $(RESULTS_DIR)

bench-rabbitmq: setup
	$(COMPOSE) up -d rabbitmq
	@echo "Waiting for RabbitMQ to be ready (timeout $(READY_TIMEOUT)s)..."
	@i=0; while [ $$i -lt $(READY_TIMEOUT) ]; do \
		if $(COMPOSE) exec rabbitmq rabbitmq-diagnostics -q ping > /dev/null 2>&1; then break; fi; \
		sleep 1; i=$$((i+1)); \
	done; \
	if [ $$i -ge $(READY_TIMEOUT) ]; then echo "ERROR: RabbitMQ not ready after $(READY_TIMEOUT)s"; exit 1; fi
	python3 bench.py rabbitmq --output-dir $(RESULTS_DIR)

bench-nats: setup
	$(COMPOSE) up -d nats
	@echo "Waiting for NATS to be ready (timeout $(READY_TIMEOUT)s)..."
	@i=0; while [ $$i -lt $(READY_TIMEOUT) ]; do \
		if curl -sf http://localhost:8222/healthz > /dev/null 2>&1; then break; fi; \
		sleep 1; i=$$((i+1)); \
	done; \
	if [ $$i -ge $(READY_TIMEOUT) ]; then echo "ERROR: NATS not ready after $(READY_TIMEOUT)s"; exit 1; fi
	python3 bench.py nats --output-dir $(RESULTS_DIR)

bench-fila: setup
	./bench-fila.sh $(RESULTS_DIR)

bench-clean:
	$(COMPOSE) down -v
	rm -rf $(RESULTS_DIR)
