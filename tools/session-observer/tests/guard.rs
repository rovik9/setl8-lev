use session_observer::guard::*;

const T: u64 = 1_000_000;

#[test]
fn a_429_or_403_backs_off_for_a_minute() {
    for status in [429u16, 403] {
        let mut g = Guard::new();
        assert!(g.may_send(T));
        assert_eq!(g.on_status(Some(status), T), Verdict::Backoff);
        assert!(!g.may_send(T));
        assert!(!g.may_send(T + 59_999));
        assert!(g.may_send(T + 60_000), "exactly 60 s later");
        assert_eq!(g.wait_ms(T + 10_000), 50_000);
        assert_eq!(g.wait_ms(T + 60_000), 0);
        assert_eq!(g.wait_ms(T + 90_000), 0);
    }
}

#[test]
fn three_in_a_row_pause_the_lane_for_ten_minutes() {
    let mut g = Guard::new();
    assert_eq!(g.on_status(Some(429), T), Verdict::Backoff);
    assert_eq!(g.on_status(Some(403), T + 61_000), Verdict::Backoff);
    assert_eq!(g.on_status(Some(429), T + 122_000), Verdict::LongPause);
    assert!(!g.may_send(T + 122_000 + 599_999));
    assert!(g.may_send(T + 122_000 + 600_000));
    assert_eq!((g.total_429, g.total_403, g.long_pauses), (2, 1, 1));
    // the count starts again after the long pause: the next incident is a plain back-off
    assert_eq!(g.on_status(Some(429), T + 800_000), Verdict::Backoff);
    assert!(g.may_send(T + 800_000 + 60_000));
}

#[test]
fn a_success_resets_the_run_and_other_errors_do_not_count() {
    let mut g = Guard::new();
    g.on_status(Some(429), T);
    g.on_status(Some(429), T + 61_000);
    assert_eq!(g.on_status(Some(200), T + 122_000), Verdict::Continue);
    assert_eq!(g.on_status(Some(429), T + 123_000), Verdict::Backoff, "two earlier incidents were forgotten");
    // transport errors and other statuses neither count nor reset
    let mut g = Guard::new();
    g.on_status(Some(429), T);
    g.on_status(Some(429), T + 61_000);
    assert_eq!(g.on_status(None, T + 62_000), Verdict::Continue);
    assert_eq!(g.on_status(Some(500), T + 63_000), Verdict::Continue);
    assert_eq!(g.on_status(Some(401), T + 64_000), Verdict::Continue);
    assert_eq!(g.on_status(Some(429), T + 125_000), Verdict::LongPause, "still the third in a row");
    // 2xx boundaries
    let mut g = Guard::new();
    g.on_status(Some(429), T);
    assert_eq!(g.on_status(Some(299), T + 61_000), Verdict::Continue);
    g.on_status(Some(429), T + 62_000);
    g.on_status(Some(429), T + 123_000);
    assert_eq!(g.on_status(Some(300), T + 200_000), Verdict::Continue);
    assert_eq!(g.on_status(Some(429), T + 201_000), Verdict::LongPause, "300 is not a success");
}

#[test]
fn only_a_signal_or_until_stops_the_run() {
    assert!(!should_stop(100, None, false));
    assert!(should_stop(100, None, true));
    assert!(!should_stop(99, Some(100), false));
    assert!(should_stop(100, Some(100), false), "at --until exactly");
    assert!(should_stop(101, Some(100), false));
    assert!(should_stop(0, Some(100), true));
}
