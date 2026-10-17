//! Every rule `verify` enforces has a passing fixture (the committed configuration) and a failing one.
mod common;
use common::*;
use launch_config::check;
use launch_config::derive::MAX_MARKETS;
use launch_config::{emit, generate, parse};

fn all(i: &launch_config::Input, production: bool) -> Vec<String> {
    check::all(i, &launch(i), production)
}

#[test]
fn the_committed_configuration_passes_every_rule() {
    let i = base();
    assert_eq!(all(&i, false), Vec::<String>::new());
    let l = launch(&i);
    assert_eq!(check::files(&i, &l, json(), md()), Vec::<String>::new());
    assert_eq!(l.markets.len(), 21);
}

// ---- rule 1: markets and sources (decisions 55 and 56)

#[test]
fn the_market_list_is_exactly_decision_55_and_the_passing_candidates() {
    let l = launch(&base());
    let got: Vec<(&str, u8, u8)> = l.markets.iter().map(|m| (m.symbol.as_str(), m.source, m.class)).collect();
    assert_eq!(
        got,
        vec![
            ("BTC", 2, 6), ("ETH", 2, 6), ("SOL", 2, 6), ("GOLD", 2, 2), ("SILVER", 2, 2), ("WTIOIL", 2, 4), ("BRENTOIL", 2, 4), ("SPY", 2, 3), ("QQQ", 2, 3),
            ("xyz:JP225", 4, 3), ("xyz:KR200", 4, 3), ("xyz:AAPL", 4, 5), ("xyz:AMD", 4, 5), ("xyz:AMZN", 4, 5), ("xyz:GOOGL", 4, 5), ("xyz:META", 4, 5),
            ("xyz:MSFT", 4, 5), ("xyz:NFLX", 4, 5), ("xyz:NVDA", 4, 5), ("xyz:PLTR", 4, 5), ("xyz:TSLA", 4, 5),
        ]
    );
    for (k, m) in l.markets.iter().enumerate() {
        assert_eq!(usize::from(m.id), k, "ids are sequential from 0");
    }
}

#[test]
fn a_forbidden_symbol_is_refused() {
    for sym in ["xyz:EUR", "xyz:GBP", "xyz:JPY", "xyz:KRW", "NATGAS", "xyz:NATGAS", "xyz:HO", "xyz:OURA", "xyz:CAMBRICON", "xyz:TWST", "xyz:UMC", "xyz:SP500", "xyz:XYZ100", "xyz:GLW"] {
        let mut i = base();
        i.market[0].symbol = sym.into();
        let v = check::sources(&i);
        assert!(has(&v, &format!("{sym}: not at launch (decision 56)")), "{sym}: {v:?}");
    }
}

#[test]
fn a_symbol_outside_the_allow_list_or_on_the_wrong_source_is_refused() {
    let mut i = base();
    i.market[0].symbol = "DOGE".into(); // a crypto alt
    assert!(has(&check::sources(&i), "DOGE: not an allowed symbol for source 2"));
    let mut i = base();
    i.market[0].source = 4; // BTC on the Hyperliquid source
    assert!(has(&check::sources(&i), "BTC: not an allowed symbol for source 4"));
    let mut i = base();
    let k = idx(&i, "xyz:AAPL");
    i.market[k].source = 2;
    assert!(has(&check::sources(&i), "xyz:AAPL: not an allowed symbol for source 2"));
    let mut i = base();
    i.market[0].source = 3; // GMX signed reports are not a launch source
    assert!(has(&check::sources(&i), "BTC: not an allowed symbol for source 3"));
}

#[test]
fn a_wrong_class_is_refused() {
    let mut i = base();
    i.market[0].class = 7; // BTC as a crypto alt
    assert!(has(&check::sources(&i), "BTC: class 7 but decision 55 says class 6"));
    let mut i = base();
    let k = idx(&i, "SPY");
    i.market[k].class = 5;
    assert!(has(&check::sources(&i), "SPY: class 5 but decision 55 says class 3"));
}

#[test]
fn never_two_markets_for_the_same_underlying_or_symbol_or_feed() {
    // the S&P 500 twice: SPY and a second market on the same underlying
    let mut i = base();
    let (spy, qqq) = (idx(&i, "SPY"), idx(&i, "QQQ"));
    i.market[qqq].underlying = i.market[spy].underlying.clone();
    assert!(has(&check::sources(&i), "QQQ: a second market on the underlying \"sp500\""));
    // the same symbol twice
    let mut i = base();
    let dup = i.market[1].clone();
    i.market[0] = dup;
    let v = check::sources(&i);
    assert!(has(&v, "ETH: two markets with this symbol"), "{v:?}");
    assert!(has(&v, "ETH: duplicate feed id"), "{v:?}");
}

#[test]
fn a_candidate_cannot_be_a_market_and_excluded_at_once() {
    let mut i = base();
    i.excluded[0].symbol = "BTC".into();
    assert!(has(&check::sources(&i), "BTC: listed as a market and as excluded"));
}

// ---- rule 1 go-live (decision 56)

#[test]
fn the_go_live_rule_boundaries() {
    let k = |i: &launch_config::Input| idx(i, "xyz:AAPL");
    for (med, p95, ok) in [(6.0, 12.0, true), (6.01, 12.0, false), (6.0, 12.01, false), (0.0, 0.0, true), (6.5, 20.0, false)] {
        let mut i = base();
        let a = k(&i);
        (i.market[a].oracle_median_bp, i.market[a].oracle_p95_bp) = (Some(med), Some(p95));
        let v = check::golive(&i);
        assert_eq!(v.is_empty(), ok, "{med}/{p95}: {v:?}");
    }
    let mut i = base();
    let a = k(&i);
    i.market[a].oracle_median_bp = Some(7.25);
    assert!(has(&check::golive(&i), "xyz:AAPL: median |oracle - mid| 7.25 bp is above 6 bp (decision 56)"));
    let mut i = base();
    i.market[a].oracle_p95_bp = Some(12.5);
    assert!(has(&check::golive(&i), "xyz:AAPL: p95 |oracle - mid| 12.50 bp is above 12 bp (decision 56)"));
}

#[test]
fn a_hyperliquid_market_without_statistics_is_refused() {
    let mut i = base();
    let a = idx(&i, "xyz:AAPL");
    i.market[a].oracle_median_bp = None;
    assert!(has(&check::golive(&i), "xyz:AAPL: a Hyperliquid market needs oracle_median_bp and oracle_p95_bp"));
    let mut i = base();
    i.market[a].oracle_p95_bp = None;
    assert!(has(&check::golive(&i), "needs oracle_median_bp and oracle_p95_bp"));
    // GMX markets need none
    let i = base();
    assert!(i.market[0].oracle_median_bp.is_none());
}

#[test]
fn an_exclusion_must_really_fail_its_rule() {
    let mut i = base();
    let coin = i.excluded.iter().position(|e| e.symbol == "xyz:COIN").unwrap();
    i.excluded[coin].p95_bp = Some(11.0);
    assert!(has(&check::golive(&i), "xyz:COIN: excluded for rule \"p95\" but its number passes that rule"));
    let mut i = base();
    let mstr = i.excluded.iter().position(|e| e.symbol == "xyz:MSTR").unwrap();
    i.excluded[mstr].median_bp = Some(6.0);
    assert!(has(&check::golive(&i), "xyz:MSTR: excluded for rule \"median\" but its number passes that rule"));
    let mut i = base();
    i.excluded[0].median_bp = None;
    i.excluded[0].p95_bp = None;
    let v = check::golive(&i);
    assert!(v.iter().any(|s| s.contains("excluded for rule")), "{v:?}");
    let mut i = base();
    i.excluded[0].rule = "because".into();
    assert!(has(&check::golive(&i), "xyz:COIN: unknown exclusion rule \"because\""));
    // policy reasons need no number
    assert!(check::golive(&base()).is_empty());
}

#[test]
fn the_excluded_candidates_are_the_ones_the_prompt_names_with_their_failing_numbers() {
    let i = base();
    let get = |s: &str| i.excluded.iter().find(|e| e.symbol == s).unwrap_or_else(|| panic!("{s} not excluded"));
    assert_eq!((get("xyz:COIN").rule.as_str(), get("xyz:COIN").p95_bp), ("p95", Some(14.74)));
    assert_eq!((get("xyz:MSTR").rule.as_str(), get("xyz:MSTR").median_bp), ("median", Some(6.59)));
    for s in ["xyz:TWST", "xyz:UMC", "xyz:OURA", "xyz:CAMBRICON", "xyz:HO"] {
        assert_eq!(get(s).rule, "median", "{s}");
    }
    assert_eq!(get("xyz:GLW").rule, "liquidity");
    for s in ["xyz:EUR", "xyz:GBP", "xyz:JPY", "NATGAS"] {
        assert_eq!(get(s).rule, "decision-56", "{s}");
    }
    for s in ["xyz:SP500", "xyz:XYZ100", "crypto alts"] {
        assert_eq!(get(s).rule, "not-at-launch", "{s}");
    }
    // every FORBIDDEN name is either excluded here or documented as a duplicate
    for f in check::FORBIDDEN {
        assert!(i.excluded.iter().any(|e| e.symbol == f) || f == "xyz:KRW" || f == "xyz:NATGAS", "{f} is forbidden but not listed as excluded");
    }
}

// ---- rule 4: sessions

#[test]
fn crypto_is_24_7_and_everything_else_has_windows_with_a_verify_flag() {
    let i = base();
    let l = launch(&i);
    for (m, o) in i.market.iter().zip(&l.markets) {
        let always = o.sessions == [(0, 10080)];
        assert_eq!(always, m.class == 6, "{}", m.symbol);
        assert_eq!(m.verify.is_empty(), always, "{}: VERIFY flags exactly where the data does not settle the window", m.symbol);
        assert!(o.sessions.len() <= 6);
    }
}

#[test]
fn the_session_windows_of_each_group_in_minutes() {
    let l = launch(&base());
    let by = |s: &str| l.markets.iter().find(|m| m.symbol == s).unwrap().sessions.clone();
    let cme = vec![(0, 1260), (1380, 2700), (2820, 4140), (4260, 5580), (5700, 7020), (10020, 10080)];
    for s in ["GOLD", "SILVER", "WTIOIL"] {
        assert_eq!(by(s), cme, "{s}");
    }
    assert_eq!(by("BRENTOIL"), vec![(60, 1320), (1500, 2760), (2940, 4200), (4380, 5640), (5820, 7080)]);
    let us = vec![(870, 1200), (2310, 2640), (3750, 4080), (5190, 5520), (6630, 6960)];
    for s in ["SPY", "QQQ", "xyz:AAPL", "xyz:TSLA", "xyz:NVDA"] {
        assert_eq!(by(s), us, "{s}");
    }
    assert_eq!(by("xyz:JP225"), vec![(0, 360), (1440, 1800), (2880, 3240), (4320, 4680), (5760, 6120)]);
    assert_eq!(by("xyz:KR200"), vec![(0, 390), (1440, 1830), (2880, 3270), (4320, 4710), (5760, 6150)]);
    assert_eq!(by("BTC"), vec![(0, 10080)]);
}

#[test]
fn session_rule_violations() {
    let mut i = base();
    i.market[0].session_variants = vec![vec!["Mon-Fri 00:00-24:00".into()]]; // BTC with a closed weekend
    assert!(has(&check::sessions(&i, &launch(&i)), "BTC: crypto trades 24/7 and has no closed window"));
    let mut i = base();
    let k = idx(&i, "xyz:AAPL");
    i.market[k].session_variants = vec![vec!["24x7".into()]];
    assert!(has(&check::sessions(&i, &launch(&i)), "xyz:AAPL: a non-crypto market must have its real trading windows, not 24/7"));
    let mut i = base();
    i.market[k].verify.clear();
    assert!(has(&check::sessions(&i, &launch(&i)), "xyz:AAPL: a boundary the data does not settle must carry a VERIFY flag"));
    // a window set the generator cannot hold is refused before the rules are even checked
    let mut i = base();
    i.market[0].session_variants = vec![vec!["Mon 00:00-Mon 01:00".into(), "Tue 00:00-Tue 01:00".into(), "Wed 00:00-Wed 01:00".into(), "Thu 00:00-Thu 01:00".into(), "Fri 00:00-Fri 01:00".into(), "Sat 00:00-Sat 01:00".into(), "Sun 00:00-Sun 01:00".into()]];
    assert!(generate(&i).unwrap_err().contains("BTC: the sessions need 7 windows"));
}

// ---- rules 5 and 6: numbers and the rule set

#[test]
fn quantity_and_notional_rules() {
    let i = base();
    let l = launch(&i);
    for (m, o) in i.market.iter().zip(&l.markets) {
        let notional_usd = u128::from(o.qty_min) * (m.price_usd * 1e6).round() as u128 / 1_000_000 / 1_000_000;
        assert!(notional_usd >= 10 || u128::from(o.qty_min) * (m.price_usd * 1e6).round() as u128 >= 10_000_000_000_000, "{}: the minimum is worth at least $10", m.symbol);
        let ceiling = if m.class == 5 { 500_000u64 } else { 5_000_000 };
        assert!(o.max_notional_usd <= ceiling * 1_000_000 && o.max_notional_usd > 0, "{}", m.symbol);
        assert_eq!(o.qty_step, o.qty_min);
    }
    // a tampered launch is caught by the rule
    let mut l = launch(&i);
    l.markets[0].qty_step = 1;
    let v = check::numbers(&i, &l);
    assert!(has(&v, "BTC: quantity step and minimum must be equal and positive"));
    let mut l = launch(&i);
    l.markets[0].max_notional_usd = 5_000_001 * 1_000_000;
    assert!(has(&check::numbers(&i, &l), "BTC: max_notional_usd 5000001000000 is zero or above the ceiling 5000000 USD"));
    let mut l = launch(&i);
    let aapl = idx(&i, "xyz:AAPL");
    l.markets[aapl].max_notional_usd = 500_001 * 1_000_000;
    assert!(has(&check::numbers(&i, &l), "xyz:AAPL: max_notional_usd 500001000000 is zero or above the ceiling 500000 USD"));
    l.markets[aapl].max_notional_usd = 500_000 * 1_000_000;
    assert!(!has(&check::numbers(&i, &l), "xyz:AAPL: max_notional_usd"), "exactly the stock ceiling is allowed");
    l.markets[aapl].max_notional_usd = 0;
    assert!(has(&check::numbers(&i, &l), "xyz:AAPL: max_notional_usd 0 is zero"));
}

#[test]
fn the_rule_set_is_the_launch_rule_set() {
    let i = base();
    let r = launch(&i).ruleset;
    assert_eq!((r.version, r.fee_bps, r.tier_count, r.ladder_len), (1, 40, 10, 5));
    assert_eq!(r.leverage_x, [50, 20, 20, 20, 10, 5, 5, 2]);
    assert_eq!(r.holding_fee_e6, [100, 200, 400, 200, 400, 400, 1000, 1600]);
    assert_eq!(r.reset_ladder_bps, [10_000, 11_500, 12_000, 12_750, 13_000]);
    assert_eq!(r.tier_size, [2_500, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000].map(|x| x * 1_000_000));
    assert_eq!(r.tier_cost, [30, 55, 99, 145, 189, 235, 489, 689, 859, 1_599].map(|x| x * 1_000_000));
    // commission per class (PENDING placeholders): metals 20, indices 20, energy 20, stocks 100, the rest 0
    assert_eq!(r.commission_e6, [0, 0, 20, 20, 20, 100, 0, 0]);
    // half-spreads by market id
    assert_eq!(&r.spread_e6[..21], &[150, 200, 300, 100, 250, 250, 250, 100, 120, 200, 300, 300, 500, 300, 300, 300, 300, 500, 300, 500, 300]);
    assert!(r.spread_e6[21..].iter().all(|s| *s == 0));
    assert_eq!(r.spread_e6.len(), MAX_MARKETS);
}

#[test]
fn a_changed_launch_value_is_refused() {
    type Edit = Box<dyn Fn(&mut launch_config::Input)>;
    let edits: Vec<Edit> = vec![
        Box::new(|i| i.ruleset.fee_bps = 41),
        Box::new(|i| i.ruleset.tier_sizes_usd[0] = 2_501),
        Box::new(|i| i.ruleset.tier_costs_usd[9] = 1_600),
        Box::new(|i| i.ruleset.reset_ladder_bps[4] = 13_001),
        Box::new(|i| i.ruleset.leverage_x[0] = 51),
        Box::new(|i| i.ruleset.holding_fee_e6[7] = 1_601),
    ];
    for (n, e) in edits.iter().enumerate() {
        let mut i = base();
        e(&mut i);
        assert!(has(&all(&i, false), "the rule set differs from the launch values of DESIGN 5.4"), "edit {n}");
    }
}

#[test]
fn an_unpriced_market_is_refused() {
    let mut i = base();
    i.market[3].half_spread_e6 = 0;
    assert!(has(&all(&i, false), "GOLD: half-spread 0 in the rule set: an unpriced market"));
    // and a spread for a market that does not exist
    let i = base();
    let mut l = launch(&i);
    l.ruleset.spread_e6[21] = 5;
    assert!(has(&check::numbers(&i, &l), "a half-spread is set for a market id that does not exist"));
}

#[test]
fn structural_errors_of_the_input() {
    let mut i = base();
    i.commission.pop();
    assert!(generate(&i).unwrap_err().contains("one entry for each of the 8 classes"));
    let mut i = base();
    i.commission[1].class = 0;
    assert!(generate(&i).unwrap_err().contains("out of range or repeated"));
    let mut i = base();
    i.commission[0].class = 8;
    assert!(generate(&i).unwrap_err().contains("out of range or repeated"));
    let mut i = base();
    i.ruleset.tier_costs_usd.pop();
    assert!(generate(&i).unwrap_err().contains("same length"));
    let mut i = base();
    i.ruleset.reset_ladder_bps = vec![];
    assert!(generate(&i).unwrap_err().contains("1 to 5 entries"));
    let mut i = base();
    i.ruleset.leverage_x.pop();
    assert!(generate(&i).unwrap_err().contains("leverage_x needs 8 entries"));
    let mut i = base();
    i.market.clear();
    assert!(generate(&i).unwrap_err().contains("no markets"));
    let mut i = base();
    i.market[0].class = 9;
    assert!(generate(&i).unwrap_err().contains("BTC: class 9 is out of range"));
    let mut i = base();
    i.market[0].price_usd = 0.0;
    assert!(generate(&i).unwrap_err().contains("BTC:"));
    let mut i = base();
    i.market[0].freeze_ms = u64::MAX;
    assert!(generate(&i).unwrap_err().contains("BTC: max_age_secs"));
    let mut i = base();
    let m = i.market[0].clone();
    i.market = vec![m; 129];
    assert!(generate(&i).unwrap_err().contains("129 markets"));
    let mut i = base();
    i.ruleset.tier_sizes_usd = vec![1; 11];
    i.ruleset.tier_costs_usd = vec![1; 11];
    assert!(generate(&i).unwrap_err().contains("same length"));
}

#[test]
fn parse_refuses_unknown_fields_and_a_wrong_format() {
    assert!(parse(&toml().replace("freeze_margin_pct = 25", "freeze_margin_pct = 25\nfreeze_margn_pct = 25")).unwrap_err().contains("unknown field"));
    assert!(parse(&toml().replace("setl8-launch-markets", "other")).unwrap_err().contains("format \"other\" version 1"));
    assert!(parse(&toml().replacen("version = 1", "version = 2", 1)).unwrap_err().contains("version 2"));
    assert!(parse("not toml [").is_err());
    assert!(parse(&toml().replace("symbol = \"BTC\"\n", "")).is_err(), "a missing field is an error");
}

// ---- rule 6/--production

#[test]
fn production_fails_while_a_pending_flag_remains_and_the_default_passes() {
    let i = base();
    assert_eq!(emit::pending_flags(&i), 21 + 8, "one per market half-spread and one per class commission");
    assert!(all(&i, false).is_empty());
    let v = all(&i, true);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("29 PENDING flags remain"), "{v:?}");
    // clearing the flags of the markets alone is not enough
    let mut j = base();
    j.market.iter_mut().for_each(|m| m.spread_pending = false);
    assert!(has(&all(&j, true), "8 PENDING flags remain"));
    // nor is clearing the commissions alone
    let mut j = base();
    j.commission.iter_mut().for_each(|c| c.pending = false);
    assert!(has(&all(&j, true), "21 PENDING flags remain"));
    // one flag left on one market
    let mut j = base();
    j.market.iter_mut().for_each(|m| m.spread_pending = false);
    j.commission.iter_mut().for_each(|c| c.pending = false);
    assert!(all(&j, true).is_empty(), "no flag, production passes");
    j.market[20].spread_pending = true;
    assert!(has(&all(&j, true), "1 PENDING flags remain"));
}

// ---- the committed files

#[test]
fn the_committed_files_must_be_what_the_input_generates() {
    let i = base();
    let l = launch(&i);
    assert!(has(&check::files(&i, &l, &json().replace("\"15\"", "\"16\""), md()), "config/launch.json differs"));
    assert!(has(&check::files(&i, &l, json(), &md().replace("BTC", "BTX")), "config/launch.md differs"));
    assert!(check::files(&i, &l, json(), md()).is_empty());
    // an input change that is not regenerated is caught
    let mut j = base();
    j.market[0].half_spread_e6 = 151;
    let lj = launch(&j);
    let v = check::files(&j, &lj, json(), md());
    assert_eq!(v.len(), 2, "{v:?}");
}

#[test]
fn two_runs_give_identical_bytes() {
    let (a, b) = (base(), base());
    let (la, lb) = (launch(&a), launch(&b));
    assert_eq!(emit::json(&la), emit::json(&lb));
    assert_eq!(emit::markdown(&a, &la), emit::markdown(&b, &lb));
    assert_eq!(emit::json(&la), json());
    assert_eq!(emit::markdown(&a, &la), md());
}

#[test]
fn launch_json_has_only_string_leaves_in_id_order_with_padded_sessions() {
    let v: serde_json::Value = serde_json::from_str(json()).unwrap();
    fn leaves(v: &serde_json::Value, path: &str, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(_) => {}
            serde_json::Value::Array(a) => a.iter().enumerate().for_each(|(k, x)| leaves(x, &format!("{path}[{k}]"), out)),
            serde_json::Value::Object(o) => o.iter().for_each(|(k, x)| leaves(x, &format!("{path}.{k}"), out)),
            other => out.push(format!("{path}: {other}")),
        }
    }
    let mut bad = Vec::new();
    leaves(&v, "", &mut bad);
    assert!(bad.is_empty(), "every integer is a decimal string: {bad:?}");
    let markets = v["markets"].as_array().unwrap();
    assert_eq!(markets.len(), 21);
    for (k, m) in markets.iter().enumerate() {
        assert_eq!(m["market_id"], k.to_string());
        let p = &m["params"];
        assert_eq!(p["sessions"].as_array().unwrap().len(), 6, "six slots");
        let used: usize = p["session_count"].as_str().unwrap().parse().unwrap();
        for (s, w) in p["sessions"].as_array().unwrap().iter().enumerate() {
            assert_eq!(w[0] == "0" && w[1] == "0", s >= used, "market {k} slot {s}");
        }
        assert_eq!(p["quote_conv_market"], "65535");
        assert_eq!(p["status"], "1");
        assert_eq!(p["feed_id"].as_str().unwrap().len(), 64);
    }
    let r = &v["ruleset"];
    assert_eq!(r["spread_e6"].as_array().unwrap().len(), 128);
    assert_eq!(r["tier_size"].as_array().unwrap().len(), 10);
    assert_eq!(r["tier_size"][8], "100000000000");
    assert_eq!(r["version"], "1");
    assert_eq!(r["fee_bps"], "40");
}

#[test]
fn the_markdown_carries_the_numbers_the_report_needs() {
    assert!(md().contains("**PENDING flags: 29**"));
    assert!(md().contains("SPY and QQQ are the ETFs, not the indices"));
    assert!(md().contains("`NO_QUOTE_CONV`"));
    assert!(md().contains("| 9 | xyz:JP225 | JP225 (Nikkei 225) | 4 | 3 Indices | 40 |"));
    assert!(md().contains("| xyz:COIN | p95 | median 5.72 bp, p95 14.74 bp, freeze 24.349 s |"));
    assert!(md().contains("PLACEHOLDER-ADMIN-TUNABLE"));
    assert!(md().contains("Adding a market after launch requires a new rule set version"));
    assert!(md().contains("## VERIFY items"));
}

#[test]
fn zero_network_access_the_crate_depends_on_no_network_library() {
    let manifest = include_str!("../Cargo.toml");
    let deps = manifest.split("[dependencies]").nth(1).unwrap().split("[dev-dependencies]").next().unwrap();
    let names: Vec<&str> = deps.lines().filter_map(|l| l.split('=').next()).map(str::trim).filter(|s| !s.is_empty()).collect();
    assert_eq!(names, vec!["serde", "serde_json", "toml", "sha2"]);
    for banned in ["ureq", "reqwest", "hyper", "tungstenite", "tokio", "curl", "solana", "anchor"] {
        assert!(!manifest.contains(banned), "{banned}");
    }
    // and no source file opens a socket or reads the environment for secrets
    for (name, src) in [("main", include_str!("../src/main.rs")), ("probe", include_str!("../src/probe.rs")), ("check", include_str!("../src/check.rs"))] {
        for banned in ["std::net", "TcpStream", "UdpSocket", "keypair", "std::env::var"] {
            assert!(!src.contains(banned), "{name}: {banned}");
        }
    }
}

// ---- boundaries found by the mutation run

#[test]
fn a_freeze_exactly_at_max_age_passes_and_one_millisecond_more_fails() {
    // with no margin and a 1 s rounding step, max_age is the freeze itself
    let mut i = base();
    i.rules.freeze_margin_pct = 0;
    i.rules.max_age_round_secs = 1;
    i.rules.max_age_floor_secs = 1;
    let k = idx(&i, "xyz:AAPL");
    i.market[k].freeze_ms = 20_000;
    assert_eq!(launch(&i).markets[k].max_age_secs, 20);
    assert!(check::golive(&i).is_empty(), "20,000 ms <= 20 s");
    // a max_age that does not fit u16 reads as 0 here: any freeze is longer
    i.market[k].freeze_ms = u64::MAX;
    assert!(has(&check::golive(&i), "xyz:AAPL: longest freeze 18446744073709551615 ms is longer than max_age_secs 0"));
}

#[test]
fn an_exclusion_with_exactly_the_limit_does_not_fail_the_rule() {
    let mut i = base();
    let coin = i.excluded.iter().position(|e| e.symbol == "xyz:COIN").unwrap();
    i.excluded[coin].p95_bp = Some(12.0);
    assert!(has(&check::golive(&i), "xyz:COIN: excluded for rule \"p95\" but its number passes that rule"));
    i.excluded[coin].p95_bp = Some(12.01);
    assert!(check::golive(&i).is_empty());
    let mstr = i.excluded.iter().position(|e| e.symbol == "xyz:MSTR").unwrap();
    i.excluded[mstr].median_bp = Some(6.0);
    assert!(has(&check::golive(&i), "xyz:MSTR: excluded for rule \"median\" but its number passes that rule"));
    i.excluded[mstr].median_bp = Some(6.01);
    assert!(check::golive(&i).is_empty());
}

#[test]
fn a_launch_with_no_or_too_many_session_windows_is_refused() {
    let i = base();
    let mut l = launch(&i);
    l.markets[0].sessions.clear();
    assert!(has(&check::sessions(&i, &l), "BTC: 0 session windows"));
    let mut l = launch(&i);
    l.markets[0].sessions = vec![(0, 1); 7];
    assert!(has(&check::sessions(&i, &l), "BTC: 7 session windows"));
    let mut l = launch(&i);
    l.markets[0].sessions = vec![(0, 10080); 1];
    assert!(!has(&check::sessions(&i, &l), "session windows"));
    let mut l = launch(&i);
    l.markets[3].sessions = vec![(0, 1); 6];
    assert!(!has(&check::sessions(&i, &l), "GOLD: 6 session windows"), "six is allowed");
}

#[test]
fn the_commission_flag_is_printed_for_its_own_class_only() {
    let mut i = base();
    i.commission[3].pending = false; // class 3, indices
    let md = emit::markdown(&i, &launch(&i));
    assert!(md.contains("| 3 | Indices | 20 | 200 | 20 | - |"), "{md}");
    assert!(md.contains("| 2 | Metals | 20 | 400 | 20 | PENDING |"));
    assert!(md.contains("| 4 | Energy | 10 | 400 | 20 | PENDING |"));
    assert!(md.contains("**PENDING flags: 28**"));
}
