//! Hyperliquid purchase: preflight, route checks, and real order placement via `hypersdk`.
//!
//! This replaces the previous stub `r/s/v` signing. `hypersdk` builds the L1 action, signs it,
//! and submits it to `/exchange`.

use chrono::Utc;
use hypersdk::hypercore::{self, types::*, PrivateKeySigner};
use reqwest::header::CONTENT_TYPE;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal::MathematicalOps;
use serde_json::Value;
use std::sync::Arc;

use super::{BuiltPayload, ExecError, LimitOrderRequest, OrderAck, OrderExecutor, OrderSide, SignedPayload};
use crate::args::Net;
use crate::orderbook::book::Exchange;

const HL_MIN_ORDER_NOTIONAL_USD: u64 = 10;
// Hyperliquid enforces "cannot be more than 80% away from reference price".
// In practice reference != allMids and the check may be strict, so keep a buffer.
const HL_MAX_DEVIATION_FROM_MID_FRAC: f64 = 0.79;

#[derive(Debug, Clone)]
pub(crate) struct HyperliquidExecutor {
    signer: Option<PrivateKeySigner>,
    signer_err: Option<String>,
    /// `meta.universe` index; if `None` and `perp_symbol == "BTC"`, [`resolve_asset_index`] uses `0`.
    asset_index: Option<u32>,
    perp_symbol: String,
    network: Net,
    ioc_cross_bps: u64,
    http: reqwest::Client,
    sz_decimals: Arc<tokio::sync::OnceCell<u32>>,
    resolved_asset: Arc<tokio::sync::OnceCell<u32>>,
}

impl HyperliquidExecutor {
    pub(crate) fn new(
        private_key: String,
        asset_index: Option<u32>,
        perp_symbol: String,
        network: Net,
        ioc_cross_bps: u64,
    ) -> Self {
        // Strict: hypersdk expects a 32-byte hex private key (64 hex chars), no `0x` prefix.
        let normalized = private_key.trim();
        let (signer, signer_err) = match normalized.parse::<PrivateKeySigner>() {
            Ok(s) => (Some(s), None),
            Err(e) => (
                None,
                Some(format!(
                    "Invalid `--hyperliquid-private-key`: expected 64 hex chars (32 bytes), no `0x` prefix. hypersdk error: {e}"
                )),
            ),
        };
        let http = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest hyperliquid client");
        Self {
            signer,
            signer_err,
            asset_index,
            perp_symbol,
            network,
            ioc_cross_bps,
            http,
            sz_decimals: Arc::new(tokio::sync::OnceCell::new()),
            resolved_asset: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    fn signer(&self) -> Result<&PrivateKeySigner, ExecError> {
        self.signer.as_ref().ok_or_else(|| {
            ExecError::MissingCredentials(
                "Invalid `--hyperliquid-private-key` (must be 32-byte hex, e.g. 0x… with 64 hex chars)",
            )
        })
    }

    fn resolve_asset_index_hint(&self) -> u32 {
        // If the user specified an explicit universe index, keep it.
        // Otherwise, we will resolve it dynamically in `send()` from `/info meta`.
        self.asset_index.unwrap_or(0)
    }

    fn info_http_url(&self) -> &'static str {
        match self.network {
            Net::Testnet => "https://api.hyperliquid-testnet.xyz/info",
            Net::Mainnet => "https://api.hyperliquid.xyz/info",
        }
    }

    async fn fetch_mid_px(&self, coin: &str) -> Result<Decimal, ExecError> {
        let url = self.info_http_url();
        let body = serde_json::json!({ "type": "allMids" });
        let resp = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid allMids http: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid allMids body: {e}")))?;
        if !status.is_success() {
            return Err(ExecError::SendFailed(format!(
                "hyperliquid allMids HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            )));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid allMids json: {e}")))?;
        let mid_s = v.get(coin).and_then(|x| x.as_str()).ok_or_else(|| {
            ExecError::SendFailed(format!("hyperliquid allMids: missing mid for coin={coin}"))
        })?;
        mid_s
            .parse::<Decimal>()
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid allMids: bad mid px: {e}")))
    }

    async fn fetch_top_of_book(&self, coin: &str) -> Result<(Decimal, Decimal), ExecError> {
        // Returns (best_bid, best_ask) from /info l2Book.
        let url = self.info_http_url();
        let body = serde_json::json!({
            "type": "l2Book",
            "coin": coin,
        });
        let resp = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid l2Book http: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid l2Book body: {e}")))?;
        if !status.is_success() {
            return Err(ExecError::SendFailed(format!(
                "hyperliquid l2Book HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            )));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid l2Book json: {e}")))?;
        let levels = v
            .get("levels")
            .and_then(|l| l.as_array())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid l2Book: missing levels".into()))?;
        if levels.len() < 2 {
            return Err(ExecError::SendFailed(
                "hyperliquid l2Book: levels too short".into(),
            ));
        }
        let best_bid = levels[0]
            .as_array()
            .and_then(|bids| bids.first())
            .and_then(|o| o.get("px").and_then(|x| x.as_str()))
            .ok_or_else(|| ExecError::SendFailed("hyperliquid l2Book: missing best bid".into()))?
            .parse::<Decimal>()
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid l2Book: bad bid px: {e}")))?;
        let best_ask = levels[1]
            .as_array()
            .and_then(|asks| asks.first())
            .and_then(|o| o.get("px").and_then(|x| x.as_str()))
            .ok_or_else(|| ExecError::SendFailed("hyperliquid l2Book: missing best ask".into()))?
            .parse::<Decimal>()
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid l2Book: bad ask px: {e}")))?;
        Ok((best_bid, best_ask))
    }

    async fn fetch_mark_px(&self, asset: u32) -> Result<Decimal, ExecError> {
        let url = self.info_http_url();
        let body = serde_json::json!({ "type": "metaAndAssetCtxs" });
        let resp = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid metaAndAssetCtxs http: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid metaAndAssetCtxs body: {e}")))?;
        if !status.is_success() {
            return Err(ExecError::SendFailed(format!(
                "hyperliquid metaAndAssetCtxs HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            )));
        }

        // Response is typically: [ { meta: { universe: [...] } }, [ { markPx: "...", ... }, ... ] ]
        let v: Value = serde_json::from_str(&text).map_err(|e| {
            ExecError::SendFailed(format!("hyperliquid metaAndAssetCtxs json: {e}"))
        })?;
        let mark_s = v
            .get(1)
            .and_then(|x| x.as_array())
            .and_then(|arr| arr.get(asset as usize))
            .and_then(|ctx| ctx.get("markPx").or_else(|| ctx.get("mark_px")))
            .and_then(|x| x.as_str())
            .ok_or_else(|| {
                ExecError::SendFailed(format!(
                    "hyperliquid metaAndAssetCtxs: missing markPx for asset={asset}"
                ))
            })?;
        mark_s.parse::<Decimal>().map_err(|e| {
            ExecError::SendFailed(format!("hyperliquid metaAndAssetCtxs: bad markPx: {e}"))
        })
    }

    async fn resolve_sz_decimals(&self, asset: u32) -> Result<u32, ExecError> {
        // Cache the first fetched value. (This bot currently only trades one perp symbol at a time.)
        self.sz_decimals
            .get_or_try_init(|| async move {
                let url = self.info_http_url();
                let body = serde_json::json!({ "type": "meta" });
                let resp = self
                    .http
                    .post(url)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.to_string())
                    .send()
                    .await
                    .map_err(|e| ExecError::SendFailed(format!("hyperliquid meta http: {e}")))?;
                let status = resp.status();
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ExecError::SendFailed(format!("hyperliquid meta body: {e}")))?;
                if !status.is_success() {
                    return Err(ExecError::SendFailed(format!(
                        "hyperliquid meta HTTP {status}: {}",
                        text.chars().take(800).collect::<String>()
                    )));
                }
                let v: Value = serde_json::from_str(&text)
                    .map_err(|e| ExecError::SendFailed(format!("hyperliquid meta json: {e}")))?;
                let sz = v
                    .pointer("/universe")
                    .and_then(|u| u.as_array())
                    .and_then(|arr| arr.get(asset as usize))
                    .and_then(|obj| obj.get("szDecimals"))
                    .and_then(|n| n.as_u64())
                    .ok_or_else(|| {
                        ExecError::SendFailed(format!(
                            "hyperliquid meta: could not read universe[{asset}].szDecimals"
                        ))
                    })?;
                Ok(sz as u32)
            })
            .await
            .copied()
    }

    async fn resolve_asset_index_from_meta(&self) -> Result<u32, ExecError> {
        if let Some(i) = self.asset_index {
            return Ok(i);
        }
        // Cache the resolved index for this bot run (single perp symbol).
        self.resolved_asset
            .get_or_try_init(|| async move {
                let url = self.info_http_url();
                let body = serde_json::json!({ "type": "meta" });
                let resp = self
                    .http
                    .post(url)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.to_string())
                    .send()
                    .await
                    .map_err(|e| ExecError::SendFailed(format!("hyperliquid meta http: {e}")))?;
                let status = resp.status();
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ExecError::SendFailed(format!("hyperliquid meta body: {e}")))?;
                if !status.is_success() {
                    return Err(ExecError::SendFailed(format!(
                        "hyperliquid meta HTTP {status}: {}",
                        text.chars().take(800).collect::<String>()
                    )));
                }
                let v: Value = serde_json::from_str(&text)
                    .map_err(|e| ExecError::SendFailed(format!("hyperliquid meta json: {e}")))?;
                let universe = v
                    .pointer("/universe")
                    .and_then(|u| u.as_array())
                    .ok_or_else(|| ExecError::SendFailed("hyperliquid meta: missing universe".into()))?;
                for (i, obj) in universe.iter().enumerate() {
                    if obj
                        .get("name")
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case(&self.perp_symbol))
                    {
                        return Ok(i as u32);
                    }
                }
                Err(ExecError::SendFailed(format!(
                    "Hyperliquid: could not resolve asset id for perp_symbol={} from /info meta (set --hyperliquid-asset-id)",
                    self.perp_symbol
                )))
            })
            .await
            .copied()
    }
}

fn decimal_from_price_cents(price_cents: u64) -> Result<Decimal, ExecError> {
    Ok(Decimal::from_i128_with_scale(price_cents as i128, 2))
}

fn decimal_from_qty_e8(qty_sats: u64) -> Result<Decimal, ExecError> {
    Ok(Decimal::from_i128_with_scale(qty_sats as i128, 8))
}

fn truncate_decimal_to_scale(d: Decimal, scale: u32) -> Decimal {
    // rust_decimal has built-in rescale, but it rounds; we want floor/truncate for safety.
    if d.scale() <= scale {
        return d;
    }
    // Multiply, truncate, divide.
    let factor = Decimal::from_i128_with_scale(10i128.pow(scale), 0);
    let scaled = (d * factor).trunc();
    // Avoid rescale surprises: create with target scale by dividing.
    (scaled / factor).rescale(scale);
    scaled / factor
}

fn ceil_decimal_to_scale(d: Decimal, scale: u32) -> Decimal {
    if d.scale() <= scale {
        return d;
    }
    let factor = Decimal::from_i128_with_scale(10i128.pow(scale), 0);
    let mut v = (d * factor).ceil() / factor;
    v.rescale(scale);
    v
}

fn round_hyperliquid_perp_px(px: Decimal, sz_decimals: u32) -> Result<Decimal, ExecError> {
    // Mirrors Hyperliquid python-sdk `examples/rounding.py` for perps:
    // - If px > 100k: round to integer
    // - Else: px = round(float(f"{px:.5g}"), 6 - szDecimals)
    let abs_px = px.abs();
    if abs_px > Decimal::from(100_000) {
        return Ok(px.round());
    }

    let max_decimals: u32 = 6;
    let frac_decimals = max_decimals.saturating_sub(sz_decimals);

    // 5 significant figures without scientific-notation string parsing (rust_decimal `FromStr`
    // does not accept `1.23e4` style inputs reliably).
    fn round_to_n_significant_figures(x: Decimal, n: u32) -> Decimal {
        if x.is_zero() {
            return x;
        }
        let ax = x.abs();
        let exp = ax.log10().floor();
        let k = exp - Decimal::from((n as i64) - 1);
        let factor = Decimal::TEN.powd(k);
        (x / factor).round() * factor
    }

    let rounded_sig = round_to_n_significant_figures(px, 5);

    let mut out = rounded_sig.round_dp(frac_decimals);
    out = out.normalize();
    Ok(out)
}

impl OrderExecutor for HyperliquidExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Hyperliquid
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        // Encode a minimal plan JSON so we keep build→sign→send shape.
        let asset = self.resolve_asset_index_hint();
        let is_buy = matches!(req.side, OrderSide::Buy);
        let limit_px = decimal_from_price_cents(req.price_cents)?;
        let sz = decimal_from_qty_e8(req.qty_sats)?;

        let root = serde_json::json!({
            "asset": asset,
            "symbol": req.symbol,
            "is_buy": is_buy,
            "limit_px": limit_px.to_string(),
            "sz": sz.to_string(),
            "reduce_only": req.reduce_only,
            "post_only": req.post_only
        });
        let body = serde_json::to_string(&root)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid json: {e}")))?;
        Ok(BuiltPayload {
            venue: Exchange::Hyperliquid,
            endpoint: "/exchange",
            body,
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        // Signing happens inside `send()` via hypersdk.
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body: built.body,
            signature: "hypersdk:sign-in-send".to_string(),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        if let Some(msg) = self.signer_err.as_deref() {
            // ExecError::MissingCredentials carries a `'static` message; use SendFailed for owned strings.
            return Err(ExecError::SendFailed(msg.to_string()));
        }
        let v: Value = serde_json::from_str(&signed.body)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid payload parse: {e}")))?;

        // Always resolve the correct universe index from `/info meta` unless explicitly provided.
        // This avoids the fragile "BTC is 0" assumption (especially on testnet).
        let asset = self.resolve_asset_index_from_meta().await?;
        let is_buy = v
            .get("is_buy")
            .and_then(|x| x.as_bool())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid: missing is_buy".into()))?;
        let mut limit_px: Decimal = v
            .get("limit_px")
            .and_then(|x| x.as_str())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid: missing limit_px".into()))?
            .parse()
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid: bad limit_px: {e}")))?;
        let mut sz: Decimal = v
            .get("sz")
            .and_then(|x| x.as_str())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid: missing sz".into()))?
            .parse()
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid: bad sz: {e}")))?;

        // Round/truncate to the venue lot step (szDecimals) for this asset.
        let sz_decimals = self.resolve_sz_decimals(asset).await?;
        let floored = truncate_decimal_to_scale(sz, sz_decimals);
        sz = if floored > Decimal::ZERO {
            floored
        } else {
            // If the requested size is smaller than the minimum step, flooring produces 0.
            // Round up to the minimum step so we can still satisfy min-notional bumping.
            ceil_decimal_to_scale(sz, sz_decimals)
        };
        if sz <= Decimal::ZERO {
            return Err(ExecError::SendFailed("hyperliquid: size rounded to 0".into()));
        }

        // Hyperliquid rejects limit orders that are too far from its reference/mid price.
        // Instead of submitting a doomed order, clamp into a conservative band around `allMids`.
        let mid = self.fetch_mid_px(&self.perp_symbol).await?;
        if mid > Decimal::ZERO {
            let band = Decimal::from_f64(1.0 - HL_MAX_DEVIATION_FROM_MID_FRAC)
                .ok_or_else(|| ExecError::SendFailed("hyperliquid: bad band (min)".into()))?;
            let buffer_lo = Decimal::from_f64(1.001)
                .ok_or_else(|| ExecError::SendFailed("hyperliquid: bad buffer (lo)".into()))?;
            let buffer_hi = Decimal::from_f64(0.999)
                .ok_or_else(|| ExecError::SendFailed("hyperliquid: bad buffer (hi)".into()))?;

            // Conservative band with a small safety margin to avoid edge-case rejects.
            let min_px = mid * band * buffer_lo;
            let max_px = mid
                * Decimal::from_f64(1.0 + HL_MAX_DEVIATION_FROM_MID_FRAC)
                    .ok_or_else(|| ExecError::SendFailed("hyperliquid: bad band (max)".into()))?
                * buffer_hi;

            // Clamp toward the nearest acceptable boundary while preserving side intent.
            // - Buys: too high → clamp down. Too low → clamp up (otherwise far-away junk prices).
            // - Sells: too low → clamp up. Too high → clamp down.
            let orig_limit_px = limit_px;
            if is_buy {
                if limit_px > max_px {
                    limit_px = max_px;
                } else if limit_px < min_px {
                    limit_px = min_px;
                }
            } else {
                if limit_px < min_px {
                    limit_px = min_px;
                } else if limit_px > max_px {
                    limit_px = max_px;
                }
            }
            if limit_px != orig_limit_px {
                tracing::warn!(
                    target: "execution",
                    exchange = "hyperliquid",
                    perp = %self.perp_symbol,
                    is_buy = is_buy,
                    mid = %mid,
                    min_px = %min_px,
                    max_px = %max_px,
                    orig_limit_px = %orig_limit_px,
                    clamped_limit_px = %limit_px,
                    "Clamped limit_px into HL band"
                );
            }
        }

        // Hyperliquid rejects orders below ~$10 notional. Because we truncate size to the venue
        // step, a "target $10" order can become $9.99... and get rejected; bump size to the
        // smallest step that clears the minimum.
        let min_notional = Decimal::from(HL_MIN_ORDER_NOTIONAL_USD);
        let mut notional = limit_px * sz;
        if notional < min_notional {
            if limit_px <= Decimal::ZERO {
                return Err(ExecError::SendFailed("hyperliquid: limit_px must be > 0".into()));
            }
            let required_sz = min_notional / limit_px;
            sz = ceil_decimal_to_scale(required_sz, sz_decimals);
            if sz <= Decimal::ZERO {
                return Err(ExecError::SendFailed("hyperliquid: bumped size rounded to 0".into()));
            }
        }

        let reduce_only = v.get("reduce_only").and_then(|x| x.as_bool()).unwrap_or(false);
        let post_only = v.get("post_only").and_then(|x| x.as_bool()).unwrap_or(true);

        // For IOC orders, ensure we actually cross the Hyperliquid spread so the order can match.
        // This is the main fix for "could not immediately match".
        if !post_only && self.ioc_cross_bps > 0 {
            let (best_bid, best_ask) = self.fetch_top_of_book(&self.perp_symbol).await?;
            let bps = Decimal::from(self.ioc_cross_bps as i64);
            let ten_k = Decimal::from(10_000);
            if is_buy {
                // Buy: cross above ask a bit.
                limit_px = best_ask * (Decimal::ONE + bps / ten_k);
            } else {
                // Sell: cross below bid a bit.
                limit_px = best_bid * (Decimal::ONE - bps / ten_k);
            }
        }

        // IOC crossing can introduce too many sigfigs/decimals for Hyperliquid's wire format.
        limit_px = round_hyperliquid_perp_px(limit_px, sz_decimals)?;

        // Re-check min notional after IOC repricing (size step rounding can interact with px rounding).
        notional = limit_px * sz;
        if notional < min_notional {
            if limit_px <= Decimal::ZERO {
                return Err(ExecError::SendFailed("hyperliquid: limit_px must be > 0".into()));
            }
            let required_sz = min_notional / limit_px;
            sz = ceil_decimal_to_scale(required_sz, sz_decimals);
            if sz <= Decimal::ZERO {
                return Err(ExecError::SendFailed("hyperliquid: bumped size rounded to 0".into()));
            }
        }

        let client = match self.network {
            Net::Testnet => hypercore::testnet(),
            Net::Mainnet => hypercore::mainnet(),
        };
        let signer = self.signer()?;

        let notional_usd = limit_px * sz;
        let mark_px = self.fetch_mark_px(asset).await.ok();
        tracing::info!(
            target: "execution",
            exchange = "hyperliquid",
            perp = %self.perp_symbol,
            asset = asset,
            is_buy = is_buy,
            limit_px = %limit_px,
            sz = %sz,
            notional_usd = %notional_usd,
            mid_px = %mid,
            mark_px = %mark_px.map(|x| x.to_string()).unwrap_or_else(|| "n/a".to_string()),
            "Place order"
        );

        let batch = BatchOrder {
            orders: vec![OrderRequest {
                asset: asset
                    .try_into()
                    .map_err(|_| ExecError::SendFailed("hyperliquid: asset id too large".into()))?,
                is_buy,
                limit_px,
                sz,
                reduce_only,
                order_type: OrderTypePlacement::Limit {
                    tif: if post_only { TimeInForce::Alo } else { TimeInForce::Ioc },
                },
                cloid: Default::default(),
            }],
            grouping: OrderGrouping::Na,
        };

        let nonce = Utc::now().timestamp_millis() as u64;
        let statuses = client
            .place(signer, batch, nonce, None, None)
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid place: {e}")))?;

        // Temporary debug: print the first order status so we can see fill vs resting vs error details.
        println!("hyperliquid status[0]: {:?}", statuses.first());

        if let Some(first) = statuses.first() {
            if !first.is_ok() {
                return Err(ExecError::SendFailed(format!("hyperliquid: {first:?}")));
            }
        }
        Ok(OrderAck {
            exchange: Exchange::Hyperliquid,
            client_order_id: statuses
                .first()
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "hyperliquid:ok".to_string()),
        })
    }
}

/// Hyperliquid leg of the purchase flow (credentials, Kraken co-existence with CEX feature).
pub(crate) struct HyperliquidPurchase;

impl HyperliquidPurchase {
    pub(crate) fn ensure(ctx: &super::ExecutorContext) -> Result<(), &'static str> {
        if ctx.has_hyperliquid() {
            Ok(())
        } else {
            Err("Missing `--hyperliquid-private-key`")
        }
    }

    /// Whether this route touches Kraken limit-order execution (only meaningful with `cex`).
    pub(crate) fn route_hits_unsupported_kraken_legs(buy: Exchange, sell: Exchange) -> bool {
        #[cfg(feature = "cex")]
        {
            buy == Exchange::Kraken || sell == Exchange::Kraken
        }
        #[cfg(not(feature = "cex"))]
        {
            let _ = (buy, sell);
            false
        }
    }
}

impl super::PurchaseVenueModule for HyperliquidPurchase {
    fn preflight(ctx: &super::ExecutorContext) -> Result<(), &'static str> {
        Self::ensure(ctx)
    }
}
