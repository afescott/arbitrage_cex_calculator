# Latency baseline runs

One CSV per timed run. Copy the template, fill metadata from how you launched the binary, and paste percentiles from the final `[telemetry]` stderr block (printed every 5s and once on shutdown).

## Workflow

1. Start Jaeger (optional, for OTel traces): `make jaeger-up`
2. Copy the template:

```bash
cp docs/runs/TEMPLATE.csv "docs/runs/$(date -u +%Y%m%d-%H%M%S)-baseline.csv"
```

3. Run a timed baseline (dry-run default):

```bash
make baseline-otel PAIR=HYPE/USDT RUN_SECONDS=60
```

Or without Make:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
OTEL_SERVICE_NAME=security_flamegraph_lowlatency \
cargo run --release --features otel -- \
  --pair HYPE/USDT --execute-live 0 --run-seconds 60
```

4. On shutdown, copy the last telemetry snapshot into the CSV:
   - Counters → `ws_msgs`, `arb_found`, `routes_*`, `orders_*`
   - Each stage line → `*_n`, `*_p50_ns`, `*_p99_ns`, `*_p999_ns`, `*_max_ns`
   - Reporter prints human units (`12.3us`); convert to **nanoseconds** in the CSV (`12300`)
5. Set `git_sha` (`git rev-parse --short HEAD`), `features`, `notes`, and open Jaeger at http://localhost:16686 for any arb-tick spans

## File naming

`YYYYMMDD-HHMMSS-<label>.csv` — e.g. `20260921-021500-hype-otel-dry.csv`

## Units

All latency columns are **integer nanoseconds**. Leave a stage blank if `n=0` (no samples that run).
