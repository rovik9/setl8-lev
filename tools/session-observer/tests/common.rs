#![allow(dead_code)]
use session_observer::analyze::Series;
use std::path::PathBuf;

/// Monday 2026-10-12 00:00:00 UTC in milliseconds.
pub const MON: u64 = 1_791_763_200_000;

/// A time `day` days after that Monday (0 = Monday ... 6 = Sunday, 7 = the next Monday), at h:m:s.
pub fn at(day: u64, h: u64, m: u64, s: u64) -> u64 {
    MON + ((day * 24 + h) * 60 + m) * 60_000 + s * 1000
}

/// A series from (seconds after Monday 00:00, price).
pub fn ser(rows: &[(u64, f64)]) -> Series {
    Series::from_rows(rows.iter().map(|(t, p)| (MON + t * 1000, *p)).collect())
}

pub fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("session-observer-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
