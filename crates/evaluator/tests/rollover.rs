//! Holding fees at the 22:00 UTC rollover: rate zero, the sample AT the rollover second, several positions, the mark.
mod common;
use common::*;
use evaluator::*;
use journal_types::{FeeKind, JournalEvent};

const R: i64 = T_1400 + 8 * HOUR; // 22:00:00 on Monday 2026-10-12

fn world_with_rate(rate: u32, n_markets: u16) -> World {
    let markets: Vec<_> = (0..n_markets).map(|i| { let mut m = common::market(i, 0, &format!("R{i}"), TEST_HALF_SPREAD_E6); m.max_age_secs = 3_600; m }).collect();
    let mut rules = base_rules(markets);
    rules.holding_fee_e6[0] = rate;
    World::new(rules, S100K, 2, T_1400 - HOUR)
}

fn holding(evs: &[JournalEvent]) -> Vec<(u16, u64, i64)> {
    evs.iter().filter_map(|e| if let JournalEvent::Fee { kind: FeeKind::Holding, market, amount, time } = e { Some((*market, *amount, *time)) } else { None }).collect()
}

#[test]
fn a_zero_rate_writes_no_fee_but_the_night_is_still_counted() {
    let mut w = world_with_rate(0, 1);
    let t = T_1400 + 7 * HOUR;
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, R + 10, p4(10_000));
    w.sample(0, R + 20, p4(10_001));
    w.ev.tick(R + 30);
    assert_eq!(w.ev.stats().holding_fees, 0);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX.with_heartbeat(true));
    assert!(holding(&events_of(&bs)).is_empty());
    // the position is marked as charged for the night: raising the rate afterwards cannot charge it retroactively
    assert_eq!(w.ev.positions()[&0].p.fee_day, engine::day_index(R));
    assert_valid(&w, &bs);
}

#[test]
fn the_sample_at_the_rollover_second_is_judged_after_the_fee() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    let t = T_1400 + 7 * HOUR; // 21:00:00
    w.sample(0, t, p4(10_000));
    w.order(market_order(0, true, MEUR, t)).unwrap();
    w.sample(0, t + 1800, p4(10_020)); // 21:30: the bid 1.001998998 less the ask 1.000001 on 1e6 units: P = +1,997.998, above 0.01 S: tight mode starts
    w.sample(0, R, p4(10_040)); // exactly 22:00:00: the bid 1.003998996: P = +3,997.996
    w.ev.tick(R + 1);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let evs = events_of(&bs);
    // the fee: 1,000,000 units x the bid 1.003998996 x 0.0001 = $100.3998996, ceiled to 100,399,900 micro-USD, marked at the LAST sample at or
    // before 22:00:00 (the one at 22:00:00)
    assert_eq!(holding(&evs), vec![(0, 100_399_900, R)]);
    // the floor raise at 22:00:00 sees the balance AFTER the fee: G = 3,997.996 - 100.3999 = 3,897.5961 (3_997_996_000 - 100_399_900),
    // L = (F_s + 0.04 S = 96,000) + (G - 0.01 S = 2,897.5961) = 98,897.5961
    let fr = evs.iter().find_map(|e| if let JournalEvent::FloorRaised { time, g_peak, l_level, .. } = e { Some((*time, *g_peak, *l_level)) } else { None }).unwrap();
    assert_eq!(fr, (R, 3_897_596_100, 98_897_596_100));
    // and the fee is queued before the raise
    let fee_pos = evs.iter().position(|e| matches!(e, JournalEvent::Fee { .. })).unwrap();
    let raise_pos = evs.iter().position(|e| matches!(e, JournalEvent::FloorRaised { time, .. } if *time == R)).unwrap();
    assert!(fee_pos < raise_pos);
    assert_valid(&w, &bs);
}

#[test]
fn one_fee_per_position_and_night_at_the_last_mark_before_the_rollover() {
    let mut w = world_with_rate(100, 2);
    let t = T_1400 + 7 * HOUR;
    w.sample(0, t, p4(10_000));
    w.sample(1, t + 1, p4(20_000));
    w.order(market_order(0, true, MEUR / 100, t + 1)).unwrap();
    w.order(market_order(1, false, MEUR / 100, t + 1)).unwrap();
    w.sample(0, R - 5, p4(10_010));
    w.sample(1, R - 4, p4(20_020));
    w.sample(0, R + 5, p4(10_500)); // after the rollover: must not be the mark
    w.sample(1, R + 6, p4(21_000));
    // two nights: the second rollover is a day later
    w.sample(0, R + 86_400 - 1, p4(10_030));
    w.sample(1, R + 86_400 - 1 + 1, p4(20_040));
    w.sample(0, R + 86_400 + 5, p4(10_031));
    w.ev.tick(R + 86_400 + 30);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    let fees = holding(&events_of(&bs));
    // night 1: long 10,000 at the bid 1.000998999 (mid 1.0010): 10,009.98999 x 0.0001 = 1,000,998.999 micro, ceiled to 1,000,999;
    // short 10,000 at the ask 2.002002002 (mid 2.0020): 20,020.02002 x 0.0001 = 2,002,002.002 micro, ceiled to 2,002,003 (the notional uses the mark)
    // night 2: marks mid 1.0030 and 2.0040: bid 1.002998997 -> 1,002,998.997 -> 1,002,999; ask 2.004002004 -> 2,004,002.004 -> 2,004,003
    assert_eq!(fees, vec![(0, 1_000_999, R), (1, 2_002_003, R), (0, 1_002_999, R + 86_400), (1, 2_004_003, R + 86_400)]);
    assert_eq!(w.ev.stats().holding_fees, 4);
    assert_valid(&w, &bs);
}

#[test]
fn a_position_opened_at_the_rollover_second_is_not_charged_for_it() {
    let mut w = fx_world(TEST_HALF_SPREAD_E6);
    w.sample(0, R - 10, p4(10_000));
    w.sample(0, R, p4(10_000));
    w.order(market_order(0, true, MEUR, R)).unwrap();
    w.sample(0, R + 10, p4(10_001));
    w.ev.tick(R + 20);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    assert!(holding(&events_of(&bs)).is_empty());
    assert_eq!(w.ev.positions()[&0].p.fee_day, engine::day_index(R));
    assert_valid(&w, &bs);
    // and charged for the NEXT one
    w.sample(0, R + 86_400 + 10, p4(10_002));
    w.ev.tick(R + 86_400 + 20);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    assert_eq!(holding(&events_of(&bs)).len(), 1);
}

#[test]
fn more_than_seven_positions_chunk_the_nights_fees() {
    // eight positions: the eight fees of one night travel as 7 + 1
    let mut w = world_with_rate(100, 8);
    let t = T_1400 + 7 * HOUR;
    for i in 0..8u16 {
        w.sample(i, t + i64::from(i), p4(10_000));
    }
    for i in 0..8u16 {
        w.order(market_order(i, true, MEUR / 100, t + 8)).unwrap();
    }
    w.sample(0, R + 5, p4(10_000));
    w.ev.tick(R + 10);
    let bs = w.ev.drain_batches(&BatchPolicy::MAX);
    assert_eq!(holding(&events_of(&bs)).len(), 8);
    let sizes: Vec<usize> = bs.iter().filter(|b| b.batch.events.iter().any(|e| matches!(e, JournalEvent::Fee { .. }))).map(|b| b.batch.events.len()).collect();
    assert_eq!(sizes, vec![8, 2]);
}
