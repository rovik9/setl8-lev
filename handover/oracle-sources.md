# Price source research (2026-10-10), facts only; UNVERIFIED items marked

## Stork (docs repo Stork-Oracle/Documentation, read via raw files)
- Access: "contact Stork Labs" (sales@stork.network, X @StorkOracle). No public signup, price or KYC statement found.
- Websocket `wss://api.jp.stork-oracle.network/evm/subscribe`, header `Authorization: Basic <token>`; subscribe `{"type":"subscribe","data":["BTCUSD",...]}`; update has `price`, `timestamp`, `stork_signed_price{public_key, encoded_asset_id, price, timestamped_signature{signature{r,s,v}, timestamp, msg_hash}, publisher_merkle_root, calculation_alg}` and per-publisher `signed_prices`.
- Coverage from the asset registry: ~131 crypto perp, ~227 crypto spot, ~219 equities (`_24_5` suffix), FX table truncated at ~11 (AUDUSD, EURUSD, GBPUSD, JPYUSD, USDCHF, USDCNH...). No XAUUSD (only gold tokens XAUT/XAUM), no index feeds. New assets: "as little as 24 hours" on request.
- Off-hours: metals/energy switch to a perp-venue derived price when the traditional market is closed (24-7 feeds doc).

## GMTrade (gmsol-labs/gmx-solana)
- Prices come from Chainlink Data Streams (RWA Advanced v11 for gold XUG, silver XAG, WTI), pulled at each execution. 60+ markets.
- The program's Oracle account persists but its prices are CLEARED after every execution (source: programs/store/src/states/oracle/mod.rs). There is nothing on chain to read between trades. Answer to "read GMTrade's on-chain data as our feed": no.

## GMX public API (arbitrum-api.gmxinfra.io), fetched 2026-10-10
- No key needed (none mentioned). `/prices/tickers`: 114 assets, unsigned min/max price: crypto ~104, GOLD, SILVER, XAUT, SPY, QQQ, SPCX, WTIOIL, BRENTOIL, NATGAS. NO forex.
- `/signed_prices/latest`: 26 entries in the sample, each with a Chainlink report `blob` (oracleType realtimeFeed2, feedVersion v3); GMX's own signature fields are null. Meant for keepers sending GMX transactions, not a data product: no SLA, terms unchecked, can be rate-limited or changed.
- Verifying a Chainlink report on Solana needs Chainlink's verifier program and its access terms (UNVERIFIED).

## Not yet checked
- Chainlink Data Streams direct access terms and KYC (mainnet access is by request, unverified).
- Pyth: docs say "Hermes now requires an API Key"; terms unverified.
- Switchboard On-Demand: unchecked.

## FX and indices, cheap first (checked 2026-10-10; Rovik chose "cheap first, signed proofs optional")
- Pyth (app.pyth.com/plans): Free $0 = view only, no API. Starter $500/mo = API, all crypto only, 1 s. Pro from $2,500/mo = all symbols, 1 ms, limited redistribution. Blog bundles: FX $5,000, FX G10 $2,500, FX+Metals $6,500. Pyth Indices: contact team. Payment methods and KYC not stated.
- Chainlink Data Streams: subscription, no prices listed (contact form), LINK payment mentioned. RWA covers equities, FX, precious metals, commodities. Indices not mentioned. KYC not stated.
- RedStone Live (docs.redstone.finance/docs/stage1-market-data/live/overview): WebSocket, "cryptographically verifiable", forex majors and EM, major equity indices, commodities, metals, equities, top-20 crypto. API key by contacting RedStone. Price, Solana verification and KYC not stated. Best coverage match, price unknown.
- Twelve Data: Basic $0 (real-time FX, 8 credits/min, 800/day), Grow $79, Pro $229 (about 500+ websocket credits), Ultra $999. Cards and PayPal. Indices not mentioned on the page. Not signed.
- TraderMade: FX and crypto GBP 599/mo, 40 CFD symbols GBP 599/mo, websocket. Index coverage unconfirmed. Not signed.
- Massive (ex Polygon): only Stocks plans visible ($29 to $199, "individual use / non-pros only"). FX and Indices pages not read.
- LICENSING RISK for every vendor: posting a price proof on chain republishes the vendor's data. Cheap plans are usually "internal / individual use". Ask each vendor for redistribution terms in writing before relying on it.

## Weekday probe 2026-10-16 (Fri 18:37 to 18:52 UTC, late US session; the open hour is NOT covered) and the source decisions it led to
Raw reports: `handover/hl-oracle-probe-report.md`, `handover/gmx-probe-weekday-report.md`. These decisions are numbered 54 to 57 here because decision 53 is taken by P17; fold them into `design-decisions.md` after P17 is committed.

**54. HL-sourced markets: proof time = evaluator receive time.** `oraclePx` carries no server timestamp (WebSocket and REST, checked). `max_age_secs` therefore counts from our own receive time for source 4; the weakness (a delayed or replayed message looks fresh) is part of the residual trust of option C and goes into the M12 report.
**55. One source per market, final for v1.** GMX (Chainlink Data Streams behind it) for crypto, GOLD, SILVER, WTI, BRENT, SPY and QQQ. Hyperliquid xyz oracle only where GMX has no market: indices (SP500, XYZ100, JP225, KR200), liquid single stocks, and extra metals if wanted later. Reason: the xyz oracle is pushed by one address (the dex deployer's setOracle holder, 0x1234...acec), GMX is a decentralised network; the cadences are comparable (metals 12 to 14 changes/min on both). The two sources differ by 6 to 20 bp on the same instrument (oil 6 to 7, NATGAS 16, gold and silver about 20, BTC/ETH/SOL about 14 the other way), so a market never switches source after launch (a switch is a repricing event); the source is part of the published market rules.
**56. Not at launch.** All FX pairs (oracle median gap 9 s, freezes up to 164 s, 2 to 3 changes/min: a stale-price attack surface bigger than the spread; revisit after a week-long weekday study), NATGAS (GMX silent up to 76 s, 16 bp from HL), xyz:HO (sits about 63 bp from its own book mid all session), OURA and CAMBRICON (oracle frozen for the whole run, up to 473 bp off the book), and the thin stocks TWST and UMC. Rule for any market to go live: over at least one full weekday session median |oracle - mid| <= 6 bp, p95 <= 12 bp and no freeze longer than `max_age_secs`; the probe tools produce exactly these numbers.
**57. `max_age_secs` per market from the measured longest freeze, not a class default**: longest freeze seen in the study plus 25 %, rounded up to 5 s, never below 15 s (crypto on GMX: gaps max 3.4 s, so 15 s). The staleness test uses the last price CHANGE (DESIGN 3.5), so a frozen oracle makes the market refuse orders instead of trading a stale price. The numbers are computed by the launch-config step from the raw CSV of the probes (git-ignored `data/` on the Mac) and re-checked after the in-window Monday run.

Open from this probe: (a) the steady offset between GMX and HL (BTC, ETH, SOL: GMX 14 bp below; gold, silver, NATGAS: GMX 16 to 20 bp above) is unexplained; a third independent reference is needed to see which side is off; (b) the open hour (14:30 to 15:30 UTC) and a weekday close are not covered; (c) the GMX blob is still unsigned (null signer), so source 3 offers no authenticity yet.
