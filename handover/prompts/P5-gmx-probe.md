You are building a small standalone measurement tool for setl8-lev-trading in THIS repo (branch main): `tools/gmx-probe`. It is NOT part of the Anchor program or the engine. Read first: handover/gmx-spec.md (the documented GMX Oracle API spec and what is UNVERIFIED), handover/oracle-sources.md, handover/design-decisions.md decision 43, docs/DESIGN.md sections 3.3, 3.4, 4.5 and 4.8.

RULES
- Work only in tools/gmx-probe (plus handover/ and .gitignore). Isolated Cargo workspace: its own Cargo.toml with an empty `[workspace]` table and its own target dir, same pattern as tests-rs. Do not add it to the root workspace. Any crates.io dependencies are fine here (ureq or reqwest, serde, serde_json, sha2); the Mac has network access.
- NEVER read, print or commit private keys, .env files or keypair JSONs. No API key is needed; do not create accounts or sign anything.
- Be a polite client: send `User-Agent: setl8-probe/0.1`. Never exceed 10 requests per second in total. If any request returns 429 or 403, STOP the load immediately, back off 60 s, and report it; do not retry harder. Total run time at most 25 minutes. Read-only GETs only.
- Do not push, tag or change git config. Commit only with explicit paths after scripts/secret-scan-staged.sh. Author env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com. Commit message ends with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Raw sample data goes under tools/gmx-probe/data/ which MUST be git-ignored. Commit only the tool source and handover/gmx-probe-report.md.

WHAT THE TOOL DOES (one binary, subcommands or one run)
1. Hosts: test primary `arbitrum-api.gmxinfra.io` and both fallbacks `arbitrum-api-fallback.gmxinfra.io`, `arbitrum-api-fallback.gmxinfra2.io`: /ping status and latency (20 requests each, 200 ms apart), whether fallbacks serve identical data.
2. Static shape: fetch /tokens, /prices/tickers, /signed_prices/latest, /markets/state, /markets/info once each and save the raw JSON. Report: token count per endpoint, the field names actually present (document any field the docs omit, e.g. `blob`, `timestamp`, tokenDecimals/decimals in /tokens), and which of {BTC, ETH, SOL, XRP, DOGE, LINK, GOLD, SILVER, WTIOIL, BRENTOIL, NATGAS, SPY, QQQ} appear in tickers, in signed_prices, and in candles.
3. Price scaling: for every ticker work out the correct USD price from minPrice/maxPrice/medianPrice using /tokens decimals. Test BOTH conversions: v / 10^(30 - tokenDecimals) and v / 10^(30 - tokenDecimals - oracleDecimals). Pick the one whose results match the latest 1m candle close from /prices/candles (relative error < 0.5%) for at least 10 tokens including BTC, ETH, SOL and GOLD. Report the formula, the table of ten checks, and the spread (max - min) / median in basis points for BTC, ETH, SOL, GOLD at the sample time.
4. Update cadence: poll /prices/tickers every 250 ms for 10 minutes (stay under the rate cap). For each of BTC, ETH, SOL, GOLD, SPY, WTIOIL record every distinct (updatedAt, price) change. Report: distinct updates per minute, median / p95 / max gap between price changes, HTTP latency p50 / p95 / p99, error count, and any duplicate or out-of-order updatedAt. State the units of updatedAt (seconds or ms) based on comparison with the wall clock.
5. Closed-market behaviour: today is a weekend, so GOLD, SILVER, WTIOIL, BRENTOIL, NATGAS, SPY and QQQ are closed on the traditional exchanges. Fetch /markets/state and report their status, closingPrice and latestMarketStatusTransitionTimestamp. Report whether their ticker price still changes while closed and how the tickers price relates to closingPrice. Record the wall-clock time of the run in UTC.
6. Signed prices: poll /signed_prices/latest once per second for 3 minutes. Report: tokens present, the age of createdAt and min/maxBlockTimestamp versus wall clock, `blob` length distribution, the first 16 bytes of the blob per token (hex), and whether the blob contains the ticker price (search the blob bytes for the price as big-endian integer at common widths and report offsets if found; if not found, say so). Do NOT claim signature verification: only describe what you saw.
7. Candle backfill: fetch /prices/candles 1m with limit 10000 for BTC, ETH, SOL, GOLD, SPY; report the actual number of candles returned, time span covered, any gaps (missing minutes) and the ordering.
8. Store every poll sample from step 4 in an append-only binary or CSV file under data/ with a sha256 of the file written at the end, in the shape DESIGN 4.5 describes (market, timestamp, bid, ask, median). Report the file size per 10 minutes so we can project storage per month for 100 markets.

TESTS
Unit tests for the price conversion function (golden vectors from your table), the gap statistics, and the CSV/binary sample writer round trip. `cargo test` must pass in tools/gmx-probe.

DEFINITION OF DONE
`cd tools/gmx-probe && cargo test && cargo run --release` completes; handover/gmx-probe-report.md written with the numbers above (tables, not prose); committed with explicit paths, not pushed.

REPORT BACK (paste this block, nothing longer):
- run time (UTC), host results (latency, fallbacks identical or not)
- field names actually present vs the docs; coverage table for the 13 symbols
- the correct price formula and the 10-token check table
- cadence numbers for BTC, ETH, SOL, GOLD, SPY, WTIOIL
- market status table and closed-market behaviour
- signed price findings (age, blob lengths, price found in blob or not)
- candle counts and gaps
- storage per 10 minutes and projected per month for 100 markets
- any 429/403/timeouts
- DECISIONS where this prompt was silent
- `git log --oneline -3` and `git status --short`
