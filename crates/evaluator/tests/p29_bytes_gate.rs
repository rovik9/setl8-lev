//! P29b: the byte-equality gate over the engine-only campaign (handover/runtime-hosting-design.md 10.1), its self-test and its
//! determinism test. The campaign test is `#[ignore]` (it needs a baseline); `scripts/p29-bytes-gate.sh` runs it.
//!
//! Environment of the campaign test: `P29_NAME` (campaign key in the baseline), `P29_PATHS` (default 2,000), `P29_SEED` (default 1),
//! `P29_MODE` (`check`, the default, or `write`), `P29_FILE` (the baseline file), `P29_THREADS`. `SETL8_LAUNCH_COSTS` selects the
//! owner-cost gate exactly as `engine_only.rs` does.
mod common;
use common::bytes_gate::*;
use common::runner::*;
use common::*;
use evaluator::testkit::{generate, generate_after_reset, launch_costs_requested, reset_story, PathSpec};
use evaluator::*;

/// One path as the engine-only campaign runs it (`engine_only.rs::run_seed`), reduced to what the gate hashes.
fn segments_of(seed: u64) -> Vec<SegmentView> {
    let spec = generate(seed);
    segments_of_spec(&spec)
}

fn segments_of_spec(spec: &PathSpec) -> Vec<SegmentView> {
    let mut chain = SimChain::default();
    let r = run_path(spec, world_for(spec), &mut chain);
    assert!(r.world.ev.fault().is_none(), "evaluator fault in seed path {:?}", spec.scenario);
    let mut out = vec![SegmentView::of(&r)];
    if reset_story(spec) && matches!(r.world.ev.status(), Status::Failed(_)) {
        let failed_phase = r.world.ev.phase();
        let reset_time = spec.end + 1_000;
        let spec2 = generate_after_reset(spec, reset_time);
        let params2 = reset_params(&r.world.opened, failed_phase, reset_time);
        let world2 = World::from_params(spec_rules(&spec2), params2);
        let mut chain2 = SimChain::default();
        let r2 = run_path(&spec2, world2, &mut chain2);
        assert!(r2.world.ev.fault().is_none(), "evaluator fault in the second segment");
        out.push(SegmentView::of(&r2));
    }
    out
}

#[test]
#[ignore]
fn p29_bytes_gate_campaign() {
    let name = std::env::var("P29_NAME").unwrap_or_else(|_| "engine_default".into());
    let n = env_u64("P29_PATHS", 2_000) as usize;
    let seed = env_u64("P29_SEED", 1);
    let threads = env_u64("P29_THREADS", std::thread::available_parallelism().map(|x| x.get() as u64).unwrap_or(4).min(16)) as usize;
    let t0 = std::time::Instant::now();
    let parts = par_map(n, threads, |i| parts_of(&segments_of(seed + i as u64)));
    let params = serde_json::json!({"harness": "engine-only (crates/evaluator/tests/p29_bytes_gate.rs)", "paths": n, "first_seed": seed,
        "launch_costs": launch_costs_requested(), "generator": "testkit::generate + reset story", "encoding": "see tests/common/bytes_gate.rs"});
    let c = Campaign { name, params, parts };
    println!("P29: {} paths hashed in {:.1}s on {threads} threads", c.parts.len(), t0.elapsed().as_secs_f64());
    finish_campaign(&baseline_file(env!("CARGO_MANIFEST_DIR"), "../.."), &c, seed);
}

// ------------------------------------------------------------------------------------------------ self-test

fn sample_segment() -> SegmentView {
    for seed in 1..200 {
        let segs = segments_of(seed);
        let s = &segs[0];
        if s.items.iter().any(|i| matches!(i, GateItem::Batch(_))) && !s.intents.is_empty() && s.archive.len() > 100 && s.state.stats.iter().any(|v| *v > 0) {
            return s.clone();
        }
    }
    panic!("no usable path in seeds 1..200");
}

fn digest(s: &SegmentView) -> [u8; 32] {
    path_digest(&parts_of(std::slice::from_ref(s)))
}

fn first_batch(s: &mut SegmentView) -> &mut Batch {
    s.items.iter_mut().find_map(|i| if let GateItem::Batch(b) = i { Some(b) } else { None }).expect("a batch")
}

#[test]
fn the_gate_changes_when_one_byte_of_one_field_changes() {
    let base = sample_segment();
    let d0 = digest(&base);
    assert_eq!(d0, digest(&base.clone()), "unchanged data, unchanged digest");
    let mut cases: Vec<(&str, SegmentView)> = Vec::new();
    // one byte of the journal batch itself (prev_hash), one field of the batch metadata each
    macro_rules! batch_case {
        ($name:expr, $f:expr) => {{
            let mut s = base.clone();
            #[allow(clippy::redundant_closure_call)]
            ($f)(first_batch(&mut s));
            cases.push(($name, s));
        }};
    }
    batch_case!("batch bytes: prev_hash[0]", |b: &mut Batch| b.batch.prev_hash[0] ^= 1);
    batch_case!("batch bytes: first_seq", |b: &mut Batch| b.batch.first_seq += 1);
    batch_case!("batch.kind", |b: &mut Batch| b.kind = BatchKind::ReportFail(engine::FailReason::PhaseQuick));
    batch_case!("batch.markets", |b: &mut Batch| b.markets.push(999));
    batch_case!("batch.meta_time", |b: &mut Batch| b.meta_time += 1);
    batch_case!("batch.samples_committed", |b: &mut Batch| b.samples_committed += 1);
    batch_case!("batch.intents_committed_bytes", |b: &mut Batch| b.intents_committed_bytes += 1);
    batch_case!("account_after.b", |b: &mut Batch| b.account_after.b += 1);
    batch_case!("account_after.journal_hash[31]", |b: &mut Batch| b.account_after.journal_hash[31] ^= 0x80);
    batch_case!("account_after.failed", |b: &mut Batch| b.account_after.failed = !b.account_after.failed);
    batch_case!("batch.tx_bytes", |b: &mut Batch| b.tx_bytes += 1);
    batch_case!("batch.cu", |b: &mut Batch| b.cu += 1);
    batch_case!("batch.cu_limit", |b: &mut Batch| b.cu_limit += 1);
    batch_case!("batch.tx_format", |b: &mut Batch| b.tx_format = if b.tx_format == TxFormat::Legacy { TxFormat::V0Lookup } else { TxFormat::Legacy });
    // the archive, the intents file
    let mut s = base.clone();
    let mid = s.archive.len() / 2;
    s.archive[mid] ^= 1;
    cases.push(("archive: one byte", s));
    let mut s = base.clone();
    s.archive.push(0);
    cases.push(("archive: one byte appended", s));
    let mut s = base.clone();
    let mid = s.intents.len() / 2;
    s.intents[mid] ^= 1;
    cases.push(("intents: one byte", s));
    // one counter, one state field, one refusal
    let mut s = base.clone();
    s.state.stats[0] += 1;
    cases.push(("stats: one counter", s));
    let mut s = base.clone();
    s.state.st_i64[1] += 1;
    cases.push(("state: b", s));
    let mut s = base.clone();
    s.state.head[0] ^= 1;
    cases.push(("state: head", s));
    let mut s = base.clone();
    s.state.refusals.push((1, 2, "NoFreshPrice".into()));
    cases.push(("refusals: one more", s));
    // a dropped item
    let mut s = base.clone();
    s.items.pop();
    cases.push(("items: one dropped", s));
    for (name, s) in &cases {
        assert_ne!(digest(s), d0, "the gate did not notice: {name}");
    }
    // and the part that changed is the part that is named
    let mut s = base.clone();
    let mid = s.archive.len() / 2;
    s.archive[mid] ^= 1;
    let (p0, p1) = (parts_of(std::slice::from_ref(&base)), parts_of(std::slice::from_ref(&s)));
    assert_eq!([p0[0] == p1[0], p0[1] == p1[1], p0[2] == p1[2], p0[3] == p1[3]], [true, false, true, true]);
    println!("P29 self-test: {} one-field changes, every one changed the digest", cases.len());
}

#[test]
fn the_check_names_the_first_differing_path_and_part() {
    let a: Vec<Parts> = (1..=5u64).map(|s| parts_of(&segments_of(s))).collect();
    let c = Campaign { name: "selftest".into(), params: serde_json::json!({}), parts: a.clone() };
    let file = format!("{}/p29_selftest_{}.json", std::env::temp_dir().display(), std::process::id());
    write_campaign(&file, &c);
    assert!(check_campaign(&file, &c, 1).is_ok());
    let mut b = a;
    b[3][2][0] ^= 1; // path number 3 (seed 4), part "intents"
    let c2 = Campaign { name: "selftest".into(), params: serde_json::json!({}), parts: b };
    let e = check_campaign(&file, &c2, 1).unwrap_err();
    let _ = std::fs::remove_file(&file);
    assert!(e.contains("path number 3") && e.contains("seed 4") && e.contains("part intents"), "{e}");
}

// ------------------------------------------------------------------------------------------------ determinism

fn digest_of_seeds() -> String {
    let parts: Vec<Parts> = (1..=40u64).map(|s| parts_of(&segments_of(s))).collect();
    Campaign { name: "det".into(), params: serde_json::json!({}), parts }.overall()
}

#[test]
fn two_runs_in_one_process_give_the_same_digest() {
    assert_eq!(digest_of_seeds(), digest_of_seeds());
    // and the same on another thread count
    let seq: Vec<Parts> = par_map(40, 1, |i| parts_of(&segments_of(1 + i as u64)));
    let par: Vec<Parts> = par_map(40, 8, |i| parts_of(&segments_of(1 + i as u64)));
    assert!(seq == par, "the digests depend on the thread schedule");
}

/// Printed by a child process (see the next test); does nothing in a normal run.
#[test]
fn p29_digest_probe() {
    if std::env::var_os("P29_PROBE").is_some() {
        println!("P29_PROBE_DIGEST={}", digest_of_seeds());
    }
}

#[test]
fn two_runs_in_separate_processes_give_the_same_digest() {
    let mine = digest_of_seeds();
    let exe = std::env::current_exe().expect("the test binary");
    let mut seen = Vec::new();
    for _ in 0..2 {
        let out = std::process::Command::new(&exe).args(["p29_digest_probe", "--exact", "--nocapture", "--test-threads=1"]).env("P29_PROBE", "1").output().expect("child run");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let d = text.split("P29_PROBE_DIGEST=").nth(1).and_then(|r| r.split_whitespace().next()).unwrap_or_else(|| panic!("no digest in the child output:\n{text}")).to_string();
        seen.push(d);
    }
    assert_eq!(seen[0], seen[1], "two child processes differ");
    assert_eq!(seen[0], mine, "a child process differs from this one");
}
