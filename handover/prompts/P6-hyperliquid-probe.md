You are building a second standalone measurement tool for setl8-lev-trading in THIS repo (branch main): `tools/hl-probe`. NOT part of the Anchor program or the engine. Read first: handover/hyperliquid-spec.md (documented API and what is UNVERIFIED), handover/gmx-spec.md and tools/gmx-probe (reuse its sample writer, gap statistics and report style; copy code, do not make the two tools depend on each other), handover/design-decisions.md decisions 43 and 44, docs/DESIGN.md sections 3.3, 3.4, 4.5, 4.8.

RULES
- Work only in tools/hl-probe (plus handover/ and .gitignore). Isolated Cargo workspace with its own empty `[workspace]`, own target dir, same pattern as tools/gmx-probe. Crates.io dependencies are fine (add a websocket client such as tungstenite or tokio-tungstenite, serde_json, sha2, ureq/reqwest). The Mac has network access.
- NEVER read, print or commit private keys, .env files or keypair JSONs. No API key, no signing, no accounts, no trading calls: ONLY the `/info` endpoint and read-only WebSocket subscriptions. Never call `/exchange`.
- Be polite: User-Agent `setl8-probe/0.1`. Stay under HALF of the documented limits: at most 300 REST weight per minute and at most 3 WebSocket connections. On any 429, 403 or WebSocket close with an error, STOP, back off 60 s and report it. Total run time at most 30 minutes.
- Do not push, tag or change git config. Commit only with explicit paths after scripts/secret-scan-staged.sh. Author env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com. Commit message ends with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Raw samples go under tools/hl-probe/data/ (git-ignored; add to .gitignore). Commit only source and handover/hl-probe-report.md.

WHAT THE TOOL DOES
1. Discovery: call `perpDexs` and `allPerpMetas`. Report every DEX name, its deployer if shown, the number of markets, and the exact coin-name format of its markets (confirm or refute `xyz:AAPL`). For the `xyz` DEX (and any other DEX with 10+ markets) list every coin with: classification guess (stock, ETF, index, FX, commodity, crypto, other), max leverage, szDecimals, and whether `isDelisted`/halted flags exist. Count per class.
2. Coverage against our needs: report presence of this list of coins: stocks AAPL MSFT NVDA TSLA AMZN GOOGL META NFLX AMD COIN MSTR PLTR; indices SP500 XYZ100 JP225 KR200 NIFTY IBOV VIX DXY; FX EUR GBP JPY KRW; metals GOLD SILVER COPPER PLATINUM PALLADIUM ALUMINIUM; energy CL BRENTOIL NATGAS HO. Show the exact coin string that works in `l2Book`/`candleSnapshot` for each.
3. Snapshot fields: `metaAndAssetCtxs` for each DEX with 10+ markets: for each of the 40 coins above store markPx, midPx, oraclePx, prevDayPx, dayNtlVlm, openInterest, impactPxs; report relative gap markPx vs oraclePx and midPx vs oraclePx in basis points (median, max per class). Report 24 h notional volume and open interest per class so we know which markets are thin.
4. Spread and depth: `l2Book` for 20 coins (AAPL NVDA TSLA SP500 XYZ100 JP225 EUR JPY GOLD SILVER COPPER CL BRENTOIL BTC ETH SOL plus 4 you choose). PROVE which levels array is bids and which asks (bid below ask). Report the top-of-book spread in basis points and the size at best bid/ask in USD.
5. Cadence via WebSocket: subscribe `allMids` with `dex:"xyz"` for 15 minutes on one connection, plus `bbo` for AAPL, NVDA, SP500, EUR, GOLD, CL on a second. For each tracked coin report: distinct price changes per minute, median/p95/max gap between changes, messages per minute, reconnects, out-of-order or stale prices. Compare the wall clock with message times.
6. Closed-market behaviour (today is a weekend: US stocks, indices, gold, oil and FX are closed on traditional venues; record the UTC wall time): during step 5 record whether xyz stock, index, FX and commodity prices keep moving, by how much per hour, and the spread in step 4. Compare GOLD, SILVER, WTI (CL) and SPY-like levels with handover/gmx-probe-report.md's values (if that report exists) or with GMX's `/prices/tickers` fetched live (same politeness rules as before: one fetch), and report the difference in basis points. State plainly that closed-market prices here are the venue's own, not a tradfi quote.
7. Candles: `candleSnapshot` 1m for AAPL, SP500, EUR, GOLD, BTC over the last 24 h and for the maximum window allowed. Report candles returned, element cap, gaps, and ordering. Report the earliest and latest timestamp.
8. Storage: write every WebSocket sample from step 5 to an append-only file with a sha256 at the end, in the DESIGN 4.5 shape (market, timestamp, bid, ask, mid); report bytes per 10 minutes and the projected monthly size for 100 markets.

TESTS
Unit tests for coin classification, the gap statistics, the bps helpers, and the sample file round trip. `cargo test` must pass in tools/hl-probe.

DEFINITION OF DONE
`cd tools/hl-probe && cargo test && cargo run --release` completes; handover/hl-probe-report.md has the tables; committed with explicit paths, not pushed.

REPORT BACK (paste this block, nothing longer):
- run time (UTC)
- DEX list, coin-name format, counts per class
- coverage table for the 40 coins with working coin strings
- mark/mid/oracle gaps per class, volume and open interest per class
- spread and depth table, bids/asks proof
- cadence numbers (updates per minute, gaps, reconnects)
- closed-market behaviour and the GMX comparison in bps
- candle counts and caps
- storage per 10 minutes and per month for 100 markets
- any 429/403/disconnects
- DECISIONS where this prompt was silent
- `git log --oneline -3` and `git status --short`
