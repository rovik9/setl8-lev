//! M12 item 5, part D: the pass boundaries to the second and the races. The tests themselves already exist (pass_flow.rs,
//! violation_reset.rs, adversarial.rs, written before the audit or by the audit); this file maps each boundary the prompt lists
//! to the test that asserts it, and FAILS when a mapped test disappears or is renamed, so the list cannot rot silently.
//! Each boundary is asserted with the exact error code in the mapped test.

const PASS_FLOW: &str = include_str!("pass_flow.rs");
const VIOLATION_RESET: &str = include_str!("violation_reset.rs");
const ADVERSARIAL: &str = include_str!("adversarial.rs");

/// (what the prompt asks for, file text, test function)
const MAP: &[(&str, &str, &str)] = &[
    ("finalize exactly pass_delay_secs, one second less (PassDelayNotElapsed), one second more", PASS_FLOW, "finalize_delay_boundary_uses_the_current_config_value"),
    ("delay clamped to 1 day minimum, a changed delay moves the date", PASS_FLOW, "finalize_delay_clamped_minimum_is_one_day_and_a_changed_delay_moves_the_date"),
    ("daily cap exactly full is accepted, one micro-USD over is refused", PASS_FLOW, "cap_exactly_the_payout_is_accepted_and_one_micro_less_is_refused"),
    ("daily cap: the window rolls at the exact second", PASS_FLOW, "cap_second_payout_in_the_window_is_refused_and_the_window_rolls_at_the_exact_second"),
    ("daily cap: up to twice the cap across a window boundary (pinned)", PASS_FLOW, "cap_allows_up_to_twice_the_cap_across_a_window_boundary"),
    ("veto by one admin is refused", PASS_FLOW, "veto_single_admin_is_refused"),
    ("veto by both is accepted, counts, changes nothing else (states 1 and 2)", PASS_FLOW, "veto_counts_and_changes_nothing_else_in_both_pending_states"),
    ("veto in state 3 (the violation marker) is not a pending pass", VIOLATION_RESET, "the_marker_value_on_an_active_account_is_not_a_pending_pass_for_any_pass_instruction"),
    ("finalize while vetoed (state 0) is refused", PASS_FLOW, "illegal_finalize_pass_from_states_0_and_1"),
    ("confirm_pass with flat false (and every other precondition alone)", PASS_FLOW, "confirm_preconditions_each_violated_alone"),
    ("reject_pass then confirm_pass (confirm from state 0 is refused)", PASS_FLOW, "illegal_confirm_pass_from_states_0_and_2"),
    ("confirm_pass then reject_pass (reject from state 2 is refused)", PASS_FLOW, "illegal_reject_pass_from_states_0_and_2"),
    ("request_pass twice (from states 1 and 2)", PASS_FLOW, "illegal_request_advance_from_states_1_and_2"),
    ("cancel_pass exactly at 3,600 s and one second earlier", PASS_FLOW, "cancel_boundary_is_3600_seconds"),
    ("evaluator key rotated between request and confirm: the old key cannot confirm, the new one can", ADVERSARIAL, "after_a_rotation_the_old_evaluator_is_an_outsider_and_the_new_one_is_in"),
];

#[test]
fn every_pass_boundary_of_the_prompt_is_asserted_by_an_existing_named_test() {
    for (what, text, name) in MAP {
        assert!(text.contains(&format!("fn {name}(")), "boundary \"{what}\": the test {name} is gone");
        assert!(text.contains(&format!("#[test]\nfn {name}(")) || text.contains(&format!("#[test]\n    fn {name}(")), "{name} is not a #[test]");
    }
    assert_eq!(MAP.len(), 15);
}
