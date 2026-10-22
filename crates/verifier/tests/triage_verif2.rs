//! Triage of claims V-10 to V-17 of the M12 independent review (verifier behaviour).
//! Each test asserts the CORRECT behaviour and is `#[ignore]`d until the finding is fixed; run with `-- --ignored`.
//! V-16 and V-17 have no test: see the verdicts (V-16 is DESIGN 4.5 "Honest limit" / R-2, V-17 is the tested design of the
//! second recorder, tests/stage2.rs `archive_disagreement_alone_and_no_disagreement_for_identical_archives`).
mod common;
use common::*;
use journal_types::JournalEvent as E;
use setl8_verify::{cli, verify_account, FindingClass as C, Inputs, Verdict};
use std::time::{Duration, Instant};

fn run_inputs(f: &Files, rules: Option<&str>) -> setl8_verify::Report {
    let mut i = f.inputs();
    if let Some(r) = rules {
        i.rules = r.as_bytes();
    }
    verify_account(&i).expect("inputs parse")
}

fn cli_run(args: &[&str], events: &[u8], f: &Files) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let read = |name: &str| -> std::io::Result<Vec<u8>> {
        match name {
            "E" => Ok(events.to_vec()),
            "R" => Ok(f.rules.clone()),
            "A" => Ok(f.archive.clone()),
            "I" => Ok(f.intents.clone()),
            _ => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file")),
        }
    };
    let mut out = String::new();
    let code = cli::run(&args, &read, &mut out);
    (code, out)
}

fn watch(events: &[u8], f: &Files) -> (i32, String) {
    cli_run(&["watch", "--events", "E", "--rules", "R", "--archive", "A", "--intents", "I"], events, f)
}

// ---------------------------------------------------------------------------------------------------------------------
// V-10: the rules file is one static snapshot; an admin update_market changes the verdict of OLD honest history

// V-10 (F-19, FIXED by P25b / decision 68): the rules file shows the state of the markets at the END of the log; the log carries the
// admin changes (`market_feed_changed`, `market_status_changed`) at the position the indexer saw them, and the replay applies them there.
// The original tests changed only the rules file; they are ADAPTED: the rules file shows the new value AND the log has the change line.

fn remapped(b: &Bundle) -> (Files, [u8; 32], [u8; 32]) {
    let f = b.files();
    let rules = String::from_utf8(f.rules.clone()).unwrap();
    let old = b.spec.market(0).feed();
    let new = [0x11u8; 32];
    assert!(rules.contains(&hex_of(&old)));
    let mut f = f;
    f.rules = rules.replace(&hex_of(&old), &hex_of(&new)).into_bytes();
    (f, old, new)
}

fn events_with(b: &Bundle, extra: &str) -> Vec<u8> {
    let mut e = b.events_text();
    e.push_str(extra);
    e.into_bytes()
}

#[test]
fn v_10_honest_history_stays_valid_after_an_admin_feed_remap() {
    let b = design_4_9();
    assert_eq!(run_inputs(&b.files(), None).verdict, Verdict::Valid, "the honest baseline");
    let (mut f, old, new) = remapped(&b);
    // without the change line the rules file shows a feed the whole history never used: ProofFeed, as before
    assert!(run_inputs(&f, None).classes().contains(&C::ProofFeed));
    // with the line (at the END of the log, after the history) the history is judged against the OLD feed
    f.events = events_with(&b, &setl8_verify::log::market_feed_line(0, &old, &new));
    let r = run_inputs(&f, None);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "honest history flagged after a remap: {:#?}", r.findings);
}

#[test]
fn a_feed_change_is_applied_at_its_log_position_and_must_chain() {
    let b = design_4_9();
    let (f, old, new) = remapped(&b);
    // the change at the START (right after `opened`): the history that follows uses the old feed, which is no longer the market's: ProofFeed
    let text = b.events_text();
    let mut lines: Vec<&str> = text.lines().collect();
    let opened = lines.iter().position(|l| l.contains("\"type\":\"opened\"")).unwrap();
    let change = setl8_verify::log::market_feed_line(0, &old, &new);
    lines.insert(opened + 1, change.trim_end());
    let mut g = f.clone();
    g.events = (lines.join("\n") + "\n").into_bytes();
    assert!(run_inputs(&g, None).classes().contains(&C::ProofFeed), "a change before the history makes the old proofs wrong");
    // a lone change with the wrong old feed makes the history's proofs wrong (the market is taken to have started with that feed)
    let mut h = f.clone();
    h.events = events_with(&b, &setl8_verify::log::market_feed_line(0, &[0x22; 32], &new));
    assert!(run_inputs(&h, None).classes().contains(&C::ProofFeed));
    // two changes that do not chain (the second starts from a feed the market never had) are a StateMismatch
    let mut h = f.clone();
    h.events = events_with(&b, &(setl8_verify::log::market_feed_line(0, &old, &[0x22; 32]) + &setl8_verify::log::market_feed_line(0, &[0x44; 32], &new)));
    assert!(run_inputs(&h, None).classes().contains(&C::StateMismatch));
    // and two that do chain are fine (old -> 22 -> new)
    let mut h = f.clone();
    h.events = events_with(&b, &(setl8_verify::log::market_feed_line(0, &old, &[0x22; 32]) + &setl8_verify::log::market_feed_line(0, &[0x22; 32], &new)));
    let r = run_inputs(&h, None);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
    // a rules file that does not show the LAST change is a StateMismatch too
    let mut k = f.clone();
    k.events = events_with(&b, &setl8_verify::log::market_feed_line(0, &old, &[0x33; 32]));
    let r = run_inputs(&k, None);
    assert!(r.findings.iter().any(|x| x.class == C::StateMismatch && x.message.contains("last feed")), "{:#?}", r.findings);
    // a change of a market the rules do not list
    let mut u = f.clone();
    u.events = events_with(&b, &setl8_verify::log::market_feed_line(9, &old, &new));
    assert!(run_inputs(&u, None).classes().contains(&C::UnknownMarket));
}

#[test]
fn v_10_honest_history_stays_valid_after_an_admin_sets_the_market_reduce_only() {
    let b = design_4_9();
    let f = b.files();
    let rules = String::from_utf8(f.rules.clone()).unwrap();
    assert!(rules.contains("\"status\":1"));
    let mut g = f.clone();
    g.rules = rules.replace("\"status\":1", "\"status\":2").into_bytes();
    // the rules file shows reduce-only; without the log line the history's opening fills are refused
    assert!(run_inputs(&g, None).classes().contains(&C::FillNotAllowed));
    g.events = events_with(&b, &setl8_verify::log::market_status_line(0, 1, 2));
    let r = run_inputs(&g, None);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "honest history flagged after reduce-only: {:#?}", r.findings);
    // the same change BEFORE the history takes effect from there
    let text = b.events_text();
    let mut lines: Vec<&str> = text.lines().collect();
    let opened = lines.iter().position(|l| l.contains("\"type\":\"opened\"")).unwrap();
    let change = setl8_verify::log::market_status_line(0, 1, 2);
    lines.insert(opened + 1, change.trim_end());
    g.events = (lines.join("\n") + "\n").into_bytes();
    assert!(run_inputs(&g, None).classes().contains(&C::FillNotAllowed));
}

fn hex_of(b: &[u8; 32]) -> String {
    setl8_verify::hexutil::encode(b)
}

// ---------------------------------------------------------------------------------------------------------------------
// V-11: `watch` on an events file that does not parse says NO_PENDING_PASS with exit 0

fn pending_pass_bundle() -> Bundle {
    let mut b = pass_case(0);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    b
}

// FINDING V-11 (FIXED in crates/verifier/src/cli.rs: try_newest_pending_pass; this test failed before the fix)
#[test]
fn v_11_watch_exits_2_when_the_events_file_does_not_parse() {
    let b = pending_pass_bundle();
    let f = b.files();
    // baseline: the pending pass is found and judged
    let (code, out) = watch(&f.events, &f);
    assert_eq!((code, out.starts_with("VALID seq=")), (0, true), "baseline: {out}");
    // one garbage line appended while the pass is pending
    let mut garbage = f.events.clone();
    garbage.extend_from_slice(b"this is not json\n");
    // invalid UTF-8 at the end
    let mut bad_utf8 = f.events.clone();
    bad_utf8.extend_from_slice(&[0xff, 0xfe, b'\n']);
    // a truncated final line
    let mut truncated = f.events.clone();
    truncated.truncate(truncated.len() - 20);
    for (label, ev) in [("garbage line", garbage), ("invalid utf-8", bad_utf8), ("truncated line", truncated)] {
        let (code, out) = watch(&ev, &f);
        assert_eq!(code, 2, "{label}: watch said {out:?} with exit {code}; an unreadable feed must be an input error (account mode says 2)");
        // and account mode really does say 2 for the same bytes
        let (acode, _) = cli_run(&["account", "--events", "E", "--rules", "R", "--archive", "A", "--intents", "I"], &ev, &f);
        assert_eq!(acode, 2, "{label}: account mode");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// V-12: a hostile log with event time i64::MAX and an open position never ends

fn open_position_bundle() -> Bundle {
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 200_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_801) },
    ];
    Gen::new(Spec::base()).run(&script)
}

fn push_meta(b: &mut Bundle, time: i64) {
    let seq = b.last_seq() + 1;
    b.lines.push(GLine::Event { seq, ev: E::Meta { time, flat: false, samples_hash: [0; 32], intents_hash: [0; 32], engine_version: 1 }, proof: None });
}

// FINDING V-12 (FIXED in crates/verifier/src/replay.rs before_event; this test hung before the fix)
#[test]
fn v_12_the_replay_terminates_on_events_at_i64_max() {
    let mut b = open_position_bundle();
    assert_eq!(verify_no_snapshot(&b).verdict, Verdict::Valid, "baseline");
    push_meta(&mut b, i64::MAX);
    // one such event ends (the first one takes the capped-nights branch)
    let one = b.files();
    let _ = verify_account(&one.inputs()).expect("one event at i64::MAX ends");
    push_meta(&mut b, i64::MAX);
    let f = b.files();
    let (tx, rx) = std::sync::mpsc::channel();
    // the worker is detached: if the replay never ends, the test process still exits when the harness is done
    std::thread::spawn(move || {
        let i = Inputs { events: &f.events, rules: &f.rules, archive: &f.archive, intents: &f.intents, snapshot: None, second_archive: None, as_of_seq: None };
        let r = verify_account(&i).map(|r| r.verdict);
        let _ = tx.send(r.is_ok());
    });
    let finished = rx.recv_timeout(Duration::from_secs(20));
    assert!(finished.is_ok(), "the verifier did not return within 20 s on a log with two events at time i64::MAX");
}

// ---------------------------------------------------------------------------------------------------------------------
// V-13: quadratic duplicate-nonce handling in IntentFile::parse (the walk's samples x pending x cancels scan is the same class)

fn intents_text(nonces: impl Iterator<Item = u32>) -> Vec<u8> {
    let order = market_order(0, true, 1_000, T_1400).to_bytes();
    let sig = [0u8; 64];
    let mut t = setl8_verify::intents::header_line();
    for n in nonces {
        t.push_str(&setl8_verify::intents::line_for(n, &order, &sig));
    }
    t.into_bytes()
}

fn parse_secs(bytes: &[u8]) -> f64 {
    let t = Instant::now();
    let f = setl8_verify::intents::IntentFile::parse(bytes).expect("parse");
    std::hint::black_box(&f);
    t.elapsed().as_secs_f64()
}

// FINDING V-13
#[test]
fn v_13_intent_file_parse_is_linear_in_the_number_of_duplicate_lines() {
    let n = 20_000u32;
    let distinct = intents_text(1..=2 * n);
    let repeated = intents_text((1..=n).chain(1..=n)); // same line count, every late line repeats an early nonce
    let a = parse_secs(&distinct).min(parse_secs(&distinct));
    let b = parse_secs(&repeated).min(parse_secs(&repeated));
    eprintln!("distinct {a:.3}s repeated {b:.3}s");
    assert!(b < 1.5 * a + 0.05, "the same number of lines takes {b:.3}s with duplicates against {a:.3}s without: quadratic");
}

// ---------------------------------------------------------------------------------------------------------------------
// V-14: program events with no line type in the verifier's format (PassCancelled, AccountAbandoned)

fn cancelled_bundle() -> (Bundle, i64) {
    let mut b = pending_pass_bundle();
    let t = last_event_time(&b) + 3_600;
    // the trader cancels the unanswered request after an hour: the program emits PassCancelled { account, phase, time }
    b.push_raw(&format!("\"type\":\"pass_cancelled\",\"phase\":2,\"time\":{t}"));
    (b, t)
}

// FINDING V-14
#[test]
fn v_14_a_pass_cancelled_line_is_understood() {
    let (b, _) = cancelled_bundle();
    let r = verify_no_snapshot(&b);
    assert!(!r.classes().contains(&C::UnknownEvent), "{:?}", r.findings);
    assert_eq!(r.verdict, Verdict::Valid, "{:?}", r.classes());
}

// FINDING V-14
#[test]
fn v_14_watch_has_no_pending_pass_after_a_cancel() {
    let (b, _) = cancelled_bundle();
    let f = b.files();
    let (code, out) = watch(&f.events, &f);
    assert_eq!((code, out.as_str()), (0, "NO_PENDING_PASS\n"));
}

// FINDING V-14
#[test]
fn v_14_an_account_abandoned_line_is_understood() {
    let mut b = design_4_9();
    let t = last_event_time(&b) + 7 * 86_400;
    let trader = setl8_verify::log::base58(&trader_keypair().public.to_bytes());
    b.push_raw(&format!("\"type\":\"account_abandoned\",\"trader\":\"{trader}\",\"time\":{t}"));
    let r = verify_no_snapshot(&b);
    assert!(!r.classes().contains(&C::UnknownEvent), "{:?}", r.findings);
    // P25b (F-24): the line ends the account (it is terminal in the replay), it is not just ignored
    assert!(r.state.terminal, "{:?}", r.state);
    assert!(!verify_no_snapshot(&design_4_9()).state.terminal);
}

// ---------------------------------------------------------------------------------------------------------------------
// V-15: plain (non-strict) ed25519 verify accepts small-order trader keys (same defect as FINDING VRF-2)

// FINDING V-15
#[test]
fn v_15_a_small_order_trader_key_cannot_sign() {
    let acct = account_key();
    let order = market_order(0, true, 1_000, T_1400).to_bytes();
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&identity);
    let accepted = (1u32..=50).filter(|n| setl8_verify::intents::verify_signature(&identity, &acct, *n, &order, &sig)).count();
    assert_eq!(accepted, 0, "an all-zero-S signature verified for {accepted} of 50 nonces");
}
