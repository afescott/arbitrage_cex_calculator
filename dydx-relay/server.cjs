/**
 * Local dYdX v4 order relay: accepts the Rust bot's planning JSON and places a limit order
 * via @dydxprotocol/v4-client-js (CompositeClient). Uses CommonJS because the package ESM
 * build omits `.js` extensions and fails under Node resolution.
 *
 * Run (from this directory):
 *   npm install
 *   DYDX_PRIVATE_KEY=0xabc... node server.cjs
 *   # or: DYDX_MNEMONIC="twelve words ..."
 *
 * Bot:
 *   --dydx-order-relay-url http://127.0.0.1:8787/
 *
 * Optional env:
 *   DYDX_RELAY_HOST (default 127.0.0.1)  DYDX_RELAY_PORT (default 8787)
 *   DYDX_NETWORK=mainnet|testnet (default mainnet)
 *   DYDX_VALIDATOR_REST  DYDX_INDEXER_REST  DYDX_INDEXER_WS  DYDX_CHAIN_ID
 *   DYDX_GOOD_TIL_SECONDS (default 300; used for post-only / long-term limits)
 *
 * Security: keep this on localhost; it holds signing material in env.
 */

'use strict';

const http = require('node:http');
const { randomInt } = require('node:crypto');
const {
  CompositeClient,
  LocalWallet,
  Network,
  OrderSide,
  OrderTimeInForce,
  OrderType,
  SubaccountInfo,
  ValidatorConfig,
  IndexerConfig,
} = require('@dydxprotocol/v4-client-js');

const BECH32_PREFIX = 'dydx';

async function loadWallet() {
  const pk = process.env.DYDX_PRIVATE_KEY?.trim();
  const mnemonic = process.env.DYDX_MNEMONIC?.trim();
  if (pk) {
    return LocalWallet.fromPrivateKey(pk.replace(/^0x/i, ''), BECH32_PREFIX);
  }
  if (mnemonic) {
    return LocalWallet.fromMnemonic(mnemonic, BECH32_PREFIX);
  }
  throw new Error('Set DYDX_PRIVATE_KEY (hex) or DYDX_MNEMONIC');
}

function buildNetwork() {
  const env = (process.env.DYDX_NETWORK || 'mainnet').toLowerCase();
  const base = env === 'testnet' ? Network.testnet() : Network.mainnet();
  const vRest = process.env.DYDX_VALIDATOR_REST || base.validatorConfig.restEndpoint;
  const iRest = process.env.DYDX_INDEXER_REST || base.indexerConfig.restEndpoint;
  const iWs = process.env.DYDX_INDEXER_WS || base.indexerConfig.websocketEndpoint;
  const chainId = process.env.DYDX_CHAIN_ID || base.validatorConfig.chainId;
  const validatorConfig = new ValidatorConfig(
    vRest,
    chainId,
    base.validatorConfig.denoms,
    base.validatorConfig.broadcastOptions,
    base.validatorConfig.defaultClientMemo,
    base.validatorConfig.useTimestampNonce,
    base.validatorConfig.timestampNonceOffsetMs,
  );
  const indexerConfig = new IndexerConfig(iRest, iWs, base.indexerConfig.proxy);
  return new Network(base.env, indexerConfig, validatorConfig);
}

function parsePlanningJson(body) {
  const j = JSON.parse(body);
  const ticker = j.market_ticker;
  const priceCents = j.price_cents_internal;
  const sizeStr = j.order?.quantums;
  const sideStr = j.order?.side;
  if (typeof ticker !== 'string' || !ticker.length) {
    throw new Error('market_ticker required');
  }
  if (priceCents == null) {
    throw new Error('price_cents_internal required');
  }
  if (sizeStr == null) {
    throw new Error('order.quantums required (human base size decimal string)');
  }
  if (typeof sideStr !== 'string') {
    throw new Error('order.side required');
  }
  const price = Number(priceCents) / 100;
  const size = Number(sizeStr);
  if (!Number.isFinite(price) || price <= 0) {
    throw new Error('invalid price_cents_internal');
  }
  if (!Number.isFinite(size) || size <= 0) {
    throw new Error('invalid order.quantums (size)');
  }
  let postOnly = j.post_only;
  if (postOnly === undefined) {
    postOnly = j.order?.time_in_force === 'TIME_IN_FORCE_POST_ONLY';
  }
  if (postOnly === undefined) {
    postOnly = true;
  }
  const reduceOnly = j.reduce_only ?? j.order?.reduce_only ?? false;
  let side;
  if (sideStr === 'SIDE_BUY') {
    side = OrderSide.BUY;
  } else if (sideStr === 'SIDE_SELL') {
    side = OrderSide.SELL;
  } else {
    throw new Error('order.side must be SIDE_BUY or SIDE_SELL');
  }
  return { ticker, price, size, side, postOnly, reduceOnly };
}

function txHashFromResult(tx) {
  if (!tx || typeof tx !== 'object') {
    return String(tx);
  }
  return tx.transactionHash || tx.txhash || tx.hash || JSON.stringify(tx);
}

async function main() {
  const wallet = await loadWallet();
  const network = buildNetwork();
  const client = await CompositeClient.connect(network);
  const subaccount = SubaccountInfo.forLocalWallet(wallet, 0);

  const host = process.env.DYDX_RELAY_HOST || '127.0.0.1';
  const port = Number(process.env.DYDX_RELAY_PORT || '8787');
  const goodTilSeconds = Number(process.env.DYDX_GOOD_TIL_SECONDS || '300');

  const server = http.createServer(async (req, res) => {
    if (req.method !== 'POST') {
      res.writeHead(405, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: 'POST only' }));
      return;
    }
    let chunks = '';
    for await (const c of req) {
      chunks += c;
    }
    try {
      const plan = parsePlanningJson(chunks);
      const clientId = randomInt(1, 0xffffffff);

      let tx;
      if (plan.postOnly) {
        tx = await client.placeOrder(
          subaccount,
          plan.ticker,
          OrderType.LIMIT,
          plan.side,
          plan.price,
          plan.size,
          clientId,
          OrderTimeInForce.GTT,
          goodTilSeconds,
          undefined,
          true,
          plan.reduceOnly,
        );
      } else {
        tx = await client.placeOrder(
          subaccount,
          plan.ticker,
          OrderType.LIMIT,
          plan.side,
          plan.price,
          plan.size,
          clientId,
          OrderTimeInForce.IOC,
          undefined,
          undefined,
          false,
          plan.reduceOnly,
        );
      }
      const txHash = txHashFromResult(tx);
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(
        JSON.stringify({
          txHash,
          txhash: txHash,
          client_order_id: String(clientId),
        }),
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      res.writeHead(500, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: msg }));
    }
  });

  server.listen(port, host, () => {
    console.error(`dydx-relay listening on http://${host}:${port}/ (POST planning JSON)`);
  });
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
