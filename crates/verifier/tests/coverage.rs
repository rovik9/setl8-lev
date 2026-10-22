//! More tampers and scenarios that pin down individual rules of the replay (found by the mutation gate).
mod common;
use common::*;
use journal_types::{CloseReason, FeeKind, JournalEvent as E, OrderKind};
use setl8_verify::FindingClass as C;
use setl8_verify::Verdict;

fn only(r: &setl8_verify::Report, c: C) {
    assert_eq!(r.classes(), vec![c], "findings: {:#?}", r.findings);
    assert_eq!(r.verdict, Verdict::Mismatch);
}

fn valid(b: &Bundle) {
    let r = verify(b);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    assert_eq!(r.verdict, Verdict::Valid);
}

fn not_flat_metas(b: &mut Bundle, from: usize) {
    for i in from..b.lines.len() {
        if let GLine::Event { ev: E::Meta { flat, .. }, .. } = &mut b.lines[i] {
            *flat = false;
        }
    }
}

// ---------------------------------------------------------------- positions, sides, indexes

#[test]
fn the_close_that_names_the_other_side_says_which() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { side, .. } = b.ev_mut(i) {
        *side = journal_types::Side::Short;
    }
    let r = verify_no_snapshot(&b);
    assert!(r.classes().contains(&C::PositionMismatch), "{:#?}", r.findings);
    let f = r.findings.iter().find(|f| f.class == C::PositionMismatch).unwrap();
    assert_eq!((f.expected.as_str(), f.found.as_str()), ("long", "short"));
    // a short position closed as a long: the other way round
    let mut b = close_case(600);
    let fi = b.find(|e| matches!(e, E::Fill { .. }));
    if let E::Fill { side, .. } = b.ev_mut(fi) {
        *side = journal_types::Side::Short;
    }
    let r = verify_no_snapshot(&b);
    let f = r.findings.iter().find(|f| f.class == C::PositionMismatch).unwrap();
    assert_eq!((f.expected.as_str(), f.found.as_str()), ("short", "long"));
}

#[test]
fn findings_carry_the_sequence_and_the_index_of_their_event() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { pnl, .. } = b.ev_mut(i) {
        *pnl += 1;
    }
    let r = verify_no_snapshot(&b);
    assert_eq!(r.findings.len(), 1);
    let f = &r.findings[0];
    // events: Fill(1) Meta(2) Close(3) Meta(4): the Close is the third event line, index 2
    assert_eq!((f.seq, f.event_index), (Some(3), Some(2)));
    assert_eq!(f.expected.parse::<i64>().unwrap() + 1, f.found.parse::<i64>().unwrap());
}

#[test]
fn event_times_may_not_go_backwards_and_neither_may_a_meta() {
    // a Close dated before the Fill that precedes it
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { time, .. } = b.ev_mut(i) {
        *time = T_1400 - 1;
    }
    let r = verify_no_snapshot(&b);
    assert!(r.findings.iter().any(|f| f.class == C::StateMismatch && f.message.contains("backwards")), "{:#?}", r.findings);
    // P25b (F-26): the Meta of the Fill batch dated earlier than the Fill IS an error, as in the program (every event of a batch,
    // Meta included, is checked against the previous one)
    let mut b = close_case(600);
    let m = b.metas[0].line;
    if let E::Meta { time, .. } = b.ev_mut(m) {
        *time = T_1400 - 5;
    }
    let r = verify_no_snapshot(&b);
    assert!(r.findings.iter().any(|f| f.class == C::StateMismatch && f.message.contains("backwards")), "{:#?}", r.findings);
    // the boundary: a Meta dated exactly like the previous event is fine
    let mut b = close_case(600);
    let m = b.metas[0].line;
    if let E::Meta { time, .. } = b.ev_mut(m) {
        *time = T_1400;
    }
    valid(&b);
}

#[test]
fn a_later_event_may_not_precede_an_earlier_one_even_after_a_meta() {
    // time goes back between the Close batch and a trailing fee-less Meta-only batch is fine; between two events it is not
    let mut b = fee_case(20);
    let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Commission, .. }));
    if let E::Fee { time, .. } = b.ev_mut(i) {
        *time -= 1; // the first commission, dated one second before its Fill
    }
    let r = verify_no_snapshot(&b);
    assert!(r.findings.iter().any(|f| f.message.contains("backwards")), "{:#?}", r.findings);
}

// ---------------------------------------------------------------- proofs and the archive

#[test]
fn a_proof_whose_source_differs_from_the_archive_record_is_missing() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Fill { .. }));
    let line = b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Fill { .. }, .. })).unwrap();
    let _ = i;
    if let GLine::Event { proof: Some(p), .. } = &mut b.lines[line] {
        p.source = 4; // Hyperliquid: also an unsigned source, so only the archive record disagrees
    }
    only(&verify_no_snapshot(&b), C::ArchiveMissing);
}

fn signed_case() -> Bundle {
    let mut spec = Spec::base();
    spec.markets[0].source = 3;
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 200_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 600, mid: eur(10_830) },
        Script::Order { t: T_1400 + 600, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 600) },
    ];
    Gen::new(spec).run(&script)
}

#[test]
fn signed_proofs_carry_the_blob_digest_and_it_must_match_the_archive() {
    let b = signed_case();
    assert!(b.lines.iter().any(|l| matches!(l, GLine::Event { proof: Some(p), .. } if p.source == 3 && p.signed_update.len() == 32)));
    valid(&b);
    let line = b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Fill { .. }, .. })).unwrap();
    // a different digest in the proof
    let mut t = b.clone();
    if let GLine::Event { proof: Some(p), .. } = &mut t.lines[line] {
        p.signed_update[0] ^= 1;
    }
    only(&verify_no_snapshot(&t), C::ArchiveMissing);
    // a different digest in the archive (and the commitments of the journal refreshed): the proof no longer matches it
    let mut t = b.clone();
    t.samples[0].digest[3] ^= 1;
    t.refresh_commitments();
    only(&verify_no_snapshot(&t), C::ArchiveMissing);
    // a proof of another length is a shape error before it is anything else
    let mut t = b.clone();
    if let GLine::Event { proof: Some(p), .. } = &mut t.lines[line] {
        p.signed_update.pop();
    }
    only(&verify_no_snapshot(&t), C::ProofShape);
}

#[test]
fn an_event_that_says_no_proof_but_ships_one_is_a_shape_error_twice() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Fill { .. }));
    if let E::Fill { proof, .. } = b.ev_mut(i) {
        *proof = journal_types::NO_PROOF;
    }
    let r = verify_no_snapshot(&b);
    only(&r, C::ProofShape);
    // once for the hash chain (the proof bytes belong to no event) and once for the Fill that has no proof
    assert_eq!(r.findings.len(), 2, "{:#?}", r.findings);
}

// ---------------------------------------------------------------- intents

#[test]
fn only_one_protective_level_differing_is_a_forged_trade() {
    for (sl, tp) in [(5u64, 0u64), (0, u64::MAX)] {
        let mut b = close_case(600);
        let i = b.find(|e| matches!(e, E::Fill { .. }));
        if let E::Fill { sl: s, tp: t, .. } = b.ev_mut(i) {
            *s = sl;
            *t = tp;
        }
        only(&verify_no_snapshot(&b), C::ForgedTrade);
    }
}

#[test]
fn trades_beyond_the_intents_quantity_are_flagged_where_they_happen() {
    let mut b = close_case(600);
    let mut i = b.intents[0].intent;
    i.qty -= 1000;
    b.intents[0] = sign(1, &i);
    b.refresh_commitments();
    let r = verify_no_snapshot(&b);
    only(&r, C::ForgedTrade);
    assert!(r.findings.iter().any(|f| f.message.contains("exceed its quantity")), "{:#?}", r.findings);
    // exactly the intent's quantity does not exceed it
    valid(&close_case(600));
}

#[test]
fn a_duplicate_nonce_after_the_committed_prefix_is_not_judged() {
    let mut b = close_case(600);
    let dup = b.intents[0].clone();
    b.intents.push(dup);
    valid(&b);
}

#[test]
fn the_intents_prefix_may_not_shrink() {
    let mut b = close_case(600);
    // the second batch commits fewer lines than the first
    b.metas[0].n_intent_lines = 2;
    b.metas[1].n_intent_lines = 1;
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::IntentsHashMismatch);
}

// ---------------------------------------------------------------- fees

#[test]
fn a_fee_on_another_market_or_of_another_kind_is_not_the_expected_one() {
    let mut b = fee_case(0);
    let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Holding, .. }));
    if let E::Fee { market, .. } = b.ev_mut(i) {
        *market = 1;
    }
    let r = verify_no_snapshot(&b);
    only(&r, C::FeeMismatch);
    assert_eq!(r.findings.len(), 2, "the wrong fee, and the night that stays unpaid: {:#?}", r.findings);
    let mut b = fee_case(0);
    let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Holding, .. }));
    if let E::Fee { kind, .. } = b.ev_mut(i) {
        *kind = FeeKind::Commission;
    }
    assert_eq!(verify_no_snapshot(&b).findings.len(), 2);
}

#[test]
fn a_missing_fee_is_reported_at_the_event_that_shows_it() {
    let mut b = fee_case(0);
    let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Holding, .. }));
    b.remove_lines(&[i]);
    let r = verify_no_snapshot(&b);
    only(&r, C::FeeMismatch);
    // events: Fill(1) Meta(2) Meta(3, of the removed fee's batch) Close(4) ...: the next event after the rollover is the Meta (3)
    assert_eq!(r.findings.len(), 1);
    let close_seq = b.events().iter().position(|e| matches!(e, E::Close { .. })).unwrap() as u64 + 1;
    assert_eq!(r.findings[0].seq, Some(close_seq), "the first event dated after the rollover shows it");
}

#[test]
fn a_commission_missing_on_the_last_event_is_found_at_the_end_of_the_log() {
    let mut b = fee_case(20);
    let fees: Vec<usize> = b.lines.iter().enumerate().filter(|(_, l)| matches!(l, GLine::Event { ev: E::Fee { kind: FeeKind::Commission, .. }, .. })).map(|(i, _)| i).collect();
    b.remove_lines(&[*fees.last().unwrap()]);
    let r = verify_no_snapshot(&b);
    only(&r, C::FeeMismatch);
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].seq, Some(b.last_seq()), "attributed to the end of the log");
}

#[test]
fn a_class_with_no_holding_fee_is_charged_nothing() {
    let mut spec = Spec::base();
    spec.holding[0] = 0;
    let t_open = T_1400 + 7 * HOUR;
    let t_roll = T_1400 + 8 * HOUR;
    let script = vec![
        Script::Sample { market: 0, t: t_open, mid: eur(10_800) },
        Script::Order { t: t_open, intent: market_order(0, true, 300_000_000_000, t_open) },
        Script::Sample { market: 0, t: t_roll, mid: eur(10_810) },
        Script::Sample { market: 0, t: t_roll + 600, mid: eur(10_820) },
        Script::Heartbeat { t: t_roll + 600 },
    ];
    let b = Gen::new(spec).run(&script);
    assert!(!b.events().iter().any(|e| matches!(e, E::Fee { .. })));
    valid(&b);
    // a zero fee that nothing calls for is still wrong
    let mut x = b.clone();
    let last = x.lines.len() - 1;
    x.lines.insert(last, GLine::Event { seq: 0, ev: E::Fee { kind: FeeKind::Holding, market: 0, amount: 0, time: t_roll }, proof: None });
    x.renumber();
    only(&verify_no_snapshot(&x), C::FeeMismatch);
}

// ---------------------------------------------------------------- typed lines

fn with_failed_line(b: &mut Bundle, bal: i64, seq: u64, flip_hash: bool) {
    let mut h = b.head_at(b.last_seq());
    if flip_hash {
        h[0] ^= 1;
    }
    b.push_raw(&format!("\"type\":\"account_failed\",\"reason\":0,\"b\":{bal},\"seq\":{seq},\"journal_hash\":\"{}\"", setl8_verify::hexutil::encode(&h)));
}

#[test]
fn account_failed_lines_are_compared_with_the_replay() {
    let base = || {
        // an account at the static floor that failed
        let mut spec = Spec::base();
        spec.markets[0].max_age_secs = 60;
        eur_run(spec, &[mid_for_pnl(-8_000_000_000), eur(10_800)])
    };
    let b0 = base();
    let (bal, seq) = (b0.final_state.b, b0.last_seq());
    let mut ok = b0.clone();
    with_failed_line(&mut ok, bal, seq, false);
    let r = verify_no_snapshot(&ok);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    assert!(r.state.terminal);
    for (b, s, f) in [(bal + 1, seq, false), (bal, seq + 1, false), (bal, seq, true)] {
        let mut x = b0.clone();
        with_failed_line(&mut x, b, s, f);
        only(&verify_no_snapshot(&x), C::StateMismatch);
    }
}

#[test]
fn a_violation_fail_ends_the_account_and_events_after_it_are_refused() {
    let mut b = meta_only(1);
    b.push_raw(&format!("\"type\":\"violation_fail\",\"trader\":\"x\",\"evidence_hash\":\"{}\"", setl8_verify::hexutil::encode(&[1u8; 32])));
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), Vec::<C>::new());
    assert!(r.state.terminal);
    // a second batch after the violation fail: its first event is flagged, the closing Meta is not
    let mut x = meta_only(1);
    x.push_raw(&format!("\"type\":\"violation_fail\",\"trader\":\"x\",\"evidence_hash\":\"{}\"", setl8_verify::hexutil::encode(&[1u8; 32])));
    x.lines.push(GLine::Event { seq: 2, ev: E::Fee { kind: FeeKind::Commission, market: 0, amount: 1, time: T_1400 + 2 }, proof: None });
    let r = verify_no_snapshot(&x);
    assert!(r.findings.iter().any(|f| f.class == C::StateMismatch && f.message.contains("terminal")), "{:#?}", r.findings);
}

#[test]
fn the_same_miss_seen_by_both_archives_is_reported_once() {
    let mut b = breach_then_recovery();
    let c = b.find(|e| matches!(e, E::Close { .. }));
    let br = b.find(|e| matches!(e, E::Breach { .. }));
    b.remove_lines(&[c, br]);
    let from = b.lines.iter().rposition(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap() - 1;
    not_flat_metas(&mut b, from);
    let r = verify_second(&b, b.archive_bytes());
    only(&r, C::MissedBreach);
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].source, "primary");
}

#[test]
fn a_miss_before_the_first_event_has_no_sequence() {
    // nothing but a heartbeat is posted although a limit order was filled earlier
    let mut b = limit_case_first_fill_dropped();
    let r = verify_no_snapshot(&b);
    only(&r, C::TriggerMissed);
    assert_eq!(r.findings[0].seq, None);
    b.lines.clear();
}

fn limit_case_first_fill_dropped() -> Bundle {
    let spec = Spec::base();
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 5) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_798) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_790) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: eur(10_810) },
    ];
    let mut b = Gen::new(spec).run(&script);
    // the evaluator is silent until a later heartbeat
    b.lines.truncate(1);
    b.metas.clear();
    b.lines.push(GLine::Event { seq: 1, ev: E::Meta { time: T_1400 + 90, flat: true, samples_hash: [0; 32], intents_hash: [0; 32], engine_version: 1 }, proof: None });
    b.metas.push(MetaCommit { line: 1, n_samples: b.samples.len(), n_intent_lines: 1 });
    b.refresh_commitments();
    b
}

// ---------------------------------------------------------------- walk scenarios

fn script_eur(extra: Vec<Script>) -> Bundle {
    Gen::new(Spec::base()).run(&extra)
}

#[test]
fn a_filled_limit_order_is_not_expected_again_at_the_next_sample() {
    let b = script_eur(vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 5) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_790) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: eur(10_788) },
        Script::Sample { market: 0, t: T_1400 + 120, mid: eur(10_786) },
        Script::Heartbeat { t: T_1400 + 120 },
    ]);
    assert_eq!(b.events().iter().filter(|e| matches!(e, E::Fill { .. })).count(), 1);
    valid(&b);
}

#[test]
fn a_limit_order_is_live_only_after_its_own_second() {
    // placed at 14:00:30, after the 14:00:30 sample that is already beyond the limit: it fills at the NEXT sample
    let b = script_eur(vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_790) },
        Script::Order { t: T_1400 + 30, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 30) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_788) },
        Script::Heartbeat { t: T_1400 + 60 },
    ]);
    let fill = b.events().into_iter().find_map(|e| if let E::Fill { time, order_kind, .. } = e { Some((*time, *order_kind)) } else { None }).unwrap();
    assert_eq!(fill, (T_1400 + 60, OrderKind::Limit));
    valid(&b);
}

#[test]
fn a_cancel_dated_at_the_triggering_second_comes_too_late() {
    let b = script_eur(vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::Limit, 0, 0, 200_000_000_000, eur(10_795), 0, 0, 0, T_1400 + 5) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_790) },
        Script::Order { t: T_1400 + 60, intent: order(IntentKind::Cancel, 0, 0, 0, 0, 0, 0, 1, T_1400 + 60) },
        Script::Heartbeat { t: T_1400 + 60 },
    ]);
    assert!(b.events().iter().any(|e| matches!(e, E::Fill { .. })));
    valid(&b);
}

#[test]
fn a_stop_loss_set_at_a_sample_second_acts_from_the_next_sample() {
    let b = script_eur(vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 300_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_775) },
        Script::Order { t: T_1400 + 30, intent: order(IntentKind::SetSlTp, 0, 0, 0, 0, eur(10_780), 0, 0, T_1400 + 30) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_770) },
        Script::Heartbeat { t: T_1400 + 60 },
    ]);
    let close = b.events().into_iter().find_map(|e| if let E::Close { time, reason, .. } = e { Some((*time, *reason)) } else { None }).unwrap();
    assert_eq!(close, (T_1400 + 60, CloseReason::StopLoss));
    valid(&b);
}

#[test]
fn a_stop_loss_dated_before_the_position_existed_is_ignored() {
    let b = script_eur(vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400 + 5, intent: order(IntentKind::SetSlTp, 0, 0, 0, 0, eur(10_790), 0, 0, T_1400 + 5) },
        Script::Sample { market: 0, t: T_1400 + 10, mid: eur(10_800) },
        Script::Order { t: T_1400 + 10, intent: market_order(0, true, 300_000_000_000, T_1400 + 10) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_780) },
        Script::Heartbeat { t: T_1400 + 60 },
    ]);
    assert!(!b.events().iter().any(|e| matches!(e, E::Close { .. })));
    valid(&b);
}

#[test]
fn a_breach_leaves_no_room_for_a_stop_loss_on_the_same_sample() {
    let b = script_eur(vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: order(IntentKind::Market, 0, 0, 1_000_000_000_000, 0, eur(10_790), 0, 0, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 30, mid: eur(10_806) },
        Script::Sample { market: 0, t: T_1400 + 60, mid: eur(10_813) },
        Script::Sample { market: 0, t: T_1400 + 90, mid: eur(10_831) },
        Script::Sample { market: 0, t: T_1400 + 120, mid: eur(10_811) },
        Script::Sample { market: 0, t: T_1400 + 150, mid: eur(10_780) },
        Script::Heartbeat { t: T_1400 + 150 },
    ]);
    let closes: Vec<CloseReason> = b.events().into_iter().filter_map(|e| if let E::Close { reason, .. } = e { Some(*reason) } else { None }).collect();
    assert_eq!(closes, vec![CloseReason::Breach], "the stop (1.0790) is also hit, but the breach comes first");
    valid(&b);
}

// ---------------------------------------------------------------- passes and phases

#[test]
fn a_pass_out_of_phase_one_asks_for_no_money() {
    let b = pass_case_in_phase(0, 1);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    let mut ok = b.clone();
    ok.push_pass_requested(1, 0, 108_000_000_000, seq, t);
    let r = verify_no_snapshot(&ok);
    assert_eq!(r.classes(), Vec::<C>::new(), "{:#?}", r.findings);
    let mut bad = b.clone();
    bad.push_pass_requested(1, 8_000_000_000, 108_000_000_000, seq, t);
    let r = verify_no_snapshot(&bad);
    only(&r, C::PassInvalid);
    assert_eq!(r.per_pass[0].reasons, vec!["amount_mismatch".to_string()]);
}

#[test]
fn a_flat_stored_flag_that_disagrees_makes_the_pass_not_flat() {
    let mut b = pass_case(0);
    let last_meta = b.lines.iter().rposition(|l| matches!(l, GLine::Event { ev: E::Meta { .. }, .. })).unwrap();
    if let GLine::Event { ev: E::Meta { flat, .. }, .. } = &mut b.lines[last_meta] {
        *flat = false;
    }
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), vec![C::StateMismatch, C::PassInvalid]);
    assert!(r.per_pass[0].reasons.contains(&"not_flat".to_string()), "{:?}", r.per_pass);
}

#[test]
fn a_failed_phase_cannot_pass() {
    // 4,600,000 EUR (margin about 99,360 of the 99,850 the account has: M9c judges the margin of every fill), a quick profit of
    // about 16,200: credited about 8,100 (>= 1.08 S) but above the quick limit
    let spec = Spec::base();
    let mut script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 4_600_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_836) },
        Script::Order { t: T_1400 + 20, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 20) },
    ];
    script.push(Script::Heartbeat { t: T_1400 + 20 });
    let mut b = Gen::new(spec).run(&script);
    assert!(b.events().iter().any(|e| matches!(e, E::Fail { reason: journal_types::FailReason::PhaseQuick, .. })));
    let bal = b.final_state.b;
    assert!(bal >= 108_000_000_000, "{bal}");
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(1, 0, bal, seq, t);
    let r = verify_no_snapshot(&b);
    only(&r, C::PassInvalid);
    assert!(r.per_pass[0].reasons.contains(&"phase_failed".to_string()), "{:?}", r.per_pass);
}

#[test]
fn the_next_phase_must_be_the_one_the_rules_name() {
    let finalize = |b: &mut Bundle, phase: u8, to: u8, t: i64| {
        b.push_raw(&format!(
            "\"type\":\"pass_finalized\",\"phase\":{phase},\"to_phase\":{to},\"amount\":8000000000,\"request_id\":1,\"tally_count\":1,\"tally_total\":8000000000,\"window_total\":8000000000,\"time\":{t}"
        ));
    };
    // 2 -> 3 is right, 2 -> 4 and 2 -> 0 are not
    for (to, ok) in [(3u8, true), (4, false), (0, false)] {
        let mut b = pass_case(0);
        let t = last_event_time(&b);
        finalize(&mut b, 2, to, t);
        let r = verify_no_snapshot(&b);
        assert_eq!(r.classes().is_empty(), ok, "2 -> {to}: {:#?}", r.findings);
    }
    // 5 -> 0 (graduation) is right, 5 -> 5 and 5 -> 6 are not
    for (to, ok) in [(0u8, true), (5, false), (6, false)] {
        let mut b = pass_case_in_phase(0, 5);
        let t = last_event_time(&b);
        finalize(&mut b, 5, to, t);
        let r = verify_no_snapshot(&b);
        assert_eq!(r.classes().is_empty(), ok, "5 -> {to}: {:#?}", r.findings);
        if ok {
            assert!(r.state.terminal);
        }
    }
    // a phase change from a phase the replay is not in
    let mut b = pass_case(0);
    let t = last_event_time(&b);
    finalize(&mut b, 3, 4, t);
    assert!(verify_no_snapshot(&b).classes().contains(&C::StateMismatch));
}

#[test]
fn each_phase_gets_the_verdict_of_its_own_history() {
    // phase 2 (seq 1..6) is clean, then it passes; phase 3 has a Meta with a wrong engine version
    let mut b = pass_case(0);
    let (seq, t) = (b.last_seq(), last_event_time(&b));
    b.push_pass_requested(2, 8_000_000_000, 108_000_000_000, seq, t);
    b.push_raw(&format!(
        "\"type\":\"pass_finalized\",\"phase\":2,\"to_phase\":3,\"amount\":8000000000,\"request_id\":1,\"tally_count\":1,\"tally_total\":8000000000,\"window_total\":8000000000,\"time\":{}",
        t + 172_800
    ));
    b.lines.push(GLine::Event { seq: seq + 1, ev: E::Meta { time: t + 172_900, flat: true, samples_hash: [0; 32], intents_hash: [0; 32], engine_version: 2 }, proof: None });
    b.metas.push(MetaCommit { line: b.lines.len() - 1, n_samples: b.samples.len(), n_intent_lines: b.intents.len() });
    b.refresh_commitments();
    let r = verify_no_snapshot(&b);
    only(&r, C::EngineVersion);
    assert_eq!(r.per_phase.len(), 2);
    assert_eq!((r.per_phase[0].phase, r.per_phase[0].events, r.per_phase[0].valid), (2, usize::try_from(seq).unwrap(), true));
    assert_eq!((r.per_phase[1].phase, r.per_phase[1].first_seq, r.per_phase[1].last_seq, r.per_phase[1].events, r.per_phase[1].valid), (3, seq + 1, seq + 1, 1, false));
    // an invalid pass marks its own phase only
    let mut c = pass_case(-1);
    let (seq, t) = (c.last_seq(), last_event_time(&c));
    c.push_pass_requested(2, 8_000_000_000, 108_000_000_000 - 1, seq, t);
    let r = verify_no_snapshot(&c);
    assert!(!r.per_phase[0].valid);
}

#[test]
fn the_request_time_moves_the_walk_forward() {
    // the evaluator is silent after 14:01:30; the pass request is dated 14:03:20, after the breach sample at 14:02:30
    let mut b = breach_then_recovery();
    let c = b.find(|e| matches!(e, E::Close { .. }));
    let br = b.find(|e| matches!(e, E::Breach { .. }));
    let mut cut = vec![c, br];
    // drop everything after the FloorRaised batch (the breach batch and the heartbeat)
    let raised_meta = b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::FloorRaised { .. }, .. })).unwrap() + 1;
    cut.extend(raised_meta + 1..b.lines.len());
    b.remove_lines(&cut);
    let seq = b.last_seq();
    b.push_pass_requested(1, 0, 100_000_000_000, seq, T_1400 + 200);
    let r = verify_no_snapshot(&b);
    assert!(r.classes().contains(&C::MissedBreach), "{:#?}", r.findings);
    assert!(r.classes().contains(&C::PassInvalid));
    // the same log without the request is incomplete but not wrong: the walk only runs up to the last Meta
    let mut quiet = b.clone();
    quiet.lines.pop();
    assert_eq!(verify_no_snapshot(&quiet).classes(), Vec::<C>::new());
}


#[test]
fn a_wrong_floor_claim_does_not_poison_what_follows() {
    // TightStart with a wrong l_level, then a sample that does NOT raise the peak: exactly one finding
    let mut b = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_812), eur(10_811)]);
    let i = b.find(|e| matches!(e, E::TightStart { .. }));
    if let E::TightStart { l_level, .. } = b.ev_mut(i) {
        *l_level += 1;
    }
    let r = verify_no_snapshot(&b);
    only(&r, C::FloorMismatch);
    assert_eq!(r.findings.len(), 1, "{:#?}", r.findings);
    // FloorRaised with a wrong l_level, then a lower sample
    let mut b = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_831), eur(10_811), eur(10_811)]);
    let i = b.find(|e| matches!(e, E::FloorRaised { .. }));
    if let E::FloorRaised { l_level, .. } = b.ev_mut(i) {
        *l_level -= 1;
    }
    let r = verify_no_snapshot(&b);
    only(&r, C::FloorMismatch);
    assert_eq!(r.findings.len(), 1, "{:#?}", r.findings);
}

#[test]
fn a_wrong_breach_claim_still_ends_tight_mode_and_locks() {
    let mut b = breach_then_recovery();
    let i = b.find(|e| matches!(e, E::Breach { .. }));
    if let E::Breach { b_after, .. } = b.ev_mut(i) {
        *b_after += 1;
    }
    let r = verify_no_snapshot(&b);
    only(&r, C::FloorMismatch);
    assert_eq!(r.findings.len(), 1, "{:#?}", r.findings);
    let s = &r.state;
    assert_eq!((s.tight_on, s.r, s.g_peak, s.l_level, s.flat), (false, 0, 0, 0, true));
    assert_eq!(s.lock_until, T_1400 + 150 + 43_200);
}

#[test]
fn a_missed_quick_fail_is_attributed_to_the_event_that_carries_on() {
    let mut b = quick_overshoot_case();
    let f = b.find(|e| matches!(e, E::Fail { .. }));
    b.lines[f] = GLine::Event { seq: 0, ev: E::TightEnd { time: T_1400 + 20 }, proof: None };
    b.renumber();
    let tight_end_seq = b.events().iter().position(|e| matches!(e, E::TightEnd { .. })).unwrap() as u64 + 1;
    let r = verify_no_snapshot(&b);
    only(&r, C::MissedFail);
    assert_eq!(r.findings[0].seq, Some(tight_end_seq));
}

fn quick_overshoot_case() -> Bundle {
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 2_000_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 20, mid: eur(10_835) },
        Script::Order { t: T_1400 + 20, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 20) },
        Script::Heartbeat { t: T_1400 + 20 },
    ];
    Gen::new(Spec::base()).run(&script)
}

#[test]
fn the_proof_of_a_floor_event_is_checked_like_any_other() {
    for (which, class) in [("tight", C::ProofFeed), ("raise", C::ProofAge), ("breach", C::ProofShape)] {
        let mut b = breach_then_recovery();
        let line = match which {
            "tight" => b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::TightStart { .. }, .. })),
            "raise" => b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::FloorRaised { .. }, .. })),
            _ => b.lines.iter().position(|l| matches!(l, GLine::Event { ev: E::Breach { .. }, .. })),
        }
        .unwrap();
        if let GLine::Event { proof: Some(p), .. } = &mut b.lines[line] {
            match which {
                "tight" => p.feed_id[0] ^= 1,
                "raise" => p.timestamp_ns -= 61_000_000_000,
                _ => p.signed_update = vec![1],
            }
        }
        let r = verify_no_snapshot(&b);
        assert!(r.classes().contains(&class), "{which}: {:#?}", r.findings);
    }
}

#[test]
fn claim_findings_say_what_was_expected_and_what_was_found() {
    let b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    let honest = b.events().into_iter().find_map(|e| if let E::Close { pnl, credited, .. } = e { Some((*pnl, *credited)) } else { None }).unwrap();
    let mut t = b.clone();
    if let E::Close { credited, .. } = t.ev_mut(i) {
        *credited += 1;
    }
    let f = &verify_no_snapshot(&t).findings[0];
    assert_eq!((f.class, f.expected.clone(), f.found.clone()), (C::CreditMismatch, honest.1.to_string(), (honest.1 + 1).to_string()));
    let q = close_case(20);
    let i = q.find(|e| matches!(e, E::Close { .. }));
    let mut t = q.clone();
    if let E::Close { quick, .. } = t.ev_mut(i) {
        *quick = false;
    }
    let f = &verify_no_snapshot(&t).findings[0];
    assert_eq!((f.class, f.expected.as_str(), f.found.as_str()), (C::QuickMismatch, "true", "false"));
    let mut t = b.clone();
    if let E::Close { pnl, .. } = t.ev_mut(i) {
        *pnl += 1;
    }
    let f = &verify_no_snapshot(&t).findings[0];
    assert_eq!((f.class, f.expected.clone(), f.found.clone()), (C::PnlMismatch, honest.0.to_string(), (honest.0 + 1).to_string()));
}

#[test]
fn a_short_order_signed_as_a_buy_is_reported_with_both_sides() {
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, false, 200_000_000_000, T_1400) },
        Script::Sample { market: 0, t: T_1400 + 600, mid: eur(10_790) },
        Script::Order { t: T_1400 + 600, intent: order(IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, T_1400 + 600) },
    ];
    let b = Gen::new(Spec::base()).run(&script);
    valid(&b);
    let mut t = b.clone();
    let mut i = t.intents[0].intent;
    i.side = 0;
    t.intents[0] = sign(1, &i);
    t.refresh_commitments();
    let r = verify_no_snapshot(&t);
    only(&r, C::ForgedTrade);
    assert_eq!((r.findings[0].expected.as_str(), r.findings[0].found.as_str()), ("short", "long"));
    // and the long fill signed as a sell
    let c = close_case(600);
    let mut t = c.clone();
    let mut i = t.intents[0].intent;
    i.side = 1;
    t.intents[0] = sign(1, &i);
    t.refresh_commitments();
    let r = verify_no_snapshot(&t);
    assert_eq!((r.findings[0].expected.as_str(), r.findings[0].found.as_str()), ("long", "short"));
}

#[test]
fn a_wrong_claim_leaves_the_replayed_floor_state_on_the_samples_numbers() {
    let honest = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_812), eur(10_811)]);
    let hs = verify_no_snapshot(&honest).state;
    let mut b = honest.clone();
    let i = b.find(|e| matches!(e, E::TightStart { .. }));
    if let E::TightStart { l_level, .. } = b.ev_mut(i) {
        *l_level += 1;
    }
    let s = verify_no_snapshot(&b).state;
    assert!(s.tight_on);
    assert_eq!((s.r, s.g_peak, s.l_level, s.b), (hs.r, hs.g_peak, hs.l_level, hs.b));
    // and after a wrong raise
    let honest = eur_run(Spec::base(), &[eur(10_806), eur(10_813), eur(10_831), eur(10_811)]);
    let hs = verify_no_snapshot(&honest).state;
    let mut b = honest.clone();
    let i = b.find(|e| matches!(e, E::FloorRaised { .. }));
    if let E::FloorRaised { l_level, .. } = b.ev_mut(i) {
        *l_level += 1;
    }
    let s = verify_no_snapshot(&b).state;
    assert_eq!((s.r, s.g_peak, s.l_level), (hs.r, hs.g_peak, hs.l_level));
}
