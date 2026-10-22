# P25b state (resume file)

Task: handover/prompts/P25b-audit-fixes-evaluator-verifier.md (decision 68). Offline: export RUSTUP_TOOLCHAIN=stable.
* DONE: groups A, B, C (all findings except F-16 STOP and F-25 DEFERRED), docs (verifier-formats, DESIGN 4.5/4.6, trust-matrix, decision 68, deploy checklist, module notes, AUDIT_REPORT section 4), mutation (240 mutants: 22 missed -> tests/refactor -> rerun 167: 0 missed)
* NEXT: gates (test-engine.sh, tests-rs full, DIFF 10000, ENGINE 100000, golden, clippy, budget, .so sha256 unchanged: localnet 6cd19d26..., default 4a7af2ec...), AUDIT_REPORT "Changes after the audit, part 3", final report
