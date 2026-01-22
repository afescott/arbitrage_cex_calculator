# Low-Latency Optimization Roadmap

## Phase 1: Measurement & Baseline (Do This First!)

### 1.1 Add Latency Metrics
```rust
// Add to Cargo.toml:
// metrics = "0.21"
// metrics-prometheus = "0.3"

// Track latency from WebSocket receive → aggregator
```

### 1.2 Profile with Flamegraph
```bash
cargo install flamegraph
cargo flamegraph --bin security_flamegraph_lowlatency
# Open flamegraph.svg to see hotspots
```

### 1.3 Run Tokio Console
```bash
cargo install tokio-console
# Terminal 1:
tokio-console
# Terminal 2:
RUSTFLAGS="-C force-frame-pointers=y" cargo run
```

## Phase 2: High-Impact Optimizations

### 2.1 Remove Hot-Path Logging ⚡ (Biggest Win)
**Current Issue**: `info!()` in `handle_message()` adds ~1-10μs per message

**Fix**: Use conditional compilation or remove entirely
```rust
// Replace info!() with:
#[cfg(debug_assertions)]
tracing::debug!("[Binance] {}: ${}", symbol, price_str);
```

### 2.2 Replace serde_json::Value with Typed Structs ⚡
**Current Issue**: `Value` allocates heap memory for every field access

**Fix**: Use zero-copy deserialization
```rust
#[derive(Deserialize)]
struct BinanceTicker {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "c")]
    price: String,
}

// Then: serde_json::from_str::<BinanceTicker>(text)
```

### 2.3 Optimize Decimal Parsing ⚡
**Current Issue**: `f64` parsing + multiplication has overhead

**Fix**: Manual parsing or fixed-point
```rust
// Manual parsing (faster):
fn parse_price_cents(s: &str) -> Option<u64> {
    // Find decimal point, parse integer parts
    // Avoids f64 allocation
}
```

## Phase 3: System-Level Optimizations

### 3.1 CPU Affinity
```rust
// Pin tasks to specific cores
use core_affinity;

// Pin aggregator to core 0
core_affinity::set_for_current(core_affinity::CoreId { id: 0 });
```

### 3.2 Increase Channel Buffer
```rust
// Current: channel(100)
// Change to: channel(10_000) or unbounded_channel()
```

### 3.3 Consider Lock-Free Channels
```rust
// crossbeam-channel has lower overhead than tokio channels
use crossbeam_channel;
```

## Phase 4: Advanced Optimizations

### 4.1 SIMD JSON Parsing
- Use `simdjson` crate for 2-3x faster JSON parsing
- Or manual parsing for known formats

### 4.2 Memory Pool/Pre-allocation
- Pre-allocate buffers for WebSocket messages
- Reuse allocations where possible

### 4.3 Zero-Copy Deserialization
- Use `serde_json::from_slice` with borrowed strings
- Avoid `String` allocations

## Recommended Implementation Order

1. ✅ **Remove hot-path logging** (5 min, huge impact)
2. ✅ **Add typed structs** (15 min, big impact)
3. ✅ **Profile with flamegraph** (10 min, identify bottlenecks)
4. ✅ **Add metrics** (30 min, measure improvements)
5. ✅ **CPU affinity** (20 min, consistent latency)
6. ✅ **Channel optimizations** (10 min, reduce backpressure)

## Expected Latency Improvements

- Remove logging: **-50-90% latency** in hot paths
- Typed structs: **-20-40% parsing time**
- CPU affinity: **-10-30% tail latency** (p99)
- Channel buffer: **-backpressure delays**

## Tools to Install

```bash
# Profiling
cargo install flamegraph
cargo install tokio-console

# Metrics
# Add to Cargo.toml: metrics = "0.21"

# CPU affinity
# Add to Cargo.toml: core-affinity = "0.3"

# Lock-free structures
# Add to Cargo.toml: crossbeam-channel = "0.5"
```
