You are applying a REVISION of the owner's fixed costs in setl8-lev-trading in THIS repo (branch main). Required state: 87ccb53 or later committed, and the P24a step commits dada536 to 751b451 present. THE PROGRAM IS FROZEN (localnet sha256 43fdd9356c774ea343686e5f6bf781b998a01670664c2af6b8233bf97f22bc1b, default ebbd0786d2d1a58a9ef11e539baed4c9057fc3eab59ed8e62a5506bcba6f2c3b): you may NOT change programs/, crates/engine or crates/journal-types. No network. Do not touch ~/setl8-observer-run or tools/session-observer/data.
Rules as always: no push/tag/git-config change; explicit-path commits after scripts/secret-scan-staged.sh; author env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com; commit messages end with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>; never read or print keys/keypairs/.env; --offline, RUSTUP_TOOLCHAIN=stable, --locked. ~/Documents/setl8-vault and ~/Documents/setl8-shared-interfaces are read-only. LIMIT DISCIPLINE: no parallel agent waves; one commit per step; keep handover/audit/STATE.md updated.

WHAT CHANGED
P24a was executed with the first numbers (commission 300 e6 per side, half_spread_e6 50). Rovik revised them the same day, before any deploy (decision 72 amendment):
1. COMMISSION: 0.02 % of notional per side = 200 e6 for every class 0 to 7 ("once" read as one flat rate for every class; a round trip costs 0.04 %; charging only once per round trip would need a program change and is out of scope). If Rovik meant 0.02 % per round trip the number would be 100 e6 per side: say so in launch.md.
2. SPREAD: "1 pip each side" = 1 bp of price per side: half_spread_e6 = 100 on EVERY market (full spread 2 bp, a round trip pays 2 bp). Per-class pip sizes are not used.
Everything else in P24a stays: `spread_pending = true` on every market with the note "owner base 100 e6 set; oracle-noise floor check pending (P24b)" (so `verify --production` still fails, and only on the spread flags), news spread not implemented (decision pending), commission `pending` cleared, rule set version as P24a decided.

WORK (one commit per step)
1. Edit config/launch-markets.toml (commission 200, half_spread 100, the pending note text), run generate and verify in tools/launch-config; print the `verify --production` failure list (must be only spread flags). Commit config/launch-markets.toml, config/launch.json, config/launch.md.
2. Fix every test, fixture and doc that pins 300 e6 / 50 e6 / 0.03 % / 0.5 bp, without weakening any assertion (grep the whole repo outside target/ and handover/prompts/ for 300, 50, "0.03", "0.5 bp", "half-spread 50"); list each in the report.
3. RE-RUN the launch-cost campaigns from P24a step 3 with the new numbers (10,000 paths). Event counts per Fill/Close and the N = 1..8 close-all table must be IDENTICAL to P24a's (rates do not change event counts): say "identical" or list every difference. Also check the rounding corner: with 200 e6 the smallest legal order (minimum quantity, about $10 notional) must still produce a fee of at least 1 micro-USD, and no Fee event may vanish or change type because a fee rounds to zero; if one does, STOP and report the market and numbers. Print `Stats::unpostable` (must be 0), the largest requested compute limit and the largest transaction size.
4. COST TABLE regenerated (numbers only, in the report and in launch.md): per class: leverage_x, round-trip commission % of account at full leverage (2 x 0.02 % x leverage), round-trip spread cost (2 x 1 bp x leverage) %, their sum, and the number of full-leverage round trips that use the 8 % static floor; the FX-like commission in USD per 100,000 notional per side ($20) next to the Pepperstone Razor reference of $3.50 per lot per side.
5. DOCS: design-decisions.md: append "72 (revised by Rovik 2026-10-23, same day, before any deploy)" with the new numbers and keep the original 72 text as history; deploy-checklist.md and launch.md agree with the new numbers.
6. GATES: launch-config unit tests, generate, verify; build-test-so.sh still prints 43fdd935...; tests-rs full and test-engine.sh if any config number feeds them; clippy -D warnings for every touched crate; secret scan; `git diff 64629c1 HEAD -- programs crates/engine crates/journal-types Cargo.lock Cargo.toml rust-toolchain.toml Anchor.toml` must be EMPTY (print it).

REPORT BACK (paste this block, nothing longer):
- confirmation of the new numbers in config/launch.json; the exact `verify --production` failure list
- event counts and close-all table: identical to P24a or the differences; rounding corner result; campaign unpostable, largest limit, largest tx size
- the cost table
- tests/fixtures/docs changed (one line each)
- gate timings, the empty program diff, the freeze hash line
- any STOP and DECISIONS where the prompt was silent
- `git log --oneline -8` and `git status --short`
