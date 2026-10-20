# M12 item 7: vault boundary (A-3, money) and the R-1 money bound

SELF-audit by the builder, not an independent audit. Prefix of findings: VLT.

File: `tests-rs/tests/vault_boundary.rs` (31 tests: 30 pass, 1 `#[ignore]` FINDING).
Run: `cd tests-rs && cargo test --offline --test vault_boundary -- --nocapture` (about 3.5 s).
Lint: `cargo clippy --offline --test vault_boundary -- -D warnings` is clean.
Real vault fixture `tests-rs/fixtures/core_vault.so`, sha256 `42c6267008ca25cfd7b71792f3797307292049d91420dbba8fe07dc3098c89a6`, not rebuilt (checked again at the end).

Extra files I had to add (test-only helper program for A-3, see below):
- `tests-rs/fixtures/attacker_cpi/{Cargo.toml,Cargo.lock,.gitignore,src/lib.rs}`: a 40-line program that CPIs into the vault and signs with the seeds `["setl8_sector_authority", bump]` under ITS OWN program id.
- `tests-rs/fixtures/attacker_cpi.so` (built with `cargo build-sbf --offline`, sha256 `8b5512762811f6b43a7223fe0ce8145c4f77257bf31f762680773d9c333fb947`). `.gitignore` ignores `tests-rs/fixtures/*.so`, so the `.so` is NOT committed (same as `core_vault.so`). The test panics with the build command if it is missing. The lead must either commit it with `git add -f`, or add the build step to the scripts. The build also writes a throw-away `attacker_cpi-keypair.json` next to the `.so`: I deleted it, nobody should commit one.

## What the harness can and cannot show (read first)

1. LiteSVM runs with signature verification OFF (the vault admins' keys are public constants whose key files are never read). Here an account can be flagged as a signer without a signature. So "random key flagged as signer" really means "somebody who can sign for that key". What the tests prove is the VAULT's check: `Signer` flag plus `key == derive_sector_authority(registry.product_program_id)`.
2. The Solana RUNTIME rule "a program can only sign for PDAs of its own id" is shown with the real helper program (`a3_real_cpi_...`), where the outer transaction does NOT flag the authority as signer.
3. The vault does NOT check a payout amount against the account size. It checks `amount > 0` and a global claims ceiling of 2,500,000 USDC (`OPEN_CLAIMS_CEILING`). The 0.08 S rule lives only in our program (`pass_amount`), so it is exactly as strong as A-3. That is DESIGN 10.4 as written, but it means: if the sector authority could be impersonated, or if our program were upgraded by a bad key (A-11), a single claim of up to $2.5M per record would be accepted by the vault (4 per record).

## Result table

Classes: BLOCKED (exact error asserted, state unchanged), LIMITED (pins exactly what still passes), OFF-CHAIN, POLICY.

| test | what it attacks | class | expected result (asserted) |
|---|---|---|---|
| `a3_deposit_fee_authority_slot` | deposit_fee with 10 wrong authority values: our PDA not flagged signer; random key; PDA of another program id; PDA of the vault id; right program, wrong seed; our config PDA; the trader's wallet; the vault registry; the vault SL8 admin; the evaluator key | BLOCKED | our PDA unsigned: Anchor 3010 `AccountNotSigner`; the other 9: vault `Unauthorized` 6000. World digest (all vault records, registry, pool, token accounts, claims, tally, config) byte-identical. Control: the right PDA flagged signer succeeds and changes the world |
| `a3_deposit_reset_authority_slot` | same 10 values on deposit_reset (record Failed first) | BLOCKED | same codes, same control |
| `a3_record_activity_authority_slot` | same 10 on record_activity (clock moved 90,000 s so the call is not throttled) | BLOCKED | same codes; control refreshes the activity clock |
| `a3_flag_trader_failed_authority_slot` | same 10 on flag_trader_failed | BLOCKED | same codes; control flags the record |
| `a3_request_payout_authority_slot` | same 10 on request_payout | BLOCKED | same codes; control queues the claim |
| `a3_real_cpi_from_another_program_cannot_act_as_this_program` | a REAL second program loaded next to the vault CPIs (flag_trader_failed, record_activity, request_payout) naming OUR sector PDA as authority and signing with our seed string under its own id; then naming ITS OWN PDA against OUR registry and record; control: the helper registered as its own product does a deposit_fee with its own PDA | BLOCKED | (1) runtime `InstructionError::PrivilegeEscalation`, the vault handler never runs; (2) vault `Unauthorized` 6000; (3) control succeeds (the CPI path works, so (1) and (2) are about the authority); helper product cannot touch our record either (`ConstraintSeeds` 2006). Our record, registry and tally bytes unchanged |
| `b_payout_is_exactly_0_08_s_in_phases_2_to_5_for_every_tier` | the amount, for all 10 launch tiers, through the real flow (fabricate, request, confirm, wait the delay, finalize) | BLOCKED by construction | phase 1: event amount 0, no claim, tally 0/0, registry 0/0, vault payout_count 0. Phases 2..5: `PassRequested.amount`, `PassFinalized.amount`, vault claim `owed`, tally, registry total, vault payout_count all equal the hand table (0.08 S: 200, 400, 800, 1,200, 1,600, 2,000, 4,000, 6,000, 8,000, 16,000 USD). A second finalize right after: `NoPendingPass`; request in the next phase: `TargetNotReached`. Chain total = 32% of S, both sides Graduated |
| `b_vault_refuses_a_repeated_or_skipped_request_id_and_a_zero_amount` | direct request_payout with request ids 0, 1 (already used), 3, 4, u64::MAX; amount 0 | BLOCKED (vault) | `RequestIdMismatch` 6009; `ZeroAmount` 6015; world unchanged |
| `b_vault_accepts_any_amount_up_to_the_global_ceiling_so_the_formula_rests_on_the_authority` | a request_payout with amount 25x the account size | LIMITED | ceiling + 1 and u64::MAX: `ClaimsCeilingExceeded` 6043; exactly 2,500,000 USDC is ACCEPTED for a $100k record. Pins the fact in section "What the harness can and cannot show", point 3 |
| `c_record_bound_calls_refuse_a_mismatched_trader_challenge_or_product` | flag / activity / payout with: trader arg B vs record A; trader arg A vs record B; challenge arg 1 vs record 0; record 1 does not exist; product arg X with our registry; our product with X's registry; X's registry+arg with our record; X's registry+arg with an empty record address; (payout) claim address of B's record; claim of the wrong request id | BLOCKED | 26 cells: `ConstraintSeeds` 2006, or `AccountNotInitialized` 3012 when the address holds no record. World and both registries unchanged |
| `c_another_sectors_authority_cannot_touch_our_record_and_ours_cannot_touch_theirs` | a second registered product X in the same vault: X's PDA on our record, our PDA on X's record, X's PDA with our registry | BLOCKED | `Unauthorized` 6000 or `ConstraintSeeds` 2006; X's record bytes and our world unchanged |
| `c_deposit_fee_refuses_mismatched_trader_price_size_token_and_a_reused_challenge` | deposit_fee: signer A with wallet arg B; price 1 micro short / over / 0 / u64::MAX; wrong size; B's token account; challenge 0 bought twice | BLOCKED | `TraderWalletMismatch` 6022; `InvalidChallengeTier` 6006; `InvalidTokenAccount` 6021; second purchase: system `Custom(0)` "already in use". Token balances unchanged |
| `c_deposit_reset_refuses_active_records_wrong_price_phase_trader_and_reused_ids` | deposit_reset on an Active record; prices off by 1 micro, zero, another ladder step; wrong phase index (1, 5, 255); signer B; B's record as previous; new record = previous record; B's address as new record | BLOCKED | `ResetNotAllowed` 6011; `WrongAmount` 6007; `InvalidResetPhase` 6012; `TraderWalletMismatch` 6022; 2006; system `Custom(0)` |
| `c_our_instructions_with_another_traders_vault_record_are_refused_and_change_nothing` | finalize_pass, touch_activity, release_account, request_advance, report_fail of A with B's vault record in the vault slot | BLOCKED | finalize/request/report_fail: vault 2006; touch/release: our own `InvalidVaultAccount` 6103. Digest unchanged; the report_fail batch rolls back (account stays Active); control report_fail with the right record flags the vault |
| `c_finalize_pass_with_a_foreign_registry_claim_or_wallet_is_refused_and_changes_nothing` | finalize_pass with X's registry, a claim address of another request id / another record, B's wallet profile | BLOCKED | 2006 each; control pays |
| `d_a_failed_record_cannot_be_paid_locally_or_at_the_vault` | finalize on a failed account; then the local account forced back to Active+confirmed while the vault record is Failed; then a direct vault call | BLOCKED | local `AccountNotActive`; vault `InvalidTraderStatus` 6005 through our finalize (rolled back, no claim, tally 0); direct call 6005 |
| `d_failed_record_terminal_and_reset_once` | flag twice, activity and payout on a Failed record; second reset of the same record; old record after the reset | BLOCKED | 6005 for flag/activity/payout; second reset `ResetNotAllowed` 6011; old record still unpayable |
| `e_max_payout_count_is_four_and_a_fifth_is_refused` | registry `max_payout_count` = 4; record forced to payout_count 4 while Active; payout_count 3; Graduated record | BLOCKED | fifth on an Active record: `PayoutCapReached` 6008 (direct and through finalize, rolled back); the fourth is accepted and graduates the record (vault status 1, local Graduated); a request on the Graduated record: 6005 |
| `e_a_reset_carries_payout_count_so_the_chain_still_pays_four_times` | try to get a fresh payout allowance by failing and resetting | BLOCKED | after 2 payouts, fail, reset: new record has payout_count 2 and the same size; local next_request_id 3; two more payouts graduate; total over TWO records = 4 x 0.08 S; no fifth on either record |
| `f_vault_pause_stops_money_calls_and_not_the_failure_flag` | DESIGN 10.8 at the vault | BLOCKED / allowed | while paused: deposit_fee, deposit_reset, request_payout `ProductNotActive` 6001, world unchanged; record_activity and flag_trader_failed succeed |
| `g_pre_funded_claim_and_record_addresses_do_not_freeze_an_honest_trader` | griefing: 1 lamport sent to the future claim PDA, the next vault record, the reset record | BLOCKED | finalize, open_account and reset_account still succeed |
| `h_long_delay_is_survivable_only_if_the_activity_clock_is_refreshed_inside_the_delay` | companion of VLT-1 | LIMITED | delay 2 days pays; delay 8 days pays only if the trader's `release_account` (day 4) or the evaluator's `touch_activity` (day 5) ran inside the delay |
| `h_a_confirmed_pass_is_paid_after_any_allowed_delay_...` | VLT-1 | FINDING, `#[ignore]` | see below |
| `r1_compromised_evaluator_without_veto_extracts_exactly_32_percent_of_size_per_chain` | R-1 one chain | measured | see numbers |
| `r1_chain_bound_is_32_percent_for_every_tier` | R-1 for all 10 tiers | measured | see numbers |
| `r1_with_the_veto_on_every_pass_nothing_is_paid` | R-1 with veto | POLICY | zero paid, trace = `vetoes` counter and `PassVetoed` event |
| `r1_a_veto_after_the_finalize_is_too_late_and_changes_nothing` | veto after payment | BLOCKED | `NoPendingPass`, claim stays |
| `r1_many_accounts_are_bounded_by_the_daily_cap_and_by_the_chain_bound` | R-1 sector level, 14 accounts, launch parameters | measured | see numbers |
| `r1_window_bound_is_exactly_the_cap_and_a_burst_at_each_end_gives_cap_times_days_plus_one` | R-1 cap arithmetic | LIMITED | see numbers |
| `r1_the_size_of_the_invented_profit_does_not_change_the_payout` | a fake close of +$999,000 on a $100k account | LIMITED | program accepts b = 11 S; payout is still exactly 0.08 S |
| `r1_the_cap_is_only_as_strong_as_the_window_the_admins_set` | `set_limits` with a 1-second window | LIMITED | accepted (only 0 is refused); two payouts in two seconds with a cap of one |

Counts (31 tests): BLOCKED 21, LIMITED 5, POLICY 1, measured R-1 3, FINDING 1 (ignored). Refused cells asserted with an exact code: 50 authority cells (5 instructions x 10 values), 6 real-CPI cells (3 instructions x 2 attempts), 26 mismatch cells in `c_record_bound_...`, plus the rows of the other `c_`, `d_`, `e_`, `f_` tests. OFF-CHAIN: nothing in this item lives off-chain; the only part LiteSVM cannot show by itself is the runtime rule, covered by the real helper program.

## R-1 numbers (measured, micro-USD; printed with `-- --nocapture r1_`)

Setup: the vault is the real fixture; launch values: delay 172,800 s, cap 100,000,000,000 per window, window 86,400 s (decision 37). The compromised evaluator holds the real evaluator key and posts consistent fake journal batches (a close with +0.08 S, flat), accepted by `post_journal` (the program tracks books, not positions).

| quantity | measured | DESIGN 13.1 R-1 | match |
|---|---|---|---|
| one record chain, $100k tier, veto NOT used | 4 claims of 8,000,000,000 = **32,000,000,000 = 32.0000% of S** (tally, registry total and the sum of the claims agree) | 32% of size per record chain | yes |
| every tier, same test | 32.0000% of S for all 10 tiers; the largest chain is the $200k tier: **64,000,000,000** | DESIGN 10.6 "largest sum of the vault cap per account is $64,000" | yes |
| veto used on every pass (6 fabricated passes, vetoed in state 1, right after confirm, 1 s before the delay ends, at the second the delay ends) | **0 micro-USD (0.0000%)**; window_total untouched; `vetoes` = 6 | zero | yes |
| payout for a phase already paid | impossible: `NoPendingPass`, then `TargetNotReached`; vault `RequestIdMismatch` | | yes |
| fail + reset to extend the chain | does not extend it (vault copies payout_count): 4 payouts per chain over any number of records | | yes |
| pacing of one chain | first payout 172,850 s (2 days) after the start, last 691,400 s (8 days) after the start; two payouts of one chain are never closer than the delay | | |
| attacker outlay per chain (price + 0.4% fee) | $100k tier 862,436,000 (0.8624% of S), so net 31,137,564,000; $200k tier 1,605,396,000 | | |
| size of the invented profit | irrelevant: +$999,000 fake profit pays 0.08 S | | |
| many accounts, no veto, cap in force (14 accounts of $100k, three wallets, tick = 88,400 s) | total 448,000,000,000 = 14 x 0.32 S exactly; **no window above the cap**; first payout tick pays 96,000,000,000 (12 x 8,000) and the cap refuses the other 2; max in one tick 96% of the cap; finished in 10 ticks | "bounded by the daily cap" | yes |
| cap arithmetic with a mix of tiers | the evaluator reaches the cap **exactly** (6 x 16,000 + a 7th 16,000 refused + 4,000 = 100,000,000,000); one second before the window ends everything more is refused; the window rolls at exactly `window_start + 86,400` | | |
| sector bound over D days | fixed windows allow a burst at each end: 4 consecutive windows paid **400,000,000,000** in 3 x 86,400 s, i.e. **cap x (D + 1)** for D days (DESIGN 10.6 already says "up to twice the cap across a window boundary"); a closed 24 h interval can hold two caps | | consistent |

Derived arithmetic (not tested, labelled as such): at the launch values a compromised evaluator with no veto can queue about $100,000 of claims per full day regardless of the number of accounts. To use the whole cap it needs a pipeline of at least 25 accounts of $100k (one $8,000 payout per account per 2 days) or at least 13 accounts of $200k, and about 1.56 fresh $200k chains per day, an outlay of about $2,500 per day. So after N accounts are paid in, the per-day bound is the cap, not N x 0.32 S; the per-chain bound is what stops it from using one account twice. The vault queues CLAIMS; tokens move at the heartbeat, pro rata from the pool, so the real loss is also bounded by the pool balance, which this item does not measure.

Verdict on R-1: the 32% number is REPRODUCED to the micro-USD. No mismatch with DESIGN 13.1.

## Findings

### VLT-1 (low, needs a DESIGN change): a long pass delay makes the vault abandon an honest confirmed pass

`set_limits` accepts `pass_delay_secs` from 1 to 14 days (DESIGN 14). The vault abandons a record idle for more than 7 days (`INACTIVITY_LIMIT_SECS` = 604,800). A frozen account (state 2) gets no orders, `confirm_pass` does not touch the vault, and the evaluator's `touch_activity` follows orders only, so the activity clock is the last request or order. With a delay above about 7 days (more precisely: activity-to-finalize gap above 604,800 s), `finalize_pass` reaches `request_payout`, which flips the record to Abandoned and returns Ok(Abandoned); our program marks the account Abandoned: the honest trader loses the account and the 0.08 S, with no error. Reproduced with an 8-day delay (status 2, no claim, tally 0). It needs both admins to choose such a delay (the default is 2 days), but the range is written in the DESIGN. Mitigation that exists today: `release_account` by the trader or `touch_activity` by the evaluator inside the delay (pinned in `h_long_delay_is_survivable_...`). Fix options, both DESIGN changes: lower `MAX_PASS_DELAY_SECS` below the idle limit (6 days), or make `confirm_pass` refresh the vault activity (new CPI accounts in `confirm_pass`). Test: `h_a_confirmed_pass_is_paid_after_any_allowed_delay_even_longer_than_the_vault_idle_limit`, `#[ignore = "FINDING VLT-1: ..."]`; `cargo test --offline --test vault_boundary -- --ignored` fails it with `the honest account must not be Abandoned by a valid pass` (left 2 = Abandoned, right 0 = Active), the reason stated here.

Related, already known (R-3): an evaluator outage of more than 5 days between `request_advance` and `confirm_pass` produces the same loss at the default delay.

### Observations that are NOT findings
- The vault does not tie the payout amount to the account size (see point 3). The control is A-3, which is BLOCKED (tested), plus the A-11 upgrade key duty.
- Fixed windows: cap x (D + 1) (DESIGN 10.6 already states it).
- `window_secs` of 1 second is accepted by `set_limits`: both admins can neutralise the cap. Pin the launch values (cap 100,000,000,000, window 86,400) in the deploy checklist and verify them with a read after `init_config`.
- Pre-funding of claim and record addresses does not freeze anybody.

## What I could NOT test, and why
- The runtime rule "a program can only `invoke_signed` for its own PDAs" is the Solana runtime, not our code. It is shown with a real helper program for the refusal path; the positive path (our program signing for our PDA) is exercised by every finalize/open/report_fail test. A real signature check of the PDA by the transaction is not possible because nobody can sign for a PDA outside a CPI; with signature verification off in the harness, the "flagged signer" cells would pass for the wrong reason, which is why the helper's outer instruction never flags the authority.
- Real token movement at the heartbeat (`settle_claims`), pool balance as a bound on payouts, `reconcile_product`: not in this item.
- A mainnet vault build: the fixture is the localnet build of vault commit 9a5c179.
- The pass-flow with a REAL evaluator process (the engine-driven honest path) is item 8; here the dishonest evaluator is the journal builder of the test harness.
