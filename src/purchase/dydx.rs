//! dYdX purchase: second-venue preflight and planning JSON → sign stub → optional relay POST.

use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};

use super::{
    BuiltPayload, ExecError, LimitOrderRequest, OrderAck, OrderExecutor, OrderSide, SignedPayload,
};
use crate::orderbook::book::Exchange;

#[derive(Debug, Clone)]
pub(crate) struct DydxExecutor {
    private_key: Option<String>,
    order_relay_url: Option<String>,
    http: reqwest::Client,
}

impl DydxExecutor {
    pub(crate) fn new(private_key: Option<String>, order_relay_url: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest dydx client");
        Self {
            private_key,
            order_relay_url,
            http,
        }
    }
}

fn dydx_position_side_label(body: &str) -> Option<&'static str> {
    let v: Value = serde_json::from_str(body).ok()?;
    match v.pointer("/order/side")?.as_str()? {
        "SIDE_BUY" => Some("long"),
        "SIDE_SELL" => Some("short"),
        _ => None,
    }
}

impl OrderExecutor for DydxExecutor {
    fn venue(&self) -> Exchange {
        Exchange::Dydx
    }

    fn build_payload(&self, req: &LimitOrderRequest) -> Result<BuiltPayload, ExecError> {
        let market = if req.symbol.contains('-') {
            req.symbol.clone()
        } else {
            format!("{}-USD", req.symbol)
        };
        let is_buy = matches!(req.side, OrderSide::Buy);
        let root = json!({
            "_comment": "Relay: POST this JSON to --dydx-order-relay-url; `order.quantums` is human base size (same decimal string as Hyperliquid qty), not chain quantums.",
            "msg_type": "dydxprotocol.clob.MsgPlaceOrder",
            "post_only": req.post_only,
            "reduce_only": req.reduce_only,
            "order": {
                "order_id": {
                    "subaccount_id": {
                        "owner": "<0x... from wallet>",
                        "number": 0
                    },
                    "client_id": 0,
                    "order_flags": 64,
                    "clob_pair_id": 0
                },
                "side": if is_buy { "SIDE_BUY" } else { "SIDE_SELL" },
                "quantums": super::qty_sats_to_decimal_string(req.qty_sats),
                "subticks": "<u64: price in subticks; resolve from clob pair + tick size>",
                "good_til_block": 0,
                "good_til_block_time": 0,
                "time_in_force": if req.post_only { "TIME_IN_FORCE_POST_ONLY" } else { "TIME_IN_FORCE_UNSPECIFIED" },
                "reduce_only": req.reduce_only,
                "client_metadata": 0,
                "condition_type": "CONDITION_TYPE_UNSPECIFIED",
                "conditional_order_trigger_subticks": "0",
                "order_router_address": ""
            },
            "market_ticker": market,
            "price_cents_internal": req.price_cents
        });
        let body = serde_json::to_string(&root)
            .map_err(|e| ExecError::SendFailed(format!("dydx json: {e}")))?;
        Ok(BuiltPayload {
            venue: Exchange::Dydx,
            endpoint: "<chain broadcast MsgPlaceOrder>",
            body,
        })
    }

    fn sign(&self, built: BuiltPayload) -> Result<SignedPayload, ExecError> {
        Ok(SignedPayload {
            venue: built.venue,
            endpoint: built.endpoint,
            body: built.body,
            signature: format!(
                "dydx-relay:local_pk_len={}",
                self.private_key.as_deref().map(|s| s.len()).unwrap_or(0)
            ),
        })
    }

    async fn send(&self, signed: SignedPayload) -> Result<OrderAck, ExecError> {
        let relay = self.order_relay_url.as_deref().ok_or_else(|| {
            ExecError::SendFailed(
                "dYdX: set --dydx-order-relay-url to POST planning JSON, or implement Cosmos tx_bytes broadcast in send()"
                    .into(),
            )
        })?;
        let side = dydx_position_side_label(&signed.body).unwrap_or("unknown");
        tracing::info!(
            target: "execution",
            exchange = "dydx",
            side,
            url = relay,
            "POST order relay"
        );
        let resp = self
            .http
            .post(relay)
            .header(CONTENT_TYPE, "application/json")
            .body(signed.body)
            .send()
            .await
            .map_err(|e| ExecError::SendFailed(format!("dydx relay http: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ExecError::SendFailed(format!("dydx relay body: {e}")))?;
        if !status.is_success() {
            return Err(ExecError::SendFailed(format!(
                "dydx relay HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            )));
        }
        let client_order_id = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("client_order_id")
                    .or(v.get("clientOrderId"))
                    .or(v.get("txhash"))
                    .or(v.get("txHash"))
                    .and_then(|x| {
                        x.as_str()
                            .map(std::string::ToString::to_string)
                            .or_else(|| x.as_u64().map(|u| u.to_string()))
                    })
            })
            .unwrap_or_else(|| {
                if text.len() > 120 {
                    format!("dydx-relay:{}", text.chars().take(120).collect::<String>())
                } else {
                    text.clone()
                }
            });
        Ok(OrderAck {
            exchange: Exchange::Dydx,
            client_order_id,
            filled_qty_e8: None,
            venue_order_id: None,
        })
    }
}

/// Second-venue checks when Hyperliquid is paired with dYdX (and optionally Binance with `cex`).
pub(crate) struct DydxPurchase;

impl DydxPurchase {
    pub(crate) fn ensure_second_venue(ctx: &super::ExecutorContext) -> Result<(), &'static str> {
        #[cfg(feature = "cex")]
        {
            if ctx.has_binance() || ctx.has_dydx() {
                Ok(())
            } else {
                Err(
                    "Missing second venue: add `--binance-api-key/--binance-api-secret` and/or `--dydx-private-key`",
                )
            }
        }
        #[cfg(not(feature = "cex"))]
        {
            if ctx.has_dydx() {
                Ok(())
            } else {
                Err("Missing dYdX config: set `--dydx-order-relay-url http://127.0.0.1:8787/` (relay) or `--dydx-private-key` (local signing); build with `--features cex` for Binance/Kraken")
            }
        }
    }
}

impl super::PurchaseVenueModule for DydxPurchase {
    fn preflight(ctx: &super::ExecutorContext) -> Result<(), &'static str> {
        Self::ensure_second_venue(ctx)
    }
}
