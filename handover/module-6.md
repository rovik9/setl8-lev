# Module M6: the pass flow (DONE, verified 2026-10-11)

`request_advance`, `cancel_pass`, `confirm_pass`, `reject_pass`, `veto_pass`, `finalize_pass`; phase transitions, graduation, `PayoutTally`, daily cap. Builds on M5a/M5b (`post_journal` already freezes `pass_pending == 2`). Tests run against the real vault fixture (HEAD 591ced6 when written; rebuilt from core commit 9a5c179 on 2026-10-13, decision 51). No money amount is ever taken from a caller: `finalize_pass` and every other instruction here have NO arguments except `reject_pass(reason u8)`; the payout is `engine::payout_micro(S)` = 0.08 S.

## Accounts in order
All six use `Box<Account>` for big accounts; `scripts/build-test-so.sh` and `scripts/anchor-build-checked.sh` report no frame over 4096 bytes. `#[event_cpi]` instructions end with `event_authority`, `program`.
- **request_advance** (10): 1 `trader` [S] · 2 `trading_account` [W] (PDA, `trader` must be its owner: `Unauthorized`) · 3 `wallet` [W] (`["wallet", account.trader]`) · 4 `config` [R] · 5 `sector_authority` [R] (PDA) · 6 `vault_program` [R] (= `config.vault_program`, `InvalidVaultProgram`) · 7 `product_registry` [R] (vault) · 8 `vault_trader_state` [W] (vault) · 9 `event_authority` · 10 `program`.
- **cancel_pass** (2): 1 `trader` [S] · 2 `trading_account` [W].
- **confirm_pass** (5): 1 `evaluator` [S] · 2 `trading_account` [W] · 3 `config` [R] · 4 `event_authority` · 5 `program`.
- **reject_pass(reason)** (5): same accounts as `confirm_pass`.
- **veto_pass** (6): 1 `config` [R] · 2 `admin_a` [S] · 3 `admin_b` [S] · 4 `trading_account` [W] · 5 `event_authority` · 6 `program`. Both admins (positional) via `require_both_admins`.
- **finalize_pass** (14): 1 `payer` [S, W] · 2 `trading_account` [W] · 3 `config` [W] · 4 `payout_tally` [W] (`["payout_tally"]`, owner = this program) · 5 `wallet` [W] · 6 `sector_authority` [R] · 7 `vault_program` [R] · 8 `product_registry` [W] · 9 `vault_trader_state` [W] · 10 `vault_state` [W] · 11 `payout_claim` [W] (vault PDA `["payout_claim", trader_state, request_id]`, created by the vault) · 12 `system_program` · 13 `event_authority` · 14 `program`.
Vault CPIs, signed by `sector_authority` (`invoke_signed`, bump from the account constraint): `record_activity` (accounts: authority, registry, trader_state) in `request_advance`; `request_payout(trader_wallet, amount = 0.08 S, product_program_id, challenge_id, proposed_request_id = next_request_id)` with accounts authority, registry [W], trader_state [W], vault_state [W], payout_claim [W], payer [S, W], system program. The return data is cleared (`set_return_data(&[])`) before each CPI, read after it, and must come from `config.vault_program` with exactly one byte (`ReturnDataInvalid`).

## RESOLVE answers
**(a)** `request_advance` gets a `wallet` [W] account (DESIGN 6.2 lists none). The vault's `record_activity` returns `ActivityOutcome` `Recorded` 0, `Throttled` 1, `Abandoned` 2. On `Abandoned` (vault idle > 7 days) the local account becomes Abandoned (status 2, `pass_pending` 0), `wallet.active_count - 1` (checked), `AccountAbandoned` is emitted and the call SUCCEEDS without a pending pass (an error would revert the status write). `Recorded` sets `last_activity_pushed = now`; `Throttled` does not. `release_account` and `touch_activity` (later modules) need the same extra wallet account. There is no payer account (nothing is created, no lamports move).
**(b)** A new phase (phase 1 -> 2 at `confirm_pass`, 2..4 -> next at `finalize_pass`) uses `engine::reset_phase_state`: `b = S`, `q_tally = 0`, `latch = 0`, `tight_on = 0`, `r = g_peak = l_level = 0`, `flat = 1`; plus `phase + 1`, `pass_pending = 0`, `pass_requested_at = pass_proposed_at = 0`. KEPT: `lock_until` (an active breach lock is not erased: strictest), `seq`, `journal_hash`, `last_batch_time` (the chain and time monotonicity), `trades_closed`, `vetoes`, `next_request_id` (only +1 by a paid finalize), `last_activity_pushed`, `held` (0 by precondition), `status`. Table below.
**(c)** After the phase 5 payout is queued: status 3 Graduated, phase stays 5, `pass_pending` 0, `wallet.active_count - 1`, `next_request_id` becomes 5 (the vault's `payout_count` is 4; a fifth id is never used). The vault's `request_payout` sets the challenge to Graduated when `payout_count >= registry.max_payout_count` (4 as registered), i.e. exactly at the 4th payout = phase 5. The full-journey test asserts local status 3 and vault status Graduated (TraderState status 1) with `payout_count` 4; `the_vaults_fifth_payout_is_not_reachable` forces the local account back to a confirmed phase 5 and the vault refuses (`InvalidTraderStatus`), reverting everything. If the vault were registered with a smaller `max_payout_count`, it would graduate earlier than the local account and the later `finalize_pass` would revert at the vault; this module assumes the registered 4.

## Phase-transition field table
| field | after confirm (phase 1) / finalize (2..4) |
|---|---|
| b | S |
| q_tally, latch, tight_on, r, g_peak, l_level | 0 |
| flat | 1 |
| phase | +1 |
| pass_pending, pass_requested_at, pass_proposed_at | 0 |
| lock_until | KEPT |
| seq, journal_hash, last_batch_time, trades_closed | KEPT |
| vetoes, last_activity_pushed | KEPT |
| next_request_id | KEPT at phase 1 -> 2; +1 at a paid finalize |
| status | Active (Graduated after phase 5 finalize) |
Graduation keeps every other field as it was (phase 5, `b` at the target) except `status = 3`, `pass_pending = 0`, the pass timestamps 0, `next_request_id + 1`.

## Behaviour notes
- `pass_pending` machine as DESIGN 10.3; `request_advance` precondition order: Paused, Active, held, nothing pending (`PassAlreadyPending`), tight off (`TightModeOn`), then the engine pass check (`PhaseFailed`, `NotFlat`, `TargetNotReached`). "Phase failed" = `b <= static floor` or `q_tally` above 0.028 S (the engine's own `apply_closed_slice`).
- `confirm_pass` repeats the pass condition (state 1 still accepts journal batches); `finalize_pass` repeats it too (it cannot fail unless the account was altered). `cancel`, `reject`, `veto` need no Active status (they only unfreeze) and do not touch timestamps.
- `finalize_pass` order: Paused, Active, state 2, not held, delay (`now >= pass_proposed_at + CURRENT config.pass_delay_secs`), pass condition, amount, daily cap (window rolled in memory first, then `window_total + amount <= max_payout_per_window`), CPI, return data. **Consequence of a delay change:** an admin lowering/raising `pass_delay_secs` (clamped 1 to 14 days) while a pass waits moves the real eligibility date; `eligible_at` in `PassConfirmed` is only the date computed at confirmation. `Paid`: tally checked adds, window stored, `next_request_id + 1`, next phase or graduation, `PassFinalized` (+ `PhaseAdvanced`). `Abandoned`: status 2, slot freed, tally, window and config untouched, no claim.
- The window counters are written only on `Paid`, so a refused or abandoned finalize never moves the window.
- `veto_pass` works whatever the account status and never touches `b`, phase, tally or the vault.
- `set_paused` blocks `request_advance` and `finalize_pass` only (plus M1/M5 ones); `confirm_pass`, `reject_pass`, `cancel_pass`, `veto_pass`, `post_journal` keep working. A paused vault product blocks `finalize_pass` at the vault (rolled back) but not `request_advance` (the vault's `record_activity` does not check it).

## Errors (new: 6084 and up; the rest were reused from DESIGN 12)
| code | name | instruction |
|---|---|---|
| 6084 | PassAlreadyPending | request_advance |
| 6085 | TightModeOn | request_advance |
| 6086 | PhaseFailed | request_advance, confirm_pass, finalize_pass |
| 6087 | RejectReasonInvalid | reject_pass |
| 6088 | PayoutTallyInvalid | finalize_pass |
Reused: Unauthorized 6000 (signers), Paused 6001, AccountNotActive 6004, NotFlat 6020, TargetNotReached 6021, NoPendingPass 6022 (wrong state for cancel/confirm/reject/veto/finalize), PassDelayNotElapsed 6023, DailyPayoutCapReached 6024, ReturnDataInvalid 6026, WrongPhase 6028, MathOverflow 6029, CancelTooEarly 6033, AccountHeld 6037, InvalidVaultProgram 6041. `PayoutNotAccepted` 6025 is not used: `Abandoned` is handled, anything else aborts as `ReturnDataInvalid`.

## Tests
`tests-rs/tests/pass_flow.rs` (72) + harness `tests-rs/tests/common/pass.rs`; program unit tests in `pass_common.rs` (5 new, lib total 24). `cd tests-rs && cargo test`: 227 tests (155 before + 72). Every failure test asserts the exact error and an unchanged digest of accounts, config, tally, vault records, claims and token balances.
- edges (8): edge_request_advance_0_to_1, edge_reject_pass_1_to_0, edge_cancel_pass_1_to_0, edge_confirm_pass_1_to_2_in_phases_2_to_5, edge_confirm_pass_in_phase_1_goes_to_0_and_starts_phase_2, edge_veto_pass_from_state_1, edge_veto_pass_from_state_2, edge_finalize_pass_2_to_0_starts_the_next_phase
- illegal edges (6): illegal_request_advance_from_states_1_and_2, illegal_cancel_pass_from_states_0_and_2, illegal_confirm_pass_from_states_0_and_2, illegal_reject_pass_from_states_0_and_2, illegal_veto_pass_from_state_0, illegal_finalize_pass_from_states_0_and_1
- request_advance (12): request_not_active/held/paused_sector/not_flat/tight_mode_on_is_refused, request_target_boundary_is_1_08_s_to_the_micro, request_phase_marked_failed_is_refused, request_wrong_signer_and_missing_signature, request_wrong_vault_program_and_wrong_sector_authority, request_amount_in_the_event_is_zero_in_phase_1_and_eight_percent_after, request_records_activity_in_the_vault_and_remembers_it, request_still_works_while_the_vault_product_is_paused, request_abandoned_by_the_vault_marks_the_account_and_frees_the_slot
- cancel_pass (3): cancel_boundary_is_3600_seconds, cancel_by_another_signer_is_refused, cancel_leaves_everything_else_untouched
- confirm_pass (5): confirm_wrong_signer_is_refused, confirm_preconditions_each_violated_alone, confirm_works_while_the_sector_is_paused, confirm_phase_1_starts_a_fresh_phase_2_with_the_exact_reset_list, confirm_phases_2_to_5_event_and_eligibility
- reject_pass (3): reject_each_reason_is_accepted_and_emitted, reject_bad_reason_codes_are_refused, reject_wrong_signer_is_refused
- veto_pass (4): veto_single_admin_is_refused, veto_counts_and_changes_nothing_else_in_both_pending_states, veto_counter_saturates_at_255, a_vetoed_trader_can_request_again
- finalize_pass (9): finalize_delay_boundary_uses_the_current_config_value, finalize_delay_clamped_minimum_is_one_day_and_a_changed_delay_moves_the_date, finalize_anyone_can_pay, finalize_held_is_refused_until_release_and_the_delay_clock_keeps_running, finalize_paused_sector_is_refused, finalize_with_the_vault_product_paused_rolls_everything_back, finalize_rechecks_the_pass_condition_and_the_phase, finalize_abandoned_by_the_vault_changes_neither_tally_nor_window, finalize_next_phase_is_fresh_with_the_same_field_list_as_confirm
- daily cap (5): cap_exactly_the_payout_is_accepted_and_one_micro_less_is_refused, cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second, cap_allows_up_to_twice_the_cap_across_a_window_boundary, cap_smaller_than_one_payout_blocks_every_finalize, cap_window_total_does_not_change_when_nothing_is_paid
- PayoutTally (6): tally_layout_is_the_25_bytes_of_the_contract, tally_only_goes_up_and_equals_what_the_vault_queued, tally_is_not_touched_by_anything_but_a_paid_finalize, tally_overflow_is_refused_and_changes_nothing, tally_account_that_does_not_parse_is_refused, tally_wrong_owner_or_address_is_refused
- journey and vault agreement (4): full_journey_phases_1_to_5_then_graduation_and_a_new_account, the_vaults_fifth_payout_is_not_reachable, a_wrong_request_id_is_refused_by_the_vault, a_confirmed_pass_freezes_post_journal_and_a_requested_one_does_not
- attacks (5): attack_no_instruction_takes_an_amount_and_the_evaluator_has_no_payout_path, attack_the_admin_pair_cannot_confirm_or_finalize_through_veto, attack_a_trader_cancelling_against_the_evaluators_decision, attack_double_finalize, attack_finalize_with_another_traders_accounts
- compute units (1): compute_units_per_instruction
- return data (unit tests, `pass_common.rs`): `return_data_must_come_from_the_vault_with_one_byte` (wrong program id, none, empty, two bytes, own cleared data). A vault double that answers from another program cannot be built in LiteSVM without a second SBF program, so the helper `parse_return` is unit-tested and the CPIs call `clear_return_data()` first. `clear_return_data` itself has no test that can fail without a double (hardening only).
- A direct vault `request_payout` with a PDA as top-level signer cannot be modelled (sigverify is off in this harness); the attack test uses the evaluator key in the authority slot, which the vault refuses.

## Full journey (S = $100,000, delay 1 day, cap $100k, real `Close` events through `post_journal`)
Each phase: a Close with pnl +8,000.000000 (age > 45 s, so full credit) brings `b` to exactly 1.08 S, then request, confirm (and finalize for 2..5).
| after | vault claim | tally (count / total) | vault registry totals | wallet `active_count` | vault TraderState |
|---|---|---|---|---|---|
| phase 1 confirm (fresh phase 2) | none | 0 / 0 | 0 / 0 | 1 | Active, payouts 0 |
| phase 2 finalize | id 1 owed 8,000 | 1 / 8,000 | 1 / 8,000 | 1 | Active, 1 |
| phase 3 finalize | id 2 owed 8,000 | 2 / 16,000 | 2 / 16,000 | 1 | Active, 2 |
| phase 4 finalize | id 3 owed 8,000 | 3 / 24,000 | 3 / 24,000 | 1 | Active, 3 |
| phase 5 finalize | id 4 owed 8,000 | 4 / 32,000 | 4 / 32,000 | 0 (Graduated) | Graduated, 4 |
Then the wallet opens a new account (challenge id 1; the next id is 2).

## Compute units (whole transaction, numbers only; M11 measured the worst cases again, handover/module-11.md: `request_advance` 45,937, `finalize_pass` up to 84,594)
request_advance 45,887 · confirm_pass (phases 2 to 5) 23,963 · confirm_pass (phase 1) 23,470 · reject_pass 21,683 · cancel_pass 4,697 · veto_pass 22,639 · finalize_pass 72,534. All under the 200k default. Stack: no frame over 4096 bytes (default and localnet builds).

## Hand-applied mutations: 48 of 48 killed (driver: patch, run `cargo test --lib`, rebuild the localnet `.so`, run `cargo test --test pass_flow`, restore)
| mutation | killed by |
|---|---|
| delay >= -> > | KILLED by 24: a_wrong_request_id_is_refused_by_the_vault, attack_double_finalize, attack_finalize_with_another_traders_accounts |
| cap <= -> < | KILLED by 3: cap_allows_up_to_twice_the_cap_across_a_window_boundary, cap_exactly_the_payout_is_accepted_and_one_micro_less_is_refused, cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second |
| window roll >= -> > | KILLED by 2: cap_allows_up_to_twice_the_cap_across_a_window_boundary, cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second |
| window roll keeps old total | KILLED by 2: cap_allows_up_to_twice_the_cap_across_a_window_boundary, cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second |
| tally not written | KILLED by 7: attack_double_finalize, cap_allows_up_to_twice_the_cap_across_a_window_boundary, full_journey_phases_1_to_5_then_graduation_and_a_new_account |
| tally count decreased | KILLED by 19: attack_double_finalize, attack_finalize_with_another_traders_accounts, attack_no_instruction_takes_an_amount_and_the_evaluator_has_no_payout_path |
| tally total decreased | KILLED by 19: attack_double_finalize, attack_finalize_with_another_traders_accounts, attack_no_instruction_takes_an_amount_and_the_evaluator_has_no_payout_path |
| tally count not bumped | KILLED by 6: attack_double_finalize, full_journey_phases_1_to_5_then_graduation_and_a_new_account, tally_layout_is_the_25_bytes_of_the_contract |
| next_request_id not incremented | KILLED by 6: cap_allows_up_to_twice_the_cap_across_a_window_boundary, cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second, finalize_next_phase_is_fresh_with_the_same_field_list_as_confirm |
| 0.08 S replaced by another amount | KILLED(unit) by: instructions::pass_common::tests::pass_amount_is_zero_in_phase_one_and_eight_percent_after |
| phase not reset after payout | KILLED by 2: finalize_next_phase_is_fresh_with_the_same_field_list_as_confirm, full_journey_phases_1_to_5_then_graduation_and_a_new_account |
| graduation missed | KILLED by 2: full_journey_phases_1_to_5_then_graduation_and_a_new_account, the_vaults_fifth_payout_is_not_reachable |
| wallet slot not freed on graduation | KILLED by 1: full_journey_phases_1_to_5_then_graduation_and_a_new_account |
| cancel threshold 3600 -> 3599 | KILLED by 1: cancel_boundary_is_3600_seconds |
| cancel >= -> > | KILLED by 6: attack_a_trader_cancelling_against_the_evaluators_decision, cancel_boundary_is_3600_seconds, cancel_leaves_everything_else_untouched |
| cancel signer constraint removed | KILLED by 1: cancel_by_another_signer_is_refused |
| target b >= 1.08 S -> > | KILLED(unit) by: instructions::pass_common::tests::pass_condition_boundaries |
| flat check removed | KILLED(unit) by: instructions::pass_common::tests::pass_condition_boundaries |
| quick limit > -> >= | KILLED(unit) by: instructions::pass_common::tests::pass_condition_boundaries |
| request: held check removed | KILLED by 1: request_held_is_refused |
| confirm: held check removed | KILLED by 1: confirm_preconditions_each_violated_alone |
| finalize: held check removed | KILLED by 1: finalize_held_is_refused_until_release_and_the_delay_clock_keeps_running |
| request: paused check removed | KILLED by 1: request_paused_sector_is_refused |
| finalize: paused check removed | KILLED by 1: finalize_paused_sector_is_refused |
| vetoes wrap instead of saturate | KILLED by 1: veto_counter_saturates_at_255 |
| veto allowed from state 0 | KILLED by 2: attack_the_admin_pair_cannot_confirm_or_finalize_through_veto, illegal_veto_pass_from_state_0 |
| veto: both admins check removed | KILLED by 1: veto_single_admin_is_refused |
| reject reason unchecked | KILLED by 1: reject_bad_reason_codes_are_refused |
| reject: evaluator check removed | KILLED by 1: reject_wrong_signer_is_refused |
| confirm: evaluator check removed | KILLED by 1: confirm_wrong_signer_is_refused |
| Abandoned outcome ignored (finalize) | KILLED by 1: finalize_abandoned_by_the_vault_changes_neither_tally_nor_window |
| Abandoned outcome ignored (request) | KILLED by 1: request_abandoned_by_the_vault_marks_the_account_and_frees_the_slot |
| return-data program id unchecked | KILLED(unit) by: instructions::pass_common::tests::return_data_must_come_from_the_vault_with_one_byte |
| request: tight_on check removed | KILLED by 1: request_tight_mode_on_is_refused |
| request: already-pending check removed | KILLED by 1: illegal_request_advance_from_states_1_and_2 |
| request: pass condition removed | KILLED by 3: request_not_flat_is_refused, request_phase_marked_failed_is_refused, request_target_boundary_is_1_08_s_to_the_micro |
| confirm: pass condition removed | KILLED by 1: confirm_preconditions_each_violated_alone |
| finalize: pass condition removed | KILLED by 1: finalize_rechecks_the_pass_condition_and_the_phase |
| confirm phase 1 branch lost | KILLED by 9: a_confirmed_pass_freezes_post_journal_and_a_requested_one_does_not, compute_units_per_instruction, confirm_phase_1_starts_a_fresh_phase_2_with_the_exact_reset_list |
| confirm: pass_proposed_at not set | KILLED by 3: confirm_phases_2_to_5_event_and_eligibility, edge_confirm_pass_1_to_2_in_phases_2_to_5, full_journey_phases_1_to_5_then_graduation_and_a_new_account |
| request: last_activity_pushed on Throttled | KILLED by 2: edge_request_advance_0_to_1, request_records_activity_in_the_vault_and_remembers_it |
| window total not stored | KILLED by 3: cap_allows_up_to_twice_the_cap_across_a_window_boundary, cap_exactly_the_payout_is_accepted_and_one_micro_less_is_refused, cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second |
| window start not stored | KILLED by 2: cap_exactly_the_payout_is_accepted_and_one_micro_less_is_refused, cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second |
| finalize: amount>0 check removed | KILLED by 1: finalize_rechecks_the_pass_condition_and_the_phase |
| finalize: active status check removed | KILLED by 1: finalize_rechecks_the_pass_condition_and_the_phase |
| request: active status check removed | KILLED by 2: full_journey_phases_1_to_5_then_graduation_and_a_new_account, request_not_active_is_refused |
| abandoned does not free the slot | KILLED by 2: finalize_abandoned_by_the_vault_changes_neither_tally_nor_window, request_abandoned_by_the_vault_marks_the_account_and_frees_the_slot |
| phase reset drops the kept lock | KILLED(unit) by: instructions::pass_common::tests::start_phase_resets_the_engine_fields_and_keeps_the_rest |

## DECISIONS (design silent; strictest option taken)
1. `request_advance` takes a `wallet` account and no payer (RESOLVE a). `Recorded` alone updates `last_activity_pushed`.
2. A new phase keeps an active `lock_until`, the chain fields and the counters (RESOLVE b); `pass_requested_at`/`pass_proposed_at` are cleared by a phase start, an Abandoned mark and a graduation, but NOT by cancel, reject or veto (they only change `pass_pending`, which gates them).
3. "Phase failed" is `b <= static floor` or `q_tally > 0.028 S`; the pass condition is re-checked at confirm and finalize.
4. `finalize_pass` refuses an account that is not Active (`AccountNotActive`) and a phase-1 account (`WrongPhase`, no amount).
5. `Abandoned` from the vault ends the call successfully with the local account Abandoned (no error, so the vault's own flip stays consistent); tally, window and claim are untouched. `pass_pending` is cleared.
6. The window counters are written only after a `Paid` outcome.
7. `PassCancelled` is a plain `emit!` log; the other pass events use `emit_cpi!`. `PassFinalized` is followed by `PhaseAdvanced` when a next phase starts (`to_phase` 0 = graduated).
8. The sector pause does not block `confirm_pass`, `reject_pass`, `cancel_pass`, `veto_pass`.
9. A malformed tally account (wrong magic/version/length) is `PayoutTallyInvalid`; overflow is `MathOverflow` and never saturates.
10. The vault's `max_payout_count` is assumed to be 4 (as registered by the harness and DESIGN 10.4).
