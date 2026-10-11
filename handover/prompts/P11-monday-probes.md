You are extending two existing measurement tools of setl8-lev-trading in THIS repo (branch main): `tools/gmx-probe` and `tools/hl-probe`, and re-running both on a WEEKDAY during US market hours. Neither is part of the Anchor program or the engine. Read first: handover/gmx-probe-report.md, handover/hl-probe-report.md (the Saturday/Sunday runs), handover/design-decisions.md decisions 43 to 50, handover/gmx-spec.md, handover/hyperliquid-spec.md, docs/DESIGN.md 3.3, 3.4, 4.8.

RULES
- Work only in tools/gmx-probe, tools/hl-probe, handover/ and .gitignore. Same rules as P5 and P6: public read-only endpoints only, never `/exchange`, no keys, no accounts, no signing; User-Agent `setl8-probe/0.1`; stay under HALF of documented limits; on any 429 or 403 STOP, back off 60 s and report; NEVER read, print or commit private keys, .env files or keypair JSONs. Raw data stays in git-ignored data/ directories (use new subfolders `data/weekday-<UTC date>/` so the weekend data is kept).
- Do not push, tag or change git config. Commit only with explicit paths after scripts/secret-scan-staged.sh. Author env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com. Commit message ends with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Run time: each probe at most 30 minutes. Run them one after the other, or in parallel only if the combined request rate still respects both limiters. Record the UTC wall time. Today must be a weekday (Monday to Friday) and the run must START between 14:30 and 15:30 UTC (the US cash open is 13:30/14:30 UTC depending on daylight saving; US stocks, indices, and the oil/gold pits must be open). If it is not a weekday or the time is outside the window, STOP and say so.

PART 1: HYPERLIQUID ORACLE PRICE (new; decision 50 says the HL-sourced markets will follow `oraclePx`, not the order-book mid or `markPx`, because the book of a thin market can be pushed around)
Add to tools/hl-probe a mode `--oracle` that, for the 28 working coin strings of the Sunday report (stocks, indices, FX EUR GBP JPY, metals, energy, all `xyz:`) PLUS BTC, ETH, SOL (main dex, as a control), measures for 15 minutes:
1. Source of the oracle price: subscribe `activeAssetCtx` per coin over WebSocket (at most 3 connections; at most 30 coins per connection is fine, check the documented subscription limits and stay inside them; if one connection per ~10 coins is needed use up to 3) AND poll REST `metaAndAssetCtxs` for dex xyz once every 10 s (weight limit as before). Store for every message: receive time, the message's own time field if any, oraclePx, markPx, midPx if present.
2. For each coin: how often `oraclePx` changes (changes per minute, median/p95/max gap between changes), whether the message carries a SERVER TIMESTAMP for the oracle price (state yes/no plainly; if no, our proof's timestamp would be the evaluator's receive time), staleness (longest period without an oracle change while the coin's book mid kept moving), and the size of oracle jumps (bp, median and max per class).
3. Oracle vs book: |oraclePx - mid| and |oraclePx - markPx| in bp per class during US hours (median, p95, max); how many seconds the oracle lags a move of the mid larger than 10 bp.
4. Oracle vs GMX during US hours: one live GMX `/prices/tickers` fetch every minute for GOLD, SILVER, WTIOIL, BRENTOIL, NATGAS, SPY, QQQ and BTC/ETH/SOL; compare against HL oraclePx (xyz:GOLD, xyz:SILVER, xyz:CL, xyz:BRENTOIL, xyz:NATGAS; SPY and QQQ against xyz:SP500 and xyz:XYZ100 as ratios; crypto against the main dex). Report the difference in bp (median, p95, max).
5. Thin-market check: for EUR, GBP, JPY, JP225, KR200, PALLADIUM and the 5 smallest stocks by 24 h volume, report oracle change cadence and oracle-vs-mid gaps separately.
6. Plainly state: who sets the oracle (deployer address from `perpDexs`, `oracleUpdater` if any), and whether the oracle price continues to move while the traditional market is open and whether it freezes at any time.

PART 2: GMX WEEKDAY
Re-run the existing GMX probe (all of its steps, same output tables) during the same US-hours window, with these additions: (a) crypto majors BTC ETH SOL: distinct price changes per minute and gaps (they were not in the weekend summary); (b) GOLD, SILVER, WTIOIL, BRENTOIL, NATGAS, SPY, QQQ: changes per minute and gaps now that the venues are open, compared with the Saturday numbers; (c) `/markets/state`: does any market report `closed` during the weekday US session or anything other than `open`? (d) skip the 7-day candle and storage steps (already done).

REPORT FILES
handover/hl-oracle-probe-report.md and handover/gmx-probe-weekday-report.md, each with the tables. Do not overwrite the earlier reports.

TESTS
Unit tests for any new helper (gap statistics reuse, oracle-vs-mid bp, the staleness measure). `cargo test` must pass in both tools.

REPORT BACK (paste this block, nothing longer):
- run times (UTC) for both parts and the weekday/time check
- Part 1: does the message carry a server timestamp for oraclePx (yes/no); oracle changes per minute by class; longest oracle freeze while the book moved; oracle-vs-mid and oracle-vs-mark gaps per class (median, p95, max); thin markets table; oracle vs GMX table; who sets the oracle
- Part 2: crypto and metals/energy/SPY/QQQ change cadence now vs Saturday; `/markets/state` values seen; any new finding
- any 429/403/disconnect
- DECISIONS where this prompt was silent
- anything you refused or stopped on
- `git log --oneline -3` and `git status --short`
