# Module M0: workspace and tooling (DONE, verified 2026-10-09)

Status: **everything in M0 now compiles and its tests pass on a machine with the full toolchain.** Nothing below is a guess; each line says what was run.

## Toolchain used
rustc 1.94.0, cargo 1.94.0, anchor-cli 0.32.1, solana-cli 4.0.2 (Agave), cargo-build-sbf 4.0.0 (platform-tools v1.53, SBF rustc 1.89.0).
Note: `anchor-lang`/`anchor-spl` resolve to **0.32.2** in `Cargo.lock` (the `"0.32.1"` requirement is a caret range); the CLI is 0.32.1.

## What was run
- `scripts/test-engine.sh` (`cargo test --offline`, workspace): **6 passed** = engine 4, journal-types 1, setl8-lev-trading 1 (program crate, now a workspace member). Before `programs/*` was a member it was 5 passed (4 + 1).
- `scripts/anchor-build-checked.sh`: builds with no source changes to the program; ends with `OK: no stack-frame overflow reported.`
  `target/deploy/setl8_lev_trading.so` sha256 `e3fab899c3356290341c6c6321a23f997e57bb2a349cea27bddc9d87150f3d05` (placeholder program id; will change with any code change).
  The build prints 9 `unexpected cfg` warnings from the `#[program]` macro (`custom-heap`, `custom-panic`, `anchor-debug`); they are harmless and were left alone.
- `cd tests-rs && cargo test`: **1 passed** (`program_and_vault_binaries_exist`). It only checks that both `.so` files exist; it does not yet load them into LiteSVM (not added: no new features in M0).

## Layout
- Cargo workspace: `crates/engine`, `crates/journal-types`, `programs/*` (members); `tests-rs` is its own workspace (own Cargo.lock/target).
- `programs/setl8-lev-trading`: placeholder program id, one `ping` instruction (both replaced in M1a; the System Program id used here as placeholder in M0 was changed in M1a, see handover/module-1a.md).
- `scripts/test-engine.sh`, `scripts/secret-scan-staged.sh`, `scripts/anchor-build-checked.sh`.

## Vault fixture (not committed)
`tests-rs/fixtures/*.so` is git-ignored. To create the fixture, copy the vault's LOCALNET (public test keys) build, the one the vault's own tests-rs load:
```
cp ../setl8-vault/target/test-deploy/core_vault.so tests-rs/fixtures/core_vault.so
```
If that file is missing, build it in a temp copy of the vault with the vault's `scripts/build-test-so.sh` (never inside the vault folder).
This session used the existing `../setl8-vault/target/test-deploy/core_vault.so` (vault HEAD `591ced6`), sha256 `ed4c8c7cb93bd209cbd03ed46e58f755ca99e3aff681b4dc75b806cb6d762ba7`; nothing was built in or modified in the vault.

## Deviations from the earlier M0 draft
- No program source fix was needed: the skeleton compiled as written. Changes were the root `Cargo.toml` (`programs/*` into `members`, program removed from `exclude`), `.gitignore`, the doc comment of the smoke test, `Cargo.lock`, `tests-rs/Cargo.lock`.
- With the program in `exclude`, `anchor build` ran only a host build and produced no `.so`; it must be a workspace member for `target/deploy/` to be filled.

## Decisions (unchanged)
- Real program keypair: Rovik generates it locally (never printed or committed); the placeholder id must be replaced before any deploy. `anchor build` creates `target/deploy/setl8_lev_trading-keypair.json`; it was not opened and `target/` is git-ignored.
