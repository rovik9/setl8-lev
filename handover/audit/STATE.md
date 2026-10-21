# M12 state (resume file; delete or rewrite when the audit report is final)

| item | state |
|---|---|
| 1 A-1..A-18 | DONE: tests-rs/tests/adversarial.rs (70), crates/evaluator/tests/adversarial_attacks.rs (8), crates/verifier/tests/adversarial_attacks.rs (8, 1 ignored FINDING ATK-1) |
| 2 who-may-call | DONE: who_may_call.rs |
| 3 account hygiene | DONE: account_hygiene.rs (49) |
| 4 journal/chain | DONE: journal_attacks.rs (1 ignored FINDING JRN-1) |
| 5 state machine | DONE: state_machine.rs |
| 6 arithmetic + fuzz | DONE: rounding_properties.rs, arithmetic_extremes.rs (2 ignored FINDINGs), fuzz.rs (50,000 iterations) |
| 7 vault + R-1 | DONE: vault_boundary.rs |
| 8 evaluator/verifier adversaries, ed25519, per-row trust tests | DONE |
| 9 reviewers | 4 reviewers returned 77 claims (journal of workflow wf_3f6fe2ca-7d3); TRIAGE NOT DONE |
| 10 mutation | NOT STARTED (driver mutdrv.py in the session scratchpad) |
| 11 unsafe/deps | DONE (fragment) |
| 12 report | NOT STARTED |
