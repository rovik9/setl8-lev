You are building module M3 (engine: netting and costs) of setl8-lev-trading in THIS repo (branch main). M1b is done (commit 4abca49). Read first, in this order: docs/DESIGN.md sections 2.4 (quick trades), 4.9 (worked example numbers), 7 (ALL of it: this is your specification), 9.1 to 9.4; docs/BUILD_PLAN.md section 0, section 1 (mutation targets, netting part) and the M3 row; handover/design-decisions.md (decisions 10, 14, 22, 23, 26); crates/engine/src (consts.rs, units.rs, lib.rs) and scripts/test-engine.sh.

RULES
- Work only in crates/engine (plus scripts/ and handover/). Do NOT touch programs/, tests-rs/ or the other repos. NEVER read, print or commit private keys, .env files or keypair JSONs.
- Do not push, tag or change git config. Commit only with explicit paths after scripts/secret-scan-staged.sh. Author env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com. Commit message ends with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- The engine stays pure: integer math only, no floats, no std::time, no unsafe, no unwrap/expect/panic/indexing-that-can-panic in non-test code, NO `as` casts that can truncate (use try_from / checked ops; widening casts are fine). Every arithmetic step is checked and returns `Err(EngineError::Overflow)` on failure. `proptest` is allowed as a dev-dependency ONLY; the library itself keeps zero dependencies.
- The DESIGN is the spec. If it is wrong or contradictory, STOP and report. Where silent, take the smallest safe option and list it under DECISIONS.

SCOPE (nothing else; rules, floors, tight mode and phases are module M4)
Files under crates/engine/src: `error.rs`, `price.rs`, `position.rs` (types + `apply_fill`), `cost.rs`, plus tests in each file's `#[cfg(test)]` module or `crates/engine/tests/`.
1. `EngineError` enum: Overflow, InvalidQuantity, InvalidStopLevel, InvalidInput (use sparingly, document each use).
2. Price (DESIGN 7.1, 9.1): `ask_from_mid(mid_e12: u64, half_spread_e6: u32) -> Result<u64>` = ceil(mid x (1e6 + h) / 1e6); `bid_from_mid` = floor(mid x (1e6 - h) / 1e6) (h > 1e6 is InvalidInput); `price_from_oracle(value_e18: u128) -> Result<u64>` = floor(value / 1e6) (the oracle value has 18 decimals, engine prices 12).
3. Types: `Side {Buy, Sell}`; `Position { qty: i64 (signed, long > 0), avg_entry_e12: u64, timer_start: i64, opened_at: i64, fee_day: i64 }`.
4. `apply_fill(pos: Option<Position>, side, dq: u64, price_e12: u64, t: i64, fee_day_now: i64) -> Result<FillOutcome>` implementing 7.2 EXACTLY. `FillOutcome` carries: the new `Option<Position>`; `closed: Option<CloseSlice>` with { qty_abs, entry_e12, exit_e12, pnl (i64 micro-USD, floor), quick (bool), credit (i64) }; `opened: Option<OpenSlice>` { qty (signed), price_e12 } for an open, add or flip remainder; `balance_delta: i64` (= the credit; fees are NOT in here); `quick_tally_delta: i64` (= floor(pnl / 2) when quick and pnl > 0, else 0). Rules to implement and test one by one: open; merge with average rounding (long ceil, short floor) and `timer_start = t`; partial reduce keeps average and timer; exact flatten returns `None`; flip closes the full old position then opens the remainder at `price` with a fresh timer, `opened_at = t` and `fee_day = fee_day_now`; quick means `t - timer_start <= 45` (boundary 45 is quick, 46 is not); loss credited in full, profit floored to half only when quick; pnl = floor(sign(q) x c x (p - A) / DEN) with floor toward minus infinity (use div_euclid, a negative pnl rounds to the larger loss).
5. `cost.rs`: `margin_micro(qty_abs: u64, price_e12: u64, leverage_x: u16) -> Result<i64>` = ceil(|q| x p / DEN / leverage) (single ceil of the whole expression, not ceil of ceil); `fee_ceil_micro(notional_micro: i64, rate_e6: u32) -> Result<i64>` = ceil(notional x rate / 1e6) used for holding fee (times nights n), commission and the swap-cost fee; `holding_fee_micro(qty_abs, mark_price_e12, rate_e6, nights: u32)`; `day_index(unix_secs: i64) -> i64` = floor((t - 22 x 3600) / 86400) with euclid division, so the index changes at 22:00 UTC; `nights_due(fee_day: i64, today_index: i64) -> Result<u32>` (error if today < fee_day); `swap_fee_micro(tier_cost_micro: u64, fee_bps: u16)` = ceil(cost x bps / 10_000); `validate_quantity(qty: u64, qty_step: u64, qty_min: u64)` (N-1); `validate_stop_level(side_of_position, stop_kind, level, bid, ask)` (N-4: SL above bid for a long, or below ask for a short, or TP on the wrong side, is InvalidStopLevel; touching counts as valid-but-triggered, so equality at the current price is NOT InvalidStopLevel; document your exact choice under DECISIONS).
6. `unrealized_micro(pos, mark_bid_e12, mark_ask_e12)`: longs marked at bid, shorts at ask, floor.

TESTS (each a named test)
- Every worked example in DESIGN 7.5 (merge 1.117142857143, partial reduce +857.142857, flip +1,967.60 with remainder short 500,000 @ 1.0820000 and margin $10,820, quick flip credit $983.80 and tally 983.80) and the DESIGN 4.9 numbers (mid 1.08, h = 30 e6 -> ask 1.0800324; notional 1,080,032.40; margin 21,600.648 at 1:50; spread cost 64.80).
- Edge cases N-1 to N-10 from 7.6 that belong to the engine (N-2, N-3 margin helper, N-4, N-7 timer restart on add then quick close, N-9). Mark the ones that belong to M4 (N-5, N-6, N-10 triggers) with a comment, do not build them.
- Boundaries: quick at 45 s vs 46 s; zero dq rejected; dq = 1 unit; max values returning Overflow not panic (qty i64::MAX with price u64::MAX; merge numerator overflow).
- Short side mirrors: every long test has a short twin (sign errors are a listed mutant).
- Rounding direction tests: one case per row of the 7.4 table where floor vs ceil differ by exactly 1 micro-USD.
- proptest invariants (256 cases each is enough): (a) a random sequence of fills never makes `qty` change by more than the fill's size and ends flat iff signed sum of fills is 0; (b) realized pnl summed over closing slices plus unrealized at the final mark never exceeds the exact rational profit (no value created by rounding); (c) apply_fill on a flat position and its mirror image gives pnl equal up to rounding direction (|a| - |b| <= 1).

MUTATION GATE (this is the point of the module)
On the Mac run `cargo install cargo-mutants` if it is not installed (crates.io works on the Mac), then `cd crates/engine && cargo mutants --no-shuffle` (mutants testing only this crate; skip proptest-heavy slowness with `-- --release` or by lowering PROPTEST_CASES if needed). Required: kill rate >= 95% of viable mutants; every mutant listed in BUILD_PLAN section 1 under "netting" MUST be killed. For each survivor either add a test that kills it or explain in handover/module-3.md why it is equivalent. Report the exact numbers (caught / missed / unviable / timeout). If cargo-mutants cannot be installed or run, STOP and report; do not claim the gate passed.

DEFINITION OF DONE
`scripts/test-engine.sh` passes; `cargo clippy --offline -p setl8-lev-engine -- -D warnings` passes (if clippy is installed; else say so); mutation gate met. Write handover/module-3.md (what was built, public API listing, test counts, mutation numbers, survivors with reasons, decisions). Commit with explicit paths; do NOT push.

REPORT BACK (paste this block, nothing longer):
- test counts (unit, integration, proptest) and the list of test names grouped by topic
- mutation result numbers and the list of survivors (or "none") with one line each
- clippy result
- public API: one line per pub fn/type
- every file added/changed, one line each
- DECISIONS where the design was silent
- anything you refused or stopped on
- `git log --oneline -3` and `git status --short`
