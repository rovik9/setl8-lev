# P25c state (resume file)

Task: F-32 (handover/prompts/P25c-F32-held-account-fail.md), option (a): report_fail keeps the hold; reset -> ReviewPending (6110);
release_hold / fail_for_violation resolve Failed + held; close_account also refuses Failed + held (decision, prompt silent).
* step 1 DONE: error 6110 ReviewPending (commit "F-32 step 1")
* step 2 DONE (RED on purpose): hand-written tests (held_fail_review.rs, state_machine FH column, who_may_call 2 rows, auth07 adapted, held_account_losses)
* step 4 DONE (docs: DESIGN 2.9 4.7 6.1 6.2 10 13.3 14, trust-matrix, deploy-checklist, module-7, design-decisions amendment); NEXT mutation, gates, AUDIT_REPORT P25c paragraph + hash, report
* step 3 DONE: program change committed (end_account_keep_hold, report_fail, reset_account ReviewPending, close_account ReviewPending, fail_for_violation accepts Failed + held and skips the vault CPI); old tests adapted (lifecycle, reset, violation_reset, fuzz checker)
* then: DESIGN/trust-matrix/deploy-checklist/AUDIT_REPORT/design-decisions amendment, mutation (mutdrv, scratch n1..n3 rebuild), gates, report
* offline: export RUSTUP_TOOLCHAIN=stable
