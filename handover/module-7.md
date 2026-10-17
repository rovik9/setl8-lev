# Module M7: fail, hold, violation, activity, release, close (DONE, verified 2026-10-11)

`report_fail`, `hold_account`, `release_hold`, `fail_for_violation`, `touch_activity`, `release_account`, `close_account`, the held-account rule in `ban_wallet`, and the `post_journal` static-floor hardening. Builds on M5a/M5b/M6. Tests run against the real vault fixture (HEAD 591ced6 when written; rebuilt from core commit 9a5c179 on 2026-10-13, see handover/deploy-checklist.md and decision 51).

## RESOLVE answers
**(a) `report_fail` batch shape.** `events..., Fail, Meta`: exactly one `Fail`, it is the event right before the final `Meta` (`FailMissing`, `FailTwice`, `FailNotBeforeMeta`), `Meta.flat == 1` for both reasons (`FailNotFlat`; a failed account's positions are void), `Fail.b_after == b` after the batch (`FailBalanceMismatch`), `Fail.reason` equals the argument (`FailReasonMismatch`; an argument other than 0/1 is `FailReasonInvalid`), and the `Fail` carries a valid proof like the other floating events. StaticFloor: closed balance `b <= static floor` with the current latch (inclusive; latch set inside the batch counts), refused with `QuickLimitNeedsPhaseQuick` if the quick tally is also over the limit. PhaseQuick: `q_tally > 0.028 S` strictly (`FailQuickNotExceeded`). A quick overshoot reached inside the batch is APPLIED (no `QuickLimitExceeded`), and from then on only the `Fail` and the `Meta` may follow. The event loop is shared: `post_journal.rs::run_batch(.., Mode::Post | Mode::ReportFail(reason))` returns an `Outcome` (state, hash, `EventLog`s) that each handler stores and emits; the M5b suite is unchanged and green.
**(b) `terminal_at`.** Stored in `pass_proposed_at` (meaningless once nothing can be pending). `TradingAccount::terminal_at()` reads it. Every transition into Failed, Abandoned or Graduated goes through `pass_common::end_account(acct, wallet, status, now)` which sets status, `terminal_at = now`, `pass_pending = 0`, clears an active hold with `held_count - 1`, and frees the wallet slot. M6's Abandoned (request_advance, finalize_pass) and Graduated (finalize_pass) paths were changed to use it, and their tests now assert `terminal_at`. The 207-byte layout is unchanged. DESIGN 5.1 got a note.
**(c) `ban_wallet` and held accounts.** `WalletProfile` has a new `held_count u8` in the first spare byte (offset 51; `pad` is now `[u8;4]`; size stays 56, `wallet.rs` layout test and all M1b tests green). `hold_account` +1, `release_hold` -1, `fail_for_violation` -1, and any other path that ends a held account (`report_fail`, an Abandoned mark) -1 through `end_account`. `ban_wallet` is refused with `WalletHasHeldAccount` while `held_count > 0`. `fail_for_violation` itself bans its own wallet (not through `ban_wallet`). A held account CAN be failed by `report_fail` (a floor fail is a floor fail); the hold is cleared with the count.
**(d) `post_journal` hardening.** At the end of a `post_journal` batch: if `Meta.flat == 1` and the resulting `b <= static floor` (0.92 S, or 0.96 S once latched, inclusive) the batch is rejected with `MustReportFail` (6089). Not when `flat == 0`. Only the final state is checked (going below and back above inside a batch is fine). Implemented in the shared `run_batch` for `Mode::Post` only. The M5b and 4.9 tests stayed green without change.

## Accounts in order (`#[event_cpi]` instructions end with `event_authority`, `program`; all big accounts are boxed)
- **report_fail(batch_bytes, reason)** (11 + markets): 1 `evaluator` [S, W] · 2 `trading_account` [W] · 3 `config` · 4 `ruleset` (PDA of the account's `ruleset_version`) · 5 `wallet` [W] · 6 `sector_authority` (PDA) · 7 `vault_program` (= `config.vault_program`) · 8 `product_registry` (read-only, vault) · 9 `vault_trader_state` [W] (vault) · 10 `event_authority` · 11 `program` · then the referenced `Market` accounts (remaining accounts, at most 8).
- **fail_for_violation(evidence_hash)** (11): 1 `config` · 2 `admin_a` [S] · 3 `admin_b` [S] · 4 `trading_account` [W] · 5 `wallet` [W] · 6 `sector_authority` · 7 `vault_program` · 8 `product_registry` · 9 `vault_trader_state` [W] · 10 `event_authority` · 11 `program`.
- **hold_account(evidence_hash)** (6): `evaluator` [S] · `trading_account` [W] · `wallet` [W] · `config` · `event_authority` · `program`.
- **release_hold** (7): `config` · `admin_a` [S] · `admin_b` [S] · `trading_account` [W] · `wallet` [W] · `event_authority` · `program`.
- **touch_activity** (10): `evaluator` [S] · `trading_account` [W] · `wallet` [W] · `config` · `sector_authority` · `vault_program` · `product_registry` · `vault_trader_state` [W] · `event_authority` · `program`.
- **release_account** (10): same as `touch_activity` with `trader` [S] first (the account's owner).
- **close_account** (2): `trader` [S, W] · `trading_account` [W, closed to `trader`].
Vault CPIs are signed by `sector_authority` (`invoke_signed`): `flag_trader_failed` (authority, registry, trader_state) and `record_activity` (return data read exactly as in M6).

## Behaviour notes
- `report_fail`: signer, reason argument, Active, `pass_pending != 2`, decode, shared loop, then store (engine state, hash, seq, time), `flat = 1`, `end_account(Failed)`, `flag_trader_failed`, `AccountFailed` (`emit_cpi!`) after the `EventLog`s. `pass_pending == 1` is cleared. Does not touch `wallet.banned`. Works while the sector and the vault product are paused.
- `touch_activity` / `release_account` share `activity::push_activity`: Active only; `now - last_activity_pushed < 86,000` is a harmless success without a vault call (same for the trader's `release_account`); otherwise the vault's `TraderState` is checked (owner = vault, address = the vault PDA of this challenge, else `InvalidVaultAccount`) and, if the vault already holds it as Abandoned, the local account is marked Abandoned without a CPI; otherwise `record_activity`: `Recorded` stores `last_activity_pushed`, `Throttled` stores nothing, `Abandoned` marks the account (`terminal_at`, slot freed, pending cleared, hold cleared) and emits `AccountAbandoned`.
- `close_account`: terminal status (`AccountNotTerminal`) and `now >= terminal_at + 604,800` (`CloseTooEarly`, inclusive); Anchor `close = trader` returns all lamports. `challenge_id` only increases, so a new `open_account` takes the next id and the closed address is never reused.
- `hold_account` works in every `pass_pending` state; `release_hold` does not write `pass_proposed_at` (the delay clock of a pending pass keeps running).

## Errors (new: 6089 and up; reused: EvidenceMissing 6036, NotHeld 6038, CloseTooEarly 6030, PassPending 6019, QuickLimitExceeded 6018, AccountNotActive 6004, Unauthorized 6000, InvalidVaultProgram 6041)
| code | name | instruction |
|---|---|---|
| 6089 | MustReportFail | post_journal (RESOLVE d) |
| 6090 | FailReasonInvalid | report_fail |
| 6091 | FailReasonMismatch | report_fail |
| 6092 | FailMissing | report_fail |
| 6093 | FailTwice | report_fail |
| 6094 | FailNotBeforeMeta | report_fail |
| 6095 | FailNotFlat | report_fail |
| 6096 | FailBalanceMismatch | report_fail |
| 6097 | FailStaticFloorNotReached | report_fail |
| 6098 | FailQuickNotExceeded | report_fail |
| 6099 | QuickLimitNeedsPhaseQuick | report_fail |
| 6100 | AlreadyHeld | hold_account |
| 6101 | WalletHasHeldAccount | ban_wallet |
| 6102 | AccountNotTerminal | close_account |
| 6103 | InvalidVaultAccount | touch_activity, release_account |

## Tests
`tests-rs/tests/lifecycle.rs` (43) + harness `tests-rs/tests/common/lifecycle.rs`; `cd tests-rs && cargo test`: 270 tests (227 before + 43), program lib unit tests 24 (unchanged count), engine and journal-types untouched (no mutants to rerun). M6 tests gained `terminal_at` assertions. Every failure test asserts the exact error and an unchanged digest.
- report_fail (15): rf_static_floor_boundary_is_inclusive, rf_latched_floor_is_096_s_and_an_unlatched_account_keeps_092_s, rf_latch_reached_inside_the_batch_counts, rf_not_flat_wrong_reason_and_balance_mismatch_are_refused, rf_batch_shape_fail_must_be_right_before_meta_and_unique, rf_wrong_signer_and_inactive_accounts_are_refused, rf_the_fail_event_needs_a_valid_proof, rf_quick_tally_boundary_is_strict, rf_quick_overshoot_inside_the_batch_forces_the_fail, rf_each_reason_needs_its_own_condition, rf_effects_status_terminal_at_vault_wallet_events_and_chain, rf_is_refused_in_state_2_allowed_in_state_1_and_a_failed_account_is_dead, rf_frees_a_slot_so_the_wallet_can_open_again_up_to_the_cap_of_five, rf_works_while_the_sector_and_the_vault_product_are_paused, rf_on_a_held_account_clears_the_hold_and_the_wallet_count
- post_journal hardening (3): pj_a_flat_batch_at_or_below_the_static_floor_must_use_report_fail, pj_the_latched_floor_is_096_s, pj_not_flat_may_end_below_the_floor_because_open_profit_can_keep_equity_up
- hold / release / violation (9): hold_preconditions_each_violated_alone, hold_works_with_a_pending_pass_in_both_states_and_changes_nothing_else, a_held_account_refuses_fills_and_passes_but_allows_closes_and_fees, a_hold_stops_finalize_and_the_delay_clock_keeps_running_across_the_release, release_hold_needs_both_admins_and_a_held_account, fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault, fail_for_violation_needs_a_held_account_and_works_while_paused, held_count_bookkeeping_across_every_path_and_ban_wallet, a_held_account_that_is_abandoned_releases_its_hold_count
- touch_activity / release_account (8): touch_boundary_is_86000_seconds_skipped_at_85999_pushed_at_86000, release_account_has_the_same_boundary_and_a_skipped_call_is_a_harmless_success, a_throttled_vault_answer_does_not_move_last_activity_pushed, activity_signers_evaluator_for_touch_trader_for_release, abandoned_through_the_vaults_idle_rule_marks_the_account_frees_the_slot_and_clears_a_pending_pass, a_challenge_already_abandoned_by_the_permissionless_vault_call_is_learned_locally, activity_on_terminal_or_non_active_accounts_is_refused, activity_with_a_wrong_vault_trader_state_is_refused
- close_account (3): close_is_refused_for_an_active_account_even_when_held, close_boundary_is_terminal_at_plus_seven_days_for_every_terminal_status, close_other_signer_refused_closing_twice_impossible_and_the_next_open_gets_the_next_id
- attacks (4): attack_the_evaluator_cannot_release_a_hold_or_fail_for_violation, attack_one_admin_alone_cannot_use_any_both_admin_instruction, attack_a_trader_cannot_report_fail_or_hold, attack_fake_wallet_and_vault_accounts_in_report_fail_and_fail_for_violation (17 account swaps, all refused and unchanged)
- compute units (1): compute_units_per_instruction

## Compute units (whole transaction, numbers only)
report_fail with 2 events 76,086 · report_fail with 8 events and 3 proofs 158,394 · hold_account 24,632 · release_hold 24,602 · fail_for_violation 44,032 · touch_activity skipped 21,995 · touch_activity with the vault call 37,147 · release_account with the vault call 37,147 · close_account 3,339. All fit the 200k default (the 8-event report_fail needs no budget instruction). Stack: no frame over 4096 bytes (default and localnet builds).

## Hand-applied mutations: 55 applied, 54 killed, 1 equivalent survivor (driver: patch, `cargo test --lib`, rebuild the localnet `.so`, run lifecycle, pass_flow, post_journal, wallet, open_account)
| mutation | killed by |
|---|---|
| static floor <= -> < | KILLED by 10: compute_units_per_instruction, rf_batch_shape_fail_must_be_right_before_meta_and_unique, rf_effects_status_terminal_at_vault_wallet_events_and_chain |
| static floor 0.92 -> 0.96 | KILLED(unit) by: state::trading_account::tests::static_floor_for_latch_zero_and_one |
| latch ignored in report_fail | KILLED by 2: rf_latch_reached_inside_the_batch_counts, rf_latched_floor_is_096_s_and_an_unlatched_account_keeps_092_s |
| flat check removed (FailNotFlat) | KILLED by 1: rf_not_flat_wrong_reason_and_balance_mismatch_are_refused |
| quick > -> >= | KILLED(unit) by: instructions::pass_common::tests::pass_condition_boundaries |
| PhaseQuick unchecked | KILLED by 2: rf_each_reason_needs_its_own_condition, rf_quick_tally_boundary_is_strict |
| StaticFloor unjustified | KILLED by 3: rf_latch_reached_inside_the_batch_counts, rf_latched_floor_is_096_s_and_an_unlatched_account_keeps_092_s, rf_static_floor_boundary_is_inclusive |
| quick overshoot with StaticFloor allowed | KILLED by 2: rf_each_reason_needs_its_own_condition, rf_quick_overshoot_inside_the_batch_forces_the_fail |
| b_after unchecked | KILLED by 1: rf_not_flat_wrong_reason_and_balance_mismatch_are_refused |
| fail reason argument unchecked | KILLED by 1: rf_not_flat_wrong_reason_and_balance_mismatch_are_refused |
| Fail proof unchecked | KILLED by 1: rf_the_fail_event_needs_a_valid_proof |
| Fail-before-Meta check removed | KILLED by 1: rf_batch_shape_fail_must_be_right_before_meta_and_unique |
| two Fails allowed | KILLED by 1: rf_batch_shape_fail_must_be_right_before_meta_and_unique |
| hardening off (flat) | KILLED by 2: pj_a_flat_batch_at_or_below_the_static_floor_must_use_report_fail, pj_the_latched_floor_is_096_s |
| hardening b > -> >= | KILLED by 2: pj_a_flat_batch_at_or_below_the_static_floor_must_use_report_fail, pj_the_latched_floor_is_096_s |
| hardening ignores flat (always) | KILLED by 2: i6_fee_amount_beyond_i64_is_refused, pj_not_flat_may_end_below_the_floor_because_open_profit_can_keep_equity_up |
| events after a quick overshoot allowed | KILLED by 1: rf_quick_overshoot_inside_the_batch_forces_the_fail |
| I-8 off in post_journal | KILLED by 3: i8_quick_tally_accumulates_across_batches_and_inside_a_batch, i8_quick_tally_at_28_percent_is_allowed_and_one_micro_more_is_refused, rf_quick_overshoot_inside_the_batch_forces_the_fail |
| report_fail: evaluator check removed | KILLED by 2: attack_a_trader_cannot_report_fail_or_hold, rf_wrong_signer_and_inactive_accounts_are_refused |
| report_fail allowed in state 2 | KILLED by 1: rf_is_refused_in_state_2_allowed_in_state_1_and_a_failed_account_is_dead |
| report_fail: Active check removed | KILLED by 2: rf_is_refused_in_state_2_allowed_in_state_1_and_a_failed_account_is_dead, rf_wrong_signer_and_inactive_accounts_are_refused |
| report_fail: reason argument unvalidated | KILLED by 1: rf_not_flat_wrong_reason_and_balance_mismatch_are_refused |
| flag_trader_failed CPI skipped | KILLED by 4: attack_fake_wallet_and_vault_accounts_in_report_fail_and_fail_for_violation, fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault, rf_effects_status_terminal_at_vault_wallet_events_and_chain |
| report_fail: flat not set | SURVIVED |
| active_count not decremented | KILLED(unit) by: instructions::pass_common::tests::wallet_slot_cannot_underflow |
| active_count decremented twice | KILLED by 24: a_challenge_already_abandoned_by_the_permissionless_vault_call_is_learned_locally, a_held_account_that_is_abandoned_releases_its_hold_count, abandoned_through_the_vaults_idle_rule_marks_the_account_frees_the_slot_and_clears_a_pending_pass |
| terminal_at not set | KILLED by 6: abandoned_through_the_vaults_idle_rule_marks_the_account_frees_the_slot_and_clears_a_pending_pass, fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault, finalize_abandoned_by_the_vault_changes_neither_tally_nor_window |
| pass_pending not cleared on end | KILLED by 5: abandoned_through_the_vaults_idle_rule_marks_the_account_frees_the_slot_and_clears_a_pending_pass, fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault, finalize_abandoned_by_the_vault_changes_neither_tally_nor_window |
| held_count not maintained on end | KILLED by 4: a_held_account_that_is_abandoned_releases_its_hold_count, fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault, held_count_bookkeeping_across_every_path_and_ban_wallet |
| hold allowed with a zero hash | KILLED by 1: hold_preconditions_each_violated_alone |
| hold: held_count not incremented | KILLED by 10: a_held_account_that_is_abandoned_releases_its_hold_count, a_hold_stops_finalize_and_the_delay_clock_keeps_running_across_the_release, attack_fake_wallet_and_vault_accounts_in_report_fail_and_fail_for_violation |
| double hold allowed | KILLED by 1: hold_preconditions_each_violated_alone |
| hold: evaluator check removed | KILLED by 2: attack_a_trader_cannot_report_fail_or_hold, hold_preconditions_each_violated_alone |
| hold: Active check removed | KILLED by 1: hold_preconditions_each_violated_alone |
| release_hold: both-admin check removed | KILLED by 3: attack_one_admin_alone_cannot_use_any_both_admin_instruction, attack_the_evaluator_cannot_release_a_hold_or_fail_for_violation, release_hold_needs_both_admins_and_a_held_account |
| release_hold: NotHeld check removed | KILLED by 1: release_hold_needs_both_admins_and_a_held_account |
| release_hold: held_count not decremented | KILLED by 2: held_count_bookkeeping_across_every_path_and_ban_wallet, release_hold_needs_both_admins_and_a_held_account |
| violation: held check removed | KILLED by 1: fail_for_violation_needs_a_held_account_and_works_while_paused |
| violation: wallet not banned | KILLED by 2: fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault, held_count_bookkeeping_across_every_path_and_ban_wallet |
| violation: both-admin check removed | KILLED by 3: attack_one_admin_alone_cannot_use_any_both_admin_instruction, attack_the_evaluator_cannot_release_a_hold_or_fail_for_violation, fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault |
| violation: zero evidence allowed | KILLED by 1: fail_for_violation_clears_a_confirmed_pass_bans_the_wallet_and_fails_in_the_vault |
| throttle 86,000 -> 85,999 | KILLED by 2: release_account_has_the_same_boundary_and_a_skipped_call_is_a_harmless_success, touch_boundary_is_86000_seconds_skipped_at_85999_pushed_at_86000 |
| throttle < -> <= | KILLED by 2: release_account_has_the_same_boundary_and_a_skipped_call_is_a_harmless_success, touch_boundary_is_86000_seconds_skipped_at_85999_pushed_at_86000 |
| activity: Abandoned ignored | KILLED by 2: a_held_account_that_is_abandoned_releases_its_hold_count, abandoned_through_the_vaults_idle_rule_marks_the_account_frees_the_slot_and_clears_a_pending_pass |
| activity: vault-side Abandoned not learned | KILLED by 1: a_challenge_already_abandoned_by_the_permissionless_vault_call_is_learned_locally |
| activity: Recorded not stored | KILLED by 3: abandoned_through_the_vaults_idle_rule_marks_the_account_frees_the_slot_and_clears_a_pending_pass, release_account_has_the_same_boundary_and_a_skipped_call_is_a_harmless_success, touch_boundary_is_86000_seconds_skipped_at_85999_pushed_at_86000 |
| activity: Throttled stores | KILLED by 1: a_throttled_vault_answer_does_not_move_last_activity_pushed |
| touch: evaluator check removed | KILLED by 1: activity_signers_evaluator_for_touch_trader_for_release |
| release_account: trader constraint removed | KILLED by 1: activity_signers_evaluator_for_touch_trader_for_release |
| activity: Active check removed | KILLED by 1: activity_on_terminal_or_non_active_accounts_is_refused |
| retention 7 days -> 6 | KILLED by 1: close_boundary_is_terminal_at_plus_seven_days_for_every_terminal_status |
| close >= -> > | KILLED by 3: close_boundary_is_terminal_at_plus_seven_days_for_every_terminal_status, close_other_signer_refused_closing_twice_impossible_and_the_next_open_gets_the_next_id, compute_units_per_instruction |
| close: terminal check removed | KILLED by 1: close_is_refused_for_an_active_account_even_when_held |
| close: trader constraint removed | KILLED by 1: close_other_signer_refused_closing_twice_impossible_and_the_next_open_gets_the_next_id |
| ban ignores held accounts | KILLED by 1: held_count_bookkeeping_across_every_path_and_ban_wallet |

Survivor: "report_fail: flat not set" removes `acct.flat = 1;`. It is equivalent: `Meta.flat == 1` is enforced (`FailNotFlat`) and `apply_engine_state` already stores the Meta's flat. The line is kept as an explicit statement of the rule.

## DESIGN edits (docs/DESIGN.md, notes only)
1. 5.1 row `pass_proposed_at`: holds `terminal_at` once the account is terminal.
2. 5.2: `WalletProfile` is `... banned u8, held_count u8, pad [u8;4]`; `ban_wallet` refuses while `held_count > 0`.
3. 4.4 I-11: the `report_fail` batch shape and the `MustReportFail` rule of `post_journal`.

## DECISIONS (design silent; strictest option taken)
1. `Meta.flat == 1` is required for BOTH fail reasons; after a quick overshoot only `Fail` and `Meta` may follow; the `Fail` event needs a valid proof (feed of a passed market, age, value).
2. `report_fail` is allowed on a held account (clears the hold) and does not ban the wallet.
3. A vault that already holds the challenge as Abandoned (the permissionless `mark_abandoned`) is learned in `touch_activity`/`release_account` by reading its `TraderState` (owner and PDA checked, status byte 3), because `record_activity` would only error. This is an addition to the scope.
4. A throttled `release_account` is a harmless success without a vault call.
5. `fail_for_violation` needs an Active account (implied by `held == 1`); `release_hold` needs only `held == 1`.
6. `AccountFailed`, `AccountHeld`, `HoldReleased`, `ViolationFail` and the existing `AccountAbandoned` use `emit_cpi!`.
7. `hold_account` is allowed in every `pass_pending` state; `close_account` never touches the wallet profile.
8. The hardening of RESOLVE (d) checks only the final state of the batch.
9. (P19, decision 60) `fail_for_violation` leaves `pass_pending = 3` (`PASS_VIOLATION`) on the account it fails, after `end_account` zeroed it: the one marker that tells `reset_account` HOW an account failed. The layout is unchanged (207 bytes). `report_fail`, the Abandoned mark and a graduation leave `pass_pending = 0`. `veto_pass` now needs `pass_pending` 1 or 2 (it used to need "not 0"; identical for every state the program writes, and the marker cannot be erased by a veto). The Active-account rule of every pass instruction is unchanged. Tests: `tests-rs/tests/violation_reset.rs`.

## Deploy checklist (values that live outside the code; none is set by this repo)
- **Program id**: replace the placeholder `9NAdnJ7ySQqPYuPKu6d6wCbcaHHMnzwFT6xEcRfXX2a7` in `declare_id!` and `Anchor.toml` with the real keypair's id (generated locally, never printed or committed); register THAT id as the vault product.
- **Build**: deploy only the DEFAULT build (real admin keys, `anchor build`); never `--features localnet`. Check `scripts/anchor-build-checked.sh` (no stack overflow) and record the sha256.
- **Admin keys**: `constants/admin.rs` real SL8 and Rov keys (equal to the vault's); upgrade authority 2-of-2 (decision 30); decide when to burn it after the audit.
- **Vault side** (`register_product` by the vault admins): `product_program_id` = the real id; `max_payout_count = 4` (one payout per phase 2 to 5; the local graduation assumes it); `fee_split_bps` (the test harness uses 6500, the real number is Rovik's); `reset_price_bps` ladder `[10000, 11500, 12000, 12750, 13000]` (decisions 17 and 51; basis points of the CURRENT tier price, indexed by the 0-based phase failed in; never a size-based table like `[100, 150]`); `challenge_sizes` = the rule set's ten tiers (size and cost) exactly, ALL SIZES UNIQUE; mints = the vault's USDC and USDT. The vault must be core commit 9a5c179 or later (older vaults price resets off the account size) and shared-interfaces v0.5.0. NEVER reprice or remove a tier (2-of-2 `update_product_config`) while Failed records exist: a pending reset follows the tier cost as registered now.
- **`init_config`** (both admins): `evaluator` = the backend hot key; `vault_program` = the real vault id; `sl8_wallet` = the SL8 fee wallet (it needs a token account of each accepted mint for the 0.4% fee); `pass_delay_secs` 172,800; `window_secs` 86,400; `max_payout_per_window` = $100,000 = 100,000,000,000 micro-USD (decision 37; the test default is $5,000, below one $8,000 payout).
- **Rule set 1** (`create_ruleset`): fee_bps 40, the ten tiers, leverage, holding fee, ladder, and the PENDING numbers `commission_e6` and `spread_e6` per market (zeros in tests; a zero spread means mid fills).
- **Markets** (`create_market`): every market with its `feed_id` (GMX/Hyperliquid symbol hashes per decision 46 to 50), `max_age_secs`, `qty_step`, `qty_min`, `max_notional_usd`, sessions in UTC minutes of the week, `quote_conv_market`.
- **Evaluator operations**: key custody and rotation (`set_evaluator`, both admins); the sample archive and the watchdog (M9) running before any pass can be confirmed; the vault keeper running `reconcile_product` before each heartbeat (the payout tally must equal the vault's books).
- **Rent and fees**: the evaluator and the payer accounts funded with SOL (each `finalize_pass` payer pays one `PayoutClaim` rent).
- **Policy items still open**: what admins do after a veto (OPEN_QUESTIONS #8), the hedging thresholds of 2.9 (#7), the reset price after a wrongful fail (#9).
