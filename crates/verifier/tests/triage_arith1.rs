//! Triage of claims arith-07 and arith-08 of the M12 independent review (verifier behaviour).
//! Each test asserts the CORRECT behaviour and is ignored until the finding is fixed.
mod common;
use common::*;
use setl8_verify::{verify_account, FindingClass as C, Verdict};

fn run(f: &Files) -> setl8_verify::Report {
    verify_account(&f.inputs()).expect("inputs parse")
}

/// arith-07 (same defect as the builder's VRF-3, FIXED in M12 as an input error): the rules file is bound to the account. The account
/// was opened under rule set 1; a rules file that says rule set 2 is refused (an input error, exit code 2), never VALID.
#[test]
fn arith_07_a_rules_file_of_another_rule_set_version_is_not_valid() {
    let b = close_case(600);
    let mut f = b.files();
    assert_eq!(run(&f).verdict, Verdict::Valid, "control: the honest files are VALID");
    let r = String::from_utf8(f.rules.clone()).unwrap();
    assert!(r.contains("\"ruleset_version\":1"));
    f.rules = r.replace("\"ruleset_version\":1", "\"ruleset_version\":2").into_bytes();
    let err = verify_account(&f.inputs()).expect_err("a rules file of version 2 for an account of version 1 is an input error");
    assert!(err.to_string().contains("rule set"), "{err}");
}

/// FINDING arith-07: `opened.size_s` must be one of the rules file's tier sizes (the chain takes it from `tier_size[tier_index]`).
#[test]
fn arith_07_a_size_that_is_not_a_tier_size_is_not_valid() {
    let b = close_case(600);
    let mut f = b.files();
    let r = String::from_utf8(f.rules.clone()).unwrap();
    assert!(r.contains("\"tier_sizes\":[100000000000]"));
    f.rules = r.replace("\"tier_sizes\":[100000000000]", "\"tier_sizes\":[5000000000,10000000000]").into_bytes();
    let rep = run(&f);
    assert_ne!(rep.verdict, Verdict::Valid, "size_s 100,000 USD is not in the tier list [5,000, 10,000] and the verifier said VALID");
}

/// FINDING arith-08: the second-recorder comparison floors the difference of two 1e-18 values, so two archives whose values
/// straddle an e12 boundary (1_999_999 and 2_000_000 in e18 units: mids 1 and 2 in e12) are not reported as disagreeing.
#[test]
fn arith_08_second_recorder_sees_a_disagreement_across_an_e12_boundary() {
    let b = close_case(600);
    let mut f = b.files();
    // control: an identical second archive agrees
    f.second = Some(b.archive_bytes());
    assert!(!run(&f).classes().contains(&C::ArchiveDisagreement));
    // the second recorder saw one e18 unit less: the e12 mid differs by one unit (value is a multiple of 1e6 in the first archive)
    let mut s2 = b.clone();
    assert_eq!(s2.samples[0].value % 1_000_000, 0);
    s2.samples[0].value -= 1;
    f.second = Some(s2.archive_bytes());
    let rep = run(&f);
    assert!(rep.classes().contains(&C::ArchiveDisagreement), "the mids differ by 1 e12 unit but no ArchiveDisagreement: {:?}", rep.classes());
}
