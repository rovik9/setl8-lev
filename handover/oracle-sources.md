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
