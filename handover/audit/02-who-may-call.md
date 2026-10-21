# 02. Who may call what (M12 item 2)

SELF-AUDIT by the builder; not an independent audit. File: `tests-rs/tests/who_may_call.rs` (39 tests). Prefix WHO.

Method: for every instruction of the program (25 instructions; `set_paused` has two rows, pause and unpause, so 26 rows) and eight signer classes (trader = the account's
owner; evaluator = `config.evaluator`; SL8 admin alone; Rov admin alone; both admins; a random funded key; the vault authority PDA passed as a plain non-signer account in the
signer slot; no signer at all) the expected result of each cell is WRITTEN IN THE TEST by hand from DESIGN 6.1 to 6.4, 10 and 14, never derived from the program. Allowed cells
succeed in a state prepared so that the allowed signer really can; refused cells must fail with the named error (Unauthorized 6000 or Anchor AccountNotSigner 3010 where the slot is a
typed `Signer`) and the trading account, config, wallet, tally and the vault accounts must be byte-identical afterwards. Extra variant cells (swapped admin slots, the same admin twice, the
old and the new evaluator key after a rotation) are listed per row.

Counts: **26 rows x 8 classes = 208 cells: 41 allowed, 167 refused** (plus 51 extra variant cells).
`rows_and_program_instructions_are_the_same_set` parses the `#[program]` block of `programs/setl8-lev-trading/src/lib.rs` and fails when an instruction has no row or a row has no instruction;
`the_instruction_parser_catches_a_new_instruction_and_a_removed_one` proves that check can fail; `the_hand_written_table_obeys_design_14_by_itself` checks the table against DESIGN 14 in words
(nobody who cannot sign is ever allowed; the authority PDA is never allowed; ...).

LIMITED, pinned: `open_account` has no role check: the evaluator key or either admin key may buy an account for itself like any wallet (the program only wants trader == payer);
`limited_swapped_admin_slots_are_refused_even_though_both_admins_signed` (WHO-2: the slots are positional). No finding: DESIGN 14 does not forbid the first.

## The matrix (printed by `WHO_MATRIX_PRINT=1 cargo test --offline --test who_may_call print_the_matrix -- --nocapture`)

| instruction | DESIGN rule | trader | evaluator | SL8 alone | Rov alone | both admins | random key | authority PDA (non-signer) | no signer |
|---|---|---|---|---|---|---|---|---|---|
| `init_config` | 6.1: SL8 admin and Rov admin, both compiled-in keys | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 | ALLOWED | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `set_evaluator` | 6.1 / 14: both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `set_limits` | 6.1 / 14: both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `set_paused(pause)` | 6.1 / 14: pause = either admin | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | ALLOWED | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `set_paused(unpause)` | 6.1 / 14: unpause = both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `create_ruleset` | 6.1 / 14: both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `create_market` | 6.1 / 14: either admin | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | ALLOWED | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `update_market` | 6.1 / 14: either admin (decision 24 exception) | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | ALLOWED | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `ban_wallet` | 6.1 / 14: both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `unban_wallet` | 6.1 / 14: both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `open_account` | 6.2: trader [S] and payer [S], payer == trader (a different payer is Unauthorized) | ALLOWED | ALLOWED | ALLOWED | ALLOWED | refused Unauthorized 6000 | ALLOWED | refused NotSigner 3010 | refused NotSigner 3010 |
| `reset_account` | 6.2: the trader of the failed account (other classes own a wallet but name the trader's failed account) | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `post_journal` | 6.3 / 14: evaluator only (I-1) | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `request_advance` | 6.2: the trader (owner of the account) | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `cancel_pass` | 6.2: the trader (owner of the account) | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `confirm_pass` | 6.3 / 14: evaluator only | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `reject_pass` | 6.3 / 14: evaluator only | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `veto_pass` | 6.1 / 10.7 / 14: both admins in one transaction | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `finalize_pass` | 6.4: anyone who signs as payer (no role) | ALLOWED | ALLOWED | ALLOWED | ALLOWED | ALLOWED | ALLOWED | refused NotSigner 3010 | refused NotSigner 3010 |
| `report_fail` | 6.3 / 14: evaluator only | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `hold_account` | 6.3 / 14: evaluator only | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `release_hold` | 6.1 / 14: both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `fail_for_violation` | 6.1 / 14: both admins | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 |
| `touch_activity` | 6.3 / 14: evaluator only | refused Unauthorized 6000 | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `release_account` | 6.2: the trader (owner of the account) | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
| `close_account` | 6.2: the trader (owner of the account) | ALLOWED | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused Unauthorized 6000 | refused NotSigner 3010 | refused NotSigner 3010 |
Extra classes:
| instruction | class | expected |
|---|---|---|
| `init_config` | admin slots swapped | refused Unauthorized 6000 |
| `init_config` | SL8 in both slots | refused Unauthorized 6000 |
| `init_config` | Rov in both slots | refused Unauthorized 6000 |
| `set_evaluator` | admin slots swapped | refused Unauthorized 6000 |
| `set_evaluator` | SL8 in both slots | refused Unauthorized 6000 |
| `set_evaluator` | Rov in both slots | refused Unauthorized 6000 |
| `set_limits` | admin slots swapped | refused Unauthorized 6000 |
| `set_limits` | SL8 in both slots | refused Unauthorized 6000 |
| `set_limits` | Rov in both slots | refused Unauthorized 6000 |
| `set_paused(pause)` | admin slots swapped | refused Unauthorized 6000 |
| `set_paused(pause)` | SL8 in both slots | ALLOWED |
| `set_paused(pause)` | Rov in both slots | ALLOWED |
| `set_paused(unpause)` | admin slots swapped | refused Unauthorized 6000 |
| `set_paused(unpause)` | SL8 in both slots | refused Unauthorized 6000 |
| `set_paused(unpause)` | Rov in both slots | refused Unauthorized 6000 |
| `create_ruleset` | admin slots swapped | refused Unauthorized 6000 |
| `create_ruleset` | SL8 in both slots | refused Unauthorized 6000 |
| `create_ruleset` | Rov in both slots | refused Unauthorized 6000 |
| `create_market` | admin slots swapped | refused Unauthorized 6000 |
| `create_market` | SL8 in both slots | ALLOWED |
| `create_market` | Rov in both slots | ALLOWED |
| `update_market` | admin slots swapped | refused Unauthorized 6000 |
| `update_market` | SL8 in both slots | ALLOWED |
| `update_market` | Rov in both slots | ALLOWED |
| `ban_wallet` | admin slots swapped | refused Unauthorized 6000 |
| `ban_wallet` | SL8 in both slots | refused Unauthorized 6000 |
| `ban_wallet` | Rov in both slots | refused Unauthorized 6000 |
| `unban_wallet` | admin slots swapped | refused Unauthorized 6000 |
| `unban_wallet` | SL8 in both slots | refused Unauthorized 6000 |

