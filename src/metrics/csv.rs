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
    pub post_only: bool,
    pub reduce_only: bool,
    pub ok: bool,
    pub err: Option<String>,
    pub profit_expect_bps: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum CsvEvent {
    OrderAttempt(CsvOrderAttempt),
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
        let header = "event_type,ts_ms,run_label,leg,exchange,side,price_cents,qty_e8,notional_usd,post_only,reduce_only,ok,err,profit_expect_bps\n";
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
                            n_orders += 1;
                            if o.ok { n_ok += 1; }
                            sum_notional_usd += notional_usd;
                            sum_price_cents += o.price_cents as u128;

                            let line = format!(
                                "order_attempt,{},{},{},{},{},{},{},{:.8},{},{},{},{},{}\n",
                                o.ts_ms,
                                csv_escape(&run_label),
                                o.leg,
                                csv_escape(&format!("{:?}", o.exchange)),
                                csv_escape(&format!("{:?}", o.side)),
                                o.price_cents,
                                o.qty_e8,
                                notional_usd,
                                o.post_only,
                                o.reduce_only,
                                o.ok,
                                csv_escape(o.err.as_deref().unwrap_or("")),
                                o.profit_expect_bps.map(|x| x.to_string()).unwrap_or_default()
                            );
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
                    }
                }
            }
        }

        // End-of-run summary row.
        let avg_notional = if n_orders > 0 { sum_notional_usd / (n_orders as f64) } else { 0.0 };
        let avg_price_cents = if n_orders > 0 { (sum_price_cents / (n_orders as u128)) as u64 } else { 0 };
        let ok_rate = if n_orders > 0 { (n_ok as f64) / (n_orders as f64) } else { 0.0 };

        let summary_err = csv_escape(&format!("n_orders={n_orders} n_ok={n_ok} ok_rate={ok_rate:.6}"));
        let summary_cols: [String; 14] = [
            "summary".to_string(),
            chrono::Utc::now().timestamp_millis().to_string(),
            csv_escape(&run_label),
            "".to_string(), // leg
            "".to_string(), // exchange
            "".to_string(), // side
            avg_price_cents.to_string(),
            "".to_string(), // qty_e8
            format!("{avg_notional:.8}"),
            "".to_string(), // post_only
            "".to_string(), // reduce_only
            "".to_string(), // ok
            summary_err,
            "".to_string(), // profit_expect_bps
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

