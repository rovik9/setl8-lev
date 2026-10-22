# P26 state (resume file)

Task: handover/prompts/P26-F16-forced-close-after-overshoot.md (decision 69). Offline: export RUSTUP_TOOLCHAIN=stable.
* step 1 DONE (RED on purpose): tests-rs/tests/quick_overshoot_closes.rs (15), arith_03 and e_03 un-ignored and adapted
* NEXT step 2: program change (post_journal.rs apply_event: the I-8 check lets Close{reason Fail} through in report_fail mode), then DESIGN 4.4 I-8, 4.7, module notes, trust-matrix row, AUDIT_REPORT part 4, decision 69
* then step 3 evaluator (greedy prefix, crossing close, forced closes after it with reason Fail, one unit for crossing+rest+Fail, unpostable = group > 8 events or > 3 proofs) + campaign story MultiQuick (floor >= 100) + verifier (accept Close{Fail} after overshoot, flag the dishonest variants)
* step 4 mutation, step 5 gates, step 6 freeze note
* FACTS: batch caps (8 events, 3 distinct proofs) stay: the failing batch can hold at most 6 closes (events) and closes in at most 3 markets (proofs) from the crossing close on; closes BEFORE the crossing go in earlier post_journal batches
