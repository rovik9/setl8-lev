//! The CSV analysers, with fixtures whose answers are worked out by hand, and the comparison with the committed numbers.
mod common;
use common::*;
use launch_config::check::against_probe_data;
use launch_config::probe::*;

const HL_HEADER: &str = "src,recv_ms,coin,oraclePx,markPx,midPx";

fn hl(rows: &[&str]) -> String {
    format!("{HL_HEADER}\n{}\n", rows.join("\n"))
}

/// A coin with five WebSocket messages: oracle 100,100,101,101,102 at 0,1,2,5,6 s against mids 100,100.5,101,100,102.
fn xyz() -> String {
    hl(&[
        "ws,0,xyz:AAPL,100,100,100",
        "rest,500,xyz:AAPL,999,999,999",
        "ws,1000,xyz:AAPL,100,100,100.5",
        "ws,2000,xyz:AAPL,101,101,101",
        "ws,5000,xyz:AAPL,101,101,100",
        "ws,6000,xyz:AAPL,102,102,102",
    ])
}

#[test]
fn oracle_statistics_by_hand() {
    let m = parse_hl_oracle(&xyz()).unwrap();
    let s = oracle_stats(&m["xyz:AAPL"]).unwrap();
    // gaps in bp: 0, 49.7512 (0.5 / 100.5), 0, 100 (1 / 100), 0 -> sorted 0 0 0 49.75 100
    assert_eq!(s.messages, 5);
    assert_eq!(s.median_bp, 0.0);
    assert_eq!(s.p95_bp, 100.0, "nearest rank: ceil(0.95 x 5) = 5");
    assert_eq!(s.max_bp, 100.0);
    // oracle changes at 2,000 and 6,000 ms: one gap of 4,000 ms
    assert_eq!(s.freeze_ms, 4_000);
    assert!(!s.never_changed);
    assert_eq!(s.price_median, 101.0);
}

#[test]
fn the_rest_rows_are_skipped_and_a_missing_mid_is_not_a_gap() {
    let csv = hl(&["ws,0,c,100,100,", "ws,1000,c,100,100,100", "ws,2000,c,100.01,100,100", "rest,3000,c,1,1,1"]);
    let s = oracle_stats(&parse_hl_oracle(&csv).unwrap()["c"]).unwrap();
    assert_eq!(s.messages, 3, "the REST row is not a message");
    // two gaps: 0 and 1 bp; the row without a mid says nothing
    assert!((s.median_bp - 0.5).abs() < 1e-9, "{}", s.median_bp);
    assert!((s.max_bp - 1.0).abs() < 1e-9);
    // a series without any mid has no statistics
    assert!(oracle_stats(&parse_hl_oracle(&hl(&["ws,0,c,100,100,", "ws,1,c,101,100,"])).unwrap()["c"]).is_none());
    assert!(oracle_stats(&[]).is_none());
}

#[test]
fn an_oracle_that_never_changed_is_frozen_for_the_whole_observation() {
    let csv = hl(&["ws,1000,c,100,100,100", "ws,6000,c,100,100,100", "ws,11000,c,100,100,100.2"]);
    let s = oracle_stats(&parse_hl_oracle(&csv).unwrap()["c"]).unwrap();
    assert!(s.never_changed);
    assert_eq!(s.freeze_ms, 10_000);
    assert!((s.median_bp - 0.0).abs() < 1e-9);
    // one change gives no gap between changes: the whole span counts (conservative)
    let csv = hl(&["ws,0,c,100,100,100", "ws,4000,c,101,100,101", "ws,9000,c,101,100,101"]);
    let s = oracle_stats(&parse_hl_oracle(&csv).unwrap()["c"]).unwrap();
    assert!(!s.never_changed);
    assert_eq!(s.freeze_ms, 9_000);
}

#[test]
fn the_median_of_an_even_count_and_the_percentile_ranks() {
    // four messages, gaps 1, 2, 3, 4 bp: median 2.5, p95 rank ceil(3.8) = 4
    let csv = hl(&["ws,0,c,100.01,1,100", "ws,1,c,100.02,1,100", "ws,2,c,100.03,1,100", "ws,3,c,100.04,1,100"]);
    let s = oracle_stats(&parse_hl_oracle(&csv).unwrap()["c"]).unwrap();
    assert!((s.median_bp - 2.5).abs() < 1e-6, "{}", s.median_bp);
    assert!((s.p95_bp - 4.0).abs() < 1e-6);
    // twenty messages: p95 is the 19th value (ceil(19) = 19), not the maximum
    let rows: Vec<String> = (1..=20).map(|k| format!("ws,{k},c,{},1,100", 100.0 + k as f64 / 100.0)).collect();
    let csv = hl(&rows.iter().map(String::as_str).collect::<Vec<_>>());
    let s = oracle_stats(&parse_hl_oracle(&csv).unwrap()["c"]).unwrap();
    assert!((s.p95_bp - 19.0).abs() < 1e-6, "{}", s.p95_bp);
    assert!((s.max_bp - 20.0).abs() < 1e-6);
    assert!((s.median_bp - 10.5).abs() < 1e-6);
}

#[test]
fn csv_errors_are_reported_with_the_line() {
    assert!(parse_hl_oracle("wrong header\n").unwrap_err().contains("unexpected header"));
    assert!(parse_hl_oracle(&hl(&["ws,1,c,100"])).unwrap_err().contains("line 2: 4 fields"));
    assert!(parse_hl_oracle(&hl(&["ws,x,c,100,1,1"])).unwrap_err().contains("line 2: recv_ms"));
    assert!(parse_hl_oracle(&hl(&["ws,1,c,x,1,1"])).unwrap_err().contains("line 2: oraclePx"));
    assert!(parse_hl_oracle("").is_err());
    assert!(gmx_price_medians("x\n").unwrap_err().contains("unexpected header"));
    assert!(gmx_price_medians("recv_ms,symbol,usd\n1,BTC\n").unwrap_err().contains("line 2: 2 fields"));
    assert!(gmx_price_medians("recv_ms,symbol,usd\n1,BTC,x\n").unwrap_err().contains("line 2: usd"));
    assert!(gmx_longest_gap_ms("x\n", "BTC").unwrap_err().contains("unexpected header"));
    let h = "market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\n";
    assert!(gmx_longest_gap_ms(&format!("{h}BTC,1,2\n"), "BTC").unwrap_err().contains("line 2: 3 fields"));
    assert!(gmx_longest_gap_ms(&format!("{h}BTC,1,x,1,1,1\n"), "BTC").unwrap_err().contains("line 2: updated_at_ms"));
}

#[test]
fn gmx_prices_are_the_median_per_symbol() {
    let csv = "recv_ms,symbol,usd\n1,BTC,100\n2,BTC,300\n3,BTC,200\n4,ETH,5\n5,ETH,7\n";
    let m = gmx_price_medians(csv).unwrap();
    assert_eq!(m["BTC"], 200.0);
    assert_eq!(m["ETH"], 6.0, "even count: mean of the middle two");
    assert_eq!(m.len(), 2);
}

#[test]
fn the_gmx_archive_rule_decides_what_counts_as_a_price_change() {
    let h = "market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\n";
    // BTC in receive order: 1000 a | 2000 a (same price) | 3000 b | 2500 c (older: ignored) | 7000 b (same) | 9000 c
    let rows = [
        "BTC,1,1000,10,10,10", "ETH,1,1500,1,1,1", "BTC,2,2000,10,10,10", "BTC,3,3000,20,20,20", "BTC,4,2500,30,30,30", "BTC,5,7000,20,20,20", "BTC,6,9000,30,30,30",
    ];
    let csv = format!("{h}{}\n", rows.join("\n"));
    // price changes at 1000 (the first), 3000, 9000: gaps 2000 and 6000
    assert_eq!(gmx_longest_gap_ms(&csv, "BTC").unwrap(), Some(6_000));
    assert_eq!(gmx_longest_gap_ms(&csv, "ETH").unwrap(), None, "one price change has no gap");
    assert_eq!(gmx_longest_gap_ms(&csv, "SOL").unwrap(), None);
    // bid and ask are compared as a pair
    let csv = format!("{h}BTC,1,1000,10,10,10\nBTC,2,2000,10,11,10\nBTC,3,5000,10,11,10\n");
    assert_eq!(gmx_longest_gap_ms(&csv, "BTC").unwrap(), Some(1_000), "bid or ask moving is a price change; 5000 repeats the pair");
}

// ---- the comparison with the committed numbers

fn matching_input() -> (launch_config::Input, String) {
    let mut i = base();
    let k = idx(&i, "xyz:AAPL");
    i.market[k].freeze_ms = 4_000;
    i.market[k].oracle_median_bp = Some(0.0);
    i.market[k].oracle_p95_bp = Some(100.0);
    i.market[k].price_usd = 101.0;
    (i, xyz())
}

#[test]
fn matching_numbers_pass_and_every_mismatch_is_named() {
    let (i, csv) = matching_input();
    assert_eq!(against_probe_data(&i, Some(&csv), None, None).unwrap(), Vec::<String>::new());
    for (edit, needle) in [
        (Box::new(|i: &mut launch_config::Input| { let k = idx(i, "xyz:AAPL"); i.market[k].freeze_ms = 4_001 }) as Box<dyn Fn(&mut launch_config::Input)>, "xyz:AAPL: freeze_ms 4001 in the input, 4000 in the data"),
        (Box::new(|i| { let k = idx(i, "xyz:AAPL"); i.market[k].oracle_median_bp = Some(0.01) }), "oracle_median_bp Some(0.01) in the input, 0.00 in the data"),
        (Box::new(|i| { let k = idx(i, "xyz:AAPL"); i.market[k].oracle_p95_bp = Some(99.99) }), "oracle_p95_bp Some(99.99) in the input, 100.00 in the data"),
        (Box::new(|i| { let k = idx(i, "xyz:AAPL"); i.market[k].price_usd = 101.5 }), "price_usd 101.5 in the input, 101 in the data"),
    ] {
        let (mut j, _) = matching_input();
        edit(&mut j);
        let v = against_probe_data(&j, Some(&csv), None, None).unwrap();
        assert!(has(&v, needle), "{needle}: {v:?}");
        assert_eq!(v.len(), 1, "{v:?}");
    }
    // a market the data does not contain is simply not compared
    let (i, _) = matching_input();
    assert!(against_probe_data(&i, Some(&hl(&["ws,0,other,1,1,1"])), None, None).unwrap().is_empty());
    assert!(against_probe_data(&i, None, None, None).unwrap().is_empty());
}

#[test]
fn gmx_prices_and_gaps_are_compared() {
    let i = base();
    let g1 = "recv_ms,symbol,usd\n1,BTC,81131.526567\n";
    assert!(against_probe_data(&i, None, Some(g1), None).unwrap().is_empty());
    let g1 = "recv_ms,symbol,usd\n1,BTC,81000\n";
    assert!(has(&against_probe_data(&i, None, Some(g1), None).unwrap(), "BTC: price_usd 81131.526567 in the input, 81000 in the data"));
    // the committed freeze must cover the largest gap in the samples: 3,664 ms is committed
    let h = "market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\n";
    let ok = format!("{h}BTC,1,1000,1,1,1\nBTC,2,4664,2,2,2\nBTC,3,8328,3,3,3\n");
    assert!(against_probe_data(&i, None, None, Some(&ok)).unwrap().is_empty());
    let bad = format!("{h}BTC,1,1000,1,1,1\nBTC,2,4664,2,2,2\nBTC,3,8329,3,3,3\n");
    assert!(has(&against_probe_data(&i, None, None, Some(&bad)).unwrap(), "BTC: freeze_ms 3664 is below the 3665 ms gap in the samples"));
}

#[test]
fn excluded_candidates_numbers_are_compared_too() {
    let i = base();
    let coin = hl(&["ws,0,xyz:COIN,100,1,100", "ws,1,xyz:COIN,100.1475,1,100"]);
    // 0 bp and 14.75 bp: median 7.375 -> not the committed 5.72
    let v = against_probe_data(&i, Some(&coin), None, None).unwrap();
    assert!(has(&v, "xyz:COIN (excluded): median_bp 5.72 in the input, 7.38 in the data") || has(&v, "xyz:COIN (excluded): median_bp 5.72 in the input, 7.37 in the data"), "{v:?}");
    assert!(has(&v, "xyz:COIN (excluded): p95_bp 14.74 in the input, 14.75 in the data"), "{v:?}");
}

#[test]
fn the_real_probe_data_reproduces_the_committed_numbers_when_it_is_present() {
    // The raw CSVs are git-ignored; on a machine that has them this is the end-to-end check of rules 1 and 3.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let hl_dir = root.join("../hl-probe/data/weekday-2026-10-16");
    let (Ok(hl), Ok(g1)) = (std::fs::read_to_string(hl_dir.join("oracle-messages.csv")), std::fs::read_to_string(hl_dir.join("gmx-tickers-1min.csv"))) else {
        eprintln!("raw probe data not present: skipped");
        return;
    };
    let gs = std::fs::read_to_string(root.join("../gmx-probe/data/samples-tracked-all-polls.csv")).ok();
    let v = against_probe_data(&base(), Some(&hl), Some(&g1), gs.as_deref()).unwrap();
    assert!(v.is_empty(), "{v:?}");
    // numbers of the committed hl-oracle-probe-report.md (section 5) as hand constants
    let m = parse_hl_oracle(&hl).unwrap();
    let s = oracle_stats(&m["xyz:JP225"]).unwrap();
    assert_eq!((format!("{:.2}", s.median_bp), format!("{:.2}", s.p95_bp), format!("{:.2}", s.max_bp), s.freeze_ms / 100), ("2.04".into(), "3.75".into(), "5.71".into(), 299));
    let s = oracle_stats(&m["xyz:KR200"]).unwrap();
    assert_eq!((format!("{:.2}", s.median_bp), format!("{:.2}", s.p95_bp), format!("{:.2}", s.max_bp)), ("3.97".into(), "10.72".into(), "20.08".into()));
    let s = oracle_stats(&m["xyz:CAMBRICON"]).unwrap();
    assert!(s.never_changed);
    assert_eq!((format!("{:.2}", s.median_bp), format!("{:.2}", s.max_bp)), ("296.77".into(), "472.79".into()));
}

#[test]
fn a_closed_markets_saturday_gap_is_not_compared_with_its_freeze() {
    // GOLD is closed on Saturday: a 262 s gap in the Saturday samples says nothing about its in-session freeze
    let h = "market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\n";
    let csv = format!("{h}GOLD,1,1000,1,1,1\nGOLD,2,100000,2,2,2\nGOLD,3,400000,3,3,3\n");
    assert!(against_probe_data(&base(), None, None, Some(&csv)).unwrap().is_empty());
}

#[test]
fn an_equal_updated_at_with_another_price_is_not_a_new_change() {
    // the archive rule counts only an `updatedAt` newer than anything seen: a second answer with the same timestamp is ignored
    let h = "market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\n";
    // 1000 price a | 1000 price b (ignored) | 4000 price a again: no second change, so no gap at all
    let csv = format!("{h}BTC,1,1000,10,10,10\nBTC,2,1000,99,99,99\nBTC,3,4000,10,10,10\n");
    assert_eq!(gmx_longest_gap_ms(&csv, "BTC").unwrap(), None);
    // an older answer between two newer ones is ignored too
    let csv = format!("{h}BTC,1,1000,10,10,10\nBTC,2,4000,20,20,20\nBTC,3,2000,30,30,30\nBTC,4,9000,30,30,30\n");
    assert_eq!(gmx_longest_gap_ms(&csv, "BTC").unwrap(), Some(5_000));
}
