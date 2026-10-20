# M12 state (resume file; delete or rewrite when the audit report is final)

Updated at the resume session (P22b). Commit cdb454f holds the first batch of tests.

| item | state | where |
|---|---|---|
| 1 A-1..A-18 | PARTIAL: tests-rs/tests/adversarial.rs (29 tests, a01..a18 not all named by number yet); off-chain halves missing | adversarial.rs |
| 2 who-may-call | DONE (39 tests) | tests-rs/tests/who_may_call.rs |
| 3 account hygiene | NOT STARTED | tests-rs/tests/account_hygiene.rs |
| 4 journal/chain | DONE (13 tests + 1 ignored FINDING JRN-1) | tests-rs/tests/journal_attacks.rs |
| 5 state machine | DONE (34 tests) | tests-rs/tests/state_machine.rs |
| 6 arithmetic | PARTIAL: engine properties DONE (31 + 2 ignored FINDINGs ARI-1, ARI-2); program extremes and LiteSVM fuzz NOT STARTED | crates/evaluator/tests/rounding_properties.rs |
| 7 vault boundary + R-1 | NOT STARTED | tests-rs/tests/vault_boundary.rs |
| 8 evaluator/verifier adversaries, ed25519, trust-matrix rows | NOT STARTED | |
| 9 reviewers | NOT STARTED (all 8 died at the limit before reading anything) | |
| 10 mutation | NOT STARTED (driver in the session scratchpad: mutdrv.py) | |
| 11 unsafe/deps | NOT STARTED | |
| 12 report | NOT STARTED | handover/AUDIT_REPORT.md |

Next command: `cd tests-rs && cargo test --offline --test adversarial --test who_may_call --test state_machine --test journal_attacks`.
