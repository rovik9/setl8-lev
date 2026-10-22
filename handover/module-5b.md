# Module M5b: post_journal (DONE, verified 2026-10-11)

Toolchain as before (rustc 1.94.0, anchor-cli 0.32.1, cargo-build-sbf 4.0.0, litesvm =0.7.0). Builds on M5a (TradingAccount, vault harness), M2 (journal-types), M4 (engine).

## post_journal(batch_bytes: Vec<u8>): accounts in order
1 `evaluator` [S, W] (must equal `config.evaluator`) · 2 `trading_account` [W] (PDA `["acct", trader, challenge_id]`) · 3 `config` · 4 `ruleset` (PDA `["ruleset", trading_account.ruleset_version]`: the rule set the account was bought under; its `spread_e6[market]` is the half-spread) · 5 `event_authority` · 6 `program` (5 and 6 from `#[event_cpi]`) · remaining accounts: the referenced `Market` PDAs (`["market", id u16]`, owner = this program, at most 8, each verified by deserialising and re-deriving the PDA).
`batch_bytes` = Borsh of `JournalBatch`, decoded by `JournalBatch::decode`. The whole batch is atomic (any error reverts everything; nothing is written until every event passed).

Order of work: signer (I-1) -> status -> pass_pending -> decode -> `first_seq`/`prev_hash` (I-2) -> exactly one Meta, last (I-12) -> markets -> hash chain over the whole batch (`apply_batch`, `sol_sha256` hasher) -> per event: time (I-3), the event's own checks, `EventLog` via `emit_cpi!` -> batch end: a batch that ends flat cannot leave tight mode on (`TightNotEnded`) -> store state, `seq`, `journal_hash`, `last_batch_time`, `trades_closed`.
Events: `EventLog { account, seq, event_bytes, proof_bytes }` per journal event (raw canonical bytes; `proof_bytes` empty when the event has no proof). No `BatchPosted` event (decision below). Events are NOT part of the hash chain beyond the journal bytes themselves.

## RESOLVE answers
**(a) Price vs spread.** The proof carries the MID; the executed price is derived: `mid = floor(quantized_value / 10^6)` (`price_e12_from_quantized`), `h = rule_set.spread_e6[market_id]`, `ask = ceil(mid x (1_000_000 + h) / 1_000_000)`, `bid = floor(mid x (1_000_000 - h) / 1_000_000)` (engine `ask_from_mid` / `bid_from_mid`, wrapped by `engine::fill_price_e12` / `close_price_e12`). Fill long = ask, Fill short = bid, Close of a long = bid, Close of a short = ask (Breach/Fail closes are ordinary Close events). The event price must EQUAL the derived price (to the unit of 1e-12). `TightStart.p_open`, `FloorRaised`, `Breach` carry a proof only for feed, value and age (no price). DESIGN 4.9: mid 1.08 and h = 30 -> `ceil(1.08e12 x 1_000_030 / 1e6) = 1_080_032_400_000` = ask 1.0800324; mid 1.078 -> bid `floor(1.078e12 x 999_970 / 1e6) = 1_077_967_660_000` = 1.07796766. `journal_types::check_proof` was split instead of changed: `check_proof_without_price` (feed, source, age, value; the program derives the price itself) and `check_signed_update_shape`; `check_proof` still exists with its old behaviour and delegates to a shared inner function.
**(b) Exposure.** Every `Fill` increases exposure (DESIGN 8.1: reductions are `Close` events), so every Fill needs `held == 0` (`AccountHeld`), `ev.time >= lock_until` using the lock as updated inside the batch (`AccountLocked`), market status 1 and inside a session window. Close, Fee, TightEnd, Breach and the floor events are always allowed.
**(c) Pause.** DESIGN 6.3 / 10.8 do not make `post_journal` pausable; `config.paused` is not read, so a paused sector still posts journals (including Fills; only `open_account`, `request_advance`, ... check it). Listed under DECISIONS. Test: `the_sector_pause_does_not_stop_post_journal`.

## Invariants -> checks -> errors
| code | name | invariant |
|---|---|---|
| 6043 | BadPrevHash | I-2 |
| 6044 | EmptyBatch | limits |
| 6045 | BatchMalformed | limits |
| 6046 | ProofIndexOutOfRange | I-4 |
| 6047 | ProofMissing | I-4 |
| 6048 | ProofWrongFeed | I-4 |
| 6049 | ProofTooOld | I-4 |
| 6050 | ProofInFuture | I-4 |
| 6051 | ProofUnknownSource | I-4 |
| 6052 | ProofMockNotAllowed | I-4 |
| 6053 | ProofBadSignedUpdate | I-4 |
| 6054 | ProofValueInvalid | I-4 |
| 6055 | EventPriceMismatch | I-4 |
| 6056 | PriceInvalid | I-4 |
| 6057 | MarketDisabled | I-4 |
| 6058 | MarketReduceOnly | I-4 |
| 6059 | MarketClosed | I-4 |
| 6060 | QuantityInvalid | I-4 |
| 6061 | TimerInvalid | I-5 |
| 6062 | CreditMismatch | I-5 |
| 6063 | TightAlreadyOn | I-9 |
| 6064 | TightNotOn | I-9 |
| 6065 | TightStartPOpen | I-9 |
| 6066 | TightStartGPeak | I-9 |
| 6067 | TightStartR | I-9 |
| 6068 | TightStartLStart | I-9 |
| 6069 | TightStartLLevel | I-9 |
| 6070 | FloorNotRaised | I-9 |
| 6071 | FloorLLevel | I-9 |
| 6072 | TightEndNotFlat | I-9 |
| 6073 | TightNotEnded | I-9 |
| 6074 | BreachLLevel | I-10 |
| 6075 | BreachBalance | I-10 |
| 6076 | BreachNotBelowFloor | I-10 |
| 6077 | BreachBelowStaticFloor | I-10 |
| 6078 | BreachNotFlat | I-10 |
| 6079 | FailEventNotAllowed | I-11 |
| 6080 | MetaMissing | I-12 |
| 6081 | MetaNotLast | I-12 |
| 6082 | MetaTwice | I-12 |
| 6083 | EngineVersionMismatch | I-12 |
| 6109 | MarketUnpriced | I-1 / decision 64 (P21): an exposure-increasing `Fill` in a market whose `spread_e6` in the account's rule set is 0 |
| 6110 | ReviewPending | P25c (decision 67 amendment): `reset_account` / `close_account` on a Failed account whose hold is still set |

Older codes reused where they were already exactly the reason: `Unauthorized` (I-1 signer), `AccountNotActive`, `PassPending` (pass_pending == 2), `AccountHeld`, `AccountLocked` (I-10), `BadSequence` (I-2 `first_seq`), `TimeBackwards`, `TimeInFuture` (I-3), `MarketUnknown`, `PnlMismatch`, `QuickMismatch` (I-5), `FeeInvalid` (I-6), `QuickLimitExceeded` (I-8), `BatchTooLarge` (limits), `MathOverflow`.

## Engine and journal-types additions (small, pure, tested)
- `crates/engine/src/claims.rs`: `fill_price_e12`, `close_price_e12`, `CloseClaim`/`check_close` (I-5), `check_tight_start`, `check_floor_raised`, `check_tight_end`, `check_breach` (I-9, I-10), `ClaimError`. No thresholds of their own: all from `consts.rs`/`account.rs`. `position.rs`: `slice_credit(pnl, quick) -> (credit, tally)` factored out of `apply_fill` so the program and the engine share the rule.
- `crates/journal-types/src/proof.rs`: `check_signed_update_shape` (sources 2 and 4 empty, source 3 exactly 32 bytes), `check_proof_without_price`, `SIGNED_UPDATE_DIGEST_BYTES`, `ProofError::BadSignedUpdate`. This is a missing capability, not a defect of M2; `check_proof` is unchanged for callers.

## Tests
Total `cd tests-rs && cargo test`: 155 (95 before + 60 new in `tests/post_journal.rs`). Program lib unit tests 19 (3 new in `post_journal.rs`: mock follows the `localnet` feature, a mock proof is refused in a production build with `ProofMockNotAllowed`, error mapping is injective). Engine `tests/claims.rs`: 24 new. journal-types `tests/proof.rs`: +3. Every failure test asserts the exact error code AT INSTRUCTION 1 (instruction 0 is the compute-budget request) and an unchanged world digest (account bytes + all token balances).
By invariant: 4.9 (`design_4_9_worked_example_in_three_transactions`, `the_executed_price_follows_the_proof_mid_and_the_spread_for_every_side`); I-1 (5, plus `the_sector_pause_does_not_stop_post_journal`); I-2 (5); I-3 (3); I-4 (12); I-5 (5); I-6 (2); I-7 (2); I-8 (2); I-9 (8); I-10 (5); I-11 (1); I-12 (2); limits (2); atomicity (1); compute units (1); `market_accounts_other_than_markets_are_refused`.
- "One batch of 4 equals two of 2" is not literally possible: I-12 forces one Meta per batch, so the two shapes are different event sequences. `i2_batches_chain_and_the_emitted_logs_reproduce_the_head` instead proves the head after two transactions equals `journal_types::verify_chain` over the emitted EventLogs of both.
- Source 255: the test `.so` is the localnet build, so mock is ACCEPTED there (`i4_unsupported_sources_are_refused_and_the_mock_follows_the_build`); the refusal path in a production build is the unit test `a_mock_proof_is_refused_in_a_production_build` plus `journal_types` `MockNotAllowed` tests (run without and with `--features localnet`).

## DESIGN 4.9 reproduced (three transactions, account 100,000)
| after | b | q_tally | tight_on | r | g_peak | l_level | lock_until | flat | seq | trades_closed |
|---|---|---|---|---|---|---|---|---|---|---|
| A (fill + tight start) | 100,000 | 0 | 1 | 100,000 | 1,235.161 | 96,235.161 | 0 | 0 | 3 | 0 |
| B (floor raised) | 100,000 | 0 | 1 | 100,000 | 3,035.107 | 98,035.107 | 0 | 0 | 5 | 0 |
| C (close + breach) | 97,935.26 | 0 | 0 | 0 | 0 | 0 | t+12 h | 1 | 8 | 1 |
Close pnl -2,064.74 (age 150 s, not quick). The head after each transaction equals the independent `journal_types` computation; after C it equals `verify_chain` over the 8 EventLogs read from the inner instructions.

## Compute units (whole transaction incl. the 150 CU compute-budget instruction; numbers only; M11 set the budgets: `post_journal` worst case 125,276 CU and 1,181 bytes, handover/module-11.md)
1 event ([Meta]): 30,760 · 4 events ([Fee x3, Meta]): 65,056 · 8 events ([Fee x7, Meta]): 107,685 · 8 events with 3 proofs (2 Fill, 2 Close, TightStart, FloorRaised, Fee, Meta): 124,445. Stack: `scripts/build-test-so.sh` and `scripts/anchor-build-checked.sh` report no frame over 4096 bytes (default and localnet builds).

## Mutation testing
`cargo mutants` on `crates/engine` `claims.rs` + `position.rs`: 99 mutants, 91 caught, 7 unviable, 1 missed = 98.9% of viable. Survivor: `position.rs:65 replace > with >= in slice_credit` (`quick && pnl > 0`): for `pnl == 0` both branches give 0, so it is equivalent. `journal-types` `proof.rs`: 33/33 caught (3 new tests).
Hand-applied mutations in `post_journal.rs`/engine/constants (driver rebuilt the localnet `.so` and ran `cargo test --test post_journal` for each), 40 of 40 killed:
lock check removed (Fill)                          KILLED by 2: i10_a_breach_locks_the_rest_of_its_own_batch, i10_fills_are_refused_until_lock_until
lock check <= : now >= lock_until -> now > lock_until KILLED by 1: i10_fills_are_refused_until_lock_until
I-3 skew +2 -> +3                                  KILLED by 1: i3_events_may_be_at_most_two_seconds_ahead_of_the_block_time
I-3 t <= latest -> <                               KILLED by 1: i3_events_may_be_at_most_two_seconds_ahead_of_the_block_time
I-3 monotone >= -> >                               KILLED by 45: atomicity_a_bad_last_event_changes_nothing, compute_units_for_one_four_and_eight_event_batches, design_4_9_worked_example_in_three_transactions
prev_hash compare removed                          KILLED by 3: i2_a_reordered_or_dropped_event_changes_the_head_and_breaks_the_next_batch, i2_a_replayed_batch_is_refused, i2_prev_hash_must_equal_the_stored_head_to_the_bit
seq compare == -> >=                               KILLED by 1: i2_first_seq_must_be_seq_plus_one
latch threshold >= -> >                            KILLED by 2: i7_latch_is_reached_inside_one_batch_by_a_chain_of_slices, i7_latch_sets_at_exactly_104_percent_and_never_clears
quick 45 -> 46                                     KILLED by 1: i5_quick_flag_boundary_is_45_seconds_inclusive
q_tally > -> >=                                    KILLED by 2: i8_quick_tally_accumulates_across_batches_and_inside_a_batch, i8_quick_tally_at_28_percent_is_allowed_and_one_micro_more_is_refused
p_open strict -> non-strict                        KILLED by 1: i9_tight_start_p_open_must_be_strictly_above_one_percent
breach <= -> <                                     KILLED by 2: i10_a_breach_locks_the_rest_of_its_own_batch, i10_breach_b_after_must_be_at_or_below_the_tight_floor
Meta-last check removed                            KILLED by 1: i12_meta_must_be_present_last_and_unique
held check removed                                 KILLED by 1: i1_a_held_account_refuses_fills_but_allows_closes
fee sign flipped                                   KILLED by 2: i6_fee_amount_beyond_i64_is_refused, i6_fee_debits_the_balance_and_zero_is_allowed
credited floor -> ceil                             KILLED by 2: i5_credited_follows_the_quick_profit_rule, i8_quick_tally_at_28_percent_is_allowed_and_one_micro_more_is_refused
spread side swapped (Fill)                         KILLED by 12: compute_units_for_one_four_and_eight_event_batches, design_4_9_worked_example_in_three_transactions, i10_fills_are_refused_until_lock_until
spread side swapped (Close)                        KILLED by 21: atomicity_a_bad_last_event_changes_nothing, compute_units_for_one_four_and_eight_event_batches, design_4_9_worked_example_in_three_transactions
confirmed pass check wrong                         KILLED by 1: i1_a_confirmed_pass_freezes_the_journal_but_a_requested_one_does_not
status check removed                               KILLED by 1: i1_account_must_be_active
evaluator check removed                            KILLED by 1: i1_wrong_signer_is_refused
reduce-only allows fills                           KILLED by 1: i4_market_status_and_sessions_decide_whether_opening_is_allowed
session check removed                              KILLED by 1: i4_market_status_and_sessions_decide_whether_opening_is_allowed
proof age max+1                                    KILLED by 2: i4_floating_events_check_their_proof_against_a_passed_markets_feed_and_age, i4_proof_age_boundary_is_max_age_secs_inclusive
TightNotEnded check removed                        KILLED by 1: i9_a_flat_batch_cannot_leave_tight_mode_on
trades_closed not incremented                      KILLED by 5: design_4_9_worked_example_in_three_transactions, i1_a_held_account_refuses_fills_but_allows_closes, i5_pnl_must_equal_the_engine_pnl_to_the_micro
last_batch_time not stored                         KILLED by 2: design_4_9_worked_example_in_three_transactions, i3_events_may_be_at_most_two_seconds_ahead_of_the_block_time
floating proof feed unchecked                      KILLED by 1: i4_floating_events_check_their_proof_against_a_passed_markets_feed_and_age
breach not-flat check removed                      KILLED by 1: i10_breach_claims_are_checked_against_the_stored_state
tight-end not-flat check removed                   KILLED by 1: i9_tight_end_requires_a_flat_batch_and_clears_the_levels
quantity validation removed                        KILLED by 1: i4_quantity_must_respect_the_markets_step_and_minimum
fill price check removed                           KILLED by 1: i4_event_price_must_equal_the_price_derived_from_the_proof_and_the_spread
close price check removed                          KILLED by 1: i4_event_price_must_equal_the_price_derived_from_the_proof_and_the_spread
engine version check removed                       KILLED by 1: i12_meta_flat_is_stored_as_given_and_the_engine_version_must_match
journal_hash not stored                            KILLED by 4: design_4_9_worked_example_in_three_transactions, i2_a_reordered_or_dropped_event_changes_the_head_and_breaks_the_next_batch, i2_a_replayed_batch_is_refused
seq not stored                                     KILLED by 5: atomicity_a_bad_last_event_changes_nothing, design_4_9_worked_example_in_three_transactions, i2_a_replayed_batch_is_refused
event log not emitted                              KILLED by 2: design_4_9_worked_example_in_three_transactions, i2_batches_chain_and_the_emitted_logs_reproduce_the_head
quick limit rejection removed                      KILLED by 2: i8_quick_tally_accumulates_across_batches_and_inside_a_batch, i8_quick_tally_at_28_percent_is_allowed_and_one_micro_more_is_refused
Fail event allowed                                 KILLED by 1: i11_a_fail_event_is_refused
Fee on unknown market allowed                      KILLED by 1: i4_markets_must_exist_and_be_passed

## DECISIONS (design silent; strictest option taken)
1. `post_journal` is not blocked by `config.paused` (RESOLVE c).
2. Every Fill is exposure-increasing (RESOLVE b). A Close on a reduce-only or disabled market is allowed (a position is never trapped); Fills there are refused (`MarketReduceOnly` / `MarketDisabled`), also outside the session window (`MarketClosed`); Closes ignore sessions.
3. Events with no market field (`TightStart`, `FloorRaised`, `Breach`): the proof's feed must equal the feed of ONE of the passed markets (feed, value, age checked; no price). `Fee` still needs its market among the passed accounts.
4. `TightEnd` and `Breach` need the batch's Meta to say `flat = true` (`TightEndNotFlat`, `BreachNotFlat`); a batch that ends flat with tight mode still on is refused (`TightNotEnded`).
5. `Breach` also requires `b_after` strictly above the static floor (at or below is a fail, `BreachBelowStaticFloor`), and its slices are already applied to `b` before it (`BreachBalance`). After a breach the lock applies inside the same batch.
6. Source 3 `signed_update` must be exactly 32 bytes (decision 47); sources 2 and 4 empty; others unconstrained; a longer-than-512 blob is rejected as `ProofBadSignedUpdate` at decode time.
7. Quick limit: any slice leaving `q_tally > 0.028 S` rejects the whole batch (`QuickLimitExceeded`); the Fail path is M7. The static floor is not checked in `post_journal` (also M7), so `b` may go below it through fees/closes.
8. `Fail` events are always refused; 9 events and 4 proofs map to `BatchTooLarge`, 0 events to `EmptyBatch`, trailing/truncated bytes to `BatchMalformed`.
9. No `BatchPosted` event: the `EventLog`s carry `seq`, and the head is readable from the account.
10. A rule set with a stale `ruleset_version` fails the `ruleset` account constraint (`RuleSetMissing`).
11. `ALLOW_MOCK = cfg!(feature = "localnet")`.

## Harness (`tests-rs/tests/common/journal.rs`)
`JEnv` (real vault + a $100k account of a fresh trader; markets: EUR 0, BTC 1, reduce-only 2, disabled 3, one-minute-session 4; spreads EUR 30, BTC 50, others 10), batch builder using `journal_types::apply_batch` with `NativeSha256` (never program code), `post*` (compute-budget instruction first), `post_rejected` (exact error + unchanged digest), `edit_account`, `add_market(id, source, symbol, status)`, `raw_batch` for malformed bytes, `event_logs` from inner instructions, `chain_head_from_logs` via `verify_chain`, and `set_clock`. `VaultEnv` now creates an `evaluator` keypair and passes it to `init_config`.

## P21 (decision 64): unpriced markets are untradable
* **Where**: the `Fill` arm of the shared batch loop (`run_batch`, so `post_journal` and `report_fail` alike), after `acct.held`, the lock and `check_open_allowed` (market status and session), BEFORE `required_proof`, the quantity check, the proof check and the price check. `require!(half_spread != 0, Setl8Error::MarketUnpriced)` on the half-spread the arm reads anyway for the expected price (`env.half_spread(market)`, `spread_e6[market]` of the account's rule set, 0 for an id the version never listed). The `Close` arm is untouched: a close is never refused by this rule.
* **Why before the proof, price and quantity**: the refusal is about the market and the account's rule set, not about the evidence; pinning it first makes the error stable whatever else is wrong with the event (`the_unpriced_check_answers_before_the_price_the_proof_and_the_quantity`, both sides: the same fill in a priced market is refused for the later check).
* **No new account, no new instruction data, no layout change.** Compute: the check is a comparison on a value already loaded; the budget ceilings of `tests-rs/tests/budget.rs` hold (numbers in handover/module-11.md).
* Tests: `error_codes.rs` pins 6109; `tests-rs/tests/launch_config.rs` has the rule's executable paths (forged fills refused with `MarketUnpriced`, spread 1 accepted and 0 refused, a close accepted, `report_fail`); `trust_matrix.rs` three rows. The program test harness (`common/journal.rs`) has `TEST_HALF_SPREAD_E6 = 1` and `spread_ruleset()` gives every harness market (ids 5 and up) that half-spread; the fills of `post_journal.rs` and `budget.rs` that were priced at the mid are at `1_000_001_000_000` (long) and `999_999_000_000` (short).

## After the audit: P26 (decision 69)
Decision 7 above (quick limit) is amended for `report_fail` only: after a close has taken `q_tally` over the limit, `Close` events with reason `Fail` may follow before the `Fail` (forced close of the remaining positions); `post_journal` is unchanged. See module-7.md "After the audit: P26".
