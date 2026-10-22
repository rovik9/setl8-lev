# P27 state (resume file): the runtime compute budget

Task: handover/prompts/P27-runtime-compute-budget.md (decision 71). Offline: `export RUSTUP_TOOLCHAIN=stable`. THE PROGRAM IS FROZEN (localnet sha256 43fdd935..., default ebbd0786...): no change to programs/, crates/engine, crates/journal-types, Cargo.lock, Cargo.toml, rust-toolchain.toml, Anchor.toml (`git diff ded93b7 HEAD -- ...` must stay empty).

## DONE
* steps 1 to 3 (one commit: the tool, the model and the campaigns depend on each other):
  * `tools/devnet-rehearsal measure` (`scripts/devnet-rehearsal.sh selftest measure`, a local solana-test-validator 4.0.2, about 25 minutes) writes `handover/cu-runtime.json` and `.md`: 102 shapes, every instruction of the program, each shape three runs, an explicit 400,000 limit; `render-runtime` rewrites the md from the json. `MEASURE_ONLY=post,worst` limits a debugging run; a failed section is recorded, the others kept.
  * `crates/evaluator/src/compute.rs` + `compute_tables.rs` (the model: `recommended_cu_limit`, `tx_size_with_budget`, `limit_for`, `fit`); `batch.rs` `tx_size_v0`; `drain.rs` uses them; `Batch` has `cu`, `cu_limit`, `tx_format`; `BatchPolicy::MAX` counts the limit instruction and may fall back to a version-0 transaction with an address lookup table (only for a batch that cannot be split and does not fit a legacy transaction: the quick-limit group of three closes in three source-3 markets is 1,244 bytes legacy).
  * campaigns: `engine_only` (engine-only, 10,000 and 100,000 paths) asserts limit >= cu, limit <= 1,400,000, size <= 1,232 (with and without a priority fee), `Stats::unpostable == 0`, prints the largest limit and size; new story `CommissionFail` (three closes, three fees, Fail, Meta: 8 events) and coverage `fail8_commission` (floor 100 per 10,000); the LiteSVM harness sends the limit instruction and the lookup-table form.
* the measured fact: LiteSVM 0.7.0 (Agave 2.3.13) under-counts every `emit_cpi!` event by 6,000 CU inside the inner invocation (6,539 against 12,539); it cannot be configured away (handover/cu-runtime.md "Why LiteSVM differs").

## NEXT (in this order)
* step 4: tests-rs/tests/budget_runtime.rs (written; fill `CEILINGS` with `BUDGET_RUNTIME_CALIBRATE=1`).
* step 5: docs: backend-compute-contract.md (new), deploy-checklist, module-11, DESIGN, AUDIT_REPORT part 5, design-decisions 71.
* step 6: D-03 note in verifier-formats.md (written).
* step 7: gates with timings, clippy, program diff, mutation (cargo-mutants --in-diff on compute.rs and callers).
