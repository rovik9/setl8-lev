# GMX Oracle API spec (from docs.gmx.io, read 2026-10-10; items marked UNVERIFIED must be proven by the probe, prompt P5)

Hosts: primary `arbitrum-api.gmxinfra.io`, `avalanche-api.gmxinfra.io`. Fallbacks: `arbitrum-api-fallback.gmxinfra.io`, `arbitrum-api-fallback.gmxinfra2.io` (Avalanche the same pattern). GMX API v1 peers (user-facing, `/v1` paths): `arbitrum.gmxapi.io` and `arbitrum.gmxapi.ai`. Botanix is gone. All GET. No key. NO published rate limit, NO public SLA ("implement retries and fallback in your app"). 10 s route timeout.

| Endpoint | Params | Returns | Cache |
|---|---|---|---|
| `/ping` | none | `{ok:true}` or 500 `{errors:[..]}` (db down) | none |
| `/prices/tickers` | none | array: tokenSymbol, tokenAddress, minPrice, maxPrice, medianPrice (strings), oracleDecimals, updatedAt | 0.3 s |
| `/signed_prices/latest` | none | `{signedPrices:[..]}`: tokenSymbol, tokenAddress, min/max/medianPrice, oracleDecimals, signer, signature, signatureWithoutBlockHash, salt, min/maxBlockHash, tokenOracleType, min/maxBlockNumber, min/maxBlockTimestamp, createdAt (+ `blob`, `oracleType`, `feedVersion` seen live) | 1 s |
| `/prices/candles` | tokenSymbol, period (1m 5m 15m 1h 4h 1d), limit (1 to 10000, default 1000) | `{period, candles:[[ts_sec, o, h, l, c], ...]}` newest first, plain numbers | 15 s (1m, 5m), 60 s others |
| `/tokens` | none | configured token list (fields undocumented; decimals expected) | n/a |
| `/markets`, `/markets/info` | none | catalog; near-live state, isListed | 10 s |
| `/markets/state` | none | market status (open/closed/unknown), closingPrice, latestMarketStatusTransitionTimestamp | 5 s |
| `/markets/tickers` | addresses, symbols | filtered tickers (400 on bad values) | n/a |

Price scaling (docs): strings scaled to 30 decimals; divide by 10^(30 - tokenDecimals - oracleDecimals) per the docs, but the docs' own ETH example does not obviously work. UNVERIFIED: the probe must find the correct conversion by comparing with candles and a sanity range. Observed live: APE minPrice "151169140215" is plausible as 0.151 USD if USD = value / 10^(30 - 18), i.e. oracleDecimals is NOT subtracted. Prove it.
`updatedAt` units: seconds in the docs example, live sample was milliseconds in tickers (1791632589793) with a separate seconds `timestamp`. UNVERIFIED.

Coverage seen live (Arbitrum tickers, 114): ~104 crypto; GOLD, SILVER, XAUT, XAUT.v2; WTIOIL, BRENTOIL, NATGAS; SPY, QQQ, SPCX. NO forex. Avalanche tickers: 17 (crypto and XAUt0). `/markets/info`: 82 entries (81 listed), many duplicate index tokens in different pools.
`/signed_prices/latest` sample (26): USDG, SPY, DYDX, PENDLE, XMR, MEGA, SUI, ORDI, BONK, NEAR, PEPE, ASTER, SOL, XPL, WBTC.b, TRUMP, ANIME, TIA, FIL, CVX, SHIB, APT, DASH, BTC, AR, PENGU. Each has a Chainlink report blob, oracleType realtimeFeed2, feedVersion v3, GMX's own signature fields null.

Licensing: no terms found for API use or republishing. Rovik's decision (43): accept the risk until Setl8 earns revenue, then buy a licensed feed. Claude's one-time warning is in design-decisions.md.
