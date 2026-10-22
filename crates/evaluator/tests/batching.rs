//! The SPLIT rule and the shape of batches: event, proof, size and compute limits, atomic units, `Meta`, heartbeat,
//! `report_fail` batches, determinism.
mod common;
use common::*;
use evaluator::batch::tx_size;
use evaluator::compute::recommended_cu_limit;
use evaluator::codec::IntentKind;
use evaluator::*;
use journal_types::{FeeKind, JournalEvent};

fn kinds(b: &Batch) -> Vec<&'static str> {
    b.batch
        .events
        .iter()
        .map(|e| match e {
            JournalEvent::Fill { .. } => "Fill",
            JournalEvent::Close { .. } => "Close",
            JournalEvent::Fee { kind: FeeKind::Holding, .. } => "Holding",
            JournalEvent::Fee { .. } => "Commission",
            JournalEvent::TightStart { .. } => "TightStart",
            JournalEvent::FloorRaised { .. } => "FloorRaised",
            JournalEvent::TightEnd { .. } => "TightEnd",
            JournalEvent::Breach { .. } => "Breach",
            JournalEvent::Fail { .. } => "Fail",
            JournalEvent::Meta { .. } => "Meta",
        })
        .collect()
}

/// `n` markets (half-spread `TEST_HALF_SPREAD_E6`, priced) of class 0 with a position each, opened at T_1400 + 21 h so a 22:00 rollover follows.
fn night_world(n: u16) -> World {
    let markets: Vec<_> = (0..n).map(|i| {
        let mut m = common::market(i, 0, &format!("N{i}"), TEST_HALF_SPREAD_E6);
        m.max_age_secs = 3_600;
        m
    }).collect();
    let mut w = World::new(base_rules(markets), S100K, 2, T_1400 - HOUR);
    let t = T_1400 + 7 * HOUR; // 21:00:00
    for i in 0..n {
        w.sample(i, t + i64::from(i), p4(10_000));
    }
    for i in 0..n {
        w.order(market_order(i, true, MEUR / 100, t + i64::from(n))).unwrap();
    }
    let r = T_1400 + 8 * HOUR;
    for i in 0..n {
        w.sample(i, r - 100 + i64::from(i), p4(10_001));
    }
    w.ev.tick(r + 10);
    w
}

#[test]
fn eight_events_fit_and_nine_split() {
    // 7 positions: 7 holding fees at 22:00:00 + the Meta = 8 events: one batch
    let mut w = night_world(7);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let fee_batches: Vec<_> = bs.iter().filter(|b| kinds(b).contains(&"Holding")).collect();
    assert_eq!(fee_batches.len(), 1);
    assert_eq!(kinds(fee_batches[0]).iter().filter(|k| **k == "Holding").count(), 7);
    assert_eq!(fee_batches[0].batch.events.len(), 8);
    // 8 positions: 8 fees + the Meta would be 9 events: the night's fees are chunked 7 + 1
    let mut w = night_world(8);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let sizes: Vec<usize> = bs.iter().filter(|b| kinds(b).contains(&"Holding")).map(|b| b.batch.events.len()).collect();
    assert_eq!(sizes, vec![8, 2]);
    // every batch of both ends with exactly one Meta and respects the limits
    for b in &bs {
        assert!(b.batch.events.len() <= 8 && b.batch.proofs.len() <= 3 && b.tx_bytes <= 1_232);
        assert!(matches!(b.batch.events.last(), Some(JournalEvent::Meta { .. })));
        assert_eq!(b.batch.events.iter().filter(|e| matches!(e, JournalEvent::Meta { .. })).count(), 1);
    }
    // the markets of a batch are exactly the markets its fee events name
    let first = bs.iter().find(|b| kinds(b).contains(&"Holding")).unwrap();
    assert_eq!(first.markets, (0..7).collect::<Vec<u16>>());
    assert_valid(&w, &bs);
}

#[test]
fn at_most_three_proofs() {
    // four markets, one fill each at four different samples: 4 proofs do not fit one batch
    let markets: Vec<_> = (0..4u16).map(|i| { let mut m = common::market(i, 0, &format!("P{i}"), TEST_HALF_SPREAD_E6); m.max_age_secs = 3_600; m }).collect();
    let mut w = World::new(base_rules(markets), S100K, 2, T_1400 - HOUR);
    for i in 0..4u16 {
        w.sample(i, T_1400 + i64::from(i), p4(10_000));
    }
    for i in 0..4u16 {
        w.order(market_order(i, true, MEUR / 100, T_1400 + 4)).unwrap();
    }
    w.ev.tick(T_1400 + 10);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    assert_eq!(bs.len(), 2);
    assert_eq!(bs[0].batch.proofs.len(), 3);
    assert_eq!(bs[1].batch.proofs.len(), 1);
    assert_eq!(kinds(&bs[0]), vec!["Fill", "Fill", "Fill", "Meta"]);
    // a lower policy limit splits earlier
    let mut w = World::new(base_rules((0..4u16).map(|i| { let mut m = common::market(i, 0, &format!("P{i}"), TEST_HALF_SPREAD_E6); m.max_age_secs = 3_600; m }).collect()), S100K, 2, T_1400 - HOUR);
    for i in 0..4u16 {
        w.sample(i, T_1400 + i64::from(i), p4(10_000));
    }
    for i in 0..4u16 {
        w.order(market_order(i, true, MEUR / 100, T_1400 + 4)).unwrap();
    }
    w.ev.tick(T_1400 + 10);
    let bs = w.ev.drain_batches(&BatchPolicy { max_proofs: 1, ..BatchPolicy::MAX });
    assert_eq!(bs.len(), 4);
    assert_valid(&w, &bs);
}

/// The transaction-size boundary: a batch whose modelled size is exactly `limit` fits, `limit - 1` splits it.
#[test]
fn split_at_exactly_the_size_limit_and_one_byte_over() {
    let build = || {
        let mut w = night_world(5);
        w.ev.tick(T_1400 + 8 * HOUR + 20);
        w
    };
    let mut w = build();
    let whole = w.ev.drain_batches(&BatchPolicy::MAX);
    let fee_batch = whole.iter().find(|b| kinds(b).contains(&"Holding")).unwrap();
    let size = fee_batch.tx_bytes;
    assert_eq!(size, tx_size(fee_batch.kind, fee_batch.markets.len(), fee_batch.batch.to_bytes().unwrap().len(), true));
    // exactly at the limit: same single batch
    let mut w2 = build();
    let at = w2.ev.drain_batches(&BatchPolicy { max_tx_bytes: size, ..BatchPolicy::MAX });
    assert_eq!(at.iter().filter(|b| kinds(b).contains(&"Holding")).count(), 1);
    // one byte less: the fees no longer fit one transaction
    let mut w3 = build();
    let under = w3.ev.drain_batches(&BatchPolicy { max_tx_bytes: size - 1, ..BatchPolicy::MAX });
    assert!(under.iter().filter(|b| kinds(b).contains(&"Holding")).count() >= 1);
    assert!(under.len() > at.len() || under.iter().all(|b| b.tx_bytes < size));
    for b in &under {
        assert!(b.tx_bytes < size || b.batch.events.len() <= 2, "a batch above the limit must be a unit that cannot be split");
    }
    // the budget instruction adds its key and its 8 bytes to every transaction (the default policy counts it, P27)
    let mut w4 = build();
    let without = w4.ev.drain_batches(&BatchPolicy { compute_budget_ix: false, ..BatchPolicy::MAX });
    let b4 = without.iter().find(|b| kinds(b).contains(&"Holding")).unwrap();
    assert_eq!(b4.tx_bytes + 32 + 8, size);
    // and the real chain limit
    assert!(whole.iter().all(|b| b.tx_bytes <= 1_232));
    assert_eq!(BatchPolicy::MAX.max_tx_bytes, 1_232);
}

#[test]
fn compute_limit_splits_too() {
    let mut w = night_world(7);
    let bs = w.ev.drain_batches(&BatchPolicy { max_cu: recommended_cu_limit(BatchKind::PostJournal, 4, 0), ..BatchPolicy::MAX });
    let fee_events: usize = bs.iter().map(|b| kinds(b).iter().filter(|k| **k == "Holding").count()).sum();
    assert_eq!(fee_events, 7);
    // 7 fees are one unit: it cannot be split, so it is emitted alone even above the limit, and it is counted (P27)
    let big = bs.iter().find(|b| kinds(b).contains(&"Holding")).unwrap();
    assert!(big.cu_limit > recommended_cu_limit(BatchKind::PostJournal, 4, 0));
    assert_eq!(w.ev.stats().unpostable, 1);
    // a policy that allows exactly that limit carries it without a count
    let mut w2 = night_world(7);
    let ok = w2.ev.drain_batches(&BatchPolicy { max_cu: big.cu_limit, ..BatchPolicy::MAX });
    assert!(ok.iter().any(|b| b.cu_limit == big.cu_limit));
    assert_eq!(w2.ev.stats().unpostable, 0);
    // one unit less than the limit it asks for: counted again
    let mut w3 = night_world(7);
    w3.ev.drain_batches(&BatchPolicy { max_cu: big.cu_limit - 1, ..BatchPolicy::MAX });
    assert_eq!(w3.ev.stats().unpostable, 1);
}

/// P27: a unit that does not fit a legacy transaction uses the lookup-table form when the policy allows it; one that fits no form is counted.
#[test]
fn a_unit_that_does_not_fit_a_legacy_transaction_uses_the_lookup_table_or_is_unpostable() {
    let build = || {
        let mut w = night_world(2);
        w.ev.tick(T_1400 + 8 * HOUR + 20);
        w
    };
    let one = BatchPolicy::MAX.with_max_events(2);
    let first = build().ev.drain_batches(&one)[0].clone();
    let size = first.tx_bytes;
    assert_eq!(first.tx_format, TxFormat::Legacy);
    // exactly at the limit: legacy, nothing counted
    let mut w = build();
    let at = w.ev.drain_batches(&BatchPolicy { max_tx_bytes: size, ..one });
    assert_eq!((at[0].tx_format, at[0].tx_bytes), (TxFormat::Legacy, size));
    assert_eq!(w.ev.stats().unpostable, 0);
    // one byte less and no lookup table allowed: the unit is emitted alone and counted
    let mut w = build();
    let no = w.ev.drain_batches(&BatchPolicy { max_tx_bytes: size - 1, allow_lookup_table: false, ..one });
    assert_eq!((no[0].tx_format, no[0].tx_bytes), (TxFormat::Legacy, size));
    assert!(w.ev.stats().unpostable >= 1);
    // one byte less with the lookup table: the smaller form, exactly the size the formula gives, nothing counted
    let mut w = build();
    let yes = w.ev.drain_batches(&BatchPolicy { max_tx_bytes: size - 1, ..one });
    assert_eq!(yes[0].tx_format, TxFormat::V0Lookup);
    assert_eq!(yes[0].tx_bytes, evaluator::batch::tx_size_v0(yes[0].kind, yes[0].markets.len(), yes[0].batch.to_bytes().unwrap().len(), true));
    assert!(yes[0].tx_bytes < size);
    assert_eq!(w.ev.stats().unpostable, 0);
    // a size no form meets: every unit is counted, each is its own batch
    let mut w = build();
    let tiny = w.ev.drain_batches(&BatchPolicy { max_tx_bytes: 100, ..one });
    assert!(tiny.len() >= 2);
    assert_eq!(w.ev.stats().unpostable as usize, tiny.len());
    // and a batch that fits the lookup-table form exactly is not counted
    let mut w = build();
    let exact = w.ev.drain_batches(&BatchPolicy { max_tx_bytes: yes[0].tx_bytes, ..one });
    assert_eq!(exact[0].tx_format, TxFormat::V0Lookup);
    assert_eq!(w.ev.stats().unpostable, 0);
}

#[test]
fn the_meta_time_never_passes_a_queued_holding_fee() {
    // 2 positions; a batch of at most 2 events holds one fee + Meta: the Meta before the second fee must be dated R - 1
    let mut w = night_world(2);
    let r = T_1400 + 8 * HOUR;
    let bs = w.ev.drain_batches(&BatchPolicy::MAX.with_max_events(2));
    let fee_batches: Vec<&Batch> = bs.iter().filter(|b| kinds(b).contains(&"Holding")).collect();
    assert_eq!(fee_batches.len(), 1, "one night's fees are one unit: they travel together");
    assert_eq!(fee_batches[0].batch.events.len(), 3);
    // the batch before the fees has its Meta before R
    let before = bs.iter().take_while(|b| !kinds(b).contains(&"Holding")).last().unwrap();
    assert!(before.meta_time < r);
    assert_valid(&w, &bs);
}

#[test]
fn flips_and_tight_end_units_are_not_split() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    let t = T_1400;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 100, p4(10_020));
    w.order(market_order(0, false, 2 * MEUR, t + 100)).unwrap(); // a flip
    w.sample(0, t + 110, p4(10_020));
    w.ev.tick(t + 111);
    // at most 3 events per batch (2 + Meta): the flip's Close and Fill must still travel together
    let bs = w.ev.drain_batches(&BatchPolicy::MAX.with_max_events(3));
    let flip = bs.iter().find(|b| kinds(b).contains(&"Close")).unwrap();
    assert_eq!(kinds(flip), vec!["Close", "Fill", "Meta"]);
    assert_valid(&w, &bs);
    // a TightEnd needs a batch that ends flat: a close (ending tight mode) and a new fill in the same second go in two batches
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 10, p4(10_030));
    w.sample(0, t + 20, p4(10_030));
    w.order(close_order(0, 0, t + 20)).unwrap();
    w.order(market_order(0, true, MEUR / 10, t + 20)).unwrap();
    w.sample(0, t + 30, p4(10_031));
    w.ev.tick(t + 31);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let end = bs.iter().find(|b| kinds(b).contains(&"TightEnd")).unwrap();
    assert_eq!(kinds(end).last(), Some(&"Meta"));
    assert!(end.account_after.flat, "the batch holding a TightEnd ends flat");
    assert!(!kinds(end).contains(&"Fill") || kinds(end).iter().position(|k| *k == "Fill") < kinds(end).iter().position(|k| *k == "TightEnd"));
    assert_valid(&w, &bs);
}

#[test]
fn a_report_fail_batch_ends_fail_then_meta_and_flat() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, 45 * MEUR / 10, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.sample(0, T_1400 + 10, p4(9_000));
    w.ev.tick(T_1400 + 11);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true));
    let last = bs.last().unwrap();
    assert!(matches!(last.kind, BatchKind::ReportFail(engine::FailReason::StaticFloor)));
    let k = kinds(last);
    assert_eq!(&k[k.len() - 2..], &["Fail", "Meta"]);
    assert!(last.account_after.failed && last.account_after.flat);
    assert!(bs[..bs.len() - 1].iter().all(|b| b.kind == BatchKind::PostJournal));
    // the transaction for a report_fail carries more accounts
    assert_eq!(last.tx_bytes, tx_size(last.kind, last.markets.len(), last.batch.to_bytes().unwrap().len(), true));
    assert_valid(&w, &bs);
}

#[test]
fn heartbeats_and_meta_hashes() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w.sample(0, T_1400 + 30, p4(10_001));
    // nothing queued, time moved on: a heartbeat carries only a Meta
    assert!(w.ev.drain_batches(&BatchPolicy::MAX).is_empty());
    let hb = w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true));
    assert_eq!(hb.len(), 1);
    assert_eq!(kinds(&hb[0]), vec!["Meta"]);
    assert_eq!(hb[0].meta_time, T_1400 + 29);
    // no time moved since: no second heartbeat
    assert!(w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true)).is_empty());
    // the hashes are sha256 of the committed prefixes: the samples with a second up to Meta.time, the whole intents file
    let JournalEvent::Meta { samples_hash, intents_hash, time, flat, engine_version } = hb[0].batch.events[0].clone() else { panic!() };
    assert_eq!((time, flat, engine_version), (T_1400 + 29, true, 1));
    let stream = w.ev.sample_stream();
    use sha2::{Digest, Sha256};
    let want: [u8; 32] = Sha256::digest(&stream[..68]).into();
    assert_eq!(samples_hash, want, "only the first sample (second T_1400) is at or before T_1400 + 29");
    assert_eq!(hb[0].samples_committed, 1);
    let want_i: [u8; 32] = Sha256::digest(w.ev.intents_bytes()).into();
    assert_eq!(intents_hash, want_i);
    assert_eq!(hb[0].intents_committed_bytes, w.ev.intents_bytes().len());
}

#[test]
fn chain_head_and_sequence_follow_the_batches() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 10, p4(10_000));
    w.ev.tick(T_1400 + 11);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX.with_max_events(2));
    let mut head = journal_types::genesis_hash(&journal_types::NativeSha256, &account_key());
    let mut seq = 0u64;
    for b in &bs {
        assert_eq!(b.batch.first_seq, seq + 1);
        assert_eq!(b.batch.prev_hash, head);
        let (h, last) = journal_types::apply_batch(&journal_types::NativeSha256, &head, b.batch.first_seq, &b.batch).unwrap();
        head = h;
        seq = last;
        assert_eq!((b.account_after.seq, b.account_after.journal_hash), (seq, head));
    }
    assert_eq!((w.ev.seq(), w.ev.head()), (seq, head));
}

#[test]
fn the_same_inputs_give_the_same_bytes() {
    let run = || {
        let mut w = fx_world(30);
        w.sample(0, T_1400, p4(10_800));
        w.order(market_order(0, true, 1_000_000_000_000, T_1400)).unwrap();
        for (dt, p) in [(30, 10_806), (60, 10_813), (90, 10_831), (120, 10_811), (150, 10_780)] {
            w.sample(0, T_1400 + dt, p4(p));
        }
        w.order(order(IntentKind::Limit, 0, 0, MEUR / 10, p4(10_000), 0, 0, 0, T_1400 + 150)).unwrap();
        w.ev.tick(T_1400 + 200);
        let bs = w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true));
        let bytes: Vec<Vec<u8>> = bs.iter().map(|b| b.batch.to_bytes().unwrap()).collect();
        (bytes, w.ev.archive_bytes().to_vec(), w.ev.intents_bytes().to_vec(), w.ev.head())
    };
    assert_eq!(run(), run());
}
