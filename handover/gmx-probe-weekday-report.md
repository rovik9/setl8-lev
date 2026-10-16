# GMX probe weekday report (prompt P11, part 2)

Measured with `tools/gmx-probe` (`cd tools/gmx-probe && cargo test && PROBE_WEEKDAY=1 cargo run --release`) against the public GMX Oracle API on **Friday 2026-10-16, 18:37:31 to 18:51:43 UTC** (852 s; 2,681 requests, 3.15 per second on average, global limiter 10 per second, HTTP 200 for every one, no 429/403, no transport error). Run in parallel with the Hyperliquid oracle probe (18:36:49 to 18:52:22 UTC), whose single GMX GET per minute adds 15 requests. Raw data: `tools/gmx-probe/data/weekday-2026-10-16/` (git-ignored); the Saturday data in `tools/gmx-probe/data/` is untouched. Steps 1 to 6 of the probe are the same code as on Saturday (`handover/gmx-probe-report.md`); step 7 (candles) and the storage tables are skipped in this mode, as asked.

**Deviation from the prompt:** the run started at 18:37 UTC, not between 14:30 and 15:30 UTC. Rovik said to run it anyway. The US cash session, the COMEX/NYMEX pits and the CME index futures were open for the whole run (the cash close is 20:00 UTC today); the open hour itself is not covered.

## Findings (read these first)
1. **Closed-market cadence is gone on a weekday.** GOLD, SILVER, WTIOIL, BRENTOIL, NATGAS, SPY and QQQ changed price 5.4 to 21.6 times a minute between 18:37 and 18:48, against 0.0 to 0.6 times a minute on Saturday. They are still much slower than crypto (57.4 per minute, gaps up to 3.4 s): gap p95 7 to 38 s and the longest silence 12 to 76 s (NATGAS 75.9 s, SILVER 34.5 s). A freshness rule of "price changed within N seconds" would need N above 76 s for NATGAS even in the middle of the US session.
2. **`/markets/state` reports every market `open` during the US session as well**: 131 of 131 `"open"`, `isMarketClosed false`, `closingPrice null`, `latestMarketStatusTransitionTimestamp null`, at 18:38:24 and again at 18:48:42. Together with Saturday (127 of 127 open) it has never reported anything else in either of two very different states of the world, so it cannot drive any market-open logic.
3. **`updatedAt` still says nothing about price freshness.** It advanced 57.0 to 57.4 times a minute for each of the ten symbols in the table below (including NATGAS, whose price changed 5.4 times a minute): same as Saturday (56.3 to 56.9).
4. **Four markets were added since Saturday** (127 to 131 in `/markets/info` and `/markets/state`): HYPE/USD, NEAR/USD, XRP/USD and ZEC/USD, all `[USDG-USDG]` pools. `/prices/tickers` and `/signed_prices/latest` still carry the same 115 tokens.
5. **Nothing else changed:** the signed blob is still 992 bytes for all 115 tokens with `signer`, `signature` and `minPrice` null; price-like 18-decimal integers at the same byte offsets (464, 496, 528, w24 at 456); price formula A (`v / 10^(30 - tokenDecimals)`) within 0.5% of the 1m candle close for 13 of 13 tokens (worst XRP 0.043%); fallback hosts serve identical symbols and `/tokens`, prices lag the primary by up to 13 s and 33 s.

## Cadence now (Friday, US session) against Saturday (price changes in `/prices/tickers`, 10 minutes, polled every 250 ms)

| symbol | changes / min Sat | changes / min Fri | gap median ms Sat | gap median ms Fri | gap p95 ms Fri | gap max ms Sat | gap max ms Fri |
|---|---|---|---|---|---|---|---|
| BTC | 55.4 | 57.4 | 1005 | 974 | 1729 | 3664 | 3396 |
| ETH | 56.9 | 57.3 | 995 | 979 | 1748 | 3664 | 3396 |
| SOL | 56.9 | 57.4 | 992 | 979 | 1729 | 3664 | 3396 |
| GOLD | 0.2 | 12.0 | 223057 | 3302 | 16091 | 262850 | 25006 |
| SILVER | 0.1 | 7.5 | 150718 | 5926 | 21086 | 150718 | 34492 |
| WTIOIL | 0.3 | 13.3 | 216140 | 3172 | 12886 | 256092 | 20946 |
| BRENTOIL | 0.6 | 13.7 | 89789 | 2875 | 13967 | 160560 | 31863 |
| NATGAS | 0.0 | 5.4 | n/a | 5845 | 38211 | n/a | 75928 |
| SPY | 0.0 | 17.6 | n/a | 1947 | 11435 | n/a | 23242 |
| QQQ | 0.2 | 21.6 | 1860 | 1986 | 7369 | 163188 | 12469 |

(Saturday values from `handover/gmx-probe-report.md` section 4; the Saturday gap p95 equals the maximum for most of these because there were only one to six gaps.)

## `/markets/state` values seen
`{"open / closed=false": 131}` at both snapshots (18:38:24 and 18:48:42 UTC); the seven tracked markets all `"open"`, `closingPrice null`, `latestMarketStatusTransitionTimestamp null`. No market reported `closed` or anything other than `open`. This probe cannot say what the endpoint does at an actual close (it never ran across one on a weekday).

# Raw tables of the run (steps 1 to 6, same format as the Saturday report)

Run started: 2026-10-16 18:37:31 UTC (Friday)
Mode: full

## 1. Hosts

/ping x N per host, 200 ms apart (latency in ms, full request time).

| host | ok | failed | p50 | p95 | p99 | max |
|---|---|---|---|---|---|---|
| arbitrum-api.gmxinfra.io | 20/20 | 0 | 526 | 1092 | 1425 | 1425 |
| arbitrum-api-fallback.gmxinfra.io | 20/20 | 0 | 324 | 432 | 1084 | 1084 |
| arbitrum-api-fallback.gmxinfra2.io | 20/20 | 0 | 513 | 1923 | 2058 | 2058 |

Fallbacks vs primary, fetched back to back (not at the same instant):

| fallback | /tokens identical | same ticker symbols | max median diff (bp) | max updatedAt diff (ms) |
|---|---|---|---|---|
| arbitrum-api-fallback.gmxinfra.io | true | true | 19.47 | 13117 |
| arbitrum-api-fallback.gmxinfra2.io | true | true | 26.22 | 33197 |

## 2. Static shape (primary host)

- `/tokens`: HTTP 200, 13024 bytes, 518 ms
- `/prices/tickers`: HTTP 200, 22188 bytes, 524 ms
- `/signed_prices/latest`: HTTP 200, 308785 bytes, 1054 ms
- `/markets/state`: HTTP 200, 21628 bytes, 534 ms
- `/markets/info`: HTTP 200, 115536 bytes, 737 ms

Fields live vs the docs (documented-but-absent and present-but-undocumented):

| endpoint | entries | live fields | in docs, NOT live | live, NOT in docs |
|---|---|---|---|---|
| /prices/tickers | 115 | 6 fields: maxPrice, minPrice, timestamp, tokenAddress, tokenSymbol, updatedAt | medianPrice, oracleDecimals | timestamp |
| /signed_prices/latest | 115 | 28 fields: blob, createdAt, feedVersion, id, invalidReason, isValid, marketStatus, maxBlockHash, maxBlockNumber, maxBlockTimestamp, maxPrice, maxPriceFull, minBlockHash, minBlockNumber, minBlockTimestamp, minPrice, minPriceFull, oracleDecimals, oracleKeeperFetchType, oracleKeeperKey, oracleKeeperRecordId, oracleType, signature, signatureWithoutBlockHash, signer, stalenessSeconds, tokenAddress, tokenSymbol | medianPrice, salt, tokenOracleType | id, invalidReason, isValid, marketStatus, maxPriceFull, minPriceFull, oracleKeeperFetchType, oracleKeeperKey, oracleKeeperRecordId, stalenessSeconds |
| /markets/state | 131 | 5 fields: closingPrice, isMarketClosed, latestMarketStatusTransitionTimestamp, marketToken, status |  | isMarketClosed |
| /tokens | 130 | address, decimals, symbol, synthetic | (undocumented) |  |
| /markets/info | 131 | availableLiquidityLong, availableLiquidityShort, borrowingRateLong, borrowingRateShort, fundingRateLong, fundingRateShort, indexToken, isListed, listingDate, longToken, marketToken, name, netRateLong, netRateShort, openInterestLong, openInterestShort, poolAmountLong, poolAmountShort, shortToken | (undocumented) |  |

Null counts in signed prices (115 entries): oracleDecimals null 115, minPrice null 115, signer null 115, signature null 115, blob null 0, minPriceFull null 0.

Coverage of the 13 symbols:

| symbol | in tickers | in signed_prices | candles (1m, limit 2) | in /tokens |
|---|---|---|---|---|
| BTC | true | true | HTTP 200 / 2 candles | true |
| ETH | true | true | HTTP 200 / 2 candles | true |
| SOL | true | true | HTTP 200 / 2 candles | true |
| XRP | true | true | HTTP 200 / 2 candles | true |
| DOGE | true | true | HTTP 200 / 2 candles | true |
| LINK | true | true | HTTP 200 / 2 candles | true |
| GOLD | true | true | HTTP 200 / 2 candles | true |
| SILVER | true | true | HTTP 200 / 2 candles | true |
| WTIOIL | true | true | HTTP 200 / 2 candles | true |
| BRENTOIL | true | true | HTTP 200 / 2 candles | true |
| NATGAS | true | true | HTTP 200 / 2 candles | true |
| SPY | true | true | HTTP 200 / 2 candles | true |
| QQQ | true | true | HTTP 200 / 2 candles | true |

## 3. Price scaling

Formula A = v / 10^(30 - tokenDecimals); formula B = v / 10^(30 - tokenDecimals - oracleDecimals). The tickers carry NO oracleDecimals (and signed prices have it null), so B is evaluated with a hypothetical oracleDecimals = 8. Median = (minPrice + maxPrice) / 2 (the live tickers have no medianPrice). Candle = close of the newest 1m candle (cached up to 15 s).

| symbol | tokenDecimals | min/max raw | A: USD | B (od=8): USD | 1m candle close | A rel err | B rel err |
|---|---|---|---|---|---|---|---|
| BTC | 8 | 813048771408465000000000000/813048771408465000000000000 | 81304.877141 | 8130487714084.650391 | 81313.760000 | 0.0109% | 9998907482.2895% |
| ETH | 18 | 2413999141744643/2413999141744643 | 2413.999142 | 241399914174.464294 | 2414.180000 | 0.0075% | 9999250750.1630% |
| SOL | 9 | 103757601156880990000000/103768105982047680000000 | 103.762854 | 10376285356.946434 | 103.790000 | 0.0262% | 9997384384.9662% |
| XRP | 6 | 1299005486941205100000000/1299100364697243900000000 | 1.299053 | 129905292.581922 | 1.299610 | 0.0429% | 9995713428.0524% |
| DOGE | 8 | 800201535208493050000/800306105625479150000 | 0.080025 | 8002538.204170 | 0.080042 | 0.0208% | 9997923745.1936% |
| LINK | 18 | 12308404014763/12309343595495 | 12.308874 | 1230887380.512900 | 12.309300 | 0.0035% | 9999653661.8947% |
| GOLD | 18 | 4167732500000000/4167737500000000 | 4167.735000 | 416773500000.000000 | 4167.260000 | 0.0114% | 10001139737.6871% |
| SILVER | 18 | 57132500000000/57137500000000 | 57.135000 | 5713500000.000000 | 57.125000 | 0.0175% | 10001750447.0460% |
| WTIOIL | 18 | 99362500000000/99367500000000 | 99.365000 | 9936500000.000000 | 99.365000 | 0.0000% | 9999999900.0000% |
| BRENTOIL | 18 | 106952500000000/106957500000000 | 106.955000 | 10695500000.000000 | 106.960000 | 0.0047% | 9999532435.5273% |
| NATGAS | 18 | 3387250000000/3387750000000 | 3.387500 | 338750000.000000 | 3.387500 | 0.0000% | 9999999900.0000% |
| SPY | 18 | 763512500000000/763517500000000 | 763.515000 | 76351500000.000000 | 763.490000 | 0.0033% | 10000327343.7124% |
| QQQ | 18 | 711572500000000/711577500000000 | 711.575000 | 71157500000.000000 | 711.570000 | 0.0007% | 10000070167.1557% |

Within 0.5%: formula A 13/13 tokens, formula B 0/13.

Spread (max - min) / median at the sample time (formula A):

| symbol | min USD | max USD | median USD | spread (bp) |
|---|---|---|---|---|
| BTC | 81304.877141 | 81304.877141 | 81304.877141 | 0.000 |
| ETH | 2413.999142 | 2413.999142 | 2413.999142 | 0.000 |
| SOL | 103.757601 | 103.768106 | 103.762854 | 1.012 |
| GOLD | 4167.732500 | 4167.737500 | 4167.735000 | 0.012 |

## 4. Update cadence (/prices/tickers every 250 ms)

Duration 600 s, 2401 ticks, 0 skipped (more than 8 in flight), 2401 ok responses, 0 transport errors/parse failures, 0 non-200. Latency ms: p50 527, p95 742, p99 1181, max 3753.

`updatedAt` units: 13-digit values; client receive time minus `updatedAt` (ms): p50 Some(821), p95 Some(1641), min Some(147), max Some(2735) -> MILLISECONDS. `timestamp` (separate field) is seconds: receive time (s) minus `timestamp`: p50 Some(2), max Some(4).

Per-symbol cadence (observation window 10.00 min; gaps are between consecutive PRICE changes, in ms of server `updatedAt`):

| symbol | price changes / min | distinct updatedAt / min | gap median ms | gap p95 ms | gap max ms | out-of-order updatedAt (by send order / by receive order) | same updatedAt, different price |
|---|---|---|---|---|---|---|---|
| BTC | 57.4 | 57.4 | 974 | 1729 | 3396 | 72 / 26 | 0 |
| ETH | 57.3 | 57.3 | 979 | 1748 | 3396 | 72 / 26 | 0 |
| SOL | 57.4 | 57.4 | 979 | 1729 | 3396 | 70 / 26 | 0 |
| GOLD | 12.0 | 57.3 | 3302 | 16091 | 25006 | 72 / 26 | 0 |
| SPY | 17.6 | 57.1 | 1947 | 11435 | 23242 | 68 / 22 | 0 |
| WTIOIL | 13.3 | 57.4 | 3172 | 12886 | 20946 | 70 / 24 | 0 |

Server-wide `updatedAt` (first ticker row) took 592 distinct values; gap median Some(959) ms, p95 Some(1586) ms, max Some(2238) ms.

Distinct updatedAt values inside ONE response: min Some(2), median Some(2), max Some(76) (one row per ticker).

Responses where ALL tickers carry the same updatedAt: 0 of 2401.

### Closed-market symbols: do their prices still change? (same window)

| symbol | price changes / min | distinct updatedAt / min | gap median ms | gap p95 ms | gap max ms | out-of-order (send / receive order) | dup updatedAt |
|---|---|---|---|---|---|---|---|
| GOLD | 12.0 | 57.3 | 3302 | 16091 | 25006 | 72 / 26 | 0 |
| SILVER | 7.5 | 57.1 | 5926 | 21086 | 34492 | 72 / 27 | 0 |
| WTIOIL | 13.3 | 57.4 | 3172 | 12886 | 20946 | 70 / 24 | 0 |
| BRENTOIL | 13.7 | 57.0 | 2875 | 13967 | 31863 | 69 / 23 | 0 |
| NATGAS | 5.4 | 57.2 | 5845 | 38211 | 75928 | 69 / 23 | 0 |
| QQQ | 21.6 | 57.0 | 1986 | 7369 | 12469 | 69 / 24 | 0 |
| SPY | 17.6 | 57.1 | 1947 | 11435 | 23242 | 68 / 22 | 0 |

### Crypto majors (BTC, ETH, SOL): distinct price changes per minute and gaps (same window)

| symbol | price changes / min | distinct updatedAt / min | gap median ms | gap p95 ms | gap max ms | out-of-order (send / receive order) | dup updatedAt |
|---|---|---|---|---|---|---|---|
| ETH | 57.3 | 57.3 | 979 | 1748 | 3396 | 72 / 26 | 0 |
| BTC | 57.4 | 57.4 | 974 | 1729 | 3396 | 72 / 26 | 0 |
| SOL | 57.4 | 57.4 | 979 | 1729 | 3396 | 70 / 26 | 0 |

## 5. Closed markets

Run wall-clock: start 2026-10-16 18:37:31 UTC (Friday) / this phase 2026-10-16 18:48:42 UTC (Friday).

/markets/state at 2026-10-16 18:38:24 UTC (Friday): 131 markets, status counts {"open / closed=false": 131}

/markets/state at 2026-10-16 18:48:42 UTC (Friday): 131 markets, status counts {"open / closed=false": 131}

| UTC time | symbol | market name | status | isMarketClosed | closingPrice (raw) | closingPrice USD (formula A) | ticker median USD (last poll) | latestMarketStatusTransitionTimestamp |
|---|---|---|---|---|---|---|---|---|
| 18:38:24 | GOLD | GOLD/USD [ETH-USDC] | "open" | false | null | n/a | 4165.0550 | null |
| 18:38:24 | SILVER | SILVER/USD [ETH-USDC] | "open" | false | null | n/a | 57.0650 | null |
| 18:38:24 | WTIOIL | WTIOIL/USD [ETH-USDC] | "open" | false | null | n/a | 99.3550 | null |
| 18:38:24 | BRENTOIL | BRENTOIL/USD [ETH-USDC] | "open" | false | null | n/a | 106.9350 | null |
| 18:38:24 | NATGAS | NATGAS/USD [ETH-USDC] | "open" | false | null | n/a | 3.3825 | null |
| 18:38:24 | SPY | SPY/USD [ETH-USDC] | "open" | false | null | n/a | 763.5250 | null |
| 18:38:24 | QQQ | QQQ/USD [ETH-USDC] | "open" | false | null | n/a | 711.1050 | null |
| 18:48:42 | GOLD | GOLD/USD [ETH-USDC] | "open" | false | null | n/a | 4165.0550 | null |
| 18:48:42 | SILVER | SILVER/USD [ETH-USDC] | "open" | false | null | n/a | 57.0650 | null |
| 18:48:42 | WTIOIL | WTIOIL/USD [ETH-USDC] | "open" | false | null | n/a | 99.3550 | null |
| 18:48:42 | BRENTOIL | BRENTOIL/USD [ETH-USDC] | "open" | false | null | n/a | 106.9350 | null |
| 18:48:42 | NATGAS | NATGAS/USD [ETH-USDC] | "open" | false | null | n/a | 3.3825 | null |
| 18:48:42 | SPY | SPY/USD [ETH-USDC] | "open" | false | null | n/a | 763.5250 | null |
| 18:48:42 | QQQ | QQQ/USD [ETH-USDC] | "open" | false | null | n/a | 711.1050 | null |

## 6. Signed prices (/signed_prices/latest, once per second)

Duration 180 s, 181 ticks, 0 skipped, 181 ok, 0 errors, 0 non-200; latency ms p50 786, p95 1224, p99 2653. Tokens per response: min Some(115), max Some(115); union over the run: 115.

Age versus the client wall clock at receipt, ms (p50 / p95 / max):

| tokens | now - createdAt | now - maxBlockTimestamp | now - minBlockTimestamp | max - min block ts (s), min / max |
|---|---|---|---|---|
| ALL tokens | Some(1201) / Some(1909) / Some(3281) | Some(2635) / Some(3084) / Some(4585) | Some(2635) / Some(3161) / Some(4585) | Some(0) / Some(1) |
| BTC | Some(1201) / Some(1909) / Some(2562) | Some(2635) / Some(3084) / Some(3666) | Some(2635) / Some(3161) / Some(3786) | Some(0) / Some(1) |
| ETH | Some(1201) / Some(1909) / Some(2562) | Some(2635) / Some(3084) / Some(3666) | Some(2635) / Some(3161) / Some(3786) | Some(0) / Some(1) |
| SOL | Some(1201) / Some(1909) / Some(2562) | Some(2635) / Some(3084) / Some(3666) | Some(2635) / Some(3161) / Some(3786) | Some(0) / Some(1) |
| GOLD | Some(1210) / Some(1903) / Some(2562) | Some(2636) / Some(3103) / Some(3666) | Some(2636) / Some(3561) / Some(3786) | Some(0) / Some(1) |
| SPY | Some(1260) / Some(1903) / Some(2562) | Some(2636) / Some(3084) / Some(3666) | Some(2636) / Some(3161) / Some(3786) | Some(0) / Some(1) |

Blob length in bytes over the 115 tokens of the first response: {992: 115}

Longest byte prefix common to all those blobs: 63 bytes.

Distinct blobs per token over 181 responses: min Some(138), median Some(140), max Some(142).

First 16 bytes and a search for the price inside the blob (big-endian integers of width 8/16/24/32 at every byte offset, within 0.5% of the price in several scalings):

| symbol | blob bytes | first 16 bytes (hex) | price USD from min/maxPriceFull | price-like integers in the blob |
|---|---|---|---|---|
| SPY | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 763.5250 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.001%); usd x1e18 w16@528 (err 0.001%); usd x1e18 w24@456 (err 0.000%) |
| SOL | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 103.1852 | usd x1e18 w16@464 (err 0.002%); usd x1e18 w16@496 (err 0.011%); usd x1e18 w16@528 (err 0.011%); usd x1e18 w24@456 (err 0.002%) |
| BTC | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 81117.2981 | usd x1e12 w8@254 (err 0.065%); usd x1e12 w16@246 (err 0.065%); usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.001%) |
| NATGAS | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 3.3825 | usd x1e18 w8@472 (err 0.000%); usd x1e18 w8@504 (err 0.015%); usd x1e18 w8@536 (err 0.015%); usd x1e18 w8@731 (err 0.102%) |
| ETH | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 2406.7434 | usd x1e18 w16@464 (err 0.001%); usd x1e18 w16@496 (err 0.005%); usd x1e18 w16@528 (err 0.005%); usd x1e18 w24@456 (err 0.001%) |
| BRENTOIL | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 106.9350 | usd x1e6 w8@795 (err 0.125%); usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.005%); usd x1e18 w16@528 (err 0.005%) |
| WTIOIL | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 99.3750 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.005%); usd x1e18 w16@528 (err 0.005%); usd x1e18 w24@456 (err 0.000%) |
| XRP | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 1.2951 | usd x1e18 w8@63 (err 0.146%); usd x1e18 w8@472 (err 0.002%); usd x1e18 w8@504 (err 0.007%); usd x1e18 w8@536 (err 0.007%) |
| GOLD | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 4165.0550 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.000%); usd x1e18 w16@528 (err 0.000%); usd x1e18 w24@456 (err 0.000%) |
| DOGE | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 0.0797 | usd x1e18 w8@472 (err 0.002%); usd x1e18 w8@504 (err 0.013%); usd x1e18 w8@536 (err 0.013%); usd x1e18 w16@464 (err 0.002%) |
| SILVER | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 57.0650 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.009%); usd x1e18 w16@528 (err 0.009%); usd x1e18 w24@456 (err 0.000%) |
| LINK | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 12.2627 | usd x1e18 w8@12 (err 0.030%); usd x1e18 w8@472 (err 0.002%); usd x1e18 w8@504 (err 0.014%); usd x1e18 w8@536 (err 0.014%) |
| QQQ | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 711.1050 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.001%); usd x1e18 w16@528 (err 0.001%); usd x1e18 w24@456 (err 0.000%) |

Tokens (of 115) with at least one price-like integer in the blob: 115. Most frequent hit positions: ["usd x1e18 width 16 at byte 464 x115", "usd x1e18 width 16 at byte 496 x115", "usd x1e18 width 16 at byte 528 x115", "usd x1e18 width 24 at byte 456 x115", "usd x1e18 width 24 at byte 488 x115", "usd x1e18 width 24 at byte 520 x115"]

Observation only: nothing here verifies a signature or decodes the report; `signer`, `signature` and the block fields are null/absent as listed in section 2.

## 7. Candle backfill

Skipped in the weekday re-run (done on Saturday).

## Run summary

Run ended 2026-10-16 18:51:43 UTC (Friday) after 852 s; 2681 requests in total (3.15 per second on average, hard cap 10/s by the global limiter).
HTTP status counts: {200: 2681}
429/403 incidents: []
Transport errors/timeouts: 0 []
Aborted: None
