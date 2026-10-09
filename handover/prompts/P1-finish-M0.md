You are finishing module M0 of setl8-lev-trading in THIS repo (~/Documents/setl8-lev-trading, branch main). Read first: docs/DESIGN.md (sections 5, 6, 11, 16), docs/BUILD_PLAN.md (section 0 and M0), handover/module-0.md. The previous author could not compile Anchor code; your job is to compile it, fix it minimally, and report exactly what happened.

RULES
- Work only inside this repo. You may READ ../setl8-vault and ../setl8-shared-interfaces; never modify them.
- NEVER read, print, open or commit private keys, .env files or keypair JSONs. `anchor build` creates target/deploy/*-keypair.json: do not open it. Do not run `anchor keys sync`, `solana-keygen`, or anything that prints a secret. The program id in declare_id! and Anchor.toml stays the placeholder.
- Do not push, tag, or change git config. Commit only with explicit paths (never `git add -A` / `git add .`). Author via env vars:
  GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com
  Commit message ends with the line: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Do not add features or instructions. Fix only what is needed for the skeleton to build and the smoke test to run. If something in the design looks wrong, STOP and report it instead of changing the design.

STEPS
1. Print versions: rustc, cargo, anchor (expect 0.32.1), solana, cargo-build-sbf. Report any that differ.
2. `scripts/test-engine.sh` (cargo test --offline in the workspace). Expect 5 tests passing (4 in engine, 1 in journal-types).
3. Run `scripts/anchor-build-checked.sh`. Fix compile errors in programs/setl8-lev-trading only (and Cargo.toml features if the IDL build needs them, copying the pattern from ../setl8-vault/programs/core-vault/Cargo.toml). The script must finish with "OK: no stack-frame overflow reported."
4. In the root Cargo.toml move "programs/*" into `members` and remove "programs/setl8-lev-trading" from `exclude`. Re-run step 2 and step 3; commit the updated Cargo.lock.
5. tests-rs: copy the vault program binary to tests-rs/fixtures/core_vault.so (use an existing build under ../setl8-vault/target/deploy/ or target/test-deploy/ if present; if none exists, build it in a TEMP COPY of the vault, not inside the vault folder, and say so). Add `*.so` under tests-rs/fixtures/ to .gitignore (binaries are not committed) and document the copy command in handover/module-0.md. Then `cd tests-rs && cargo test`. Fix the smoke test only as needed (litesvm is pinned =0.7.0 as in the vault).
6. Update handover/module-0.md: replace "UNVERIFIED / NOT compiled" wording with what you actually ran, versions, test counts, and any deviation.
7. `git status`, then commit with explicit paths only (Cargo.toml, Cargo.lock, programs/, tests-rs/, .gitignore, scripts/ if changed, handover/module-0.md). Do NOT push.

REPORT BACK (paste this block, nothing longer):
- versions (rustc/cargo/anchor/solana)
- test counts: workspace, tests-rs
- anchor-build-checked.sh final line and the .so sha256
- every file you changed and why (one line each)
- anything that surprised you or that you refused to change
- output of `git log --oneline -3` and `git status --short`
