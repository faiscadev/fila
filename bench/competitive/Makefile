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
	@echo "Waiting for Kafka to be ready..."
	@$(COMPOSE) exec kafka bash -c 'until /opt/kafka/bin/kafka-broker-api-versions.sh --bootstrap-server localhost:9092 > /dev/null 2>&1; do sleep 1; done'
	python3 bench.py kafka --output-dir $(RESULTS_DIR)

bench-rabbitmq: setup
	$(COMPOSE) up -d rabbitmq
	@echo "Waiting for RabbitMQ to be ready..."
	@until $(COMPOSE) exec rabbitmq rabbitmq-diagnostics -q ping > /dev/null 2>&1; do sleep 1; done
	python3 bench.py rabbitmq --output-dir $(RESULTS_DIR)

bench-nats: setup
	$(COMPOSE) up -d nats
	@echo "Waiting for NATS to be ready..."
	@until curl -sf http://localhost:8222/healthz > /dev/null 2>&1; do sleep 1; done
	python3 bench.py nats --output-dir $(RESULTS_DIR)

bench-fila: setup
	./bench-fila.sh $(RESULTS_DIR)

bench-clean:
	$(COMPOSE) down -v
	rm -rf $(RESULTS_DIR)
