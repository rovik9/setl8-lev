# P25b state (resume file)

Task: handover/prompts/P25b-audit-fixes-evaluator-verifier.md (decision 68). Offline: export RUSTUP_TOOLCHAIN=stable.
* DONE group A (verifier): F-10 F-11 (archive validation), F-08 F-09 F-12 F-13 (price rule, forced-close expectations), F-14 (nonces across the log, 5 s window)
* DONE group B (evaluator): F-15 (static fail wins over a breach in one second, both components), F-17 (hostile samples refused + counted), F-18 (order time = evaluator's own second; clock never moved by orders; order_sample without the arrival cut), F-21 (marks snapshot at the decision sample)
* F-16: STOP for the infeasible class (evidence in the ignore messages of arith_03 / e_03 and the notes below); the feasible part is pinned by p25b_inputs.rs; Stats::unpostable asserted 0 over the campaigns
* NEXT group C: F-19 F-20 F-24 (verifier log vocabulary: market changes on the log, pass_cancelled/account_abandoned lines, size tier), F-22 F-23 F-26, F-35, F-36, F-37; then mutation, gates, docs (verifier-formats, DESIGN 4.6/13.3, AUDIT_REPORT, decision 68, deploy checklist F-25 deferred, module notes)
* NOTES: m9b vectors d1_untriggered and d3_refused_then_filled were regenerated (a close dated 1 s ahead of the evaluator's second now executes in the evaluator's second, so its Close time is one second earlier)
