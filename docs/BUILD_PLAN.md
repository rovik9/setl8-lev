# setl8-lev-trading: BUILD PLAN

Status: DRAFT. Nothing is built until Rovik approves `docs/DESIGN.md`. Last updated: 2026-10-09.

## 0. Ground rules for every module
- **One module = one Claude Code prompt**, sized to fit one session: a stated scope, a file list, and an exit check that can be run. If a module grows past about 1,500 changed lines it is split.
- Anchor 0.32.1; `setl8-shared-interfaces` pinned to **v0.4.0** (re-check the tag when building); no program upgrade authority logic in code (2-of-2 is deployment config).
- After each module: tests green, `scripts/anchor-build-checked.sh` clean, an entry appended to `handover/design-decisions.md` or `handover/module-<n>.md` (what was decided, what was surprising), commit with explicit paths only, author Rovik, trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never push, tag or change git config.
- Test workspace: **`tests-rs/`, an isolated LiteSVM workspace with its own `Cargo.toml` and target dir**, same pattern as `setl8-vault/tests-rs` (the vault's workspace is read for reference, not modified). It loads this program's `.so`, the vault's `.so` (built from the vault repo at a pinned commit and copied in as a fixture) and a mock oracle program.
- **Pure logic lives in a no-Solana crate `engine/`** (netting, rule engine, rounding). It is tested by unit tests, property tests (proptest) and **mutation testing (`cargo-mutants`)** with a required kill score, so the arithmetic that decides payouts is checked independently of the VM.

## 1. Mutation-testing targets (modules M3 and M4)
Mutants that MUST be killed (each has at least one test that fails when it is applied):
- netting: rounding direction of P&L (floor -> ceil), average entry (long ceil <-> floor), margin (ceil -> floor), quick credit (floor -> ceil); `<= 45` -> `< 45`; timer not restarted on add; partial close resetting the timer; flip remainder keeping the old average; `min`/`max` swaps in the closing slice;
  sign errors on short positions; i128 -> i64 overflow paths.
- rules: `E <= F_s` -> `<`; `E <= L` -> `<`; L allowed to fall; `g_peak` not monotonic; latch clearing when `B` drops below 1.04 S; `0.01 S` strict vs non-strict; tight mode not ending when flat; quick tally reduced by losses; `q_tally > 0.028 S` -> `>=`; check order in the pipeline (breach before SL/TP).
- A mutation score below 95% on `engine/` blocks the module.

## 2. Modules
| # | Module | Scope | Depends on | Exit check |
|---|---|---|---|---|
| M0 | Workspace and tooling | Anchor workspace, `engine/` crate skeleton, `tests-rs/` LiteSVM workspace, vault `.so` fixture, `scripts/anchor-build-checked.sh` (fails on "Stack offset of ... exceeded max offset"), const-assert helper, CI script, multisig tooling decision (OPEN #12) | approval | `anchor build` clean; one smoke test loads both programs |
| M1 | Config accounts | `SectorConfig`, `RuleSet`, `Market`, `WalletProfile`, `BountyVault`, `PayoutTally` (0/0, magic) ; admin instructions (`init_config`, `create_ruleset`, `create_market`, `update_market`, `set_bounty`, `withdraw_bounty`, `set_paused`, `fund_bounty`); size const-asserts equal section 5 | M0 | Admin auth tests (either/both), immutability of rule sets, sizes match the doc |
| M2 | Oracle adapter | `OracleAdapter` trait, mock adapter, **Stork adapter** (needs OPEN #1; otherwise ship mock only), session windows, freshness and future-time checks, wrong-owner/wrong-PDA rejection | M1 | Adapter tests incl. replay, stale, closed session, wrong feed |
| M3 | Netting engine (`engine/`) | `apply_fill`, VWAP, partial/flip/flat, margin, rounding rules 7.4, quick classification | M0 | All 7.5 examples as vectors; proptest invariants (no value created, flat = zero qty); **mutation targets above killed** |
| M4 | Rule engine (`engine/`) | floors, latch, tight mode ratchet, quick tally, phase pass check, breach/fail decisions, A-RULE-1/2/3 as explicit switches | M3 | R1, R2, R3, section 4.12 vectors reproduced exactly; mutation targets killed |
| M5 | Trading account and journal | zero-copy `TradingAccount`, positions/orders/ring, journal events with `emit_cpi!`, rolling hash, hash re-verification helper | M1, M3 | Byte layout test vs 5.1; hash chain test (tamper detected) |
| M6 | Pipeline and keepers | `refresh_marks`, `crank` (steps 1 to 10), snapshot coherence, sample clock, bounty (floor settlements only), static-fail path with CPI stub | M2, M4, M5 | Race tests (forward-only clock), gap tests, 4.12 table reproduced on-chain, bounty farming tests |
| M7 | Trader trading instructions | `place_order` (market/limit/stop), `close_position`, `modify_position`, `cancel_order`, pending-order triggers, holding-fee accrual, 12 h lock, margin checks, reduce-only | M6 | Order lifecycle tests; holding-fee weekend test (3 charges); lock tests |
| M8 | Purchase and reset | `open_account` (0.4% fee transfer + `deposit_fee` CPI), `reset_account` (`deposit_reset` CPI), wallet cap 5, rule-set snapshot; needs the vault change (OPEN #3) | M1, M5, vault fixture | CPI tests against the real vault `.so`; wallet cap tests; rounding of fee |
| M9 | Phase advance, payout, abandonment | `advance_phase` (`request_payout` CPI, return-data handling, `PayoutTally`), `record_activity` plumbing, `release_account`, `close_account`, graduation, vault-pause behaviour | M8, M6 | `Paid` vs `Abandoned` return data tests; tally monotonic tests; cap of 4 payouts |
| M10 | Budgets | Measure CU for every instruction, stack check, transaction size with 8 positions through an address lookup table, replace the section 11 estimates | M6 to M9 | A table of measured numbers committed; failures if any instruction exceeds its budget |
| M11 | Adversarial integration suite | A-1 to A-14 from section 13 as executable tests where testable; fuzz of the pipeline with random price paths against the pure engine (differential test: on-chain result must equal the engine's) | M10 | Differential fuzz 10,000 paths green |
| M12 | Auditor report | Independent review pass over the diff, decisions log finalised, residual risk list R-1 to R-5 restated, known gaps | M11 | `handover/AUDIT_REPORT.md` |

## 3. Order and parallelism
M0 first. M3 and M4 (pure engine) can run in parallel with M1, M2. M5 needs M3 and M1. Everything after M6 is sequential. The vault change (OPEN #3) must land before M8's integration tests are final.

## 4. What is not built until you answer
Stork production adapter (OPEN #1), real market list (#4), spread/commission numbers (#5), hedge-farming mitigations (#2), position cap and notional cap (#8, #10).
