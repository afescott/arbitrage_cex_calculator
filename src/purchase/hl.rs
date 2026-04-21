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

#[derive(Debug, Clone)]
pub(crate) struct HyperliquidExecutor {
    signer: PrivateKeySigner,
    /// `meta.universe` index; if `None` and `perp_symbol == "BTC"`, [`resolve_asset_index`] uses `0`.
    asset_index: Option<u32>,
    perp_symbol: String,
}

impl HyperliquidExecutor {
    pub(crate) fn new(private_key: String, asset_index: Option<u32>, perp_symbol: String) -> Self {
        let signer: PrivateKeySigner = private_key
            .parse()
            .expect("hyperliquid private key parse (hypersdk)");
        Self {
            signer,
            asset_index,
            perp_symbol,
        }
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
}

fn decimal_from_price_cents(price_cents: u64) -> Result<Decimal, ExecError> {
    Ok(Decimal::from_i128_with_scale(price_cents as i128, 2))
}

fn decimal_from_qty_e8(qty_sats: u64) -> Result<Decimal, ExecError> {
    Ok(Decimal::from_i128_with_scale(qty_sats as i128, 8))
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
        let sz = decimal_from_qty_e8(req.qty_sats)?;
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

        let client = hypercore::mainnet();
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
                    tif: if post_only { TimeInForce::Alo } else { TimeInForce::Gtc },
                },
                cloid: Default::default(),
            }],
            grouping: OrderGrouping::Na,
        };

        let nonce = Utc::now().timestamp_millis() as u64;
        let statuses = client
            .place(&self.signer, batch, nonce, None, None)
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid place: {e}")))?;

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
