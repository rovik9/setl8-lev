//! Stage 1: every finding class has a tamper test that changes exactly one thing and must produce exactly that class.
mod common;
use common::*;
use journal_types::{CloseReason, FeeKind, JournalEvent as E};
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

// ---------------------------------------------------------------- honest baselines

#[test]
fn honest_small_journals_verify_valid() {
    valid(&meta_only(3));
    valid(&close_case(600));
    valid(&close_case(20));
    valid(&fee_case(0));
    valid(&fee_case(20));
    valid(&design_4_9());
}

#[test]
fn replayed_numbers_match_the_generator() {
    let b = close_case(20);
    let r = verify(&b);
    assert_eq!(r.state.b, b.final_state.b);
    assert_eq!(r.state.q_tally, b.final_state.q_tally);
    assert!(r.state.q_tally > 0, "a quick profit feeds the quick tally");
    assert_eq!(r.state.trades_closed, 1);
    assert_eq!(r.state.seq, b.last_seq());
    assert_eq!(r.state.journal_hash, setl8_verify::hexutil::encode(&b.head()));
}

// ---------------------------------------------------------------- HashChain, SequenceGap, BadGenesis, EventDecode, UnknownEvent

#[test]
fn hash_chain_flipped_bit_in_the_stored_head() {
    let b = close_case(600);
    let mut f = b.files();
    let mut snap: serde_json::Value = serde_json::from_slice(f.snapshot.as_ref().unwrap()).unwrap();
    let mut h = setl8_verify::hexutil::decode(snap["journal_hash"].as_str().unwrap()).unwrap();
    h[0] ^= 1;
    snap["journal_hash"] = serde_json::Value::String(setl8_verify::hexutil::encode(&h));
    f.snapshot = Some(serde_json::to_vec(&snap).unwrap());
    let r = setl8_verify::verify_account(&f.inputs()).unwrap();
    only(&r, C::HashChain);
}

#[test]
fn hash_chain_altered_event_bytes_change_the_head() {
    // the log carries an event that differs (here only in a field nothing checks) from the one the account hashed:
    // every claim verifies, but the replayed head is not the stored one
    let honest = close_case(600);
    let mut b = honest.clone();
    let m = b.metas[1].line;
    if let E::Meta { time, .. } = b.ev_mut(m) {
        *time += 1;
    }
    let mut f = b.files();
    f.snapshot = Some(honest.snapshot_json().into_bytes());
    only(&setl8_verify::verify_account(&f.inputs()).unwrap(), C::HashChain);
}

#[test]
fn sequence_gap_dropped_event() {
    let mut b = meta_only(4);
    b.lines.remove(2); // drop seq 2 (the first line is Opened)
    let r = verify_no_snapshot(&b);
    only(&r, C::SequenceGap);
    assert_eq!(r.findings.len(), 1);
    assert_eq!((r.findings[0].expected.as_str(), r.findings[0].found.as_str()), ("2", "3"));
}

#[test]
fn sequence_gap_reordered_events() {
    let mut b = meta_only(4);
    b.lines.swap(2, 3);
    let r = verify_no_snapshot(&b);
    only(&r, C::SequenceGap);
}

#[test]
fn sequence_gap_snapshot_beyond_the_log() {
    // the last event is missing from the log but the stored account is further on
    let b = meta_only(3);
    let mut short = b.clone();
    short.lines.pop();
    let mut f = short.files();
    f.snapshot = Some(b.snapshot_json().into_bytes());
    let r = setl8_verify::verify_account(&f.inputs()).unwrap();
    only(&r, C::SequenceGap);
}

#[test]
fn bad_genesis() {
    let mut b = meta_only(2);
    if let GLine::Opened(o) = &mut b.lines[0] {
        o.genesis_hash[3] ^= 0x10;
    }
    only(&verify_no_snapshot(&b), C::BadGenesis);
}

#[test]
fn event_decode() {
    let b = meta_only(2);
    let text = b.events_text();
    // cut the last byte of the second Meta's event hex pair
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let n = lines.len();
    let l = lines[n - 1].clone();
    let v: serde_json::Value = serde_json::from_str(&l).unwrap();
    let hex = v["event"].as_str().unwrap();
    let cut = &hex[..hex.len() - 2];
    lines[n - 1] = l.replace(hex, cut);
    let mut f = b.files();
    f.events = (lines.join("\n") + "\n").into_bytes();
    f.snapshot = None;
    only(&setl8_verify::verify_account(&f.inputs()).unwrap(), C::EventDecode);
}

#[test]
fn unknown_event_type() {
    let mut b = meta_only(2);
    b.lines.push(GLine::Raw(format!("{{\"type\":\"mystery\",\"account\":\"{}\"}}", setl8_verify::log::base58(&account_key()))));
    only(&verify_no_snapshot(&b), C::UnknownEvent);
}

// ---------------------------------------------------------------- proofs and archive

fn fill_line(b: &Bundle) -> usize {
    b.find(|e| matches!(e, E::Fill { .. }))
}

fn proof_mut(b: &mut Bundle, line: usize) -> &mut journal_types::PriceProof {
    match &mut b.lines[line] {
        GLine::Event { proof: Some(p), .. } => p,
        _ => panic!("no proof"),
    }
}

#[test]
fn proof_feed_is_not_the_markets_feed() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    proof_mut(&mut b, i).feed_id[0] ^= 1;
    only(&verify_no_snapshot(&b), C::ProofFeed);
}

#[test]
fn proof_age_is_outside_the_markets_window() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    proof_mut(&mut b, i).timestamp_ns -= 61_000_000_000;
    only(&verify_no_snapshot(&b), C::ProofAge);
    // exactly max_age_secs old is fine for the age rule (it then fails the archive lookup, a different class)
    let mut b = close_case(600);
    let i = fill_line(&b);
    proof_mut(&mut b, i).timestamp_ns -= 60_000_000_000;
    only(&verify_no_snapshot(&b), C::ArchiveMissing);
}

#[test]
fn proof_shape_signed_update_on_an_unsigned_source() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    proof_mut(&mut b, i).signed_update = vec![1, 2, 3];
    only(&verify_no_snapshot(&b), C::ProofShape);
}

#[test]
fn archive_missing_proof_value_not_in_the_archive() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    proof_mut(&mut b, i).quantized_value += 1; // below the e12 floor: the price check still passes
    only(&verify_no_snapshot(&b), C::ArchiveMissing);
}

#[test]
fn archive_record_altered_after_the_commitment() {
    let mut b = close_case(600);
    b.samples[0].value += 1;
    // both the lookup of the proof and the hash of the prefix notice
    assert_eq!(verify_no_snapshot(&b).classes(), vec![C::ArchiveMissing, C::SamplesHashMismatch]);
}

#[test]
fn price_mismatch_close_exit_off_by_one_unit() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { exit_e12, pnl, credited, qty, .. } = b.ev_mut(i) {
        *exit_e12 += 1;
        // keep the dependent claims consistent so only the price is wrong: pnl = qty x diff / 1e12
        let d = i64::try_from(u128::from(*qty) / u128::from(E12)).unwrap();
        *pnl += d;
        *credited += d;
        assert_eq!(d, 0, "this scenario's quantity is below 1e12: one e12 unit is below one micro-USD");
    }
    only(&verify_no_snapshot(&b), C::PriceMismatch);
}

#[test]
fn price_mismatch_fill_price_off_by_one_unit() {
    // the Fill's price moves by one e12 unit together with everything that derives from it, so only the price is wrong
    let mut b = close_case(600);
    let i = fill_line(&b);
    let np = match b.ev_mut(i) {
        E::Fill { price_e12, avg_entry_after, margin, .. } => {
            *price_e12 += 1;
            *avg_entry_after += 1;
            *margin += 1; // ceil(qty x price / 50) moves by one micro for this size
            *price_e12
        }
        _ => unreachable!(),
    };
    let ci = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { entry_e12, pnl, credited, .. } = b.ev_mut(ci) {
        *entry_e12 = np;
        *pnl -= 1; // 200,000 EUR x 1e-12 USD = 0.2 micro: the floor moves by one
        *credited -= 1;
    }
    only(&verify_no_snapshot(&b), C::PriceMismatch);
}

#[test]
fn samples_hash_mismatch() {
    let mut b = close_case(600);
    let m = b.metas[0].line;
    if let E::Meta { samples_hash, .. } = b.ev_mut(m) {
        samples_hash[5] ^= 1;
    }
    only(&verify_no_snapshot(&b), C::SamplesHashMismatch);
}

#[test]
fn intents_hash_mismatch() {
    let mut b = close_case(600);
    let m = b.metas[1].line;
    if let E::Meta { intents_hash, .. } = b.ev_mut(m) {
        intents_hash[9] ^= 1;
    }
    only(&verify_no_snapshot(&b), C::IntentsHashMismatch);
}

#[test]
fn samples_prefix_must_not_shrink() {
    let mut b = close_case(600);
    // the second batch commits the (shorter) prefix of the first batch's size
    let first = b.metas[0].n_samples;
    b.metas[1].n_samples = first.saturating_sub(1);
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::SamplesHashMismatch);
}

#[test]
fn proof_sample_beyond_the_committed_prefix() {
    let mut b = close_case(600);
    // the first batch (the Fill) commits an empty prefix of the archive although its proof is record 0
    b.metas[0].n_samples = 0;
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::ArchiveMissing);
}

// ---------------------------------------------------------------- intents

#[test]
fn forged_trade_no_such_intent() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    if let E::Fill { intent_id, .. } = b.ev_mut(i) {
        *intent_id = 99;
    }
    only(&verify_no_snapshot(&b), C::ForgedTrade);
}

#[test]
fn forged_trade_fill_without_an_intent() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    if let E::Fill { intent_id, .. } = b.ev_mut(i) {
        *intent_id = 0;
    }
    only(&verify_no_snapshot(&b), C::ForgedTrade);
}

#[test]
fn forged_trade_intent_is_for_another_market() {
    let mut b = close_case(600);
    b.spec.markets.push(MarketSpec { id: 1, class: 6, source: 2, symbol: "BTCUSD", spread_e6: 50, max_age_secs: 60, sessions: vec![(0, 10_080)] });
    let mut i = b.intents[0].intent;
    i.market = 1;
    b.intents[0] = sign(1, &i);
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::ForgedTrade);
}

#[test]
fn forged_trade_intent_is_the_other_side() {
    let mut b = close_case(600);
    let mut i = b.intents[0].intent;
    i.side = 1;
    b.intents[0] = sign(1, &i);
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::ForgedTrade);
}

#[test]
fn forged_trade_intent_has_another_quantity() {
    let mut b = close_case(600);
    let mut i = b.intents[0].intent;
    i.qty += 1000;
    b.intents[0] = sign(1, &i);
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::ForgedTrade);
}

#[test]
fn forged_trade_evaluator_close_claiming_an_intent_and_trader_close_without_one() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { intent_id, .. } = b.ev_mut(i) {
        *intent_id = 0; // a Manual close needs the trader's intent
    }
    only(&verify_no_snapshot(&b), C::ForgedTrade);
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { reason, .. } = b.ev_mut(i) {
        *reason = CloseReason::Breach; // an evaluator-originated close must carry intent 0
    }
    only(&verify_no_snapshot(&b), C::ForgedTrade);
}

#[test]
fn bad_intent_signature() {
    let mut b = close_case(600);
    b.intents[0].sig[7] ^= 1;
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::BadIntentSignature);
}

#[test]
fn bad_intent_signature_by_another_key() {
    let mut b = close_case(600);
    // signed with the right nonce and order but for ANOTHER account
    let ob = b.intents[0].order.clone();
    let kp = trader_keypair();
    use ed25519_dalek::Signer;
    b.intents[0].sig = kp.sign(&setl8_verify::intents::signing_message(&[1u8; 32], 1, &ob)).to_bytes();
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::BadIntentSignature);
}

#[test]
fn intent_decode_duplicate_nonce() {
    let mut b = close_case(600);
    let dup = b.intents[0].clone();
    b.intents.insert(1, dup);
    b.metas[1].n_intent_lines += 1;
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::IntentDecode);
}

// ---------------------------------------------------------------- market, position, claims

#[test]
fn unknown_market() {
    let mut b = close_case(600);
    for i in [b.find(|e| matches!(e, E::Fill { .. })), b.find(|e| matches!(e, E::Close { .. }))] {
        match b.ev_mut(i) {
            E::Fill { market, .. } | E::Close { market, .. } => *market = 5,
            _ => {}
        }
    }
    // the skipped Fill leaves the account flat: a coherent liar says so in the Meta
    let m = b.metas[0].line;
    if let E::Meta { flat, .. } = b.ev_mut(m) {
        *flat = true;
    }
    only(&verify_no_snapshot(&b), C::UnknownMarket);
}

#[test]
fn position_mismatch_close_larger_than_the_position() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { qty, .. } = b.ev_mut(i) {
        *qty += 1000;
    }
    // the refused close leaves the position open, so a coherent liar also says "not flat" in the last Meta
    let m = b.metas[1].line;
    if let E::Meta { flat, .. } = b.ev_mut(m) {
        *flat = false;
    }
    only(&verify_no_snapshot(&b), C::PositionMismatch);
}

#[test]
fn position_mismatch_close_without_a_fill() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    b.lines.remove(i);
    b.renumber();
    // the batch of the Fill now holds only its Meta
    let r = verify_no_snapshot(&b);
    assert!(r.classes().contains(&C::PositionMismatch), "{:#?}", r.findings);
}

#[test]
fn pnl_mismatch_by_one_micro() {
    for d in [1i64, -1] {
        let mut b = close_case(600);
        let i = b.find(|e| matches!(e, E::Close { .. }));
        if let E::Close { pnl, .. } = b.ev_mut(i) {
            *pnl += d;
        }
        only(&verify_no_snapshot(&b), C::PnlMismatch);
    }
}

#[test]
fn credit_mismatch_by_one_micro() {
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { credited, .. } = b.ev_mut(i) {
        *credited += 1;
    }
    only(&verify_no_snapshot(&b), C::CreditMismatch);
    // a quick profit credited in full instead of half
    let mut b = close_case(20);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { credited, pnl, quick, .. } = b.ev_mut(i) {
        assert!(*quick);
        *credited = *pnl;
    }
    only(&verify_no_snapshot(&b), C::CreditMismatch);
}

#[test]
fn quick_mismatch_flag_and_timer() {
    let mut b = close_case(20);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { quick, .. } = b.ev_mut(i) {
        *quick = false;
    }
    only(&verify_no_snapshot(&b), C::QuickMismatch);
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { timer_start, .. } = b.ev_mut(i) {
        *timer_start += 1;
    }
    only(&verify_no_snapshot(&b), C::QuickMismatch);
}

#[test]
fn quick_boundary_45_seconds() {
    for (age, quick) in [(45i64, true), (46, false)] {
        let b = close_case(age);
        valid(&b);
        let flag = b.events().into_iter().find_map(|e| if let E::Close { quick, .. } = e { Some(*quick) } else { None }).unwrap();
        assert_eq!(flag, quick, "age {age}");
    }
}

#[test]
fn avg_entry_mismatch_and_entry_of_a_close() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    if let E::Fill { avg_entry_after, .. } = b.ev_mut(i) {
        *avg_entry_after += 1;
    }
    only(&verify_no_snapshot(&b), C::AvgEntryMismatch);
    let mut b = close_case(600);
    let i = b.find(|e| matches!(e, E::Close { .. }));
    if let E::Close { entry_e12, .. } = b.ev_mut(i) {
        *entry_e12 += 1;
    }
    let r = verify_no_snapshot(&b);
    assert!(r.classes().contains(&C::AvgEntryMismatch), "{:#?}", r.findings);
}

#[test]
fn margin_mismatch() {
    let mut b = close_case(600);
    let i = fill_line(&b);
    if let E::Fill { margin, .. } = b.ev_mut(i) {
        *margin += 1;
    }
    only(&verify_no_snapshot(&b), C::MarginMismatch);
}

// ---------------------------------------------------------------- fees

#[test]
fn fee_mismatch_holding_fee_wrong_by_one_micro() {
    for d in [1i64, -1] {
        let mut b = fee_case(0);
        let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Holding, .. }));
        if let E::Fee { amount, .. } = b.ev_mut(i) {
            *amount = u64::try_from(i64::try_from(*amount).unwrap() + d).unwrap();
        }
        only(&verify_no_snapshot(&b), C::FeeMismatch);
    }
}

#[test]
fn fee_mismatch_missing_night() {
    let mut b = fee_case(0);
    let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Holding, .. }));
    b.lines.remove(i);
    b.renumber();
    only(&verify_no_snapshot(&b), C::FeeMismatch);
}

#[test]
fn fee_mismatch_fee_on_a_flat_day() {
    let mut b = close_case(600);
    // after the close the account is flat: a holding fee there is wrong
    let last_meta = b.metas.last().unwrap().line;
    let t = match b.events()[b.events().len() - 1] {
        E::Meta { time, .. } => *time,
        _ => unreachable!(),
    };
    b.lines.insert(last_meta, GLine::Event { seq: 0, ev: E::Fee { kind: FeeKind::Holding, market: 0, amount: 5, time: t }, proof: None });
    b.renumber();
    only(&verify_no_snapshot(&b), C::FeeMismatch);
}

#[test]
fn fee_mismatch_commission_amount_and_missing() {
    let mut b = fee_case(20);
    let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Commission, .. }));
    if let E::Fee { amount, .. } = b.ev_mut(i) {
        *amount += 1;
    }
    only(&verify_no_snapshot(&b), C::FeeMismatch);
    let mut b = fee_case(20);
    let i = b.find(|e| matches!(e, E::Fee { kind: FeeKind::Commission, .. }));
    b.lines.remove(i);
    b.renumber();
    only(&verify_no_snapshot(&b), C::FeeMismatch);
}

#[test]
fn holding_fee_is_charged_at_the_rollover_and_not_before() {
    let b = fee_case(0);
    let fee = b.events().into_iter().find_map(|e| if let E::Fee { time, .. } = e { Some(*time) } else { None }).unwrap();
    assert_eq!(fee, T_1400 + 8 * HOUR);
    // the fee after the position was closed (hours later) is "a flat day"; an earlier time is also wrong
    let mut early = fee_case(0);
    let i = early.find(|e| matches!(e, E::Fee { kind: FeeKind::Holding, .. }));
    if let E::Fee { time, .. } = early.ev_mut(i) {
        *time -= 1;
    }
    only(&verify_no_snapshot(&early), C::FeeMismatch);
}

// ---------------------------------------------------------------- floors, state, engine version

#[test]
fn floor_mismatch_level_off_by_one_micro() {
    let mut b = design_4_9();
    let i = b.find(|e| matches!(e, E::TightStart { .. }));
    if let E::TightStart { l_level, .. } = b.ev_mut(i) {
        *l_level += 1;
    }
    only(&verify_no_snapshot(&b), C::FloorMismatch);
    let mut b = design_4_9();
    let i = b.find(|e| matches!(e, E::TightStart { .. }));
    if let E::TightStart { r, .. } = b.ev_mut(i) {
        *r += 1;
    }
    only(&verify_no_snapshot(&b), C::FloorMismatch);
    let mut b = design_4_9();
    let i = b.find(|e| matches!(e, E::FloorRaised { .. }));
    if let E::FloorRaised { l_level, .. } = b.ev_mut(i) {
        *l_level -= 1;
    }
    only(&verify_no_snapshot(&b), C::FloorMismatch);
    let mut b = design_4_9();
    let i = b.find(|e| matches!(e, E::Breach { .. }));
    if let E::Breach { b_after, .. } = b.ev_mut(i) {
        *b_after += 1;
    }
    only(&verify_no_snapshot(&b), C::FloorMismatch);
}

#[test]
fn state_mismatch_snapshot_fields_and_meta_flat() {
    let b = close_case(600);
    for (field, delta) in [("b", 1i64), ("q_tally", 1), ("lock_until", 1), ("trades_closed", 1)] {
        let mut f = b.files();
        let mut snap: serde_json::Value = serde_json::from_slice(f.snapshot.as_ref().unwrap()).unwrap();
        let v = snap[field].as_i64().unwrap() + delta;
        snap[field] = serde_json::json!(v);
        f.snapshot = Some(serde_json::to_vec(&snap).unwrap());
        let r = setl8_verify::verify_account(&f.inputs()).unwrap();
        only(&r, C::StateMismatch);
        assert!(r.findings[0].message.contains(field));
    }
    let mut b = close_case(600);
    let m = b.metas[0].line;
    if let E::Meta { flat, .. } = b.ev_mut(m) {
        *flat = true; // a position is open after the Fill
    }
    only(&verify_no_snapshot(&b), C::StateMismatch);
}

#[test]
fn engine_version() {
    let mut b = meta_only(2);
    let m = b.metas[1].line;
    if let E::Meta { engine_version, .. } = b.ev_mut(m) {
        *engine_version = 2;
    }
    only(&verify_no_snapshot(&b), C::EngineVersion);
}

#[test]
fn false_fail() {
    let mut b = design_4_9();
    let last = b.metas.last().unwrap().clone();
    let t = T_1400 + 150;
    let proof = b.lines.iter().rev().find_map(|l| if let GLine::Event { proof: Some(p), .. } = l { Some(p.clone()) } else { None }).unwrap();
    let b_after = b.final_state.b;
    b.lines.push(GLine::Event { seq: 0, ev: E::Fail { time: t, reason: journal_types::FailReason::StaticFloor, b_after, proof: 0 }, proof: Some(proof) });
    b.lines.push(GLine::Event { seq: 0, ev: E::Meta { time: t, flat: true, samples_hash: [0; 32], intents_hash: [0; 32], engine_version: 1 }, proof: None });
    b.metas.push(MetaCommit { line: b.lines.len() - 1, n_samples: last.n_samples, n_intent_lines: last.n_intent_lines });
    b.renumber();
    b.refresh_commitments();
    only(&verify_no_snapshot(&b), C::FalseFail);
}

#[test]
fn log_that_ends_inside_a_batch_is_flagged() {
    let mut b = close_case(600);
    b.lines.pop(); // the last Meta
    let r = verify_no_snapshot(&b);
    assert_eq!(r.classes(), vec![C::StateMismatch]);
    assert!(r.findings[0].message.contains("ends inside a batch"));
}

// ---------------------------------------------------------------- the file formats

#[test]
fn input_errors_are_typed() {
    let b = close_case(600);
    let f = b.files();
    let mut bad = f.events.clone();
    bad.extend_from_slice(b"{ not json\n");
    let mut g = b.files();
    g.events = bad;
    assert!(setl8_verify::verify_account(&g.inputs()).is_err());
    let mut g = b.files();
    g.archive = vec![1, 2, 3];
    assert!(setl8_verify::verify_account(&g.inputs()).is_err());
    let mut g = b.files();
    g.intents = b"x".to_vec();
    assert!(setl8_verify::verify_account(&g.inputs()).is_err());
    let mut g = b.files();
    g.rules = b"{}".to_vec();
    assert!(setl8_verify::verify_account(&g.inputs()).is_err());
    let mut g = b.files();
    g.snapshot = Some(b"[]".to_vec());
    assert!(setl8_verify::verify_account(&g.inputs()).is_err());
    // a log without an opened line
    let mut g = b.files();
    let text = String::from_utf8(g.events.clone()).unwrap();
    g.events = text.lines().filter(|l| !l.contains("\"opened\"")).collect::<Vec<_>>().join("\n").into_bytes();
    assert!(setl8_verify::verify_account(&g.inputs()).is_err());
}

#[test]
fn deterministic_output() {
    let b = design_4_9();
    let a = serde_json::to_string(&verify(&b)).unwrap();
    let c = serde_json::to_string(&verify(&b)).unwrap();
    assert_eq!(a, c);
}
