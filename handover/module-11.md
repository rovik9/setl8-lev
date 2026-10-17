# Module M11: measured budgets (P19 part C)

Every number of DESIGN section 11 that was an estimate is now measured, and a test fails the build when an instruction grows past its budget. **No instruction exceeds 200,000 compute units or 1,232 bytes, and the SBF stack check passes** (largest frame 2,944 bytes of 4,096), so nothing was stopped and nothing was optimised.

## How it is measured

* `tests-rs/tests/budget.rs`, test `measured_budgets_of_every_instruction` (`cargo test --test budget -- --nocapture` prints the table; about 2 minutes). It runs every instruction in the real LiteSVM world (real vault, real SPL token) in the shapes that cost the most and records, from the transaction metadata and the signed transaction: compute units consumed, serialized size, distinct accounts, signatures and the deepest invocation level.
* Shapes. Admin instructions: fee payer plus both admins (3 signatures, the largest form). `post_journal`, `report_fail`, `confirm_pass`, `reject_pass`, `hold_account`, `touch_activity`: the evaluator alone, as fee payer, one signature, NO compute-budget instruction (the SPLIT rule). Trader instructions: the trader (also the payer). `post_journal` is run as the grid 1, 4 and 8 events x 0 to 3 proofs x sources 2, 3 and 4 (36 shapes), every event kind alone (Fee, Fill, Close, TightStart, FloorRaised, TightEnd, Breach, Meta; Fail through `report_fail`), the heaviest mix (8 events, 3 proofs: 2 fills, 2 closes, tight start, floor raise, fee, meta), and the largest transaction (3 proofs and all 8 market accounts, also with source-3 digests). `report_fail` is run with 2 and 8 events and with 3 proofs (source 2 and 3). The differential campaign (`DIFF_PATHS`, 150 paths in `cargo test`, 10,000 in the gate) is merged into the same rows; in a 300-path run its largest `post_journal` was 116,654 CU and 1,055 B and its largest `report_fail` 132,818 CU and 1,082 B, both below the hand-built worst cases.
* **The cost depends on the trader's key.** `find_program_address` loops from bump 255 down; each failed attempt costs about 1,500 CU, and `open_account`, `reset_account`, `finalize_pass`, `touch_activity` and `release_account` derive several addresses (the program's and the vault's). The harness uses traders of FIXED keys (the same numbers in every run): the first 4 to 24 seeds and the 8 keys, out of the first 8,000 seeds, whose addresses need the most attempts (a hand-built worst case: `bump_depth` in `budget.rs`). The worst key adds about 20,000 CU over a typical one for `reset_account` (typical 125,000 to 145,000, worst 158,200).
* Ceilings: `CEILINGS` in `budget.rs`, each = measured worst case + 15 %, rounded up to 100 CU and 10 bytes, never above 200,000 CU or 1,232 bytes. The test fails when a measurement exceeds its ceiling, when a ceiling exceeds a limit, when an instruction has no ceiling, or when a ceiling has no measurement. After a budget changes ON PURPOSE run `BUDGET_CALIBRATE=1 cargo test --test budget -- --nocapture`, which prints the new ceilings instead of asserting them, and update this table in the same commit.
* Stack: `scripts/build-test-so.sh` fails on any frame over 4,096 bytes (output: `OK: no stack-frame overflow (localnet build)`). The toolchain prints no sizes, so `scripts/stack-frames.py` rebuilds with `-Zemit-stack-sizes` into `target/stack-sizes/` and names the compiler-reported frame of every function.

## The table

CU = the worst case measured; bytes = the largest signed transaction; accounts = distinct accounts in the transaction (programs included); depth = deepest invocation level (1 = the instruction itself, 2 = the `emit_cpi!` self-call or one vault call, 3 = the vault's own token or system call). The last two columns are the asserted ceilings and the compute-unit limit a client should request (the ceiling; the default of 200,000 also works for every row, see below).

| instruction | CU worst | tx bytes | accounts | sigs | depth | ceiling CU | ceiling bytes | worst case |
|---|---|---|---|---|---|---|---|---|
| `init_config` | 16,050 | 583 | 7 | 3 | 2 | 18,500 | 680 | both admins + payer |
| `set_evaluator` | 4,394 | 436 | 5 | 3 | 1 | 5,100 | 510 | |
| `set_paused` | 4,285 | 405 | 5 | 3 | 1 | 5,000 | 470 | pause |
| `set_limits` | 4,365 | 420 | 5 | 3 | 1 | 5,100 | 490 | |
| `create_market` | 16,815 | 560 | 7 | 3 | 2 | 19,400 | 650 | six sessions |
| `update_market` | 8,031 | 525 | 6 | 3 | 1 | 9,300 | 610 | feed remap, six sessions |
| `create_ruleset` | 31,823 | 984 | 7 | 3 | 2 | 36,600 | 1,140 | the 552-byte rule set |
| `ban_wallet` | 12,049 | 535 | 7 | 3 | 2 | 13,900 | 620 | wallet profile created |
| `unban_wallet` | 6,075 | 469 | 6 | 3 | 1 | 7,000 | 540 | |
| `open_account` | 135,595 | 838 | 20 | 2 | 3 | 156,000 | 970 | first account of a wallet, worst key |
| `reset_account` | 158,200 | 911 | 22 | 2 | 3 | 182,000 | 1,050 | failed in phase 1, worst key |
| `post_journal` | 125,276 | 1,181 | 14 | 1 | 2 | 144,100 | 1,232 | heaviest mix (CU); 3 proofs of source 3 and 8 markets (bytes) |
| `request_advance` | 45,937 | 539 | 11 | 2 | 2 | 52,900 | 620 | |
| `cancel_pass` | 4,699 | 307 | 4 | 2 | 1 | 5,500 | 360 | |
| `confirm_pass` | 23,971 | 374 | 6 | 2 | 2 | 27,600 | 440 | phase 2 to 5 (phase 1 starts phase 2 at once) |
| `reject_pass` | 21,688 | 375 | 6 | 2 | 2 | 25,000 | 440 | |
| `veto_pass` | 22,643 | 471 | 7 | 3 | 2 | 26,100 | 550 | |
| `finalize_pass` | 84,594 | 671 | 15 | 2 | 3 | 97,300 | 780 | phase 2 to 5 (phase 5 graduates), worst key |
| `report_fail` | 151,317 | 1,149 | 12 | 1 | 2 | 174,000 | 1,232 | 8 events, 3 proofs, source 3 (the 15 % would pass the byte limit; capped) |
| `hold_account` | 24,633 | 439 | 7 | 2 | 2 | 28,400 | 510 | |
| `release_hold` | 24,600 | 504 | 8 | 3 | 2 | 28,300 | 580 | |
| `fail_for_violation` | 44,035 | 668 | 12 | 3 | 2 | 50,700 | 770 | held account |
| `touch_activity` | 52,145 | 539 | 11 | 2 | 2 | 60,000 | 620 | with the vault call, worst key (a throttled call is about 22,000) |
| `release_account` | 52,151 | 539 | 11 | 2 | 2 | 60,000 | 620 | with the vault call, worst key |
| `close_account` | 3,337 | 307 | 4 | 2 | 1 | 3,900 | 360 | |

`post_journal` by shape, as the 300-path campaign saw it (CU / bytes): 1 event 31,208 / 440; 4 events 64,827 to 79,389 / 533 to 952; 7 events up to 116,654 / 1,055; the hand-built heaviest mix 125,276 CU; 3 proofs of source 3 with 8 markets 1,181 bytes. Source 3 costs 32 bytes more per proof (the digest) and about 100 to 1,000 CU more than sources 2 and 4. `report_fail` is `post_journal` plus the vault's `flag_trader_failed` call (+20,000 to +40,000) and 5 more fixed accounts.

## Compute-unit request per instruction

Every row is below the default of 200,000 units per instruction, so **no `SetComputeUnitLimit` is needed anywhere** and `post_journal` and `report_fail` carry none (an extra instruction would add 40 bytes to a transaction that is already at 1,181 and 1,149). A client that prefers to ask for exactly what it needs asks for the ceiling of its row. Two rows deserve a margin: `reset_account` (ceiling 182,000) and `open_account` (156,000) depend on the trader's key, so a hand-picked key far beyond the 8,000 searched could cost another 10,000 to 20,000 units; asking for 200,000 explicitly is the safe request for both. Priority fees are out of scope.

## Close to a limit, and why

1. **`post_journal` bytes: 1,181 of 1,232 (96 %).** The worst case is three proofs of source 3 (93 bytes each with the 32-byte digest) and eight market accounts (32 bytes each as account keys). The evaluator's batch policy (`BatchPolicy::MAX`: 8 events, 3 proofs, 8 markets, 1,232 bytes, 200,000 CU) keeps every batch inside it; `crates/evaluator/src/batch.rs` models the size exactly (one signature, no budget instruction) and the compute with a bound that this table stays below (30,000 + 12,500 per event + 8,000 per proof, + 40,000 for `report_fail`).
2. **`report_fail`: 1,149 B (93 %) and 151,317 CU (76 %).** Eleven fixed accounts (the wallet, the sector authority, the vault program, the registry, the vault record) and the vault CPI on top of the batch.
3. **`reset_account`: 158,200 CU (79 %).** The vault's `deposit_reset` is about 42,000 of it; the rest is the new addresses found by bump search, the token transfers and account creations, and the registry decoded by hand. Its stack frame (2,944 bytes) is also the largest of the program.
4. `create_ruleset`: 984 bytes (80 %); the arguments are the whole 552-byte rule set (the 128 half-spreads are 256 bytes), so it needs a transaction of its own, as DESIGN 6.1 already said.

## Wire sizes (replace "a proof is about 300 bytes", DESIGN 4.9 and 8.2)

Borsh, measured by `encoded_sizes_of_events_and_proofs` (pinned): `Fill` 66, `Close` 67, `Fee` 20, `TightStart` 50, `FloorRaised` 26, `TightEnd` 9, `Breach` 26, `Fail` 19, `Meta` 76 bytes; a proof is **61 bytes** for sources 2 and 4 and **93** for source 3; a batch adds 48 bytes of header and length prefixes. A legacy transaction adds 1 + 64 (one signature) + 3 + the account keys (32 each) + 32 + the instruction. So the worked example of DESIGN 4.9 fits in two transactions (a batch carries at most three proofs), and size is not the limit that makes batches split: the proof count, the event count and the market count are.

## Stack

Largest frames the compiler reports (`scripts/stack-frames.py`, localnet build, 469 functions, limit 4,096 per frame): `handle_reset_account` **2,944**, `ResetAccount::try_accounts` 2,752, `OpenAccount::try_accounts` 2,560, `handle_finalize_pass` 1,984, `ReportFail::try_accounts` 1,856, `FinalizePass::try_accounts` 1,856, `CreateRuleset::try_accounts` 1,728, `handle_open_account` 1,664, then 1,408 and below. None is over 4,096; the build check passes. (The default build, without the `localnet` feature, differs only in the constants.)

## Rent, and what one account costs to open

Rent-exempt minimum = (128 + length) x 6,960 lamports (the LiteSVM cluster; read the live rate before a mainnet budget):

| account | bytes | lamports |
|---|---|---|
| `TradingAccount` | 207 | 2,331,600 |
| `WalletProfile` | 56 | 1,280,640 |
| `Market` | 128 | 1,781,760 |
| `RuleSet` | 552 | 4,732,800 |
| `SectorConfig` | 270 | 2,770,080 |
| `PayoutTally` | 25 | 1,064,880 |
| vault `TraderState` | 115 | 1,691,280 |

Opening the first account of a wallet at the $100,000 tier, end to end: **5,303,520 lamports of rent** (TradingAccount 2,331,600 + WalletProfile 1,280,640 + vault TraderState 1,691,280) **+ 5,000 lamports** signature fee (the trader is the payer, one signature) = **5,308,520 lamports (0.0053 SOL)**, and **862.436000 USDC**: the tier price 859.000000 plus the 0.4 % fee 3.436000, split by the vault. A second account of the same wallet pays 4,022,880 lamports of rent (no profile) + 5,000. Closing an account after 7 days returns its 2,331,600 to the trader (`close_account`; the vault record stays).

## DECISIONS where the prompt was silent

1. **Account count** = distinct accounts in the signed transaction (the program ids included), because that is what the size depends on; the per-instruction account list is in each module's account table.
2. **Depth** counts the instruction itself as 1; `emit_cpi!` is a self-invocation (2).
3. **Ceilings are rounded up** to 100 CU and 10 bytes, and capped at the limits; `post_journal` and `report_fail` reach the byte limit, which is the honest statement that their bytes ARE the budget.
4. **Keys:** the worst-case search uses bump depth of the program's and the vault's addresses for account ids 0 and 1 and request ids 1 to 4; it is a hand-built worst case, not a proof that no key is worse.
5. The campaign uses 150 paths in `cargo test` (10,000 in the gate, whose larger shapes are already in the grid).
6. **Not measured:** priority fees, address lookup tables, compute units of the mainnet vault build (the fixture is the localnet build of core commit 9a5c179).

## P21 (decision 64): what the unpriced-market check cost
`post_journal` worst case 125,263 -> **125,276 CU** and `report_fail` 151,304 -> **151,317 CU** (+13 CU each: one comparison and one `require!` on the half-spread the `Fill` arm already reads); bytes unchanged (1,181 and 1,149). Both are far under their asserted ceilings (144,100 / 1,232 and 174,000 / 1,232), and no ceiling was raised. The budget grid's fills in markets 5 and up are priced at the ask or the bid (`1_000_001_000_000`, `999_999_000_000`) now, since a fill at the mid of a zero-spread market is refused.

The ceilings are kept as they were (the measurement moved by 13 CU of about 18,800 CU of headroom); nothing in `budget.rs` changed except the fill prices.
