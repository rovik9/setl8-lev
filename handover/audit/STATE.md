# P23 state (resume file): the devnet rehearsal

Task: handover/prompts/P23-devnet-rehearsal.md. Offline: `export RUSTUP_TOOLCHAIN=stable`. Everything below is PUBLIC data; keys live in ~/.setl8-devnet-lev (outside the repo, mode 700).

## DONE
* step 1: tools/devnet-rehearsal (the driver, own Cargo.lock, 39 unit tests, clippy -D warnings clean) + scripts/devnet-rehearsal.sh (keys, build, needs, fund, deploy, run, status, all, selftest). The two programs build in scratch trees under ~/.setl8-devnet-lev (throwaway program ids AND throwaway admin keys; the scratch build WITHOUT the swaps reproduces the freeze note's localnet sha256 43fdd935...).
* the offline selftest (`scripts/devnet-rehearsal.sh selftest ...`, a local solana-test-validator, Agave 4.0.2, a scratch program build whose MINIMUM pass delay is 60 s so the day-2 part runs in minutes): S0, S1, S2 (day 1 and day 2), S3, S5 PASS; S4 FAIL = the findings below.

* step 2: handover/devnet-rehearsal.md (runbook, results, CU table, findings D-01..D-04, not covered), deploy-checklist section "Devnet rehearsal", decision 70. Repo gates re-run: build-test-so.sh = 43fdd935... (the freeze note), budget.rs green, the tool's clippy -D warnings and 39 tests green.
* the devnet run itself: STOPPED at the faucet (every request rate limited, 0 SOL on all keys).

## NEXT
* the devnet part needs SOL: `scripts/devnet-rehearsal.sh --cluster devnet needs` prints the amounts (the upgrade authority needs about 15.9 SOL for the two deployments, 17.4 SOL in all). The public faucet refused the first request (rate limited): Rovik funds the PUBLIC keys that `needs` prints, in the browser (https://faucet.solana.com).
* then, in this order:
  1. `scripts/devnet-rehearsal.sh --cluster devnet build`            (idempotent; rebuilds the scratch trees)
  2. `scripts/devnet-rehearsal.sh --cluster devnet fund`             (retries the faucet; `needs` shows what is missing)
  3. `scripts/devnet-rehearsal.sh --cluster devnet deploy`
  4. `scripts/devnet-rehearsal.sh --cluster devnet run`              (day 1: S0..S5)
  5. one day later (the pass delay is 86,400 s; the printed unix second says when): the SAME command `scripts/devnet-rehearsal.sh --cluster devnet run` (resume: finalize_pass, phase 3, the third reset, the 12 h lock of F1; S4 and S5 are redone)
* state.json: ~/.setl8-devnet-lev/work-devnet/state.json (resumable, no secrets); results.md and measure.md next to it.

## FINDINGS so far (selftest on a local validator, to be confirmed on devnet)
* D-01 (high): every `emit_cpi!` event costs about 17,300 CU on this runtime (12,539 inside the self-invocation); LiteSVM costs 10-12k. A legal `report_fail` of 8 events and 3 proofs fails with ComputationalBudgetExceeded (signature in state.json, label "report_fail ev=8 pr=3 heavy H1"); 7 events use 196,812 CU, 6 use 179,548, 5 use 162,284. A legal `post_journal` of 8 events and 3 proofs uses 186,144 CU. The evaluator's `cu_estimate` and the budget.rs ceilings were fitted on LiteSVM.
* D-02 (medium): seven instructions with one event cost more than their budget.rs ceilings (confirm_pass, create_market, create_ruleset, hold_account, unban_wallet, veto_pass, and request_advance is within 3%); all far below 200,000.
* D-03 (low): the verifier does not know the mock proof source 255: an account traded with mock proofs is never VALID (ProofShape); the rehearsal trades its verification-grade accounts with the markets' real source format (2) on the localnet build.
