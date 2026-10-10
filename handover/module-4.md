# Module M4: engine rules (DONE, verified 2026-10-11)

Added to `crates/engine`: `account.rs`, `floors.rs`, `triggers.rs`, one function in `position.rs`, tests. Zero library dependencies; only `crates/engine`, `Cargo.lock` (unchanged) and this file were touched.

## Public API
- `account.rs`: `AccountState` (+ `new_phase`), `Rules {frozen_ratchet, quick_strict}` (+ `Default`: both true), `FailReason {StaticFloor, PhaseQuick}`, `frac_exact`, `static_floor_micro`, `effective_floor_micro`, `can_increase_exposure`, `apply_closed_slice` -> `ClosedSliceDecision {state, fail}`, `end_tight_if_flat`, `pass_check` -> `PassError {NotFlat, TargetNotReached, PhaseFailed}`, `payout_micro`, `next_phase` -> `NextPhase {Next{phase, payout}, Graduated}` (+ `pays()`), `reset_phase_state`.
- `floors.rs`: `evaluate_sample` -> `SampleDecision {action, state, equity_micro}`, `SampleAction {None, Fail(FailReason), Breach{close_all, lock_until}, TightStart{r, l_start, g_peak, l_level}, FloorRaised{g_peak, l_level}}`.
- `triggers.rs`: `check_protective` -> `Option<ProtectiveTrigger {kind, price_e12}>`, `ProtectiveKind`, `check_pending` -> `Option<fill price>`, `PendingKind {Limit, Stop}`, `sl_tp_allowed_after`.
- `position.rs`: `open_profit_micro(&[(Position, bid, ask)])`.

## Behaviour
- `evaluate_sample` order is fixed: (a) `E <= F_s` -> `Fail(StaticFloor)`; (b) `tight_on && E <= max(F_s, L)` -> `Breach`; (c) `!tight_on && p_open > 0.01 S` (strict, on P not E) -> `TightStart` with `r = b`, `l_start = min(F_s + 0.04 S, r)`, `g_peak = max(0, E - r)`, `l_level = l_start + max(0, g_peak - 0.01 S)`; (d) `tight_on && G = E - r > g_peak` -> `FloorRaised`; (e) `None`. `E = b + p_open - closing commission estimate`. All thresholds are `frac_exact(S, bps)` (an inexact size is `InvalidInput`).
- `Breach` returns the state with tight mode off and its levels reset to 0, `flat = true`, `lock_until = now + 12 h`; the evaluator then closes everything at the sample and credits each slice with `apply_closed_slice`. `Fail` leaves the state unchanged. A locked account still runs all floors; `can_increase_exposure` is `now >= lock_until`.
- **A-RULE-1 (`frozen_ratchet`)**: default true: a latch in the middle of a tight mode leaves `l_start`/`l_level` untouched; the breach floor is `max(F_s, L)`. Off: when `apply_closed_slice` flips the latch while `tight_on`, `l_start = min(0.96 S + 0.04 S, R)` and `l_level = l_start + max(0, g_peak - 0.01 S)` (R and `g_peak` kept; this can only raise L because F_s only rises); breach floor is still `max(F_s, L)`. Nothing else differs.
- **A-RULE-2 (`quick_strict`)**: default true: `Q > 0.028 S` fails (`PhaseQuick`) at the slice that crosses it; off: `Q >= 0.028 S`.
- `apply_closed_slice(state, rules, credit, quick_tally_delta)`: `b += credit`, `q += delta` (negative delta is `InvalidInput`: quick losses never reduce Q), latch when `b >= 1.04 S` (never clears), quick check. The `rules` argument is an addition to the prompt's signature (the quick switch and the A-RULE-1 recompute need it).
- Triggers: long SL `bid <= sl`, TP `bid >= tp`; short SL `ask >= sl`, TP `ask <= tp`; SL beats TP; fills at the sample's bid/ask (gap borne by the trader). Buy limit `ask <= level`, sell limit `bid >= level`, buy stop `ask >= level`, sell stop `bid <= level`, filled at the sample price. Closed market blocks both kinds; a locked account blocks pending orders only; level 0 = not set.

## Tests (new in M4: 64; engine crate total 133)
- Unit 6, integration 116 (`costs` 23, `netting` 33, `rules` 45, `triggers` 15), proptest 11 (`props` 7, `rules_props` 4; 256 cases each).
- `rules.rs`: constructors, DESIGN 2.6 R1 (all five rows, breach at t5, B 98.0k, 12 h lock), R2 permanent shift, R3 quick credit/loss/limit (strict, non-strict, boundaries), decision 31 example (B 93k -> L 93.1k) and cap inactive at B >= F_s + 0.04 S, DESIGN 4.9 row by row (drives `apply_fill`, `margin_micro`, `open_profit_micro`, `evaluate_sample`, `apply_closed_slice`), late sample variant, every boundary of the prompt, check order, tight lifecycle, A-RULE-1 default and off, phases, portfolio helper.
- `triggers.rs`: every case with long and short mirrors. `rules_props.rs`: L never falls inside a mode, static floor non-decreasing, latch monotone, Q non-decreasing, Fail iff E <= F_s whatever the tight state, Breach only when tight was on, pass definition, determinism, no panics on arbitrary state.

## 4.9 numbers reproduced exactly
ask 1.0800324; margin 21,600.648; tight start `r = 100,000.000000`, `l_start = 96,000.000000`, `g_peak = 1,235.161000`, `l_level = 96,235.161000`; raise `g_peak = 3,035.107000`, `l_level = 98,035.107000`; at 14:02:00 nothing changes; breach at 14:02:30 with E 97,935.26 <= 98,035.107; close at bid 1.07796766, pnl -2,064.740000 (age 150 s, not quick); B 97,935.260000. Late variant: E 79,935.80 <= 92,000 gives `Fail(StaticFloor)`, B 79,935.80.

## Mutation gate (whole crate, M3 + M4)
`cargo mutants --no-shuffle` (v27.1.0): **219 mutants: 210 caught, 1 missed, 8 unviable, 0 timeouts: 210 of 211 viable = 99.5%** (gate 95%).
- Survivor (equivalent): `position.rs:164` `quick && pnl > 0` -> `>=`: for `pnl == 0` both give credit 0.
- Unviable (do not compile): `Ok(Default::default())`-style replacements of `new_phase`, `apply_closed_slice`, `end_tight_if_flat`, `next_phase`, `reset_phase_state`, `evaluate_sample`, `apply_fill`, `check_protective`.
- A first run had 2 survivors; the extra one (Breach state's `flat: true` field deleted) is now killed by `a_breach_sets_flat_even_if_the_account_was_not_flat`. One hand mutant (`slack max(0)` removed) was only caught by a property test, so a deterministic assertion was added to `boundary_tight_start_is_strictly_above_one_percent`.
- **Hand-applied "rules" mutants** (BUILD_PLAN section 1; cargo-mutants cannot make semantic ones), 37 applied one at a time, full suite each, all KILLED: `E <= F_s` -> `<`; `E <= L` -> `<`; L allowed to fall; `g_peak` not monotonic (two variants); raise using the old `g_peak`; latch clearing; latch at `>` 1.04 S; `0.01 S` non-strict; `min(F_s + 0.04 S, R)` -> `max`, cap removed, R only; `g_peak` not floored at 0; slack `max(0)` removed; tight mode not ending when flat / ending when not flat; quick tally reduced by losses (netting delta and engine check); `q_tally > limit` -> `>=`; `quick_strict` off behaving strict; check order (breach before static fail; SL/TP before breach); effective floor `max` -> `min` / ignoring L; wrong lock length; lock off by one at `lock_until`; commission added instead of subtracted; breach not ending tight mode / not setting the lock; static floor ignoring the latch; pass at `>` 1.08 S; pass ignoring flat / failed; A-RULE-1 recompute disabled / applied when frozen; SL no longer beating TP; pending orders ignoring the lock.

## Clippy
`cargo clippy --offline -p setl8-lev-engine --all-targets -- -D warnings` passes.

## Decisions (design silent)
1. `evaluate_sample` keeps the prompt's `rules` parameter but does not use it (both switches act in `apply_closed_slice`); it is `_rules`.
2. `Breach` carries `close_all: true` as specified (always true). The state it returns is described above.
3. `apply_closed_slice` takes `rules` (see above); a negative `quick_tally_delta` is `InvalidInput`.
4. `pass_check` reports `PhaseFailed` first, then `NotFlat`, then `TargetNotReached`. An unusable size (inexact thresholds) is `TargetNotReached`.
5. `next_phase` returns `Result<NextPhase>` (phases other than 1 to 5 are `InvalidInput`).
6. `check_protective` takes `market_open` (a closed market triggers nothing, DESIGN 3.4) although the prompt listed it only for pending orders; locks do not block SL/TP (they are exits).
7. `sl_tp_allowed_after(&SampleAction)` was added so the rule "static fail, then breach, then SL/TP" is a tested function.
8. `end_tight_if_flat` also stores `flat` and resets `r`, `l_start`, `g_peak`, `l_level` to 0 when it ends a mode.
9. `evaluate_sample` rejects a negative commission estimate (`InvalidInput`). `TightStart` is decided on `p_open`, not on `E`; `g_peak` at start is `max(0, E - r)`.
10. `unrealized_micro`/`open_profit_micro` of a zero-quantity position is `InvalidInput` (from M3).
11. The `flat` flag is not read by `evaluate_sample`; it is state that the evaluator maintains through `end_tight_if_flat` and `Breach`.

## Open points for the next modules
- The program (M5) re-implements checks I-7 to I-11 from this module's formulas; the hand-written boundary suite there should use the same boundary values as `rules.rs`.
- `Rules` has no representation in the journal yet; if the owner changes A-RULE-1/2, the rule-set version must carry the switches.
