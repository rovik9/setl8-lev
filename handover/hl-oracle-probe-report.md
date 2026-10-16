# Hyperliquid oracle probe report (prompt P11, part 1)

Measured with `tools/hl-probe --oracle` (`cd tools/hl-probe && cargo test && cargo run --release -- --oracle`) against the public Hyperliquid API on **Friday 2026-10-16, 18:36:49 to 18:52:22 UTC** (900 s of collection; US cash session open until 20:00 UTC). Read-only: WebSocket `activeAssetCtx` (3 connections, 12 coins each) and `POST /info` (`perpDexs`, `allPerpMetas`, `metaAndAssetCtxs` dex `xyz` every 10 s), plus one GMX `/prices/tickers` GET per minute. No `/exchange`, no keys, `User-Agent: setl8-probe/0.1`. Raw data: `tools/hl-probe/data/weekday-2026-10-16/` (git-ignored; `oracle-messages.csv` has every message). Recompute the tables from that CSV with `hl-probe --oracle-analyze data/weekday-2026-10-16`.

**Deviation from the prompt:** the run started at 18:36 UTC, not between 14:30 and 15:30 UTC. Rovik said to run it anyway. The US stock, index, metal and energy markets were still inside their regular session for the whole run (the cash close is 20:00 UTC today), but this is the late session, not the open: the open hour's volatility is not in these numbers.

Class tables use the 28 working coins of the Sunday report plus BTC, ETH and SOL (31 coins). The five thinnest active `xyz` stocks (xyz:OURA, xyz:CAMBRICON, xyz:TWST, xyz:GLW, xyz:UMC) were subscribed too but appear only in the per-coin table and in section 5, so that two frozen coins do not distort the stock row.

## Findings that matter for the design (read these first)
1. **No server timestamp.** Neither the WebSocket `activeAssetCtx` message (`{channel, data:{coin, ctx}}`) nor the REST `metaAndAssetCtxs` rows carry any time field next to `oraclePx`: the ctx has exactly `dayBaseVlm, dayNtlVlm, funding, impactPxs, markPx, midPx, openInterest, oraclePx, premium, prevDayPx`. The only time we can attach to an oracle price is the evaluator's receive time. A proof built from this source has no verifiable time of its own.
2. **The oracle is pushed about every 3 seconds, not on a market event.** For 23 of the 31 core coins the median gap between oracle changes is 3.2 s or less (14 to 17.5 changes per minute for most stocks, indices, BTC/ETH/SOL, gold, silver, platinum). The exceptions are BRENTOIL, CL, COPPER, NATGAS, PALLADIUM and the three FX coins (5 to 11 s). The cadence is independent of the book: xyz:AAPL had 242 oracle changes and 549 mid changes, xyz:HO 161 oracle changes and 12 mid changes.
3. **Slow oracles exist on the same dex.** FX is updated far less often: EUR 2.0 changes per minute with gaps up to 164 s, GBP 2.9 (max gap 141 s), JPY 3.4 (63 s). Energy is slower than stocks (CL 9.9, BRENTOIL 7.4, NATGAS 4.7 per minute; NATGAS gap up to 37 s). A staleness bound of 60 s would have been exceeded by the oracle of EUR, GBP and JPY (164, 141 and 63 s) while the book kept moving.
4. **Two thin stocks have a frozen oracle.** xyz:OURA and xyz:CAMBRICON had NO oracle change in 900 s while their book mids kept moving (8 and 85 mid changes). CAMBRICON's book mid drifted 297 bp (median) and up to 473 bp away from the frozen oracle; `markPx` followed the book only partly (|oracle - mark| up to 260 bp). A market that follows `oraclePx` for these coins would hold a price that is not moving at all. (Inference, not measured: CAMBRICON is listed on a Chinese exchange that is closed at this hour, OURA is a pre-IPO name; the API does not say.)
5. **One liquid-looking coin has a permanent oracle/book offset:** xyz:HO (heating oil) sits 63 bp (median, max 85 bp) away from its own book mid for the whole run; the oracle moves 10.8 times a minute but the book (12 mid changes) did not follow. Every other core coin is within 2 to 8 bp of its book mid (median).
6. **Oracle versus GMX during the US session:** oil 6 to 7 bp, SPY and QQQ ratios stay within 2 to 3 bp of their own median, but gold and silver differ by a steady 19.6 and 20.0 bp (GMX above), natural gas by 16 bp, and BTC, ETH and SOL by a steady -14.4 bp (GMX below). A constant offset that is the same for three different crypto coins is not noise; the reason is not visible from the data.
7. **WebSocket and REST agree.** The REST `oraclePx` equals the latest WebSocket value in 89 to 97 % of polls (the rest are one tick apart, p95 under 0.8 bp, max 14 bp on a stock). They are the same source; REST every 10 s is a usable fallback but 3 s coarser than the push.

## Who sets the oracle
`perpDexs` for dex `xyz` (full record in `raw/perpDexs.json`): deployer `0x88806a71d74ad0a510b350545c9ae490912f0888`, `oracleUpdater` **null**, fee recipient `0x83ffcfb1f2ad843c474b2e28df86c721cb869d3a`. The `subDeployers` list gives the right `setOracle` to exactly one address: `0x1234567890545d1df9ee64b35fdd16966e08acec`. So the oracle prices on xyz are written by that sub-deployer key (the deployer's operator), a single party that is not a decentralised feed; the API does not say where its prices come from. The same address is not the deployer. `registerAsset`, `haltTrading` and most other rights sit with three other addresses (`0x7d16...7657`, `0x8c41...0ff2`, `0xc089...059d`).

Does the oracle keep moving while the traditional market is open? Yes: all 31 core coins changed their oracle in the 15 minutes, none froze for longer than 164 s (EUR). Does it freeze at any time? Yes for single coins as above (OURA, CAMBRICON: the whole window). Whether the whole venue freezes when the underlying market closes is the Sunday question, answered in `handover/hl-probe-report.md` section 6 (coins kept moving at the weekend).

## 1. Source and message format (this run)

Collection: 2026-10-16 18:36:49 UTC (Friday) to 2026-10-16 18:52:22 UTC (Friday) (900 s planned).

Field sets seen (count of messages/polls):

- `REST metaAndAssetCtxs[1][i] keys [dayBaseVlm,dayNtlVlm,funding,impactPxs,markPx,midPx,openInterest,oraclePx,premium,prevDayPx]; top-level array length 2`: 90
- `WS message keys [channel,data]; data keys [coin,ctx]; ctx keys [dayBaseVlm,dayNtlVlm,funding,impactPxs,markPx,midPx,openInterest,oraclePx,premium,prevDayPx]`: 31539

**Server timestamp for `oraclePx` in these messages: NO.** (Looked for any key containing `time`/`timestamp` in the WebSocket envelope, `data` and `ctx` objects and in the REST context rows.)

| connection | connects | reconnects | messages | pongs | incidents |
|---|---|---|---|---|---|
| ws 1 | 1 | 0 | 10554 | 29 |  |
| ws 2 | 1 | 0 | 10554 | 29 |  |
| ws 3 | 1 | 0 | 10554 | 29 |  |

REST: 90 polls of `metaAndAssetCtxs` (dex xyz), 0 errors; REST weight total 1860, max in a trailing minute 180; HTTP statuses {200: 93}. GMX: 15 fetches, incidents [].


REST `oraclePx` against the latest WebSocket `oraclePx` at the poll's receive time (xyz coins only):

| class | REST rows | same as the latest WS value | WS had it only within the next 2 s | |REST - WS| bp median | p95 | max |
|---|---|---|---|---|---|---|
| FX | 270 | 261 (96.7%) | 6 | 0.00 | 0.00 | 0.89 |
| energy | 360 | 335 (93.1%) | 21 | 0.00 | 0.63 | 2.96 |
| index | 360 | 324 (90.0%) | 32 | 0.00 | 0.58 | 3.50 |
| metal | 450 | 404 (89.8%) | 41 | 0.00 | 0.48 | 3.08 |
| stock | 1530 | 1360 (88.9%) | 153 | 0.00 | 0.75 | 14.46 |

## 2. Oracle cadence (WebSocket `activeAssetCtx`; a change = `oraclePx` differs from the previous message of the coin)

### Per class

| class | coins | oracle changes / min (median coin) | min coin | max coin | gap median s (pooled) | gap p95 s | gap max s | longest freeze while the book mid moved |
|---|---|---|---|---|---|---|---|---|
| stock | 12 | 15.96 | 14.02 | 17.42 | 3.1 | 6.7 | 24.3 | 24.3 s on xyz:COIN (15 mid changes) |
| index | 4 | 13.59 | 10.68 | 17.22 | 3.1 | 9.3 | 29.9 | 29.9 s on xyz:JP225 (15 mid changes) |
| FX | 3 | 2.94 | 2.00 | 3.40 | 9.3 | 100.1 | 164.4 | 164.4 s on xyz:EUR (18 mid changes) |
| metal | 5 | 14.36 | 8.15 | 17.49 | 3.1 | 12.2 | 44.7 | 44.7 s on xyz:PALLADIUM (3 mid changes) |
| energy | 4 | 8.65 | 4.74 | 10.75 | 5.3 | 20.4 | 36.8 | 36.8 s on xyz:NATGAS (8 mid changes) |
| crypto | 3 | 16.94 | 16.21 | 17.61 | 3.1 | 6.3 | 25.3 | 18.4 s on BTC (2 mid changes) |

Coins whose oracle never changed in the window: xyz:CAMBRICON, xyz:OURA.

### Per coin

| coin | class | messages | oracle changes | changes / min | gap median s | gap p95 s | gap max s | mid changes | longest oracle freeze while the mid moved |
|---|---|---|---|---|---|---|---|---|---|
| BTC | crypto | 877 | 243 | 16.21 | 3.1 | 6.7 | 25.3 | 325 | 18.4 s (2 mid changes) |
| ETH | crypto | 877 | 254 | 16.94 | 3.1 | 6.3 | 15.5 | 405 | 15.5 s (1 mid changes) |
| SOL | crypto | 877 | 264 | 17.61 | 3.1 | 6.1 | 9.1 | 311 | 8.1 s (2 mid changes) |
| xyz:AAPL | stock | 876 | 242 | 16.15 | 3.1 | 6.4 | 15.3 | 549 | 15.3 s (9 mid changes) |
| xyz:AMD | stock | 876 | 261 | 17.42 | 3.1 | 6.2 | 15.2 | 789 | 15.2 s (11 mid changes) |
| xyz:AMZN | stock | 876 | 210 | 14.02 | 3.2 | 9.1 | 18.5 | 453 | 18.5 s (10 mid changes) |
| xyz:BRENTOIL | energy | 876 | 111 | 7.41 | 6.0 | 21.7 | 30.3 | 326 | 30.3 s (2 mid changes) |
| xyz:CAMBRICON (thin-stock check only) | stock | 876 | 0 | 0.00 | n/a | n/a | n/a | 85 | 898.6 s (85 mid changes) |
| xyz:CL | energy | 876 | 148 | 9.88 | 5.3 | 14.5 | 20.9 | 651 | 20.9 s (15 mid changes) |
| xyz:COIN | stock | 876 | 212 | 14.15 | 3.1 | 9.3 | 24.3 | 655 | 24.3 s (15 mid changes) |
| xyz:COPPER | metal | 876 | 122 | 8.15 | 5.9 | 15.7 | 30.8 | 223 | 30.8 s (7 mid changes) |
| xyz:EUR | FX | 876 | 30 | 2.00 | 9.2 | 131.9 | 164.4 | 53 | 164.4 s (18 mid changes) |
| xyz:GBP | FX | 876 | 44 | 2.94 | 8.8 | 110.5 | 140.7 | 15 | 123.2 s (3 mid changes) |
| xyz:GLW (thin-stock check only) | stock | 876 | 162 | 10.82 | 3.2 | 15.4 | 26.8 | 66 | 26.8 s (2 mid changes) |
| xyz:GOLD | metal | 876 | 225 | 15.02 | 3.1 | 9.4 | 12.6 | 47 | 12.6 s (1 mid changes) |
| xyz:GOOGL | stock | 876 | 231 | 15.42 | 3.1 | 9.1 | 12.3 | 596 | 12.3 s (8 mid changes) |
| xyz:HO | energy | 876 | 161 | 10.75 | 3.2 | 15.3 | 21.4 | 12 | 18.6 s (2 mid changes) |
| xyz:JP225 | index | 876 | 160 | 10.68 | 3.2 | 15.2 | 29.9 | 462 | 29.9 s (15 mid changes) |
| xyz:JPY | FX | 876 | 51 | 3.40 | 11.1 | 48.4 | 63.2 | 48 | 63.2 s (2 mid changes) |
| xyz:KR200 | index | 876 | 222 | 14.82 | 3.1 | 8.4 | 26.9 | 101 | 26.9 s (1 mid changes) |
| xyz:META | stock | 876 | 258 | 17.22 | 3.1 | 6.3 | 12.2 | 537 | 12.2 s (7 mid changes) |
| xyz:MSFT | stock | 876 | 233 | 15.55 | 3.1 | 8.2 | 12.5 | 579 | 12.5 s (8 mid changes) |
| xyz:MSTR | stock | 876 | 237 | 15.82 | 3.1 | 9.1 | 15.3 | 614 | 15.3 s (6 mid changes) |
| xyz:NATGAS | energy | 876 | 71 | 4.74 | 11.0 | 33.6 | 36.8 | 338 | 36.8 s (8 mid changes) |
| xyz:NFLX | stock | 876 | 243 | 16.22 | 3.1 | 6.7 | 15.3 | 492 | 15.3 s (3 mid changes) |
| xyz:NVDA | stock | 876 | 229 | 15.29 | 3.1 | 7.7 | 12.6 | 619 | 12.6 s (11 mid changes) |
| xyz:OURA (thin-stock check only) | stock | 876 | 0 | 0.00 | n/a | n/a | n/a | 8 | 898.6 s (8 mid changes) |
| xyz:PALLADIUM | metal | 876 | 145 | 9.68 | 3.4 | 17.5 | 44.7 | 96 | 44.7 s (3 mid changes) |
| xyz:PLATINUM | metal | 876 | 215 | 14.36 | 3.1 | 9.2 | 18.4 | 251 | 18.4 s (2 mid changes) |
| xyz:PLTR | stock | 876 | 241 | 16.09 | 3.1 | 6.3 | 12.3 | 649 | 12.3 s (6 mid changes) |
| xyz:SILVER | metal | 876 | 262 | 17.49 | 3.1 | 6.1 | 9.4 | 524 | 9.4 s (8 mid changes) |
| xyz:SP500 | index | 876 | 258 | 17.22 | 3.1 | 6.2 | 12.3 | 567 | 12.3 s (8 mid changes) |
| xyz:TSLA | stock | 876 | 257 | 17.16 | 3.1 | 6.2 | 12.5 | 674 | 12.5 s (7 mid changes) |
| xyz:TWST (thin-stock check only) | stock | 876 | 111 | 7.41 | 3.4 | 24.0 | 51.2 | 115 | 51.2 s (7 mid changes) |
| xyz:UMC (thin-stock check only) | stock | 876 | 135 | 9.01 | 3.2 | 18.5 | 54.1 | 53 | 21.6 s (4 mid changes) |
| xyz:XYZ100 | index | 876 | 185 | 12.35 | 3.2 | 9.4 | 15.9 | 489 | 15.9 s (11 mid changes) |

## 3. Size of oracle jumps (|new - old| / old, bp, per change)

| class | jumps | median bp | p95 bp | max bp |
|---|---|---|---|---|
| stock | 2854 | 0.75 | 3.81 | 49.46 |
| index | 825 | 0.65 | 2.40 | 6.41 |
| FX | 125 | 0.75 | 0.89 | 0.89 |
| metal | 969 | 0.62 | 1.92 | 4.48 |
| energy | 491 | 1.01 | 3.74 | 10.36 |
| crypto | 761 | 0.97 | 5.44 | 17.63 |

## 4. Oracle against the book mid and against `markPx` (|gap| in bp over every message)

| class | messages with a mid | |oracle-mid| median | p95 | max | |oracle-mark| median | p95 | max |
|---|---|---|---|---|---|---|---|
| stock | 10512 | 3.18 | 8.05 | 55.84 | 2.65 | 6.61 | 8.01 |
| index | 3504 | 3.07 | 8.31 | 20.08 | 2.62 | 5.82 | 7.71 |
| FX | 2628 | 4.75 | 8.63 | 9.38 | 5.06 | 6.75 | 7.50 |
| metal | 4380 | 3.42 | 5.38 | 10.31 | 2.89 | 4.38 | 5.05 |
| energy | 3504 | 6.09 | 68.80 | 84.35 | 4.14 | 6.56 | 7.10 |
| crypto | 2631 | 5.12 | 10.18 | 23.74 | 5.02 | 7.48 | 11.39 |

### Oracle lag behind the book: episodes with the mid more than 10 bp away from the oracle

An episode starts at the first message with |mid - oracle| > 10 bp and ends at the first message back inside 10 bp (an open one ends at the last message). Duration = how long the oracle was more than 10 bp off the book.

| class | episodes | total seconds off | % of observed time | episode median s | p95 s | max s |
|---|---|---|---|---|---|---|
| stock | 84 | 225 | 2.08 | 2.0 | 5.3 | 15.1 |
| index | 11 | 70 | 1.96 | 3.2 | 24.2 | 24.2 |
| FX | 0 | 0 | 0.00 | n/a | n/a | n/a |
| metal | 1 | 1 | 0.03 | 1.1 | 1.1 | 1.1 |
| energy | 11 | 935 | 26.01 | 3.4 | 898.5 | 898.5 |
| crypto | 51 | 151 | 5.60 | 2.5 | 6.2 | 9.3 |

## 5. Thin markets

| coin | 24 h notional USD | messages | oracle changes | changes / min | gap median s | gap max s | |oracle-mid| median bp | p95 | max | |oracle-mark| median bp | max | longest freeze while mid moved (mid changes) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| xyz:EUR | 4910632 | 876 | 30 | 2.00 | 9.2 | 164.4 | 3.56 | 4.45 | 4.45 | 3.56 | 4.45 | 164.4 s (18) |
| xyz:GBP | 316212 | 876 | 44 | 2.94 | 8.8 | 140.7 | 8.25 | 9.00 | 9.38 | 6.75 | 7.50 | 123.2 s (3) |
| xyz:JPY | 1033887 | 876 | 51 | 3.40 | 11.1 | 63.2 | 4.75 | 5.38 | 5.38 | 5.06 | 5.06 | 63.2 s (2) |
| xyz:JP225 | 4814460 | 876 | 160 | 10.68 | 3.2 | 29.9 | 2.04 | 3.75 | 5.71 | 2.04 | 2.97 | 29.9 s (15) |
| xyz:KR200 | 1660461 | 876 | 222 | 14.82 | 3.1 | 26.9 | 3.97 | 10.72 | 20.08 | 0.00 | 7.71 | 26.9 s (1) |
| xyz:PALLADIUM | 568393 | 876 | 145 | 9.68 | 3.4 | 44.7 | 3.14 | 6.27 | 10.31 | 1.79 | 4.49 | 44.7 s (3) |
| xyz:OURA | 5722 | 876 | 0 | 0.00 | n/a | n/a | 28.58 | 28.58 | 40.36 | 28.58 | 28.99 | 898.6 s (8) |
| xyz:CAMBRICON | 8423 | 876 | 0 | 0.00 | n/a | n/a | 296.77 | 390.61 | 472.79 | 5.88 | 260.44 | 898.6 s (85) |
| xyz:TWST | 21687 | 876 | 111 | 7.41 | 3.4 | 51.2 | 7.89 | 26.23 | 122.26 | 0.00 | 0.75 | 51.2 s (7) |
| xyz:GLW | 29559 | 876 | 162 | 10.82 | 3.2 | 26.8 | 2.63 | 6.76 | 9.41 | 1.50 | 3.77 | 26.8 s (2) |
| xyz:UMC | 38706 | 876 | 135 | 9.01 | 3.2 | 54.1 | 11.05 | 23.88 | 114.72 | 0.00 | 0.00 | 21.6 s (4) |

## 6. Hyperliquid oracle against GMX tickers (one GMX fetch per minute; HL value = latest WS `oraclePx` at the fetch's send time)

| pair | minutes compared | level | signed median bp (GMX - HL; for ratios: ratio vs its own median) | |gap| median bp | p95 | max |
|---|---|---|---|---|---|---|
| GOLD vs xyz:GOLD | 15 | GMX last 4165.1250 / HL oracle last 4157.1000 | 19.58 | 19.58 | 20.64 | 20.64 |
| SILVER vs xyz:SILVER | 15 | GMX last 57.0650 / HL oracle last 56.9410 | 20.02 | 20.02 | 21.78 | 21.78 |
| WTIOIL vs xyz:CL | 15 | GMX last 99.1950 / HL oracle last 99.1400 | 6.55 | 6.55 | 9.57 | 9.57 |
| BRENTOIL vs xyz:BRENTOIL | 15 | GMX last 106.7450 / HL oracle last 106.6800 | 6.09 | 6.09 | 7.96 | 7.96 |
| NATGAS vs xyz:NATGAS | 15 | GMX last 3.3815 / HL oracle last 3.3770 | 16.25 | 16.25 | 22.22 | 22.22 |
| SPY / xyz:SP500 (ratio) | 15 | median ratio 0.099845 | 0.00 | 0.52 | 1.83 | 1.83 |
| QQQ / xyz:XYZ100 (ratio) | 15 | median ratio 0.024374 | 0.00 | 0.79 | 2.86 | 2.86 |
| BTC vs BTC | 15 | GMX last 81042.8703 / HL oracle last 81158.0000 | -14.38 | 14.38 | 15.97 | 15.97 |
| ETH vs ETH | 15 | GMX last 2403.9825 / HL oracle last 2407.4800 | -14.41 | 14.41 | 16.56 | 16.56 |
| SOL vs SOL | 15 | GMX last 103.0030 / HL oracle last 103.1550 | -14.77 | 14.77 | 15.96 | 15.96 |


