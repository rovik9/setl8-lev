//! M12 triage of independent-review claims V-01 .. V-09 (verifier side). SELF-audit by the builder.
//! Every `#[ignore]` test asserts the CORRECT behaviour and is expected to FAIL today (`-- --ignored`); the control tests
//! (not ignored) pin what the verifier does today, so the claim is reproduced and a later fix has to change them.
mod common;
use common::*;
use journal_types::{CloseReason, JournalEvent as E};
use setl8_verify::archive::{Sample, RECORD_LEN};
use setl8_verify::FindingClass as C;
use setl8_verify::Verdict;

fn set_proof(b: &mut Bundle, line: usize, p: journal_types::PriceProof) {
    match &mut b.lines[line] {
        GLine::Event { proof, .. } => *proof = Some(p),
        _ => panic!("not an event"),
    }
}

/// Sets `flat = false` in every Meta from line `from` on (a coherent liar: the position it hides stays open).
fn metas_not_flat_from(b: &mut Bundle, from: usize) {
    for i in from..b.lines.len() {
        if let GLine::Event { ev: E::Meta { flat, .. }, .. } = &mut b.lines[i] {
            *flat = false;
        }
    }
}

/// Removes every floor event, close and breach from the journal (the "omitted breach" liar of the trust matrix: it posts
/// only the fills, the fees and the Meta heartbeats, with `flat = false` from the first batch that would have breached on).
fn strip_floor_events(mut b: Bundle) -> Bundle {
    let rm: Vec<usize> = b
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| matches!(l, GLine::Event { ev: E::TightStart { .. } | E::FloorRaised { .. } | E::Close { .. } | E::Breach { .. } | E::TightEnd { .. }, .. }))
        .map(|(i, _)| i)
        .collect();
    b.remove_lines(&rm);
    let first_meta = b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    metas_not_flat_from(&mut b, first_meta + 1);
    b
}

fn omitted_breach() -> Bundle {
    strip_floor_events(breach_then_recovery())
}

fn raw_record(market: u16, ts_ns: u64, value: i128) -> Sample {
    Sample { market, source: 2, recv_ns: ts_ns.saturating_add(1_000_000), ts_ns, value, digest: [0; 32] }
}

// ------------------------------------------------------------------------------------------------ V-01

#[test]
fn v01_control_the_omitted_breach_is_a_missed_breach_with_a_clean_archive() {
    let r = verify_no_snapshot(&omitted_breach());
    assert!(r.classes().contains(&C::MissedTightStart), "{:#?}", r.findings);
    assert_eq!(r.verdict, Verdict::Mismatch);
}

// FINDING V-01
#[test]
#[ignore = "FINDING V-01: one record far in the future at stream index 0 stops the archive walk; the omitted breach is called VALID (module-9 decision 7 says order must be non-decreasing, nothing checks it)"]
fn v01_an_out_of_order_poison_record_must_not_hide_an_omitted_breach() {
    let mut b = omitted_breach();
    // a well-framed record of an unknown market, dated year 2096, as the very first record of the stream
    b.samples.insert(0, raw_record(7, 4_000_000_000_000_000_000, 1_080_000_000_000_000_000));
    for m in &mut b.metas {
        m.n_samples += 1;
    }
    b.refresh_commitments();
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "an archive whose records are not in non-decreasing second order, or the omitted breach behind it, must be a finding: {:#?}", r.findings);
}

#[test]
fn v01_control_the_poison_record_is_framing_valid_and_today_the_verdict_is_valid() {
    // pins the claim: if this starts to fail, V-01 was fixed (then delete this control and un-ignore the test above)
    let mut b = omitted_breach();
    b.samples.insert(0, raw_record(7, 4_000_000_000_000_000_000, 1_080_000_000_000_000_000));
    for m in &mut b.metas {
        m.n_samples += 1;
    }
    b.refresh_commitments();
    assert_eq!(b.stream().len() % RECORD_LEN, 0);
    let r = verify_no_snapshot(&b);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
}

// ------------------------------------------------------------------------------------------------ V-02

/// Two long positions (EURUSD 1,000,000 and GBPUSD 1,000 units at a flat price); the GBPUSD feed has one sample only (at the fill) and the EURUSD
/// path of `breach_then_recovery` follows. Returns the honest bundle.
fn two_markets() -> Bundle {
    let mut spec = Spec::base();
    spec.markets.push(MarketSpec { id: 1, class: 0, source: 2, symbol: "GBPUSD", spread_e6: 30, max_age_secs: 60, sessions: vec![(0, 10_080)] });
    let mut script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Sample { market: 1, t: T_1400, mid: eur(13_000) },
        Script::Order { t: T_1400, intent: market_order(0, true, 1_000_000_000_000, T_1400) },
        Script::Order { t: T_1400, intent: market_order(1, true, 1_000_000_000, T_1400) },
    ];
    let mut t = T_1400;
    for m in [eur(10_806), eur(10_813), eur(10_831), eur(10_811), eur(10_780), eur(10_830)] {
        t += 30;
        script.push(Script::Sample { market: 0, t, mid: m });
    }
    script.push(Script::Heartbeat { t });
    Gen::new(spec).run(&script)
}

fn two_markets_omitted_breach() -> Bundle {
    strip_floor_events(two_markets())
}

#[test]
fn v02_control_two_market_omitted_breach_is_found_with_a_clean_archive() {
    assert!(two_markets().events().iter().any(|e| matches!(e, E::Breach { .. })), "the scenario must breach");
    let r = verify_no_snapshot(&two_markets_omitted_breach());
    assert!(r.classes().contains(&C::MissedTightStart), "{:#?}", r.findings);
}

fn with_zero_gbp_record(mut b: Bundle) -> Bundle {
    // after the last real GBPUSD sample (the fill) and before the first EURUSD sample that matters (+30 s); in order, value 0
    let at = b.samples.iter().position(|s| s.market == 0 && s.ts_sec() == T_1400 + 30).expect("first path sample");
    let ts = u64::try_from(T_1400 + 20).unwrap() * 1_000_000_000;
    b.samples.insert(at, raw_record(1, ts, 0));
    for m in &mut b.metas {
        if m.n_samples > at {
            m.n_samples += 1;
        }
    }
    b.refresh_commitments();
    b
}

// FINDING V-02
#[test]
#[ignore = "FINDING V-02: a value-0 record of another market, after its last real sample, makes marks_and_profit return None and the walk skips every later sample; the omitted breach is VALID"]
fn v02_a_zero_value_record_must_not_suspend_the_walk_for_all_markets() {
    let b = with_zero_gbp_record(two_markets_omitted_breach());
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "a non-positive archive value (the evaluator itself rejects it as BadValue) must be a finding, or the omitted breach must still be found: {:#?}", r.findings);
}

#[test]
fn v02_control_today_the_zero_value_record_gives_valid() {
    let b = with_zero_gbp_record(two_markets_omitted_breach());
    let r = verify_no_snapshot(&b);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
}

// ------------------------------------------------------------------------------------------------ V-03 (a): protective exit

/// 200,000 EUR long at 14:00:00 with a stop-loss at 1.0790; +30 s mid 1.0795 (above the stop), +60 s mid 1.0770 (through it).
fn sl_run() -> Bundle {
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: order(IntentKind::Market, 0, 0, 200_000_000_000, 0, eur(10_790), 0, 0, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_795) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_770) },
        Script::Heartbeat { t: T_1400 + 60 },
    ];
    Gen::new(spec).run(&script)
}

/// The stop-loss Close re-priced with the +30 s sample (still inside the 60 s window, above the stop: the stop "saved" ~1.7 pips).
fn sl_close_at_the_old_sample(honest: &Bundle) -> Bundle {
    let mut b = honest.clone();
    let ci = b.find(|e| matches!(e, E::Close { reason: CloseReason::StopLoss, .. }));
    let old = b.samples.iter().position(|s| s.ts_sec() == T_1400 + 30).expect("the +30 s sample");
    let old_proof = proof_of(&b.spec, &b.samples[old]);
    let h = u32::from(b.spec.market(0).spread_e6);
    let exit = engine::bid_from_mid(b.samples[old].mid_e12().unwrap(), h).unwrap();
    set_proof(&mut b, ci, old_proof);
    if let E::Close { exit_e12, pnl, credited, entry_e12, qty, quick, .. } = b.ev_mut(ci) {
        *exit_e12 = exit;
        *pnl = engine::pnl_micro(true, *qty, *entry_e12, exit).unwrap();
        *quick = false;
        *credited = *pnl;
    }
    b
}

// FINDING V-03
#[test]
#[ignore = "FINDING V-03a: an evaluator-originated StopLoss close priced with an older sample of the max_age window (not the triggering sample) is VALID; the walk's Trigger expectation carries the price but never compares it"]
fn v03a_a_stop_loss_close_must_use_the_triggering_sample() {
    let honest = sl_run();
    let r = verify_no_snapshot(&honest);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "control: {:#?}", r.findings);
    let b = sl_close_at_the_old_sample(&honest);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "a stop-loss executed at the +30 s sample instead of the +60 s trigger sample: {:#?}", r.findings);
}

#[test]
fn v03a_control_today_the_old_sample_stop_close_is_valid() {
    let b = sl_close_at_the_old_sample(&sl_run());
    let r = verify_no_snapshot(&b);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
}

// ------------------------------------------------------------------------------------------------ V-04

fn reason_of_the_close(mut b: Bundle, reason: CloseReason) -> Bundle {
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { reason: r, intent_id, quick, .. } = b.ev_mut(ci) {
        *r = reason;
        *intent_id = 0;
        let _ = quick;
    }
    b
}

// FINDING V-04
#[test]
#[ignore = "FINDING V-04: a Close{reason Breach, intent 0} with no Breach event, no floor sample and no trader order is VALID (walk_close ignores Breach/Fail); it flattens a pass account"]
fn v04_a_breach_reason_close_needs_a_breach() {
    let honest = pass_case(0);
    let r = verify_no_snapshot(&honest);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "control: {:#?}", r.findings);
    let b = reason_of_the_close(honest, CloseReason::Breach);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "a Close{{Breach}} with no breach event or breach sample: {:#?}", r.findings);
}

// FINDING V-04
#[test]
#[ignore = "FINDING V-04: same for reason Fail"]
fn v04_a_fail_reason_close_needs_a_fail() {
    let b = reason_of_the_close(pass_case(0), CloseReason::Fail);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "a Close{{Fail}} with no Fail event or floor sample: {:#?}", r.findings);
}

#[test]
fn v04_control_today_the_breach_reason_close_is_valid() {
    let r = verify_no_snapshot(&reason_of_the_close(pass_case(0), CloseReason::Breach));
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
}

// ------------------------------------------------------------------------------------------------ V-05

/// Two honest market buys of 100,000 EUR (intents 1 and 2, two batches), then a trader close-all (intent 3), a second buy
/// (intent 4) and a second close-all (intent 5), every one in its own batch.
fn two_cycles() -> Bundle {
    let spec = Spec::base();
    let q = 100_000_000_000u64;
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10801) },
        Script::Order { t: T_1400, intent: market_order(0, true, q, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10800) },
        Script::Order { t: T_1400 + 60, intent: market_order(0, true, q, T_1400 + 60) },
        Script::Sample { market: 0, t: T_1400 + 120, mid: eur(10801) },
        Script::Order { t: T_1400 + 120, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 120) },
        Script::Sample { market: 0, t: T_1400 + 180, mid: eur(10800) },
        Script::Order { t: T_1400 + 180, intent: market_order(0, true, q, T_1400 + 180) },
        Script::Sample { market: 0, t: T_1400 + 240, mid: eur(10801) },
        Script::Order { t: T_1400 + 240, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 240) },
        Script::Heartbeat { t: T_1400 + 240 },
    ];
    Gen::new(spec).run(&script)
}

fn fills(b: &Bundle) -> Vec<usize> {
    b.lines.iter().enumerate().filter(|(_, l)| matches!(l, GLine::Event { ev: E::Fill { .. }, .. })).map(|(i, _)| i).collect()
}

fn closes(b: &Bundle) -> Vec<usize> {
    b.lines.iter().enumerate().filter(|(_, l)| matches!(l, GLine::Event { ev: E::Close { .. }, .. })).map(|(i, _)| i).collect()
}

fn set_intent(b: &mut Bundle, line: usize, nonce: u32) {
    match b.ev_mut(line) {
        E::Fill { intent_id, .. } | E::Close { intent_id, .. } => *intent_id = nonce,
        _ => panic!("not a trade"),
    }
}

// FINDING V-05
#[test]
#[ignore = "FINDING V-05: a signed market intent is accepted for a second fill in a later batch (quantity accounting is per batch, no global used-nonce set)"]
fn v05_a_market_intent_must_not_be_reused_by_a_later_batch() {
    let honest = two_cycles();
    let r = verify_no_snapshot(&honest);
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "control: {:#?}", r.findings);
    let mut b = honest.clone();
    let f = fills(&b);
    assert_eq!(f.len(), 3);
    set_intent(&mut b, f[1], 1); // the second buy cites the first buy's intent (a different batch)
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "intent 1 (100,000 EUR) filled twice, in two batches: {:#?}", r.findings);
}

// FINDING V-05
#[test]
#[ignore = "FINDING V-05: a Close(qty 0) intent signed once closes any later position in that market, in any later batch"]
fn v05_a_close_all_intent_must_not_close_a_later_position() {
    let mut b = two_cycles();
    let c = closes(&b);
    assert_eq!(c.len(), 2);
    set_intent(&mut b, c[1], 3); // the second close-all cites the first close-all's intent (nonce 3), signed 120 s earlier
    let r = verify_no_snapshot(&b);
    assert_eq!(r.verdict, Verdict::Mismatch, "close intent 3 reused for a position opened after it: {:#?}", r.findings);
}

#[test]
fn v05_control_today_both_reuses_are_valid() {
    let honest = two_cycles();
    let mut a = honest.clone();
    let f = fills(&a);
    set_intent(&mut a, f[1], 1);
    let ra = verify_no_snapshot(&a);
    assert_eq!((ra.classes(), ra.verdict), (vec![], Verdict::Valid), "{:#?}", ra.findings);
    let mut b = honest;
    let c = closes(&b);
    set_intent(&mut b, c[1], 3);
    let rb = verify_no_snapshot(&b);
    assert_eq!((rb.classes(), rb.verdict), (vec![], Verdict::Valid), "{:#?}", rb.findings);
}

// ------------------------------------------------------------------------------------------------ V-08

fn run_cli(b: &Bundle, cmd: &str, with_snapshot: bool) -> (i32, String) {
    let f = b.files();
    let snap = b.snapshot_json().into_bytes();
    let read = move |p: &str| -> std::io::Result<Vec<u8>> {
        Ok(match p {
            "E" => f.events.clone(),
            "R" => f.rules.clone(),
            "A" => f.archive.clone(),
            "I" => f.intents.clone(),
            "S" => snap.clone(),
            _ => return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file")),
        })
    };
    let mut args: Vec<String> = [cmd, "--events", "E", "--rules", "R", "--archive", "A", "--intents", "I"].iter().map(|s| (*s).to_string()).collect();
    if with_snapshot {
        args.extend(["--snapshot".to_string(), "S".to_string()]);
    }
    let mut out = String::new();
    let code = setl8_verify::cli::run(&args, &read, &mut out);
    (code, out)
}

fn cut_after_first_batch(honest: &Bundle) -> Bundle {
    // a log cut at the first batch boundary (the indexer or the operator simply stopped serving lines)
    let first_meta = honest.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    let drop: Vec<usize> = (first_meta + 1..honest.lines.len()).collect();
    let mut b = honest.clone();
    b.remove_lines(&drop);
    b
}

#[test]
fn v08_control_a_truncated_log_is_valid_without_a_snapshot_and_a_mismatch_with_one() {
    let b = cut_after_first_batch(&breach_then_recovery());
    let (code, out) = run_cli(&b, "account", false);
    assert_eq!(code, 0, "{out}");
    let (code, out) = {
        // the snapshot of the FULL honest account against the cut log
        let honest = breach_then_recovery();
        let mut f = b.files();
        f.snapshot = Some(honest.snapshot_json().into_bytes());
        let r = setl8_verify::verify_account(&f.inputs()).unwrap();
        (r.verdict == Verdict::Valid, format!("{:?}", r.classes()))
    };
    assert!(!code, "with the snapshot the cut is a finding: {out}");
}

// FINDING V-08
#[test]
#[ignore = "FINDING V-08: a VALID verdict printed without --snapshot (and by `watch`, which drops it) never says the replay is not anchored to the account on chain"]
fn v08_a_valid_verdict_without_a_chain_anchor_must_say_so() {
    let b = cut_after_first_batch(&breach_then_recovery());
    for cmd in ["account", "watch"] {
        let (code, out) = run_cli(&b, cmd, false);
        let low = out.to_lowercase();
        assert!(code != 0 || low.contains("anchor") || low.contains("unverified against the chain") || low.contains("no snapshot"), "`{cmd}` printed an unqualified verdict for a log that no chain state vouches for: {out}");
    }
}

// ------------------------------------------------------------------------------------------------ V-09

fn with_rules(b: &Bundle, from: &str, to: &str) -> setl8_verify::Report {
    let mut f = b.files();
    f.snapshot = None;
    let text = String::from_utf8(f.rules.clone()).unwrap();
    assert!(text.contains(from), "{text}");
    f.rules = text.replace(from, to).into_bytes();
    setl8_verify::verify_account(&f.inputs()).expect("inputs parse")
}

#[test]
fn v09_control_today_a_rules_file_of_another_version_or_tier_is_valid() {
    let b = honest_long();
    let r = with_rules(&b, "\"ruleset_version\":1", "\"ruleset_version\":9");
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
    let r = with_rules(&b, "\"tier_sizes\":[100000000000]", "\"tier_sizes\":[5000000000]");
    assert_eq!((r.classes(), r.verdict), (vec![], Verdict::Valid), "{:#?}", r.findings);
}

// FINDING V-09
#[test]
#[ignore = "FINDING V-09: the rules file's ruleset_version is never compared with opened.ruleset_version"]
fn v09_the_rules_version_must_match_the_account() {
    let r = with_rules(&honest_long(), "\"ruleset_version\":1", "\"ruleset_version\":9");
    assert_eq!(r.verdict, Verdict::Mismatch, "the account was bought under rule set 1, the file is rule set 9: {:#?}", r.findings);
}

// FINDING V-09
#[test]
#[ignore = "FINDING V-09: opened.size_s is never checked against the rules file's tier_sizes"]
fn v09_the_account_size_must_be_a_tier_of_the_rules() {
    let r = with_rules(&honest_long(), "\"tier_sizes\":[100000000000]", "\"tier_sizes\":[5000000000]");
    assert_eq!(r.verdict, Verdict::Mismatch, "the account is a $100,000 account, the rules list only a $5,000 tier: {:#?}", r.findings);
}
