//! CLI and execution configuration (credentials reserved for future trading paths).
#![allow(dead_code)]

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Bias {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Net {
    Mainnet,
    Testnet,
}

impl Net {
    fn parse_flag(flag: &str, value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mainnet" => Ok(Net::Mainnet),
            "testnet" => Ok(Net::Testnet),
            other => Err(format!(
                "Invalid value for `{flag}`: `{other}` (expected mainnet|testnet)"
            )),
        }
    }
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

    /// Base asset for perp legs (e.g. `BTC`, `SOL`). Defaults from `--pair` prefix or `BTC`.
    pub perp_symbol: String,

    /// Target USD notional **per leg** (long and short use the same size). If unset, defaults to
    /// `budget/2` and is capped by `max_margin_leverage_assumption`.
    pub notional_usd_per_leg: Option<u64>,

    /// Upper bound: `notional_per_leg <= (budget/2) * max_margin_leverage_assumption` (integer USD).
    pub max_margin_leverage_assumption: u64,

    // Hyperliquid
    pub hyperliquid_private_key: Option<String>,
    /// Perpetual `universe` index from [`meta`](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint).
    /// If unset and `perp_symbol` is `BTC`, defaults to `0` (verify on mainnet).
    pub hyperliquid_asset_id: Option<u32>,
    /// Hyperliquid network selection for both market data and execution.
    /// If unset: defaults to `testnet` when `--execute-live 0`, otherwise `mainnet`.
    pub hyperliquid_network: Net,

    #[cfg(feature = "cex")]
    // Kraken
    pub kraken_api_key: Option<String>,
    #[cfg(feature = "cex")]
    pub kraken_api_secret: Option<String>,

    #[cfg(feature = "cex")]
    // Binance
    pub binance_api_key: Option<String>,
    #[cfg(feature = "cex")]
    pub binance_api_secret: Option<String>,

    // dYdX v4 (order execution / signing; market data uses public indexer WS)
    pub dydx_private_key: Option<String>,
    /// If set, `DydxExecutor::send` POSTs the planning JSON here (signing service / composite client).
    pub dydx_order_relay_url: Option<String>,
    /// dYdX network selection for indexer WebSocket market data.
    /// If unset: defaults to `testnet` when `--execute-live 0`, otherwise `mainnet`.
    pub dydx_network: Net,

    pub bias: Bias,

    pub budget: u64,

    /// 0 = no live orders (dry run); 1 = submit real orders.
    pub execute_live: bool,
}

impl Args {
    /// Parse args from `std::env::args()`.
    ///
    /// Supported flags (all optional):
    /// - `--pair <SYMBOL>` (example: `BTC/USDT`)
    /// - `--hyperliquid-private-key <KEY>`
    /// - With feature `cex`: `--kraken-api-key`, `--kraken-api-secret`, `--binance-api-key`, `--binance-api-secret`
    /// - `--dydx-private-key <KEY>`
    /// - `--dydx-order-relay-url <URL>` (POST planning JSON; run local relay from `dydx-relay/server.cjs`)
    /// - `--dydx-network <mainnet|testnet>` (optional; default depends on `--execute-live`)
    /// - `--bias <buy|sell>`
    /// - `--budget <USD>` (integer dollars; default `1` for small test runs)
    /// - `--execute-live <0|1>` (default `0`; when `1`, submits real orders)
    /// - `--hyperliquid-asset-id <N>` (perp index in `meta.universe`; optional for `BTC` default guess `0`)
    /// - `--hyperliquid-network <mainnet|testnet>` (optional; default depends on `--execute-live`)
    /// - `--perp-symbol <SYM>` (e.g. `SOL`; defaults from `--pair` before `/` or `BTC`)
    /// - `--notional-usd-per-leg <USD>` (integer; default: `budget/2`, capped by leverage field below)
    /// - `--max-margin-leverage <N>` (integer ≥ 1, default `3`) caps default/explicit notional per leg
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
            None => 1,
        };

        let execute_live = match map
            .get("--execute-live")
            .map(|s| s.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("1") | Some("true") | Some("yes") => true,
            Some("0") | Some("false") | Some("no") | None => false,
            Some(other) => return Err(format!("Invalid value for `--execute-live`: `{other}`")),
        };

        let pair = map.get("--pair").cloned();

        let perp_symbol = map
            .get("--perp-symbol")
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                pair.as_ref()
                    .and_then(|p| p.split('/').next())
                    .map(|s| s.trim().to_uppercase())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "BTC".to_string());

        let notional_usd_per_leg = match map.get("--notional-usd-per-leg") {
            Some(v) => Some(
                v.parse::<u64>()
                    .map_err(|_| format!("Invalid value for `--notional-usd-per-leg`: `{v}`"))?,
            ),
            None => None,
        };

        let max_margin_leverage_assumption = match map.get("--max-margin-leverage") {
            Some(v) => v
                .parse::<u64>()
                .map_err(|_| format!("Invalid value for `--max-margin-leverage`: `{v}`"))?
                .max(1),
            None => 3,
        };

        let hyperliquid_asset_id = match map.get("--hyperliquid-asset-id") {
            Some(v) => Some(
                v.parse::<u32>()
                    .map_err(|_| format!("Invalid value for `--hyperliquid-asset-id`: `{v}`"))?,
            ),
            None => None,
        };

        let hyperliquid_network = map
            .get("--hyperliquid-network")
            .map(|s| Net::parse_flag("--hyperliquid-network", s))
            .unwrap_or_else(|| {
                Ok(if execute_live {
                    Net::Mainnet
                } else {
                    Net::Testnet
                })
            })?;

        let dydx_network = map
            .get("--dydx-network")
            .map(|s| Net::parse_flag("--dydx-network", s))
            .unwrap_or_else(|| {
                Ok(if execute_live {
                    Net::Mainnet
                } else {
                    Net::Testnet
                })
            })?;

        Ok(Self {
            pair,
            perp_symbol,
            notional_usd_per_leg,
            max_margin_leverage_assumption,
            hyperliquid_private_key: map.get("--hyperliquid-private-key").cloned(),
            hyperliquid_asset_id,
            hyperliquid_network,
            #[cfg(feature = "cex")]
            kraken_api_key: map.get("--kraken-api-key").cloned(),
            #[cfg(feature = "cex")]
            kraken_api_secret: map.get("--kraken-api-secret").cloned(),
            #[cfg(feature = "cex")]
            binance_api_key: map.get("--binance-api-key").cloned(),
            #[cfg(feature = "cex")]
            binance_api_secret: map.get("--binance-api-secret").cloned(),
            dydx_private_key: map.get("--dydx-private-key").cloned(),
            dydx_order_relay_url: map.get("--dydx-order-relay-url").cloned(),
            dydx_network,
            bias,
            budget,
            execute_live,
        })
    }

    /// Conservative USD notional for **each** leg of a cross-venue hedge, from `budget` split across two venues.
    pub fn clamped_notional_usd_per_leg(&self) -> u64 {
        let per_venue = (self.budget / 2).max(1);
        let cap = per_venue.saturating_mul(self.max_margin_leverage_assumption.max(1));
        let requested = self.notional_usd_per_leg.unwrap_or(per_venue);
        requested.min(cap).max(1)
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
