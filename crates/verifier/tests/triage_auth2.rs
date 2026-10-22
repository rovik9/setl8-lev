//! Triage AUTH-13: the watch mode must know that a cancelled pass request is no longer pending.
use setl8_verify::{cli::newest_pending_pass, log};

fn requested(acct: &str) -> String {
    let h = "01".repeat(32);
    format!("{{\"type\":\"pass_requested\",\"account\":\"{acct}\",\"phase\":2,\"amount\":8,\"b\":9,\"seq\":3,\"journal_hash\":\"{h}\",\"time\":5}}\n")
}

#[test]
fn pin_a_request_alone_is_pending() {
    let acct = log::base58(&[9; 32]);
    let text = format!("{}{}", log::header_line(), requested(&acct));
    assert_eq!(newest_pending_pass(text.as_bytes()), Some(3));
}

// FINDING AUTH-13
#[test]
fn a_cancelled_request_is_not_pending() {
    let acct = log::base58(&[9; 32]);
    let text = format!(
        "{}{}{{\"type\":\"pass_cancelled\",\"account\":\"{acct}\",\"phase\":2,\"time\":3700}}\n",
        log::header_line(),
        requested(&acct)
    );
    assert_eq!(newest_pending_pass(text.as_bytes()), None);
}
