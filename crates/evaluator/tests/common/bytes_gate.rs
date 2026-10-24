//! P29b: the BYTE-EQUALITY GATE (handover/runtime-hosting-design.md 10.1). For every campaign path it hashes, in a fixed order and
//! with a fixed encoding (SHA-256; length-prefixed byte fields, big-endian integers, no map iteration, no clock), everything the
//! evaluator emits for the same inputs: each `Batch`, the pass-flow lines, the archive, the intents file and the final state.
//! A baseline is written once from the unchanged code (`handover/audit/p29-baseline.json`); every later change must reproduce it.
//!
//! A path digest is SHA-256("P29PATH" ++ batches ++ archive ++ intents ++ state), the four part digests in that order. The baseline
//! stores, per path, the first two bytes of each part digest (16 hex characters), so a mismatch names the part; one full digest per
//! 1,000 paths; and one overall digest (SHA-256 of all full path digests in path order).
#![allow(dead_code)]

use ::evaluator::{Batch, Evaluator, Stats, TxFormat};
use sha2::{Digest, Sha256};

use super::runner::{Item, RunResult};

/// Big-endian, length-prefixed encoder over a running SHA-256.
pub struct Enc(Sha256);

impl Enc {
    pub fn new(tag: &[u8]) -> Enc {
        let mut e = Enc(Sha256::new());
        e.bytes(tag);
        e
    }
    pub fn u8(&mut self, v: u8) {
        self.0.update([v]);
    }
    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    pub fn u16(&mut self, v: u16) {
        self.0.update(v.to_be_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.update(v.to_be_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.update(v.to_be_bytes());
    }
    pub fn i64(&mut self, v: i64) {
        self.0.update(v.to_be_bytes());
    }
    pub fn usize(&mut self, v: usize) {
        self.u64(v as u64);
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.u64(b.len() as u64);
        self.0.update(b);
    }
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    pub fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// An item of the file order, cloneable (the runner's `Item` is not).
#[derive(Clone)]
pub enum GateItem {
    Batch(Batch),
    Line(String),
}

/// Everything about the evaluator's final state that the gate hashes, as plain data (so the self-test can change one field).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateView {
    pub seq: u64,
    pub head: [u8; 32],
    pub phase: u8,
    pub trades_closed: u32,
    pub status: String,
    pub clock: i64,
    pub processed_through: i64,
    pub queued_events: usize,
    pub frozen: bool,
    pub ended: bool,
    /// size_micro, b, r, l_start, g_peak, l_level, q_tally, lock_until
    pub st_i64: [i64; 8],
    /// latch, tight_on, flat
    pub st_bool: [bool; 3],
    /// market, qty, avg_entry_e12, timer_start, opened_at, fee_day, sl, tp
    pub positions: Vec<(u16, i64, u64, i64, i64, i64, u64, u64)>,
    pub pending: Vec<(u32, [u8; 48])>,
    pub stats: Vec<u32>,
    pub refusals: Vec<(i64, u32, String)>,
}

pub fn stats_vec(s: &Stats) -> Vec<u32> {
    vec![
        s.breaches,
        s.static_fails,
        s.quick_fails,
        s.quick_trades,
        s.holding_fees,
        s.limit_fills,
        s.stop_fills,
        s.sl_triggers,
        s.tp_triggers,
        s.flips,
        s.tight_starts,
        s.tight_ends,
        s.floor_raises,
        s.closed_market_rejects,
        s.closed_with_position,
        s.refused_fills,
        s.unpostable,
        s.forced_after_overshoot,
        s.margin_exact,
        s.exec_refused,
        s.orders_accepted,
        s.orders_rejected,
        s.unpriced_rejects,
        s.samples_future,
        s.samples_stale,
        s.samples_out_of_order,
    ]
}

impl StateView {
    pub fn of(ev: &Evaluator) -> StateView {
        let st = ev.state();
        StateView {
            seq: ev.seq(),
            head: ev.head(),
            phase: ev.phase(),
            trades_closed: ev.trades_closed(),
            status: format!("{:?}", ev.status()),
            clock: ev.clock(),
            processed_through: ev.processed_through(),
            queued_events: ev.queued_events(),
            frozen: ev.is_frozen(),
            ended: ev.is_ended(),
            st_i64: [st.size_micro, st.b, st.r, st.l_start, st.g_peak, st.l_level, st.q_tally, st.lock_until],
            st_bool: [st.latch, st.tight_on, st.flat],
            positions: ev.positions().iter().map(|(m, p)| (*m, p.p.qty, p.p.avg_entry_e12, p.p.timer_start, p.p.opened_at, p.p.fee_day, p.sl, p.tp)).collect(),
            pending: ev.pending().iter().map(|p| (p.nonce, p.intent.to_bytes())).collect(),
            stats: stats_vec(ev.stats()),
            refusals: ev.refusals().iter().map(|r| (r.time, r.nonce, format!("{:?}", r.reason))).collect(),
        }
    }
}

/// One finished segment (a path, or the second half of a reset story) as plain data.
#[derive(Clone)]
pub struct SegmentView {
    pub items: Vec<GateItem>,
    pub archive: Vec<u8>,
    pub intents: Vec<u8>,
    pub state: StateView,
}

impl SegmentView {
    pub fn of(r: &RunResult) -> SegmentView {
        SegmentView {
            items: r.items.iter().map(|i| match i {
                Item::Batch(b) => GateItem::Batch(b.clone()),
                Item::Line(l) => GateItem::Line(l.clone()),
            }).collect(),
            archive: r.world.ev.archive_bytes().to_vec(),
            intents: r.world.ev.intents_bytes().to_vec(),
            state: StateView::of(&r.world.ev),
        }
    }
}

pub fn enc_batch(e: &mut Enc, b: &Batch) {
    e.u8(0xB0);
    match b.batch.to_bytes() {
        Ok(bytes) => {
            e.u8(1);
            e.bytes(&bytes);
        }
        Err(_) => e.u8(0),
    }
    // the kind by its Debug name (PostJournal, ReportFail(StaticFloor), ReportFail(PhaseQuick)): stable and unambiguous
    e.str(&format!("{:?}", b.kind));
    e.usize(b.markets.len());
    for m in &b.markets {
        e.u16(*m);
    }
    e.i64(b.meta_time);
    e.usize(b.samples_committed);
    e.usize(b.intents_committed_bytes);
    let a = &b.account_after;
    e.i64(a.b);
    e.i64(a.q_tally);
    e.i64(a.lock_until);
    e.i64(a.last_batch_time);
    e.u64(a.seq);
    e.bytes(&a.journal_hash);
    e.u32(a.trades_closed);
    e.bool(a.latch);
    e.bool(a.flat);
    e.bool(a.tight_on);
    e.i64(a.r);
    e.i64(a.g_peak);
    e.i64(a.l_level);
    e.bool(a.failed);
    e.usize(b.tx_bytes);
    e.u64(b.cu);
    e.u64(b.cu_limit);
    e.u8(match b.tx_format {
        TxFormat::Legacy => 0,
        TxFormat::V0Lookup => 1,
    });
}

pub fn enc_items(e: &mut Enc, items: &[GateItem]) {
    e.usize(items.len());
    for i in items {
        match i {
            GateItem::Batch(b) => enc_batch(e, b),
            GateItem::Line(l) => {
                e.u8(0x1E);
                e.str(l);
            }
        }
    }
}

pub fn enc_state(e: &mut Enc, s: &StateView) {
    e.u64(s.seq);
    e.bytes(&s.head);
    e.u8(s.phase);
    e.u32(s.trades_closed);
    e.str(&s.status);
    e.i64(s.clock);
    e.i64(s.processed_through);
    e.usize(s.queued_events);
    e.bool(s.frozen);
    e.bool(s.ended);
    for v in s.st_i64 {
        e.i64(v);
    }
    for v in s.st_bool {
        e.bool(v);
    }
    e.usize(s.positions.len());
    for (m, q, avg, ts, oa, fd, sl, tp) in &s.positions {
        e.u16(*m);
        e.i64(*q);
        e.u64(*avg);
        e.i64(*ts);
        e.i64(*oa);
        e.i64(*fd);
        e.u64(*sl);
        e.u64(*tp);
    }
    e.usize(s.pending.len());
    for (n, o) in &s.pending {
        e.u32(*n);
        e.bytes(o);
    }
    e.usize(s.stats.len());
    for v in &s.stats {
        e.u32(*v);
    }
    e.usize(s.refusals.len());
    for (t, n, r) in &s.refusals {
        e.i64(*t);
        e.u32(*n);
        e.str(r);
    }
}

/// The four part digests of a path, in the order batches, archive, intents, state.
pub type Parts = [[u8; 32]; 4];

/// Feeds the segments of one path (one, or two after a reset) into four running hashes.
pub struct PathHasher {
    batches: Enc,
    archive: Enc,
    intents: Enc,
    state: Enc,
}

impl PathHasher {
    pub fn new() -> PathHasher {
        PathHasher { batches: Enc::new(b"P29/batches"), archive: Enc::new(b"P29/archive"), intents: Enc::new(b"P29/intents"), state: Enc::new(b"P29/state") }
    }
    pub fn add(&mut self, s: &SegmentView) {
        enc_items(&mut self.batches, &s.items);
        self.archive.bytes(&s.archive);
        self.intents.bytes(&s.intents);
        enc_state(&mut self.state, &s.state);
    }
    pub fn finish(self) -> Parts {
        [self.batches.finish(), self.archive.finish(), self.intents.finish(), self.state.finish()]
    }
}

pub fn parts_of(segments: &[SegmentView]) -> Parts {
    let mut h = PathHasher::new();
    for s in segments {
        h.add(s);
    }
    h.finish()
}

/// SHA-256("P29PATH" ++ the four part digests).
pub fn path_digest(p: &Parts) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"P29PATH");
    for x in p {
        h.update(x);
    }
    h.finalize().into()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The 16-character per-path entry of the baseline: two bytes of each part digest.
pub fn path_entry(p: &Parts) -> String {
    p.iter().map(|x| hex(&x[..2])).collect()
}

pub const PART_NAMES: [&str; 4] = ["batches", "archive", "intents", "state"];
pub const CHUNK: usize = 1_000;

/// A finished campaign: the parts of every path in path order.
pub struct Campaign {
    pub name: String,
    pub params: serde_json::Value,
    pub parts: Vec<Parts>,
}

impl Campaign {
    pub fn chunk_digests(&self) -> Vec<String> {
        self.parts
            .chunks(CHUNK)
            .map(|c| {
                let mut h = Sha256::new();
                h.update(b"P29CHUNK");
                for p in c {
                    h.update(path_digest(p));
                }
                hex(&h.finalize())
            })
            .collect()
    }
    pub fn overall(&self) -> String {
        let mut h = Sha256::new();
        h.update(b"P29ALL");
        for p in &self.parts {
            h.update(path_digest(p));
        }
        hex(&h.finalize())
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "params": self.params,
            "paths": self.parts.len(),
            "overall": self.overall(),
            "chunk_size": CHUNK,
            "chunks": self.chunk_digests(),
            "path_digests": self.parts.iter().map(path_entry).collect::<Vec<_>>(),
        })
    }
}

fn run_cmd(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// Writes (or replaces) one campaign in the baseline file, keeping the others.
pub fn write_campaign(file: &str, c: &Campaign) {
    let mut root: serde_json::Value = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    let dirty_src = run_cmd("git", &["status", "--porcelain", "--", "programs", "crates/engine", "crates/journal-types", "crates/evaluator/src", "crates/verifier/src"]);
    root["format"] = "setl8-p29-bytes-gate".into();
    root["version"] = 1.into();
    root["commit"] = run_cmd("git", &["rev-parse", "HEAD"]).into();
    root["src_unchanged_since_commit"] = dirty_src.is_empty().into();
    root["rustc"] = run_cmd("rustc", &["--version"]).into();
    if root.get("campaigns").is_none() {
        root["campaigns"] = serde_json::json!({});
    }
    root["campaigns"][&c.name] = c.to_json();
    std::fs::write(file, serde_json::to_string_pretty(&root).unwrap_or_default() + "\n").expect("write the baseline");
}

/// Compares a fresh campaign with the baseline file. On a difference: the first differing path (campaign, path number, seed, part).
pub fn check_campaign(file: &str, c: &Campaign, first_seed: u64) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let root: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{file} is not JSON: {e}"))?;
    let base = root["campaigns"].get(&c.name).ok_or_else(|| format!("campaign {} is not in {file}", c.name))?;
    let want_paths = base["paths"].as_u64().unwrap_or(0) as usize;
    if want_paths != c.parts.len() {
        return Err(format!("campaign {}: {} paths in the baseline, {} in this run", c.name, want_paths, c.parts.len()));
    }
    let entries: Vec<&str> = base["path_digests"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
    for (i, p) in c.parts.iter().enumerate() {
        let got = path_entry(p);
        let want = entries.get(i).copied().unwrap_or("");
        if got != want {
            let part = (0..4).find(|k| got.get(k * 4..k * 4 + 4) != want.get(k * 4..k * 4 + 4)).map(|k| PART_NAMES[k]).unwrap_or("?");
            return Err(format!("campaign {}: first differing path number {} (seed {}), part {part}: baseline {want}, now {got}", c.name, i, first_seed + i as u64));
        }
    }
    let chunks = c.chunk_digests();
    let want_chunks: Vec<&str> = base["chunks"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
    for (k, ch) in chunks.iter().enumerate() {
        if want_chunks.get(k) != Some(&ch.as_str()) {
            return Err(format!("campaign {}: chunk {k} (paths {} to {}) differs although every path prefix matches", c.name, k * CHUNK, (k + 1) * CHUNK));
        }
    }
    if base["overall"].as_str() != Some(c.overall().as_str()) {
        return Err(format!("campaign {}: the overall digest differs although every path prefix and chunk matches", c.name));
    }
    Ok(())
}

/// Runs `f(i)` for i in 0..n on `threads` threads and returns the results in index order (deterministic whatever the schedule).
pub fn par_map<T: Send, F: Fn(usize) -> T + Sync>(n: usize, threads: usize, f: F) -> Vec<T> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|sc| {
        for _ in 0..threads.max(1) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let v = f(i);
                if let Ok(mut g) = out.lock() {
                    g[i] = Some(v);
                }
            });
        }
    });
    out.into_inner().unwrap_or_default().into_iter().flatten().collect()
}

pub fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// The baseline file, relative to the crate's manifest directory (`P29_FILE` overrides).
pub fn baseline_file(manifest_dir: &str, rel_to_root: &str) -> String {
    std::env::var("P29_FILE").unwrap_or_else(|_| format!("{manifest_dir}/{rel_to_root}/handover/audit/p29-baseline.json"))
}

/// Runs a campaign in the mode `P29_MODE` (`write` or `check`). Panics (non-zero exit) on a difference.
pub fn finish_campaign(file: &str, c: &Campaign, first_seed: u64) {
    match std::env::var("P29_MODE").as_deref() {
        Ok("write") => {
            write_campaign(file, c);
            println!("P29 {}: wrote {} paths, overall {}", c.name, c.parts.len(), c.overall());
        }
        _ => match check_campaign(file, c, first_seed) {
            Ok(()) => println!("P29 {}: {} paths equal the baseline, overall {}", c.name, c.parts.len(), c.overall()),
            Err(e) => panic!("P29 BYTE GATE FAILED: {e}"),
        },
    }
}
