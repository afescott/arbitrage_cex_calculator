//! Optional CSV metrics writer (feature = "csv").
//!
//! Design: producers send small events over a bounded channel; a single writer task owns the file
//! and uses a `BufWriter` for efficient append.

use std::path::PathBuf;

use tokio::{
    fs::File,
    io::{AsyncWriteExt, BufWriter},
    sync::mpsc,
    task::JoinHandle,
    time::{self, Duration},
};

use crate::calculation::fees::FeeCalculator;
use crate::orderbook::book::Exchange;
use crate::purchase::OrderSide;

#[derive(Debug, Clone)]
pub struct CsvOrderAttempt {
    pub ts_ms: i64,
    pub leg: &'static str, // "first" | "second" | "dry_run"
    pub exchange: Exchange,
    pub side: OrderSide,
    pub price_cents: u64,
    pub qty_e8: u64,
    pub filled_qty_e8: Option<u64>,
    pub post_only: bool,
    pub reduce_only: bool,
    pub ok: bool,
    pub err: Option<String>,
    pub profit_expect_bps: Option<u64>,
}

/// One cross-venue hedge attempt (after first/second leg resolution).
#[derive(Debug, Clone)]
pub struct CsvRouteOutcome {
    pub buy_exchange: Exchange,
    pub sell_exchange: Exchange,
    pub buy_price_cents: u64,
    pub sell_price_cents: u64,
    pub qty_e8: u64,
    pub profit_expect_bps: u64,
    pub first_ok: bool,
    pub second_ok: bool,
}

#[derive(Debug, Clone)]
pub enum CsvEvent {
    OrderAttempt(CsvOrderAttempt),
    RouteOutcome(CsvRouteOutcome),
}

#[derive(Debug)]
pub struct CsvHandle {
    pub tx: mpsc::Sender<CsvEvent>,
    pub join: JoinHandle<()>,
}

pub fn start_csv_writer(path: PathBuf, run_label: String) -> CsvHandle {
    // Bounded channel = explicit backpressure.
    let (tx, mut rx) = mpsc::channel::<CsvEvent>(4096);

    let join = tokio::spawn(async move {
        let file = match File::create(&path).await {
            Ok(f) => f,
            Err(e) => {
                eprintln!("csv metrics: failed to create {}: {e}", path.display());
                return;
            }
        };
        let mut w = BufWriter::new(file);

        // Header: stable schema for downstream parsing.
        // Keep this in sync with the per-row writer below (21 columns).
        let header = "event_type,ts_ms,run_label,leg,exchange,side,price_cents,qty_e8,filled_qty_e8,fill_pct,notional_usd,post_only,reduce_only,ok,err,profit_expect_bps,total_cost_usd,total_fees_usd,total_cost_plus_fees_usd,expected_profit_usd,n_completed_routes\n";
        if let Err(e) = w.write_all(header.as_bytes()).await {
            eprintln!("csv metrics: failed to write header: {e}");
            return;
        }

        let mut flush_interval = time::interval(Duration::from_secs(2));
        flush_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut since_flush: u64 = 0;

        // Running aggregates for an end-of-run summary row.
        let mut n_orders: u64 = 0;
        let mut n_ok: u64 = 0;
        let mut sum_notional_usd: f64 = 0.0;
        let mut sum_price_cents: u128 = 0;

        let mut total_cost_usd: f64 = 0.0;
        let mut total_fees_usd: f64 = 0.0;
        let mut expected_profit_usd: f64 = 0.0;
        let mut n_completed_routes: u64 = 0;

        loop {
            tokio::select! {
                _ = flush_interval.tick() => {
                    if since_flush > 0 {
                        let _ = w.flush().await;
                        since_flush = 0;
                    }
                }
                maybe = rx.recv() => {
                    let Some(ev) = maybe else {
                        break;
                    };
                    match ev {
                        CsvEvent::OrderAttempt(o) => {
                            let notional_usd = (o.price_cents as f64 / 100.0) * (o.qty_e8 as f64 / 100_000_000.0);
                            let fill_pct = o
                                .filled_qty_e8
                                .and_then(|f| if o.qty_e8 > 0 { Some(f as f64 / o.qty_e8 as f64) } else { None });
                            n_orders += 1;
                            if o.ok { n_ok += 1; }
                            sum_notional_usd += notional_usd;
                            sum_price_cents += o.price_cents as u128;
                            if o.ok {
                                total_cost_usd += notional_usd;
                                let bps = FeeCalculator::taker_fee_bps(o.exchange) as f64;
                                total_fees_usd += notional_usd * bps / 10_000.0;
                            }

                            let cols: [String; 21] = [
                                "order_attempt".to_string(),
                                o.ts_ms.to_string(),
                                csv_escape(&run_label),
                                o.leg.to_string(),
                                csv_escape(&format!("{:?}", o.exchange)),
                                csv_escape(&format!("{:?}", o.side)),
                                o.price_cents.to_string(),
                                o.qty_e8.to_string(),
                                o.filled_qty_e8.map(|x| x.to_string()).unwrap_or_default(),
                                fill_pct.map(|x| format!("{x:.6}")).unwrap_or_default(),
                                format!("{notional_usd:.8}"),
                                o.post_only.to_string(),
                                o.reduce_only.to_string(),
                                o.ok.to_string(),
                                csv_escape(o.err.as_deref().unwrap_or("")),
                                o.profit_expect_bps.map(|x| x.to_string()).unwrap_or_default(),
                                "".to_string(), // total_cost_usd
                                "".to_string(), // total_fees_usd
                                "".to_string(), // total_cost_plus_fees_usd
                                "".to_string(), // expected_profit_usd
                                "".to_string(), // n_completed_routes
                            ];
                            let line = format!("{}\n", cols.join(","));
                            if let Err(e) = w.write_all(line.as_bytes()).await {
                                eprintln!("csv metrics: write failed: {e}");
                                // Keep running; don't take down the trading loop.
                            } else {
                                since_flush += 1;
                                if since_flush >= 200 {
                                    let _ = w.flush().await;
                                    since_flush = 0;
                                }
                            }
                        }
                        CsvEvent::RouteOutcome(r) => {
                            if r.first_ok && r.second_ok {
                                n_completed_routes += 1;
                                let buy_n = (r.buy_price_cents as f64 / 100.0)
                                    * (r.qty_e8 as f64 / 100_000_000.0);
                                expected_profit_usd +=
                                    buy_n * (r.profit_expect_bps as f64 / 10_000.0);
                            }
                            let pair = format!("{:?}->{:?}", r.buy_exchange, r.sell_exchange);
                            let buy_notional = (r.buy_price_cents as f64 / 100.0)
                                * (r.qty_e8 as f64 / 100_000_000.0);
                            let hedge_ok = r.first_ok && r.second_ok;
                            let details = format!(
                                "first_ok={} second_ok={} sell_px_cents={}",
                                r.first_ok, r.second_ok, r.sell_price_cents
                            );
                            let cols: [String; 21] = [
                                "route_outcome".to_string(),
                                chrono::Utc::now().timestamp_millis().to_string(),
                                csv_escape(&run_label),
                                "route".to_string(),
                                csv_escape(&pair), // exchange column used for "BUY->SELL"
                                "".to_string(),     // side
                                r.buy_price_cents.to_string(),
                                r.qty_e8.to_string(),
                                "".to_string(), // filled_qty_e8
                                "".to_string(), // fill_pct
                                format!("{buy_notional:.8}"),
                                "false".to_string(),
                                "false".to_string(),
                                hedge_ok.to_string(),
                                csv_escape(&details),
                                r.profit_expect_bps.to_string(),
                                "".to_string(),
                                "".to_string(),
                                "".to_string(),
                                "".to_string(),
                                "".to_string(),
                            ];
                            let route_line = format!("{}\n", cols.join(","));
                            let _ = w.write_all(route_line.as_bytes()).await;
                            since_flush += 1;
                            if since_flush >= 200 {
                                let _ = w.flush().await;
                                since_flush = 0;
                            }
                        }
                    }
                }
            }
        }

        // End-of-run summary row.
        let avg_notional = if n_orders > 0 { sum_notional_usd / (n_orders as f64) } else { 0.0 };
        let avg_price_cents = if n_orders > 0 { (sum_price_cents / (n_orders as u128)) as u64 } else { 0 };
        let ok_rate = if n_orders > 0 { (n_ok as f64) / (n_orders as f64) } else { 0.0 };

        let total_cost_plus_fees = total_cost_usd + total_fees_usd;
        let summary_err = csv_escape(&format!("n_orders={n_orders} n_ok={n_ok} ok_rate={ok_rate:.6}"));
        let summary_cols: [String; 21] = [
            "summary".to_string(),
            chrono::Utc::now().timestamp_millis().to_string(),
            csv_escape(&run_label),
            "".to_string(), // leg
            "".to_string(), // exchange
            "".to_string(), // side
            avg_price_cents.to_string(),
            "".to_string(), // qty_e8
            "".to_string(), // filled_qty_e8
            "".to_string(), // fill_pct
            format!("{avg_notional:.8}"),
            "".to_string(), // post_only
            "".to_string(), // reduce_only
            "".to_string(), // ok
            summary_err,
            "".to_string(), // profit_expect_bps
            format!("{total_cost_usd:.8}"),
            format!("{total_fees_usd:.8}"),
            format!("{total_cost_plus_fees:.8}"),
            format!("{expected_profit_usd:.8}"),
            n_completed_routes.to_string(),
        ];
        let summary = format!("{}\n", summary_cols.join(","));
        let _ = w.write_all(summary.as_bytes()).await;
        let _ = w.flush().await;
    });

    CsvHandle { tx, join }
}

fn csv_escape(s: &str) -> String {
    // Always quote strings; escape quotes; strip newlines.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\"\""),
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

