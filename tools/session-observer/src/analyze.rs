//! The analyzer: turns the recorded CSVs into `handover/session-observation-report.md`. Everything here is pure (strings and
//! vectors in, strings and numbers out) so it is tested with small synthetic files; only [`cli`] touches the file system.
use crate::markets::{self, Market};
use crate::timeutil::{fmt_dur, fmt_minute, fmt_utc, hour_of_week, minute_of_week, MIN_PER_WEEK};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// A gap between price changes longer than this (strictly) is reported.
pub const GAP_REPORT_MS: u64 = 120_000;
/// A reported gap at least this long, in a market that is not 24/7, is a break or a close and not a freeze.
pub const BREAK_MS: u64 = 900_000;
/// The heartbeat gap above which the observer itself counts as down.
pub const DOWN_MS: u64 = 120_000;
/// Offsets are taken on a grid of this step, from values at most this old.
pub const GRID_MS: u64 = 10_000;
pub const FRESH_MS: u64 = 120_000;
/// A source is the outlier only when it is this much further away than the other two are from each other (and at least 3 bp).
pub const OUTLIER_FACTOR: f64 = 2.0;
pub const OUTLIER_MIN_BP: f64 = 3.0;
const SAT_NOON_MIN: u32 = 5 * 1440 + 720;

// ------------------------------------------------------------------ series

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Series {
    /// (receive ms, price) of every price CHANGE, ascending.
    pub points: Vec<(u64, f64)>,
}

impl Series {
    /// Builds a series from raw (ms, price) rows: sorted by time, consecutive equal prices dropped.
    pub fn from_rows(mut rows: Vec<(u64, f64)>) -> Series {
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut points: Vec<(u64, f64)> = Vec::new();
        for (t, p) in rows {
            if points.last().map(|l| l.1) != Some(p) {
                points.push((t, p));
            }
        }
        Series { points }
    }

    /// The price at `t` if the last change is at most `fresh_ms` old.
    pub fn value_at(&self, t: u64, fresh_ms: u64) -> Option<f64> {
        let i = self.points.partition_point(|p| p.0 <= t);
        let (pt, pv) = *self.points.get(i.checked_sub(1)?)?;
        (t - pt <= fresh_ms).then_some(pv)
    }

    pub fn first_ms(&self) -> Option<u64> {
        self.points.first().map(|p| p.0)
    }

    pub fn last_ms(&self) -> Option<u64> {
        self.points.last().map(|p| p.0)
    }
}

/// The GMX rows `(recv_ms, symbol, updated_at_ms, mid)`: the archive rule of the probe (an `updatedAt` older than anything seen
/// is an out-of-order answer and ignored), then price changes only.
pub fn gmx_series(rows: &[(u64, String, u64, f64)], symbol: &str) -> Series {
    let mut max_seen = 0u64;
    let mut out = Vec::new();
    let mut sorted: Vec<&(u64, String, u64, f64)> = rows.iter().filter(|r| r.1 == symbol).collect();
    sorted.sort_by_key(|r| r.0);
    for r in sorted {
        if r.2 > max_seen {
            max_seen = r.2;
            out.push((r.0, r.3));
        }
    }
    Series::from_rows(out)
}

// ------------------------------------------------------------------ gaps

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    DailyBreak,
    WeekendClose,
    InSessionFreeze,
    ObserverDown,
}

impl GapKind {
    pub fn label(self) -> &'static str {
        match self {
            GapKind::DailyBreak => "DAILY BREAK",
            GapKind::WeekendClose => "WEEKEND CLOSE",
            GapKind::InSessionFreeze => "IN-SESSION FREEZE",
            GapKind::ObserverDown => "OBSERVER DOWN",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    pub start_ms: u64,
    pub end_ms: u64,
    pub prev_price: f64,
    pub next_price: f64,
}

impl Gap {
    pub fn len_ms(&self) -> u64 {
        self.end_ms - self.start_ms
    }
}

/// Every gap between consecutive changes (all lengths).
pub fn all_gaps(s: &Series) -> Vec<Gap> {
    s.points.windows(2).map(|w| Gap { start_ms: w[0].0, end_ms: w[1].0, prev_price: w[0].1, next_price: w[1].1 }).collect()
}

/// Whether a gap covers a Saturday-noon-UTC instant (a weekend close, whatever the market): the first Saturday noon at or
/// after the start is inside the gap (both ends inclusive).
pub fn covers_saturday_noon(g: &Gap) -> bool {
    let m0 = u64::from(minute_of_week(g.start_ms));
    let noon = u64::from(SAT_NOON_MIN);
    let delta_min = if m0 <= noon { noon - m0 } else { u64::from(MIN_PER_WEEK) - m0 + noon };
    let mut noon_ms = (g.start_ms / 60_000 + delta_min) * 60_000;
    if noon_ms < g.start_ms {
        // the start is inside the noon minute but after its first millisecond: the next noon is a week away
        noon_ms += 7 * 86_400_000;
    }
    noon_ms <= g.end_ms
}

/// Classifies a gap longer than [`GAP_REPORT_MS`]. `always_open`: the market is configured 24/7 (no break or close exists for
/// it, so every long gap is a freeze). `down`: intervals during which the observer itself was not recording.
pub fn classify(g: &Gap, always_open: bool, down: &[(u64, u64)]) -> GapKind {
    if down.iter().any(|d| g.start_ms < d.1 && d.0 < g.end_ms) {
        return GapKind::ObserverDown;
    }
    if always_open {
        return GapKind::InSessionFreeze;
    }
    if covers_saturday_noon(g) {
        GapKind::WeekendClose
    } else if g.len_ms() >= BREAK_MS {
        GapKind::DailyBreak
    } else {
        GapKind::InSessionFreeze
    }
}

/// The reported gaps (strictly longer than 120 s) with their kinds.
pub fn reported_gaps(s: &Series, always_open: bool, down: &[(u64, u64)]) -> Vec<(Gap, GapKind)> {
    all_gaps(s).into_iter().filter(|g| g.len_ms() > GAP_REPORT_MS).map(|g| (g, classify(&g, always_open, down))).collect()
}

/// Longest gap that is not a break, a close or observer downtime (gaps of 120 s and less included).
pub fn longest_in_session_ms(s: &Series, always_open: bool, down: &[(u64, u64)]) -> u64 {
    all_gaps(s)
        .iter()
        .filter(|g| g.len_ms() <= GAP_REPORT_MS || classify(g, always_open, down) == GapKind::InSessionFreeze)
        .map(Gap::len_ms)
        .max()
        .unwrap_or(0)
}

/// `max_age_secs` by decision 57: longest freeze + 25 %, rounded up to 5 s, never below 15 s.
pub fn max_age_for(longest_ms: u64) -> u64 {
    let ms = (u128::from(longest_ms) * 125).div_ceil(100);
    let secs = ms.div_ceil(1000);
    (secs.div_ceil(5) * 5).max(15) as u64
}

// ------------------------------------------------------------------ sessions

/// Minute-of-week windows `[start, end)` covered by the interval `[a_ms, b_ms]` (split at the week end).
pub fn week_windows(a_ms: u64, b_ms: u64) -> Vec<(u32, u32)> {
    let start = minute_of_week(a_ms);
    let minutes = (b_ms / 60_000 - a_ms / 60_000) as u32 + 1;
    let end = start + minutes;
    if minutes >= MIN_PER_WEEK {
        vec![(0, MIN_PER_WEEK)]
    } else if end <= MIN_PER_WEEK {
        vec![(start, end)]
    } else {
        vec![(start, MIN_PER_WEEK), (0, end - MIN_PER_WEEK)]
    }
}

/// Merges windows into ascending, non-touching windows.
pub fn merge_windows(mut w: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    w.sort();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (s, e) in w {
        match out.last_mut() {
            Some(l) if s <= l.1 => l.1 = l.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// The activity intervals `[first, last]` of a series: changes closer than `break_ms` to each other belong to one interval.
pub fn active_intervals(s: &Series, break_ms: u64) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &(t, _) in &s.points {
        match out.last_mut() {
            Some(l) if t - l.1 < break_ms => l.1 = t,
            _ => out.push((t, t)),
        }
    }
    out
}

/// Observed weekly windows: the union, over the activity intervals, of their minutes of the week.
pub fn observed_windows(s: &Series, break_ms: u64) -> Vec<(u32, u32)> {
    merge_windows(active_intervals(s, break_ms).into_iter().flat_map(|(a, b)| week_windows(a, b)).collect())
}

fn mask(w: &[(u32, u32)]) -> Vec<bool> {
    let mut m = vec![false; MIN_PER_WEEK as usize];
    for &(s, e) in w {
        for x in m.iter_mut().take(e.min(MIN_PER_WEEK) as usize).skip(s as usize) {
            *x = true;
        }
    }
    m
}

fn unmask(m: &[bool]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut start: Option<u32> = None;
    for (i, &x) in m.iter().enumerate() {
        match (x, start) {
            (true, None) => start = Some(i as u32),
            (false, Some(s)) => {
                out.push((s, i as u32));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, MIN_PER_WEEK));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// Covered minutes where the configured window is open and nothing was observed.
    pub configured_only: Vec<(u32, u32)>,
    /// Covered minutes where prices were changing but the configuration says closed.
    pub observed_only: Vec<(u32, u32)>,
    /// The narrower choice: covered minutes open in BOTH; minutes the run did not cover keep the configuration.
    pub recommended: Vec<(u32, u32)>,
    pub covered_minutes: u32,
}

/// Compares configured and observed windows over the minutes of the week the run covered.
pub fn compare(configured: &[(u32, u32)], observed: &[(u32, u32)], covered: &[(u32, u32)]) -> Comparison {
    let (c, o, cv) = (mask(configured), mask(observed), mask(covered));
    let n = MIN_PER_WEEK as usize;
    let configured_only: Vec<bool> = (0..n).map(|i| cv[i] && c[i] && !o[i]).collect();
    let observed_only: Vec<bool> = (0..n).map(|i| cv[i] && o[i] && !c[i]).collect();
    let recommended: Vec<bool> = (0..n).map(|i| if cv[i] { c[i] && o[i] } else { c[i] }).collect();
    Comparison { configured_only: unmask(&configured_only), observed_only: unmask(&observed_only), recommended: unmask(&recommended), covered_minutes: cv.iter().filter(|x| **x).count() as u32 }
}

pub fn fmt_windows(w: &[(u32, u32)]) -> String {
    if w.is_empty() {
        return "(none)".into();
    }
    w.iter().map(|&(s, e)| format!("{}-{}", fmt_minute(s), fmt_minute(e))).collect::<Vec<_>>().join(", ")
}

// ------------------------------------------------------------------ cadence per hour of the week

#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub hour_of_week: u32,
    pub changes: u64,
    pub covered_minutes: u32,
    pub p95_gap_ms: u64,
    pub max_gap_ms: u64,
}

/// Signed jump in bp from `prev` to `next`.
pub fn jump_bp(prev: f64, next: f64) -> f64 {
    (next - prev) / prev * 10_000.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reopen {
    pub jump_bp: f64,
    pub changes: usize,
    /// Changes per minute over the first 30 minutes (or what the data covers of them); `None` when the data ends at the reopen.
    pub per_min: Option<f64>,
    pub max_gap_ms: u64,
}

/// What happened after a gap ended: the jump over it and the cadence of the first 30 minutes (both ends inclusive).
pub fn reopen_stats(s: &Series, g: &Gap, data_end_ms: u64) -> Reopen {
    let window_end = g.end_ms + 30 * 60_000;
    let pts: Vec<u64> = s.points.iter().map(|p| p.0).filter(|t| *t >= g.end_ms && *t <= window_end).collect();
    let covered_ms = data_end_ms.min(window_end).saturating_sub(g.end_ms);
    let mins = covered_ms as f64 / 60_000.0;
    Reopen { jump_bp: jump_bp(g.prev_price, g.next_price), changes: pts.len(), per_min: (mins > 0.0).then(|| pts.len() as f64 / mins), max_gap_ms: pts.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0) }
}

/// One row of the cadence table.
pub fn cadence_row(b: &Bucket) -> String {
    format!("| {} | {} | {:.2} | {:.1} | {:.1} |", fmt_minute(b.hour_of_week * 60), b.changes, b.changes as f64 / f64::from(b.covered_minutes), b.p95_gap_ms as f64 / 1000.0, b.max_gap_ms as f64 / 1000.0)
}

/// The lines that flag a disagreement between the configured and the observed windows (or say they agree).
pub fn comparison_lines(c: &Comparison) -> Vec<String> {
    if c.configured_only.is_empty() && c.observed_only.is_empty() {
        return vec![format!("* agreement over the {} covered minutes", c.covered_minutes)];
    }
    let mut out = Vec::new();
    if !c.configured_only.is_empty() {
        out.push(format!("* **DISAGREE: configured open, nothing observed**: {}", fmt_windows(&c.configured_only)));
    }
    if !c.observed_only.is_empty() {
        out.push(format!("* **DISAGREE: prices changing, configured closed**: {}", fmt_windows(&c.observed_only)));
    }
    out
}

/// Percentile (nearest rank) of values.
pub fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}

pub fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Per hour-of-week bucket: changes, covered minutes, p95 and max gap (a gap belongs to the bucket where it ENDS).
/// `covered` are the minutes-of-week windows the run covered.
pub fn buckets(s: &Series, covered: &[(u32, u32)]) -> Vec<Bucket> {
    let cv = mask(covered);
    let mut changes = vec![0u64; 168];
    let mut gaps: Vec<Vec<f64>> = vec![Vec::new(); 168];
    for p in &s.points {
        changes[hour_of_week(p.0) as usize] += 1;
    }
    for g in all_gaps(s) {
        gaps[hour_of_week(g.end_ms) as usize].push(g.len_ms() as f64);
    }
    (0..168u32)
        .filter_map(|h| {
            let minutes = cv[(h * 60) as usize..(h * 60 + 60) as usize].iter().filter(|x| **x).count() as u32;
            if minutes == 0 {
                return None;
            }
            let mut g = std::mem::take(&mut gaps[h as usize]);
            let max = g.iter().cloned().fold(0.0, f64::max) as u64;
            let p95 = if g.is_empty() { 0 } else { percentile(&mut g, 95.0) as u64 };
            Some(Bucket { hour_of_week: h, changes: changes[h as usize], covered_minutes: minutes, p95_gap_ms: p95, max_gap_ms: max })
        })
        .collect()
}

// ------------------------------------------------------------------ cross-source offsets

#[derive(Debug, Clone, PartialEq)]
pub struct OffsetStats {
    pub n: usize,
    pub median_bp: f64,
    pub p95_abs_bp: f64,
    pub max_abs_bp: f64,
}

pub fn stats_of(mut v: Vec<f64>) -> Option<OffsetStats> {
    if v.is_empty() {
        return None;
    }
    let mut abs: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    Some(OffsetStats { n: v.len(), median_bp: median(&mut v), p95_abs_bp: percentile(&mut abs, 95.0), max_abs_bp: abs.iter().cloned().fold(0.0, f64::max) })
}

/// Signed offsets in bp, `(a - b) / b`, on a grid; `ratio` compares `a / b` with its own median instead (SPY against an index).
/// The grid runs from the later first change to `end_ms` (the end of the data); returns (grid time, offset) pairs where both
/// values are fresh (the last change at most `fresh_ms` old).
pub fn offsets(a: &Series, b: &Series, end_ms: u64, grid_ms: u64, fresh_ms: u64, ratio: bool) -> Vec<(u64, f64)> {
    let (Some(a0), Some(b0)) = (a.first_ms(), b.first_ms()) else { return vec![] };
    let (from, to) = (a0.max(b0), end_ms);
    let mut pairs = Vec::new();
    let mut t = from.div_ceil(grid_ms) * grid_ms;
    while t <= to {
        if let (Some(x), Some(y)) = (a.value_at(t, fresh_ms), b.value_at(t, fresh_ms)) {
            if y != 0.0 {
                pairs.push((t, x, y));
            }
        }
        t += grid_ms;
    }
    if ratio {
        let mut r: Vec<f64> = pairs.iter().map(|p| p.1 / p.2).collect();
        let med = median(&mut r);
        pairs.iter().map(|p| (p.0, (p.1 / p.2 / med - 1.0) * 10_000.0)).collect()
    } else {
        pairs.iter().map(|p| (p.0, (p.1 - p.2) / p.2 * 10_000.0)).collect()
    }
}

/// Offset statistics overall and per hour of the week.
pub fn offset_report(off: &[(u64, f64)]) -> (Option<OffsetStats>, BTreeMap<u32, OffsetStats>) {
    let overall = stats_of(off.iter().map(|o| o.1).collect());
    let mut by: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
    for (t, v) in off {
        by.entry(hour_of_week(*t)).or_default().push(*v);
    }
    (overall, by.into_iter().filter_map(|(h, v)| stats_of(v).map(|s| (h, s))).collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outlier {
    Gmx,
    Hyperliquid,
    Reference,
    Undecided(String),
}

/// Three sources, three signed medians (GMX - HL, GMX - REF, HL - REF in bp): which one sits furthest from the other two?
pub fn outlier(gmx_hl: f64, gmx_ref: f64, hl_ref: f64) -> Outlier {
    let (a, b, c) = (gmx_hl.abs(), gmx_ref.abs(), hl_ref.abs());
    let far = |own: f64, mutual: f64| own > OUTLIER_FACTOR * mutual && own >= OUTLIER_MIN_BP;
    let g = far(a.min(b), c);
    let h = far(a.min(c), b);
    let r = far(b.min(c), a);
    match (g, h, r) {
        (true, false, false) => Outlier::Gmx,
        (false, true, false) => Outlier::Hyperliquid,
        (false, false, true) => Outlier::Reference,
        _ => Outlier::Undecided(format!("|GMX-HL| {a:.1}, |GMX-REF| {b:.1}, |HL-REF| {c:.1} bp: no source is more than {OUTLIER_FACTOR}x (and 3 bp) further from the other two than they are from each other")),
    }
}

// ------------------------------------------------------------------ data quality

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Heartbeat {
    pub times: Vec<u64>,
    /// Per lane: cumulative (errors, reconnects, 429, 403, requests), summed over restarts.
    pub lanes: BTreeMap<String, [u64; 5]>,
}

/// Sum of the increases of a counter that restarts from zero when the process restarts.
pub fn cumulative(values: &[u64]) -> u64 {
    let mut total = 0;
    let mut prev = 0;
    for &v in values {
        total += if v >= prev { v - prev } else { v };
        prev = v;
    }
    total
}

/// Parses heartbeat rows (`utc_ms,lane,requests,errors,rows,reconnects,http429,http403,...`).
pub fn parse_heartbeat(lines: &[String]) -> Heartbeat {
    let mut times = std::collections::BTreeSet::new();
    let mut per: BTreeMap<String, Vec<[u64; 5]>> = BTreeMap::new();
    for l in lines {
        let c: Vec<&str> = l.splitn(9, ',').collect();
        if c.len() < 8 {
            continue;
        }
        let nums: Vec<Option<u64>> = c[..8].iter().enumerate().map(|(i, x)| if i == 1 { Some(0) } else { x.parse().ok() }).collect();
        if nums.iter().any(Option::is_none) {
            continue;
        }
        let n: Vec<u64> = nums.into_iter().flatten().collect();
        times.insert(n[0]);
        per.entry(c[1].to_string()).or_default().push([n[3], n[5], n[6], n[7], n[2]]);
    }
    let mut lanes = BTreeMap::new();
    for (lane, rows) in per {
        let mut tot = [0u64; 5];
        for k in 0..5 {
            tot[k] = cumulative(&rows.iter().map(|r| r[k]).collect::<Vec<_>>());
        }
        lanes.insert(lane, tot);
    }
    Heartbeat { times: times.into_iter().collect(), lanes }
}

/// Intervals during which the observer was not recording: consecutive heartbeat times further apart than [`DOWN_MS`].
pub fn downtime(times: &[u64]) -> Vec<(u64, u64)> {
    times.windows(2).filter(|w| w[1] - w[0] > DOWN_MS).map(|w| (w[0], w[1])).collect()
}

// ------------------------------------------------------------------ loading

/// One row of the published Pyth hours: (recv ms, launch symbol, pyth symbol, is_open, next_open, next_close).
pub type PythRow = (u64, String, String, String, String, String);

#[derive(Debug, Default)]
pub struct Data {
    pub gmx: Vec<(u64, String, u64, f64)>,
    /// Hyperliquid oracle rows by coin (ws and rest merged): (recv ms, oraclePx).
    pub hl: BTreeMap<String, Vec<(u64, f64)>>,
    /// Reference rows by launch symbol: (recv ms, price).
    pub reference: BTreeMap<String, Vec<(u64, f64)>>,
    pub heartbeat: Vec<String>,
    /// Pyth published hours: (recv ms, launch symbol, pyth symbol, is_open, next_open, next_close).
    pub pyth: Vec<PythRow>,
    pub first_ms: Option<u64>,
    pub last_ms: Option<u64>,
}

fn csv_rows(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines().skip(1).filter(|l| !l.is_empty()).map(|l| l.split(',').collect())
}

impl Data {
    fn note(&mut self, ms: u64) {
        self.first_ms = Some(self.first_ms.map_or(ms, |f| f.min(ms)));
        self.last_ms = Some(self.last_ms.map_or(ms, |l| l.max(ms)));
    }

    /// Adds the text of one lane file (`name` is the file stem: gmx, hl-a, hl-b, hl-rest, ref, heartbeat, pyth-hours).
    pub fn add_file(&mut self, name: &str, text: &str) {
        match name {
            "gmx" => {
                for c in csv_rows(text) {
                    if let (Some(Ok(t)), Some(sym), Some(Ok(u)), Some(Ok(mid))) = (c.first().map(|x| x.parse::<u64>()), c.get(1), c.get(2).map(|x| x.parse::<u64>()), c.get(5).map(|x| x.parse::<f64>())) {
                        self.note(t);
                        self.gmx.push((t, sym.to_string(), u, mid));
                    }
                }
            }
            "hl-a" | "hl-b" | "hl-rest" => {
                for c in csv_rows(text) {
                    if let (Some(Ok(t)), Some(coin), Some(Ok(o))) = (c.first().map(|x| x.parse::<u64>()), c.get(2), c.get(3).map(|x| x.parse::<f64>())) {
                        self.note(t);
                        self.hl.entry(coin.to_string()).or_default().push((t, o));
                    }
                }
            }
            "ref" => {
                for c in csv_rows(text) {
                    if let (Some(Ok(t)), Some(sym), Some(Ok(p))) = (c.first().map(|x| x.parse::<u64>()), c.get(2), c.get(4).map(|x| x.parse::<f64>())) {
                        self.note(t);
                        self.reference.entry(sym.to_string()).or_default().push((t, p));
                    }
                }
            }
            "heartbeat" => {
                for l in text.lines().skip(1).filter(|l| !l.is_empty()) {
                    if let Some(Ok(t)) = l.split(',').next().map(|x| x.parse::<u64>()) {
                        self.note(t);
                        self.heartbeat.push(l.to_string());
                    }
                }
            }
            "pyth-hours" => {
                for c in csv_rows(text) {
                    if c.len() >= 6 {
                        if let Ok(t) = c[0].parse::<u64>() {
                            self.pyth.push((t, c[1].into(), c[2].into(), c[3].into(), c[4].into(), c[5].into()));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Reads every lane file under the given directories (a data dir with date sub-directories, or date directories).
pub fn load(dirs: &[PathBuf]) -> Result<Data, String> {
    let mut data = Data::default();
    let mut files: Vec<PathBuf> = Vec::new();
    fn walk(p: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(p) {
            let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            entries.sort();
            for e in entries {
                if e.is_dir() && depth < 2 {
                    walk(&e, depth + 1, out);
                } else if e.extension().and_then(|x| x.to_str()) == Some("csv") {
                    out.push(e);
                }
            }
        }
    }
    for d in dirs {
        if !d.exists() {
            return Err(format!("{}: no such directory", d.display()));
        }
        walk(d, 0, &mut files);
    }
    if files.is_empty() {
        return Err("no CSV files found".into());
    }
    for f in files {
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        data.add_file(&stem, &text);
    }
    Ok(data)
}

/// Minutes of the week the run covered: the span of the data, cut at the observer's own downtime.
pub fn covered_windows(first: u64, last: u64, down: &[(u64, u64)]) -> Vec<(u32, u32)> {
    let mut segs = Vec::new();
    let mut from = first;
    for &(a, b) in down {
        if a >= from {
            segs.push((from, a));
        }
        from = from.max(b);
    }
    if last >= from {
        segs.push((from, last));
    }
    merge_windows(segs.into_iter().flat_map(|(a, b)| week_windows(a, b)).collect())
}

// ------------------------------------------------------------------ report

fn bp(x: f64) -> String {
    format!("{x:.2}")
}

fn series_of(m: &Market, d: &Data) -> Series {
    if m.source == 2 {
        gmx_series(&d.gmx, &m.symbol)
    } else {
        Series::from_rows(d.hl.get(&m.symbol).cloned().unwrap_or_default())
    }
}

fn is_24_7(m: &Market) -> bool {
    m.sessions == [(0, MIN_PER_WEEK)]
}

fn sources_of(m: &Market, d: &Data) -> Vec<(&'static str, Series, String)> {
    // (label, series, display name) of every source this market's underlying has data for
    let mut out = Vec::new();
    if m.source == 2 {
        out.push(("GMX", gmx_series(&d.gmx, &m.symbol), m.symbol.clone()));
        if let Some((_, hl)) = markets::HL_TWINS.iter().find(|t| t.0 == m.symbol) {
            out.push(("HL", Series::from_rows(d.hl.get(*hl).cloned().unwrap_or_default()), (*hl).to_string()));
        }
    } else {
        out.push(("HL", Series::from_rows(d.hl.get(&m.symbol).cloned().unwrap_or_default()), m.symbol.clone()));
    }
    if let Some(r) = d.reference.get(&m.symbol) {
        out.push(("REF", Series::from_rows(r.clone()), format!("{} (public spot reference)", m.symbol)));
    }
    out.retain(|s| !s.1.points.is_empty());
    out
}

/// The whole report.
pub fn render(markets: &[Market], d: &Data) -> String {
    let mut s = String::new();
    let (Some(first), Some(last)) = (d.first_ms, d.last_ms) else { return "# Session observation report\n\nNo data.\n".into() };
    let hb = parse_heartbeat(&d.heartbeat);
    let down = downtime(&hb.times);
    let covered = covered_windows(first, last, &down);
    let _ = writeln!(s, "# Session observation report (generated by `session-observer analyze`)\n");
    let _ = writeln!(s, "* Data: {} to {} ({}). Recording downtime inside it: {}.", fmt_utc(first), fmt_utc(last), fmt_dur(last - first), if down.is_empty() { "none".into() } else { down.iter().map(|d| format!("{} to {}", fmt_utc(d.0), fmt_utc(d.1))).collect::<Vec<_>>().join("; ") });
    let _ = writeln!(s, "* Minutes of the UTC week covered: {} of {} ({}).", covered.iter().map(|w| w.1 - w.0).sum::<u32>(), MIN_PER_WEEK, fmt_windows(&covered));
    let _ = writeln!(s, "* Price-change gaps longer than {} s are reported; a gap of {} s or more that is not 24/7 and not over a Saturday is a DAILY BREAK, a gap over Saturday noon UTC a WEEKEND CLOSE, any other gap an IN-SESSION FREEZE. Windows are minutes of the UTC week (Monday 00:00 = 0).\n", GAP_REPORT_MS / 1000, BREAK_MS / 1000);

    let _ = writeln!(s, "## Summary per market\n");
    let _ = writeln!(s, "| id | symbol | src | changes | longest in-session gap (s) | max_age now (s) | max_age advised (s) | weekend closes | daily breaks | freezes > 120 s | session disagreements |");
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|");
    let mut details = String::new();
    for m in markets {
        let ser = series_of(m, d);
        let always = is_24_7(m);
        let gaps = reported_gaps(&ser, always, &down);
        let count = |k: GapKind| gaps.iter().filter(|g| g.1 == k).count();
        let longest = longest_in_session_ms(&ser, always, &down);
        let observed = observed_windows(&ser, if always { u64::MAX } else { BREAK_MS });
        let cmp = compare(&m.sessions, &observed, &covered);
        let disagreements = cmp.configured_only.len() + cmp.observed_only.len();
        let _ = writeln!(s, "| {} | {} | {} | {} | {:.1} | {} | {} | {} | {} | {} | {} |", m.id, m.symbol, m.source, ser.points.len(), longest as f64 / 1000.0, m.max_age_secs, if ser.points.is_empty() { "n/a".into() } else { max_age_for(longest).to_string() }, count(GapKind::WeekendClose), count(GapKind::DailyBreak), count(GapKind::InSessionFreeze), disagreements);

        let _ = writeln!(details, "### {} (id {}, source {}, class {})\n", m.symbol, m.id, m.source, m.class);
        if ser.points.is_empty() {
            let _ = writeln!(details, "No price changes recorded for this market.\n");
            continue;
        }
        let _ = writeln!(details, "**(a) Gaps longer than 120 s**\n");
        if gaps.is_empty() {
            let _ = writeln!(details, "None.\n");
        } else {
            let _ = writeln!(details, "| kind | start (UTC) | start minute of week | end (UTC) | end minute of week | length |");
            let _ = writeln!(details, "|---|---|---|---|---|---|");
            for (g, k) in &gaps {
                let _ = writeln!(details, "| {} | {} | {} | {} | {} | {} |", k.label(), fmt_utc(g.start_ms), fmt_minute(minute_of_week(g.start_ms)), fmt_utc(g.end_ms), fmt_minute(minute_of_week(g.end_ms)), fmt_dur(g.len_ms()));
            }
            let _ = writeln!(details);
        }
        let _ = writeln!(details, "**(b) Session windows**\n\n* configured (config/launch-markets.toml via launch.json): {}\n* observed ({}): {}", fmt_windows(&m.sessions), if always { "24/7 market: no break is looked for".to_string() } else { format!("activity separated by less than {} s", BREAK_MS / 1000) }, fmt_windows(&observed));
        for l in comparison_lines(&cmp) {
            let _ = writeln!(details, "{l}");
        }
        let _ = writeln!(details, "* recommendation (the narrower: open in both over covered minutes, configuration elsewhere): {}\n", fmt_windows(&cmp.recommended));

        let _ = writeln!(details, "**(c) Cadence per hour of the week** (covered hours with at least one change; a gap counts in the hour where it ends)\n");
        let b = buckets(&ser, &covered);
        let _ = writeln!(details, "| hour | changes | changes per minute | p95 gap (s) | max gap (s) |");
        let _ = writeln!(details, "|---|---|---|---|---|");
        for x in b.iter().filter(|x| x.changes > 0) {
            let _ = writeln!(details, "{}", cadence_row(x));
        }
        let silent = b.iter().filter(|x| x.changes == 0).count();
        let _ = writeln!(details, "\n{silent} covered hour(s) without any change.\n");
        let _ = writeln!(details, "Longest in-session gap: **{:.1} s** (max_age_secs must exceed it; decision 57 gives {} s, configured {} s).\n", longest as f64 / 1000.0, max_age_for(longest), m.max_age_secs);

        let closes: Vec<&(Gap, GapKind)> = gaps.iter().filter(|g| matches!(g.1, GapKind::DailyBreak | GapKind::WeekendClose)).collect();
        if !closes.is_empty() {
            let _ = writeln!(details, "**(d) Reopen gaps and (e) the first 30 minutes after each reopen**\n");
            let _ = writeln!(details, "| after | last price before | first price after | jump (bp) | changes per minute, first 30 min | max gap in the first 30 min (s) |");
            let _ = writeln!(details, "|---|---|---|---|---|---|");
            for (g, k) in closes {
                let r = reopen_stats(&ser, g, last);
                let _ = writeln!(details, "| {} ({}) | {} | {} | {} | {} | {:.1} |", k.label(), fmt_utc(g.start_ms), g.prev_price, g.next_price, bp(r.jump_bp), r.per_min.map_or("n/a".to_string(), |x| format!("{x:.2}")), r.max_gap_ms as f64 / 1000.0);
            }
            let _ = writeln!(details);
        }

        let srcs = sources_of(m, d);
        if srcs.len() >= 2 {
            let _ = writeln!(details, "**(f) Cross-source offsets (signed bp, first minus second)**\n");
            let ratio = matches!(m.symbol.as_str(), "SPY" | "QQQ");
            let mut medians: BTreeMap<(&str, &str), f64> = BTreeMap::new();
            for i in 0..srcs.len() {
                for j in (i + 1)..srcs.len() {
                    let off = offsets(&srcs[i].1, &srcs[j].1, last, GRID_MS, FRESH_MS, ratio && srcs[j].0 == "HL");
                    let (overall, by) = offset_report(&off);
                    let _ = writeln!(details, "* {} ({}) minus {} ({}){}:", srcs[i].0, srcs[i].2, srcs[j].0, srcs[j].2, if ratio && srcs[j].0 == "HL" { " as a ratio against its own median (an ETF against an index)" } else { "" });
                    match overall {
                        None => {
                            let _ = writeln!(details, "  no overlapping fresh data");
                        }
                        Some(o) => {
                            medians.insert((srcs[i].0, srcs[j].0), o.median_bp);
                            let _ = writeln!(details, "  overall n {}, median {} bp, p95 |x| {} bp, max |x| {} bp", o.n, bp(o.median_bp), bp(o.p95_abs_bp), bp(o.max_abs_bp));
                            let rows: Vec<String> = by.iter().filter(|(_, v)| v.n >= 30).map(|(h, v)| format!("{} n {} med {} p95 {} max {}", fmt_minute(h * 60), v.n, bp(v.median_bp), bp(v.p95_abs_bp), bp(v.max_abs_bp))).collect();
                            if !rows.is_empty() {
                                let _ = writeln!(details, "  per hour: {}", rows.join(" | "));
                            }
                        }
                    }
                }
            }
            match (medians.get(&("GMX", "HL")), medians.get(&("GMX", "REF")), medians.get(&("HL", "REF"))) {
                (Some(a), Some(b), Some(c)) => {
                    let _ = writeln!(details, "* Which source is furthest from the other two: {}", match outlier(*a, *b, *c) {
                        Outlier::Gmx => "GMX".to_string(),
                        Outlier::Hyperliquid => "Hyperliquid".to_string(),
                        Outlier::Reference => "the public reference".to_string(),
                        Outlier::Undecided(why) => format!("cannot decide ({why})"),
                    });
                }
                _ => {
                    let _ = writeln!(details, "* Which source is furthest from the other two: cannot decide, fewer than three sources overlap for this market.");
                }
            }
            let _ = writeln!(details);
        }
    }
    let _ = writeln!(s, "\n## Per market\n\n{details}");

    let _ = writeln!(s, "## Pyth published market hours (the prices need an API key; the hours do not)\n");
    let mut by_sym: BTreeMap<(String, String), Vec<&PythRow>> = BTreeMap::new();
    for r in &d.pyth {
        by_sym.entry((r.1.clone(), r.2.clone())).or_default().push(r);
    }
    let _ = writeln!(s, "| launch symbol | pyth symbol | transitions seen (is_open) | last state | next open | next close |");
    let _ = writeln!(s, "|---|---|---|---|---|---|");
    for ((launch, pyth), rows) in &by_sym {
        let mut rows = rows.clone();
        rows.sort_by_key(|r| r.0);
        let trans: Vec<String> = rows.iter().map(|r| format!("{} {}", fmt_utc(r.0), r.3)).collect();
        let l = rows.last().map(|r| (r.3.clone(), r.4.clone(), r.5.clone())).unwrap_or_default();
        let _ = writeln!(s, "| {launch} | {pyth} | {} | {} | {} | {} |", trans.join("; "), l.0, l.1, l.2);
    }

    let _ = writeln!(s, "\n## Data quality\n");
    let _ = writeln!(s, "| lane | requests | errors | reconnects | HTTP 429 | HTTP 403 |");
    let _ = writeln!(s, "|---|---|---|---|---|---|");
    for (lane, t) in &hb.lanes {
        let _ = writeln!(s, "| {lane} | {} | {} | {} | {} | {} |", t[4], t[0], t[1], t[2], t[3]);
    }
    let _ = writeln!(s, "\nHeartbeat rows: {}; recording downtime: {}.", hb.times.len(), if down.is_empty() { "none (no heartbeat gap above 120 s)".to_string() } else { down.iter().map(|d| format!("{} for {}", fmt_utc(d.0), fmt_dur(d.1 - d.0))).collect::<Vec<_>>().join("; ") });
    s
}

pub fn cli(args: &[String]) -> Result<(), String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let (mut out, mut launch) = (None, PathBuf::from("config/launch.json"));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().map(PathBuf::from),
            "--launch" => launch = it.next().map(PathBuf::from).ok_or("--launch needs a path")?,
            _ => dirs.push(PathBuf::from(a)),
        }
    }
    let out = out.ok_or("analyze needs --out <report.md>")?;
    if dirs.is_empty() {
        return Err("analyze needs at least one data directory".into());
    }
    let ms = markets::parse_launch(&std::fs::read_to_string(&launch).map_err(|e| format!("{}: {e}", launch.display()))?)?;
    let data = load(&dirs)?;
    std::fs::write(&out, render(&ms, &data)).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
