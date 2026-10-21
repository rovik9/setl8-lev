# 05. State machine (M12 item 5)

SELF-AUDIT by the builder; not an independent audit. Files: `tests-rs/tests/state_machine.rs` (34 tests), `tests-rs/tests/pass_boundaries_map.rs` (1). Prefix STM.

**Part A (hand-written table).** 16 instructions that depend on the account state x 12 account coordinates x sector paused yes/no = **384 cells**: 95 allowed, 215 refused with one named error, 74
refused with the set of errors DESIGN allows (a cell with two violated preconditions: DESIGN does not say which comes first, so every allowed error is listed). The 12 coordinates of the
grid status x held x pass_pending (4 x 2 x 4 = 32 coordinates; only 10 can exist): Active not held pass_pending 0/1/2 (A0 A1 A2), Active HELD 0/1/2 (H0 H1 H2), Failed (F), Failed already reset (FX), Failed by
violation with the wallet banned (FV), violation-failed with the wallet unbanned (Fvu), Abandoned (AB), Graduated (GR). An allowed cell names the exact (status, held, pass_pending) triple that must follow, and a full-struct comparison of the
account checks that nothing else moved. Expected results come from DESIGN 4.4, 4.6, 4.7, 6.1 to 6.4, 10.1 to 10.8 and 12, not from the program.
**Part B.** the 22 grid coordinates that cannot exist x paused x every instruction, bytes edited in (`sweep_*`, 8 tests, 704 grid cells): no expected value is claimed, the program must return an error code or succeed (never crash, never
a non-Custom failure) and every refusal leaves the world byte-identical.
**Part C** (a demonstration with real instructions that the 22 non-existent coordinates cannot be reached) was NOT written (the writer ran out of budget): it is argued from the instruction code only. Open item.
**Part D (pass boundaries to the second and races):** `tests-rs/tests/pass_boundaries_map.rs` maps each boundary of the prompt to the existing test that asserts it with the exact error code:
delay exactly / one second less / more (`finalize_delay_boundary_uses_the_current_config_value`), daily cap exactly full and one over (`cap_exactly_the_payout_is_accepted_and_one_micro_less_is_refused`), the window roll to the exact second,
veto by one admin (refused) and by both, veto in state 3 (the violation marker), finalize while vetoed (state 0), confirm_pass with flat=false, reject then confirm and confirm then reject, request_pass twice, cancel_pass at exactly 3,600 s, and the
evaluator key rotated between request and confirm (the old key is Unauthorized, the new one confirms). The map test fails if any of the 15 tests disappears or is renamed.
No finding.
