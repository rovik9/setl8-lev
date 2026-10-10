# Module M3: engine, netting and costs (DONE, verified 2026-10-10)

Crate `crates/engine` (package `setl8-lev-engine`, lib `engine`). Library has zero dependencies; `proptest` is a dev-dependency only. Touched only `crates/engine`, `Cargo.lock` and this file.

## What was built
- `error.rs`: `EngineError {Overflow, InvalidQuantity, InvalidStopLevel, InvalidInput}` and `Result<T>`. Each use is documented on the variant (see the decisions below).
- `price.rs`: `ask_from_mid`, `bid_from_mid`, `price_from_oracle` (DESIGN 7.1, 9.1); crate-private `ceil_div_u128`.
- `position.rs`: `Side`, `Position`, `CloseSlice`, `OpenSlice`, `FillOutcome`, `apply_fill` (DESIGN 7.2), `pnl_micro`, `unrealized_micro`.
- `cost.rs`: `margin_micro`, `fee_ceil_micro`, `holding_fee_micro`, `day_index`, `nights_due`, `swap_fee_micro`, `validate_quantity`, `StopKind`, `validate_stop_level`.
- `units.rs`: `notional_micro` is now checked and returns `Result<i128>` (the old version could overflow `i128` for two near-`u64::MAX` factors).
- `lib.rs`: strict lints for non-test code (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `arithmetic_side_effects`, `cast_*` are denied) and re-exports. `consts.rs::frac` carries a documented `#[allow(clippy::arithmetic_side_effects)]` (plain arithmetic on at-most-$200,000 sizes; M4 revisits).

## Public API
Types: `EngineError`, `Result`, `Side {Buy, Sell}`, `Position`, `CloseSlice`, `OpenSlice`, `FillOutcome`, `StopKind {StopLoss, TakeProfit}`.
Functions: `ask_from_mid`, `bid_from_mid`, `price_from_oracle`, `apply_fill`, `pnl_micro`, `unrealized_micro`, `margin_micro`, `fee_ceil_micro`, `holding_fee_micro`, `day_index`, `nights_due`, `swap_fee_micro`, `validate_quantity`, `validate_stop_level`, `units::notional_micro`; constants in `consts` (unchanged) and `ENGINE_VERSION`.

## Tests (69)
- Unit 6 (`consts` 4 incl. new `time_constants_match_design`; `units` 2).
- Integration 56: `tests/costs.rs` 23, `tests/netting.rs` 33.
- Property 7 (`tests/props.rs`, 256 cases each): (a) quantity follows the signed sum, flat iff sum 0, change equals the fill size, closing slice bounded; (b) realized + unrealized never exceeds the exact rational profit (and is within 120 micro-USD of it); (c1) mirror trade has identical pnl; (c2) the same move on a long and a short cancels up to one micro-USD (`a + b` is 0 or -1); margin is a tight ceiling; ask/bid bracket the mid; `apply_fill` etc. never panic on arbitrary input.
- Worked examples: DESIGN 7.5 (merge 1.117142857143, partial reduce +857.142857, flip +1,967.60 with remainder short 500,000 @ 1.0820000 and margin $10,820, quick flip credit 983.80 and tally 983.80) and 4.9 (ask 1.0800324, notional $1,080,032.40, margin $21,600.648, spread cost $64.80, the whole bid column, the breach close -$2,064.74, the late-sample -$20,064.20). Short twins for merge, reduce, flip, quick flip, quick boundary, quick loss, odd credit, N-7, N-9, stop levels.
- Edge cases N-1, N-2, N-3 (margin of the remainder), N-4, N-7, N-8, N-9 are tested. N-5, N-6 and the trigger part of N-10 belong to M4 (comments in the tests); the validator side of N-10 (touching is valid) is tested.

## Mutation gate
`cd crates/engine && cargo mutants --no-shuffle` (cargo-mutants 27.1.0): **97 mutants: 95 caught, 1 missed, 1 unviable, 0 timeouts. 95 of 96 viable killed = 99.0%** (gate 95%).
First run was 106 mutants / 9 missed (91%): four were the untested `LOCK_SECS = 12 * 60 * 60` (fixed by `time_constants_match_design`), the others `>` vs `>=` comparisons on values that can never be zero; those were removed by restructuring (`signum() == 1`, `long == (side == Side::Buy)`, an explicit zero-quantity check in `unrealized_micro`, and the tally as `credit.max(0)`).
Survivor (equivalent): `position.rs:164` `quick && pnl > 0` -> `>=`. For `pnl == 0` both branches give 0 (`floor(0 / 2) == 0`), so no test can tell them apart.
Unviable: `apply_fill -> Ok(Default::default())` (does not compile).

### Hand-applied mutants (BUILD_PLAN section 1, "netting")
cargo-mutants cannot generate semantic mutants such as "floor to ceil", so each listed mutant was applied by hand (`python3` driver, restored after each run) and the full suite run. All 22 were killed except one equivalent:
pnl floor to ceil (6 tests fail), average entry long ceil to floor, short floor to ceil, margin ceil to floor, quick credit floor to ceil, `<= 45` to `< 45`, timer not restarted on add, partial close resetting the timer, flip remainder keeping the old average, flip remainder keeping the old timer, closing slice `min` to `max`, sign error on shorts (pnl), short marked at the bid, pnl truncating `as i64` instead of `try_from`, pnl wrapping multiply (i128 overflow path), exact flatten not clearing the position, ask floor instead of ceil, bid ceil instead of floor, fee floor instead of ceil, swap fee floor instead of ceil, day index without the 22 h shift.
SURVIVED (equivalent): merge numerator `checked_add` -> `wrapping_add`. The sum `|q| A + |dq| p` is below 2^127 whenever the merged quantity fits `i64` (it is at most `(|q| + |dq|) x u64::MAX`), and a merged quantity beyond `i64::MAX` is rejected first, so the addition cannot overflow; the check is defensive.

## Clippy
`cargo clippy --offline -p setl8-lev-engine -- -D warnings` passes, also with `--all-targets`.

## Decisions (design silent)
1. `apply_fill`: `dq == 0` is `InvalidQuantity`; `dq > i64::MAX` is `InvalidQuantity` (cannot be a signed quantity); price 0 is `InvalidInput`; a stored position with `qty == 0` is `InvalidInput` (flat is `None`).
2. Time never runs backwards: `t < timer_start` on an existing position is `InvalidInput` (otherwise a negative age would count as quick); `t - timer_start` overflowing is `Overflow`. A merge or close at the same second as the timer is allowed (age 0 is quick).
3. `|qty|` is limited to `i64::MAX` on both sides: a short merging down to `i64::MIN` is `Overflow`, so longs and shorts are symmetric.
4. Merge keeps `opened_at` and `fee_day`; a flip remainder gets `opened_at = t`, `fee_day = fee_day_now`, a fresh timer and the fill price as average.
5. Quick tally delta is `credit.max(0)` for a quick slice (the credited half-profit; losses, zero and a 1 micro-USD profit add nothing), 0 otherwise; `balance_delta` equals the slice credit.
6. `holding_fee_micro` takes ONE ceiling over `nights x rate x qty x price / (DEN x 1e6)` (the notional is not rounded first; two ceilings could charge one micro-USD more). `fee_ceil_micro` is for an already-rounded micro-USD notional. Commission on a position should use `holding_fee_micro(qty, price, rate, 1)` for the same exact form.
7. `margin_micro`: single ceiling; for integer leverage this equals ceil of a ceiled notional, so there is no separate test for "single vs double ceil". Zero leverage is `InvalidInput`.
8. `bid_from_mid` with `h > 1e6` is `InvalidInput`; `ask_from_mid` has no upper bound on `h` (overflow is `Overflow`). `price_from_oracle` returns 0 for values below 1e6 (callers reject a zero price).
9. `validate_stop_level(position_side, ...)`: `Buy` means the position is long. Valid (equality counts as valid, it triggers): long SL `level <= bid`, long TP `level >= bid`, short SL `level >= ask`, short TP `level <= ask`. A level of 0 (the journal's "none") is `InvalidStopLevel`. Bid above ask is not checked.
10. `validate_quantity`: zero step is `InvalidInput` (misconfigured market); everything else is `InvalidQuantity`.
11. `day_index` returns `i64` (not a `Result`) and saturates at the extreme ends of `i64` time, where the index can be off by one; real times are around 1.7e9.
12. `unrealized_micro` of a zero-quantity position is `InvalidInput`.
13. `nights_due` is `InvalidInput` when `today < fee_day` and `Overflow` when the count exceeds `u32`.

## Findings
- The proptest suite found a bug in the test itself during the mutation run (a zero exit price in `c1`); fixed. `props.proptest-regressions` is committed.
- Proptest randomness makes mutation results slightly non-deterministic for mutants that only property tests catch; the final run's numbers are above.
