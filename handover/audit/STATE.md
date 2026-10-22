# P25b state (resume file)

Task: handover/prompts/P25b-audit-fixes-evaluator-verifier.md (decision 68). Order: group A verifier F-08..F-14, group B evaluator F-15..F-21, group C the rest.
Offline: export RUSTUP_TOOLCHAIN=stable. Test helpers for editing ignored tests: /tmp/edit_helpers.py (may be gone; just remove the #[ignore] line by hand).
* DONE: F-10, F-11 (archive validation: Archive::bad / is_bad, class ArchiveRecordInvalid appended = class 43)
* NEXT: F-08/F-12 (price sample rule), F-09, F-13, F-14; then group B; then group C; mutation; gates; docs (DESIGN 4.5, verifier-formats, trust-matrix, AUDIT_REPORT, decision 68, deploy checklist F-25 deferred)
