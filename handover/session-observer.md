# Session observer (P20)

`tools/session-observer` is a long-running, read-only recorder of the 21 launch markets plus the controls that give them a second source, and the analyzer that turns the recordings into `handover/session-observation-report.md`. It settles the VERIFY flags of `config/launch-markets.toml` (weekend closure and reopen, daily breaks, the Asian index sessions, the US cash open hour, the real gaps inside sessions, the 14 to 20 bp offsets between sources) with measurements instead of memory.

## What it records (`tools/session-observer/data/<UTC date>/`, git-ignored)

The market list is read from `config/launch.json` at start (never hand-typed): 21 markets, 9 of source 2 (GMX) and 12 of source 4 (Hyperliquid xyz oracle).

| lane / file | source | what, and when a row is written |
|---|---|---|
| `gmx.csv` | `/prices/tickers` on `arbitrum-api.gmxinfra.io`, fallbacks `arbitrum-api-fallback.gmxinfra.io` and `...gmxinfra2.io`, every 5 s (one request, all symbols; `/tokens` hourly for the decimals) | the 9 GMX launch symbols; a row ONLY when `updatedAt` or min/max changes: `recv_ms,symbol,updated_at_ms,min,max,mid` |
| `gmx-state.csv` | `/markets/state` every 5 minutes | compact summary: market count, status counts, closed flags, transition timestamps set |
| `hl-a.csv`, `hl-b.csv` | Hyperliquid WebSocket `activeAssetCtx`, 2 connections (11 and 10 coins) | the 12 launch coins plus the 9 twins that give the GMX markets a second source (BTC, ETH, SOL, xyz:GOLD, xyz:SILVER, xyz:CL, xyz:BRENTOIL, xyz:SP500, xyz:XYZ100); a row ONLY when oraclePx, markPx or midPx changes: `recv_ms,transport,coin,oraclePx,markPx,midPx`; reconnect with back-off |
| `hl-rest.csv` | REST `metaAndAssetCtxs` dex `xyz` every 30 s (weight 20, 40 a minute of the 1,200 allowed) | the same columns, transport `rest`, change-only |
| `ref.csv` | the public reference (below), 4 products every 5 s | BTC, ETH, SOL (Coinbase spot) and GOLD (PAXG-USD, a gold-backed token): change-only `recv_ms,product,symbol,server_ms,price,bid,ask` |
| `pyth-feeds.csv`, `pyth-hours.csv` | Pyth Hermes `price_feeds` query (public), every 10 minutes | the feed ids resolved at start and the market hours Pyth PUBLISHES for each (`is_open`, `next_open`, `next_close`), a row when they change |
| `heartbeat.csv` | every 60 s | per lane: requests, errors, rows, reconnects, HTTP 429 and 403 counts, last change time per market |
| `../observer.log` | | one status line a minute, every error, back-off, reconnect and restart gap |

Files rotate at 00:00 UTC, carry a header line, are flushed every 10 s and fsynced every 60 s, and a restart appends (the restart line in `observer.log` says how long the gap was).

**The reference lane is NOT Pyth prices.** Pyth's price endpoints (`/v2/updates/price/latest`, the stream, `/api/latest_price_feeds`, the WebSocket, and Benchmarks) all answer HTTP 401 without an API key (checked 2026-10-17), and this run uses no keys. The Pyth lane therefore resolves the feeds and records Pyth's own published market hours (useful for the sessions), and the price reference is the public Coinbase Exchange spot ticker for BTC, ETH, SOL and gold (PAXG, which tracks spot gold with a small token premium or discount: a weak reference, labelled as such). There is NO price reference for silver, WTI, Brent, SPY, QQQ, the stocks or the Asian indices; the report says "cannot decide" for them.

## Start, status, stop

The binary runs from `~/setl8-observer-run/` (outside the repo), so rebuilding the tool cannot touch it.

```
# start (or restart after a crash; it appends to today's files)
cd ~/setl8-observer-run && nohup caffeinate -i ~/setl8-observer-run/session-observer run --until 2026-10-19T21:30:00Z --data-dir ~/Documents/setl8-lev-trading/tools/session-observer/data --launch ~/setl8-observer-run/launch.json > ~/setl8-observer-run/stdout.log 2>&1 &
# the process id (the observer, not caffeinate) is written by hand after the start:
pgrep -f "^$HOME/setl8-observer-run/session-observer run" | head -1 > ~/setl8-observer-run/observer.pid

# is it running, and how much memory
ps -o pid,rss,etime -p $(cat ~/setl8-observer-run/observer.pid)
# follow it
tail -f ~/setl8-observer-run/stdout.log
# the last heartbeat rows (one per lane per minute)
tail -n 6 ~/Documents/setl8-lev-trading/tools/session-observer/data/$(date -u +%F)/heartbeat.csv | cut -c1-160
# stop it cleanly (flushes and fsyncs, then exits)
kill $(cat ~/setl8-observer-run/observer.pid)
```

It exits only on SIGINT/SIGTERM or when `--until` passes. On a 429 or 403 a lane backs off 60 s; three in a row pause that lane 10 minutes; all other errors are logged and the lane retries. `caffeinate -i` keeps the Mac from idle sleep for as long as the process lives.

## What the owner must do

* Keep the Mac awake, online and on power until **Monday 2026-10-19 21:30 UTC = Tuesday 2026-10-20 03:00 IST**. Do not close the lid (a closed lid sleeps the Mac even under `caffeinate -i`; use an external display or leave the lid open). Do not let the battery run out.
* If the run dies (check `ps` above, or no new lines in `stdout.log`), restart with the same command. Nothing is lost except the minutes it was down, and the analyzer marks that downtime as OBSERVER DOWN instead of calling it a market break.
* Do not delete `tools/session-observer/data/`.

## Planned analysis

After the run ends, a later prompt runs

```
tools/session-observer/target/release/session-observer analyze tools/session-observer/data --out handover/session-observation-report.md --launch config/launch.json
```

and updates `config/launch-markets.toml` from the report (sessions, `max_age` freezes, the VERIFY flags). It works on a partial run too. The report has, per market: (a) every price-change gap over 120 s with start and end (UTC, day, minute of week) classified DAILY BREAK, WEEKEND CLOSE or IN-SESSION FREEZE; (b) the observed weekly windows next to the configured ones, every disagreement flagged and the narrower recommended; (c) cadence and p95 / max gap per hour of the week and the longest in-session gap (what `max_age_secs` must exceed); (d) the jump over every reopen; (e) the first 30 minutes after every reopen; (f) cross-source offsets (signed median, p95, max, per hour of the week) and which source sits furthest from the other two; (g) data quality (downtime, reconnects, 429 and 403 counts). Classification rules (all in `analyze.rs`, tested): a gap over Saturday noon UTC is a WEEKEND CLOSE; a gap of 15 minutes or more in a market that is not 24/7 is a DAILY BREAK; any other gap over 120 s is an IN-SESSION FREEZE; a gap that overlaps a heartbeat hole of more than 120 s is OBSERVER DOWN.

## The run that was started

* Started **2026-10-17 12:03:03 UTC (Saturday)**, PID 39757, under `caffeinate -i` (yes), until 2026-10-19T21:30:00Z. That is well before the Sunday 22:00 UTC reopen, so the run also records the Saturday closure itself, the whole Friday-evening-to-Sunday-evening weekend state of every market, and the reopen.
* Lanes: 6 data lanes (GMX tickers, Hyperliquid WebSocket in 2 connections, Hyperliquid REST, public reference, Pyth hours) plus the heartbeat. Markets: 21 launch markets (9 GMX, 12 Hyperliquid); Hyperliquid coins subscribed: 21 (12 launch + 9 twins); reference products: 4.
* Pyth Hermes feeds found: 23 of 24 asked (BTC, ETH, SOL, gold XAU, silver XAG, WTI 1-month index, Brent Z6 future and the Brent index, SPY, QQQ, JP225, KR200, AAPL, AMD, AMZN, GOOGL, META, MSFT, NFLX, NVDA, PLTR, TSLA, US500). Not found: `Equity.Index.NAS100/USD` (control xyz:XYZ100). No Pyth feed was found by that name for WTI futures (only the index products); the exact Pyth symbols are in `markets.rs`. **Pyth prices are not recorded** (HTTP 401, a key is required), only its published market hours.
* First hour (Saturday, quiet markets): 1.26 MB in the day directory (about 1.3 MB an hour; 20.9 thousand rows an hour: gmx 6,076, hl-a 4,126, hl-b 6,901, hl-rest 1,531, ref 1,883, heartbeat 342, pyth-hours 23, gmx-state 12), RSS 10.5 MB after 4 minutes and 11.3 MB after an hour (flat), no HTTP 429 or 403, no reconnect. Weekday sessions will write more.
* One incident: a DNS outage of the Mac from about 12:23 to 12:29:36 UTC made the reference and Pyth lanes fail their requests (250 and 24 errors; the GMX lane lost one request; the WebSocket lanes were unaffected). The lanes recovered by themselves; the log has one line per failed request. The analyzer reports lane errors per lane and marks no market gap for them (a missing reference interval simply has no overlap).

## Tests and mutation (the analyzer and the decision logic; the network code is excluded)

* 59 offline tests with hand-built timelines and fixtures, zero network: CSV round trip, rotation at 00:00 UTC, change-only recording, restart-append, flush every 10 s, gap classification (a gap of exactly 120 s is not reported, 120.001 s is; 899.999 s is a freeze, 900 s a break; the weekend rule around Saturday noon to the millisecond; observer downtime; 24/7), minute-of-week across Sunday midnight and the week wrap, session comparison and the narrower-window rule, reopen statistics, cadence buckets, offset statistics with hand numbers (ratio pairs, the outlier rule at its boundaries), heartbeat counters across restarts, back-off and pause logic with explicit clock values, `--until` handling, loading and the exact text of a hand-built report.
* cargo-mutants 27.1.0 on `analyze.rs`, `guard.rs`, `store.rs`, `timeutil.rs`, `markets.rs` (`lanes.rs` and `main.rs` need the network and a process, so they are excluded): first run 471 mutants, 418 caught, 32 missed, 16 unviable, 5 timeouts (hangs, counted as detected); the misses were report arithmetic and formatting, fixed by pulling the arithmetic out of `render` into functions (`reopen_stats`, `cadence_row`, `comparison_lines`, `due`, a loop-free `covers_saturday_noon`) and testing the exact text; second run 491 mutants, 468 caught, 2 missed, 17 unviable, 4 timeouts; the two misses (an hour without changes listed in the cadence table, a source compared with itself) got tests; third run on `render` and its helpers: 51 mutants, 50 caught, 1 unviable. No viable mutant survives. The 4 timeouts are `+` -> `*`/`/` mutations that loop forever.
* clippy `-D warnings`: clean.

## Decisions where the prompt was silent

1. **The price reference is Coinbase Exchange spot (BTC, ETH, SOL) and PAXG for gold, not Pyth prices**, because Pyth's price endpoints need a key (401). Pyth still contributes its published hours. Silver, WTI, Brent, SPY, QQQ, stocks and the Asian indices have no reference.
2. **The run started on Saturday**, as soon as it was ready, not just before Sunday 22:00 (more data, same stop time).
3. **Hyperliquid twins:** the 9 coins that give the GMX markets a second source (BTC, ETH, SOL on the main dex; xyz:GOLD, SILVER, CL, BRENTOIL, SP500, XYZ100) are subscribed in addition to the 12 launch coins.
4. **The classification thresholds** (gap over 120 s reported; 15 minutes or more and not 24/7 is a break; over Saturday noon UTC is the weekend; a heartbeat hole over 120 s is observer downtime) are mine; they are constants in `analyze.rs`.
5. **Observed windows** are the minutes of the week in which prices were changing, activity separated by less than 15 minutes counting as one stretch; the comparison with the configured windows is limited to the minutes of the week the run covered; the recommendation keeps the configuration for minutes the run did not cover.
6. **The GMX row** stores `mid` as the mean of min and max price; the archive rule of the GMX probe (an older `updatedAt` is ignored) is applied by the analyzer, not the recorder.
7. **The data directory per row date**: the files rotate at 00:00 UTC into `data/<date>/`, so `data/2026-10-17/`, `2026-10-18/`, ... instead of one directory named after the start date.
8. **`launch.json` is copied to `~/setl8-observer-run/launch.json`** at start, so the run reads nothing from the repo except writing its data there.
