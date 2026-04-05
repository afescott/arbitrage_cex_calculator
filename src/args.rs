//! CLI and execution configuration (credentials reserved for future trading paths).
#![allow(dead_code)]

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Bias {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionTuning {
    /// Hard cap on acceptable slippage, in basis points.
    pub max_slippage_bps: u64,
    /// Minimum required orderbook depth window, in basis points.
    pub depth_window_bps: u64,
    /// Limit order expiry before cancel/requote.
    pub limit_first_expiry_ms: u64,
    /// Maximum number of requotes before skipping a trade.
    pub max_requotes: u64,
    /// Skip trading when spread exceeds this value, in basis points.
    pub max_spread_bps: u64,
}

/// Command-line arguments for configuring exchange credentials.
///
/// Note: passing secrets via CLI arguments is not ideal because command-line
/// args can be visible in process listings. Prefer environment variables or a
/// secrets manager when possible.
#[derive(Debug, Clone)]
pub struct Args {
    pub pair: Option<String>,

    // Hyperliquid
    pub hyperliquid_private_key: Option<String>,

    // Kraken
    pub kraken_api_key: Option<String>,
    pub kraken_api_secret: Option<String>,

    // Binance
    pub binance_api_key: Option<String>,
    pub binance_api_secret: Option<String>,

    pub bias: Bias,

    pub budget: u64,
}

impl Args {
    /// Parse args from `std::env::args()`.
    ///
    /// Supported flags (all optional):
    /// - `--pair <SYMBOL>` (example: `BTC/USDT`)
    /// - `--hyperliquid-private-key <KEY>`
    /// - `--kraken-api-key <KEY>`
    /// - `--kraken-api-secret <SECRET>`
    /// - `--binance-api-key <KEY>`
    /// - `--binance-api-secret <SECRET>`
    /// - `--bias <buy|sell>`
    /// - `--budget <USD>` (integer dollars, e.g. `10`)
    ///
    /// Unknown flags are ignored to keep this lightweight.
    pub fn from_env() -> Result<Self, String> {
        let raw_args: Vec<String> = std::env::args().collect();
        let mut map: HashMap<String, String> = HashMap::new();

        let mut i = 1; // skip argv[0]
        while i < raw_args.len() {
            let key = raw_args[i].as_str();
            if key.starts_with("--") {
                if i + 1 >= raw_args.len() {
                    return Err(format!("Missing value for flag `{}`", key));
                }
                let val = raw_args[i + 1].clone();
                map.insert(key.to_string(), val);
                i += 2;
            } else {
                i += 1;
            }
        }

        let bias = match map
            .get("--bias")
            .map(|s| s.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("sell") => Bias::Sell,
            Some("buy") | None => Bias::Buy,
            Some(other) => return Err(format!("Invalid value for `--bias`: `{other}`")),
        };

        let budget = match map.get("--budget") {
            Some(v) => v
                .parse::<u64>()
                .map_err(|_| format!("Invalid value for `--budget`: `{v}`"))?,
            None => 10,
        };

        Ok(Self {
            pair: map.get("--pair").cloned(),
            hyperliquid_private_key: map.get("--hyperliquid-private-key").cloned(),
            kraken_api_key: map.get("--kraken-api-key").cloned(),
            kraken_api_secret: map.get("--kraken-api-secret").cloned(),
            binance_api_key: map.get("--binance-api-key").cloned(),
            binance_api_secret: map.get("--binance-api-secret").cloned(),
            bias,
            budget,
        })
    }

    /// Derive execution tuning defaults from `budget` (USD).
    ///
    /// Designed to be conservative for small budgets (e.g. $10), and gradually
    /// allow more slippage / wider spread / longer expiry up to $100.
    pub fn execution_tuning(&self) -> ExecutionTuning {
        let b = self.budget;

        // Tiered defaults (basis points unless noted).
        // These are intentionally conservative: for small notional sizes, prefer
        // skipping trades over paying through spread/impact.
        match b {
            0..=10 => ExecutionTuning {
                max_slippage_bps: 20, // 0.20%
                depth_window_bps: 25, // require depth within 0.25%
                limit_first_expiry_ms: 500,
                max_requotes: 2,
                max_spread_bps: 10,
            },
            11..=25 => ExecutionTuning {
                max_slippage_bps: 30, // 0.30%
                depth_window_bps: 35,
                limit_first_expiry_ms: 700,
                max_requotes: 3,
                max_spread_bps: 12,
            },
            26..=50 => ExecutionTuning {
                max_slippage_bps: 40, // 0.40%
                depth_window_bps: 50,
                limit_first_expiry_ms: 900,
                max_requotes: 3,
                max_spread_bps: 15,
            },
            51..=100 => ExecutionTuning {
                max_slippage_bps: 60, // 0.60%
                depth_window_bps: 75,
                limit_first_expiry_ms: 1200,
                max_requotes: 4,
                max_spread_bps: 20,
            },
            _ => ExecutionTuning {
                // For larger sizes, default to a cautious cap rather than
                // scaling unboundedly.
                max_slippage_bps: 80, // 0.80%
                depth_window_bps: 100,
                limit_first_expiry_ms: 1500,
                max_requotes: 5,
                max_spread_bps: 25,
            },
        }
    }
}
