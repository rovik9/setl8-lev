# P25b state (resume file)

Task: handover/prompts/P25b-audit-fixes-evaluator-verifier.md (decision 68). Order: group A verifier F-08..F-14, group B evaluator F-15..F-21, group C the rest.
Offline: export RUSTUP_TOOLCHAIN=stable.
* DONE: F-10, F-11 (archive validation, class ArchiveRecordInvalid); F-08, F-09, F-12, F-13 (price rule + forced-close expectations: classes PriceSampleMismatch, ForcedCloseUnjustified, FalseFailMarks; DESIGN 4.5; trust-matrix moved 2 rows: older-sample row NEITHER->VERIFIER, event-dated-30s PROGRAM->BOTH)
* NEXT: F-14 (intent nonces across the log, close(0) bound), then group B (evaluator): F-15 (same second breach+static fail), F-16 (D-F16: forced-close order; STOP for infeasible class: see notes), F-17, F-18 (+ evaluator order_sample without `arrival` cut, so the price rule is exact), F-21 (marks snapshot at the breach sample), then group C (F-19 F-20 F-24 F-22 F-23 F-26 F-35 F-36 F-37), mutation, gates, docs
* NOTES: F-16 STOP evidence: E-03 scenario (3 x 0.5M EUR + 1M, +0.6%) cannot be ordered so the overshoot is last (sum of two smallest credits > limit); no sequence is postable by the program (Post refuses overshoot, report_fail allows one overshooting close then only Fail, and Fail needs Meta.flat). Needs a program/DESIGN change.
