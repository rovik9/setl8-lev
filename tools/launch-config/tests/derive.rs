//! Hand-computed arithmetic of rules 2, 3 and 5.
mod common;
use common::*;
use launch_config::derive::*;
use launch_config::model::Rules;
use journal_types as jt;

fn rules() -> Rules {
    base().rules
}

#[test]
fn max_age_by_decision_57_with_hand_numbers() {
    let r = rules();
    // 29,936 ms x 1.25 = 37,420 ms -> 38 s -> 40 s
    assert_eq!(max_age_secs(29_936, &r).unwrap(), 40);
    // 25,006 ms x 1.25 = 31,257.5 ms -> 31,258 ms -> 32 s -> 35 s
    assert_eq!(max_age_secs(25_006, &r).unwrap(), 35);
    // 34,492 -> 43,115 ms -> 44 s -> 45 s
    assert_eq!(max_age_secs(34_492, &r).unwrap(), 45);
    // an exact multiple stays: 20,000 ms x 1.25 = 25,000 ms = 25 s
    assert_eq!(max_age_secs(20_000, &r).unwrap(), 25);
    // one millisecond more rounds up a whole step: 20,001 x 1.25 = 25,001.25 -> 25,002 ms -> 26 s -> 30 s
    assert_eq!(max_age_secs(20_001, &r).unwrap(), 30);
    // 19,999 ms x 1.25 = 24,998.75 ms -> 24,999 -> 25 s
    assert_eq!(max_age_secs(19_999, &r).unwrap(), 25);
    // the floor: 3,664 ms -> 4.58 s -> 5 s -> 15 s; 12,000 ms x 1.25 = 15 s exactly -> 15 s; 12,001 -> 15.00125 -> 16 -> 20
    assert_eq!(max_age_secs(3_664, &r).unwrap(), 15);
    assert_eq!(max_age_secs(0, &r).unwrap(), 15);
    assert_eq!(max_age_secs(12_000, &r).unwrap(), 15);
    assert_eq!(max_age_secs(12_001, &r).unwrap(), 20);
    // 164,357 ms (the EUR freeze) -> 205,446.25 -> 205.447 s -> 206 -> 210
    assert_eq!(max_age_secs(164_357, &r).unwrap(), 210);
}

#[test]
fn max_age_uses_the_rule_constants_and_must_fit_u16() {
    let mut r = rules();
    r.freeze_margin_pct = 0;
    assert_eq!(max_age_secs(20_000, &r).unwrap(), 20);
    r.freeze_margin_pct = 100;
    assert_eq!(max_age_secs(20_000, &r).unwrap(), 40);
    r.max_age_round_secs = 1;
    assert_eq!(max_age_secs(20_001, &r).unwrap(), 41);
    r.max_age_floor_secs = 90;
    assert_eq!(max_age_secs(1_000, &r).unwrap(), 90);
    r.max_age_round_secs = 0;
    assert!(max_age_secs(1_000, &r).is_err());
    let r = rules();
    // 65,535 s is the last value: 52,428,000 ms x 1.25 = 65,535,000 ms -> 65,535 s, rounded up to 65,535 / 5 = 13,107 exactly
    assert_eq!(max_age_secs(52_428_000, &r).unwrap(), 65_535);
    assert!(max_age_secs(52_428_001, &r).is_err());
    assert!(max_age_secs(u64::MAX, &r).is_err());
}

#[test]
fn rounding_to_one_two_five() {
    for (n, up) in [(1, 1), (2, 2), (3, 5), (4, 5), (5, 5), (6, 10), (10, 10), (11, 20), (21, 50), (51, 100), (123, 200), (1_000_001, 2_000_000)] {
        assert_eq!(round_up_125(n), up, "up {n}");
    }
    for (n, down) in [(0, 0), (1, 1), (2, 2), (4, 2), (5, 5), (9, 5), (10, 10), (19, 10), (20, 20), (49, 20), (50, 50), (99_999, 50_000), (200_000, 200_000), (u64::MAX, 10_000_000_000_000_000_000)] {
        assert_eq!(round_down_125(n), down, "down {n}");
    }
}

#[test]
fn quantity_placeholder_with_hand_numbers() {
    // 10 USD at 81,131.526567 USD = 0.00012325 BTC = 123.25 units of 1e-6 -> 124 -> 200
    assert_eq!(qty_min(price_micro(81_131.526567).unwrap(), 10).unwrap(), 200);
    // at exactly 10 USD one whole unit is worth 10 USD: 1,000,000 units of 1e-6
    assert_eq!(qty_min(10_000_000, 10).unwrap(), 1_000_000);
    // at 100 USD: 100,000 units, already a 1-2-5 number
    assert_eq!(qty_min(100_000_000, 10).unwrap(), 100_000);
    // at 99 USD: 101,010.1 -> 101,011 -> 200,000
    assert_eq!(qty_min(99_000_000, 10).unwrap(), 200_000);
    // a dearer order: 100 USD at 100 USD
    assert_eq!(qty_min(100_000_000, 100).unwrap(), 1_000_000);
    // a price above the order value still needs one unit
    assert_eq!(qty_min(1_000_000_000_000_000, 10).unwrap(), 1);
    assert_eq!(price_micro(0.0000015).unwrap(), 2);
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 2.0e12] {
        assert!(price_micro(bad).is_err(), "{bad}");
    }
}

#[test]
fn cap_placeholder_with_hand_numbers() {
    // the committed basis is the biggest tier, 200,000 USD (decision 64); ceilings 5,000,000 and, for stocks, 500,000
    let r = rules();
    assert_eq!(r.cap_basis_tier_usd, 200_000);
    // 200,000 x 20 = 4,000,000: rounded down to 2,000,000 (1-2-5 steps), under the 5,000,000 ceiling
    assert_eq!(cap_micro(2, 20, &r), 2_000_000 * 1_000_000);
    // energy 1:10: exactly 2,000,000
    assert_eq!(cap_micro(4, 10, &r), 2_000_000 * 1_000_000);
    // crypto 1:5: exactly 1,000,000
    assert_eq!(cap_micro(6, 5, &r), 1_000_000 * 1_000_000);
    // 200,000 x 50 = 10,000,000: clipped to 5,000,000
    assert_eq!(cap_micro(0, 50, &r), 5_000_000 * 1_000_000);
    // stocks: 200,000 x 5 = 1,000,000 -> clipped to 500,000; the stock ceiling applies to class 5 only
    assert_eq!(cap_micro(5, 5, &r), 500_000 * 1_000_000);
    // the P19 basis (the 10K tier) for comparison: far below what the biggest tier may hold at its leverage
    let mut small = r.clone();
    small.cap_basis_tier_usd = 10_000;
    assert_eq!(cap_micro(2, 20, &small), 200_000 * 1_000_000);
    assert_eq!(cap_micro(4, 10, &small), 100_000 * 1_000_000);
    assert_eq!(cap_micro(6, 5, &small), 50_000 * 1_000_000);
    assert_eq!(cap_micro(0, 50, &small), 500_000 * 1_000_000);
    assert_eq!(cap_micro(5, 5, &small), 50_000 * 1_000_000);
}

#[test]
fn feed_ids_are_pinned_hex_constants_and_equal_the_journal_types_function() {
    const PINNED: [(&str, u8, &str); 21] = [
        ("BTC", 2, "c9970690cb8f84ccf28015018833aa73ca3566177b52a85ab6f18593e45f65c8"),
        ("ETH", 2, "7525c59eea5a3be0ca004d409a000d332e9be3d22688ecae2737c677a6132855"),
        ("SOL", 2, "475c49d13a176523c4dd9396d33dc6a2b6ccbaad09e5b16b5be661227aca2ee1"),
        ("GOLD", 2, "141585d9aa6ef64ea2aaa049dadc368eb8c21a701cc654d0780df48b62625182"),
        ("SILVER", 2, "0fe0de3059a1afd4a1b1b5ea94310cb03ed8ea1681a39c0df3202585f3ca5ec7"),
        ("WTIOIL", 2, "09219f69620290210896df4bffe632f9fe469bb27c320641a0f7fbb8687b1dcc"),
        ("BRENTOIL", 2, "000d10e1b0f08229e25da0b7a63a8e8c01b7ae9cef614eee56eef643b9fb3ddf"),
        ("SPY", 2, "529503a2a0cf28b452a79fc27ffa322806bc3f34bb52b9fcbb3ebc3fcee36a15"),
        ("QQQ", 2, "b7ecc85dcd30d220abcbf8d3c271a88d9164fd0b4a41122a823b1338695c06e7"),
        ("xyz:JP225", 4, "3822a26acc79bc6c2085beb27ea8326ba5b8d0756b5f86d09c7a53b89eca37a2"),
        ("xyz:KR200", 4, "cbe76b73f8a061245e43bb49195a5af4e02b37195f977671cafd0d94902933e1"),
        ("xyz:AAPL", 4, "f49d76a7a86a36888b2352386efdb8a01272e806706fd5defbfe8a95c61daa9a"),
        ("xyz:AMD", 4, "414386b00a5ab02de79f12c9331762c49f377ce42cd00caa8e227749d8f8f570"),
        ("xyz:AMZN", 4, "4dd400d13918a772bb4c29717f6c3690e7f558f2353cdc9808512daa863e8213"),
        ("xyz:GOOGL", 4, "02692704bfff67e3ea3ef99b14a7db02f1e77f3c842df6e01879289bba58948b"),
        ("xyz:META", 4, "3e1674a96a6b6f5cb8833034d01ca1f63cb9665cd85470a6b99f31ddd89aeb85"),
        ("xyz:MSFT", 4, "3f40725d7304ee6c83d91ea5c8f0f50eac95669526564590863a1a90e5802e6e"),
        ("xyz:NFLX", 4, "59d44982b417850bc22eada303c07f0e1bec44cdd5701de86dee00d55754073a"),
        ("xyz:NVDA", 4, "baea0c8c47bb3371cc71c05c8e00461d1b3ceadf216933c38ee656f33cf82a47"),
        ("xyz:PLTR", 4, "6ff0d52d6cab6ec3d16c1f1bd7fc2975326acb97b1fd0d24d19d1cc0ccac2174"),
        ("xyz:TSLA", 4, "d214a904779716586b142a7c671c77dd0f4a154c1ed3051161d63e1ff9cecd88"),
    ];
    let l = launch(&base());
    assert_eq!(l.markets.len(), PINNED.len());
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    for (m, (sym, src, want)) in l.markets.iter().zip(PINNED) {
        assert_eq!((m.symbol.as_str(), m.source), (sym, src));
        assert_eq!(hex(&m.feed_id), want, "{sym}");
        assert_eq!(feed_id(src, sym), jt::feed_id(&jt::NativeSha256, src, sym), "{sym} vs journal-types");
        assert_eq!(m.feed_id, jt::feed_id(&jt::NativeSha256, src, sym));
    }
    // the source byte and the exact symbol are part of the hash
    assert_ne!(feed_id(2, "BTC"), feed_id(4, "BTC"));
    assert_ne!(feed_id(4, "xyz:AAPL"), feed_id(4, "AAPL"));
}

#[test]
fn every_derived_number_of_every_market_against_an_independent_computation() {
    // (symbol, max_age_secs, qty_step = qty_min, max_notional_usd in micro-USD), computed outside this crate
    const WANT: [(&str, u16, u64, u64); 21] = [
        ("BTC", 15, 200, 1000000000000),
        ("ETH", 15, 5000, 1000000000000),
        ("SOL", 15, 100000, 1000000000000),
        ("GOLD", 35, 5000, 2000000000000),
        ("SILVER", 45, 200000, 2000000000000),
        ("WTIOIL", 30, 200000, 2000000000000),
        ("BRENTOIL", 40, 100000, 2000000000000),
        ("SPY", 30, 20000, 2000000000000),
        ("QQQ", 20, 20000, 2000000000000),
        ("xyz:JP225", 40, 200, 2000000000000),
        ("xyz:KR200", 35, 20000, 2000000000000),
        ("xyz:AAPL", 20, 50000, 500000000000),
        ("xyz:AMD", 20, 20000, 500000000000),
        ("xyz:AMZN", 25, 50000, 500000000000),
        ("xyz:GOOGL", 20, 50000, 500000000000),
        ("xyz:META", 20, 20000, 500000000000),
        ("xyz:MSFT", 20, 20000, 500000000000),
        ("xyz:NFLX", 20, 200000, 500000000000),
        ("xyz:NVDA", 20, 50000, 500000000000),
        ("xyz:PLTR", 20, 100000, 500000000000),
        ("xyz:TSLA", 20, 50000, 500000000000),
    ];
    let l = launch(&base());
    for (m, (sym, age, q, cap)) in l.markets.iter().zip(WANT) {
        assert_eq!((m.symbol.as_str(), m.max_age_secs, m.qty_step, m.qty_min, m.max_notional_usd), (sym, age, q, q, cap));
        assert_eq!((m.status, m.quote_conv_market), (1, 0xFFFF), "{sym}");
    }
}

#[test]
fn the_price_range_is_inclusive_at_the_top() {
    assert_eq!(price_micro(1.0e12).unwrap(), 1_000_000_000_000_000_000);
    assert!(price_micro(1.0e12 + 1.0e6).is_err());
}

#[test]
fn a_ladder_of_six_and_exactly_128_markets() {
    use launch_config::generate;
    let mut i = base();
    i.ruleset.reset_ladder_bps = vec![10_000, 11_500, 12_000, 12_750, 13_000, 13_500];
    assert!(generate(&i).unwrap_err().contains("1 to 5 entries"));
    i.ruleset.reset_ladder_bps = vec![10_000];
    assert_eq!(generate(&i).unwrap().ruleset.ladder_len, 1);
    i.ruleset.reset_ladder_bps = vec![10_000, 11_500, 12_000, 12_750, 13_000];
    assert_eq!(generate(&i).unwrap().ruleset.ladder_len, 5);
    // 128 markets are the most the rule set indexes
    let m = i.market[0].clone();
    i.market = vec![m; 128];
    assert_eq!(generate(&i).unwrap().markets.len(), 128);
    i.market.push(i.market[0].clone());
    assert!(generate(&i).unwrap_err().contains("129 markets"));
    // ten tiers are the most
    let mut j = base();
    j.ruleset.tier_sizes_usd = (1..=10).collect();
    j.ruleset.tier_costs_usd = (1..=10).collect();
    assert_eq!(generate(&j).unwrap().ruleset.tier_count, 10);
}
