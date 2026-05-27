//! In-process atomic telemetry for the hot path.
//!
//! Two layers:
//! - Counters: `AtomicU64`, `Relaxed` ordering. Free to read/write from any task.
//! - Per-stage latency: one `hdrhistogram::Histogram<u64>` per stage behind a `Mutex`.
//!   Each stage is only recorded from a single task (aggregator OR purchase), so the
//!   mutex is effectively uncontended; the read-side (reporter) takes it briefly every
//!   `period`.
//!
//! Stage list (`Stage`) covers price-tick → order-ack on the hot path. Anything outside
//! that path (shutdown flatten, etc.) is intentionally not timed here.
//!
//! Subscriber setup ([`init_subscriber`]) is also here so all tracing/observability
//! plumbing lives in one place. With `--features otel`, traces are exported via OTLP
//! gRPC to whatever `OTEL_EXPORTER_OTLP_ENDPOINT` points at (default `localhost:4317`).

#![allow(dead_code)]

pub mod subscriber;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{interval, MissedTickBehavior};

#[derive(Copy, Clone, Debug)]
#[repr(usize)]
pub enum Stage {
    /// WS recv (`ExchangePrice.received_at`) → aggregator dequeue.
    WsToAggregator = 0,
    /// `OrderBook::add_exchange_price_level`.
    BookUpdate = 1,
    /// `OrderBook::check_for_immediate_purchase` (arb detection).
    ArbDetect = 2,
    /// Aggregator emit (`route.t_emitted`) → `PurchaseManager` dequeue.
    AggregatorToPurchase = 3,
    /// `OrderExecutor::build_payload`.
    PurchaseBuild = 4,
    /// `OrderExecutor::sign`.
    PurchaseSign = 5,
    /// `OrderExecutor::send` (HTTP request → ack).
    PurchaseSendToAck = 6,
}

pub const N_STAGES: usize = 7;

const STAGE_NAMES: [&str; N_STAGES] = [
    "ws_to_aggregator",
    "book_update",
    "arb_detect",
    "aggregator_to_purchase",
    "purchase_build",
    "purchase_sign",
    "purchase_send_to_ack",
];

struct StageHist {
    hist: Mutex<Histogram<u64>>,
}

impl StageHist {
    fn new() -> Self {
        // 1 ns .. 60 s, 3 significant figures.
        // ~120 KB per histogram. Bounded so `record` never allocates.
        Self {
            hist: Mutex::new(
                Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3)
                    .expect("hdrhistogram bounds valid"),
            ),
        }
    }
}

pub struct Telemetry {
    pub n_ws_msgs: AtomicU64,
    pub n_book_updates: AtomicU64,
    pub n_arb_found: AtomicU64,
    pub n_routes_emitted: AtomicU64,
    pub n_routes_dropped_full: AtomicU64,
    pub n_orders_sent: AtomicU64,
    pub n_orders_ok: AtomicU64,
    pub n_orders_err: AtomicU64,
    stages: [StageHist; N_STAGES],
}

impl Telemetry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            n_ws_msgs: AtomicU64::new(0),
            n_book_updates: AtomicU64::new(0),
            n_arb_found: AtomicU64::new(0),
            n_routes_emitted: AtomicU64::new(0),
            n_routes_dropped_full: AtomicU64::new(0),
            n_orders_sent: AtomicU64::new(0),
            n_orders_ok: AtomicU64::new(0),
            n_orders_err: AtomicU64::new(0),
            stages: [
                StageHist::new(),
                StageHist::new(),
                StageHist::new(),
                StageHist::new(),
                StageHist::new(),
                StageHist::new(),
                StageHist::new(),
            ],
        })
    }

    /// Record a single stage observation. Saturates at the histogram's configured upper bound
    /// (60s); zero durations are clamped to 1ns so they show up in the lowest bucket.
    #[inline]
    pub fn record(&self, stage: Stage, dur: Duration) {
        let ns = u64::try_from(dur.as_nanos()).unwrap_or(u64::MAX);
        if let Ok(mut h) = self.stages[stage as usize].hist.lock() {
            let _ = h.saturating_record(ns.max(1));
        }
    }
}

/// Spawn a task that prints a per-stage latency summary to stderr every `period`,
/// and exits when `shutdown` flips to `true`.
pub fn spawn_reporter(
    telemetry: Arc<Telemetry>,
    period: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = interval(period);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // Skip the immediate first tick — gives feeds a chance to warm up.
        tick.tick().await;

        let start = Instant::now();
        let mut prev_ws = 0u64;
        let mut prev_arb = 0u64;
        let mut prev_routes = 0u64;
        let mut prev_orders = 0u64;
        let mut prev_at = start;

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    emit_snapshot(
                        &telemetry,
                        start,
                        &mut prev_at,
                        &mut prev_ws,
                        &mut prev_arb,
                        &mut prev_routes,
                        &mut prev_orders,
                    );
                }
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        // One final snapshot on the way out.
                        emit_snapshot(
                            &telemetry,
                            start,
                            &mut prev_at,
                            &mut prev_ws,
                            &mut prev_arb,
                            &mut prev_routes,
                            &mut prev_orders,
                        );
                        return;
                    }
                }
            }
        }
    })
}

fn emit_snapshot(
    telemetry: &Telemetry,
    start: Instant,
    prev_at: &mut Instant,
    prev_ws: &mut u64,
    prev_arb: &mut u64,
    prev_routes: &mut u64,
    prev_orders: &mut u64,
) {
    let now = Instant::now();
    let dt_s = (now - *prev_at).as_secs_f64().max(1e-6);

    let ws = telemetry.n_ws_msgs.load(Ordering::Relaxed);
    let books = telemetry.n_book_updates.load(Ordering::Relaxed);
    let arb = telemetry.n_arb_found.load(Ordering::Relaxed);
    let routes = telemetry.n_routes_emitted.load(Ordering::Relaxed);
    let dropped = telemetry.n_routes_dropped_full.load(Ordering::Relaxed);
    let orders = telemetry.n_orders_sent.load(Ordering::Relaxed);
    let ok = telemetry.n_orders_ok.load(Ordering::Relaxed);
    let err = telemetry.n_orders_err.load(Ordering::Relaxed);

    let d_ws = ws.saturating_sub(*prev_ws);
    let d_arb = arb.saturating_sub(*prev_arb);
    let d_routes = routes.saturating_sub(*prev_routes);
    let d_orders = orders.saturating_sub(*prev_orders);
    let ws_rate = (d_ws as f64) / dt_s;

    let uptime_s = (now - start).as_secs();
    eprintln!(
        "[telemetry t+{uptime_s}s] ws={ws} (+{d_ws}, {ws_rate:.0}/s) books={books} arb={arb} (+{d_arb}) routes={routes} (+{d_routes}) dropped={dropped} orders={orders} (+{d_orders}) ok={ok} err={err}"
    );
    for (i, name) in STAGE_NAMES.iter().enumerate() {
        let Ok(h) = telemetry.stages[i].hist.lock() else {
            continue;
        };
        if h.len() == 0 {
            continue;
        }
        let p50 = h.value_at_quantile(0.5);
        let p99 = h.value_at_quantile(0.99);
        let p999 = h.value_at_quantile(0.999);
        let max = h.max();
        eprintln!(
            "  {:<22} n={:<10} p50={:>10} p99={:>10} p999={:>10} max={:>10}",
            name,
            h.len(),
            fmt_ns(p50),
            fmt_ns(p99),
            fmt_ns(p999),
            fmt_ns(max),
        );
    }

    *prev_ws = ws;
    *prev_arb = arb;
    *prev_routes = routes;
    *prev_orders = orders;
    *prev_at = now;
}

fn fmt_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.2}s", ns as f64 / 1e9)
    } else if ns >= 1_000_000 {
        format!("{:.2}ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.2}us", ns as f64 / 1e3)
    } else {
        format!("{ns}ns")
    }
}
