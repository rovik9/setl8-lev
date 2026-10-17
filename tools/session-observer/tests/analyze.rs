mod common;
use common::*;
use session_observer::analyze::*;
use session_observer::markets::{parse_launch, Market};

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} vs {b}");
}

// ---- series

#[test]
fn a_series_keeps_only_price_changes_in_time_order() {
    let s = Series::from_rows(vec![(30, 2.0), (10, 1.0), (20, 1.0), (40, 2.0), (50, 3.0)]);
    assert_eq!(s.points, vec![(10, 1.0), (30, 2.0), (50, 3.0)]);
    assert_eq!((s.first_ms(), s.last_ms()), (Some(10), Some(50)));
    assert_eq!(Series::default().first_ms(), None);
}

#[test]
fn value_at_needs_a_fresh_change() {
    let s = Series::from_rows(vec![(1_000, 5.0), (3_000, 6.0)]);
    assert_eq!(s.value_at(999, 10_000), None, "before the first change");
    assert_eq!(s.value_at(1_000, 10_000), Some(5.0));
    assert_eq!(s.value_at(2_999, 10_000), Some(5.0));
    assert_eq!(s.value_at(3_000, 10_000), Some(6.0));
    assert_eq!(s.value_at(13_000, 10_000), Some(6.0), "exactly fresh_ms old");
    assert_eq!(s.value_at(13_001, 10_000), None);
}

#[test]
fn gmx_rows_out_of_order_or_repeated_are_ignored() {
    let r = |t: u64, sym: &str, u: u64, p: f64| (t, sym.to_string(), u, p);
    let rows = vec![r(1, "BTC", 100, 1.0), r(2, "BTC", 90, 5.0), r(3, "BTC", 110, 2.0), r(4, "BTC", 110, 7.0), r(5, "ETH", 500, 9.0), r(6, "BTC", 120, 2.0)];
    let s = gmx_series(&rows, "BTC");
    assert_eq!(s.points, vec![(1, 1.0), (3, 2.0)], "an older updatedAt and an equal one are ignored; an unchanged price is no change");
    assert_eq!(gmx_series(&rows, "ETH").points, vec![(5, 9.0)]);
    assert!(gmx_series(&rows, "SOL").points.is_empty());
}

// ---- gaps

#[test]
fn a_gap_of_exactly_120_seconds_is_not_reported_and_one_millisecond_over_is() {
    let a = Series::from_rows(vec![(MON, 1.0), (MON + 120_000, 2.0)]);
    assert_eq!(all_gaps(&a).len(), 1);
    assert!(reported_gaps(&a, false, &[]).is_empty());
    let b = Series::from_rows(vec![(MON, 1.0), (MON + 120_001, 2.0)]);
    let g = reported_gaps(&b, false, &[]);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].1, GapKind::InSessionFreeze);
    assert_eq!((g[0].0.len_ms(), g[0].0.prev_price, g[0].0.next_price), (120_001, 1.0, 2.0));
}

#[test]
fn a_gap_of_fifteen_minutes_is_a_break_and_one_millisecond_less_a_freeze() {
    let t = at(1, 10, 0, 0);
    let brk = Series::from_rows(vec![(t, 1.0), (t + 900_000, 2.0)]);
    assert_eq!(reported_gaps(&brk, false, &[])[0].1, GapKind::DailyBreak);
    let freeze = Series::from_rows(vec![(t, 1.0), (t + 899_999, 2.0)]);
    assert_eq!(reported_gaps(&freeze, false, &[])[0].1, GapKind::InSessionFreeze);
    // an always-open market has no breaks: the same 15 minutes are a freeze
    assert_eq!(reported_gaps(&brk, true, &[])[0].1, GapKind::InSessionFreeze);
}

#[test]
fn the_weekend_close_is_a_gap_over_saturday_noon() {
    let close = Series::from_rows(vec![(at(4, 21, 0, 0), 1.0), (at(6, 22, 0, 5), 2.0)]);
    let g = reported_gaps(&close, false, &[]);
    assert_eq!(g[0].1, GapKind::WeekendClose);
    assert_eq!(g[0].0.len_ms(), (25 * 3600 + 5) * 1000 + 24 * 3600 * 1000);
    // ending a minute before Saturday noon: a break, not the weekend
    let early = Series::from_rows(vec![(at(4, 21, 0, 0), 1.0), (at(5, 11, 59, 59), 2.0)]);
    assert_eq!(reported_gaps(&early, false, &[])[0].1, GapKind::DailyBreak);
    // ending exactly at Saturday noon, or starting exactly there, covers it
    let ends = Series::from_rows(vec![(at(4, 21, 0, 0), 1.0), (at(5, 12, 0, 0), 2.0)]);
    assert_eq!(reported_gaps(&ends, false, &[])[0].1, GapKind::WeekendClose);
    let starts = Series::from_rows(vec![(at(5, 12, 0, 0), 1.0), (at(6, 22, 0, 0), 2.0)]);
    assert_eq!(reported_gaps(&starts, false, &[])[0].1, GapKind::WeekendClose);
    // starting one millisecond after Saturday noon of that week: the next noon is a week away
    let after = Series::from_rows(vec![(at(5, 12, 0, 0) + 1, 1.0), (at(6, 22, 0, 0), 2.0)]);
    assert_eq!(reported_gaps(&after, false, &[])[0].1, GapKind::DailyBreak);
    // Sunday evening to Monday morning is a break
    let night = Series::from_rows(vec![(at(6, 21, 0, 0), 1.0), (at(7, 6, 0, 0), 2.0)]);
    assert_eq!(reported_gaps(&night, false, &[])[0].1, GapKind::DailyBreak);
    // a gap that spans a whole week covers a Saturday
    let long = Series::from_rows(vec![(at(0, 1, 0, 0), 1.0), (at(8, 1, 0, 0), 2.0)]);
    assert_eq!(reported_gaps(&long, false, &[])[0].1, GapKind::WeekendClose);
    assert!(covers_saturday_noon(&all_gaps(&long)[0]));
    // 24/7 markets have no weekend either
    assert_eq!(reported_gaps(&close, true, &[])[0].1, GapKind::InSessionFreeze);
}

#[test]
fn a_gap_during_observer_downtime_is_not_the_markets() {
    let t = at(1, 10, 0, 0);
    let s = Series::from_rows(vec![(t, 1.0), (t + 1_000_000, 2.0)]);
    assert_eq!(reported_gaps(&s, false, &[(t + 100_000, t + 500_000)])[0].1, GapKind::ObserverDown);
    assert_eq!(reported_gaps(&s, true, &[(t + 999_000, t + 1_700_000)])[0].1, GapKind::ObserverDown);
    // touching the gap from outside is no overlap
    assert_eq!(reported_gaps(&s, false, &[(t - 5_000, t)])[0].1, GapKind::DailyBreak);
    assert_eq!(reported_gaps(&s, false, &[(t + 1_000_000, t + 1_700_000)])[0].1, GapKind::DailyBreak);
    assert_eq!(reported_gaps(&s, false, &[(t - 5_000, t + 1)])[0].1, GapKind::ObserverDown);
}

#[test]
fn the_longest_in_session_gap_ignores_breaks_closes_and_downtime() {
    let t = at(1, 10, 0, 0);
    let s = Series::from_rows(vec![(t, 1.0), (t + 100_000, 2.0), (t + 400_000, 3.0), (t + 400_000 + 3_600_000, 4.0), (t + 4_000_000 + 50_000, 5.0)]);
    assert_eq!(longest_in_session_ms(&s, false, &[]), 300_000, "the 300 s freeze; the hour is a break");
    assert_eq!(longest_in_session_ms(&s, true, &[]), 3_600_000, "24/7: the hour is a freeze");
    assert_eq!(longest_in_session_ms(&s, false, &[(t + 150_000, t + 350_000)]), 100_000, "the 300 s gap was observer downtime");
    assert_eq!(longest_in_session_ms(&Series::default(), false, &[]), 0);
    let quiet = Series::from_rows(vec![(t, 1.0), (t + 120_000, 2.0)]);
    assert_eq!(longest_in_session_ms(&quiet, false, &[]), 120_000, "gaps of 120 s and less count too");
}

#[test]
fn max_age_by_decision_57() {
    assert_eq!(max_age_for(29_936), 40);
    assert_eq!(max_age_for(0), 15);
    assert_eq!(max_age_for(12_000), 15);
    assert_eq!(max_age_for(12_001), 20);
    assert_eq!(max_age_for(20_000), 25);
    assert_eq!(max_age_for(20_001), 30);
    assert_eq!(max_age_for(300_000), 375);
}

// ---- sessions

#[test]
fn week_windows_split_at_the_week_end() {
    assert_eq!(week_windows(at(0, 0, 0, 30), at(0, 0, 5, 10)), vec![(0, 6)]);
    assert_eq!(week_windows(at(0, 0, 0, 0), at(0, 0, 0, 0)), vec![(0, 1)], "one instant is one minute");
    assert_eq!(week_windows(at(6, 23, 50, 0), at(7, 0, 10, 0)), vec![(10_070, 10_080), (0, 11)]);
    assert_eq!(week_windows(at(6, 23, 50, 0), at(6, 23, 59, 59)), vec![(10_070, 10_080)], "ending in the last minute of the week");
    assert_eq!(week_windows(at(0, 0, 0, 0), at(7, 0, 0, 0)), vec![(0, 10_080)], "a whole week or more");
    assert_eq!(week_windows(at(2, 5, 0, 0), at(2, 5, 59, 0)), vec![(2 * 1440 + 300, 2 * 1440 + 360)]);
}

#[test]
fn merge_windows_joins_overlapping_and_touching_ones() {
    assert_eq!(merge_windows(vec![(10, 20), (20, 30), (40, 50), (5, 12)]), vec![(5, 30), (40, 50)]);
    assert_eq!(merge_windows(vec![]), vec![]);
    assert_eq!(merge_windows(vec![(10, 20), (21, 30)]), vec![(10, 20), (21, 30)]);
    assert_eq!(merge_windows(vec![(0, 100), (10, 20)]), vec![(0, 100)]);
}

#[test]
fn active_intervals_split_where_the_gap_reaches_the_break() {
    let s = Series::from_rows(vec![(0, 1.0), (100_000, 2.0), (1_000_000, 3.0), (1_500_000, 4.0)]);
    assert_eq!(active_intervals(&s, 900_000), vec![(0, 100_000), (1_000_000, 1_500_000)], "a gap of exactly 900 s splits");
    let joined = Series::from_rows(vec![(0, 1.0), (100_000, 2.0), (999_999, 3.0)]);
    assert_eq!(active_intervals(&joined, 900_000), vec![(0, 999_999)], "899.999 s joins");
    assert!(active_intervals(&Series::default(), 900_000).is_empty());
}

#[test]
fn observed_windows_from_two_days_of_the_same_session() {
    // activity 14:30 to 20:00 on Monday and on Tuesday
    let mut rows = Vec::new();
    for day in [0u64, 1] {
        let mut t = at(day, 14, 30, 0);
        while t <= at(day, 20, 0, 0) {
            rows.push((t, (t / 1000) as f64));
            t += 60_000;
        }
    }
    let s = Series::from_rows(rows);
    assert_eq!(observed_windows(&s, 900_000), vec![(870, 1201), (2310, 2641)]);
}

#[test]
fn the_narrower_window_is_recommended() {
    // configured open 100..200, observed active 120..220, covered 90..230
    let c = compare(&[(100, 200)], &[(120, 220)], &[(90, 230)]);
    assert_eq!(c.configured_only, vec![(100, 120)]);
    assert_eq!(c.observed_only, vec![(200, 220)]);
    assert_eq!(c.recommended, vec![(120, 200)]);
    assert_eq!(c.covered_minutes, 140);
    // minutes the run did not cover keep the configuration
    let c = compare(&[(100, 300)], &[(100, 150)], &[(100, 150)]);
    assert_eq!(c.recommended, vec![(100, 300)]);
    assert!(c.configured_only.is_empty() && c.observed_only.is_empty());
    assert_eq!(c.covered_minutes, 50);
    // observed activity outside the configuration is flagged, and not recommended inside the covered minutes
    let c = compare(&[(0, 10)], &[(0, 30)], &[(0, 30)]);
    assert_eq!((c.observed_only.clone(), c.recommended.clone()), (vec![(10, 30)], vec![(0, 10)]));
    // nothing observed in covered minutes: the recommendation is empty
    let c = compare(&[(0, 10)], &[], &[(0, 30)]);
    assert_eq!((c.configured_only.clone(), c.recommended.clone()), (vec![(0, 10)], vec![]));
    // the last minute of the week
    let c = compare(&[(10_070, 10_080)], &[(10_075, 10_080)], &[(10_000, 10_080)]);
    assert_eq!((c.configured_only, c.recommended), (vec![(10_070, 10_075)], vec![(10_075, 10_080)]));
}

#[test]
fn covered_windows_cut_at_downtime() {
    let w = covered_windows(at(0, 0, 0, 0), at(0, 1, 0, 0), &[(at(0, 0, 20, 0), at(0, 0, 40, 0))]);
    assert_eq!(w, vec![(0, 21), (40, 61)]);
    assert_eq!(covered_windows(at(0, 0, 0, 0), at(0, 0, 10, 0), &[]), vec![(0, 11)]);
    // downtime before the start of the data is ignored
    assert_eq!(covered_windows(at(0, 0, 30, 0), at(0, 0, 40, 0), &[(at(0, 0, 0, 0), at(0, 0, 10, 0))]), vec![(30, 41)]);
}

#[test]
fn window_formatting() {
    assert_eq!(fmt_windows(&[]), "(none)");
    assert_eq!(fmt_windows(&[(870, 1200), (10_020, 10_080)]), "Mon 14:30-Mon 20:00, Sun 23:00-Sun 24:00");
}

// ---- cadence

#[test]
fn percentile_and_median_by_hand() {
    let mut v: Vec<f64> = (1..=20).map(f64::from).collect();
    approx(percentile(&mut v, 95.0), 19.0);
    approx(percentile(&mut v, 100.0), 20.0);
    approx(percentile(&mut v, 50.0), 10.0);
    approx(percentile(&mut v, 1.0), 1.0);
    let mut one = vec![7.0];
    approx(percentile(&mut one, 95.0), 7.0);
    assert!(percentile(&mut [], 95.0).is_nan());
    approx(median(&mut [3.0, 1.0, 2.0]), 2.0);
    approx(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
    assert!(median(&mut []).is_nan());
}

#[test]
fn buckets_count_changes_and_gaps_per_hour_of_the_week() {
    // changes at 0 s, 10 s, 30 s of Monday 00:00 and at 3700 s (Monday 01:01:40)
    let s = ser(&[(0, 1.0), (10, 2.0), (30, 3.0), (3700, 4.0)]);
    let b = buckets(&s, &[(0, 120)]);
    assert_eq!(b.len(), 2);
    assert_eq!(b[0], Bucket { hour_of_week: 0, changes: 3, covered_minutes: 60, p95_gap_ms: 20_000, max_gap_ms: 20_000 });
    assert_eq!(b[1], Bucket { hour_of_week: 1, changes: 1, covered_minutes: 60, p95_gap_ms: 3_670_000, max_gap_ms: 3_670_000 });
    // only covered hours are listed, a partly covered hour reports its covered minutes
    let b = buckets(&s, &[(30, 90)]);
    assert_eq!(b.iter().map(|x| (x.hour_of_week, x.covered_minutes)).collect::<Vec<_>>(), vec![(0, 30), (1, 30)]);
    assert!(buckets(&s, &[]).is_empty());
    // an hour without changes is listed with zero changes
    let quiet = buckets(&ser(&[(0, 1.0)]), &[(0, 60), (60, 120)]);
    assert_eq!((quiet[1].changes, quiet[1].max_gap_ms, quiet[1].p95_gap_ms), (0, 0, 0));
}

// ---- offsets

#[test]
fn offsets_on_a_grid_with_hand_numbers() {
    let a = Series::from_rows(vec![(MON, 100.0), (MON + 20_000, 100.5)]);
    let b = Series::from_rows(vec![(MON, 100.0), (MON + 10_000, 100.2), (MON + 20_000, 100.5)]);
    let o = offsets(&a, &b, MON + 20_000, 10_000, 120_000, false);
    assert_eq!(o.len(), 3);
    approx(o[0].1, 0.0);
    approx(o[1].1, (100.0 - 100.2) / 100.2 * 10_000.0);
    approx(o[2].1, 0.0);
    assert_eq!(o[1].0, MON + 10_000);
    // the grid starts at the later first change
    let late = Series::from_rows(vec![(MON + 15_000, 100.0)]);
    let o = offsets(&late, &b, MON + 30_000, 10_000, 120_000, false);
    assert_eq!(o.iter().map(|x| x.0).collect::<Vec<_>>(), vec![MON + 20_000, MON + 30_000]);
    // a stale value is skipped: b's last change is older than fresh_ms
    let o = offsets(&a, &b, MON + 200_000, 10_000, 120_000, false);
    assert_eq!(o.len(), 15, "grid points up to 140 s, when b's change at 20 s is 120 s old");
    assert!(offsets(&Series::default(), &b, MON, 10_000, 120_000, false).is_empty());
}

#[test]
fn ratio_offsets_compare_with_the_ratios_own_median() {
    let a = Series::from_rows(vec![(MON, 200.0), (MON + 10_000, 202.0), (MON + 20_000, 200.0)]);
    let b = Series::from_rows(vec![(MON, 100.0)]);
    let o = offsets(&a, &b, MON + 20_000, 10_000, 1_000_000, true);
    assert_eq!(o.len(), 3);
    approx(o[0].1, 0.0);
    approx(o[1].1, 100.0);
    approx(o[2].1, 0.0);
}

#[test]
fn offset_statistics_overall_and_per_hour() {
    let s = stats_of(vec![-2.0, -1.0, 0.0, 3.0]).unwrap();
    assert_eq!(s.n, 4);
    approx(s.median_bp, -0.5);
    approx(s.p95_abs_bp, 3.0);
    approx(s.max_abs_bp, 3.0);
    assert!(stats_of(vec![]).is_none());
    let off = vec![(at(0, 1, 0, 0), 2.0), (at(0, 1, 30, 0), 4.0), (at(0, 2, 0, 0), -6.0), (at(0, 1, 59, 59), 6.0)];
    let (overall, by) = offset_report(&off);
    approx(overall.unwrap().median_bp, 3.0);
    assert_eq!(by.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
    approx(by[&1].median_bp, 4.0);
    assert_eq!(by[&1].n, 3);
    approx(by[&2].median_bp, -6.0);
    assert!(offset_report(&[]).0.is_none());
}

#[test]
fn which_source_is_furthest() {
    assert_eq!(outlier(10.0, 10.0, 0.5), Outlier::Gmx);
    assert_eq!(outlier(-10.0, 0.3, 10.3), Outlier::Hyperliquid);
    assert_eq!(outlier(0.2, 9.0, 9.0), Outlier::Reference);
    assert!(matches!(outlier(5.0, 5.0, 5.0), Outlier::Undecided(_)));
    assert!(matches!(outlier(2.0, 2.0, 0.1), Outlier::Undecided(_)), "under 3 bp");
    assert!(matches!(outlier(2.99, 2.99, 0.1), Outlier::Undecided(_)));
    assert_eq!(outlier(3.0, 3.0, 0.1), Outlier::Gmx, "3 bp is enough");
    assert!(matches!(outlier(6.0, 6.0, 3.0), Outlier::Undecided(_)), "exactly twice is not more than twice");
    assert_eq!(outlier(6.01, 6.01, 3.0), Outlier::Gmx);
    // signs do not matter
    assert_eq!(outlier(-10.0, -10.0, -0.5), Outlier::Gmx);
    if let Outlier::Undecided(why) = outlier(5.0, 5.0, 5.0) {
        assert!(why.contains("|GMX-HL| 5.0"), "{why}");
    }
}

// ---- data quality

#[test]
fn counters_that_restart_are_summed_over_restarts() {
    assert_eq!(cumulative(&[5, 7, 9]), 9);
    assert_eq!(cumulative(&[5, 7, 2, 4]), 11);
    assert_eq!(cumulative(&[]), 0);
    assert_eq!(cumulative(&[0, 0, 3]), 3);
    assert_eq!(cumulative(&[4, 4, 4]), 4);
    assert_eq!(cumulative(&[3, 0, 0]), 3);
}

#[test]
fn heartbeat_rows_and_downtime() {
    let lines: Vec<String> = [
        "1000,gmx,10,0,100,0,0,0,BTC=900",
        "1000,hl-ws-a,1,0,50,0,0,0,",
        "61000,gmx,20,1,200,0,1,0,BTC=60000",
        "61000,hl-ws-a,1,0,90,1,0,0,",
        "121000,gmx,5,0,10,0,0,0,BTC=120000",
        "not a row",
        "x,gmx,1,1,1,1,1,1,",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let hb = parse_heartbeat(&lines);
    assert_eq!(hb.times, vec![1000, 61000, 121000]);
    // [errors, reconnects, 429, 403, requests]: gmx restarted at the third row (requests 20 -> 5)
    assert_eq!(hb.lanes["gmx"], [1, 0, 1, 0, 25]);
    assert_eq!(hb.lanes["hl-ws-a"], [0, 1, 0, 0, 1]);
    assert_eq!(downtime(&[0, 60_000, 200_000, 260_000]), vec![(60_000, 200_000)]);
    assert!(downtime(&[0, 120_000, 240_000]).is_empty(), "exactly 120 s is not down");
    assert_eq!(downtime(&[0, 120_001]), vec![(0, 120_001)]);
    assert!(downtime(&[]).is_empty() && downtime(&[5]).is_empty());
}

// ---- loading and the report

fn two_markets() -> Vec<Market> {
    vec![
        Market { id: 0, symbol: "BTC".into(), source: 2, class: 6, max_age_secs: 15, sessions: vec![(0, 10_080)] },
        Market { id: 1, symbol: "xyz:AAPL".into(), source: 4, class: 5, max_age_secs: 20, sessions: vec![(870, 1200), (2310, 2640)] },
    ]
}

#[test]
fn launch_json_is_parsed_for_the_market_list() {
    let text = r#"{"markets":[{"market_id":"3","class":"2","params":{"max_age_secs":"35","session_count":"2","sessions":[["0","1260"],["1380","2700"],["0","0"]]},"meta":{"symbol":"GOLD","source":"2"}}]}"#;
    let m = parse_launch(text).unwrap();
    assert_eq!(m, vec![Market { id: 3, symbol: "GOLD".into(), source: 2, class: 2, max_age_secs: 35, sessions: vec![(0, 1260), (1380, 2700)] }]);
    assert!(parse_launch("{}").is_err());
    assert!(parse_launch(r#"{"markets":[]}"#).is_err());
    assert!(parse_launch("not json").is_err());
    // the committed configuration, when it can be found
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/launch.json");
    if let Ok(t) = std::fs::read_to_string(path) {
        let ms = parse_launch(&t).unwrap();
        assert_eq!(ms.len(), 21);
        assert_eq!(ms.iter().filter(|m| m.source == 4).count(), 12);
        assert_eq!(session_observer::markets::hl_coins(&ms).len(), 21);
    }
}

#[test]
fn the_hyperliquid_coin_list_is_the_launch_coins_plus_the_twins_without_duplicates() {
    let coins = session_observer::markets::hl_coins(&two_markets());
    assert_eq!(coins[0], "xyz:AAPL");
    for twin in ["BTC", "ETH", "SOL", "xyz:GOLD", "xyz:SILVER", "xyz:CL", "xyz:BRENTOIL", "xyz:SP500", "xyz:XYZ100"] {
        assert!(coins.contains(&twin.to_string()), "{twin}");
    }
    assert_eq!(coins.len(), 10);
    let mut sorted = coins.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), coins.len());
}

#[test]
fn lane_files_are_read_into_the_data() {
    let mut d = Data::default();
    d.add_file("gmx", "recv_ms,symbol,updated_at_ms,min,max,mid\n1000,BTC,900,1,3,2\nbad\n2000,ETH,1900,10,10,10\n");
    d.add_file("hl-a", "recv_ms,transport,coin,oraclePx,markPx,midPx\n3000,ws,xyz:AAPL,150.5,150.6,150.55\n");
    d.add_file("hl-rest", "recv_ms,transport,coin,oraclePx,markPx,midPx\n4000,rest,xyz:AAPL,150.7,150.6,\n");
    d.add_file("ref", "recv_ms,product,symbol,server_ms,price,bid,ask\n500,BTC-USD,BTC,490,81000.5,81000,81001\n");
    d.add_file("heartbeat", "utc_ms,lane\n7000,gmx,1,0,5,0,0,0,\n");
    d.add_file("pyth-hours", "recv_ms,launch_symbol,pyth_symbol,is_open,next_open,next_close\n6000,BTC,Crypto.BTC/USD,true,null,null\n");
    d.add_file("unknown", "a\nb\n");
    assert_eq!(d.gmx, vec![(1000, "BTC".to_string(), 900, 2.0), (2000, "ETH".to_string(), 1900, 10.0)]);
    assert_eq!(d.hl["xyz:AAPL"], vec![(3000, 150.5), (4000, 150.7)]);
    assert_eq!(d.reference["BTC"], vec![(500, 81000.5)]);
    assert_eq!(d.pyth.len(), 1);
    assert_eq!((d.first_ms, d.last_ms), (Some(500), Some(7000)));
}

#[test]
fn the_report_names_the_weekend_close_the_break_and_the_disagreement() {
    // AAPL (configured Mon and Tue 14:30 to 20:00): ticks every 10 s Monday 14:30 to 20:00, nothing until Tuesday 14:30, ticks to 21:00
    let mut d = Data::default();
    let mut rows = String::from("recv_ms,transport,coin,oraclePx,markPx,midPx\n");
    let mut price = 100.0;
    for (from, to) in [(at(0, 14, 30, 0), at(0, 20, 0, 0)), (at(1, 14, 30, 0), at(1, 21, 0, 0))] {
        let mut t = from;
        while t <= to {
            price += 0.01;
            rows.push_str(&format!("{t},ws,xyz:AAPL,{price:.2},{price:.2},{price:.2}\n"));
            t += 10_000;
        }
    }
    d.add_file("hl-a", &rows);
    // BTC rows over the same days, with a 3-minute freeze on Monday
    let mut g = String::from("recv_ms,symbol,updated_at_ms,min,max,mid\n");
    let mut t = at(0, 14, 0, 0);
    let mut u = 1u64;
    while t <= at(1, 22, 0, 0) {
        if !(t >= at(0, 16, 0, 0) && t < at(0, 16, 3, 0)) {
            u += 1;
            g.push_str(&format!("{t},BTC,{u},{p},{p},{p}\n", p = 80_000 + (u % 7)));
        }
        t += 10_000;
    }
    d.add_file("gmx", &g);
    let mut hb = String::from("utc_ms,lane\n");
    let mut t = at(0, 14, 0, 0);
    while t < at(1, 22, 0, 0) {
        hb.push_str(&format!("{t},gmx,1,0,1,0,0,0,\n"));
        t += 60_000;
    }
    d.add_file("heartbeat", &hb);
    let r = render(&two_markets(), &d);
    // the summary row of AAPL: one daily break, and a disagreement because it traded to 21:00 on Tuesday
    let aapl = r.lines().find(|l| l.starts_with("| 1 | xyz:AAPL")).unwrap();
    assert!(aapl.ends_with("| 0 | 1 | 0 | 2 |"), "no weekend, one daily break, no freeze, two disagreements: {aapl}");
    assert!(r.contains("DAILY BREAK"));
    assert!(r.contains("**DISAGREE: prices changing, configured closed**: Mon 20:00-Mon 20:01, Tue 20:00-Tue 21:01"), "{r}");
    // BTC: a 3-minute freeze in an always-open market
    let btc = r.lines().find(|l| l.starts_with("| 0 | BTC")).unwrap();
    assert!(btc.contains("| 0 | 0 | 1 |"), "{btc}");
    assert!(r.contains("IN-SESSION FREEZE"));
    assert!(r.contains("(d) Reopen gaps"));
    assert!(r.contains("Recording downtime inside it: none"));
    // an empty data set
    assert!(render(&two_markets(), &Data::default()).contains("No data"));
}

#[test]
fn the_report_compares_two_sources_for_a_market() {
    let mut d = Data::default();
    let mut g = String::from("recv_ms,symbol,updated_at_ms,min,max,mid\n");
    let (mut h, mut r) = (String::from("recv_ms,transport,coin,oraclePx,markPx,midPx\n"), String::from("recv_ms,product,symbol,server_ms,price,bid,ask\n"));
    for k in 0..120u64 {
        let t = at(0, 10, 0, 0) + k * 10_000;
        let base = 80_000.0 + (k % 5) as f64;
        g.push_str(&format!("{t},BTC,{},{:.4},{:.4},{:.4}\n", 1000 + k, base * 0.9986, base * 0.9986, base * 0.9986));
        h.push_str(&format!("{t},ws,BTC,{base:.4},{base:.4},{base:.4}\n"));
        r.push_str(&format!("{t},BTC-USD,BTC,{t},{base:.4},{base:.4},{base:.4}\n"));
    }
    d.add_file("gmx", &g);
    d.add_file("hl-a", &h);
    d.add_file("ref", &r);
    let markets = vec![Market { id: 0, symbol: "BTC".into(), source: 2, class: 6, max_age_secs: 15, sessions: vec![(0, 10_080)] }];
    let report = render(&markets, &d);
    assert!(report.contains("GMX (BTC) minus HL (BTC)"));
    assert!(report.contains("median -14.00 bp"), "{report}");
    assert!(report.contains("Which source is furthest from the other two: GMX"), "{report}");
    // without the reference only two sources overlap
    let mut d2 = Data::default();
    d2.add_file("gmx", &g);
    d2.add_file("hl-a", &h);
    assert!(render(&markets, &d2).contains("cannot decide, fewer than three sources"));
}

#[test]
fn analyze_cli_end_to_end_on_a_directory() {
    let dir = scratch("cli");
    let day = dir.join("data").join("2026-10-12");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("gmx.csv"), "recv_ms,symbol,updated_at_ms,min,max,mid\n".to_string() + &format!("{},BTC,5,1,1,1\n{},BTC,6,2,2,2\n", at(0, 10, 0, 0), at(0, 10, 0, 5))).unwrap();
    std::fs::write(dir.join("launch.json"), r#"{"markets":[{"market_id":"0","class":"6","params":{"max_age_secs":"15","session_count":"1","sessions":[["0","10080"]]},"meta":{"symbol":"BTC","source":"2"}}]}"#).unwrap();
    let out = dir.join("report.md");
    let args: Vec<String> = [dir.join("data").to_str().unwrap(), "--out", out.to_str().unwrap(), "--launch", dir.join("launch.json").to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    cli(&args).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("# Session observation report") && text.contains("| 0 | BTC | 2 | 2 |"), "{text}");
    // errors
    assert!(cli(&["--out".into(), out.to_str().unwrap().into()]).unwrap_err().contains("at least one data directory"));
    assert!(cli(&[dir.to_str().unwrap().into()]).unwrap_err().contains("--out"));
    assert!(cli(&["/nonexistent-dir".into(), "--out".into(), out.to_str().unwrap().into(), "--launch".into(), dir.join("launch.json").to_str().unwrap().into()]).unwrap_err().contains("no such directory"));
    let empty = scratch("cli-empty");
    assert!(load(&[empty]).unwrap_err().contains("no CSV files"));
}

#[test]
fn gap_kind_labels() {
    assert_eq!(GapKind::DailyBreak.label(), "DAILY BREAK");
    assert_eq!(GapKind::WeekendClose.label(), "WEEKEND CLOSE");
    assert_eq!(GapKind::InSessionFreeze.label(), "IN-SESSION FREEZE");
    assert_eq!(GapKind::ObserverDown.label(), "OBSERVER DOWN");
}

// ---- found by the mutation run

#[test]
fn saturday_noon_after_the_start_of_the_week_branch() {
    // a gap that starts after Saturday noon needs the NEXT week's noon
    let start = at(5, 13, 0, 0);
    let ends_before = Series::from_rows(vec![(start, 1.0), (at(12, 11, 59, 59), 2.0)]);
    assert_eq!(reported_gaps(&ends_before, false, &[])[0].1, GapKind::DailyBreak);
    let ends_at = Series::from_rows(vec![(start, 1.0), (at(12, 12, 0, 0), 2.0)]);
    assert_eq!(reported_gaps(&ends_at, false, &[])[0].1, GapKind::WeekendClose);
    // start a minute after noon, end exactly at next week's noon
    let s2 = Series::from_rows(vec![(at(5, 12, 1, 0), 1.0), (at(12, 12, 0, 0), 2.0)]);
    assert_eq!(reported_gaps(&s2, false, &[])[0].1, GapKind::WeekendClose);
    // a start within the minute before noon, end at noon
    let s3 = Series::from_rows(vec![(at(5, 11, 59, 30), 1.0), (at(5, 12, 0, 0), 2.0)]);
    assert!(covers_saturday_noon(&all_gaps(&s3)[0]));
    let s4 = Series::from_rows(vec![(at(5, 11, 59, 30), 1.0), (at(5, 11, 59, 59), 2.0)]);
    assert!(!covers_saturday_noon(&all_gaps(&s4)[0]));
    // a Monday start
    let s5 = Series::from_rows(vec![(at(0, 3, 0, 0), 1.0), (at(5, 11, 59, 59), 2.0)]);
    assert!(!covers_saturday_noon(&all_gaps(&s5)[0]));
}

#[test]
fn heartbeat_rows_need_eight_columns() {
    let hb = parse_heartbeat(&["1000,gmx,1,0,5,0,0,0".to_string(), "2000,gmx,1,0,5,0,0".to_string()]);
    assert_eq!(hb.times, vec![1000], "eight columns are enough, seven are not");
}

#[test]
fn loading_descends_two_levels_and_no_further() {
    let root = scratch("depth");
    let write = |rel: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "recv_ms,symbol,updated_at_ms,min,max,mid\n1000,BTC,1,1,1,1\n").unwrap();
    };
    write("gmx.csv");
    write("2026-10-12/gmx.csv");
    write("a/b/gmx.csv");
    write("a/b/c/gmx.csv");
    let d = load(&[root]).unwrap();
    assert_eq!(d.gmx.len(), 3, "the file at three directory levels is not read");
}

#[test]
fn reopen_statistics_by_hand() {
    let t0 = at(0, 14, 30, 0);
    let s = Series::from_rows(vec![(t0 - 900_000, 100.0), (t0, 101.0), (t0 + 60_000, 102.0), (t0 + 90_000, 103.0), (t0 + 30 * 60_000, 104.0), (t0 + 30 * 60_000 + 1, 105.0)]);
    let g = all_gaps(&s)[0];
    assert_eq!((g.len_ms(), g.prev_price, g.next_price), (900_000, 100.0, 101.0));
    let r = reopen_stats(&s, &g, t0 + 40 * 60_000);
    approx(r.jump_bp, 100.0);
    assert_eq!(r.changes, 4, "the point exactly 30 minutes after the reopen is inside, one millisecond later is not");
    approx(r.per_min.unwrap(), 4.0 / 30.0);
    assert_eq!(r.max_gap_ms, 30 * 60_000 - 90_000);
    // data that ends 10 minutes after the reopen
    let r = reopen_stats(&s, &g, t0 + 10 * 60_000);
    approx(r.per_min.unwrap(), 4.0 / 10.0);
    assert_eq!(r.changes, 4, "points at 0, 60 s, 90 s and 30 min are inside the 30-minute window whatever the data end");
    // data that ends at the reopen: no rate
    assert_eq!(reopen_stats(&s, &g, t0).per_min, None);
    assert_eq!(reopen_stats(&s, &g, t0 - 1).per_min, None);
    // a lone point: no gap inside the window
    let lone = Series::from_rows(vec![(t0 - 900_000, 1.0), (t0, 2.0)]);
    let r = reopen_stats(&lone, &all_gaps(&lone)[0], t0 + 60_000);
    assert_eq!((r.changes, r.max_gap_ms), (1, 0));
    approx(jump_bp(100.0, 99.0), -100.0);
    approx(jump_bp(103.0, 104.0), 97.0873786407767);
}

#[test]
fn cadence_rows_and_comparison_lines() {
    let b = Bucket { hour_of_week: 14, changes: 180, covered_minutes: 60, p95_gap_ms: 10_000, max_gap_ms: 12_345 };
    assert_eq!(cadence_row(&b), "| Mon 14:00 | 180 | 3.00 | 10.0 | 12.3 |");
    let b = Bucket { hour_of_week: 25, changes: 6, covered_minutes: 23, p95_gap_ms: 1_550, max_gap_ms: 300_000 };
    assert_eq!(cadence_row(&b), "| Tue 01:00 | 6 | 0.26 | 1.6 | 300.0 |");
    let agree = Comparison { configured_only: vec![], observed_only: vec![], recommended: vec![], covered_minutes: 77 };
    assert_eq!(comparison_lines(&agree), vec!["* agreement over the 77 covered minutes"]);
    let c = Comparison { configured_only: vec![(10, 20)], observed_only: vec![], recommended: vec![], covered_minutes: 5 };
    assert_eq!(comparison_lines(&c), vec!["* **DISAGREE: configured open, nothing observed**: Mon 00:10-Mon 00:20"]);
    let c = Comparison { configured_only: vec![], observed_only: vec![(10, 20)], recommended: vec![], covered_minutes: 5 };
    assert_eq!(comparison_lines(&c), vec!["* **DISAGREE: prices changing, configured closed**: Mon 00:10-Mon 00:20"]);
    let c = Comparison { configured_only: vec![(1, 2)], observed_only: vec![(3, 4)], recommended: vec![], covered_minutes: 5 };
    assert_eq!(comparison_lines(&c).len(), 2);
}

fn section<'a>(report: &'a str, symbol: &str) -> &'a str {
    let head = format!("### {symbol} (");
    let from = report.find(&head).unwrap_or_else(|| panic!("no section for {symbol}"));
    let rest = &report[from + 4..];
    let to = rest.find("\n### ").or_else(|| rest.find("\n## ")).unwrap_or(rest.len());
    &rest[..to]
}

fn heartbeat_every_minute(from: u64, to: u64) -> String {
    let mut s = String::from("utc_ms,lane\n");
    let mut t = from;
    while t <= to {
        s.push_str(&format!("{t},gmx,1,0,1,0,0,0,\n"));
        t += 60_000;
    }
    s
}

#[test]
fn the_report_for_a_hand_built_session_has_exactly_these_numbers() {
    // xyz:AAPL on Monday from 14:30: 100 @0, 101 @60 s, 102 @360 s (a 300 s freeze), 103 @390 s, 104 @1290 s (a 900 s break), 104.04 @1320 s
    let t0 = at(0, 14, 30, 0);
    let mut rows = String::from("recv_ms,transport,coin,oraclePx,markPx,midPx\n");
    for (dt, p) in [(0u64, 100.0), (60, 101.0), (360, 102.0), (390, 103.0), (1290, 104.0), (1320, 104.04)] {
        rows.push_str(&format!("{},ws,xyz:AAPL,{p},{p},{p}\n", t0 + dt * 1000));
    }
    let mut d = Data::default();
    d.add_file("hl-a", &rows);
    d.add_file("heartbeat", &heartbeat_every_minute(t0, t0 + 1_320_000));
    let m = vec![Market { id: 1, symbol: "xyz:AAPL".into(), source: 4, class: 5, max_age_secs: 20, sessions: vec![(870, 1200)] }];
    let r = render(&m, &d);
    // summary: 6 changes, longest in-session gap 300.0 s, now 20, advised 375, no weekend, one break, one freeze, one disagreement (nothing between 14:37 and 14:51)
    let row = r.lines().find(|l| l.starts_with("| 1 | xyz:AAPL")).unwrap();
    assert_eq!(row, "| 1 | xyz:AAPL | 4 | 6 | 300.0 | 20 | 375 | 0 | 1 | 1 | 1 |");
    let sec = section(&r, "xyz:AAPL");
    // the break (the 900 s gap) with its reopen numbers: jump 103 -> 104 = 97.09 bp; the window holds 2 changes in 0.5 min of data
    assert!(sec.contains("| DAILY BREAK (2026-10-12 14:36:30 UTC (Mon)) | 103 | 104 | 97.09 | 4.00 | 30.0 |"), "{sec}");
    // the 300 s gap is listed as a freeze with its length, minute of the week and UTC time
    assert!(sec.contains("| IN-SESSION FREEZE | 2026-10-12 14:31:00 UTC (Mon) | Mon 14:31 | 2026-10-12 14:36:00 UTC (Mon) | Mon 14:36 | 00:05:00 |"), "{sec}");
    // cadence: 23 covered minutes (14:30 to 14:52), 6 changes
    assert!(sec.contains("| Mon 14:00 | 6 | 0.26 | 900.0 | 900.0 |"), "{sec}");
    assert!(sec.contains("Longest in-session gap: **300.0 s** (max_age_secs must exceed it; decision 57 gives 375 s, configured 20 s)."), "{sec}");
    assert!(sec.contains("* **DISAGREE: configured open, nothing observed**: Mon 14:37-Mon 14:51"), "the 14-minute break is configured open: {sec}");
    assert!(sec.contains("0 covered hour(s) without any change."), "{sec}");
    assert!(sec.contains("* configured (config/launch-markets.toml via launch.json): Mon 14:30-Mon 20:00"), "{sec}");
    assert!(sec.contains("* observed (activity separated by less than 900 s): Mon 14:30-Mon 14:37, Mon 14:51-Mon 14:53"), "{sec}");
    assert!(sec.contains("0 covered hour(s) without any change."));
    assert!(sec.contains("* recommendation (the narrower: open in both over covered minutes, configuration elsewhere): Mon 14:30-Mon 14:37, Mon 14:51-Mon 20:00"), "{sec}");
}

#[test]
fn a_24_7_market_has_freezes_and_no_breaks_and_no_reopen_section() {
    let t0 = at(0, 10, 0, 0);
    let mut g = String::from("recv_ms,symbol,updated_at_ms,min,max,mid\n");
    for (k, dt) in [0u64, 10, 20, 1220, 1230].iter().enumerate() {
        g.push_str(&format!("{},BTC,{},{},{},{}\n", t0 + dt * 1000, 100 + k, 50 + k, 50 + k, 50 + k));
    }
    let mut d = Data::default();
    d.add_file("gmx", &g);
    d.add_file("heartbeat", &heartbeat_every_minute(t0, t0 + 1_230_000));
    let m = vec![Market { id: 0, symbol: "BTC".into(), source: 2, class: 6, max_age_secs: 15, sessions: vec![(0, 10_080)] }];
    let r = render(&m, &d);
    let row = r.lines().find(|l| l.starts_with("| 0 | BTC")).unwrap();
    assert_eq!(row, "| 0 | BTC | 2 | 5 | 1200.0 | 15 | 1500 | 0 | 0 | 1 | 0 |", "a 20-minute gap of a 24/7 market is a freeze, not a break, and not a disagreement");
    let sec = section(&r, "BTC");
    assert!(!sec.contains("(d) Reopen gaps"), "{sec}");
    assert!(sec.contains("IN-SESSION FREEZE"));
    assert!(!sec.contains("(f) Cross-source"), "no second source");
    // a market without any change
    let none = vec![Market { id: 5, symbol: "SOL".into(), source: 2, class: 6, max_age_secs: 15, sessions: vec![(0, 10_080)] }];
    let r2 = render(&none, &d);
    assert!(section(&r2, "SOL").contains("No price changes recorded for this market."));
    assert!(r2.contains("| 5 | SOL | 2 | 0 | 0.0 | 15 | n/a |"), "{r2}");
}

#[test]
fn the_hourly_offset_rows_need_thirty_samples() {
    let build = |ticks: u64| {
        let t0 = at(0, 10, 0, 0);
        let (mut g, mut h) = (String::from("recv_ms,symbol,updated_at_ms,min,max,mid\n"), String::from("recv_ms,transport,coin,oraclePx,markPx,midPx\n"));
        for k in 0..ticks {
            let t = t0 + k * 10_000;
            let base = 80_000.0 + (k % 2) as f64;
            g.push_str(&format!("{t},BTC,{},{:.6},{:.6},{:.6}\n", 1 + k, base * 0.9986, base * 0.9986, base * 0.9986));
            h.push_str(&format!("{t},ws,BTC,{base:.6},{base:.6},{base:.6}\n"));
        }
        let mut d = Data::default();
        d.add_file("gmx", &g);
        d.add_file("hl-a", &h);
        let m = vec![Market { id: 0, symbol: "BTC".into(), source: 2, class: 6, max_age_secs: 15, sessions: vec![(0, 10_080)] }];
        render(&m, &d)
    };
    let r30 = build(30);
    assert!(r30.contains("overall n 30, median -14.00 bp"), "{r30}");
    assert!(r30.contains("per hour: Mon 10:00 n 30 med -14.00 p95 14.00 max 14.00"), "{r30}");
    let r29 = build(29);
    assert!(r29.contains("overall n 29,"), "{r29}");
    assert!(!r29.contains("per hour:"), "29 samples are not enough for a row");
}

#[test]
fn an_etf_against_an_index_is_compared_as_a_ratio() {
    let t0 = at(0, 15, 0, 0);
    let (mut g, mut h) = (String::from("recv_ms,symbol,updated_at_ms,min,max,mid\n"), String::from("recv_ms,transport,coin,oraclePx,markPx,midPx\n"));
    for k in 0..40u64 {
        let t = t0 + k * 10_000;
        let idx = 7000.0 + (k % 2) as f64;
        g.push_str(&format!("{t},SPY,{},{:.6},{:.6},{:.6}\n", 1 + k, idx / 10.0, idx / 10.0, idx / 10.0));
        h.push_str(&format!("{t},ws,xyz:SP500,{idx:.6},{idx:.6},{idx:.6}\n"));
    }
    let mut d = Data::default();
    d.add_file("gmx", &g);
    d.add_file("hl-a", &h);
    let m = vec![Market { id: 7, symbol: "SPY".into(), source: 2, class: 3, max_age_secs: 30, sessions: vec![(0, 10_080)] }];
    let r = render(&m, &d);
    assert!(r.contains("as a ratio against its own median (an ETF against an index)"), "{r}");
    assert!(r.contains("overall n 40, median 0.00 bp"), "{r}");
}

#[test]
fn the_pyth_hours_and_the_data_quality_tables() {
    let t = at(0, 10, 0, 0);
    let mut d = Data::default();
    d.add_file("pyth-hours", &format!("h\n{t},GOLD,Metal.XAU/USD,false,1792360800,1792443600\n{},GOLD,Metal.XAU/USD,true,null,null\n", t + 60_000));
    d.add_file("heartbeat", &format!("h\n{t},gmx,10,1,5,0,1,0,\n{},gmx,20,2,9,1,1,0,\n", t + 60_000));
    let r = render(&[], &d);
    assert!(r.contains("| GOLD | Metal.XAU/USD |"), "{r}");
    assert!(r.contains("false") && r.contains("true"));
    assert!(r.contains("| gmx | 20 | 2 | 1 | 1 | 0 |"), "{r}");
    assert!(r.contains("Heartbeat rows: 2; recording downtime: none"), "{r}");
    // downtime is reported with its start and length
    let mut d2 = Data::default();
    d2.add_file("heartbeat", &format!("h\n{t},gmx,1,0,1,0,0,0,\n{},gmx,2,0,2,0,0,0,\n", t + 3_600_000));
    let r2 = render(&[], &d2);
    assert!(r2.contains("2026-10-12 10:00:00 UTC (Mon) for 01:00:00"), "{r2}");
}

#[test]
fn a_covered_hour_without_changes_is_counted_not_listed() {
    let t0 = at(0, 10, 0, 0);
    let mut g = String::from("recv_ms,symbol,updated_at_ms,min,max,mid\n");
    for k in 0..6u64 {
        g.push_str(&format!("{},BTC,{},{},{},{}\n", t0 + k * 60_000, 1 + k, 100 + k, 100 + k, 100 + k));
    }
    let mut d = Data::default();
    d.add_file("gmx", &g);
    // the observer ran for two more hours, but the market was silent in the second one
    d.add_file("heartbeat", &heartbeat_every_minute(t0, t0 + 3_600_000 + 1_800_000));
    let m = vec![Market { id: 0, symbol: "BTC".into(), source: 2, class: 6, max_age_secs: 15, sessions: vec![(0, 10_080)] }];
    let r = render(&m, &d);
    let sec = section(&r, "BTC");
    assert!(sec.contains("| Mon 10:00 | 6 |"), "{sec}");
    assert!(!sec.contains("| Mon 11:00 |"), "an hour without changes is not a row: {sec}");
    assert!(sec.contains("1 covered hour(s) without any change."), "{sec}");
}

#[test]
fn each_pair_of_sources_is_compared_once_and_a_source_never_with_itself() {
    let t0 = at(0, 10, 0, 0);
    let (mut g, mut h, mut c) = (String::from("recv_ms,symbol,updated_at_ms,min,max,mid\n"), String::from("recv_ms,transport,coin,oraclePx,markPx,midPx\n"), String::from("recv_ms,product,symbol,server_ms,price,bid,ask\n"));
    for k in 0..40u64 {
        let t = t0 + k * 10_000;
        let p = 80_000.0 + (k % 2) as f64;
        g.push_str(&format!("{t},BTC,{},{p},{p},{p}\n", 1 + k));
        h.push_str(&format!("{t},ws,BTC,{p},{p},{p}\n"));
        c.push_str(&format!("{t},BTC-USD,BTC,{t},{p},{p},{p}\n"));
    }
    let mut d = Data::default();
    d.add_file("gmx", &g);
    d.add_file("hl-a", &h);
    d.add_file("ref", &c);
    let m = vec![Market { id: 0, symbol: "BTC".into(), source: 2, class: 6, max_age_secs: 15, sessions: vec![(0, 10_080)] }];
    let r = render(&m, &d);
    assert_eq!(r.matches(") minus ").count(), 3, "GMX-HL, GMX-REF, HL-REF");
    assert!(r.contains("GMX (BTC) minus HL (BTC)") && r.contains("GMX (BTC) minus REF (BTC (public spot reference))") && r.contains("HL (BTC) minus REF (BTC (public spot reference))"));
    assert!(!r.contains("GMX (BTC) minus GMX") && !r.contains("HL (BTC) minus HL"));
    assert!(r.contains("cannot decide"), "all three agree exactly");
}
