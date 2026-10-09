# Module M0: workspace and tooling (PARTIAL)

Date: 2026-10-09. Status: **engine and journal-types verified; Anchor program, tests-rs and anchor build NOT verified** (no compiler access for them in the build sandbox).

## What exists and was run
- Cargo workspace with two dependency-free crates: `crates/engine` (constants and units from DESIGN 2 and 7; 4 tests) and `crates/journal-types` (batch limits; 1 test). `cargo test --offline`: 5 passed.
- `scripts/test-engine.sh`, `scripts/secret-scan-staged.sh`, `scripts/anchor-build-checked.sh` (mirrors the vault's; not run).

## What exists but was NOT compiled
- `programs/setl8-lev-trading` (placeholder program id = System Program id, one `ping` instruction), `Anchor.toml`, `tests-rs/` (isolated LiteSVM workspace, `litesvm =0.7.0`, smoke test that only checks the two `.so` files exist).
- Reason: the sandbox blocks crates.io, static.crates.io, static.rust-lang.org and the Solana tool hosts (organization network policy), and Anchor/Solana tools are not installed there.

## Decisions
- Only the dependency-free crates are workspace members for now; `programs/*` returns to `members` once `anchor build` works.
- Build artifacts must not be written inside the connected folder from the sandbox; use `CARGO_TARGET_DIR` elsewhere.
- Real program keypair: Rovik generates it locally (never printed or committed); the placeholder id must be replaced before any deploy.

## To finish M0 (needs a machine with Rust + Anchor 0.32.1 + Solana tools + network)
1. `anchor build` (via `scripts/anchor-build-checked.sh`); fix whatever the compiler says in the skeleton.
2. Add `programs/*` back to the workspace `members`; commit `Cargo.lock`.
3. Copy the vault `.so` into `tests-rs/fixtures/core_vault.so`; `cd tests-rs && cargo test`.
