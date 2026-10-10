# Hyperliquid (HIP-3 builder markets) price source spec (docs read 2026-10-10; UNVERIFIED items must be proven by probe P6)

REST: `POST https://api.hyperliquid.xyz/info`, `Content-Type: application/json`, no key. Rate limit (docs): all REST requests share 1200 weight per minute per IP; `allMids` and `l2Book` weigh 2, most other info calls 20, `candleSnapshot` adds weight per 60 items.
WebSocket (docs): per IP max 10 connections, 30 new connections per minute, 1000 subscriptions, 2000 messages per minute sent to Hyperliquid. Prefer WebSocket for prices.

Calls used
- `{"type":"perpDexs"}` list the perp DEXs; `{"type":"allPerpMetas"}` metadata for all of them; `{"type":"meta","dex":"xyz"}` and `{"type":"metaAndAssetCtxs","dex":"xyz"}` (ctx fields: markPx, midPx, oraclePx, openInterest, funding, premium, prevDayPx, dayNtlVlm, impactPxs).
- `{"type":"allMids","dex":"xyz"}` coin to mid price string (empty book falls back to last trade).
- `{"type":"l2Book","coin":"xyz:XYZ100"}` up to 20 levels per side; docs do not say which array is bids, assume [bids, asks] and PROVE it.
- `{"type":"candleSnapshot","req":{"coin":..,"interval":"1m","startTime":ms,"endTime":ms}}` only the latest 5000 candles; the page also says responses cap at 500 elements: UNVERIFIED.
- WS subscribe: `{"method":"subscribe","subscription":{"type":"allMids","dex":"xyz"}}`; also `bbo` (coin), `l2Book`, `trades`, `candle`, `activeAssetCtx`. Messages carry `channel`; data in `data`.
- HIP-3 coin names carry the dex prefix, e.g. `xyz:XYZ100`. The docs do not show `xyz:AAPL`: UNVERIFIED.

Listing seen on hyperliquidguide.com (third party, trade.xyz markets, not authoritative, counts do not reconcile): equities about 88 single stocks plus ETFs (AAPL, AMD, AMZN, COIN, GOOGL, META, MSFT, MSTR, NFLX, NVDA, PLTR, TSLA, ...; ETFs EWJ, EWY, EWZ, SMH, TLT, XLE, ...), indices SP500, XYZ100, JP225, KR200, NIFTY, IBOV, VIX, DXY, FX EUR, GBP, JPY, KRW, commodities GOLD, SILVER, COPPER, PLATINUM, PALLADIUM, ALUMINIUM, BRENTOIL, CL, HO, NATGAS, TTF, CORN, WHEAT, URANIUM. Other builders (Felix and more) exist and are NOT listed there.

Risks (docs): the oracle price is set by the market's DEPLOYER, not by a signed vendor message; the price is an exchange mark/mid, not a signed quote; HIP-3 docs admit most price indices are poor oracle sources and halts (`haltTrading`) can settle positions. Closed-underlying behaviour (weekends, holidays) is not documented: probe must record it. No SLA and no terms found for API reuse; same licence-risk rule as GMX (decision 43).
