use journal_types::*;

// 2024-01-01 was a Monday. 1704067200 = 2024-01-01 00:00:00 UTC.
const MON_2024: i64 = 1_704_067_200;

#[test]
fn monday_midnight_utc_is_minute_zero() {
    assert_eq!(minute_of_week(MON_2024), 0);
    assert_eq!(minute_of_week(MON_2024 + 7 * 86_400), 0);
    assert_eq!(minute_of_week(MON_2024 - 7 * 86_400), 0);
    assert_eq!(minute_of_week(MON_2024 + 59), 0);
    assert_eq!(minute_of_week(MON_2024 + 60), 1);
}

#[test]
fn sunday_2359_is_minute_10079() {
    assert_eq!(minute_of_week(MON_2024 + 7 * 86_400 - 1), 10_079);
    assert_eq!(minute_of_week(MON_2024 + 6 * 86_400 + 23 * 3600 + 59 * 60), 10_079);
    assert_eq!(MINUTES_PER_WEEK, 10_080);
}

#[test]
fn each_weekday_starts_on_its_day_boundary() {
    for d in 0..7i64 {
        assert_eq!(i64::from(minute_of_week(MON_2024 + d * 86_400)), d * 1440, "day {d}");
    }
}

#[test]
fn the_unix_epoch_was_a_thursday() {
    assert_eq!(minute_of_week(0), 3 * 1440);
    assert_eq!(minute_of_week(59), 3 * 1440);
    assert_eq!(minute_of_week(3600), 3 * 1440 + 60);
}

#[test]
fn times_before_1970_land_in_the_right_minute() {
    // 1969-12-31 23:59:00 was a Wednesday 23:59: minute 2*1440 + 1439
    assert_eq!(minute_of_week(-60), 2 * 1440 + 1439);
    assert_eq!(minute_of_week(-1), 2 * 1440 + 1439);
    assert_eq!(minute_of_week(-61), 2 * 1440 + 1438);
    // exactly one week before the epoch is the epoch's minute
    assert_eq!(minute_of_week(-7 * 86_400), 3 * 1440);
    // 1969-12-29 (Monday) 00:00 UTC = -3 days
    assert_eq!(minute_of_week(-3 * 86_400), 0);
    // extremes do not panic and stay in range
    for t in [i64::MIN, i64::MAX, i64::MIN + 1, i64::MAX - 1] {
        assert!(minute_of_week(t) < MINUTES_PER_WEEK);
    }
}

const WINDOWS: [(u16, u16); 6] = [(60, 120), (1440, 1500), (0, 0), (0, 0), (0, 0), (0, 0)];

fn at_minute(m: i64) -> i64 {
    MON_2024 + m * 60
}

#[test]
fn window_start_is_inclusive_and_end_is_exclusive() {
    assert!(!in_session(&WINDOWS, 2, at_minute(59)));
    assert!(in_session(&WINDOWS, 2, at_minute(60)));
    assert!(in_session(&WINDOWS, 2, at_minute(119)));
    assert!(!in_session(&WINDOWS, 2, at_minute(120)));
    assert!(in_session(&WINDOWS, 2, at_minute(1440)));
    assert!(in_session(&WINDOWS, 2, at_minute(1499)));
    assert!(!in_session(&WINDOWS, 2, at_minute(1500)));
    // a time inside the minute behaves like its minute
    assert!(in_session(&WINDOWS, 2, at_minute(60) + 59));
    assert!(!in_session(&WINDOWS, 2, at_minute(120) + 59));
}

#[test]
fn the_same_window_applies_every_week() {
    assert!(in_session(&WINDOWS, 2, at_minute(60) + 3 * 7 * 86_400));
    assert!(in_session(&WINDOWS, 2, at_minute(60) - 5 * 7 * 86_400));
}

#[test]
fn zero_sessions_means_closed() {
    assert!(!in_session(&WINDOWS, 0, at_minute(60)));
    assert!(!in_session(&[], 0, at_minute(60)));
    assert!(!in_session(&[], 3, at_minute(60)));
}

#[test]
fn only_the_first_session_count_windows_are_used() {
    assert!(in_session(&WINDOWS, 1, at_minute(60)));
    assert!(!in_session(&WINDOWS, 1, at_minute(1440)));
    assert!(in_session(&WINDOWS, 2, at_minute(1440)));
    // a count above the slice length uses the slots that exist
    assert!(in_session(&WINDOWS, 200, at_minute(1440)));
}

#[test]
fn windows_do_not_wrap_past_the_end_of_the_week() {
    // (10000, 100) is not a wrapping window: start >= end never matches
    let w = [(10_000u16, 100u16)];
    for m in [0i64, 50, 99, 100, 5_000, 10_000, 10_079] {
        assert!(!in_session(&w, 1, at_minute(m)), "minute {m}");
    }
    // a window may end exactly at 10080 and covers the last minute
    let last = [(10_000u16, 10_080u16)];
    assert!(in_session(&last, 1, at_minute(10_079)));
    assert!(!in_session(&last, 1, at_minute(9_999)));
}

#[test]
fn a_stale_feed_means_closed_and_a_fresh_one_open() {
    let now = at_minute(61);
    assert!(market_open(&WINDOWS, 2, now, now, 60));
    assert!(market_open(&WINDOWS, 2, now, now - 60, 60), "exactly max_age old is fresh");
    assert!(!market_open(&WINDOWS, 2, now, now - 61, 60), "one second older is stale");
    assert!(market_open(&WINDOWS, 2, now, now + 2, 60), "2 s ahead is within the skew");
    assert!(!market_open(&WINDOWS, 2, now, now + 3, 60), "3 s ahead is not");
    assert!(!market_open(&WINDOWS, 2, now, 0, 60));
}

#[test]
fn a_fresh_feed_outside_the_session_is_closed() {
    let now = at_minute(200);
    assert!(!market_open(&WINDOWS, 2, now, now, 60));
    assert!(!market_open(&WINDOWS, 0, at_minute(61), at_minute(61), 60));
}

#[test]
fn market_open_needs_both_conditions() {
    let in_s = at_minute(61);
    let out_s = at_minute(200);
    for (now, publish, want) in [(in_s, in_s, true), (in_s, in_s - 3600, false), (out_s, out_s, false), (out_s, out_s - 3600, false)] {
        assert_eq!(market_open(&WINDOWS, 2, now, publish, 60), want);
    }
}
