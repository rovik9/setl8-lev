# Module M8: `reset_account` (DONE, verified 2026-10-17)

The trader pays the phase-reset price for a locally Failed account and gets a NEW account in the SAME phase with fresh engine state. The price is the vault's (decision 51); this module is decision 58. Requirements: `handover/module-8-requirements.md` (all six test items are in `tests-rs/tests/reset.rs`). Everything below was run against the REAL vault fixture (core_vault.so, vault core 9a5c179, shared-interfaces v0.5.0), through OUR instruction.

## The instruction: `reset_account(prev_challenge_id: u64)`, the mint is an account (as in `open_account`)
Accounts in order (22; `#[event_cpi]` adds the last two). The order is `open_account`'s with two additions: the previous local account after the wallet, the previous vault record after the new one.

1 `trader` [S, W] · 2 `payer` [S, W] (must be the trader) · 3 `config` · 4 `ruleset` (of `config.current_ruleset`) · 5 `wallet` [W] (`["wallet", trader]`; it must exist) · 6 `prev_trading_account` [W] (PDA of its own stored trader and challenge id; `trader` must be the signer) · 7 `trading_account` [W] (`init`, `["acct", trader, wallet.next_challenge_id]`) · 8 `sector_authority` (PDA) · 9 `vault_program` (= `config.vault_program`) · 10 `product_registry` [W] (checked in the handler, see below) · 11 `vault_trader_state` [W] (the NEW vault record, created by the vault) · 12 `prev_vault_trader_state` [W] · 13 `vault_state` · 14 `trader_token_account` [W] · 15 `mint` · 16 `pool_token_account` [W] · 17 `vault_sl8_token_account` [W] · 18 `fee_token_account` [W] (owner `config.sl8_wallet`) · 19 `token_program` · 20 `system_program` · 21 `event_authority` · 22 `program`.

### Order of checks (each with its own error; the order is pinned by `the_order_of_the_checks_is_pinned_pair_by_pair`)
1. sector not paused: `Paused` 6001
2. payer is the trader: `Unauthorized` 6000
3. the rule set is the current one (version and PDA): `RuleSetMissing` 6031
4. wallet not banned: `WalletBanned` 6035
5. the previous account is the one named (`prev.challenge_id == prev_challenge_id`) and is locally `Failed` (an Active, Held, Abandoned or Graduated account is refused): `ResetNotAllowed` 6027
6. the previous account was not reset yet: `AccountAlreadyReset` 6107 (NEW)
7. `active_count < 5` (the failed account was freed at its fail; the new one is counted): `WalletCapReached` 6002
8. the registry is the vault-owned `["product_registry", this program]` PDA (address, owner, discriminator, `product_program_id` all checked, the tier and ladder counts bounded) and the previous vault record is the vault-owned PDA of the failed challenge: `InvalidVaultAccount` 6103
9. the price from `reset_price::reset_amount_micro(registry tiers, size, phase - 1, registry ladder)` (the ONLY pricing code): `ResetTierInvalid` 6104 (no tier or a duplicate size), `ResetPhaseOutOfRange` 6105 (phase 0 or beyond the ladder), `ResetPriceOverflow` 6106
10. the fee `ceil(price x ruleset.fee_bps / 10_000)`: `MathOverflow`

Then, atomically: wallet counters (`next_challenge_id` +1, `active_count` +1); CPI `deposit_reset` (builder of shared-interfaces v0.5.0, `invoke_signed` with the `sector_authority` seeds, args `amount`, `trader_wallet`, `product_program_id`, `prev_challenge_id`, `new_challenge_id`, `reset_phase = phase - 1`; NEVER `payout_count` or `account_size`, the builder has no such field); the new vault record is read back (PDA, owner, header; `account_size` must equal the failed account's: `InvalidVaultAccount`); the fee transfer (plain SPL, trader -> SL8 fee account); the new account; the mark on the previous one; two events.

New error codes: **6107 `AccountAlreadyReset`** (added to `tests-rs/tests/error_codes.rs` and the DESIGN error table). 6104 to 6106 came with the price helper (P16).

### The new account
`TradingAccount::init_fresh` (shared with `open_account`, so a purchase and a reset write the same fresh state): `b = S`, `q = 0`, no lock, `flat = 1`, tight off, latch off, `r = g_peak = l_level = 0`, status Active, nothing pending, not held, `seq 0`, `journal_hash = sha256("SL8LEV" || new account)`, `created_at = last_batch_time = last_activity_pushed = now`. Differences from a purchase: `phase` = the failed account's phase, `next_request_id` = the vault's `payout_count + 1`, `ruleset_version` = the current rule set (the failed account's snapshot is not carried over), `challenge_id` = the wallet counter.

### Payouts carry over
The vault copies `payout_count` from the failed record into the new record. The sector reads it back from the NEW vault record after the CPI (the vault is the authority: it will refuse a claim whose `proposed_request_id` is not `payout_count + 1`). It does not trust the failed local account's `next_request_id` (test `the_vaults_payout_count_not_the_local_record_decides_the_next_request_id`: local 9 / vault 2 gives 3; local 1 / vault 3 gives 4). The proof that the chain cannot over-pay is `a_phase_5_reset_after_three_payouts_can_earn_only_the_remaining_one`: three real payouts (phases 2, 3, 4), a fail in phase 5, a reset, ONE more payout (request id 4), the vault Graduates the account at its `max_payout_count` 4, the tally and the registry totals both say 4 x 0.08 S, a further request is `AccountNotActive` and the first vault record has no claim 4. `full_chain_fail_in_phase_{1..5}` walk the new account to graduation with real passes.

### Marking the previous account
No spare byte exists in the 207-byte layout. `reset_at` is stored in `pass_requested_at`, which `end_account` zeroes and which nothing reads once an account is terminal, exactly like `terminal_at` in `pass_proposed_at` (M7 RESOLVE b). Nonzero = reset. Using `pass_pending` instead would have let `veto_pass` (it needs `pass_pending != 0`) clear the mark of a Failed account, so it was rejected.

### Events
`Opened` (the record every purchase emits, with `phase` = the failed phase and `tier_index` = the position of the size in the current rule set, 255 if it is not there) then `AccountReset { prev_account, account, trader, prev_challenge_id, challenge_id, phase, price, fee, payout_count, time }`. Neither is part of the hash chain. **Indexer story:** a log consumer that already turns `Opened` into the `opened` line of `events.jsonl` needs nothing new for a reset: the line carries `phase` (the verifier and the evaluator already start an account in any phase 1 to 5). `AccountReset` is the link between two logs (previous account, phase, price paid); the verifier of ONE account does not follow it, an auditor compares `Opened.phase` with the last phase of the previous account's log (trust-matrix row "an evaluator log that opens the new account in a higher phase").

### Fee decision (one line)
The 0.4% is charged on top of the reset price with the same `ruleset.fee_bps` field and the same `swap_fee_micro` as `open_account` (assumption A-FEE-1, still unconfirmed): a plain SPL transfer trader -> SL8 fee account, not part of the vault deposit, so there is one constant to change if Rovik drops the fee on resets.

## Prices asserted (100K tier; 2.5K tier), each = price + fee, the vault's 65/35 split, same phase, vault record with the failed record's size and payouts
| failed in | price 100K | fee 100K | price 2.5K | fee 2.5K |
|---|---|---|---|---|
| phase 1 | 859.000000 | 3.436000 | 30.000000 | 0.120000 |
| phase 2 | 987.850000 | 3.951400 | 34.500000 | 0.138000 |
| phase 3 | 1,030.800000 | 4.123200 | 36.000000 | 0.144000 |
| phase 4 | 1,095.225000 | 4.380900 | 38.250000 | 0.153000 |
| phase 5 | 1,116.700000 | 4.466800 | 39.000000 | 0.156000 |

## Compute units (whole transaction; `compute_units_per_phase_and_tier`: eight fresh traders per cell, min / median / max)
The spread comes from the PDA bump searches of the trader's random keys, not from the phase. About 42,000 are inside the vault's `deposit_reset`, three token transfers cost 6,100 each, the two event CPIs 6,500 each.
| tier | phase 1 | phase 2 | phase 3 | phase 4 | phase 5 |
|---|---|---|---|---|---|
| 100K | 126,671 / 131,171 / 135,671 | 125,171 / 128,171 / 140,171 | 126,671 / 128,171 / 135,671 | 125,171 / 131,171 / 147,671 | 126,671 / 129,671 / 140,171 |
| 2.5K | 124,947 / 130,947 / 135,447 | 126,447 / 130,947 / 139,947 | 127,947 / 129,447 / 135,447 | 124,947 / 129,447 / 135,447 | 127,947 / 127,947 / 132,447 |

Largest seen in this run: **147,671**; an earlier run saw 156,702. Both fit the default 200,000-unit limit; clients should request 250,000. Stack: `scripts/build-test-so.sh` reports no frame overflow (all big accounts are boxed).

## Tests
* Program library unit tests: 33 -> **43** (`vault_view` 8: the discriminators are the Anchor hashes, registry decode, bad header, truncation, count caps, trader-state decode and refusals; `state::trading_account` 2: `init_fresh` writes every field, `reset_at`).
* `tests-rs/tests/reset.rs`: **43 tests** (new). `tests-rs/tests/trust_reset.rs`: **2** (10 trust rows). `tests-rs`: 288 -> **333** tests in 17 files; `error_codes.rs` gained 6107.
  * Prices and payments: `reset_charges_the_ladder_at_the_100k_tier_in_all_five_phases`, `..._2_5k_tier_...`, `the_launch_ladder_numbers_are_the_ones_of_design_9_5`.
  * Same-phase resume and fresh state: `a_phase_3_failure_resets_into_phase_3_with_fresh_engine_state`, `the_new_account_trades_passes_and_the_chain_starts_at_its_own_genesis`, `the_new_account_follows_the_current_rule_set_and_its_fee_rate`.
  * Payouts and the whole chain: `a_phase_5_reset_after_three_payouts_can_earn_only_the_remaining_one`, `the_vaults_payout_count_not_the_local_record_decides_the_next_request_id`, `full_chain_fail_in_phase_1..5_reset_trade_pass_to_graduation`, `a_reset_account_that_fails_again_can_be_reset_again_...`.
  * The registry as it is NOW: `a_tier_repriced_between_the_fail_and_the_reset_is_followed`, `a_tier_removed_...`, `duplicate_tier_sizes_are_refused_before_any_cpi_...`, `a_ladder_shorter_than_the_failed_phase_...`, `a_price_that_does_not_fit_u64_...`.
  * Refusals: paused sector, banned wallet, Active / Held / Abandoned / Graduated account, wrongly named previous account, another trader's account, second reset (local mark and the vault backstop), 6th active account (and the 5th allowed), payer is not the trader and signatures, wrong vault program / sector authority / rule set, wrong registry (7 variants) and wrong previous vault record (5 variants), paused vault product, payer without funds (price - 1 and price + fee - 1), wrong mint / token accounts / fee account, no free-reset path (the instruction data is 16 bytes, only the trader signs), USDT.
* New standalone suites: `crates/evaluator/tests/reset_start.rs` 4, `crates/verifier/tests/m8.rs` 7. Verifier integration 178 -> 185, evaluator integration +4. Nothing else changed in a count.
* Gates (this session, one machine): `scripts/build-test-so.sh` 1 s (no stack-frame overflow, sha256 `1269d778c16459051d1e36d2dfd5769d06fa8e6164ee2a82444b4c669acf427b`); `scripts/test-engine.sh` 129 s; `cd tests-rs && cargo test` all 333 green; `ENGINE_PATHS=100000` 36.5 s; `DIFF_PATHS=10000` (release) 56.3 s, 86,566 transactions; clippy `-D warnings` clean for the program, evaluator, verifier and tests-rs.

### Coverage of the reset story (10,000 differential paths against the real program and vault; the 100,000-path engine-only run in brackets)
| counter | count | floor |
|---|---|---|
| resets | 1,266 (12,558) | 150 |
| failed in phase 2 | 254 (2,475) | 20 |
| failed in phase 3 | 269 (2,576) | 20 |
| failed in phase 4 | 275 (2,489) | 20 |
| failed in phase 5 | 231 (2,556) | 20 |
| reset, then the new account completes a pass | 216 (2,255) | 20 |
(phase 1: resets - the four rows above.) The run fails if a floor is missed.

### Hand-applied mutations: 61 applied, 59 killed, 2 equivalent survivors (driver: patch, rebuild the localnet `.so` with `scripts/build-test-so.sh`, run `reset` and `open_account`, restore; the driver also applies the five `cargo mutants --list` mutants of `reset_account.rs`)
Killed, each by a named test (the first failing one is listed; most are killed by several):
price from the account size (`a_ladder_shorter_than_...`) · price from the wrong ladder index (`a_phase_3_failure_resets_...`) · payout count ignored (`a_reset_account_that_fails_again_...`) · next_request_id from the local record (`a_phase_3_failure_resets_...`) · banned check skipped (`a_banned_wallet_cannot_reset_...`) · Failed check skipped (`a_held_account_cannot_be_reset`) · Failed check accepts Abandoned (`abandoned_and_graduated_accounts_cannot_be_reset`) · new account in phase 1 / phase + 1 (`a_phase_3_failure_resets_...`) · no reset mark / mark never checked / wrong error for the mark (`a_second_reset_of_one_failed_record_...`) · registry PDA address / owner / product / discriminator not compared (`a_registry_that_is_not_the_vault_owned_pda_...`) · fee skipped / fee twice / fee on the account size / fee rounded down / fee to the trader's own account (`a_payer_without_funds_...`, `reset_charges_the_ladder_at_the_100k_tier_...`) · check order: cap before ban, ban before pause, registry before cap, already-reset before Failed (`the_order_of_the_checks_is_pinned_pair_by_pair`) · wrong seeds: new account at the previous id, sector authority, wallet PDA · payer == trader skipped · pause skipped · rule set currency skipped · cap removed / off by one · `active_count` / `next_challenge_id` not incremented · `reset_phase` 0 / phase (`a_phase_3_failure_...`) · new challenge id / prev challenge id / product id wrong in the CPI · genesis hash of the previous account · `created_at`, `last_batch_time`, `b`, status, `flat` of the fresh account (`open_account_happy_path_for_every_tier_index`) · ruleset version of the previous account (`the_new_account_follows_the_current_rule_set_and_its_fee_rate`) · pool and SL8 accounts swapped (`a_failed_account_with_the_hold_cleared_...`) · previous vault record check skipped (`a_previous_vault_record_that_is_not_...`) · size copied by the vault not compared (`the_vault_record_it_creates_must_carry_the_size_...`) · Opened carries phase 1 / tier 0, AccountReset price is the fee / payout_count is the next id, mark is a constant (`reset_charges_the_ladder_at_the_100k_tier_...`) · cargo-mutants: handler body replaced by `Ok(())`, `fee > 0` -> `== 0` / `< 0`, `account_size ==` -> `!=`.

Survivors, argued equivalent (cannot be observed):
* *Failed check before the named-challenge check*: both raise `ResetNotAllowed` 6027, so the order of the two cannot be seen from outside.
* *`fee > 0` -> `fee >= 0`*: with a zero fee `transfer_checked` of 0 succeeds and moves nothing; the only difference is a few compute units (a zero fee needs `fee_bps = 0`).

### cargo-mutants (27.1.0) on the files M8 added or changed in the program: `vault_view.rs` and `state/trading_account.rs`
55 mutants: 27 caught, 24 unviable, **4 missed, all in code M8 did not touch**: `TradingAccount::terminal_at` replaced by 0 / 1 / -1 and a deleted match arm in `engine_err`. They are killed by the LiteSVM suites (M7 `close_account` and the pass tests), not by the library unit tests cargo-mutants runs. Every mutant of `init_fresh`, `reset_at` and `vault_view.rs` is caught or unviable. `reset_account.rs` has 5 mutants in cargo-mutants' catalogue (it only mutates operators and function bodies, not `require!`), run by the driver above: 5 killed 4 + the `fee >= 0` equivalent.

## Evaluator, verifier, campaigns (what phase-p starts needed)
* **Evaluator: nothing changed.** `Evaluator::new(rules, AccountParams { phase: p, .. })` already starts any phase 1 to 5 with `engine::reset_phase_state(S)`, the genesis of its own account and no lock (the differential campaign has started paths in phases 2 to 5 since M10). `crates/evaluator/tests/reset_start.rs` (4 tests) pins it: fresh state for every p, phases 0 and 6 refused, a pass from every phase-p start with the verifier VALID, the new account independent of the failed one (a different address, an order signed for the old account is `BadSignature`).
* **Verifier: nothing changed.** It takes the starting phase from the `opened` line and replays from the fresh phase state. `crates/verifier/tests/m8.rs` (7 tests) pins it for every phase (VALID, fresh state, the pass judged in phase p, `p+1`/graduation after the finalize) and keeps the dishonest variants: a pass request for another phase is `phase_mismatch`, an `opened` phase that differs from the chain's account is `StateMismatch`, a `phase_advanced` from a phase the account never was in is refused.
* **Generator** (`crates/evaluator/src/testkit.rs`): `reset_story(&first)` (a failing story, 70% of them) and `generate_after_reset(&first, opened_time)` (the second segment: same tier, size, phase, markets and commissions, another story; a pass story from the $15K tier up). `generate` itself is unchanged.
* **Campaigns.** Engine-only (`engine_only.rs`): after a failed first segment the world of a new account (another address, challenge + 1, the failed phase) runs the second segment, with every check a path has. LiteSVM (`tests-rs/tests/differential.rs`, `LiteChain::reset`): the failed account goes through the REAL `reset_account` against the real vault; the price and fee emitted and the trader's balance change are compared with the DESIGN table computed independently; the program's `Opened` event must equal the log's `opened` line; the whole 207-byte account is compared after every transaction of both accounts; the verifier says VALID on the program's own events of each account. Floors over 10,000 paths: 150 resets, 20 for each of phases 2 to 5, 20 resets that then complete a pass.

## DECISIONS (the prompt or DESIGN was silent)
1. `reset_at` lives in `pass_requested_at` (see above); `pass_pending` was rejected because `veto_pass` could clear it.
2. The new account takes the CURRENT rule set (its `fee_bps` and its snapshot), like a purchase, so the `ruleset` account is part of the instruction. A rule set change between fail and reset therefore changes the rules of the resumed phase; nothing in DESIGN says otherwise.
3. `next_request_id` comes from the vault's record, not from the failed local account.
4. `tier_index` of the `Opened` event is the index of the size in the current rule set (255 when absent): the evaluator and the verifier only echo it.
5. A new error 6107 `AccountAlreadyReset`; `ResetNotAllowed` (6027) stays for an account that is not Failed or the wrong id.
6. The vault's new record must carry the failed account's size (`InvalidVaultAccount` otherwise): a belt-and-braces check that the price paid is the price of THIS size.
7. `deposit_reset` first, the fee transfer after it (the prompt's order; `open_account` does the fee first). Inside one atomic transaction the order only decides which error a doubly-failing call shows.
8. The registry's tier and ladder counts are bounded at 64 when decoded by hand (the vault's tier cap is lower); a larger count is `InvalidVaultAccount`.
9. A violation-failed account can be reset once its wallet is unbanned (see "Open points"). Nothing on the account records HOW it failed.

## Open points for Rovik
* DESIGN 2.9 says a violation fail is "no refund, no reset". The ban enforces it while it lasts (`WalletBanned`); `unban_wallet` "does not revive failed accounts" but does not stop a reset. Forbidding it needs a marker on the failed account (a status or a spare field), which would change `fail_for_violation`.
* A-FEE-1 (the 0.4% on resets) and OPEN_QUESTIONS #9 (the price of a reset after a wrongful fail) are unchanged.
