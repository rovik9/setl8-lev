# Hyperliquid probe report (prompt P6)

Measured with `tools/hl-probe` (`cd tools/hl-probe && cargo test && cargo run --release`) against the public Hyperliquid API on **Sunday 2026-10-11, 04:49:15 to 05:05:24 UTC** (968 s; the traditional venues for US stocks, indices, FX, gold and oil are closed at this time). Client: a Mac on a home connection. Only `POST /info` and read-only WebSocket subscriptions (`allMids` with `dex:"xyz"`, `bbo`) were used: no `/exchange`, no key, no signing. REST: 59 requests, total weight 1,059, never more than 280 in any trailing minute (limit asked: 300), all HTTP 200. WebSocket: 2 connections (limit 3), one connect each, no reconnect, no error or close, 29 pongs each. **No 429, no 403, no disconnects.** Raw data is in `tools/hl-probe/data/` (git-ignored).

## Findings that change the design (read these first)
1. **Only `xyz` is a usable source.** Of the 10 builder dexes, `flx`, `vntl`, `hyna`, `km`, `abcd` and `cash` have EVERY market flagged `isDelisted` today; only `xyz` (114 active markets), `para` (30), `mkts` (5) and `io` (9) are live, plus the main dex (178 crypto-like markets). `xyz` has one deployer (`0x88806a71...0888`) and no `oracleUpdater`: the whole price source hangs on one builder (spec risk, now measured).
2. **JPY is quoted as yen per USD.** `xyz:JPY` = 158.26 (and delisted `xyz:KRW` = 1340.9); `xyz:EUR` = 1.1209 and `xyz:GBP` = 1.3235 are USD per unit. A JPY market is therefore USD/JPY: P&L comes out in yen and needs division by the current price (a self-conversion), which the engine's `qty x (exit - entry)` USD formula does not do. DESIGN 3.5 needs a decision for JPY (an inverse-pair mode, or no JPY market).
3. **Six of the coins we wanted are delisted and return empty books:** NIFTY, IBOV, VIX, DXY, KRW, ALUMINIUM (their ctx still shows frozen placeholder prices: 24,250; 175,000; 20; 97.15; 1,340.9; 3,080). The other 28 of 34 have working `xyz:` strings with 20 levels per side.
4. **The venue trades through the weekend.** Every active xyz class kept moving: median mid changes per hour 56 for stocks, 32 ETFs, 18 indices, 8 FX, 20 commodities (14.9-minute window, linear extrapolation); only 6 of 114 coins did not move at all. Net moves were small (median 2.2 bp for stocks, 0.4 bp FX) but a 43.7 bp stock move and a 49 bp ETF move occurred in 15 minutes. These prices are the venue's own, NOT a tradfi quote; there is no "market closed" state to detect (session windows in DESIGN 3.4 would be a rule we add, not something the feed tells us).
5. **`allMids` is a coarse snapshot (one message every ~5 s, always all 133 coins incl. delisted ones); `bbo` is event-driven.** For the six tracked coins the bbo price changes 0.4 to 2.9 times per minute at quiet weekend hours with bursts (median gap 0.3 to 12 s, p95 up to 692 s); allMids saw fewer changes than bbo for every tracked coin. Decisive samples should come from bbo or l2Book, not from allMids.
6. **Order-book orientation proven:** `levels[0]` = bids (descending), `levels[1]` = asks (ascending): best bid < best ask, sorted correctly in 20 of 20 books; `bbo[0]` is the bid in 1,819 of 1,819 messages.
7. **Candles:** `candleSnapshot` 1m returned 1,432 to 1,441 candles for 24 h and 5,086 to 5,212 candles (about 3.6 days, up to the present) for the maximum window, oldest first, no gap. The documented 500 cap does not apply to a single call here.
8. **Venue vs GMX (closed-market prices differ by 4 to 18 bp):** see section 6; GMX tickers were frozen at their Friday-close levels while the Hyperliquid mids kept moving.

## Direct answers
| Question | Answer |
|---|---|
| Coin-name format | `<dex>:<TICKER>` for builder dexes (`xyz:AAPL` confirmed, `xyz:XYZ100`, `xyz:CL`); bare ticker on the main dex (`BTC`, `kPEPE`). Bare `AAPL` returns `null`. |
| Counts per class on `xyz` (active, guessed) | 73 stocks, 13 ETFs, 4 indices (SP500, XYZ100, JP225, KR200), 3 FX (EUR, GBP, JPY), 9 commodities, 12 other (pre-IPO / non-US listings); 19 delisted on top. |
| Liquidity of the coins we want (24 h notional, USD) | GOLD 5.5 M, SILVER 15.4 M, CL 14.6 M, BRENTOIL 14.6 M, NATGAS 0.8 M, SP500 20.3 M, XYZ100 42.6 M, NVDA 6.6 M, AAPL 1.6 M, JPY 0.24 M, EUR 0.05 M, **GBP 8.7 k**, **KR200 15 k**, JP225 87 k, NFLX 113 k. Thin spots: GBP, KR200, JP225, EUR, PALLADIUM (69 k). |
| Spread at the weekend (top of book) | 0.1 to 1.1 bp for CL, GOLD, SILVER, SP500, XYZ100, megacaps and BTC/ETH/SOL; EUR 1.8 bp, JPY 2.5 bp, PLATINUM 5.3 bp, GBP 6.8 bp, **JP225 24 bp**. Size at the best level is small for stocks and FX ($100 to $2,800) and large for oil, gold and indices. |
| Storage | allMids changes for all xyz coins: 92 KB per 10 minutes; bbo for 6 coins: 78 KB per 10 minutes (every message). See section 8 for monthly projections (weekend activity; weekdays were not measured). |

## 1. Discovery (perpDexs + allPerpMetas)

| dex | full name | deployer | oracleUpdater | markets | delisted | active | collateralToken | first coin |
|---|---|---|---|---|---|---|---|---|
| (main) | Hyperliquid main perps | - | - | 234 | 56 | 178 | 0 | BTC |
| xyz | XYZ | 0x88806a71d74ad0a510b350545c9ae490912f0888 | - | 133 | 19 | 114 | 0 | xyz:XYZ100 |
| flx | Felix Exchange | 0x2fab552502a6d45920d5741a2f3ebf4c35536352 | 0x94757f8dcb4bf73b850195660e959d1105cfedd5 | 16 | 16 | 0 | 360 | flx:TSLA |
| vntl | Ventuals | 0x8888888192a4a0593c13532ba48449fc24c3beda | - | 15 | 15 | 0 | 360 | vntl:SPACEX |
| hyna | HyENA | 0x53e655101ea361537124ef814ad4e654b54d0637 | 0xaab93501e78f5105e265a1eafda10ce6530de17e | 25 | 25 | 0 | 235 | hyna:BTC |
| km | Markets by Kinetiq | 0x71f0019cc7fa79e4f42587fb7b9a817d8d2429ec | - | 23 | 23 | 0 | 360 | km:US500 |
| abcd | ABCDEx | 0x372c7f69d0ecc14f73f89bd8a7dfb41bdf29857c | - | 1 | 1 | 0 | 0 | abcd:USA500 |
| cash | dreamcash | 0xffa8198c62adb1e811629bd54c9b646d726deef7 | - | 17 | 17 | 0 | 268 | cash:USA500 |
| para | Paragon | 0x8888888c43cbb7e1c4132542e46831bffd866ed3 | 0x8888888c43cbb7e1c4132542e46831bffd866ed3 | 38 | 8 | 30 | 0 | para:TOTAL2 |
| mkts | Markets By Kinetiq | 0x71f0019cc7fa79e4f42587fb7b9a817d8d2429ec | - | 24 | 19 | 5 | 0 | mkts:US500 |
| io | EntropyIO | 0x320c8988e3d1b5198f335802d7bfd2728a8fcac6 | 0x94757f8dcb4bf73b850195660e959d1105cfedd5 | 12 | 3 | 9 | 0 | io:OAI |

Coin-name format: `<dex>:<TICKER>` for builder dexes (for example `xyz:AAPL`, `xyz:XYZ100`), a bare ticker on the main dex (`BTC`, `kPEPE`). A bare `AAPL` returns `null` from l2Book.

Active (not delisted) coins per class (class = a guess from the ticker, see decisions):

| dex | stock | ETF | index | FX | commodity | crypto | other | total |
|---|---|---|---|---|---|---|---|---|
| (main) | 0 | 0 | 0 | 0 | 1 | 177 | 0 | 178 |
| abcd | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| cash | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| flx | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| hyna | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| io | 6 | 1 | 0 | 0 | 0 | 0 | 2 | 9 |
| km | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| mkts | 0 | 0 | 5 | 0 | 0 | 0 | 0 | 5 |
| para | 19 | 2 | 6 | 0 | 0 | 0 | 3 | 30 |
| vntl | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| xyz | 73 | 13 | 4 | 3 | 9 | 0 | 12 | 114 |

### Every coin on `xyz` (and flags)

| coin | class (guess) | max leverage | szDecimals | isDelisted | other flags |
|---|---|---|---|---|---|
| xyz:XYZ100 | index | 30 | 4 | active | growthMode=enabled |
| xyz:TSLA | stock | 20 | 3 | active | growthMode=enabled |
| xyz:NVDA | stock | 20 | 3 | active | growthMode=enabled |
| xyz:GOLD | commodity | 25 | 4 | active |  |
| xyz:HOOD | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:INTC | stock | 10 | 2 | active | growthMode=enabled |
| xyz:PLTR | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:COIN | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:META | stock | 20 | 3 | active | growthMode=enabled |
| xyz:AAPL | stock | 20 | 3 | active | growthMode=enabled |
| xyz:MSFT | stock | 20 | 3 | active | growthMode=enabled |
| xyz:ORCL | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:GOOGL | stock | 20 | 3 | active | growthMode=enabled |
| xyz:AMZN | stock | 20 | 3 | active | growthMode=enabled |
| xyz:AMD | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:MU | stock | 10 | 3 | active | growthMode=enabled |
| xyz:SNDK | stock | 10 | 3 | active | growthMode=enabled |
| xyz:MSTR | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross |
| xyz:CRCL | stock | 10 | 3 | active | growthMode=enabled |
| xyz:NFLX | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:COST | stock | 20 | 4 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:LLY | stock | 10 | 4 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SKHX | stock | 10 | 3 | active | growthMode=enabled |
| xyz:TSM | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:JPY | FX | 50 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:EUR | FX | 50 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SILVER | commodity | 25 | 2 | active | growthMode=enabled |
| xyz:RIVN | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:BABA | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:CL | commodity | 20 | 3 | active | growthMode=enabled |
| xyz:COPPER | commodity | 20 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:NATGAS | commodity | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:URANIUM | commodity | 10 | 3 | DELISTED | onlyIsolated=true,marginMode=strictIsolated,growthMode=enabled |
| xyz:ALUMINIUM | commodity | 20 | 4 | DELISTED | onlyIsolated=true,marginMode=strictIsolated,growthMode=enabled |
| xyz:SMSN | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:PLATINUM | commodity | 20 | 4 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:USAR | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:CRWV | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:URNM | ETF | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:PALLADIUM | commodity | 20 | 4 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:DXY | index | 20 | 2 | DELISTED | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:GME | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:KR200 | index | 20 | 4 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SOFTBANK | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:JP225 | index | 20 | 5 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:HYUNDAI | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:KIOXIA | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:EWY | ETF | 20 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:EWJ | ETF | 20 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:BRENTOIL | commodity | 20 | 2 | active | growthMode=enabled |
| xyz:VIX | index | 3 | 2 | DELISTED | onlyIsolated=true,marginMode=strictIsolated,growthMode=enabled |
| xyz:HIMS | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SP500 | index | 50 | 3 | active | growthMode=enabled |
| xyz:DKNG | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:LITE | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:CORN | commodity | 20 | 0 | DELISTED | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:XLE | ETF | 20 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:WHEAT | commodity | 20 | 0 | DELISTED | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:TTF | commodity | 20 | 1 | DELISTED | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:BX | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:PURRDAT | other | 10 | 0 | active | onlyIsolated=true,marginMode=noCross |
| xyz:MRVL | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:RKLB | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:BIRD | other | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:VOL | other | 10 | 1 | DELISTED | onlyIsolated=true,marginMode=strictIsolated,growthMode=enabled |
| xyz:DRAM | stock | 20 | 1 | active | growthMode=enabled |
| xyz:CBRS | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:EWZ | ETF | 20 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:KRW | FX | 50 | 3 | DELISTED | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:ZM | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:EBAY | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:H100 | other | 10 | 0 | DELISTED | onlyIsolated=true,marginMode=strictIsolated,growthMode=enabled |
| xyz:NIFTY | index | 20 | 4 | DELISTED | onlyIsolated=true,marginMode=strictIsolated,growthMode=enabled |
| xyz:ARM | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:EWT | ETF | 20 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:GBP | FX | 50 | 0 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SPCX | other | 20 | 2 | active | growthMode=enabled |
| xyz:IBOV | index | 20 | 5 | DELISTED | onlyIsolated=true,marginMode=strictIsolated,growthMode=enabled |
| xyz:ASML | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:MINIMAX | other | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:BB | stock | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:QNT | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:DELL | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:IBM | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:AVGO | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:NOW | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:NBIS | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:WDC | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:NOK | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SMH | ETF | 20 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:BE | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:ZHIPU | other | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:QCOM | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:STRC | other | 10 | 1 | active | onlyIsolated=true,marginMode=noCross |
| xyz:BOT | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:AMAT | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:IBIDEN | stock | 10 | 2 | DELISTED | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:GIGADEV | other | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SHAZ | other | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SKHY | stock | 10 | 2 | active | growthMode=enabled |
| xyz:KSTR | other | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:CXMT | other | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:GEV | stock | 10 | 3 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:KORU | ETF | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:UNITREE | other | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:LYTE | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:NCLD | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SOXL | ETF | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:MAGS | ETF | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:IREN | stock | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:NET | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:CRWD | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:RDDT | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:AAOI | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:MRNA | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:XBI | ETF | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:SHEIN | other | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:YMTC | other | 5 | 1 | DELISTED | onlyIsolated=true,marginMode=strictIsolated |
| xyz:BMNR | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross |
| xyz:SNXX | ETF | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:CVX | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:TLT | ETF | 20 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:HO | commodity | 10 | 0 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:OURA | stock | 5 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:LRCX | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:UMC | stock | 10 | 1 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:CAMBRICON | stock | 10 | 2 | DELISTED | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:TWST | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:GLW | stock | 10 | 2 | active | onlyIsolated=true,marginMode=noCross,growthMode=enabled |
| xyz:ACN | stock | 10 | 2 | DELISTED | onlyIsolated=true,marginMode=noCross |
| xyz:INNOLIGHT | stock | 10 | 2 | DELISTED | onlyIsolated=true,marginMode=noCross |
| xyz:AMEC | stock | 10 | 1 | DELISTED | onlyIsolated=true,marginMode=noCross |
| xyz:CATL | stock | 10 | 1 | DELISTED | onlyIsolated=true,marginMode=noCross |


## 2. Coverage of the coins we need

(The prompt says 40 coins; its list has 34 entries.) `works` = l2Book with that exact string returned non-empty bid and ask sides.

| class | base | coin string used | status | l2Book works | book | all coins with this ticker (any dex) |
|---|---|---|---|---|---|---|
| stock | AAPL | `xyz:AAPL` | active | yes | 20 bid / 20 ask levels | xyz:AAPL, km:AAPL (delisted), mkts:AAPL (delisted) |
| stock | MSFT | `xyz:MSFT` | active | yes | 20 bid / 20 ask levels | xyz:MSFT, cash:MSFT (delisted) |
| stock | NVDA | `xyz:NVDA` | active | yes | 20 bid / 20 ask levels | xyz:NVDA, flx:NVDA (delisted), km:NVDA (delisted), cash:NVDA (delisted), mkts:NVDA (delisted) |
| stock | TSLA | `xyz:TSLA` | active | yes | 20 bid / 20 ask levels | xyz:TSLA, flx:TSLA (delisted), km:TSLA (delisted), cash:TSLA (delisted), mkts:TSLA (delisted) |
| stock | AMZN | `xyz:AMZN` | active | yes | 20 bid / 20 ask levels | xyz:AMZN, cash:AMZN (delisted) |
| stock | GOOGL | `xyz:GOOGL` | active | yes | 20 bid / 20 ask levels | xyz:GOOGL, km:GOOGL (delisted), cash:GOOGL (delisted), mkts:GOOGL (delisted) |
| stock | META | `xyz:META` | active | yes | 20 bid / 20 ask levels | xyz:META, cash:META (delisted) |
| stock | NFLX | `xyz:NFLX` | active | yes | 20 bid / 20 ask levels | xyz:NFLX |
| stock | AMD | `xyz:AMD` | active | yes | 20 bid / 20 ask levels | xyz:AMD |
| stock | COIN | `xyz:COIN` | active | yes | 20 bid / 20 ask levels | xyz:COIN, flx:COIN (delisted) |
| stock | MSTR | `xyz:MSTR` | active | yes | 20 bid / 20 ask levels | xyz:MSTR |
| stock | PLTR | `xyz:PLTR` | active | yes | 20 bid / 20 ask levels | xyz:PLTR, km:PLTR (delisted), mkts:PLTR (delisted) |
| index | SP500 | `xyz:SP500` | active | yes | 20 bid / 20 ask levels | xyz:SP500 |
| index | XYZ100 | `xyz:XYZ100` | active | yes | 20 bid / 20 ask levels | xyz:XYZ100 |
| index | JP225 | `xyz:JP225` | active | yes | 20 bid / 20 ask levels | xyz:JP225 |
| index | KR200 | `xyz:KR200` | active | yes | 20 bid / 20 ask levels | xyz:KR200 |
| index | NIFTY | `xyz:NIFTY` | DELISTED | NO (empty book) | levels empty | xyz:NIFTY (delisted) |
| index | IBOV | `xyz:IBOV` | DELISTED | NO (empty book) | levels empty | xyz:IBOV (delisted) |
| index | VIX | `xyz:VIX` | DELISTED | NO (empty book) | levels empty | xyz:VIX (delisted) |
| index | DXY | `xyz:DXY` | DELISTED | NO (empty book) | levels empty | xyz:DXY (delisted) |
| FX | EUR | `xyz:EUR` | active | yes | 20 bid / 20 ask levels | xyz:EUR, km:EUR (delisted), mkts:EUR (delisted) |
| FX | GBP | `xyz:GBP` | active | yes | 20 bid / 20 ask levels | xyz:GBP |
| FX | JPY | `xyz:JPY` | active | yes | 20 bid / 20 ask levels | xyz:JPY |
| FX | KRW | `xyz:KRW` | DELISTED | NO (empty book) | levels empty | xyz:KRW (delisted) |
| metal | GOLD | `xyz:GOLD` | active | yes | 20 bid / 20 ask levels | xyz:GOLD, flx:GOLD (delisted), hyna:GOLD (delisted), km:GOLD (delisted), cash:GOLD (delisted), mkts:GOLD (delisted) |
| metal | SILVER | `xyz:SILVER` | active | yes | 20 bid / 20 ask levels | xyz:SILVER, flx:SILVER (delisted), hyna:SILVER (delisted), km:SILVER (delisted), cash:SILVER (delisted), mkts:SILVER (delisted) |
| metal | COPPER | `xyz:COPPER` | active | yes | 20 bid / 20 ask levels | xyz:COPPER, flx:COPPER (delisted) |
| metal | PLATINUM | `xyz:PLATINUM` | active | yes | 20 bid / 20 ask levels | xyz:PLATINUM, flx:PLATINUM (delisted) |
| metal | PALLADIUM | `xyz:PALLADIUM` | active | yes | 20 bid / 20 ask levels | xyz:PALLADIUM, flx:PALLADIUM (delisted) |
| metal | ALUMINIUM | `xyz:ALUMINIUM` | DELISTED | NO (empty book) | levels empty | xyz:ALUMINIUM (delisted) |
| energy | CL | `xyz:CL` | active | yes | 20 bid / 20 ask levels | xyz:CL |
| energy | BRENTOIL | `xyz:BRENTOIL` | active | yes | 20 bid / 20 ask levels | xyz:BRENTOIL |
| energy | NATGAS | `xyz:NATGAS` | active | yes | 20 bid / 20 ask levels | xyz:NATGAS |
| energy | HO | `xyz:HO` | active | yes | 20 bid / 20 ask levels | xyz:HO |

### 3a. Snapshot of the wanted coins (metaAndAssetCtxs; OI in USD = openInterest x markPx; gaps in bp, signed)

| coin | base | markPx | midPx | oraclePx | prevDayPx | dayNtlVlm USD | OI USD | mark vs oracle bp | mid vs oracle bp | impactPxs |
|---|---|---|---|---|---|---|---|---|---|---|
| xyz:AAPL | AAPL | 336.0500 | 336.0450 | 336.0500 | 336.1900 | 1619558 | 33462966 | 0.00 | -0.15 | ["336.006";"336.078"] |
| xyz:MSFT | MSFT | 534.2200 | 534.2400 | 534.1200 | 534.9500 | 778512 | 22829307 | 1.87 | 2.25 | ["534.19";"534.328"] |
| xyz:NVDA | NVDA | 230.2500 | 230.2550 | 230.2300 | 230.3800 | 6563726 | 110437177 | 0.87 | 1.09 | ["230.244";"230.265"] |
| xyz:TSLA | TSLA | 382.2900 | 382.2900 | 382.2800 | 382.9000 | 1089804 | 43481652 | 0.26 | 0.26 | ["382.239";"382.332"] |
| xyz:AMZN | AMZN | 262.3200 | 262.3250 | 262.3100 | 262.5400 | 894082 | 37550817 | 0.38 | 0.57 | ["262.277";"262.369"] |
| xyz:GOOGL | GOOGL | 350.6900 | 350.6750 | 350.7300 | 352.4100 | 5333931 | 98101293 | -1.14 | -1.57 | ["350.66";"350.71"] |
| xyz:META | META | 718.8400 | 718.8600 | 718.7000 | 719.7900 | 3542742 | 53281227 | 1.95 | 2.23 | ["718.795";"718.902"] |
| xyz:NFLX | NFLX | 70.2270 | 70.2270 | 70.2320 | 70.3440 | 112825 | 6497406 | -0.71 | -0.71 | ["70.181";"70.246"] |
| xyz:AMD | AMD | 608.7800 | 609.3650 | 608.5400 | 611.2600 | 1068155 | 18634193 | 3.94 | 13.56 | ["609.243";"609.433"] |
| xyz:COIN | COIN | 180.6300 | 180.6450 | 180.5800 | 180.7700 | 651517 | 17716012 | 2.77 | 3.60 | ["180.62";"180.689"] |
| xyz:MSTR | MSTR | 155.2500 | 155.2450 | 155.1500 | 154.7200 | 1572999 | 45139542 | 6.45 | 6.12 | ["155.227";"155.269"] |
| xyz:PLTR | PLTR | 209.0100 | 209.0300 | 209.0000 | 209.4500 | 997469 | 15756471 | 0.48 | 1.44 | ["208.993";"209.065"] |
| xyz:SP500 | SP500 | 7807.3000 | 7807.5000 | 7807.3000 | 7819.6000 | 20254233 | 377012690 | 0.00 | 0.26 | ["7807.297";"7807.955"] |
| xyz:XYZ100 | XYZ100 | 30898.0000 | 30892.5000 | 30900.0000 | 30971.0000 | 42614133 | 234850371 | -0.65 | -2.43 | ["30891.99";"30893.0"] |
| xyz:JP225 | JP225 | 68909.0000 | 68873.5000 | 68909.0000 | 68860.0000 | 87389 | 2914762 | 0.00 | -5.15 | ["68767.9";"68959.5"] |
| xyz:KR200 | KR200 | 1034.5000 | 1034.5000 | 1036.0000 | 1036.2000 | 15116 | 581945 | -14.48 | -14.48 | ["1030.87";"1036.98"] |
| xyz:NIFTY | NIFTY | 24250.0000 | - | 24250.0000 | 24250.0000 | 0 | 0 | 0.00 | - | null |
| xyz:IBOV | IBOV | 175000.0000 | - | 175000.0000 | 175000.0000 | 0 | 0 | 0.00 | - | null |
| xyz:VIX | VIX | 20.0000 | - | 20.0000 | 20.0000 | 0 | 0 | 0.00 | - | null |
| xyz:DXY | DXY | 97.1500 | - | 97.1500 | 97.1500 | 0 | 0 | 0.00 | - | null |
| xyz:EUR | EUR | 1.1209 | 1.1209 | 1.1209 | 1.1208 | 50518 | 25594356 | 0.00 | 0.00 | ["1.12071";"1.12112"] |
| xyz:GBP | GBP | 1.3235 | 1.3236 | 1.3240 | 1.3237 | 8703 | 793046 | -3.78 | -3.40 | ["1.321388";"1.325988"] |
| xyz:JPY | JPY | 158.2700 | 158.2600 | 158.2700 | 158.2500 | 238066 | 28428036 | 0.00 | -0.63 | ["158.2302";"158.298"] |
| xyz:KRW | KRW | 1340.9000 | - | 1340.9000 | 1340.9000 | 0 | 0 | 0.00 | - | null |
| xyz:GOLD | GOLD | 4191.6000 | 4191.5500 | 4192.3000 | 4190.0000 | 5502328 | 300192901 | -1.67 | -1.79 | ["4191.5";"4191.6"] |
| xyz:SILVER | SILVER | 60.6760 | 60.6715 | 60.6750 | 60.8390 | 15426714 | 158466299 | 0.16 | -0.58 | ["60.6695";"60.672"] |
| xyz:COPPER | COPPER | 6.7188 | 6.7185 | 6.7191 | 6.7138 | 348677 | 10678758 | -0.45 | -0.89 | ["6.716";"6.7204"] |
| xyz:PLATINUM | PLATINUM | 1687.9000 | 1687.9500 | 1687.9000 | 1690.8000 | 246760 | 3563104 | 0.00 | 0.30 | ["1687.4";"1688.92"] |
| xyz:PALLADIUM | PALLADIUM | 1153.2000 | 1152.8000 | 1153.5000 | 1150.3000 | 68668 | 3085490 | -2.60 | -6.07 | ["1152.12";"1153.74"] |
| xyz:ALUMINIUM | ALUMINIUM | 3080.0000 | - | 3080.0000 | 3080.0000 | 0 | 0 | 0.00 | - | null |
| xyz:CL | CL | 91.1450 | 91.1440 | 91.1740 | 90.7340 | 14596086 | 138493895 | -3.18 | -3.29 | ["91.144";"91.145"] |
| xyz:BRENTOIL | BRENTOIL | 102.4100 | 102.4050 | 102.4200 | 101.7200 | 14595247 | 199047509 | -0.98 | -1.46 | ["102.4";"102.41"] |
| xyz:NATGAS | NATGAS | 3.4256 | 3.4256 | 3.4279 | 3.4298 | 797305 | 20848857 | -6.71 | -6.71 | ["3.42523";"3.42616"] |
| xyz:HO | HO | 4.5906 | 4.5891 | 4.5714 | 4.5400 | 193799 | 2261936 | 42.00 | 38.72 | ["4.566499";"4.601595"] |

### 3b. Gaps, volume and open interest per class: dex `(main)` (|gap| in bp: median / max; USD)

| class | coins | |mark-oracle| median | max | |mid-oracle| median | max | 24h notional sum | 24h notional median per coin | open interest sum (USD) | coins under $100k/24h |
|---|---|---|---|---|---|---|---|---|---|
| commodity | 1 | 0.72 | 0.72 | 0.36 | 0.36 | 104321 | 104321 | 97056 | 0 |
| crypto | 177 | 6.06 | 59.80 | 6.34 | 57.50 | 3026981421 | 486657 | 11977881690 | 32 |

### 3b. Gaps, volume and open interest per class: dex `io` (|gap| in bp: median / max; USD)

| class | coins | |mark-oracle| median | max | |mid-oracle| median | max | 24h notional sum | 24h notional median per coin | open interest sum (USD) | coins under $100k/24h |
|---|---|---|---|---|---|---|---|---|---|
| stock | 6 | 4.53 | 41.16 | 6.54 | 83.97 | 18903933 | 418283 | 15668408 | 3 |
| ETF | 1 | 2.25 | 2.25 | 1.69 | 1.69 | 229793 | 229793 | 376991 | 0 |
| other | 2 | 12.68 | 13.39 | 6.76 | 8.86 | 12263088 | 6131544 | 48384693 | 0 |

### 3b. Gaps, volume and open interest per class: dex `mkts` (|gap| in bp: median / max; USD)

| class | coins | |mark-oracle| median | max | |mid-oracle| median | max | 24h notional sum | 24h notional median per coin | open interest sum (USD) | coins under $100k/24h |
|---|---|---|---|---|---|---|---|---|---|
| index | 5 | 1.93 | 22.64 | 4.12 | 23.80 | 1107013 | 273885 | 11256100 | 2 |

### 3b. Gaps, volume and open interest per class: dex `para` (|gap| in bp: median / max; USD)

| class | coins | |mark-oracle| median | max | |mid-oracle| median | max | 24h notional sum | 24h notional median per coin | open interest sum (USD) | coins under $100k/24h |
|---|---|---|---|---|---|---|---|---|---|
| stock | 19 | 0.00 | 24.73 | 10.60 | 24.73 | 584879 | 5551 | 8831203 | 18 |
| ETF | 2 | 0.04 | 0.08 | 1.61 | 2.63 | 1654 | 827 | 94243 | 2 |
| index | 6 | 3.09 | 8.58 | 5.86 | 9.82 | 1245138 | 35400 | 5603337 | 4 |
| other | 3 | 0.00 | 13.58 | 3.73 | 15.04 | 380897 | 79371 | 3109860 | 2 |

### 3b. Gaps, volume and open interest per class: dex `xyz` (|gap| in bp: median / max; USD)

| class | coins | |mark-oracle| median | max | |mid-oracle| median | max | 24h notional sum | 24h notional median per coin | open interest sum (USD) | coins under $100k/24h |
|---|---|---|---|---|---|---|---|---|---|
| stock | 73 | 2.13 | 31.14 | 3.60 | 81.86 | 65570597 | 154728 | 1800597106 | 28 |
| ETF | 13 | 5.56 | 11.98 | 8.29 | 24.41 | 3354645 | 98240 | 46736055 | 7 |
| index | 4 | 0.32 | 14.48 | 3.79 | 14.48 | 62970871 | 10170811 | 615359769 | 2 |
| FX | 3 | 0.00 | 3.78 | 0.63 | 3.40 | 297287 | 50518 | 54815438 | 2 |
| commodity | 9 | 1.67 | 42.00 | 1.79 | 38.72 | 51775584 | 797305 | 836638749 | 1 |
| other | 12 | 0.94 | 39.97 | 6.72 | 39.97 | 5106537 | 48825 | 227738675 | 9 |

## 4. Spread and depth (l2Book; `levels[0]` vs `levels[1]`)

PROOF of orientation: `levels[0]` is the BIDS and `levels[1]` the ASKS when, for every book, best level[0] price < best level[1] price, level[0] prices strictly descend and level[1] prices strictly ascend (columns: best bid < best ask / bids descending / asks ascending).

| coin | best bid (levels[0][0]) | best ask (levels[1][0]) | spread bp | size at best bid USD | size at best ask USD | levels bid/ask | proof: bid<ask / bids desc / asks asc | book `time` vs receive time (ms) |
|---|---|---|---|---|---|---|---|---|
| xyz:AAPL | 336.0300 | 336.0600 | 0.89 | 411 | 1908 | 20/20 | true / true / true | 602 ms |
| xyz:NVDA | 230.2500 | 230.2600 | 0.43 | 2760 | 2553 | 20/20 | true / true / true | 550 ms |
| xyz:TSLA | 382.2700 | 382.3100 | 1.05 | 1107 | 1701 | 20/20 | true / true / true | 281 ms |
| xyz:SP500 | 7807.3000 | 7807.7000 | 0.51 | 5832 | 3248 | 20/20 | true / true / true | 471 ms |
| xyz:XYZ100 | 30892.0000 | 30893.0000 | 0.32 | 5965 | 24399 | 20/20 | true / true / true | 688 ms |
| xyz:JP225 | 68790.0000 | 68957.0000 | 24.25 | 120 | 121 | 20/20 | true / true / true | 366 ms |
| xyz:EUR | 1.1208 | 1.1210 | 1.78 | 2099 | 134 | 20/20 | true / true / true | 253 ms |
| xyz:JPY | 158.2400 | 158.2800 | 2.53 | 130 | 579 | 20/20 | true / true / true | 702 ms |
| xyz:GOLD | 4191.5000 | 4191.6000 | 0.24 | 9969 | 23669 | 20/20 | true / true / true | 644 ms |
| xyz:SILVER | 60.6630 | 60.6640 | 0.16 | 1222 | 2690 | 20/20 | true / true / true | 332 ms |
| xyz:COPPER | 6.7180 | 6.7190 | 1.49 | 159 | 1105 | 20/20 | true / true / true | 550 ms |
| xyz:CL | 91.1710 | 91.1720 | 0.11 | 20663 | 54745 | 20/20 | true / true / true | 453 ms |
| xyz:BRENTOIL | 102.4300 | 102.4400 | 0.98 | 14448 | 67341 | 20/20 | true / true / true | 286 ms |
| BTC | 82954.0000 | 82955.0000 | 0.12 | 450130 | 404881 | 20/20 | true / true / true | 472 ms |
| ETH | 2505.7000 | 2505.8000 | 0.40 | 866269 | 4654 | 20/20 | true / true / true | 689 ms |
| SOL | 109.5800 | 109.5900 | 0.91 | 204839 | 23205 | 20/20 | true / true / true | 411 ms |
| xyz:MSFT | 534.2300 | 534.2500 | 0.37 | 1897 | 646 | 20/20 | true / true / true | 278 ms |
| xyz:GBP | 1.3231 | 1.3240 | 6.80 | 310 | 102 | 20/20 | true / true / true | 486 ms |
| xyz:NATGAS | 3.4255 | 3.4257 | 0.58 | 2742 | 1469 | 20/20 | true / true / true | 521 ms |
| xyz:PLATINUM | 1687.5000 | 1688.4000 | 5.33 | 4001 | 50 | 20/20 | true / true / true | 198 ms |

Books that pass the whole proof: 20 of 20.

## 5. WebSocket cadence

Connection 1 `allMids` (dex xyz): 179 messages in 14.94 min (12.0 per minute), connects 1, reconnects 0, pongs 29. Connection 2 `bbo` x6: 1819 messages, connects 1, reconnects 0, pongs 29.
Incidents (errors/closes): conn 1 [], conn 2 []

Coins per allMids message: min Some(133), median Some(133), max Some(133).

Per tracked coin (gaps in ms between price CHANGES: median / p95 / max; allMids gaps use client receive time because allMids carries no server time; bbo gaps use the message `time`):

| coin | allMids mid changes/min | allMids gap ms | bbo msgs/min | bbo price changes/min | bbo gap ms | bbo out-of-order `time` | bbo recv - time ms (p50 / p95 / max) |
|---|---|---|---|---|---|---|---|
| xyz:AAPL | 0.94 | 15050 / 251879 / 251879 | 3.9 | 1.94 | 863 / 157839 / 250689 | 0 | Some(186) / Some(299) / Some(428) |
| xyz:NVDA | 0.54 | 10085 / 690150 / 690150 | 15.7 | 1.07 | 1091 / 692778 / 692778 | 0 | Some(179) / Some(297) / Some(487) |
| xyz:SP500 | 0.47 | 30135 / 347519 / 347519 | 44.8 | 0.80 | 12270 / 348509 / 348509 | 0 | Some(184) / Some(294) / Some(513) |
| xyz:EUR | 0.07 | 861172 / 861172 / 861172 | 0.9 | 0.40 | 340 / 267227 / 267227 | 0 | Some(211) / Some(1441) / Some(1441) |
| xyz:GOLD | 0.20 | 15165 / 468415 / 468415 | 10.4 | 0.47 | 376 / 461671 / 461671 | 0 | Some(217) / Some(343) / Some(557) |
| xyz:CL | 0.80 | 20196 / 342697 / 342697 | 46.0 | 2.88 | 319 / 119240 / 340352 | 0 | Some(186) / Some(328) / Some(1441) |

The wall-clock comparison exists only for bbo (`time` is a server millisecond timestamp); a negative or large lag would show a skewed client clock. allMids messages have no time field.

bbo messages older than 5 s on arrival: 0 of 1819.

bbo orientation: `bbo[0]` price below `bbo[1]` price in 1819 of 1819 messages (0 had a missing side) -> bbo[0] is the bid.

## 6. Closed-market behaviour

Run wall-clock: 2026-10-11 04:49:15 UTC (Sunday) to 2026-10-11 05:05:24 UTC (Sunday). US stocks, indices, FX, gold and oil are closed on their traditional venues at this time (weekend); the prices below are the HIP-3 venue's OWN mids, not a tradfi quote.

Per class on `xyz` over the 14.9 min allMids window (movement per hour = observed activity x 60 / window, linear extrapolation):

| class | active coins seen | frozen (<=1 change) | mid changes/hour median | max | net move bp (|last-first|) median | max | range bp median | sum of |moves| bp/hour median |
|---|---|---|---|---|---|---|---|---|
| stock | 73 | 4 | 56.2 | 381.4 | 2.20 | 43.67 | 3.94 | 50.2 |
| ETF | 13 | 0 | 32.1 | 220.8 | 3.23 | 48.95 | 6.40 | 43.8 |
| index | 4 | 0 | 18.1 | 28.1 | 0.58 | 1.29 | 1.62 | 9.2 |
| FX | 3 | 0 | 8.0 | 20.1 | 0.38 | 0.45 | 0.45 | 2.5 |
| commodity | 9 | 1 | 20.1 | 48.2 | 0.73 | 3.73 | 0.73 | 4.1 |
| other | 12 | 1 | 80.3 | 265.0 | 0.68 | 12.51 | 2.93 | 48.5 |

Tracked coins:

| coin | mid changes in window | net move bp | range bp | sum |moves| bp/hour |
|---|---|---|---|---|
| xyz:AAPL | 14 | 0.45 | 1.64 | 16.1 |
| xyz:NVDA | 8 | -0.65 | 0.65 | 9.6 |
| xyz:SP500 | 7 | -0.19 | 0.70 | 5.4 |
| xyz:EUR | 1 | 0.45 | 0.45 | 1.8 |
| xyz:GOLD | 3 | 0.00 | 0.48 | 3.8 |
| xyz:CL | 12 | 3.73 | 5.59 | 36.1 |

### GMX comparison (one live fetch of GMX /prices/tickers at 2026-10-11 04:52:23 UTC (Sunday); GMX median USD = (minPrice + maxPrice) / 2 / 10^12 for 18-decimal tokens; HL value = nearest allMids mid)

| GMX symbol | HL coin | GMX USD | HL mid USD | difference (HL - GMX) |
|---|---|---|---|---|
| GOLD | xyz:GOLD | 4196.5250 | 4191.5500 | -11.9 bp |
| SILVER | xyz:SILVER | 60.7650 | 60.6645 | -16.5 bp |
| WTIOIL | xyz:CL | 91.1950 | 91.1580 | -4.1 bp |
| BRENTOIL | xyz:BRENTOIL | 102.4850 | 102.4250 | -5.9 bp |
| NATGAS | xyz:NATGAS | 3.4195 | 3.4255 | 17.7 bp |
| SPY | xyz:SP500 | 779.4950 | 7807.6000 | ratio HL/GMX 10.016 |
| QQQ | xyz:XYZ100 | 751.1950 | 30892.5000 | ratio HL/GMX 41.124 |

SPY and QQQ are ETFs while `xyz:SP500` and `xyz:XYZ100` are index levels, so only the ratio is shown (it need not be a constant).

## 7b. FX quote direction

| coin | level seen | direction | status |
|---|---|---|---|
| xyz:EUR | 1.12095 | USD per 1 foreign unit (base = foreign currency) | active |
| xyz:GBP | 1.3235 | USD per 1 foreign unit (base = foreign currency) | active |
| xyz:JPY | 158.26 | foreign units per 1 USD | active |
| xyz:KRW | 1340.9 | foreign units per 1 USD | delisted |

## 7. Candles (candleSnapshot, 1m)

| coin | request | candles returned | earliest (UTC) | latest (UTC) | ordering | missing minutes | gaps | largest gaps (missing) | latency |
|---|---|---|---|---|---|---|---|---|---|
| xyz:AAPL | last 24 h | 1436 | 2026-10-10 04:50 | 2026-10-11 04:45 | oldest first | 0 | 0 |  | 248 ms |
| xyz:AAPL | max window (asked 30 days) | 5186 | 2026-10-07 14:20 | 2026-10-11 04:45 | oldest first | 0 | 0 |  | 232 ms |
| xyz:SP500 | last 24 h | 1440 | 2026-10-10 04:50 | 2026-10-11 04:49 | oldest first | 0 | 0 |  | 230 ms |
| xyz:SP500 | max window (asked 30 days) | 5086 | 2026-10-07 16:04 | 2026-10-11 04:49 | oldest first | 0 | 0 |  | 248 ms |
| xyz:EUR | last 24 h | 1432 | 2026-10-10 04:50 | 2026-10-11 04:41 | oldest first | 0 | 0 |  | 227 ms |
| xyz:EUR | max window (asked 30 days) | 5151 | 2026-10-07 14:51 | 2026-10-11 04:41 | oldest first | 0 | 0 |  | 232 ms |
| xyz:GOLD | last 24 h | 1441 | 2026-10-10 04:51 | 2026-10-11 04:51 | oldest first | 0 | 0 |  | 230 ms |
| xyz:GOLD | max window (asked 30 days) | 5204 | 2026-10-07 14:08 | 2026-10-11 04:51 | oldest first | 0 | 0 |  | 236 ms |
| BTC | last 24 h | 1441 | 2026-10-10 04:51 | 2026-10-11 04:51 | oldest first | 0 | 0 |  | 303 ms |
| BTC | max window (asked 30 days) | 5212 | 2026-10-07 14:01 | 2026-10-11 04:52 | oldest first | 0 | 0 |  | 239 ms |

## 8. Storage (DESIGN 4.5 shape: market, timestamp, bid, ask, mid; 64-byte records, prices x1e12)

| file | records | bytes | bytes per 10 min | markets in file | per market per month | x 100 markets per month |
|---|---|---|---|---|---|---|
| ws-allmids-changes.bin (mid only; one record per mid change of any xyz coin) | 2163 | 138432 | 92288 | 133 | 3.00 MB | 0.30 GB |
| ws-bbo.bin (bid, ask, mid; every bbo message of 6 coins) | 1819 | 116416 | 77611 | 6 | 55.88 MB | 5.59 GB |

sha256 (also in data/SHA256SUMS):

```
4b6653113c3424d423a9aa1f2534257b938ac2ea300de25639ce5d859ad1069d  ws-allmids-changes.bin
94e7da90e5dd2e4e8423cdcba3509512f94db8555a3d5bfedf2349a07932c9cf  ws-bbo.bin
```

## Run summary

Run ended 2026-10-11 05:05:24 UTC (Sunday) after 968 s. REST: 59 requests, total weight 1059 (highest trailing-60 s sum 280 of the 280 budget), status counts {200: 59}, 429/403 incidents [], transport errors []. WebSocket: connections 2 (limit 3), incidents [] / [].

## Decisions where the prompt was silent
1. The prompt asks for "40 coins"; the list it gives has 34 entries (12 stocks, 8 indices, 4 FX, 6 metals, 4 energy). All 34 were tested.
2. Class labels are a GUESS from the ticker (the API has no class field): fixed lists for indices, FX, commodities, ETFs and a short "other" list (pre-IPO / non-US names); every other plain ticker on a builder dex is a stock; the main dex and `hyna` are crypto.
3. The wanted coin string is the `xyz:` one when it exists, else the first match in any dex; all matches are listed.
4. "Works" means l2Book returned non-empty bid and ask sides (a delisted coin returns two empty arrays, a wrong string returns `null`).
5. The REST limiter budgets weight (20 for meta calls, 2 for l2Book, 20 + 1 per 60 candles returned) at 280 per trailing minute; waiting for the budget stretched the REST phase, which is why the book "time vs receive" column is the lag at fetch time (200 to 700 ms) rather than a snapshot of one instant.
6. allMids messages carry no server time, so their gaps are in client receive time; bbo gaps use the message `time`. Movement per hour is the 14.9-minute window scaled linearly (weekend sample; not a market statistic).
7. For the GMX comparison one fetch of GMX `/prices/tickers` was made at 04:52:23 UTC; GMX value = (minPrice + maxPrice) / 2 / 10^12 (18-decimal tokens, per handover/gmx-probe-report.md), Hyperliquid value = the allMids mid nearest in time. SPY and QQQ are ETFs against index levels, so only ratios (10.016 and 41.124) are shown.
8. Storage files: one 64-byte record per mid change (allMids; bid and ask are 0) and one per bbo message (bid, ask, mid); the per-month projection scales the per-market size by 4,320 ten-minute blocks and by 100 markets. The allMids figure averages over all 133 xyz coins, most of them quiet on a weekend.
9. `isDelisted` coins are excluded from the class statistics but kept in the listings and CSV.

## Limits of this measurement
One Sunday morning (UTC), one client. No weekday, no busy-hour and no halt/settlement behaviour was observed; the oracle mechanism of each builder (who sets `oraclePx`, how often, what happens at a halt) is not visible through the API and was not tested. No terms of use were found or read; the licence question of decisions 43 and 44 is unchanged.
