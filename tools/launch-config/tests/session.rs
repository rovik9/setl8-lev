//! Session derivation (rule 4): segments, the week-end edge, the narrower-window (intersection) rule.
use launch_config::session::*;

fn v(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

#[test]
fn a_segment_in_one_day() {
    assert_eq!(segment("Mon 14:30-Mon 20:00").unwrap(), vec![(870, 1200)]);
    assert_eq!(segment("Tue 00:00-Tue 06:30").unwrap(), vec![(1440, 1830)]);
}

#[test]
fn a_segment_across_midnight_but_inside_the_week() {
    assert_eq!(segment("Mon 23:00-Tue 21:00").unwrap(), vec![(1380, 2700)]);
}

#[test]
fn a_session_ending_exactly_at_the_week_end_is_one_window_to_10080() {
    assert_eq!(segment("Sun 23:00-Sun 24:00").unwrap(), vec![(10020, 10080)]);
    // the same moment written as Monday 00:00 wraps to the start of the week and adds no empty window
    assert_eq!(segment("Sun 23:00-Mon 00:00").unwrap(), vec![(10020, 10080)]);
}

#[test]
fn a_session_crossing_the_week_end_is_two_windows() {
    assert_eq!(segment("Sun 23:00-Mon 21:00").unwrap(), vec![(10020, 10080), (0, 1260)]);
}

#[test]
fn a_day_range_repeats_the_clock_window() {
    assert_eq!(segment("Mon-Fri 14:30-20:00").unwrap(), vec![(870, 1200), (2310, 2640), (3750, 4080), (5190, 5520), (6630, 6960)]);
    assert_eq!(segment("Sat-Sun 00:00-24:00").unwrap(), vec![(7200, 8640), (8640, 10080)]);
}

#[test]
fn the_whole_week() {
    assert_eq!(segment("24x7").unwrap(), vec![(0, 10080)]);
    assert_eq!(windows(&[v(&["24x7"])]).unwrap(), vec![(0, 10080)]);
}

#[test]
fn malformed_segments_are_refused() {
    for bad in [
        "", "Mon", "Mon 25:00-Mon 26:00", "Mon 12:60-Mon 13:00", "Foo 10:00-Foo 11:00", "Mon 10:00", "Mon 10:00-Mon 10:00", "Fri-Mon 10:00-11:00", "Mon-Fri 11:00-10:00",
        "Mon-Fri 10:00-10:00", "Mon 24:30-Tue 01:00", "Mon 1000-Mon 1100", "Mon-Fri 10:00", "Mon 10:00-Mon",
    ] {
        assert!(segment(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn the_narrower_window_wins_when_the_variants_disagree() {
    // US hours in summer (13:30-20:00 UTC) and winter (14:30-21:00 UTC): only 14:30-20:00 is open in both
    let w = windows(&[v(&["Mon-Fri 13:30-20:00"]), v(&["Mon-Fri 14:30-21:00"])]).unwrap();
    assert_eq!(w, vec![(870, 1200), (2310, 2640), (3750, 4080), (5190, 5520), (6630, 6960)]);
    // the order of the variants does not matter
    assert_eq!(windows(&[v(&["Mon-Fri 14:30-21:00"]), v(&["Mon-Fri 13:30-20:00"])]).unwrap(), w);
    // a variant that contains the other adds nothing
    assert_eq!(windows(&[v(&["Mon-Fri 14:30-20:00"]), v(&["Mon-Fri 00:00-24:00"])]).unwrap(), w);
}

#[test]
fn cme_hours_in_both_daylight_saving_states_leave_six_windows() {
    let a = v(&["Sun 22:00-Mon 21:00", "Mon 22:00-Tue 21:00", "Tue 22:00-Wed 21:00", "Wed 22:00-Thu 21:00", "Thu 22:00-Fri 21:00"]);
    let b = v(&["Sun 23:00-Mon 22:00", "Mon 23:00-Tue 22:00", "Tue 23:00-Wed 22:00", "Wed 23:00-Thu 22:00", "Thu 23:00-Fri 22:00"]);
    assert_eq!(windows(&[a, b]).unwrap(), vec![(0, 1260), (1380, 2700), (2820, 4140), (4260, 5580), (5700, 7020), (10020, 10080)]);
}

#[test]
fn touching_windows_are_merged() {
    let w = windows(&[v(&["Mon 00:00-Mon 12:00", "Mon 12:00-Tue 00:00"])]).unwrap();
    assert_eq!(w, vec![(0, 1440)]);
}

#[test]
fn no_common_minute_is_an_error() {
    let e = windows(&[v(&["Mon 00:00-Mon 06:00"]), v(&["Mon 06:00-Mon 12:00"])]).unwrap_err();
    assert_eq!(e, "the session variants have no minute in common");
    assert!(windows(&[]).is_err());
}

#[test]
fn more_than_six_windows_is_an_error_and_exactly_six_is_fine() {
    let six = v(&["Mon 00:00-Mon 01:00", "Tue 00:00-Tue 01:00", "Wed 00:00-Wed 01:00", "Thu 00:00-Thu 01:00", "Fri 00:00-Fri 01:00", "Sat 00:00-Sat 01:00"]);
    assert_eq!(windows(std::slice::from_ref(&six)).unwrap().len(), 6);
    let mut seven = six;
    seven.push("Sun 00:00-Sun 01:00".into());
    assert_eq!(windows(&[seven]).unwrap_err(), "the sessions need 7 windows, the market holds 6");
}

#[test]
fn describe_and_padded() {
    assert_eq!(describe(&[(0, 10080)]), "24/7");
    assert_eq!(describe(&[(870, 1200), (10020, 10080)]), "Mon 14:30-Mon 20:00, Sun 23:00-Sun 24:00");
    assert_eq!(describe(&[(0, 1440)]), "Mon 00:00-Mon 24:00");
    assert_eq!(padded(&[(1, 2), (3, 4)]), [(1, 2), (3, 4), (0, 0), (0, 0), (0, 0), (0, 0)]);
    assert_eq!(padded(&[(0, 10080)])[0], (0, 10080));
}

#[test]
fn clock_limits() {
    assert_eq!(segment("Mon 00:00-Mon 23:59").unwrap(), vec![(0, 1439)]);
    assert_eq!(segment("Mon 12:59-Mon 14:00").unwrap(), vec![(779, 840)]);
    assert!(segment("Mon 12:60-Mon 14:00").is_err(), "minute 60");
    assert!(segment("Mon 25:00-Mon 26:00").is_err());
    assert!(segment("Mon 24:01-Tue 01:00").is_err());
    assert_eq!(segment("Mon 23:00-Mon 24:00").unwrap(), vec![(1380, 1440)]);
}

#[test]
fn a_one_day_range_and_a_missing_dash() {
    assert_eq!(segment("Mon-Mon 10:00-11:00").unwrap(), vec![(600, 660)]);
    assert_eq!(segment("Fri-Fri 10:00-11:00").unwrap(), vec![(5760 + 600, 5760 + 660)]);
    assert!(segment("Mon 23:00 Tue 21:00").is_err(), "a space is not a dash");
    assert!(segment("Mon 23:00Tue 21:00").is_err());
    assert!(segment("Mon 23:00-21:00").is_err(), "the end needs its day");
}

#[test]
fn describe_survives_a_zero_end() {
    // not a valid window, but describing it must not underflow
    assert_eq!(describe(&[(0, 0)]), "Mon 00:00-Mon 00:00");
    assert_eq!(describe(&[(1380, 1440)]), "Mon 23:00-Mon 24:00");
    assert_eq!(describe(&[(1440, 1500)]), "Tue 00:00-Tue 01:00");
}
