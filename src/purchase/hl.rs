//! Hyperliquid purchase: preflight, route checks, and limit-order build → sign → POST `/exchange`.

use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    BuiltPayload, ExecError, LimitOrderRequest, OrderAck, OrderExecutor, OrderSide, SignedPayload,
};
use crate::orderbook::book::Exchange;

const HYPERLIQUID_EXCHANGE_URL: &str = "https://api.hyperliquid.xyz/exchange";

#[derive(Debug, Clone)]
pub(crate) struct HyperliquidExecutor {
    private_key: String,
    /// `meta.universe` index; if `None` and `perp_symbol == "BTC"`, [`resolve_asset_index`] uses `0`.
    asset_index: Option<u32>,
    perp_symbol: String,
    http: reqwest::Client,
}

impl HyperliquidExecutor {
    pub(crate) fn new(private_key: String, asset_index: Option<u32>, perp_symbol: String) -> Self {
        let http = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest hyperliquid client");
        Self {
            private_key,
            asset_index,
            perp_symbol,
            http,
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

fn hl_nonce_ms() -> Result<u64, ExecError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|_| ExecError::SendFailed("clock".into()))
}

fn hl_price_decimal_from_cents(price_cents: u64) -> String {
    let dollars = price_cents / 100;
    let cents = price_cents % 100;
    if cents == 0 {
        dollars.to_string()
    } else if cents % 10 == 0 {
        format!("{}.{:01}", dollars, cents / 10)
    } else {
        format!("{}.{:02}", dollars, cents)
    }
}

fn hl_position_side_label(body: &str) -> Option<&'static str> {
    let v: Value = serde_json::from_str(body).ok()?;
    let b = v.pointer("/action/orders/0/b")?.as_bool()?;
    Some(if b { "long" } else { "short" })
}

fn parse_hyperliquid_order_response(text: &str) -> Result<OrderAck, ExecError> {
    let v: Value = serde_json::from_str(text).map_err(|e| {
        ExecError::SendFailed(format!(
            "hyperliquid: bad JSON ({e}), body: {}",
            text.chars().take(500).collect::<String>()
        ))
    })?;
    if v.get("status").and_then(|s| s.as_str()) != Some("ok") {
        return Err(ExecError::SendFailed(format!(
            "hyperliquid: {}",
            text.chars().take(1000).collect::<String>()
        )));
    }
    let statuses = v
        .pointer("/response/data/statuses")
        .and_then(|x| x.as_array())
        .ok_or_else(|| {
            ExecError::SendFailed(format!(
                "hyperliquid: missing response.data.statuses: {}",
                text.chars().take(500).collect::<String>()
            ))
        })?;
    let first = statuses
        .first()
        .ok_or_else(|| ExecError::SendFailed("hyperliquid: empty response.data.statuses".into()))?;
    if let Some(err) = first.get("error").and_then(|e| e.as_str()) {
        return Err(ExecError::SendFailed(format!("hyperliquid: {err}")));
    }
    let oid = first
        .get("resting")
        .and_then(|r| r.get("oid"))
        .or_else(|| first.get("filled").and_then(|f| f.get("oid")));
    let client_order_id = if let Some(oidv) = oid {
        oidv.as_u64()
            .map(|u| u.to_string())
            .or_else(|| oidv.as_i64().map(|i| i.to_string()))
            .or_else(|| oidv.as_str().map(std::string::ToString::to_string))
            .unwrap_or_else(|| "hyperliquid-ok".to_string())
    } else {
        "hyperliquid-ok".to_string()
    };
    Ok(OrderAck {
        exchange: Exchange::Hyperliquid,
        client_order_id,
    })
}

impl OrderExecutor for HyperliquidExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Hyperliquid
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        let asset = self.resolve_asset_index()?;
        let is_buy = matches!(req.side, OrderSide::Buy);
        let tif = if req.post_only { "Alo" } else { "Gtc" };
        let action = json!({
            "type": "order",
            "orders": [{
                "a": asset,
                "b": is_buy,
                "p": hl_price_decimal_from_cents(req.price_cents),
                "s": super::qty_sats_to_decimal_string(req.qty_sats),
                "r": req.reduce_only,
                "t": { "limit": { "tif": tif } }
            }],
            "grouping": "na"
        });
        let nonce = hl_nonce_ms()?;
        let root = json!({
            "action": action,
            "nonce": nonce
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
        let mut root: Value = serde_json::from_str(&built.body)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid parse: {e}")))?;
        root["signature"] = json!({
            "r": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "s": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "v": 27
        });
        let body = serde_json::to_string(&root)
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid json: {e}")))?;
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body,
            signature: format!("hyperliquid-stub-ecdsa:pk_len={}", self.private_key.len()),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let side = hl_position_side_label(&signed.body).unwrap_or("unknown");
        tracing::info!(
            target: "execution",
            exchange = "hyperliquid",
            side,
            "POST {}",
            HYPERLIQUID_EXCHANGE_URL
        );
        let resp = self
            .http
            .post(HYPERLIQUID_EXCHANGE_URL)
            .header(CONTENT_TYPE, "application/json")
            .body(signed.body)
            .send()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid http: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ExecError::SendFailed(format!("hyperliquid body: {e}")))?;
        if !status.is_success() {
            return Err(ExecError::SendFailed(format!(
                "hyperliquid HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            )));
        }
        parse_hyperliquid_order_response(&text)
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
