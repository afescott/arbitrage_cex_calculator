//! Hyperliquid purchase: preflight, route checks, and real order placement via `hypersdk`.
//!
//! This replaces the previous stub `r/s/v` signing. `hypersdk` builds the L1 action, signs it,
//! and submits it to `/exchange`.

use chrono::Utc;
use hypersdk::hypercore::{self, types::*, PrivateKeySigner};
use rust_decimal::Decimal;
use serde_json::Value;

use super::{BuiltPayload, ExecError, LimitOrderRequest, OrderAck, OrderExecutor, OrderSide, SignedPayload};
use crate::orderbook::book::Exchange;

const HL_MIN_ORDER_NOTIONAL_USD: u64 = 10;

#[derive(Debug, Clone)]
pub(crate) struct HyperliquidExecutor {
    signer: Option<PrivateKeySigner>,
    signer_err: Option<String>,
    /// `meta.universe` index; if `None` and `perp_symbol == "BTC"`, [`resolve_asset_index`] uses `0`.
    asset_index: Option<u32>,
    perp_symbol: String,
    network: String,
}

impl HyperliquidExecutor {
    pub(crate) fn new(
        private_key: String,
        asset_index: Option<u32>,
        perp_symbol: String,
        network: String,
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
        Self {
            signer,
            signer_err,
            asset_index,
            perp_symbol,
            network,
        }
    }

    fn signer(&self) -> Result<&PrivateKeySigner, ExecError> {
        self.signer.as_ref().ok_or_else(|| {
            ExecError::MissingCredentials(
                "Invalid `--hyperliquid-private-key` (must be 32-byte hex, e.g. 0x… with 64 hex chars)",
            )
        })
    }

    fn resolve_asset_index(&self) -> Result<u32, ExecError> {
        if let Some(i) = self.asset_index {
            return Ok(i);
        }
        if self.perp_symbol == "BTC" {
            return Ok(0);
        }
        Err(ExecError::SendFailed(format!(
            "Hyperliquid: set --hyperliquid-asset-id for perp_symbol={}",
            self.perp_symbol
        )))
    }

    fn qty_step_decimals(&self) -> Option<u32> {
        if self.perp_symbol == "BTC" {
            Some(5) // from `info/meta`: BTC szDecimals=5
        } else {
            None
        }
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

impl OrderExecutor for HyperliquidExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Hyperliquid
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        // Encode a minimal plan JSON so we keep build→sign→send shape.
        let asset = self.resolve_asset_index()?;
        let is_buy = matches!(req.side, OrderSide::Buy);
        let limit_px = decimal_from_price_cents(req.price_cents)?;
        let mut sz = decimal_from_qty_e8(req.qty_sats)?;
        if let Some(decimals) = self.qty_step_decimals() {
            sz = truncate_decimal_to_scale(sz, decimals);
        }
        if sz <= Decimal::ZERO {
            return Err(ExecError::SendFailed("hyperliquid: size rounded to 0".into()));
        }

        // Hyperliquid rejects orders below ~$10 notional. Because we truncate size to the venue
        // step, a "target $10" order can become $9.99... and get rejected; bump size to the
        // smallest step that clears the minimum.
        let min_notional = Decimal::from(HL_MIN_ORDER_NOTIONAL_USD);
        let notional = limit_px * sz;
        if notional < min_notional {
            if limit_px <= Decimal::ZERO {
                return Err(ExecError::SendFailed("hyperliquid: limit_px must be > 0".into()));
            }
            let required_sz = min_notional / limit_px;
            sz = if let Some(decimals) = self.qty_step_decimals() {
                ceil_decimal_to_scale(required_sz, decimals)
            } else {
                required_sz
            };
            if sz <= Decimal::ZERO {
                return Err(ExecError::SendFailed("hyperliquid: bumped size rounded to 0".into()));
            }
        }

        let root = serde_json::json!({
            "asset": asset,
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

        let asset = v
            .get("asset")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid: missing asset".into()))? as u32;
        let is_buy = v
            .get("is_buy")
            .and_then(|x| x.as_bool())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid: missing is_buy".into()))?;
        let limit_px: Decimal = v
            .get("limit_px")
            .and_then(|x| x.as_str())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid: missing limit_px".into()))?
            .parse()
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid: bad limit_px: {e}")))?;
        let sz: Decimal = v
            .get("sz")
            .and_then(|x| x.as_str())
            .ok_or_else(|| ExecError::SendFailed("hyperliquid: missing sz".into()))?
            .parse()
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid: bad sz: {e}")))?;
        let reduce_only = v.get("reduce_only").and_then(|x| x.as_bool()).unwrap_or(false);
        let post_only = v.get("post_only").and_then(|x| x.as_bool()).unwrap_or(true);

        let client = match self.network.as_str() {
            "testnet" => hypercore::testnet(),
            _ => hypercore::mainnet(),
        };
        let signer = self.signer()?;
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
