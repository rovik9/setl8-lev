mod common;
use common::*;
use session_observer::timeutil::*;

#[test]
fn minute_of_week_across_sunday_midnight_and_the_week_wrap() {
    assert_eq!(minute_of_week(MON), 0);
    assert_eq!(minute_of_week(at(0, 0, 0, 59)), 0);
    assert_eq!(minute_of_week(at(0, 0, 1, 0)), 1);
    assert_eq!(minute_of_week(at(6, 23, 59, 59)), 10_079);
    assert_eq!(minute_of_week(at(7, 0, 0, 0)), 0, "Sunday midnight is Monday 00:00 of the next week");
    assert_eq!(minute_of_week(at(5, 12, 0, 0)), 7_920);
    assert_eq!(minute_of_week(at(14, 3, 7, 0)), 3 * 60 + 7);
    // the Unix epoch was a Thursday 00:00
    assert_eq!(minute_of_week(0), 3 * 1440);
    assert_eq!(minute_of_week(86_400_000 * 4 - 1), 7 * 1440 - 1);
}

#[test]
fn hour_of_week_and_week_index() {
    assert_eq!(hour_of_week(at(0, 1, 59, 59)), 1);
    assert_eq!(hour_of_week(at(6, 23, 0, 0)), 167);
    assert_eq!(hour_of_week(at(7, 0, 0, 0)), 0);
    assert_eq!(week_index(at(6, 23, 59, 59)) + 1, week_index(at(7, 0, 0, 0)));
    assert_eq!(week_index(at(0, 0, 0, 0)), week_index(at(0, 12, 0, 0)));
}

#[test]
fn formatting() {
    assert_eq!(fmt_minute(0), "Mon 00:00");
    assert_eq!(fmt_minute(1440 + 90), "Tue 01:30");
    assert_eq!(fmt_minute(10_079), "Sun 23:59");
    assert_eq!(fmt_minute(10_080), "Sun 24:00");
    assert_eq!(fmt_minute(20_000), "Sun 24:00");
    assert_eq!(fmt_utc(MON), "2026-10-12 00:00:00 UTC (Mon)");
    assert_eq!(fmt_utc(at(6, 23, 59, 59)), "2026-10-18 23:59:59 UTC (Sun)");
    assert_eq!(fmt_utc(at(5, 12, 5, 9)), "2026-10-17 12:05:09 UTC (Sat)");
    assert_eq!(fmt_dur(0), "0 s");
    assert_eq!(fmt_dur(59_999), "59 s");
    assert_eq!(fmt_dur(60_000), "00:01:00");
    assert_eq!(fmt_dur(3_661_000), "01:01:01");
    assert_eq!(fmt_dur(90_061_000), "1d 01:01:01");
}

#[test]
fn midnight_and_until() {
    assert_eq!(next_midnight_ms(at(0, 23, 59, 59) + 999), at(1, 0, 0, 0));
    assert_eq!(next_midnight_ms(at(1, 0, 0, 0)), at(2, 0, 0, 0));
    assert_eq!(parse_until("2026-10-19T21:30:00Z").unwrap(), at(7, 21, 30, 0));
    for bad in ["2026-10-19 21:30:00", "tomorrow", "2026-10-19T21:30:00", ""] {
        assert!(parse_until(bad).unwrap_err().contains("--until"), "{bad}");
    }
    assert!(now_ms() > MON);
}
