.PHONY: jaeger-up jaeger-down jaeger-logs jaeger-ui run-otel

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

run-otel:
	OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
	OTEL_SERVICE_NAME=security_flamegraph_lowlatency \
	cargo run --features otel -- --pair $(PAIR) $(EXTRA)
