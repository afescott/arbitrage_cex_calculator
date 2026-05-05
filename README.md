# Security Flamegraph Low-Latency

A low-latency Rust application with performance profiling and security best practices.

## Features

- **Low-latency optimizations**: Zero-copy, lock-free data structures, CPU affinity
- **Performance profiling**: Flamegraph integration for CPU profiling
- **Async monitoring**: Tokio-console for runtime observability
- **Security**: Input validation, secure defaults, dependency auditing

## Quick Start

```bash
# Build with optimizations
cargo build --release

# Run with flamegraph
cargo flamegraph --bin security_flamegraph_lowlatency

# Run with tokio-console (requires RUSTFLAGS)
RUSTFLAGS="-C force-frame-pointers=y" cargo run --bin security_flamegraph_lowlatency
```

## Run (CLI)

Arguments after `--` are passed to the binary. `--pair` is required; `--bias` defaults to `buy` and `--budget` defaults to `10` (USD).

```bash
# Minimal
cargo run -- --pair HYPE/USDT

# Explicit buy bias and $10 budget (same as defaults)
cargo run -- --pair HYPE/USDT --bias buy --budget 10

# Sell bias, $50 budget
cargo run -- --pair HYPE/USDT --bias sell --budget 50
```

Optional exchange credentials (prefer env vars or a secrets manager in production—CLI args can show up in process listings):

```bash
cargo run -- \
  --pair HYPE/USDT \
  --bias buy \
  --budget 10 \
  --hyperliquid-private-key YOUR_HL_PRIVATE_KEY \
  --kraken-api-key YOUR_KRAKEN_KEY \
  --kraken-api-secret YOUR_KRAKEN_SECRET \
  --binance-api-key YOUR_BINANCE_KEY \
  --binance-api-secret YOUR_BINANCE_SECRET
```

Supported flags: `--pair`, `--bias` (`buy` | `sell`), `--budget` (integer USD), `--hyperliquid-private-key`, `--kraken-api-key`, `--kraken-api-secret`, `--binance-api-key`, `--binance-api-secret`.

## Development Tools

### Flamegraph
```bash
cargo install flamegraph
cargo flamegraph --bin security_flamegraph_lowlatency
```

### Tokio Console
```bash
cargo install tokio-console
# In one terminal:
tokio-console
# In another terminal:
RUSTFLAGS="-C force-frame-pointers=y" cargo run --bin security_flamegraph_lowlatency
```

## Project Ideas

### Recommended: Order Book Aggregator
- Aggregate order books from multiple exchanges (Binance, Coinbase, Kraken)
- Real-time best bid/offer (BBO) calculation
- Sub-millisecond latency requirements
- Perfect for profiling websocket parsing and order book updates

### Other Ideas
- **Arbitrage Scanner**: Monitor price differences across exchanges for opportunities
- **Mempool Monitor**: Analyze blockchain mempools for MEV opportunities
- **Market Data Feed Handler**: High-throughput websocket processing with zero-copy deserialization
- **Smart Order Router**: Intelligent order routing with TWAP/VWAP algorithms

## Recommendations

- Add `cargo-audit` for dependency security scanning
- Consider `jemalloc` for better memory allocation performance
- Use `perf` for system-level profiling
- Enable LTO (`lto = true` in `Cargo.toml`) for release builds
- For crypto trading: Use `tokio-tungstenite` for websockets, `serde` for fast deserialization
- Consider `crossbeam` for lock-free data structures

