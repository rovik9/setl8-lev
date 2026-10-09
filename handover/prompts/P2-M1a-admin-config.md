You are building module M1a of setl8-lev-trading in THIS repo (branch main). Read first, in this order: docs/DESIGN.md sections 1.2, 5 (5.3 SectorConfig), 6.1 (admin table), 10.5 (PayoutTally), 12 (error list), 14; docs/BUILD_PLAN.md section 0 and the M1a row; handover/module-0.md; handover/design-decisions.md (decisions 24, 30, 33, 34). Then read for reference only: ../setl8-vault/programs/core-vault/src/constants/admin.rs, the vault's scripts/build-test-so.sh and its tests-rs (how tests build instructions and load the localnet .so), and ../setl8-shared-interfaces (PayoutTally::write_into / derive_payout_tally; pinned tag v0.4.0).

RULES
- Work only inside this repo. Never modify ../setl8-vault or ../setl8-shared-interfaces.
- NEVER read, print, open or commit private keys, .env files or keypair JSONs. Do not create any keypair JSON file. Test admin keys are derived IN CODE from fixed public seed bytes (see ADMIN KEYS); only their PUBLIC keys are pasted as constants.
- Do not push, tag or change git config. Commit only with explicit paths (never `git add -A` or `git add .`); run scripts/secret-scan-staged.sh before committing. Author via env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com. Commit messages end with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Build exactly what is listed. If the design looks wrong or contradictory, STOP and report instead of changing it. Where the design is silent, choose the smallest safe option and list it under DECISIONS in the report.
- All arithmetic checked (no wrapping, no `as` casts that can truncate, no unwrap/expect in program code). No floats.

SCOPE (nothing else)
1. Errors: `programs/setl8-lev-trading/src/errors.rs` with the FULL error enum of DESIGN section 12, in order, so codes are 6000 to 6038 exactly (Unauthorized = 6000 ... NotHeld = 6038). A test asserts a few anchors (6000 Unauthorized, 6024 DailyPayoutCapReached, 6038 NotHeld).
2. Constants: seeds (`b"config"`, `b"payout_tally"`, `b"setl8_sector_authority"`), `MAX_MARKETS = 128`, delay bounds 86_400..=1_209_600 seconds, default delay 172_800, default window 86_400.
3. ADMIN KEYS (decision 30 family): mirror the vault's approach. `constants/admin.rs` exposes `SL8_ADMIN` and `ROV_ADMIN` pubkeys. Default build: the SAME two real admin public keys the vault uses (read them from the vault's admin.rs; they are public). Feature `localnet`: two PUBLIC TEST pubkeys whose secret seeds are fixed bytes in the test code: seed A = [0x11; 32], seed B = [0x22; 32] (derive with `Keypair::new_from_array`, print only the public keys once, paste them as constants, add a test that re-derives them and compares). `localnet` must never be in `default`.
4. State: `SectorConfig` exactly as DESIGN 5.3 (270 bytes including discriminator; fields in that order; `const_assert`/test that `8 + size == 270`). `PayoutTally` NOT an Anchor account: raw 25-byte account, PDA `["payout_tally"]`, layout via setl8_shared_interfaces (magic b"SL8TALLY", version 1, counts 0/0), owner = this program, created in `init_config`.
5. Instructions (all in `instructions/`):
   - `init_config(args)`: signers MUST be exactly `SL8_ADMIN` and `ROV_ADMIN` (checked against the compiled-in constants, not against anything the caller supplies) so nobody can front-run the singleton. Args: evaluator, vault_program, sl8_wallet, pass_delay_secs, max_payout_per_window, window_secs. Validates: delay in bounds, cap > 0, window > 0, evaluator != default pubkey. Stores admin_a = SL8_ADMIN, admin_b = ROV_ADMIN, current_ruleset = 0, market_count = 0, paused = 0, window_start = 0, window_total = 0. Creates `SectorConfig` PDA `["config"]` and the `PayoutTally`. Second call must fail (account exists). Rule set 1 is NOT created here (M1b).
   - `set_evaluator(new_key)`: both admins sign; new_key != default; emits `EvaluatorChanged{old,new}`.
   - `set_limits(pass_delay_secs, max_payout_per_window, window_secs)`: both admins; delay clamp 86_400..=1_209_600 else `InvalidLimits`; cap > 0; window > 0; emits `LimitsChanged`.
   - `set_paused(paused: bool)`: pausing = either admin; unpausing = both admins; emits `PausedChanged`.
   "Both admins" = both `admin_a` and `admin_b` accounts are signers AND equal the keys stored in config. "Either" = at least one of them is a signer and its key matches.
6. Build/test plumbing: `scripts/build-test-so.sh` (mirror the vault's: `cargo build-sbf --features localnet --sbf-out-dir target/test-deploy`, stack-overflow check, remove the throwaway keypair json WITHOUT opening it) and make `scripts/anchor-build-checked.sh` also run it (same DEFAULT_ONLY=1 switch as the vault). Update `tests-rs/tests/smoke.rs` to look for `../target/test-deploy/setl8_lev_trading.so`.
7. Tests (tests-rs, LiteSVM, litesvm =0.7.0, loading the localnet .so) - each of these is a test:
   - init_config succeeds with the two test admins; resulting account data length is 270 and every field equals the args; `payout_tally` bytes equal `b"SL8TALLY" | 1 | 0u64 LE | 0u64 LE`, owner is the program.
   - init_config fails for: one admin only; two random signers; a third key substituted for an admin; a second init; delay 86_399 and 1_209_601; cap 0; window 0; default evaluator.
   - set_evaluator / set_limits fail with one admin signer, succeed with both; set_limits boundary values 86_400 and 1_209_600 succeed.
   - set_paused: either admin can pause; one admin cannot unpause; both can; a non-admin cannot do either.
   - error code anchors from item 1.
   Use real transactions (not direct calls) so signer checks are exercised.

DEFINITION OF DONE
`scripts/test-engine.sh`, `scripts/anchor-build-checked.sh` (default AND localnet builds clean, no stack overflow) and `cd tests-rs && cargo test` all pass. Update handover/module-1a.md (what was built, versions, test counts, decisions, surprises). Commit with explicit paths; do NOT push.

REPORT BACK (paste this block, nothing longer):
- test counts: workspace, tests-rs (list the test names)
- both build scripts' final lines and the two .so sha256 values
- the two TEST admin PUBLIC keys and confirmation that the real keys equal the vault's (public keys only)
- every file added/changed, one line each
- DECISIONS where the design was silent
- anything you refused or stopped on
- `git log --oneline -3` and `git status --short`
