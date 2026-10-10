mod common;
use common::*;
use journal_types::*;
use proptest::prelude::*;

// ---------------------------------------------------------------- round trips

#[test]
fn every_event_round_trips() {
    for name in EVENT_ORDER {
        let e = event(name);
        assert_eq!(JournalEvent::decode(&e.to_bytes()).unwrap(), e, "{name}");
        let n = without_proof(&e);
        assert_eq!(JournalEvent::decode(&n.to_bytes()).unwrap(), n, "{name} no proof");
    }
}

#[test]
fn every_proof_round_trips() {
    for name in ["src2", "src3", "src4", "src255"] {
        let p = proof(name);
        assert_eq!(PriceProof::decode(&p.to_bytes().unwrap()).unwrap(), p, "{name}");
    }
}

#[test]
fn batch_round_trips() {
    let b = batch3();
    assert_eq!(JournalBatch::decode(&b.to_bytes().unwrap()).unwrap(), b);
}

#[test]
fn event_tags_follow_declaration_order() {
    for (tag, name) in EVENT_ORDER.iter().enumerate() {
        assert_eq!(event(name).to_bytes()[0], tag as u8, "{name}");
    }
}

#[test]
fn event_encoded_lengths_match_the_design_field_widths() {
    // tag + fields, DESIGN 8.2
    let want = [
        ("Fill", 1 + 2 + 1 + 1 + 8 + 8 + 8 + 8 + 8 + 8 + 8 + 4 + 1),
        ("Close", 1 + 2 + 1 + 8 + 8 + 8 + 8 + 8 + 8 + 1 + 8 + 1 + 4 + 1),
        ("Fee", 1 + 1 + 2 + 8 + 8),
        ("TightStart", 1 + 8 * 6 + 1),
        ("FloorRaised", 1 + 8 * 3 + 1),
        ("TightEnd", 1 + 8),
        ("Breach", 1 + 8 * 3 + 1),
        ("Fail", 1 + 8 + 1 + 8 + 1),
        ("Meta", 1 + 8 + 1 + 32 + 32 + 2),
    ];
    for (name, len) in want {
        assert_eq!(event(name).to_bytes().len(), len, "{name}");
    }
}

#[test]
fn enum_numeric_tables_match_the_design() {
    for (v, s) in [(0u8, Side::Long), (1, Side::Short)] {
        assert_eq!(Side::try_from(v), Ok(s));
        assert_eq!(s.as_u8(), v);
    }
    for (v, s) in [(0u8, OrderKind::Market), (1, OrderKind::Limit), (2, OrderKind::Stop)] {
        assert_eq!(OrderKind::try_from(v), Ok(s));
        assert_eq!(s.as_u8(), v);
    }
    let closes = [CloseReason::Manual, CloseReason::StopLoss, CloseReason::TakeProfit, CloseReason::Flip, CloseReason::Breach, CloseReason::Fail];
    for (v, s) in closes.into_iter().enumerate() {
        assert_eq!(CloseReason::try_from(v as u8), Ok(s));
        assert_eq!(s.as_u8(), v as u8);
    }
    for (v, s) in [(0u8, FeeKind::Holding), (1, FeeKind::Commission)] {
        assert_eq!(FeeKind::try_from(v), Ok(s));
        assert_eq!(s.as_u8(), v);
    }
    for (v, s) in [(0u8, FailReason::StaticFloor), (1, FailReason::PhaseQuick)] {
        assert_eq!(FailReason::try_from(v), Ok(s));
        assert_eq!(s.as_u8(), v);
    }
    assert!(Side::try_from(2).is_err());
    assert!(OrderKind::try_from(3).is_err());
    assert!(CloseReason::try_from(6).is_err());
    assert!(FeeKind::try_from(2).is_err());
    assert!(FailReason::try_from(2).is_err());
    assert!(Side::try_from(255).is_err());
}

// ---------------------------------------------------------------- rejection (item 2)

#[test]
fn rejects_trailing_bytes() {
    let mut b = batch3().to_bytes().unwrap();
    b.push(0);
    assert_eq!(JournalBatch::decode(&b), Err(DecodeError::TrailingBytes));
    let mut e = event("Fee").to_bytes();
    e.push(0);
    assert_eq!(JournalEvent::decode(&e), Err(DecodeError::TrailingBytes));
    let mut p = proof("src2").to_bytes().unwrap();
    p.push(0);
    assert_eq!(PriceProof::decode(&p), Err(DecodeError::TrailingBytes));
}

#[test]
fn rejects_event_count_zero() {
    let raw = raw_batch(1, [0; 32], &[], 0, &[], 0);
    assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::BadEventCount));
}

#[test]
fn event_count_boundary_eight_accepted_nine_rejected() {
    let eight = small_batch(8, 0);
    assert_eq!(JournalBatch::decode(&eight.to_bytes().unwrap()).unwrap(), eight);
    let events: Vec<Vec<u8>> = (0..9).map(tight_end_bytes).collect();
    let raw = raw_batch(1, [0; 32], &events, 9, &[], 0);
    assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::BadEventCount));
    assert!(small_batch(9, 0).to_bytes().is_err());
    assert!(small_batch(0, 0).to_bytes().is_err());
}

#[test]
fn proof_count_boundary_three_accepted_four_rejected() {
    let three = small_batch(1, 3);
    assert_eq!(JournalBatch::decode(&three.to_bytes().unwrap()).unwrap(), three);
    let p = proof("src2").to_bytes().unwrap();
    let raw = raw_batch(1, [0; 32], &[tight_end_bytes(1)], 1, &[p.clone(), p.clone(), p.clone(), p], 4);
    assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::BadProofCount));
    assert!(small_batch(1, 4).to_bytes().is_err());
}

#[test]
fn signed_update_boundary_512_accepted_513_rejected() {
    let mut p = proof("src3");
    p.signed_update = vec![9; 512];
    assert_eq!(PriceProof::decode(&p.to_bytes().unwrap()).unwrap(), p);
    let raw = raw_proof(513, &[9; 513]);
    assert_eq!(PriceProof::decode(&raw), Err(DecodeError::SignedUpdateTooLong));
    p.signed_update = vec![9; 513];
    assert_eq!(p.to_bytes(), Err(EncodeError::SignedUpdateTooLong));
    // inside a batch
    let b = JournalBatch { proofs: vec![p], ..small_batch(1, 0) };
    assert_eq!(b.to_bytes(), Err(EncodeError::SignedUpdateTooLong));
    let raw = raw_batch(1, [0; 32], &[tight_end_bytes(1)], 1, &[raw_proof(513, &[9; 513])], 1);
    assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::SignedUpdateTooLong));
}

#[test]
fn rejects_a_proof_index_that_is_not_255_and_not_below_the_proof_count() {
    let fill_ref = |p: u8| JournalEvent::Fail { time: 1, reason: FailReason::StaticFloor, b_after: 0, proof: p };
    let make = |p: u8, n_proofs: usize| JournalBatch { events: vec![fill_ref(p)], ..small_batch(1, n_proofs) };
    // valid: 255 with no proofs, index 1 with two proofs
    for (p, n) in [(255u8, 0usize), (1, 2), (0, 1)] {
        let b = make(p, n);
        assert_eq!(JournalBatch::decode(&b.to_bytes().unwrap()).unwrap(), b);
    }
    // invalid: index == len, index beyond len, any index with no proofs, 254
    for (p, n) in [(2u8, 2usize), (3, 2), (0, 0), (254, 3), (1, 1)] {
        let b = make(p, n);
        assert_eq!(b.to_bytes(), Err(EncodeError::BadProofIndex), "p={p} n={n}");
        let raw = raw_batch(
            1,
            [7; 32],
            &[fill_ref(p).to_bytes()],
            1,
            &(0..n).map(|_| proof("src2").to_bytes().unwrap()).collect::<Vec<_>>(),
            n as u32,
        );
        assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::BadProofIndex), "p={p} n={n}");
    }
}

#[test]
fn rejects_unknown_enum_values_and_tags() {
    // offsets inside the encoded events: Fill side 3, order_kind 4; Close side 3, quick 52, reason 61; Fee kind 1; Fail reason 9
    let patch = |name: &str, at: usize, v: u8| {
        let mut b = event(name).to_bytes();
        b[at] = v;
        JournalEvent::decode(&b)
    };
    assert_eq!(patch("Fill", 3, 2), Err(DecodeError::UnknownEnumValue)); // side
    assert_eq!(patch("Fill", 4, 3), Err(DecodeError::UnknownEnumValue)); // order_kind
    assert_eq!(patch("Close", 3, 2), Err(DecodeError::UnknownEnumValue)); // side
    assert_eq!(patch("Close", 61, 6), Err(DecodeError::UnknownEnumValue)); // reason
    assert_eq!(patch("Fee", 1, 2), Err(DecodeError::UnknownEnumValue)); // kind
    assert_eq!(patch("Fail", 9, 2), Err(DecodeError::UnknownEnumValue)); // reason
    assert_eq!(patch("Fee", 0, 9), Err(DecodeError::UnknownEventTag));
    assert_eq!(patch("Fee", 0, 255), Err(DecodeError::UnknownEventTag));
    // sanity: the same offsets with valid values decode
    assert!(patch("Fill", 3, 1).is_ok());
    assert!(patch("Close", 61, 5).is_ok());
}

#[test]
fn rejects_booleans_that_are_not_zero_or_one() {
    let mut c = event("Close").to_bytes();
    c[52] = 2;
    assert_eq!(JournalEvent::decode(&c), Err(DecodeError::InvalidBool));
    let mut m = event("Meta").to_bytes();
    m[9] = 2;
    assert_eq!(JournalEvent::decode(&m), Err(DecodeError::InvalidBool));
    c[52] = 0;
    assert!(JournalEvent::decode(&c).is_ok());
    m[9] = 0;
    assert!(JournalEvent::decode(&m).is_ok());
}

#[test]
fn a_length_prefix_claiming_more_than_remains_fails_without_allocating() {
    // huge signed_update length: rejected by the limit
    assert_eq!(PriceProof::decode(&raw_proof(u32::MAX, &[])), Err(DecodeError::SignedUpdateTooLong));
    // within the limit but more than the remaining bytes
    assert_eq!(PriceProof::decode(&raw_proof(100, &[1; 10])), Err(DecodeError::UnexpectedEof));
    // huge event / proof counts
    let raw = raw_batch(1, [0; 32], &[], u32::MAX, &[], 0);
    assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::BadEventCount));
    let raw = raw_batch(1, [0; 32], &[tight_end_bytes(1)], 1, &[], u32::MAX);
    assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::BadProofCount));
    // a count of 3 events with bytes for only one
    let raw = raw_batch(1, [0; 32], &[tight_end_bytes(1)], 3, &[], 0);
    assert_eq!(JournalBatch::decode(&raw), Err(DecodeError::UnexpectedEof));
}

#[test]
fn every_truncation_of_a_valid_batch_is_rejected() {
    let full = batch3().to_bytes().unwrap();
    for cut in 0..full.len() {
        assert!(JournalBatch::decode(&full[..cut]).is_err(), "cut {cut}");
    }
    assert!(JournalBatch::decode(&full).is_ok());
}

#[test]
fn proof_index_helpers() {
    assert_eq!(event("Fill").proof_ref(), Some(0));
    assert_eq!(event("Fill").proof_index(), Some(0));
    assert_eq!(without_proof(&event("Fill")).proof_ref(), Some(255));
    assert_eq!(without_proof(&event("Fill")).proof_index(), None);
    for n in ["Fee", "TightEnd", "Meta"] {
        assert_eq!(event(n).proof_ref(), None, "{n}");
        assert_eq!(event(n).proof_index(), None, "{n}");
    }
    assert_eq!(event("Close").proof_index(), Some(1));
}

#[test]
fn event_time_accessor() {
    let times = [1_760_000_123, 1_760_000_456, 1_760_000_500, 1_760_000_600, 1_760_000_700, 1_760_000_800, 1_760_000_900, 1_760_001_000, 1_760_001_100];
    for (n, t) in EVENT_ORDER.iter().zip(times) {
        assert_eq!(event(n).time(), t, "{n}");
    }
}

// ---------------------------------------------------------------- property tests

fn arb_side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Long), Just(Side::Short)]
}

fn arb_event() -> impl Strategy<Value = JournalEvent> {
    let fill = (any::<u16>(), arb_side(), prop_oneof![Just(OrderKind::Market), Just(OrderKind::Limit), Just(OrderKind::Stop)], any::<u64>(), any::<u64>(), any::<i64>(), any::<u64>(), any::<u64>(), any::<u64>(), any::<u64>(), any::<u32>(), any::<u8>())
        .prop_map(|(market, side, order_kind, qty, price_e12, time, avg_entry_after, margin, sl, tp, intent_id, proof)| JournalEvent::Fill { market, side, order_kind, qty, price_e12, time, avg_entry_after, margin, sl, tp, intent_id, proof });
    let close = (
        (any::<u16>(), arb_side(), any::<u64>(), any::<u64>(), any::<u64>(), any::<i64>(), any::<i64>()),
        (any::<i64>(), any::<bool>(), any::<i64>(), prop_oneof![Just(CloseReason::Manual), Just(CloseReason::StopLoss), Just(CloseReason::TakeProfit), Just(CloseReason::Flip), Just(CloseReason::Breach), Just(CloseReason::Fail)], any::<u32>(), any::<u8>()),
    )
        .prop_map(|((market, side, qty, entry_e12, exit_e12, time, timer_start), (pnl, quick, credited, reason, intent_id, proof))| JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, reason, intent_id, proof });
    let fee = (prop_oneof![Just(FeeKind::Holding), Just(FeeKind::Commission)], any::<u16>(), any::<u64>(), any::<i64>())
        .prop_map(|(kind, market, amount, time)| JournalEvent::Fee { kind, market, amount, time });
    let tight = (any::<i64>(), any::<i64>(), any::<i64>(), any::<i64>(), any::<i64>(), any::<i64>(), any::<u8>())
        .prop_map(|(time, p_open, r, l_start, g_peak, l_level, proof)| JournalEvent::TightStart { time, p_open, r, l_start, g_peak, l_level, proof });
    let raised = (any::<i64>(), any::<i64>(), any::<i64>(), any::<u8>())
        .prop_map(|(time, g_peak, l_level, proof)| JournalEvent::FloorRaised { time, g_peak, l_level, proof });
    let end = any::<i64>().prop_map(|time| JournalEvent::TightEnd { time });
    let breach = (any::<i64>(), any::<i64>(), any::<i64>(), any::<u8>())
        .prop_map(|(time, l_level, b_after, proof)| JournalEvent::Breach { time, l_level, b_after, proof });
    let fail = (any::<i64>(), prop_oneof![Just(FailReason::StaticFloor), Just(FailReason::PhaseQuick)], any::<i64>(), any::<u8>())
        .prop_map(|(time, reason, b_after, proof)| JournalEvent::Fail { time, reason, b_after, proof });
    let meta = (any::<i64>(), any::<bool>(), any::<[u8; 32]>(), any::<[u8; 32]>(), any::<u16>())
        .prop_map(|(time, flat, samples_hash, intents_hash, engine_version)| JournalEvent::Meta { time, flat, samples_hash, intents_hash, engine_version });
    prop_oneof![fill, close, fee, tight, raised, end, breach, fail, meta]
}

fn arb_proof() -> impl Strategy<Value = PriceProof> {
    (any::<u8>(), any::<[u8; 32]>(), any::<u64>(), any::<i128>(), proptest::collection::vec(any::<u8>(), 0..=512))
        .prop_map(|(source, feed_id, timestamp_ns, quantized_value, signed_update)| PriceProof { source, feed_id, timestamp_ns, quantized_value, signed_update })
}

/// Point every proof reference at a real proof (or 255).
fn fix_refs(mut b: JournalBatch) -> JournalBatch {
    let n = b.proofs.len();
    for e in &mut b.events {
        let set = |p: &mut u8| {
            *p = if n == 0 { 255 } else { match usize::from(*p) % (n + 1) { i if i == n => 255, i => i as u8 } };
        };
        match e {
            JournalEvent::Fill { proof, .. }
            | JournalEvent::Close { proof, .. }
            | JournalEvent::TightStart { proof, .. }
            | JournalEvent::FloorRaised { proof, .. }
            | JournalEvent::Breach { proof, .. }
            | JournalEvent::Fail { proof, .. } => set(proof),
            _ => {}
        }
    }
    b
}

fn arb_batch() -> impl Strategy<Value = JournalBatch> {
    (any::<u64>(), any::<[u8; 32]>(), proptest::collection::vec(arb_event(), 1..=8), proptest::collection::vec(arb_proof(), 0..=3))
        .prop_map(|(first_seq, prev_hash, events, proofs)| fix_refs(JournalBatch { first_seq, prev_hash, events, proofs }))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_events_round_trip(e in arb_event()) {
        prop_assert_eq!(JournalEvent::decode(&e.to_bytes()).unwrap(), e);
    }

    #[test]
    fn random_proofs_round_trip(p in arb_proof()) {
        prop_assert_eq!(PriceProof::decode(&p.to_bytes().unwrap()).unwrap(), p);
    }

    #[test]
    fn random_batches_round_trip(b in arb_batch()) {
        let bytes = b.to_bytes().unwrap();
        prop_assert_eq!(JournalBatch::decode(&bytes).unwrap(), b);
    }

    #[test]
    fn random_bytes_never_panic_the_decoder(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = JournalBatch::decode(&bytes);
        let _ = JournalEvent::decode(&bytes);
        let _ = PriceProof::decode(&bytes);
    }

    #[test]
    fn mutated_valid_batches_never_panic_and_never_decode_to_something_that_re_encodes_differently(
        b in arb_batch(), idx in any::<usize>(), val in any::<u8>(), cut in any::<usize>()
    ) {
        let mut bytes = b.to_bytes().unwrap();
        let i = idx % bytes.len();
        bytes[i] = val;
        if let Ok(d) = JournalBatch::decode(&bytes) {
            prop_assert_eq!(d.to_bytes().unwrap(), bytes.clone());
        }
        bytes.truncate(cut % (bytes.len() + 1));
        let _ = JournalBatch::decode(&bytes);
    }
}
