# P26 state (resume file)

Task: handover/prompts/P26-F16-forced-close-after-overshoot.md (decision 69). Offline: export RUSTUP_TOOLCHAIN=stable.
* step 1 DONE (RED on purpose; green since step 2): tests-rs/tests/quick_overshoot_closes.rs (15), arith_03 and e_03 un-ignored and adapted
* step 2 DONE (program change 9864bac; docs partly: DESIGN I-8, 4.7; the rest of the docs go with step 6/5 — see NEXT). Was: program change (post_journal.rs apply_event: the I-8 check lets Close{reason Fail} through in report_fail mode), then DESIGN 4.4 I-8, 4.7, module notes, trust-matrix row, AUDIT_REPORT part 4, decision 69
* then step 3 evaluator (greedy prefix, crossing close, forced closes after it with reason Fail, one unit for crossing+rest+Fail, unpostable = group > 8 events or > 3 proofs) + campaign story MultiQuick (floor >= 100) + verifier (accept Close{Fail} after overshoot, flag the dishonest variants)
* step 4 mutation, step 5 gates, step 6 freeze note
* FACTS: batch caps (8 events, 3 distinct proofs) stay: the failing batch can hold at most 6 closes (events) and closes in at most 3 markets (proofs) from the crossing close on; closes BEFORE the crossing go in earlier post_journal batches
* step 3 DONE: evaluator (close_checked/force_close_all/forced groups, Stats::forced_after_overshoot), verifier (pending Close{Fail} settled by a same-second PhaseQuick Fail, survive Meta), Scenario::MultiQuick (r 600..=659, floor forced_after_overshoot 100 per 10,000 paths in M9cCover::floors), crates/evaluator/tests/multi_quick.rs (VALID + 3 dishonest variants)
* NEXT: module notes + trust-matrix BOTH row + decision 69 + AUDIT_REPORT part 4 + deploy-checklist duty text (docs commit, still step 2's list), step 4 mutation, step 5 gates (build-test-so.sh, anchor-build-checked.sh, test-engine.sh, tests-rs, DIFF 10000, ENGINE 100000, golden, clippy, budget), step 6 freeze note
