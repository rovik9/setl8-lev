# GMX probe report (prompt P5)

Measured with `tools/gmx-probe` (`cd tools/gmx-probe && cargo test && cargo run --release`) against the public GMX Oracle API on **Saturday 2026-10-10, 18:55:59 to 19:10:18 UTC** (859 s). Client: a Mac on a home connection (round trips of 0.3 to 1 s; no cloud region). 2,686 requests in total, 3.13 per second on average, never above the 10 per second limiter; **every response was HTTP 200: no 429, no 403, no timeouts, no transport errors**. Raw data is in `tools/gmx-probe/data/` (git-ignored); sha256 of the three sample files is listed in section 4. Nothing was signed, no account or key was used.

## Findings that change the design (read these first)
1. **The signed blob is 992 bytes, DESIGN 8.2 / M2 allow 512.** Every `/signed_prices/latest` entry has a 992-byte `blob` (all 115 tokens, section 6). `MAX_SIGNED_UPDATE_BYTES = 512` in `crates/journal-types` and decision 46 therefore cannot carry a source-3 proof as written. A proof with the full blob is 61 + 992 = 1,053 bytes, which does not fit a 1,232-byte transaction next to anything else. Options (not decided here): carry only a digest of the blob plus the few fields the program checks; keep the blob in the published archive; raise the limit and use one proof per transaction. Source 2 (unsigned tickers) proofs are 61 bytes and are not affected.
2. **`/markets/state` never reports a market closed.** On a Saturday all 127 markets show `status "open"`, `isMarketClosed false`, `closingPrice null`, `latestMarketStatusTransitionTimestamp null` (also 10 minutes later). It cannot drive market-open logic.
3. **`updatedAt` is not a staleness signal for closed markets.** For GOLD, SILVER, WTIOIL, BRENTOIL, NATGAS, SPY and QQQ the price changes only every 1.5 to 4 minutes or not at all in the 10-minute window (0.0 to 0.6 changes per minute) while `updatedAt` keeps advancing about 56 times a minute. DESIGN 3.4 ("the feed goes stale, so the market reads as closed") does not hold for GMX tickers; the session windows have to decide, and a staleness check must look at the time of the last PRICE change, not at `updatedAt`.
4. **Tickers are not a consistent snapshot.** A response carries a median of 2 distinct `updatedAt` values (up to 81) across its rows, and `updatedAt` goes backwards for the same symbol in 1.1% to 3.8% of responses (27 to 30 by receive order, 92 by send order, out of 2,401). A recorder has to keep only samples whose `updatedAt` is newer than the last one it stored.
5. **Tickers have no spread to speak of.** `minPrice == maxPrice` for BTC and ETH at the sample time; SOL 0.57 bp, GOLD 0.012 bp. The engine's synthetic half-spread (DESIGN 9.1) is the only spread a trader sees.
6. **The price is in the blob.** In every one of the 115 blobs there are 18-decimal big-endian integers within 0.015% (most within 0.001%) of the endpoint's own `minPriceFull`/`maxPriceFull` midpoint at fixed positions (section 6). Whether they are the report's benchmark, bid and ask, and whether the report verifies, was NOT checked.

## Direct answers
| Question | Answer |
|---|---|
| Correct price formula | `USD = v / 10^(30 - tokenDecimals)` with `tokenDecimals` from `/tokens`; price = (minPrice + maxPrice) / 2. 13 of 13 tokens within 0.5% of the 1m candle close (worst XRP 0.0123%). The doc formula with `oracleDecimals` cannot be used: tickers have no `oracleDecimals` and signed prices carry it as null; with a hypothetical 8 it is wrong by a factor of 10^8. |
| `updatedAt` units | milliseconds (13 digits; client receive time minus `updatedAt`: p50 0.97 s). The separate `timestamp` field is seconds. The server clock leads the client by an unknown offset (client clock not synchronised), so ages here are upper bounds on skew. |
| Hosts | all three 20 of 20; fallbacks serve the same `/tokens` and the same symbols; prices and `updatedAt` can lag the primary by up to 12 s (fallback) and 32 s (fallback 2) in a back-to-back fetch. |
| Candles | 10,000 candles of 1m cover 166.7 hours (6.9 days), newest first, no missing minute, including across the weekend for GOLD and SPY. |
| Storage | see section 4: 3.6 MB per 10 minutes for all 115 tickers when only price changes are stored; about 13.5 GB per month for 100 markets (crypto-like cadence); 95 GB per month for 100 markets if every poll is stored. |

## 1. Hosts

/ping x N per host, 200 ms apart (latency in ms, full request time).

| host | ok | failed | p50 | p95 | p99 | max |
|---|---|---|---|---|---|---|
| arbitrum-api.gmxinfra.io | 20/20 | 0 | 532 | 708 | 4160 | 4160 |
| arbitrum-api-fallback.gmxinfra.io | 20/20 | 0 | 336 | 449 | 546 | 546 |
| arbitrum-api-fallback.gmxinfra2.io | 20/20 | 0 | 524 | 2006 | 2046 | 2046 |

Fallbacks vs primary, fetched back to back (not at the same instant):

| fallback | /tokens identical | same ticker symbols | max median diff (bp) | max updatedAt diff (ms) |
|---|---|---|---|---|
| arbitrum-api-fallback.gmxinfra.io | true | true | 14.23 | 12456 |
| arbitrum-api-fallback.gmxinfra2.io | true | true | 39.71 | 32158 |

## 2. Static shape (primary host)

- `/tokens`: HTTP 200, 13024 bytes, 610 ms
- `/prices/tickers`: HTTP 200, 22192 bytes, 613 ms
- `/signed_prices/latest`: HTTP 200, 308789 bytes, 1045 ms
- `/markets/state`: HTTP 200, 20968 bytes, 575 ms
- `/markets/info`: HTTP 200, 112765 bytes, 579 ms

Fields live vs the docs (documented-but-absent and present-but-undocumented):

| endpoint | entries | live fields | in docs, NOT live | live, NOT in docs |
|---|---|---|---|---|
| /prices/tickers | 115 | 6 fields: maxPrice, minPrice, timestamp, tokenAddress, tokenSymbol, updatedAt | medianPrice, oracleDecimals | timestamp |
| /signed_prices/latest | 115 | 28 fields: blob, createdAt, feedVersion, id, invalidReason, isValid, marketStatus, maxBlockHash, maxBlockNumber, maxBlockTimestamp, maxPrice, maxPriceFull, minBlockHash, minBlockNumber, minBlockTimestamp, minPrice, minPriceFull, oracleDecimals, oracleKeeperFetchType, oracleKeeperKey, oracleKeeperRecordId, oracleType, signature, signatureWithoutBlockHash, signer, stalenessSeconds, tokenAddress, tokenSymbol | medianPrice, salt, tokenOracleType | id, invalidReason, isValid, marketStatus, maxPriceFull, minPriceFull, oracleKeeperFetchType, oracleKeeperKey, oracleKeeperRecordId, stalenessSeconds |
| /markets/state | 127 | 5 fields: closingPrice, isMarketClosed, latestMarketStatusTransitionTimestamp, marketToken, status |  | isMarketClosed |
| /tokens | 130 | address, decimals, symbol, synthetic | (undocumented) |  |
| /markets/info | 127 | availableLiquidityLong, availableLiquidityShort, borrowingRateLong, borrowingRateShort, fundingRateLong, fundingRateShort, indexToken, isListed, listingDate, longToken, marketToken, name, netRateLong, netRateShort, openInterestLong, openInterestShort, poolAmountLong, poolAmountShort, shortToken | (undocumented) |  |

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
| BTC | 8 | 829464189426712150000000000/829464189426712150000000000 | 82946.418943 | 8294641894267.122070 | 82945.100000 | 0.0016% | 10000158913.9347% |
| ETH | 18 | 2509584327229797/2509584327229797 | 2509.584327 | 250958432722.979706 | 2509.550000 | 0.0014% | 10000136686.3952% |
| SOL | 9 | 110189121391252093750000/110195435788775531250000 | 110.192279 | 11019227859.001381 | 110.190000 | 0.0021% | 10000206687.3685% |
| XRP | 6 | 1402634462495451725000000/1402709698245224375000000 | 1.402672 | 140267208.037034 | 1.402500 | 0.0123% | 10001226854.5122% |
| DOGE | 8 | 859839322500000000000/859912307500000000000 | 0.085988 | 8598758.150000 | 0.085988 | 0.0006% | 9999939600.9586% |
| LINK | 18 | 13061862500000/13063347500000 | 13.062605 | 1306260500.000000 | 13.061800 | 0.0062% | 10000616200.9692% |
| GOLD | 18 | 4196832500000000/4196837500000000 | 4196.835000 | 419683500000.000000 | 4196.840000 | 0.0001% | 9999987986.2744% |
| SILVER | 18 | 60772500000000/60777500000000 | 60.775000 | 6077500000.000000 | 60.775000 | 0.0000% | 9999999900.0000% |
| WTIOIL | 18 | 90792500000000/90797500000000 | 90.795000 | 9079500000.000000 | 90.795000 | 0.0000% | 9999999900.0000% |
| BRENTOIL | 18 | 102022500000000/102027500000000 | 102.025000 | 10202500000.000000 | 102.030000 | 0.0049% | 9999509848.0545% |
| NATGAS | 18 | 3418250000000/3418750000000 | 3.418500 | 341850000.000000 | 3.418500 | 0.0000% | 9999999900.0000% |
| SPY | 18 | 779772500000000/779777500000000 | 779.775000 | 77977500000.000000 | 779.780000 | 0.0006% | 9999935779.3506% |
| QQQ | 18 | 751782500000000/751787500000000 | 751.785000 | 75178500000.000000 | 751.790000 | 0.0007% | 9999933392.0656% |

Within 0.5%: formula A 13/13 tokens, formula B 0/13.

Spread (max - min) / median at the sample time (formula A):

| symbol | min USD | max USD | median USD | spread (bp) |
|---|---|---|---|---|
| BTC | 82946.418943 | 82946.418943 | 82946.418943 | 0.000 |
| ETH | 2509.584327 | 2509.584327 | 2509.584327 | 0.000 |
| SOL | 110.189121 | 110.195436 | 110.192279 | 0.573 |
| GOLD | 4196.832500 | 4196.837500 | 4196.835000 | 0.012 |

## 4. Update cadence (/prices/tickers every 250 ms)

Duration 600 s, 2401 ticks, 0 skipped (more than 8 in flight), 2401 ok responses, 0 transport errors/parse failures, 0 non-200. Latency ms: p50 546, p95 798, p99 1566, max 3936.

`updatedAt` units: 13-digit values; client receive time minus `updatedAt` (ms): p50 Some(969), p95 Some(1803), min Some(301), max Some(4110) -> MILLISECONDS. `timestamp` (separate field) is seconds: receive time (s) minus `timestamp`: p50 Some(2), max Some(5).

Per-symbol cadence (observation window 10.00 min; gaps are between consecutive PRICE changes, in ms of server `updatedAt`):

| symbol | price changes / min | distinct updatedAt / min | gap median ms | gap p95 ms | gap max ms | out-of-order updatedAt (by send order / by receive order) | same updatedAt, different price |
|---|---|---|---|---|---|---|---|
| BTC | 55.4 | 56.9 | 1005 | 1853 | 3664 | 92 / 27 | 0 |
| ETH | 56.9 | 56.9 | 995 | 1750 | 3664 | 92 / 27 | 0 |
| SOL | 56.9 | 56.9 | 992 | 1750 | 3664 | 92 / 28 | 0 |
| GOLD | 0.2 | 56.5 | 223057 | 262850 | 262850 | 92 / 28 | 0 |
| SPY | 0.0 | 56.6 | n/a | n/a | n/a | 92 / 29 | 0 |
| WTIOIL | 0.3 | 56.5 | 216140 | 256092 | 256092 | 93 / 30 | 0 |

Server-wide `updatedAt` (first ticker row) took 589 distinct values; gap median Some(970) ms, p95 Some(1613) ms, max Some(2940) ms.

Distinct updatedAt values inside ONE response: min Some(2), median Some(2), max Some(81) (one row per ticker).

Responses where ALL tickers carry the same updatedAt: 0 of 2401.

### Closed-market symbols: do their prices still change? (same window)

| symbol | price changes / min | distinct updatedAt / min | gap median ms | gap p95 ms | gap max ms | out-of-order (send / receive order) | dup updatedAt |
|---|---|---|---|---|---|---|---|
| GOLD | 0.2 | 56.5 | 223057 | 262850 | 262850 | 92 / 28 | 0 |
| SILVER | 0.1 | 56.5 | 150718 | 150718 | 150718 | 93 / 30 | 0 |
| WTIOIL | 0.3 | 56.5 | 216140 | 256092 | 256092 | 93 / 30 | 0 |
| BRENTOIL | 0.6 | 56.8 | 89789 | 160560 | 160560 | 93 / 30 | 0 |
| NATGAS | 0.0 | 56.7 | n/a | n/a | n/a | 92 / 29 | 0 |
| QQQ | 0.2 | 56.3 | 1860 | 163188 | 163188 | 92 / 28 | 0 |
| SPY | 0.0 | 56.6 | n/a | n/a | n/a | 92 / 29 | 0 |


### Sample files (DESIGN 4.5 shape: market, timestamp, bid = minPrice, ask = maxPrice, median)

| file | bytes | rows | bytes per 10 min | per market per month (30 d) | x 100 markets per month |
|---|---|---|---|---|---|
| samples-changes.bin (64-byte records, ALL tickers, only when (updatedAt, min, max) changes) | 4193216 | 65519 | 4193216 | 157.52 MB | 15.75 GB |
| samples-price-changes.bin (64-byte records, ALL tickers, only when (minPrice, maxPrice) changes) | 3598976 | 56234 | 3598976 | 135.20 MB | 13.52 GB |
| samples-tracked-all-polls.csv (6 tracked symbols, EVERY poll) | 1320611 | 14406 | 1320611 | 950.84 MB | 95.08 GB |

sha256 of both files is in data/SHA256SUMS:

```
57dae14f43c399095d24594546cdf73abcfc255d5f494a4afd2fe7be1d3cb57f  samples-changes.bin
68e6b8ce9472fe31eb731e96959f9dfa5d7d8572fe5b35560b7f2dd95b90fdb5  samples-price-changes.bin
87372b87db2e067f29447364d0328346584dcf51692af2e427477274e32939f5  samples-tracked-all-polls.csv
```

## 5. Closed markets

Run wall-clock: start 2026-10-10 18:55:59 UTC (Saturday) / this phase 2026-10-10 19:07:12 UTC (Saturday).

/markets/state at 2026-10-10 18:56:53 UTC (Saturday): 127 markets, status counts {"open / closed=false": 127}

/markets/state at 2026-10-10 19:07:12 UTC (Saturday): 127 markets, status counts {"open / closed=false": 127}

| UTC time | symbol | market name | status | isMarketClosed | closingPrice (raw) | closingPrice USD (formula A) | ticker median USD (last poll) | latestMarketStatusTransitionTimestamp |
|---|---|---|---|---|---|---|---|---|
| 18:56:53 | GOLD | GOLD/USD [ETH-USDC] | "open" | false | null | n/a | 4196.8050 | null |
| 18:56:53 | SILVER | SILVER/USD [ETH-USDC] | "open" | false | null | n/a | 60.7650 | null |
| 18:56:53 | WTIOIL | WTIOIL/USD [ETH-USDC] | "open" | false | null | n/a | 90.7850 | null |
| 18:56:53 | BRENTOIL | BRENTOIL/USD [ETH-USDC] | "open" | false | null | n/a | 102.0150 | null |
| 18:56:53 | NATGAS | NATGAS/USD [ETH-USDC] | "open" | false | null | n/a | 3.4185 | null |
| 18:56:53 | SPY | SPY/USD [ETH-USDC] | "open" | false | null | n/a | 779.7750 | null |
| 18:56:53 | QQQ | QQQ/USD [ETH-USDC] | "open" | false | null | n/a | 751.7350 | null |
| 19:07:12 | GOLD | GOLD/USD [ETH-USDC] | "open" | false | null | n/a | 4196.8050 | null |
| 19:07:12 | SILVER | SILVER/USD [ETH-USDC] | "open" | false | null | n/a | 60.7650 | null |
| 19:07:12 | WTIOIL | WTIOIL/USD [ETH-USDC] | "open" | false | null | n/a | 90.7850 | null |
| 19:07:12 | BRENTOIL | BRENTOIL/USD [ETH-USDC] | "open" | false | null | n/a | 102.0150 | null |
| 19:07:12 | NATGAS | NATGAS/USD [ETH-USDC] | "open" | false | null | n/a | 3.4185 | null |
| 19:07:12 | SPY | SPY/USD [ETH-USDC] | "open" | false | null | n/a | 779.7750 | null |
| 19:07:12 | QQQ | QQQ/USD [ETH-USDC] | "open" | false | null | n/a | 751.7350 | null |

## 6. Signed prices (/signed_prices/latest, once per second)

Duration 180 s, 181 ticks, 0 skipped, 181 ok, 0 errors, 0 non-200; latency ms p50 751, p95 1302, p99 2021. Tokens per response: min Some(115), max Some(115); union over the run: 115.

Age versus the client wall clock at receipt, ms (p50 / p95 / max):

| tokens | now - createdAt | now - maxBlockTimestamp | now - minBlockTimestamp | max - min block ts (s), min / max |
|---|---|---|---|---|
| ALL tokens | Some(1204) / Some(2137) / Some(2972) | Some(2297) / Some(3315) / Some(4241) | Some(2297) / Some(3315) / Some(4241) | Some(0) / Some(1) |
| BTC | Some(1204) / Some(2137) / Some(2869) | Some(2297) / Some(3315) / Some(4241) | Some(2297) / Some(3315) / Some(4241) | Some(0) / Some(1) |
| ETH | Some(1189) / Some(2137) / Some(2869) | Some(2295) / Some(3315) / Some(4241) | Some(2295) / Some(3315) / Some(4241) | Some(0) / Some(1) |
| SOL | Some(1189) / Some(2137) / Some(2869) | Some(2295) / Some(3315) / Some(4241) | Some(2295) / Some(3315) / Some(4241) | Some(0) / Some(1) |
| GOLD | Some(1193) / Some(2137) / Some(2869) | Some(2297) / Some(3315) / Some(4241) | Some(2297) / Some(3315) / Some(4241) | Some(0) / Some(1) |
| SPY | Some(1193) / Some(2137) / Some(2869) | Some(2297) / Some(3315) / Some(4241) | Some(2297) / Some(3315) / Some(4241) | Some(0) / Some(1) |

Blob length in bytes over the 115 tokens of the first response: {992: 115}

Longest byte prefix common to all those blobs: 63 bytes.

Distinct blobs per token over 181 responses: min Some(150), median Some(151), max Some(152).

First 16 bytes and a search for the price inside the blob (big-endian integers of width 8/16/24/32 at every byte offset, within 0.5% of the price in several scalings):

| symbol | blob bytes | first 16 bytes (hex) | price USD from min/maxPriceFull | price-like integers in the blob |
|---|---|---|---|---|
| SPY | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 779.7750 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.001%); usd x1e18 w16@528 (err 0.001%); usd x1e18 w24@456 (err 0.000%) |
| SOL | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 110.1501 | usd x1e18 w16@464 (err 0.001%); usd x1e18 w16@496 (err 0.005%); usd x1e18 w16@528 (err 0.005%); usd x1e18 w24@456 (err 0.001%) |
| BTC | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 82947.2507 | usd x1e12 w8@731 (err 0.094%); usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.000%); usd x1e18 w16@528 (err 0.000%) |
| NATGAS | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 3.4185 | usd x1e18 w8@472 (err 0.000%); usd x1e18 w8@504 (err 0.015%); usd x1e18 w8@536 (err 0.015%); usd x1e18 w16@464 (err 0.000%) |
| ETH | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 2510.0013 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.001%); usd x1e18 w16@528 (err 0.001%); usd x1e18 w24@456 (err 0.000%) |
| BRENTOIL | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 102.0150 | usd x1e6 w8@795 (err 0.395%); usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.005%); usd x1e18 w16@528 (err 0.005%) |
| WTIOIL | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 90.7850 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.006%); usd x1e18 w16@528 (err 0.006%); usd x1e18 w24@456 (err 0.000%) |
| XRP | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 1.4026 | usd x1e18 w8@472 (err 0.000%); usd x1e18 w8@504 (err 0.006%); usd x1e18 w8@536 (err 0.006%); usd x1e18 w16@464 (err 0.000%) |
| GOLD | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 4196.8050 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.000%); usd x1e18 w16@528 (err 0.000%); usd x1e18 w24@456 (err 0.000%) |
| DOGE | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 0.0860 | usd x1e18 w8@472 (err 0.001%); usd x1e18 w8@504 (err 0.008%); usd x1e18 w8@536 (err 0.008%); usd x1e18 w16@464 (err 0.001%) |
| SILVER | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 60.7650 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.008%); usd x1e18 w16@528 (err 0.008%); usd x1e18 w24@456 (err 0.000%) |
| LINK | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 13.0623 | usd x1e18 w8@413 (err 0.004%); usd x1e18 w8@472 (err 0.005%); usd x1e18 w8@504 (err 0.012%); usd x1e18 w8@536 (err 0.012%) |
| QQQ | 992 | 00094baebfda9b87680d8e59aa20a3e5 | 751.7350 | usd x1e18 w16@464 (err 0.000%); usd x1e18 w16@496 (err 0.001%); usd x1e18 w16@528 (err 0.001%); usd x1e18 w24@456 (err 0.000%) |

Tokens (of 115) with at least one price-like integer in the blob: 115. Most frequent hit positions: ["usd x1e18 width 16 at byte 464 x115", "usd x1e18 width 16 at byte 496 x115", "usd x1e18 width 16 at byte 528 x115", "usd x1e18 width 24 at byte 456 x115", "usd x1e18 width 24 at byte 488 x115", "usd x1e18 width 24 at byte 520 x115"]

Observation only: nothing here verifies a signature or decodes the report; `signer`, `signature` and the block fields are null/absent as listed in section 2.

## 7. Candle backfill (1m, limit 10000)

| symbol | status | candles | span (UTC) | span hours | ordering | missing minutes | gaps | largest gaps | latency |
|---|---|---|---|---|---|---|---|---|---|
| BTC | HTTP 200 | 10000 | 2026-10-03 20:31 .. 2026-10-10 19:10 | 166.7 | newest first | 0 | 0 |  | 1061 ms |
| ETH | HTTP 200 | 10000 | 2026-10-03 20:31 .. 2026-10-10 19:10 | 166.7 | newest first | 0 | 0 |  | 1138 ms |
| SOL | HTTP 200 | 10000 | 2026-10-03 20:31 .. 2026-10-10 19:10 | 166.7 | newest first | 0 | 0 |  | 896 ms |
| GOLD | HTTP 200 | 10000 | 2026-10-03 20:31 .. 2026-10-10 19:10 | 166.7 | newest first | 0 | 0 |  | 1104 ms |
| SPY | HTTP 200 | 10000 | 2026-10-03 20:31 .. 2026-10-10 19:10 | 166.7 | newest first | 0 | 0 |  | 830 ms |

## Run summary

Run ended 2026-10-10 19:10:18 UTC (Saturday) after 859 s; 2686 requests in total (3.13 per second on average, hard cap 10/s by the global limiter).
HTTP status counts: {200: 2686}
429/403 incidents: []
Transport errors/timeouts: 0 []
Aborted: None

## Closed-market candles (from the saved candle files)
Candles for GOLD and SPY continue without gaps through the weekend, but with far fewer distinct closes: GOLD has 66 distinct closes in the newest 360 candles (6 h) against 306 to 329 on a weekday; SPY 33 against 117 to 237. No candle has open = high = low = close.

## Decisions where the prompt was silent
1. Requests are sent by a worker per tick (at most 8 in flight) so the 250 ms cadence holds although the round trip is 0.5 to 1 s; a global gate enforces at least 100 ms between request starts.
2. The median of a ticker is (minPrice + maxPrice) / 2 because tickers have no `medianPrice`; "bid" = minPrice, "ask" = maxPrice in the sample files.
3. Formula B (with `oracleDecimals`) was evaluated with a hypothetical value of 8 because no endpoint supplies one.
4. Cadence gaps are measured between price changes in server `updatedAt` time, counting only samples whose `updatedAt` is newer than the last accepted one (the archive rule in finding 4); out-of-order answers are counted separately.
5. Three sample files were written: every `(updatedAt, bid, ask)` change (4.2 MB per 10 minutes for 115 tickers), price changes only (3.6 MB), and a CSV of every poll for the six tracked symbols (1.3 MB). The 100-market projection scales per-market size by 4,320 ten-minute blocks (30 days); crypto cadence applies to about 100 of the 115 tickers, so the projection suits a crypto-heavy list and is high for closed-market assets.
6. Blob search: big-endian integers of 8, 16, 24 and 32 bytes at every byte offset, within 0.5% of the price scaled by 1e6, 1e8, 1e12, 1e18 or the raw 30-decimal value. Random hits exist for small widths (for example BTC `usd x1e12 w8@731`, BRENTOIL `usd x1e6 w8@795`), so only positions that hit for all 115 tokens are reported as the price.
7. Run length 14.3 minutes (limit 25): cadence 10 min, signed prices 3 min, the rest about 1.3 min.

## Limits of this measurement
One Saturday evening, one client, one connection; weekday behaviour, the busy hours around exchange opens and the long-term availability of the API were not measured. No signature or report was verified or decoded. The licence question of decision 43 is unchanged.
