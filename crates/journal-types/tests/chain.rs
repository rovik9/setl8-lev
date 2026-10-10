mod common;
use common::*;
use journal_types::*;

/// Five-event chain: Fill(p0), Fee, Close(p1), TightEnd, Meta; seq 1..=5 from the genesis hash.
fn honest() -> Vec<ChainEntry> {
    let proofs = proof_list();
    let events = [event("Fill"), event("Fee"), event("Close"), event("TightEnd"), event("Meta")];
    let mut prev = genesis();
    events
        .into_iter()
        .enumerate()
        .map(|(i, ev)| {
            let proof = ev.proof_index().map(|idx| proofs[idx].clone());
            let seq = 1 + i as u64;
            prev = next_hash(&sha(), &prev, seq, &ev, proof.as_ref()).unwrap();
            ChainEntry { seq, event: ev, proof, hash: prev }
        })
        .collect()
}

/// Recompute the head from the (possibly tampered) entries, ignoring the recorded hashes.
fn recompute(start: &[u8; 32], entries: &[ChainEntry]) -> [u8; 32] {
    let mut h = *start;
    for e in entries {
        h = next_hash(&sha(), &h, e.seq, &e.event, e.proof.as_ref()).unwrap();
    }
    h
}

fn final_hash(entries: &[ChainEntry]) -> [u8; 32] {
    entries.last().unwrap().hash
}

#[test]
fn the_honest_chain_verifies_and_matches_apply_batch() {
    let entries = honest();
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &entries), Ok(final_hash(&entries)));
    let batch = JournalBatch {
        first_seq: 1,
        prev_hash: genesis(),
        events: entries.iter().map(|e| e.event.clone()).collect(),
        proofs: proof_list(),
    };
    assert_eq!(apply_batch(&sha(), &genesis(), 1, &batch), Ok((final_hash(&entries), 5)));
}

#[test]
fn an_empty_chain_verifies_to_the_start_hash() {
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &[]), Ok(genesis()));
}

#[test]
fn inserting_an_event_breaks_at_the_insertion_point() {
    let good = honest();
    let mut t = good.clone();
    t.insert(2, ChainEntry { seq: 3, event: JournalEvent::Fee { kind: FeeKind::Holding, market: 1, amount: 1, time: 5 }, proof: None, hash: good[1].hash });
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 2, kind: ChainBreakKind::BadHash }));
    // inserted with its neighbour's seq: the seq already gives it away
    let mut t2 = good.clone();
    t2.insert(2, ChainEntry { seq: 4, event: good[3].event.clone(), proof: None, hash: good[2].hash });
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t2), Err(ChainBreak { index: 2, kind: ChainBreakKind::BadSeq }));
    assert_ne!(recompute(&genesis(), &t), final_hash(&good));
}

#[test]
fn dropping_an_event_breaks_at_the_gap() {
    let good = honest();
    let mut t = good.clone();
    t.remove(1);
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 1, kind: ChainBreakKind::BadSeq }));
    // even if the seqs are renumbered, the hashes no longer fit
    let mut r = t.clone();
    for (i, e) in r.iter_mut().enumerate() {
        e.seq = 1 + i as u64;
    }
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &r), Err(ChainBreak { index: 1, kind: ChainBreakKind::BadHash }));
    assert_ne!(recompute(&genesis(), &r), final_hash(&good));
}

#[test]
fn dropping_the_first_event_breaks_at_index_zero() {
    let mut t = honest();
    t.remove(0);
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 0, kind: ChainBreakKind::BadSeq }));
}

#[test]
fn dropping_the_last_event_leaves_a_valid_prefix_with_a_different_head() {
    let good = honest();
    let mut t = good.clone();
    t.pop();
    let head = verify_chain(&sha(), &genesis(), 1, &t).unwrap();
    assert_ne!(head, final_hash(&good), "the caller must compare the head with the account's journal_hash");
}

#[test]
fn swapping_two_events_breaks_at_the_first_swapped_index() {
    let good = honest();
    let mut t = good.clone();
    t.swap(1, 2);
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 1, kind: ChainBreakKind::BadSeq }));
    // swap only the events, keeping seq and hash slots
    let mut u = good.clone();
    let (a, b) = (u[1].event.clone(), u[2].event.clone());
    u[1].event = b;
    u[2].event = a;
    // the proof slot of entry 1 (Fee, none) no longer fits a Close that references a proof
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &u), Err(ChainBreak { index: 1, kind: ChainBreakKind::BadProof }));
    assert_ne!(recompute(&genesis(), &t), final_hash(&good));
}

#[test]
fn altering_one_byte_of_an_event_breaks_at_that_event() {
    let good = honest();
    let mut t = good.clone();
    if let JournalEvent::Fee { amount, .. } = &mut t[1].event {
        *amount += 1;
    }
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 1, kind: ChainBreakKind::BadHash }));
    assert_ne!(recompute(&genesis(), &t), final_hash(&good));

    // and each field of a Fill / Close alters the hash
    let base = next_hash(&sha(), &genesis(), 1, &event("Fill"), Some(&proof("src2"))).unwrap();
    let mut variants: Vec<JournalEvent> = Vec::new();
    for tweak in 0..12 {
        if let JournalEvent::Fill { market, qty, price_e12, time, avg_entry_after, margin, sl, tp, intent_id, side, order_kind, .. } = event("Fill") {
            let mut m = (market, qty, price_e12, time, avg_entry_after, margin, sl, tp, intent_id, side, order_kind);
            match tweak {
                0 => m.0 += 1,
                1 => m.1 += 1,
                2 => m.2 += 1,
                3 => m.3 += 1,
                4 => m.4 += 1,
                5 => m.5 += 1,
                6 => m.6 += 1,
                7 => m.7 += 1,
                8 => m.8 += 1,
                9 => m.9 = Side::Long,
                10 => m.10 = OrderKind::Market,
                _ => {}
            }
            variants.push(JournalEvent::Fill { market: m.0, side: m.9, order_kind: m.10, qty: m.1, price_e12: m.2, time: m.3, avg_entry_after: m.4, margin: m.5, sl: m.6, tp: m.7, intent_id: m.8, proof: 0 });
        }
    }
    for (i, v) in variants.iter().enumerate() {
        let h = next_hash(&sha(), &genesis(), 1, v, Some(&proof("src2"))).unwrap();
        if i == 11 {
            assert_eq!(h, base, "the untouched copy hashes the same");
        } else {
            assert_ne!(h, base, "tweak {i} must change the hash");
        }
    }
}

#[test]
fn altering_a_seq_breaks_at_that_entry() {
    let good = honest();
    let mut t = good.clone();
    t[3].seq += 1;
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 3, kind: ChainBreakKind::BadSeq }));
    // the seq is inside the hash
    let a = next_hash(&sha(), &genesis(), 7, &event("TightEnd"), None).unwrap();
    let b = next_hash(&sha(), &genesis(), 8, &event("TightEnd"), None).unwrap();
    assert_ne!(a, b);
    // changing the first seq expectation also breaks at index 0
    assert_eq!(verify_chain(&sha(), &genesis(), 2, &good), Err(ChainBreak { index: 0, kind: ChainBreakKind::BadSeq }));
}

#[test]
fn altering_a_proof_breaks_at_that_entry() {
    let good = honest();
    for (idx, field) in [(0usize, 0), (0, 1), (0, 2), (2, 3)] {
        let mut t = good.clone();
        let p = t[idx].proof.as_mut().unwrap();
        match field {
            0 => p.timestamp_ns += 1,
            1 => p.quantized_value += 1,
            2 => p.feed_id[0] ^= 1,
            _ => p.signed_update[0] ^= 1,
        }
        assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: idx, kind: ChainBreakKind::BadHash }), "idx {idx} field {field}");
        assert_ne!(recompute(&genesis(), &t), final_hash(&good));
    }
    // proof removed or added where the event does not expect it
    let mut t = good.clone();
    t[0].proof = None;
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 0, kind: ChainBreakKind::BadProof }));
    let mut t = good.clone();
    t[1].proof = Some(proof("src2"));
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 1, kind: ChainBreakKind::BadProof }));
}

#[test]
fn changing_prev_hash_breaks_at_index_zero() {
    let good = honest();
    let mut other = genesis();
    other[0] ^= 1;
    assert_eq!(verify_chain(&sha(), &other, 1, &good), Err(ChainBreak { index: 0, kind: ChainBreakKind::BadHash }));
    assert_ne!(recompute(&other, &good), final_hash(&good));
}

#[test]
fn a_recorded_hash_that_is_wrong_breaks_there() {
    let mut t = honest();
    t[4].hash[31] ^= 1;
    assert_eq!(verify_chain(&sha(), &genesis(), 1, &t), Err(ChainBreak { index: 4, kind: ChainBreakKind::BadHash }));
}

#[test]
fn verify_chain_rejects_seq_zero_and_overflow() {
    let one = [ChainEntry { seq: 0, event: event("TightEnd"), proof: None, hash: [0; 32] }];
    assert_eq!(verify_chain(&sha(), &genesis(), 0, &one), Err(ChainBreak { index: 0, kind: ChainBreakKind::BadSeq }));
    let two = [
        ChainEntry { seq: u64::MAX, event: event("TightEnd"), proof: None, hash: [0; 32] },
        ChainEntry { seq: 1, event: event("TightEnd"), proof: None, hash: [0; 32] },
    ];
    let r = verify_chain(&sha(), &genesis(), u64::MAX, &two);
    assert_eq!(r, Err(ChainBreak { index: 0, kind: ChainBreakKind::BadHash }));
    let first = next_hash(&sha(), &genesis(), u64::MAX, &event("TightEnd"), None).unwrap();
    let two = [
        ChainEntry { seq: u64::MAX, event: event("TightEnd"), proof: None, hash: first },
        ChainEntry { seq: 1, event: event("TightEnd"), proof: None, hash: [0; 32] },
    ];
    assert_eq!(verify_chain(&sha(), &genesis(), u64::MAX, &two), Err(ChainBreak { index: 1, kind: ChainBreakKind::SeqOverflow }));
}

#[test]
fn next_hash_demands_a_proof_exactly_when_the_event_references_one() {
    let h = genesis();
    assert_eq!(next_hash(&sha(), &h, 1, &event("Fill"), None), Err(ChainError::ProofMismatch));
    assert_eq!(next_hash(&sha(), &h, 1, &without_proof(&event("Fill")), Some(&proof("src2"))), Err(ChainError::ProofMismatch));
    assert_eq!(next_hash(&sha(), &h, 1, &event("Fee"), Some(&proof("src2"))), Err(ChainError::ProofMismatch));
    assert_eq!(next_hash(&sha(), &h, 1, &event("Meta"), Some(&proof("src2"))), Err(ChainError::ProofMismatch));
    assert!(next_hash(&sha(), &h, 1, &event("Fill"), Some(&proof("src2"))).is_ok());
    assert!(next_hash(&sha(), &h, 1, &without_proof(&event("Fill")), None).is_ok());
    // a proof that cannot be encoded
    let mut big = proof("src2");
    big.signed_update = vec![0; 513];
    assert_eq!(next_hash(&sha(), &h, 1, &event("Fill"), Some(&big)), Err(ChainError::Encode(EncodeError::SignedUpdateTooLong)));
}

#[test]
fn apply_batch_rejects_seq_zero_overflow_and_invalid_batches() {
    let b = batch3();
    assert_eq!(apply_batch(&sha(), &b.prev_hash, 0, &b), Err(ChainError::ZeroFirstSeq));
    // 3 events starting at u64::MAX - 1 end exactly at u64::MAX; starting at u64::MAX - 0 overflows
    assert_eq!(apply_batch(&sha(), &b.prev_hash, u64::MAX - 2, &b).map(|r| r.1), Ok(u64::MAX));
    assert_eq!(apply_batch(&sha(), &b.prev_hash, u64::MAX - 1, &b), Err(ChainError::SeqOverflow));
    let one = small_batch(1, 0);
    assert_eq!(apply_batch(&sha(), &one.prev_hash, u64::MAX, &one).map(|r| r.1), Ok(u64::MAX));
    // invalid batches
    let empty = small_batch(0, 0);
    assert_eq!(apply_batch(&sha(), &empty.prev_hash, 1, &empty), Err(ChainError::Encode(EncodeError::BadEventCount)));
    let mut bad_ref = batch3();
    bad_ref.proofs.pop();
    assert_eq!(apply_batch(&sha(), &bad_ref.prev_hash, 1, &bad_ref), Err(ChainError::Encode(EncodeError::BadProofIndex)));
}

#[test]
fn apply_batch_uses_the_given_head_not_the_batch_field() {
    let b = batch3();
    let other = [9u8; 32];
    let a = apply_batch(&sha(), &b.prev_hash, 1, &b).unwrap();
    let c = apply_batch(&sha(), &other, 1, &b).unwrap();
    assert_ne!(a.0, c.0);
}

#[test]
fn two_batches_chain_like_one() {
    let all = honest();
    let first = JournalBatch { first_seq: 1, prev_hash: genesis(), events: all[..2].iter().map(|e| e.event.clone()).collect(), proofs: vec![proof("src2")] };
    let (h2, s2) = apply_batch(&sha(), &genesis(), 1, &first).unwrap();
    assert_eq!((h2, s2), (all[1].hash, 2));
    let second = JournalBatch { first_seq: 3, prev_hash: h2, events: all[2..].iter().map(|e| e.event.clone()).collect(), proofs: vec![proof("src2"), proof("src3")] };
    // the Close references proof 1 of the SECOND batch's list
    let (h5, s5) = apply_batch(&sha(), &h2, 3, &second).unwrap();
    assert_eq!((h5, s5), (all[4].hash, 5));
}
