# Competitive benchmark orchestration.
#
# Prerequisites:
#   - Docker (with compose plugin)
#   - Rust toolchain (for all benchmarks)
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
READY_TIMEOUT := 120
REPO_ROOT := $(shell cd ../.. && pwd)
BENCH_BIN := $(REPO_ROOT)/target/release/bench-competitive
AGG_BIN := $(REPO_ROOT)/target/release/bench-aggregate
RUNS := 3

.PHONY: bench-competitive bench-kafka bench-rabbitmq bench-nats bench-fila bench-clean setup build build-fila-image

setup:
	@mkdir -p $(RESULTS_DIR)

build:
	cd $(REPO_ROOT) && cargo build --release -p fila-server -p fila-cli -p fila-bench --bins
	cd $(REPO_ROOT) && cargo build --release -p fila-bench --features competitive --bin bench-competitive

bench-competitive: setup build bench-fila bench-kafka bench-rabbitmq bench-nats
	@echo ""
	@echo "============================================================"
	@echo "  All competitive benchmarks complete."
	@echo "  Results in $(RESULTS_DIR)/"
	@echo "============================================================"
	@ls -la $(RESULTS_DIR)/bench-*.json

bench-kafka: setup build
	$(COMPOSE) up -d kafka
	@echo "Waiting for Kafka to be ready (timeout $(READY_TIMEOUT)s)..."
	@i=0; while [ $$i -lt $(READY_TIMEOUT) ]; do \
		if $(COMPOSE) exec -T kafka /opt/kafka/bin/kafka-broker-api-versions.sh --bootstrap-server localhost:9092 > /dev/null 2>&1; then break; fi; \
		sleep 1; i=$$((i+1)); \
	done; \
	if [ $$i -ge $(READY_TIMEOUT) ]; then echo "ERROR: Kafka not ready after $(READY_TIMEOUT)s"; $(COMPOSE) logs kafka; exit 1; fi
	@for run in $$(seq 1 $(RUNS)); do \
		echo "[kafka] Run $$run/$(RUNS)..."; \
		$(BENCH_BIN) kafka $(RESULTS_DIR) && \
		mv $(RESULTS_DIR)/bench-kafka.json $(RESULTS_DIR)/bench-kafka-run-$$run.json; \
	done
	$(AGG_BIN) $(RESULTS_DIR)/bench-kafka.json $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-kafka-run-$$r.json; done)
	@rm -f $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-kafka-run-$$r.json; done)
	$(COMPOSE) down -v

bench-rabbitmq: setup build
	$(COMPOSE) up -d rabbitmq
	@echo "Waiting for RabbitMQ to be ready (timeout $(READY_TIMEOUT)s)..."
	@i=0; while [ $$i -lt $(READY_TIMEOUT) ]; do \
		if $(COMPOSE) exec -T rabbitmq rabbitmq-diagnostics -q check_port_connectivity > /dev/null 2>&1; then break; fi; \
		sleep 1; i=$$((i+1)); \
	done; \
	if [ $$i -ge $(READY_TIMEOUT) ]; then echo "ERROR: RabbitMQ not ready after $(READY_TIMEOUT)s"; $(COMPOSE) logs rabbitmq; exit 1; fi
	@for run in $$(seq 1 $(RUNS)); do \
		echo "[rabbitmq] Run $$run/$(RUNS)..."; \
		$(BENCH_BIN) rabbitmq $(RESULTS_DIR) && \
		mv $(RESULTS_DIR)/bench-rabbitmq.json $(RESULTS_DIR)/bench-rabbitmq-run-$$run.json; \
	done
	$(AGG_BIN) $(RESULTS_DIR)/bench-rabbitmq.json $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-rabbitmq-run-$$r.json; done)
	@rm -f $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-rabbitmq-run-$$r.json; done)
	$(COMPOSE) down -v

bench-nats: setup build
	$(COMPOSE) up -d nats
	@echo "Waiting for NATS to be ready (timeout $(READY_TIMEOUT)s)..."
	@i=0; while [ $$i -lt $(READY_TIMEOUT) ]; do \
		if curl -sf http://localhost:8222/healthz > /dev/null 2>&1; then break; fi; \
		sleep 1; i=$$((i+1)); \
	done; \
	if [ $$i -ge $(READY_TIMEOUT) ]; then echo "ERROR: NATS not ready after $(READY_TIMEOUT)s"; exit 1; fi
	@for run in $$(seq 1 $(RUNS)); do \
		echo "[nats] Run $$run/$(RUNS)..."; \
		$(BENCH_BIN) nats $(RESULTS_DIR) && \
		mv $(RESULTS_DIR)/bench-nats.json $(RESULTS_DIR)/bench-nats-run-$$run.json; \
	done
	$(AGG_BIN) $(RESULTS_DIR)/bench-nats.json $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-nats-run-$$r.json; done)
	@rm -f $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-nats-run-$$r.json; done)
	$(COMPOSE) down -v

build-fila-image:
	@echo "Building Fila Docker image (first build is slow, subsequent builds use cache)..."
	$(COMPOSE) build fila

bench-fila: setup build build-fila-image
	$(COMPOSE) up -d fila
	@echo "Waiting for Fila to be ready (timeout $(READY_TIMEOUT)s)..."
	@i=0; while [ $$i -lt $(READY_TIMEOUT) ]; do \
		if $(REPO_ROOT)/target/release/fila --addr http://localhost:5555 queue list > /dev/null 2>&1; then break; fi; \
		sleep 1; i=$$((i+1)); \
	done; \
	if [ $$i -ge $(READY_TIMEOUT) ]; then echo "ERROR: Fila not ready after $(READY_TIMEOUT)s"; $(COMPOSE) logs fila; $(COMPOSE) down -v; exit 1; fi
	@for run in $$(seq 1 $(RUNS)); do \
		echo "[fila] Run $$run/$(RUNS)..."; \
		$(BENCH_BIN) fila $(RESULTS_DIR) && \
		mv $(RESULTS_DIR)/bench-fila.json $(RESULTS_DIR)/bench-fila-run-$$run.json; \
	done
	$(AGG_BIN) $(RESULTS_DIR)/bench-fila.json $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-fila-run-$$r.json; done)
	@rm -f $(shell for r in $$(seq 1 $(RUNS)); do echo $(RESULTS_DIR)/bench-fila-run-$$r.json; done)
	$(COMPOSE) down -v

bench-clean:
	$(COMPOSE) down -v
	rm -rf $(RESULTS_DIR)
