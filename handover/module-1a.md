# Module M1a: admin keys, config, errors (DONE, verified 2026-10-09)

Toolchain as in module-0.md (rustc 1.94.0, anchor-cli 0.32.1, solana-cli 4.0.2, cargo-build-sbf 4.0.0; `anchor-lang` resolves to 0.32.2, `litesvm =0.7.0`).

## Built
- `errors.rs`: full DESIGN 12 enum `Setl8Error`, 39 variants, `Unauthorized` = 6000 ... `NotHeld` = 6038 (order checked against the design table; append-only).
- `constants/`: seeds (`config`, `payout_tally`, `setl8_sector_authority`), `MAX_MARKETS = 128`, delay bounds 86_400..=518_400 (was 1_209_600 until decision 67), default delay 172_800, default window 86_400. `constants/admin.rs`: `SL8_ADMIN`, `ROV_ADMIN`. Default build = the vault's REAL admin keys (same two addresses, copied from the vault's `admin.rs`). Feature `localnet` (never in `default`) = public test keys from seeds `[0x11;32]` (SL8) and `[0x22;32]` (Rov).
- `state/sector_config.rs`: `SectorConfig`, 270 bytes with discriminator (const assert + test), fields in DESIGN 5.3 order.
- `instructions/`: `init_config`, `set_evaluator`, `set_limits`, `set_paused`; the last three share the `AdminAction` accounts (config + two unchecked admin accounts; `both_admins` / `either_admin` check signer flag AND equality with the stored keys). `events.rs`: `EvaluatorChanged`, `LimitsChanged`, `PausedChanged` (plain `emit!`).
- `PayoutTally`: raw 25-byte account, PDA `["payout_tally"]`, owner = this program, created in `init_config` and written with `PayoutTally::default().write_into` (shared-interfaces v0.4.0). If the PDA address was pre-funded by a third party, `init_config` tops up, allocates and assigns instead of `create_account` (tested).
- `scripts/build-test-so.sh` (localnet build into `target/test-deploy/`, stack check, throwaway keypair json removed without opening). `scripts/anchor-build-checked.sh` runs it afterwards (`DEFAULT_ONLY=1` skips).
- tests-rs: harness `tests/common/mod.rs` (real signatures, sigverify ON), `init_config.rs`, `admin.rs`, `admin_keys.rs`, `error_codes.rs`, `smoke.rs` (now looks for `../target/test-deploy/setl8_lev_trading.so`). tests-rs depends on the program crate (`no-entrypoint`, `localnet`) only to build instructions and decode accounts; the program under test is the `.so`.

## Test counts
- Workspace (`scripts/test-engine.sh`): engine 4, journal-types 1, program 6 (errors, constants, admin keys, SectorConfig size, generated id test) = 11. The program's admin-key unit tests also pass with `--features localnet`.
- tests-rs: 30 (init_config 14, admin 13, admin_keys 1, error_codes 1, smoke 1).
- A manual mutation (`both_admins` using `||`) was caught by 4 tests, then reverted.

## Decisions (design silent)
1. PLACEHOLDER PROGRAM ID CHANGED. M0 used the System Program id; LiteSVM then replaces the system program with our `.so` and every account-creating CPI fails. New placeholder = base58 of sha256("setl8-lev-trading/placeholder-program-id/v1") (`9NAdnJ7ySQqPYuPKu6d6wCbcaHHMnzwFT6xEcRfXX2a7`), no keypair exists for it. Changed in `declare_id!` and `Anchor.toml`. Still must be replaced by the real id before deploy.
2. Error for "evaluator is the default pubkey" (`init_config`, `set_evaluator`): `Unauthorized` (no better code exists; codes are fixed by DESIGN 12).
3. `init_config` has a separate `payer` signer (DESIGN 6.1 lists "payer"); the two admin accounts must be exactly `SL8_ADMIN` / `ROV_ADMIN` (`Unauthorized` otherwise, `AccountNotSigner` 3010 if not signed).
4. Admin events used `emit!` until decision 67 (audit F-27); now every event is `emit_cpi!` and the nine admin/cancel instructions end their account list with `event_authority`, `program`.
5. `LimitsChanged` carries old and new values; `PausedChanged` the new flag; `set_paused` is idempotent.
6. "Both admins" is positional: `admin_a` must be `config.admin_a`, `admin_b` must be `config.admin_b`.
7. NOT validated in `init_config` (not listed in scope): `vault_program` and `sl8_wallet` may be any key including default. No setter exists in M1a, so a wrong value there can only be fixed by a program upgrade. Worth adding a check or a both-admin setter in M1b if Rovik wants one.

## Surprises
- The byte-scan check the vault harness uses ("`.so` contains the admin keys") does not work here: the compiler inlines the 32 bytes as immediates. Dropped; a real-key `.so` would make every `init_config` test fail with `Unauthorized`, and the harness only ever loads `target/test-deploy/`.
- The 9 `unexpected cfg` macro warnings from M0 remain; harmless.
