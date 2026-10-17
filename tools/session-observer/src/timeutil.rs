//! Time arithmetic in UTC milliseconds. The week starts on Monday 00:00 UTC (minute 0), as `journal_types::session` does.
use std::time::{SystemTime, UNIX_EPOCH};

pub const MIN_PER_WEEK: u32 = 10_080;
const DAY_MS: u64 = 86_400_000;
/// 1970-01-01 was a Thursday, three days after Monday.
const EPOCH_FROM_MONDAY_MS: u64 = 3 * DAY_MS;
const WEEK_MS: u64 = 7 * DAY_MS;
pub const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Minute of the UTC week (Monday 00:00 = 0, Sunday 23:59 = 10079).
pub fn minute_of_week(ms: u64) -> u32 {
    (((ms + EPOCH_FROM_MONDAY_MS) % WEEK_MS) / 60_000) as u32
}

/// Index of the UTC week (Monday-based) a time falls in.
pub fn week_index(ms: u64) -> u64 {
    (ms + EPOCH_FROM_MONDAY_MS) / WEEK_MS
}

/// Hour of the UTC week 0..167.
pub fn hour_of_week(ms: u64) -> u32 {
    minute_of_week(ms) / 60
}

/// `Mon 14:30` for a minute of the week (10080 prints as `Sun 24:00`).
pub fn fmt_minute(m: u32) -> String {
    if m >= MIN_PER_WEEK {
        return "Sun 24:00".into();
    }
    format!("{} {:02}:{:02}", DAY_NAMES[(m / 1440) as usize], m % 1440 / 60, m % 60)
}

/// `2026-10-18 22:00:05 UTC (Sun)`.
pub fn fmt_utc(ms: u64) -> String {
    let secs = ms / 1000;
    let m = minute_of_week(ms);
    format!("{} {:02}:{:02}:{:02} UTC ({})", gmx_probe::utc_date(ms), secs % 86_400 / 3600, secs % 3600 / 60, secs % 60, DAY_NAMES[(m / 1440) as usize])
}

/// Unix ms of 00:00 UTC of the day after `ms`.
pub fn next_midnight_ms(ms: u64) -> u64 {
    (ms / DAY_MS + 1) * DAY_MS
}

/// A duration as `1d 02:03:04` / `02:03:04` / `45 s`.
pub fn fmt_dur(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        return format!("{s} s");
    }
    let (d, h, m, sec) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60, s % 60);
    if d > 0 {
        format!("{d}d {h:02}:{m:02}:{sec:02}")
    } else {
        format!("{h:02}:{m:02}:{sec:02}")
    }
}

/// `--until` value: an ISO-8601 UTC time like `2026-10-19T21:30:00Z`.
pub fn parse_until(s: &str) -> Result<u64, String> {
    gmx_probe::parse_iso_ms(s).ok_or_else(|| format!("--until {s:?} is not an ISO-8601 UTC time like 2026-10-19T21:30:00Z"))
}
