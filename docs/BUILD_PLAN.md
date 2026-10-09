# setl8-lev-trading: BUILD PLAN

Status: DRAFT v2 (architecture C). Nothing is built until Rovik approves `docs/DESIGN.md`. Last updated: 2026-10-09.
The first-pass plan for the on-chain keeper design (option B) is in git commit `29221e7`.

## 0. Ground rules for every module
- **One module = one Claude Code prompt**, sized to fit one session: a stated scope, a file list, and an exit check that can be run. If a module grows past about 1,500 changed lines it is split.
- Anchor 0.32.1; `setl8-shared-interfaces` pinned to **v0.4.0** (re-check the tag when building); no upgrade-authority logic in code (2-of-2 is deployment config).
- After each module: tests green, `scripts/anchor-build-checked.sh` clean, an entry in `handover/design-decisions.md` or `handover/module-<n>.md` (what was decided, what was surprising), commit with explicit paths only, author Rovik, trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never push, tag or change git config.
- Test workspace: **`tests-rs/`, an isolated LiteSVM workspace with its own `Cargo.toml` and target dir**, same pattern as `setl8-vault/tests-rs` (the vault's workspace is read for reference, not modified). It loads this program's `.so` and the vault's `.so` (built from the vault repo at a pinned commit and copied in as a fixture). No oracle program is needed: price proofs are data.
- **Pure logic lives in no-Solana crates** `crates/engine` (netting, rules, fees; integer only, no floats, no `std::time`) and `crates/journal-types` (events, proofs, Borsh, hashing). Both compile for the SBF target (the program links only the checking subset) and for native/WASM.
- **Mutation testing (`cargo-mutants`) on `engine`** with a required kill score, so the arithmetic that decides payouts is checked independently of the VM. A score below 95% blocks the module.

## 1. Mutation-testing targets (modules M3 and M4, plus the program-side re-checks in M5)
Mutants that MUST be killed (each has at least one test that fails when it is applied):
- netting: rounding direction of P&L (floor -> ceil), average entry (long ceil <-> floor), margin (ceil -> floor), quick credit (floor -> ceil); `<= 45` -> `< 45`; timer not restarted on add; partial close resetting the timer; flip remainder keeping the old average; `min`/`max` swaps in the closing slice;
  sign errors on short positions; i128 -> i64 overflow paths.
- rules: `E <= F_s` -> `<`; `E <= L` -> `<`; L allowed to fall; `g_peak` not monotonic; latch clearing when `B` drops below 1.04 S; `0.01 S` strict vs non-strict; `min(F_s + 0.04 S, R)` cap removed or turned into `max`; tight mode not ending when flat; quick tally reduced by losses; `q_tally > 0.028 S` -> `>=`; check order (breach before SL/TP, static fail before tight breach).
- program re-checks (I-5 to I-10): each of the above rules expressed as an invariant must have a test that submits a batch violating it by one micro-USD and sees a rejection (this is a hand-written boundary suite, not cargo-mutants, because it runs in LiteSVM).

## 2. Modules
| # | Module | Scope | Depends on | Exit check |
|---|---|---|---|---|
| M0 | Workspace and tooling | Cargo/Anchor workspace, crate skeletons (`engine`, `journal-types`, `verifier`), `tests-rs/` LiteSVM workspace, vault `.so` fixture, `scripts/anchor-build-checked.sh` (fails on "Stack offset of ... exceeded max offset"), const-assert helper, CI script, multisig tooling decision (OPEN #17), engine language decision recorded (OPEN #2) | approval | `anchor build` clean; one smoke test loads both programs |
| M1 | Config accounts and admin | `SectorConfig` (270 B), `RuleSet`, `Market`, `WalletProfile`, `PayoutTally` (0/0, magic); `init_config`, `create_ruleset`, `create_market`/`update_market`, `set_evaluator`, `set_limits`, `set_paused`, `release_hold`, `fail_for_violation`, `ban_wallet`, `unban_wallet` (the last three need the vault `flag_trader_failed` CPI, tested in M7); const-asserts equal section 5 | M0 | Admin auth tests (either/both), immutability of rule sets, delay clamp 1 to 14 days, sizes match the doc |
| M2 | Journal types, hash chain and price proofs | `journal-types` complete (8.2, 8.3), hash chain with test vectors, `PriceProof` checks (I-4), **Stork signed-update research and format** (needs OPEN #1; otherwise mock proofs only and the unverified items stay in the list), session-window helpers | M0 | Hash vectors; tamper tests (insert, drop, reorder, alter); proof tests for wrong feed, stale, future |
| M3 | Engine: netting and costs | `apply_fill`, VWAP, partial/flip/flat, margin, spread, holding fee, commission, rounding rules 7.4, quick classification | M0 | All 7.5 examples and the 4.9 numbers as vectors; proptest invariants (no value created, flat = zero qty); **mutation targets killed** |
| M4 | Engine: rules | floors, latch, tight-mode ratchet, quick tally, phase pass check, breach/fail decisions, SL/TP/limit/stop trigger rules, A-RULE-1/2 as explicit switches | M3 | R1, R2, R3 and the 4.9 table reproduced exactly; **mutation targets killed** |
| M5 | `TradingAccount`, `open_account`, `post_journal` | 207-byte account, `open_account` (0.4% fee + `deposit_fee` CPI, wallet cap 5, rule-set snapshot), `post_journal` with invariants I-1 to I-12 using the `engine` checking subset, `emit_cpi!` events | M1, M2, M4, vault fixture | Byte layout test vs 5.1; the 4.9 example posted as three transactions with the exact state; boundary suite for every invariant |
| M6 | Pass flow | `request_advance`, `confirm_pass`, `reject_pass`, `cancel_pass`, `veto_pass`, `finalize_pass` (CPI `request_payout`, return-data handling, `PayoutTally`, daily cap), phase transitions, graduation | M5 | State machine tests for every edge of 10.3; delay tests with a mocked clock; cap rollover tests; tally monotonic; max 4 payouts; `Paid` vs `Abandoned` return data |
| M7 | Fail, activity, release | `report_fail` (I-11, `flag_trader_failed` CPI), `hold_account`, `fail_for_violation`, `touch_activity`, `release_account`, `close_account`, abandonment handling, vault-pause behaviour | M5 | Fail path tests incl. quick-limit forced fail (I-8); abandonment tests; rent return after 7 days |
| M8 | Reset | `reset_account` (`deposit_reset` CPI, previous account Failed, same phase, ladder pricing, 0.4% fee); needs the vault change (OPEN #3) | M5, M7, vault change | CPI tests against the real vault `.so`; ladder table test vs 9.5 |
| M9 | Public verifier | `setl8-verify` CLI: reads events, sample archive and intents file; replays with `engine`; checks proofs and the hash chain; verdict per account and per pass | M2, M4 | Honest journal passes; each tamper class (altered price, dropped sample, forged trade, wrong fee) reported |
| M10 | Evaluator reference harness and differential tests | Drives the engine with recorded samples and signed orders, builds batches, posts through the program in LiteSVM; fuzz of random price paths: the program's accepted state must equal the engine's. If the production backend is not Rust, the WASM build is fuzzed against the native engine bit for bit | M5, M6, M9 | 10,000 random paths green |
| M11 | Budgets | Measure CU for every instruction, transaction size with 2 proofs, stack check, replace the section 11 estimates | M5 to M8 | Table of measured numbers committed; failure if any instruction exceeds its budget |
| M12 | Adversarial suite and auditor report | A-1 to A-18 from section 13 as executable tests where testable; independent review pass over the diff; decisions log finalised; residual risk list restated | M10, M11 | `handover/AUDIT_REPORT.md` |

## 3. Order and parallelism
M0 first. M3 and M4 (pure engine) can run in parallel with M1 and M2. M5 needs M1, M2 and M4. M6 and M7 need M5 and can run in parallel. M9 needs only M2 and M4 and can start early. M8 waits for the vault change (OPEN #3). Everything from M10 on is sequential.

## 4. What is not built until you answer
Stork production proof checks (OPEN #1), the real market list (#4), spread/commission numbers (#5), the daily cap value for production (#6), hedge-farming mitigations (#7), veto policy (#8).
