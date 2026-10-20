# Audit fragment 08a: trust-matrix rows and the dishonest-evaluator table (M12 item 8, first part)

SELF-AUDIT by the builder; not an independent audit. Finding prefix TMX. Files: `tests-rs/tests/trust_matrix.rs` (edited), `tests-rs/tests/adversarial_evaluator.rs` (new). No production code was touched.

## 1. One named test per trust-matrix row

Before: one table-driven test (`trust_matrix`) over `rows()` and one over `chain_rows()`. If row 40 failed, the report listed it among the others and nothing in the test list said which rows existed.

Now: `check_row` / `check_chain_row` run one row; `row_tests!` / `chain_row_tests!` (macro_rules) write one `#[test]` per row AND the listing `ROW_TESTS` / `CHAIN_ROW_TESTS` from the same line. A per-row test finds its row by exact NAME in `rows()` (not by position), so reordering the table cannot make a test check another row, and a renamed or deleted row fails its test. The aggregate tests `trust_matrix` and `trust_matrix_chain_rows` are kept.

New reconciliation tests in `trust_matrix.rs`:

* `every_row_has_exactly_one_named_test`: 72 rows and 4 chain rows (the numbers the document states); `ROW_TESTS.len() == rows().len()`; every row name appears in exactly one listing entry and in the same position; no duplicate row names; no two rows share a test function name.
* `trust_matrix_document_matches_rows`: parses the tables of `handover/trust-matrix.md` (BOTH, PROGRAM, VERIFIER, NEITHER = 72 rows; chain-level = 4 rows) and compares, for every row, the WHO, the program error name and code, and the verifier finding classes with the test table. Result: document and tests agree on all 76 rows (no edit to the document was needed).
* `reset_rows_listing_matches_document_and_source`: the 11 Reset (M8) rows are listed in the const `RESET_ROWS`; the test pins that the document has exactly these 11 and that `tests/trust_reset.rs` carries each name once and no other row.

Result: 72 + 4 per-row tests, all passing (`cargo test --offline --test trust_matrix`: 87 passed, 0 failed; 8 old tests + 76 per-row + 3 new). Run time about 14 s.

### Row to test (72 batch/log rows)

| test function | row | WHO | program | verifier |
|---|---|---|---|---|
| `row_001_fill_price_altered_by_one_unit` | fill price altered by one unit | Both | EventPriceMismatch (6055) | [PriceMismatch] |
| `row_002_close_exit_price_altered_pnl_recomputed` | close exit price altered (pnl recomputed) | Both | EventPriceMismatch (6055) | [PriceMismatch] |
| `row_003_close_pnl_wrong_by_one_micro_usd` | close pnl wrong by one micro-USD | Both | PnlMismatch (6011) | [PnlMismatch] |
| `row_004_quick_flag_flipped_on_a_quick_slice` | quick flag flipped on a quick slice | Both | QuickMismatch (6012) | [QuickMismatch] |
| `row_005_quick_profit_credited_in_full` | quick profit credited in full | Both | CreditMismatch (6062) | [CreditMismatch] |
| `row_006_fee_amount_beyond_i64` | fee amount beyond i64 | Both | FeeInvalid (6013) | [FeeMismatch] |
| `row_007_tight_start_l_level_one_micro_off` | tight start: l_level one micro off | Both | TightStartLLevel (6069) | [FloorMismatch] |
| `row_008_floor_raise_l_level_one_micro_off` | floor raise: l_level one micro off | Both | FloorLLevel (6071) | [FloorMismatch] |
| `row_009_floor_raise_dropped_the_breach_then_names_a_stale_level` | floor raise dropped (the breach then names a stale level) | Both | BreachLLevel (6074) | [FloorMismatch] |
| `row_010_tight_start_and_floor_raise_swapped` | tight start and floor raise swapped | Both | TightNotOn (6064) | [FloorMismatch] |
| `row_011_meta_engine_version_wrong` | meta: engine version wrong | Both | EngineVersionMismatch (6083) | [EngineVersion] |
| `row_012_batch_replayed` | batch replayed | Both | BadSequence (6007) | [SequenceGap] |
| `row_013_wrong_prev_hash` | wrong prev_hash | Program | BadPrevHash (6043) | - |
| `row_014_posted_by_a_key_that_is_not_the_evaluator` | posted by a key that is not the evaluator | Program | Unauthorized (6000) | - |
| `row_015_event_time_goes_backwards` | event time goes backwards | Both | TimeBackwards (6008) | [StateMismatch] |
| `row_016_event_dated_30_s_after_the_block_time` | event dated 30 s after the block time | Program | TimeInFuture (6009) | - |
| `row_017_proof_carries_another_market_s_feed` | proof carries another market's feed | Both | ProofWrongFeed (6048) | [ProofFeed] |
| `row_018_proof_older_than_the_market_s_max_age` | proof older than the market's max_age | Both | ProofTooOld (6049) | [ProofAge] |
| `row_019_batch_of_nine_events` | batch of nine events | Both | BatchTooLarge (6032) | [FeeMismatch] |
| `row_020_fill_in_a_closed_session` | fill in a closed session | Both | MarketClosed (6059) | [FillNotAllowed] |
| `row_021_fill_quantity_not_a_multiple_of_the_step` | fill quantity not a multiple of the step | Both | QuantityInvalid (6060) | [ForgedTrade, FillNotAllowed] |
| `row_022_commission_fee_missing_after_a_close` | commission fee missing after a close | Verifier | - | [FeeMismatch] |
| `row_023_commission_fee_one_micro_usd_too_high` | commission fee one micro-USD too high | Verifier | - | [FeeMismatch] |
| `row_024_commission_fee_placed_before_its_fill` | commission fee placed before its fill | Verifier | - | [FeeMismatch] |
| `row_025_holding_fee_omitted_at_the_rollover` | holding fee omitted at the rollover | Verifier | - | [FeeMismatch] |
| `row_026_holding_fee_one_micro_usd_too_low` | holding fee one micro-USD too low | Verifier | - | [FeeMismatch] |
| `row_027_meta_samples_hash_is_not_a_prefix_of_the_archive` | Meta.samples_hash is not a prefix of the archive | Verifier | - | [SamplesHashMismatch] |
| `row_028_meta_intents_hash_is_not_a_prefix_of_the_intents_file` | Meta.intents_hash is not a prefix of the intents file | Verifier | - | [IntentsHashMismatch] |
| `row_029_meta_flat_says_flat_while_a_position_is_open` | Meta.flat says flat while a position is open | Verifier | - | [StateMismatch] |
| `row_030_breach_posted_late_with_the_old_sample_replayed_price` | breach posted late with the old sample (replayed price) | Verifier | - | [MissedBreach, FalseBreach] |
| `row_031_breach_built_from_the_worst_sample_of_each_market_never` | breach built from the worst sample of each market (never simultaneous) | Verifier | - | [FalseBreach] |
| `row_032_breach_omitted_the_trader_keeps_the_position` | breach omitted (the trader keeps the position) | Verifier | - | [MissedBreach] |
| `row_033_fail_omitted_at_the_static_floor` | fail omitted at the static floor | Verifier | - | [MissedFail] |
| `row_034_static_floor_fail_on_an_account_above_the_floor` | static-floor fail on an account above the floor | Both | FailStaticFloorNotReached (6097) | [FalseFail] |
| `row_035_quick_limit_fail_on_an_account_below_the_limit` | quick-limit fail on an account below the limit | Both | FailQuickNotExceeded (6098) | [FalseFail] |
| `row_036_pass_requested_and_confirmed_on_an_account_that_is_not` | pass requested and confirmed on an account that is not flat | Verifier | - | [PassInvalid, StateMismatch] |
| `row_037_slow_profitable_slice_booked_as_quick_half_credited` | slow profitable slice booked as quick (half credited) | Verifier | - | [QuickMismatch] |
| `row_038_close_names_another_entry_price_pnl_consistent_with_it` | close names another entry price (pnl consistent with it) | Verifier | - | [AvgEntryMismatch] |
| `row_039_fill_reports_a_wrong_average_entry` | fill reports a wrong average entry | Verifier | - | [AvgEntryMismatch] |
| `row_040_fill_reports_a_wrong_margin` | fill reports a wrong margin | Verifier | - | [MarginMismatch] |
| `row_041_trade_with_no_signed_intent_forged_trade` | trade with no signed intent (forged trade) | Verifier | - | [ForgedTrade] |
| `row_042_intent_signed_by_a_key_that_is_not_the_trader_s` | intent signed by a key that is not the trader's | Verifier | - | [BadIntentSignature] |
| `row_043_archive_the_price_of_the_proof_s_sample_altered` | archive: the price of the proof's sample altered | Verifier | - | [ArchiveMissing] |
| `row_044_archive_the_sample_behind_a_fill_dropped` | archive: the sample behind a fill dropped | Verifier | - | [ArchiveMissing] |
| `row_045_stop_loss_triggered_but_not_executed` | stop-loss triggered but not executed | Verifier | - | [TriggerMissed] |
| `row_046_limit_order_triggered_but_never_filled` | limit order triggered but never filled | Verifier | - | [TriggerMissed] |
| `row_047_stop_loss_close_with_no_trigger` | stop-loss close with no trigger | Verifier | - | [TriggerFalse] |
| `row_048_fill_on_a_held_account` | fill on a held account | Both | AccountHeld (6037) | [FillNotAllowed] |
| `row_049_fill_during_the_12_h_lock_after_a_breach` | fill during the 12 h lock after a breach | Both | AccountLocked (6005) | [FillNotAllowed] |
| `row_050_fill_on_an_account_that_already_failed` | fill on an account that already failed | Both | AccountNotActive (6004) | [StateMismatch] |
| `row_051_a_sample_that_caused_a_breach_dropped_from_the_archive` | a sample that caused a breach dropped from the archive, breach omitted (no second recorder) | Neither | - | - |
| `row_052_the_same_with_an_independent_second_recorder` | the same, with an independent second recorder | Verifier | - | [MissedBreach] |
| `row_053_market_order_executed_at_an_older_sample_inside_max_age` | market order executed at an older sample inside max_age (worse price) | Neither | - | - |
| `row_054_a_trader_s_market_order_ignored_no_fill_intent_on_file` | a trader's market order ignored (no fill, intent on file) | Neither | - | - |
| `row_055_pending_order_that_only_reduces_the_close_that_executes` | pending order that only reduces: the Close that executes it is missing | Verifier | - | [TriggerMissed] |
| `row_056_pending_order_that_only_reduces_its_close_at_a_price` | pending order that only reduces: its Close at a price altered by one unit | Both | EventPriceMismatch (6055) | [PriceMismatch] |
| `row_057_close_under_the_nonce_of_a_pending_order_that_no_sample` | Close under the nonce of a pending order that no sample triggers | Verifier | - | [TriggerFalse] |
| `row_058_close_under_the_nonce_of_a_set_sl_tp` | Close under the nonce of a set_sl_tp | Verifier | - | [ForgedTrade] |
| `row_059_commission_fee_posted_after_the_slice_that_overshoots` | commission fee posted after the slice that overshoots the quick limit | Both | QuickLimitExceeded (6018) | [FeeMismatch] |
| `row_060_flip_that_drops_its_opening_remainder_although_its` | flip that drops its opening remainder although its closing slice did not overshoot | Verifier | - | [ForgedTrade] |
| `row_061_pending_order_the_account_could_not_margin_is_filled` | pending order the account could not margin is filled anyway | Verifier | - | [PendingFillWithoutMargin] |
| `row_062_breach_posted_where_its_closes_end_at_or_below_the` | breach posted where its closes end at or below the static floor | Both | BreachBelowStaticFloor (6077) | [FalseBreach] |
| `row_063_fail_omitted_a_tightend_instead_where_a_breach_s_closes` | Fail omitted (a TightEnd instead) where a breach's closes end at or below the static floor | Both | MustReportFail (6089) | [MissedFail] |
| `row_064_breach_posted_where_its_closes_overshoot_the_quick_limit` | breach posted where its closes overshoot the quick limit | Both | QuickLimitExceeded (6018) | [FalseBreach] |
| `row_065_phase_advanced_to_another_phase_after_pass_finalized` | phase_advanced to another phase after pass_finalized | Verifier | - | [StateMismatch] |
| `row_066_phase_advanced_repeated_after_pass_finalized` | phase_advanced repeated after pass_finalized | Verifier | - | [StateMismatch] |
| `row_067_phase_advanced_with_no_pass_line_before_it` | phase_advanced with no pass line before it | Verifier | - | [StateMismatch] |
| `row_068_a_market_fill_the_account_cannot_margin` | a market fill the account cannot margin | Verifier | - | [FillWithoutMargin] |
| `row_069_a_market_fill_above_the_market_s_notional_cap` | a market fill above the market's notional cap | Verifier | - | [FillNotAllowed] |
| `row_070_a_market_fill_in_a_market_the_account_s_rule_set_does` | a market fill in a market the account's rule set does not price | Both | MarketUnpriced (6109) | [FillNotAllowed] |
| `row_071_the_fill_of_a_triggered_pending_order_in_a_market_the` | the fill of a triggered pending order in a market the rule set does not price | Both | MarketUnpriced (6109) | [FillNotAllowed] |
| `row_072_a_triggered_pending_order_in_an_unpriced_market_that` | a triggered pending order in an unpriced market that the evaluator does not fill (the rule itself) | Neither | - | - |

### Row to test (4 chain rows, WHO = Program)

| test function | row | WHO | program |
|---|---|---|---|
| `chain_row_01_confirm_pass_with_no_pass_requested` | confirm_pass with no pass requested | Program | NoPendingPass (6022) |
| `chain_row_02_post_journal_while_a_pass_is_confirmed_the_journal_is` | post_journal while a pass is confirmed (the journal is frozen) | Program | PassPending (6019) |
| `chain_row_03_request_advance_below_the_target_b_1_08_s` | request_advance below the target (b < 1.08 S) | Program | TargetNotReached (6021) |
| `chain_row_04_finalize_pass_before_the_delay_has_elapsed` | finalize_pass before the delay has elapsed | Program | PassDelayNotElapsed (6023) |

### Rows without an individually named test

* Batch/log rows: **0**. Chain rows: **0**.
* Reset (M8) rows: **11 rows without an individually named test.** `tests/trust_reset.rs` (not part of this task, not edited) asserts all 11 inside ONE table-driven test, `reset_trust_rows` (plus `reset_trust_rows_are_documented`). The rows, in the order of the document and of that file:

| # | Reset row |
|---|---|
| 1 | reset: a price below the registry's tier price paid to the vault (a fake reset price) |
| 2 | reset: the registry replaced by a copy with cheaper tiers at another address |
| 3 | reset: an account that has not failed (an Active account of a high phase) |
| 4 | reset: a second reset of the same failed record |
| 5 | reset: a second reset of the same failed record with the local mark wiped (the vault is the backstop) |
| 6 | reset: by a banned wallet |
| 7 | reset: a violation-failed account after the wallet was unbanned (DESIGN 2.9: no reset) |
| 8 | reset: while the sector is paused |
| 9 | reset: while the vault pauses this product |
| 10 | reset into a higher phase: the new account copies the failed phase (there is no phase argument) |
| 11 | reset: an evaluator log that opens the new account in a higher phase than the chain's account |

Whoever owns `trust_reset.rs` can apply the same macro pattern; `RESET_ROWS` and `reset_rows_listing_matches_document_and_source` already pin the names, so the conversion cannot silently lose a row.

## 2. The dishonest-evaluator table (`adversarial_evaluator.rs`)

Method: for each attack, an honest history is built with the reference evaluator, then edited the way a dishonest evaluator that holds the key would edit it (it keeps its hash chain and its Meta commitments consistent, so `T::fix()` re-chains and re-hashes). The edited batches go, in order, to the REAL program and vault in LiteSVM until the first refusal; the same data (as if all had been accepted) goes through the public verifier. The scaffolding is copied from `trust_matrix.rs` because that file's helpers are private.

Differences from `trust_matrix.rs` (stricter):

* the verifier's finding classes are compared for EQUALITY (sorted, distinct), not "contains";
* on a refusal the digest of the whole world (trading account, config, wallet, rule sets, markets, vault records, every tracked token account, trader lamports) must be byte-identical to the digest taken just before the refused transaction;
* a case that claims NEITHER must name a row that is in the NEITHER table AND under "Residual trust" of the document; a case that names a trust-matrix row must agree with the WHO the document gives that row.

Vocabulary: PROGRAM, VERIFIER, BOTH, NEITHER as in the trust matrix. GAP (new, only used once): accepted by the program and VALID for the verifier although the DESIGN says it must be visible, and not on the NEITHER list. That is a FINDING.

### Cases (29 table cases, each its own test; 3 more tests around them)

| id | family | attack | trust-matrix row | outcome | exact error / findings | test |
|---|---|---|---|---|---|---|
| DE-01a | fabricated pass | exit price of the close moved up 0.009, pnl and credit recomputed, so the journal reaches 1.08 S | close exit price altered (pnl recomputed) | BOTH | EventPriceMismatch (6055) / PriceMismatch; afterwards `request_pass` on the real account is still refused with TargetNotReached (6021) | `de_01a_fabricated_pass_inflated_exit_price` |
| DE-01b | fabricated pass | REAL prices and proofs, Close quantity x20 (+600 becomes +12,000), pass requested and confirmed | none (new) | VERIFIER | PassInvalid, PositionMismatch, StateMismatch. The program accepts: b reaches the target and `pass_pending = 2` (confirmed) | `de_01b_fabricated_pass_inflated_close_quantity` |
| DE-01c | fabricated pass | `pass_requested` / `pass_confirmed` lines forged into the log of an account below the target | none (new) | VERIFIER | PassInvalid (the chain never emitted these) | `de_01c_fabricated_pass_forged_log_lines` |
| DE-01d | fabricated pass | Meta says flat while a position is open, pass requested and confirmed | pass requested and confirmed on an account that is not flat | VERIFIER | PassInvalid, StateMismatch | `de_01d_fabricated_pass_not_flat` |
| DE-01e | fabricated pass | confirm_pass with no pass requested, on an account that IS above the target | chain row "confirm_pass with no pass requested" | PROGRAM | NoPendingPass (6022), digest unchanged | `de_01e_confirm_pass_without_a_request_is_refused_and_changes_nothing` |
| DE-01f | fabricated pass | REAL exit, entry price of the Close 0.009 lower (pnl consistent): +600 becomes +9,600, pass confirmed | close names another entry price (pnl consistent with it) | VERIFIER | AvgEntryMismatch, PassInvalid, PnlMismatch. The program accepts, b reaches the target, `pass_pending = 2` | `de_01f_fabricated_pass_lowered_entry_price` |
| DE-02a | omitted breach | breach batch replaced by a heartbeat, the position stays | breach omitted (the trader keeps the position) | VERIFIER | MissedBreach | `de_02a_omitted_breach` |
| DE-02b | omitted fail | report_fail at the static floor replaced by a heartbeat | fail omitted at the static floor | VERIFIER | MissedFail | `de_02b_omitted_fail` |
| DE-02c | omitted breach | the crash sample dropped from the archive and the breach omitted, consistently, no second recorder | a sample that caused a breach dropped from the archive, breach omitted (no second recorder) | NEITHER (row 1 of the NEITHER list) | none, accepted and VALID | `de_02c_omitted_breach_sample_dropped_no_second_recorder` |
| DE-02d | omitted breach | the same, but an independent second recorder holds the sample | the same, with an independent second recorder | VERIFIER | MissedBreach | `de_02d_omitted_breach_sample_dropped_with_second_recorder` |
| DE-03a | replayed old price | breach posted 30 s late with the old sample (inside max_age) | breach posted late with the old sample (replayed price) | VERIFIER | FalseBreach, MissedBreach | `de_03a_replayed_price_late_breach` |
| DE-03b | replayed old price | market order filled at a 20 s old sample (max_age 60 s), worse for the trader | market order executed at an older sample inside max_age (worse price) | NEITHER (row 2 of the NEITHER list) | none | `de_03b_replayed_price_older_sample_inside_max_age` |
| DE-03c | replayed old price | proof EXACTLY max_age_secs (60 s) older than the fill | none (boundary of the same NEITHER row) | NEITHER (same row as DE-03b) | none: the boundary is inclusive, accepted and VALID | `de_03c_replayed_price_exactly_max_age` |
| DE-03d | replayed old price | proof max_age_secs + 1 (61 s) older | proof older than the market's max_age | BOTH | ProofTooOld (6049) / ProofAge | `de_03d_replayed_price_one_second_beyond_max_age` |
| DE-04a | mass fail | report_fail(StaticFloor) on an honest account with its true balance | static-floor fail on an account above the floor | BOTH | FailStaticFloorNotReached (6097) / FalseFail | `de_04a_mass_fail_static_floor_on_honest_account` |
| DE-04b | mass fail | report_fail(PhaseQuick) on an honest account below the limit | quick-limit fail on an account below the limit | BOTH | FailQuickNotExceeded (6098) / FalseFail | `de_04b_mass_fail_quick_limit_on_honest_account` |
| DE-04c | mass fail | report_fail(StaticFloor) with a fabricated b_after = 0 | none (new) | BOTH | FailBalanceMismatch (6096) / FalseFail, FloorMismatch | `de_04c_mass_fail_fabricated_balance` |
| DE-04d | mass fail | what the evaluator MUST fabricate: the real lows of two markets that never existed together, closed in one report_fail(StaticFloor) (b_after about 90,000 against a floor of 92,000, honest minimum equity about 95,000) | the Breach version is row 31 (VERIFIER) | **GAP, FINDING TMX-1** | program accepts, account Failed (status 1); verifier VALID | `de_04d_mass_fail_non_simultaneous_lows_gap_tmx_1` (pins today) and `tmx_1_verifier_must_flag_a_fail_built_from_non_simultaneous_lows` (`#[ignore]`, asserts the DESIGN behaviour) |
| DE-04e | mass fail | 9 honest Active accounts x 2 reasons = 18 wrongful fails (breach, quick, night, pass_open, limit_sl, dips, slow, win, stale) | rows 34 and 35 on 9 histories | BOTH x 18 | FailStaticFloorNotReached / FailQuickNotExceeded; refused batch is the fabricated one only; digest unchanged; account still Active; FalseFail (plus StateMismatch where the account still holds a position and the fake batch says flat) | `de_04e_mass_fail_every_honest_account_is_refused_by_the_program_and_flagged_by_the_verifier` |
| DE-05a | double fill | the Fill of a triggered limit order posted twice in one batch | none (new) | VERIFIER | ForgedTrade, MarginMismatch, StateMismatch, TriggerFalse | `de_05a_double_fill_limit_same_batch` |
| DE-05b | double fill | the same limit order filled again in a later batch at a later sample, same intent id | none (new) | VERIFIER | ForgedTrade, TriggerFalse | `de_05b_double_fill_limit_later_batch` |
| DE-05c | double fill | a market order and its commission booked twice | none (new) | VERIFIER | ForgedTrade, MarginMismatch, MissedTightStart, StateMismatch | `de_05c_double_fill_market_order_and_commission` |
| DE-06a | wrong side of the spread | LONG fill priced at the bid | fill price altered by one unit (same check, bigger alteration) | BOTH | EventPriceMismatch (6055) / PriceMismatch | `de_06a_long_fill_at_the_bid` |
| DE-06b | wrong side of the spread | Close of a long priced at the ask, pnl recomputed | close exit price altered (pnl recomputed) | BOTH | EventPriceMismatch (6055) / FeeMismatch, PriceMismatch | `de_06b_close_at_the_wrong_side` |
| DE-06c | wrong side of the spread | signed order is a BUY, the evaluator books a SELL at the correct bid price (every number right for the side it chose) | none (new) | VERIFIER | ForgedTrade | `de_06c_opposite_side_to_the_signed_order` |
| DE-07a | skipped rollover fee | holding fee at 22:00 UTC left out | holding fee omitted at the rollover | VERIFIER | FeeMismatch | `de_07a_skipped_holding_fee` |
| DE-07b | skipped rollover fee | holding fee booked with amount 0 | none (new) | VERIFIER | FeeMismatch | `de_07b_zero_holding_fee` |
| DE-07c | skipped rollover fee | holding fee one micro-USD too low | holding fee one micro-USD too low | VERIFIER | FeeMismatch | `de_07c_holding_fee_one_micro_low` |
| DE-08a | fee applied twice | holding fee booked twice | none (new) | VERIFIER | FeeMismatch | `de_08a_holding_fee_twice` |
| DE-08b | fee applied twice | commission booked twice | none (new) | VERIFIER | FeeMismatch | `de_08b_commission_twice` |
| DE-08c | fee applied twice | commission one micro-USD too high | commission fee one micro-USD too high | VERIFIER | FeeMismatch | `de_08c_commission_one_micro_high` |

`dishonest_evaluator_table_is_complete` checks: one test per case id, unique ids, every case has one of the four outcomes (or GAP), every family of the prompt has at least one case, every NEITHER case names a NEITHER row of the document.

### Counts

* Table cases: **29** (BOTH 7, VERIFIER 18, NEITHER 3, GAP 1, PROGRAM 0 in the table). Plus DE-01e (PROGRAM, chain level) and DE-04e (18 sub-attacks, all BOTH) as separate tests, plus the ignored TMX-1 test, plus the completeness test: **33 tests in the file, 32 pass, 1 ignored (FINDING)**.
* Dishonest behaviours exercised in all: 29 + 1 + 18 = 48 attack instances (29 table cases, DE-01e, 18 mass-fail instances). Of the 29 table cases, 17 name a trust-matrix row of the same class and the test checks that the document gives that row the same WHO; 12 are not rows of the matrix (DE-01b, 01c, 03c, 04c, 04d, 05a to 05c, 06c, 07b, 08a, 08b).
* Of the nine families (fabricated pass, omitted breach, omitted fail, replayed old price, mass fail, double fill, wrong side of the spread, skipped rollover fee, fee applied twice): all covered. "Fabricates a pass" has six cases because it is the attack with the largest payout.

### Reading the table honestly

* The program stops very little of this: only what it can recompute from the event itself (price against proof, pnl, credit, fee range, fail conditions against the stored b, proof age, order of time). Everything else (quantities, entry prices, the position book, intents, fees, simultaneous equity, fills of pending orders) is the verifier's. 18 of 29 cases are VERIFIER-only: the safety of those rests on someone running `setl8-verify watch` inside the 2-day window (DESIGN 13.3).
* DE-01b and DE-01f show the weight of that sentence: with real prices and real proofs an evaluator can reach 1.08 S and get the pass CONFIRMED on the chain (`pass_pending = 2`); what remains is the 2-day delay, the veto of both admins, the daily cap and the vault (R-1). Neither test claims the pass was stopped by the program; it was not.
* The wrongful fail has no delay and no veto (DE-04d): `report_fail` calls the vault at once. For wrongful fails that the program cannot recompute (DE-04d) the only protection is the verifier, and that is where TMX-1 is.
* The 4 NEITHER rows: DE-02c is row "a sample that caused a breach dropped ...", DE-03b and DE-03c are row "market order executed at an older sample inside max_age". The rows "a trader's market order ignored" and "a triggered pending order in an unpriced market that the evaluator does not fill (the rule itself)" are censorship and a rule, not one of the dishonest behaviours of this table; they are covered by `row_054_...` and `row_072_...`.

## 3. Findings

### TMX-1 (medium): a Fail built from non-simultaneous lows is accepted by the program and VALID for the verifier

Description. A compromised evaluator can fail an honest account that holds positions in two or more markets, without the account's equity ever having been at or below the static floor. It posts `report_fail(StaticFloor)` with Close events (reason Breach) at the real LOW of each market (each a real sample, each inside `max_age_secs` of the batch; every pnl, credit and `b_after` arithmetically true) and a `Fail` whose `b_after` is at or below 0.92 S. The program re-derives only `b` and the floor from the claimed events, so it accepts (the account becomes Failed, status 1, and the vault is flagged at once: there is no 2-day delay and no veto). The verifier answers VALID: `walk_floor` (crates/verifier/src/walk.rs, "a Fail is justified by the replayed state (stage 1); the walk only reports a MISSED one") removes any expectation for a Fail and returns, and `on_fail` (crates/verifier/src/replay.rs) justifies the Fail from the replayed closed balance alone. For a Breach built the same way (trust-matrix row "breach built from the worst sample of each market (never simultaneous)") the verifier does answer FalseBreach, so the verifier does know how to tell. DESIGN 13.2 A-18 and the stolen-key row of 1.3 call a mass fail "possible and visible"; for this vector it is not visible to the verifier, only to a human reading the journal and noticing Closes of reason Breach with no Breach event.

Reproduction in the test: scenario `sc_dips` (two markets, 1 M EUR long each, each dips 0.5% alone, honest minimum equity about $95,000, honest history accepted by both); `dips_fail` closes the two lows together at +50 s, `b_after` is about $90,000. Program: accepts, account Failed. Verifier: no finding.

Test: `de_04d_mass_fail_non_simultaneous_lows_gap_tmx_1` pins today's behaviour (accepted, Failed, no finding); `tmx_1_verifier_must_flag_a_fail_built_from_non_simultaneous_lows` is `#[ignore]` and asserts the DESIGN behaviour (FalseFail or FalseBreach). I ran it with `--ignored`: it fails with "the verifier must flag a Fail whose Closes were never justified ...; it said: []", which is the stated reason.

Fix class: plain implementation error in the VERIFIER (crates/verifier). No DESIGN change and no program change. Proposed fix: in `walk_floor` for `FloorKind::Fail`, when the walk has no expectation of a Fail (or of the Breach that precedes it) at that second, add FalseFail, as the Breach branch already does with FalseBreach; take care of the case where the floor is reached by closes the trader asked for (then the equity was at or below the floor one sample earlier and the walk has the expectation there). Not fixed here (rule: no quiet fixes; this task may not touch production code).

Limits of the finding. Needs a stolen evaluator key (or a malicious operator) and an account whose open positions in different markets have real lows, inside one `max_age_secs` window, that add up to the 8% distance of the floor. The harness has one account per `LiteChain`, so "mass" is shown for 1 account by DE-04d and 18 refused attempts by DE-04e; the argument that it repeats per account is by construction, not measured.

Other disagreements looked for and not found: for every other case in the table the verifier's findings match the trust matrix or are stronger (DE-01b, DE-05a to DE-05c are caught with more classes than a minimal story would need).

## 4. What I could not test, and why

* A fabricated `pass_requested` ON CHAIN: `request_pass` is a trader instruction signed by the trader; the evaluator cannot emit it. Only the forged-log version is possible (DE-01c). That the evaluator key cannot call `request_pass` is a who-may-call cell (`who_may_call.rs`), not repeated here.
* "Mass" over many accounts in one run: `LiteChain` holds one account. Done as 9 distinct honest histories x 2 reasons (DE-04e), each a fresh world.
* The vault consequence of a wrongful fail was checked through the stored account (`status == 1`) and the world digest, not by reading the vault record's flag separately.
* The trader's wallet losses in money terms for a wrongful fail (reset price paid again, OPEN_QUESTIONS #9) are not modelled here.
* Cases beyond what the evaluator scenarios can express (e.g. a market with a closing session AND a replayed price) are covered by the existing rows only.
* I did not run cargo-mutants on these tests; the sanity check that each refusal test would fail without its control is by reading the program source path of each error, not by mutating it (no production edits allowed).

## 5. Commands

```
cd tests-rs
cargo test --offline --test trust_matrix            # 87 passed
cargo test --offline --test adversarial_evaluator   # 32 passed, 1 ignored
cargo test --offline --test adversarial_evaluator -- --ignored   # TMX-1 fails, as intended
cargo clippy --offline --test trust_matrix --test adversarial_evaluator -- -D warnings   # clean
```
