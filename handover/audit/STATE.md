# P25c state (resume file)

Task: F-32 (handover/prompts/P25c-F32-held-account-fail.md), option (a): report_fail keeps the hold; reset -> ReviewPending (6110);
release_hold / fail_for_violation resolve Failed + held; close_account also refuses Failed + held (decision, prompt silent).
* step 1 DONE: error 6110 ReviewPending (commit "F-32 step 1")
* step 2 DONE (RED on purpose): hand-written tests (held_fail_review.rs, state_machine FH column, who_may_call 2 rows, auth07 adapted, held_account_losses)
* step 3 NEXT: program change (pass_common end_account keep-hold, report_fail, reset_account, close_account, fail_for_violation); vault re-flag reverts (6005) so fail_for_violation on a Failed account skips the vault CPI (the record is already Failed)
* then: DESIGN/trust-matrix/deploy-checklist/AUDIT_REPORT/design-decisions amendment, mutation (mutdrv, scratch n1..n3 rebuild), gates, report
* offline: export RUSTUP_TOOLCHAIN=stable
