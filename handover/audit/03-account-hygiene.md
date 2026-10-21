# M12 item 3: account confusion and account hygiene

SELF-AUDIT by the builder (not an independent audit). Test file: `tests-rs/tests/account_hygiene.rs` (48 tests, clippy `-D warnings` clean).
Finding prefix HYG. **Result: no finding.** Every account-list mutation that was tried is either refused with a named error and no
change of any byte, or accepted for a reason that is written down in the test and checked (section 5).

How to run: `cd tests-rs && cargo test --offline --test account_hygiene` (about one minute wall on 10 cores, the 25 table tests run in parallel).
Wider run: `HYG_FULL=1 cargo test --offline --test account_hygiene -- table_` (about 135 s). Row numbers:
`HYG_COUNT=1 cargo test --offline --test account_hygiene -- --nocapture row_counts_are_pinned`; result numbers:
`HYG_REPORT=1 cargo test --offline --test account_hygiene -- --nocapture report_summary`.

## 1. Method, in plain words

1. For each of the 25 instructions of the program there is one *scenario*: a world (real vault fixture, 5 markets, one trader with one
   $100k account, built by `JEnv`) in which the correct call succeeds. The instruction is made by the real builders of
   `tests-rs/tests/common` (`accounts::X { .. }.to_account_metas`, `OpenAccounts`, `ResetAccts`, `FinalizeAccts`, `JEnv::*_ix`).
   A new instruction without a scenario fails `every_instruction_of_the_program_has_a_scenario_and_vice_versa`.
2. Every account slot of every instruction has a hand-written *type* (`Ty`, 37 kinds, 41 with their `mut` variants): what the Anchor struct and the handler demand of
   it, read from `programs/setl8-lev-trading/src/instructions/*.rs` and the vault's account structs (vault commit 9a5c179).
3. The generic mutations are applied one at a time to a fresh copy of the world (`LiteSVM::clone`):
   wrong owner; lookalike of the right size and discriminator at another address owned by this program / the vault; the same owned by a
   foreign program; closed account; closed account refunded with lamports (the revival shape); a key nobody ever created; balance one
   below rent exemption; wrong discriminator; data cut to 4 bytes; flipped stored bump; a lookalike at the NON-canonical bump address;
   writable -> read-only; read-only -> writable; signer -> non-signer; non-signer -> signer; the key of another slot copied into a slot
   (a duplicate); two adjacent slots swapped; a random key or a foreign EXECUTABLE program in a program slot; accounts cut off the end
   (every length from 0 to n-1); one account too many; another trader's valid account in the trading-account slot.
4. The expected result of every row is computed by a hand-written model (`expect()` and `Mx` in the test file, about 500 lines) from the slot type
   and the state of the mutated accounts. It is NOT read from the program run. It encodes the order in which Anchor 0.32 works (phase 0:
   deserialise every slot in order; phase 1: the `init` fields first, then all other constraints in field order, in the order seeds, mut,
   custom constraint, owner, address; phase 2: the handler; phase 3: the CPI into the vault) and the handler / vault checks of each slot.
   Where two slots are wrong at once (duplicate, swap) the earliest in that order wins.
   The model was first written from the sources, then run against the program; where it disagreed the model was corrected only after the
   reason was found in the Anchor or vault source (about eight corrections in the first runs, all in the model: for example "an `init_if_needed`
   account that exists is checked for its seeds before its discriminator", "the system program is not made read-only by LiteSVM",
   "a vault slot that holds the address of the account this program just created sees a program-owned account"). Since the final model
   agrees with the program on every row, the model is also a compact statement of how the account validation works.
5. A refused row must carry the exact `InstructionError` (an Anchor / vault / program code, or a named runtime error such as
   `PrivilegeEscalation`) AT THE RIGHT INSTRUCTION INDEX, and the digest of every involved account must be byte-identical afterwards
   (config, payout tally, wallets, markets, rule sets, trading accounts of the trader, vault registry / state / records / claims, every
   token account of the world, lamports of every key in the instruction; only the fee payer of the transaction is left out, it pays the fee).
   An accepted row needs a written reason (`ok_reason`) and, for the flag-only reasons, the account it touched must be unchanged.
6. Signature verification is OFF in this world (the vault harness cannot sign for the vault admins): a signer flag without a signature
   reaches the program, which is what is under test. Real signatures are covered by `who_may_call.rs`.

## 2. Counts

| what | number |
|---|---|
| instructions with a scenario | 25 of 25 (all `pub fn` of the `#[program]` block, checked against `lib.rs`) |
| slot types in the model | 37 kinds, 41 with their `mut` variants (every one used by a scenario, checked) |
| table rows, default run | 2411 |
| table rows, `HYG_FULL=1` (every slot as donor of a duplicate) | 3644 |
| of the default rows: refused with an exact code | REFUSED_N |
| of the default rows: accepted for a written reason | OK_N |
| rows where the program disagreed with the model at the end | 0 |
| dedicated tests | 23 (87 further exact refusals inside them, listed in section 4) |
| tests in the file | 48 (25 table tests + 23 others), all passing |

Rows per instruction (default / full): init_config 48/64, set_evaluator 34/35, set_limits 34/35, set_paused 34/35, create_ruleset 61/74,
create_market 61/74, update_market 51/55, ban_wallet 61/74, unban_wallet 52/56, open_account 249/556, reset_account 307/669,
post_journal 98/115, request_advance 136/186, cancel_pass 22/22, confirm_pass 59/64, reject_pass 59/64, veto_pass 71/84,
finalize_pass 193/315, report_fail 175/257, hold_account 79/89, release_hold 90/111, fail_for_violation 143/216, touch_activity 136/186,
release_account 136/186, close_account 22/22. The totals are pinned by `row_counts_are_pinned`: widening or narrowing the mutation
set is a deliberate edit (the numbers also move if someone changes the shared harness defaults).

Refusals by error (default run): REFUSAL_TABLE

## 3. What each family of rows shows (class: BLOCKED = exact refusal proven, LIMITED = accepted and pinned)

| mutation | which slots | expected result | class |
|---|---|---|---|
| wrong owner (same address, data intact) | every program-owned or vault-owned account | `WrongOwner` 3007 (Anchor `Account<T>`) | BLOCKED |
| lookalike of right size and discriminator owned by a foreign program, at another address | same | 3007 | BLOCKED |
| the same, owned by this program / the vault, at another address (a "TradingAccount of another sector" or a copy) | PDAs | `ConstraintSeeds` 2006; for `ruleset` in open/reset the handler's `RuleSetMissing`; for the vault records the vault's 2006 or our `InvalidVaultAccount` | BLOCKED |
| closed (0 lamports) or a key nobody created | every account slot | `AccountNotInitialized` 3012 (typed accounts); `InvalidVaultAccount` where the handler reads the vault record first | BLOCKED |
| closed and refunded with lamports (system-owned, empty) | same | 3007 | BLOCKED |
| wrong discriminator / data cut to 4 bytes | typed accounts | 3002 / 3001 (`InvalidAccountData` for SPL accounts) | BLOCKED |
| stored bump flipped | accounts with a stored bump | 2006 (`RuleSetMissing` for open/reset) | BLOCKED |
| lookalike at the NON-canonical bump address | `sector_authority` (checked with the canonical bump) | 2006 | BLOCKED |
| the same | accounts with a stored bump (config, trading account, wallet, rule set, market) | accepted (the stored-bump seeds check passes) | LIMITED, see 5 |
| writable account passed read-only | every `mut` slot | 2000 `ConstraintMut`; for accounts the program CREATES: runtime `PrivilegeEscalation` (the `init` CPI runs before the `mut` checks) | BLOCKED |
| read-only account passed writable | every non-`mut` slot | accepted, account unchanged | LIMITED, see 5 |
| signer flag removed | every `Signer` | 3010 | BLOCKED |
| signer flag added | any non-signer | accepted, account unchanged | LIMITED |
| key of another slot copied into a slot / two slots swapped | all pairs in the full run, donors 0, trading account, config, neighbours in the default run | the error the slot gives for an account of that kind (3007, 3002, 3012, 2006, 6000, 6040, 6041, 6021 ...), computed by the model | BLOCKED (accepted only for unchecked slots, see 5) |
| program slot gets a random key / the vault / the other of system and token program | `system_program`, `token_program` | `InvalidProgramId` 3008 | BLOCKED |
| accounts cut off the end, every length | all | `AccountNotEnoughKeys` 3005; for the market of post_journal / report_fail the handler's error (see 4) | BLOCKED |
| one account too many | all | ignored by Anchor; `MarketUnknown` for post_journal / report_fail, which read every remaining account as a market | BLOCKED / LIMITED |
| another trader's valid account in the trading-account slot | all 15 TA instructions | `Unauthorized` for trader instructions; 2006 where a wallet slot is bound to the account; `NoPendingPass` / `BadPrevHash` where the evaluator is allowed to act on any account and the state refuses | BLOCKED |

## 4. Dedicated tests (each asserts exact code, instruction index and byte-identical accounts)

| test | what it attacks | class | result |
|---|---|---|---|
| `reinit_open_account_cannot_overwrite_an_existing_trading_account` | wallet counter wound back to 0 so the new-account address is account 0, which exists; the vault record exists too | BLOCKED | system `AccountAlreadyInUse` Custom(0), account 0 bytes unchanged |
| `reinit_open_account_naming_the_existing_account_is_a_seeds_error` | existing account 0 named in the new-account slot | BLOCKED | 2006 |
| `reinit_init_config_twice_is_refused_and_changes_nothing` | init_config again with both real admins and other parameters | BLOCKED | Custom(0), config and tally unchanged |
| `reinit_create_ruleset_and_create_market_twice_are_refused` | version 1 / market 0 again with different content | BLOCKED | Custom(0) |
| `ban_wallet_on_an_existing_wallet_keeps_its_counters` | `init_if_needed` on an existing wallet | LIMITED | only `banned` changes; `next_challenge_id` 1, `active_count` 1, `trader`, `bump` survive |
| `closed_address_reused_in_the_same_transaction_is_not_initialized_for_every_instruction` | `[close_account, X]` in ONE transaction, X = each of the 15 instructions that take the account | BLOCKED | 3012 at instruction 1 for all 15; the atomic failure leaves the account open |
| `revived_closed_address_is_a_system_account_for_every_instruction` | `[close, transfer lamports to the dead address, X]`, then the revival alone, then X in a later transaction | BLOCKED | 3007 in all 15 x 2 cases; the revived address stays an empty system account |
| `a_closed_account_address_is_never_recreated_by_open_account` | open_account naming a closed address (same transaction and later); a correct open afterwards | BLOCKED | 2006 / 2006; the new account is id 1, address 0 stays empty |
| `a_prefunded_future_address_does_not_block_account_creation` | lamports sent to the future trading account, vault record, wallet profile, config and payout tally (DoS by pre-funding) | BLOCKED (the DoS does not work) | creation succeeds |
| `a_fake_clock_account_changes_nothing` | an account with a Clock layout and a far-future time as an extra account of finalize_pass and cancel_pass | BLOCKED | `PassDelayNotElapsed`, `CancelTooEarly`; no instruction has a sysvar slot, time comes from the syscall |
| `stored_bumps_are_canonical` | creation paths | evidence | config, trading accounts (first and later), wallet, rule set, markets all store the canonical bump |
| `money_slots_cannot_be_redirected_in_open_and_reset` | another person's token account as the buyer's source; the buyer's own account as the fee destination, the pool or the vault SL8 destination; pool and SL8 swapped; fee and SL8 swapped | BLOCKED | 6040 (ours) for source and fee, 6021 (vault) for pool / SL8; 12 refusals, plus the correct call succeeds in the same worlds |
| `wrong_mint_and_wrong_token_program_are_refused_in_open_account` | USDT mint with USDC accounts; USDT everywhere but the USDC pool; a fake mint with matching accounts; Token-2022 / system program as token program; token program as system program | BLOCKED | 6040, 6021, vault `InvalidMint` 6017 (the fee transfer that already ran is rolled back), 3008, 3008, 3008 |
| `usdt_in_every_slot_is_a_valid_alternative_and_moves_only_usdt` | USDT in every slot | LIMITED | accepted by design (the vault's second mint); no USDC balance moves |
| `more_than_eight_remaining_market_accounts_are_refused` | 9 remaining accounts in post_journal and report_fail | BLOCKED | `BatchTooLarge`; 8 are accepted |
| `valid_accounts_of_another_challenge_or_trader_in_the_wrong_slot_are_refused` | the vault record of the trader's OTHER challenge, the wallet of ANOTHER trader, the claim of the NEXT request, in 6 instructions | BLOCKED | 13 refusals: 2006 (wallet, vault record, claim), `InvalidVaultAccount` for touch_activity / release_account |
| `every_instruction_of_the_program_has_a_scenario_and_vice_versa`, `every_slot_type_is_used_by_a_scenario`, `the_table_runner_reports_wrong_expectations_dirty_refusals_and_unexplained_acceptances`, `row_counts_are_pinned`, `report_summary` | the harness itself | | the table cannot pass vacuously (a wrong expectation, a dirty refusal and an unexplained acceptance each produce a problem); a new instruction fails until it has a scenario |

## 5. Accepted rows and why that is acceptable (all pinned by the test, reason strings in `ok_reason`)

* **FLAG, read-only passed writable / extra signer flag.** Anchor does not reject it; the program never writes such an account; the
  test checks that the account is byte-identical afterwards.
* **RENT, balance below rent exemption.** The runtime lets a rent-paying account be read or written when its size does not change and its
  lamports do not fall. `init_if_needed` accounts ARE checked (`ConstraintRentExempt` 2005, row `BelowRent` on the wallet of open_account).
  Accounts of this program are created rent-exempt and only the program can debit them (only `close_account` does, to zero), so the
  state cannot arise on a cluster; the rows show that the program has no extra defence.
* **BUMP, lookalike at a non-canonical bump** for config, trading account, wallet, rule set, market. These accounts are checked with
  `bump = <stored bump>`; a program-owned copy at the non-canonical off-curve address would be accepted. It cannot exist: only this program
  can create an account at an address derived from its seeds (nobody holds a key for an off-curve address), and every creation uses the
  canonical bump (`stored_bumps_are_canonical`). If a later change ever lets a caller choose a bump, this row turns into a refusal test.
  `sector_authority`, which is checked with the canonical bump, refuses the non-canonical address (2006); `event_authority` and the payout tally are canonical-checked too (a wrong key is 2006 in the `Missing` rows) but have no data to copy, so no non-canonical row exists for them.
* **TOKEN, any token account with the right owner and mint** as the buyer's source, the fee destination or the vault's SL8 destination.
  Token accounts are not PDAs; the constraints are owner and mint (6040 ours, 6021 the vault). A copy of the buyer's own account is the
  buyer's own account for all purposes.
* **ADMIN.** The two admin slots are not deserialised; only key and signer flag matter. One signature is enough for create_market,
  update_market and pause (DESIGN 6.1, 14), so replacing the other admin slot by anything is accepted there and refused (6000) elsewhere.
* **EXTRA / MARKET.** A trailing account that no slot reads is ignored. post_journal does not need the market account for a batch that names
  no market (a Meta-only batch), so cutting it off is accepted there; report_fail needs it (`ProofWrongFeed`).
* **EVENT.** The `program` slot that `#[event_cpi]` adds has no constraint at all and is never read.
* **PAYER / ADMIN KEY.** Any funded signer can pay rent; the compiled-in admin slot of init_config is `Signer` + address, its balance is irrelevant.

## 6. Observations (information only, not findings)

* **O-1.** In open_account / reset_account the check `payer == trader` (Unauthorized) runs in the handler, AFTER Anchor has created the new accounts
  with the payer's lamports. A payer slot that holds a key which is not a funded system wallet therefore ends in a system / runtime error
  (`InvalidArgument`, Custom(1), `PrivilegeEscalation`) instead of `Unauthorized`. The refusal and the unchanged state are the same; only the
  code differs (rows `Dup`/`Swap` on the payer slot). Unauthorized for a funded different payer is pinned in `who_may_call.rs`.
* **O-2.** The stored-bump pattern means the program does not re-derive the canonical bump on every call (cheaper); see BUMP above.
* **O-3.** `finalize_pass` takes any payer and pays the claim rent from it; an attacker who finalizes someone's pass pays for the claim account. No
  money of the trader is involved (DESIGN 6.4).

## 7. What could NOT be tested here, and why

* **Real signatures.** The vault world has signature verification off; the program's own signer checks are what the rows test. The runtime
  rejecting a missing signature before the program runs is not shown here (`who_may_call.rs` runs the admin and trader paths with real signatures).
* **A non-canonical lookalike cannot be created through the program or the system program**, so only the acceptance on a hand-made account is pinned.
* **Token-2022.** The program id is refused by the key comparison (3008); a real Token-2022 program is not loaded, so a CPI into it is not exercised.
* **Sysvar from a fake account.** No instruction has a sysvar account slot (the program calls `Clock::get` / `Rent::get`); `a_fake_clock_account_changes_nothing`
  shows that a fake clock passed as an extra account has no influence. LiteSVM would not let the real clock address hold different data (the sysvar cache wins),
  so the fake is at a random address.
* **Runtime demotion of read-only keys.** This LiteSVM version demotes only the invoked program id; on a cluster more reserved keys are read-only. It
  changes rows that flag the system or token program writable (accepted either way) and the payer rows in section 6 (O-1), not any control.
* **Vault codes** (2006, 3007, 3012, 6017, 6021) are those of the vault fixture sha256 `42c62670...c89a6` (vault commit 9a5c179); another vault build can change them.
* **Compute units** of the refusing paths were not measured (budget.rs owns the CU ceilings).
* **Mutation testing of this suite** (item 10) is the lead's: the tests accept an alternative `.so` through `SETL8_TEST_SO` like the rest of the suite.

## 8. Findings

None (prefix HYG). No test is `#[ignore]`d.
