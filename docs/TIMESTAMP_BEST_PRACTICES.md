# Timestamp Best Practices for Low-Latency Systems

## Answer: **Use BOTH - Exchange Timestamp + Instant::now()**

For low-latency trading systems, you need **both timestamps** for different purposes.

## Why Both?

### Exchange Timestamp (from WebSocket)
**Purpose**: Event ordering, correctness, backtesting
- ✅ Shows when exchange generated the price update
- ✅ Authoritative for "when did this price actually exist?"
- ✅ Essential for ordering events correctly
- ✅ Needed for regulatory/audit logs
- ✅ Critical for backtesting accuracy
- ⚠️ Doesn't include network latency to your system
- ⚠️ May not be available from all exchanges

### Instant::now() (when you receive)
**Purpose**: Latency measurement, staleness detection
- ✅ Measures YOUR processing latency (receive → parse → send)
- ✅ Monotonic clock (no clock adjustments)
- ✅ Always available
- ✅ Can detect network delays
- ✅ Can detect clock skew issues
- ⚠️ Doesn't show exchange-side timing

## What Each Exchange Provides

### Binance
- Field: `"E"` (event time) - Unix epoch milliseconds
- Field: `"T"` (trade time) - Unix epoch milliseconds
- Always included in ticker messages

### Coinbase
- Field: `"time"` - ISO 8601 timestamp
- Included in ticker messages

### Kraken
- Field: `timestamp` - RFC3339 format
- Included in ticker messages

## Recommended Data Structure

```rust
pub struct PriceUpdate {
    pub exchange: Exchange,
    pub price: u64,
    pub exchange_timestamp: Option<u64>,  // From exchange (if available)
    pub received_at: Instant,              // When we received it
}
```

## Use Cases

### 1. **Ordering Events** → Use Exchange Timestamp
```rust
// Sort prices by when they actually occurred on exchange
prices.sort_by_key(|p| p.exchange_timestamp);
```

### 2. **Measuring Latency** → Use Both
```rust
// Calculate network + processing latency
if let Some(exchange_ts) = price.exchange_timestamp {
    let latency = received_at.elapsed() - exchange_ts;
    // This shows: Exchange → Your System latency
}
```

### 3. **Staleness Detection** → Use Instant::now()
```rust
// Reject prices older than 100ms
if price.received_at.elapsed() > Duration::from_millis(100) {
    return; // Too stale
}
```

### 4. **Best Bid/Offer** → Use Exchange Timestamp
```rust
// Only compare prices from same time window
if prices.iter().all(|p| {
    p.exchange_timestamp.map_or(false, |ts| ts > cutoff_time)
}) {
    // All prices are current
}
```

## Performance Impact

- **Exchange timestamp parsing**: ~10-50ns (just reading JSON field)
- **Instant::now()**: ~10-20ns (CPU clock read)
- **Total overhead**: ~20-70ns per message
- **Benefit**: Enables latency measurement, ordering, staleness detection

## Implementation Priority

1. **Always capture `Instant::now()`** - Essential for latency measurement
2. **Parse exchange timestamp if available** - Use for ordering/correctness
3. **Fallback to `Instant::now()`** - If exchange doesn't provide timestamp

## Summary

**For low-latency: Use BOTH timestamps!**

- Exchange timestamp → Ordering, correctness, backtesting
- Instant::now() → Latency measurement, staleness detection
- Minimal overhead (~20-70ns)
- Critical for production systems
