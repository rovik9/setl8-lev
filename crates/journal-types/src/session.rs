//! Weekly session windows (DESIGN 3.4). Minutes of the week in UTC, Monday 00:00 = 0, Sunday 23:59 = 10079.
//! A window is `[start, end)`. Windows do not wrap past 10080: the program's `create_market` requires
//! `start < end <= 10080`, so a session that crosses Sunday midnight is two windows.
use crate::proof::MAX_PROOF_FUTURE_SECS;

/// Minutes in a week.
pub const MINUTES_PER_WEEK: u16 = 10_080;

const SECS_PER_WEEK: i128 = 7 * 24 * 3600;
/// 1970-01-01 was a Thursday: three days after Monday.
const EPOCH_OFFSET_FROM_MONDAY_SECS: i128 = 3 * 24 * 3600;

/// Minute of the week (UTC) of a unix time, Monday 00:00 = 0. Correct for negative times.
pub fn minute_of_week(unix_secs: i64) -> u16 {
    let week_secs = i128::from(unix_secs).saturating_add(EPOCH_OFFSET_FROM_MONDAY_SECS).rem_euclid(SECS_PER_WEEK);
    // week_secs / 60 is below 10080 by construction
    u16::try_from(week_secs / 60).unwrap_or(0)
}

/// True if the time falls inside any of the first `session_count` windows `(start, end)`, start inclusive, end exclusive.
/// Zero sessions means closed. A `session_count` above `sessions.len()` only considers the slots that exist.
pub fn in_session(sessions: &[(u16, u16)], session_count: u8, unix_secs: i64) -> bool {
    let m = minute_of_week(unix_secs);
    sessions
        .iter()
        .take(usize::from(session_count))
        .any(|&(start, end)| start <= m && m < end)
}

/// Inside a session window AND the feed's last publish time is fresh:
/// `now - max_age_secs <= feed_publish_time <= now + 2` (inclusive; a stale or far-future feed reads as closed).
pub fn market_open(
    sessions: &[(u16, u16)],
    session_count: u8,
    now: i64,
    feed_publish_time: i64,
    max_age_secs: u32,
) -> bool {
    let now_w = i128::from(now);
    let t = i128::from(feed_publish_time);
    in_session(sessions, session_count, now)
        && t >= now_w.saturating_sub(i128::from(max_age_secs))
        && t <= now_w.saturating_add(i128::from(MAX_PROOF_FUTURE_SECS))
}
