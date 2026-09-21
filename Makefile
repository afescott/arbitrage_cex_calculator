.PHONY: jaeger-up jaeger-down jaeger-logs jaeger-ui run-otel baseline-otel

# --- Observability backend (local Jaeger via docker compose) ----------------

jaeger-up:
	docker compose up -d jaeger
	@echo "Jaeger UI: http://localhost:16686"

jaeger-down:
	docker compose down

jaeger-logs:
	docker compose logs -f jaeger

jaeger-ui:
	@echo "Jaeger UI: http://localhost:16686"

# --- Convenience: run the app with OTel export enabled ----------------------

# Override with `make run-otel PAIR=SOL/USDT EXTRA="--budget 25"`.
PAIR ?= HYPE/USDT
EXTRA ?=
RUN_SECONDS ?= 60

run-otel:
	OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
	OTEL_SERVICE_NAME=security_flamegraph_lowlatency \
	cargo run --features otel -- --pair $(PAIR) $(EXTRA)

# Timed dry-run baseline: stderr histograms every 5s + final snapshot on exit.
# Copy the last [telemetry] block into docs/runs/ (see docs/runs/README.md).
baseline-otel:
	@mkdir -p docs/runs
	@test -f docs/runs/TEMPLATE.csv || { echo "missing docs/runs/TEMPLATE.csv"; exit 1; }
	@echo "Jaeger UI: http://localhost:16686"
	@echo "After exit: cp docs/runs/TEMPLATE.csv docs/runs/\$$(date -u +%Y%m%d-%H%M%S)-baseline.csv"
	@echo "           then fill from the final [telemetry] stderr block (latencies in ns)."
	OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
	OTEL_SERVICE_NAME=security_flamegraph_lowlatency \
	cargo run --release --features otel -- \
	  --pair $(PAIR) --execute-live 0 --run-seconds $(RUN_SECONDS) $(EXTRA)
