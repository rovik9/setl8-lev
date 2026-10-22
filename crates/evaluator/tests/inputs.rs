//! Configuration, samples, the archive and intents files, and the clock.
mod common;
use common::*;
use evaluator::codec::{IntentKind, OrderIntent, SampleRec};
use evaluator::*;
use setl8_verify::archive::{Archive, ArchiveWriter, Sample};

fn good() -> (RulesCfg, AccountParams) {
    (base_rules(vec![eur_market(0)]), params(S100K, 2, T_1400))
}

fn new_err(f: impl FnOnce(&mut RulesCfg, &mut AccountParams)) -> ConfigError {
    let (mut r, mut p) = good();
    f(&mut r, &mut p);
    match Evaluator::new(r, p, Box::new(AcceptAll)) {
        Ok(_) => panic!("expected a configuration error"),
        Err(e) => e,
    }
}

#[test]
fn configuration_errors() {
    assert!(Evaluator::new(good().0, good().1, Box::new(AcceptAll)).is_ok());
    assert_eq!(new_err(|_, p| p.size_micro = 0), ConfigError::BadSize);
    assert_eq!(new_err(|_, p| p.size_micro = -1_000_000), ConfigError::BadSize);
    assert_eq!(new_err(|_, p| p.size_micro = 100_000_500_000), ConfigError::BadSize, "not a whole number of dollars");
    assert_eq!(new_err(|_, p| p.phase = 0), ConfigError::BadPhase);
    assert_eq!(new_err(|_, p| p.phase = 6), ConfigError::BadPhase);
    assert_eq!(new_err(|r, _| r.leverage_x[3] = 0), ConfigError::BadLeverage);
    assert_eq!(new_err(|r, _| r.markets[0].class = 8), ConfigError::BadMarket(0));
    assert_eq!(new_err(|r, _| r.markets[0].sessions = vec![(0, 1); 7]), ConfigError::BadMarket(0));
    assert_eq!(new_err(|r, _| r.markets[0].qty_step = 0), ConfigError::BadMarket(0));
    assert_eq!(new_err(|r, _| r.markets[0].max_age_secs = 0), ConfigError::BadMarket(0));
    assert_eq!(new_err(|r, _| r.markets[0].half_spread_e6 = 1_000_001), ConfigError::BadMarket(0));
    assert_eq!(new_err(|r, _| r.markets.push(eur_market(0))), ConfigError::DuplicateMarket(0));
    // boundaries that are fine
    let (mut r, mut p) = good();
    r.markets[0].half_spread_e6 = 1_000_000;
    r.markets[0].sessions = vec![(0, 1); 6];
    p.phase = 5;
    assert!(Evaluator::new(r, p, Box::new(AcceptAll)).is_ok());
}

#[test]
fn a_new_evaluator_starts_clean() {
    let w = World::base();
    let ev = &w.ev;
    assert_eq!((ev.seq(), ev.trades_closed(), ev.phase(), ev.queued_events(), ev.sample_count()), (0, 0, 2, 0, 0));
    assert_eq!(ev.head(), journal_types::genesis_hash(&journal_types::NativeSha256, &account_key()));
    assert_eq!(ev.state().b, S100K);
    assert!(ev.positions().is_empty() && ev.pending().is_empty());
    assert!(matches!(ev.status(), Status::Active) && ev.fault().is_none() && !ev.is_ended() && !ev.is_frozen());
    assert_eq!((ev.clock(), ev.processed_through()), (T_1400 - HOUR, T_1400 - HOUR));
    assert_eq!(ev.archive_bytes(), &evaluator::codec::archive_header());
    assert_eq!(ev.intents_bytes(), evaluator::codec::intents_header_line().as_bytes());
    assert_eq!(ev.quick_headroom(), 2_800_000_000);
}

#[test]
fn sample_errors() {
    let mut w = World::base();
    let t = T_1400;
    let ns = |s: i64| (s as u64) * 1_000_000_000;
    let rcv = ns(t) + 700_000_000;
    assert_eq!(w.ev.on_sample(7, ns(t), 1_000_000_000_000_000_000, 0, [0; 32]), Err(SampleError::UnknownMarket));
    assert_eq!(w.ev.on_sample(0, ns(t), 0, 0, [0; 32]), Err(SampleError::BadValue));
    assert_eq!(w.ev.on_sample(0, ns(t), -5, 0, [0; 32]), Err(SampleError::BadValue));
    assert_eq!(w.ev.on_sample(0, ns(t), 999_999, 0, [0; 32]), Err(SampleError::BadValue), "floors to a zero price");
    assert_eq!(w.ev.sample_count(), 0);
    assert!(w.ev.on_sample(0, ns(t) + 600_000_000, 1_000_000_000_000_000_000, rcv, [0; 32]).is_ok());
    // earlier in the SAME second is a backwards step of the market's own clock
    assert_eq!(w.ev.on_sample(0, ns(t) + 100_000_000, 1_000_000_000_000_000_000, rcv, [0; 32]), Err(SampleError::MarketTimeBackwards));
    // an earlier second is out of order for the stream
    assert_eq!(w.ev.on_sample(0, ns(t - 1), 1_000_000_000_000_000_000, rcv, [0; 32]), Err(SampleError::OutOfOrder));
    assert_eq!(w.ev.sample_count(), 1);
    // a value too large for a price
    assert_eq!(w.ev.on_sample(0, ns(t + 1), i128::MAX, rcv, [0; 32]), Err(SampleError::BadValue));
    // the half-spread of a huge price overflows
    let mut r = base_rules(vec![eur_market(0)]);
    r.markets[0].half_spread_e6 = 500_000;
    let mut w2 = World::new(r, S100K, 2, T_1400 - HOUR);
    assert_eq!(w2.ev.on_sample(0, ns(t), i128::from(u64::MAX) * 1_000_000, rcv, [0; 32]), Err(SampleError::BadValue));
    // samples before the account existed are out of order
    let mut w3 = World::base();
    assert_eq!(w3.ev.on_sample_mid(0, T_1400 - 2 * HOUR, E12), Err(SampleError::OutOfOrder));
}

#[test]
fn archive_bytes_equal_the_verifiers_writer() {
    let mut w = World::base();
    let mut vw = ArchiveWriter::new();
    let day2 = 86_400 * (T_1400 / 86_400 + 1);
    let mut times = [T_1400, T_1400 + 1, T_1400 + 600, day2 + 5, day2 + 6];
    times.sort();
    for (i, t) in times.iter().enumerate() {
        let mid = E12 + i as u64 * 1_000_000;
        let ts = (*t as u64) * 1_000_000_000;
        let recv = ts + 123_456;
        w.ev.on_sample(0, ts, i128::from(mid) * 1_000_000, recv, [0; 32]).unwrap();
        // a new UTC day starts a new segment in the verifier's writer too
        if i == 3 {
            vw.start_segment();
        }
        vw.push(&Sample { market: 0, source: 2, recv_ns: recv, ts_ns: ts, value: i128::from(mid) * 1_000_000, digest: [0; 32] });
    }
    let want = vw.into_bytes();
    assert_eq!(w.ev.archive_bytes(), &want[..]);
    // and the verifier parses it back to the same stream
    let a = Archive::parse(w.ev.archive_bytes()).unwrap();
    assert_eq!(a.stream, w.ev.sample_stream());
    let recs: Vec<SampleRec> = (0..5).map(|i| w.ev.sample(i).unwrap().rec).collect();
    assert_eq!(recs[0].market, 0);
    assert_eq!(recs[4].recv_ns - recs[4].ts_ns, 123_456);
}

#[test]
fn intents_file_equals_the_verifiers_lines() {
    let mut w = World::base();
    w.sample(0, T_1400, p4(10_800));
    let a = market_order(0, true, 1_000_000, T_1400);
    let b = order(IntentKind::Limit, 0, 1, 2_000_000, p4(11_000), 0, 0, 0, T_1400);
    let na = w.order(a).unwrap();
    let nb = w.order(b).unwrap();
    let mut want = setl8_verify::intents::header_line();
    for (n, i) in [(na, a), (nb, b)] {
        let s = sign(&w.opened.account, n, &i);
        want.push_str(&setl8_verify::intents::line_for(n, &s.order, &s.sig));
    }
    assert_eq!(w.ev.intents_bytes(), want.as_bytes());
    let _ = OrderIntent::from_bytes(&a.to_bytes());
}

#[test]
fn clock_and_processing() {
    let mut w = World::base();
    let t = T_1400;
    w.sample(0, t, p4(10_800));
    // the second of the newest input is still open: nothing is processed yet
    assert_eq!((w.ev.clock(), w.ev.processed_through()), (t, t - 1), "the first sample closes every second before it");
    w.order(market_order(0, true, 1_000_000, t)).unwrap();
    assert_eq!(w.ev.positions().len(), 0, "orders execute when their second closes");
    w.sample(0, t + 3, p4(10_800));
    assert_eq!((w.ev.clock(), w.ev.processed_through()), (t + 3, t + 2));
    assert_eq!(w.ev.positions().len(), 1);
    // tick never goes back
    w.ev.tick(t + 1);
    assert_eq!(w.ev.clock(), t + 3);
    w.ev.tick(t + 10);
    assert_eq!((w.ev.clock(), w.ev.processed_through()), (t + 10, t + 9));
}

#[test]
fn several_samples_in_one_second_act_on_the_last_numbers() {
    // two markets, both sampled in the same second: the decision uses the account at the start of the second and the marks
    // as they stand sample by sample; the verifier's walk does the same
    let mut m0 = eur_market(0);
    m0.half_spread_e6 = TEST_HALF_SPREAD_E6;
    m0.max_age_secs = 3_600;
    let mut m1 = common::market(1, 0, "SECOND", TEST_HALF_SPREAD_E6);
    m1.max_age_secs = 3_600;
    let mut w = World::new(base_rules(vec![m0, m1]), S100K, 2, T_1400 - HOUR);
    w.sample(0, T_1400, p4(10_000));
    w.sample(1, T_1400, p4(10_000));
    w.order(market_order(0, true, MEUR, T_1400)).unwrap();
    w.sample(0, T_1400 + 10, p4(10_000));
    w.sample(1, T_1400 + 10, p4(10_000));
    w.sample(0, T_1400 + 20, p4(10_020));
    w.sample(1, T_1400 + 20, p4(10_021));
    w.ev.tick(T_1400 + 21);
    let batches = w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true));
    assert_valid(&w, &batches);
}

#[test]
fn an_ended_evaluator_still_archives() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w.order(market_order(0, true, 45 * MEUR / 10, T_1400)).unwrap();
    w.sample(0, T_1400 + 5, p4(10_000));
    w.sample(0, T_1400 + 10, p4(9_000));
    w.ev.tick(T_1400 + 11);
    assert!(matches!(w.ev.status(), Status::Failed(_)));
    let n = w.ev.sample_count();
    w.sample(0, T_1400 + 20, p4(9_100));
    assert_eq!(w.ev.sample_count(), n + 1);
    assert!(w.ev.queued_events() > 0);
    w.ev.tick(T_1400 + 30);
    assert!(w.ev.fault().is_none());
    let batches = w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true));
    assert_valid(&w, &batches);
    // no heartbeat after a fail: the last batch is the report_fail batch
    assert!(matches!(batches.last().unwrap().kind, BatchKind::ReportFail(_)));
    assert!(w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true)).is_empty());
}

#[test]
fn external_end_stops_the_evaluator() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, T_1400, p4(10_000));
    w.ev.end_externally();
    assert!(w.ev.is_ended() && w.ev.is_frozen());
    assert_eq!(w.order(market_order(0, true, MEUR, T_1400)), Err(OrderReject::AccountEnded));
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 10);
    assert_eq!(w.ev.queued_events(), 0);
}
