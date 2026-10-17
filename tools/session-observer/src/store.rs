//! CSV lanes: one file per lane per UTC day (`data/<date>/<lane>.csv`), a header line per file, appended on restart, rotated
//! at 00:00 UTC, flushed at least every 10 s and fsynced every 60 s. Rows are written only when the value changed
//! ([`ChangeFilter`]). Nothing is buffered beyond one `BufWriter`, so memory is flat.
use crate::timeutil;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub const FLUSH_EVERY_MS: u64 = 10_000;
pub const SYNC_EVERY_MS: u64 = 60_000;

pub struct LaneWriter {
    root: PathBuf,
    name: String,
    header: String,
    cur: Option<(String, BufWriter<File>)>,
    last_flush: u64,
    last_sync: u64,
    pub rows: u64,
    pub bytes: u64,
}

impl LaneWriter {
    pub fn new(root: &Path, name: &str, header: &str) -> LaneWriter {
        LaneWriter { root: root.to_path_buf(), name: name.to_string(), header: header.to_string(), cur: None, last_flush: 0, last_sync: 0, rows: 0, bytes: 0 }
    }

    pub fn path_for(root: &Path, date: &str, name: &str) -> PathBuf {
        root.join(date).join(format!("{name}.csv"))
    }

    fn open(&mut self, date: &str) -> std::io::Result<()> {
        if let Some((d, w)) = self.cur.as_mut() {
            if d == date {
                return Ok(());
            }
            w.flush()?;
            w.get_ref().sync_all()?;
            self.cur = None;
        }
        let path = Self::path_for(&self.root, date, &self.name);
        fs::create_dir_all(path.parent().unwrap_or(&self.root))?;
        let fresh = fs::metadata(&path).map(|m| m.len() == 0).unwrap_or(true);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut w = BufWriter::new(file);
        if fresh {
            writeln!(w, "{}", self.header)?;
            self.bytes += self.header.len() as u64 + 1;
        }
        self.cur = Some((date.to_string(), w));
        Ok(())
    }

    /// Appends `line` (no newline) to the file of the UTC day of `ms`, rotating at midnight.
    pub fn write(&mut self, ms: u64, line: &str) -> std::io::Result<()> {
        self.open(&gmx_probe::utc_date(ms))?;
        if let Some((_, w)) = self.cur.as_mut() {
            writeln!(w, "{line}")?;
            self.rows += 1;
            self.bytes += line.len() as u64 + 1;
        }
        self.tick(ms)
    }

    /// Flushes every 10 s and fsyncs every 60 s (call it from the heartbeat as well, so a quiet lane still flushes).
    pub fn tick(&mut self, now_ms: u64) -> std::io::Result<()> {
        if let Some((_, w)) = self.cur.as_mut() {
            if due(now_ms, self.last_flush, FLUSH_EVERY_MS) {
                w.flush()?;
                self.last_flush = now_ms;
            }
            if due(now_ms, self.last_sync, SYNC_EVERY_MS) {
                w.flush()?;
                w.get_ref().sync_all()?;
                self.last_sync = now_ms;
            }
        }
        Ok(())
    }

    pub fn close(&mut self) -> std::io::Result<()> {
        if let Some((_, w)) = self.cur.as_mut() {
            w.flush()?;
            w.get_ref().sync_all()?;
        }
        self.cur = None;
        Ok(())
    }
}

/// Whether an action that runs every `every` ms and last ran at `last` is due at `now`.
pub fn due(now: u64, last: u64, every: u64) -> bool {
    now >= last + every
}

/// Remembers the last value per key; `changed` is true for the first value and for every different one.
#[derive(Default)]
pub struct ChangeFilter {
    last: HashMap<String, String>,
}

impl ChangeFilter {
    pub fn new() -> ChangeFilter {
        ChangeFilter::default()
    }

    pub fn changed(&mut self, key: &str, value: &str) -> bool {
        match self.last.get(key) {
            Some(v) if v == value => false,
            _ => {
                self.last.insert(key.to_string(), value.to_string());
                true
            }
        }
    }

    pub fn len(&self) -> usize {
        self.last.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last.is_empty()
    }
}

/// The time of the last heartbeat row in `root/<date>/heartbeat.csv` (the first CSV column), for the restart log line.
pub fn last_heartbeat_ms(root: &Path) -> Option<u64> {
    let mut dates: Vec<String> = fs::read_dir(root).ok()?.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
    dates.sort();
    for d in dates.iter().rev() {
        if let Ok(text) = fs::read_to_string(LaneWriter::path_for(root, d, "heartbeat")) {
            if let Some(ms) = text.lines().rev().find_map(|l| l.split(',').next()?.parse::<u64>().ok()) {
                return Some(ms);
            }
        }
    }
    None
}

/// `restart: previous heartbeat ... gap ...` or `first start`.
pub fn restart_line(prev: Option<u64>, now_ms: u64) -> String {
    match prev {
        Some(p) if p <= now_ms => format!("{} restart: previous heartbeat {}; recording gap {}", timeutil::fmt_utc(now_ms), timeutil::fmt_utc(p), timeutil::fmt_dur(now_ms - p)),
        Some(_) => format!("{} restart: previous heartbeat is in the future (clock moved?)", timeutil::fmt_utc(now_ms)),
        None => format!("{} first start: no earlier heartbeat", timeutil::fmt_utc(now_ms)),
    }
}
