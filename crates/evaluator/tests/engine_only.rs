//! The cheap campaign (no LiteSVM): random histories through the evaluator, then the public verifier's replay and an
//! independent re-derivation of pnl, credits, fees and floors. `ENGINE_PATHS` (default 2,000), `ENGINE_SEED` (default 1).
mod common;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use common::runner::*;
use common::*;
use evaluator::testkit::{generate, generate_wild};
use evaluator::*;
use journal_types::JournalEvent;

#[derive(Default, Debug, Clone, Copy)]
struct Cover {
    paths: u64,
    breach: u64,
    static_fail: u64,
    quick_fail: u64,
    quick_trade: u64,
    holding_night: u64,
    limit_fill: u64,
    sl: u64,
    tp: u64,
    flip: u64,
    tight_clean: u64,
    pass_finalized: u64,
    pass_reject_cancel: u64,
    closed_market: u64,
    verifier_invalid: u64,
}

impl Cover {
    fn add(&mut self, o: &Cover) {
        self.paths += o.paths;
        self.breach += o.breach;
        self.static_fail += o.static_fail;
        self.quick_fail += o.quick_fail;
        self.quick_trade += o.quick_trade;
        self.holding_night += o.holding_night;
        self.limit_fill += o.limit_fill;
        self.sl += o.sl;
        self.tp += o.tp;
        self.flip += o.flip;
        self.tight_clean += o.tight_clean;
        self.pass_finalized += o.pass_finalized;
        self.pass_reject_cancel += o.pass_reject_cancel;
        self.closed_market += o.closed_market;
        self.verifier_invalid += o.verifier_invalid;
    }
}

fn run_seed(seed: u64) -> Result<Cover, String> {
    let spec = if std::env::var("ENGINE_WILD").is_ok() { generate_wild(seed) } else { generate(seed) };
    let mut chain = SimChain::default();
    let r = run_path(&spec, world_for(&spec), &mut chain);
    let w = &r.world;
    if let Some(f) = w.ev.fault() {
        return Err(format!("evaluator fault {f:?} ({:?})", spec.scenario));
    }
    if !r.notes.is_empty() {
        return Err(format!("runner notes: {:?} ({:?})", r.notes, spec.scenario));
    }
    // the public verifier
    let failed = matches!(w.ev.status(), Status::Failed(_));
    let phase = w.ev.phase();
    let snap = snapshot_json(&w.ev, &batches_of(&r.items), phase, u8::from(failed));
    let rep = verify_items(w, &r.items, Some(&snap), None);
    if rep.verdict != setl8_verify::Verdict::Valid {
        return Err(format!("verifier MISMATCH ({:?}, {} batches, {:?}):\n{}", spec.scenario, r.batches_posted, w.ev.stats(), describe(&rep)));
    }
    // the independent re-derivation
    let batches = batches_of(&r.items);
    let samples: Vec<evaluator::codec::SampleRec> = (0..w.ev.sample_count()).filter_map(|i| w.ev.sample(i).map(|s| s.rec)).collect();
    // the reference works on one phase at a time; paths that changed phase are checked up to the phase change by the verifier only
    if r.pass.finalized == 0 && r.pass.confirmed == 0 {
        common::reference::check(w, &batches, &samples).map_err(|e| format!("reference: {e} ({:?})", spec.scenario))?;
    }
    // the SPLIT model is exact for every batch of every path: the size follows from the real bytes, the compute bound from the shape
    for b in &batches {
        let len = b.batch.to_bytes().expect("encodes").len();
        if b.tx_bytes != evaluator::batch::tx_size(b.kind, b.markets.len(), len, false) || b.cu != evaluator::batch::cu_estimate(b.kind, b.batch.events.len(), b.batch.proofs.len()) {
            return Err(format!("batch model off: {} bytes vs {len}-byte batch ({:?})", b.tx_bytes, spec.scenario));
        }
        if b.tx_bytes > 1_232 || b.cu > 200_000 || b.batch.events.len() > 8 || b.batch.proofs.len() > 3 {
            return Err(format!("batch above the chain's limits ({:?})", spec.scenario));
        }
    }
    let st = w.ev.stats();
    let evs = events(&r.items);
    let mut c = Cover { paths: 1, ..Default::default() };
    c.breach = u64::from(st.breaches > 0);
    c.static_fail = u64::from(st.static_fails > 0);
    c.quick_fail = u64::from(st.quick_fails > 0);
    c.quick_trade = u64::from(st.quick_trades > 0);
    c.holding_night = u64::from(st.holding_fees > 0);
    c.limit_fill = u64::from(st.limit_fills > 0 || st.stop_fills > 0);
    c.sl = u64::from(st.sl_triggers > 0);
    c.tp = u64::from(st.tp_triggers > 0);
    c.flip = u64::from(st.flips > 0);
    c.tight_clean = u64::from(st.tight_ends > 0 && st.breaches == 0);
    c.pass_finalized = u64::from(r.pass.finalized > 0);
    c.pass_reject_cancel = u64::from(r.pass.rejected + r.pass.cancelled > 0);
    c.closed_market = u64::from(st.closed_market_rejects > 0 || st.closed_with_position > 0);
    let _ = evs.iter().filter(|e| matches!(e, JournalEvent::Meta { .. })).count();
    if st.unpostable > 0 {
        return Err(format!("unpostable sequence ({:?})", spec.scenario));
    }
    if st.refused_fills > 0 {
        return Err(format!("a triggered pending order was refused ({:?})", spec.scenario));
    }
    Ok(c)
}

fn batches_of(items: &[Item]) -> Vec<Batch> {
    items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b.clone()) } else { None }).collect()
}

#[test]
fn engine_only_campaign() {
    let n: u64 = std::env::var("ENGINE_PATHS").ok().and_then(|v| v.parse().ok()).unwrap_or(2_000);
    let start: u64 = std::env::var("ENGINE_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let threads = std::env::var("ENGINE_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map(|x| x.get()).unwrap_or(4).min(16) as u64);
    let next = AtomicU64::new(0);
    let total = Mutex::new(Cover::default());
    let failures = Mutex::new(Vec::<(u64, String)>::new());
    let t0 = std::time::Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                let mut local = Cover::default();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let seed = start + i;
                    match std::panic::catch_unwind(|| run_seed(seed)) {
                        Ok(Ok(c)) => local.add(&c),
                        Ok(Err(e)) => failures.lock().unwrap().push((seed, e)),
                        Err(_) => failures.lock().unwrap().push((seed, "panic".into())),
                    }
                }
                total.lock().unwrap().add(&local);
            });
        }
    });
    let c = *total.lock().unwrap();
    let f = failures.lock().unwrap();
    println!("engine-only: {n} paths from seed {start} in {:.1}s on {threads} threads", t0.elapsed().as_secs_f64());
    println!("{c:#?}");
    if !f.is_empty() {
        let mut fs: Vec<_> = f.iter().collect();
        fs.sort_by_key(|x| x.0);
        // group by signature: the first finding class (or the first words of the error)
        let mut groups: std::collections::BTreeMap<String, (usize, u64)> = std::collections::BTreeMap::new();
        for (seed, e) in &fs {
            let sig = match e.lines().nth(1) {
                Some(l) if e.contains("MISMATCH") => l.split_whitespace().next().unwrap_or("?").to_string(),
                _ => e.split(['(', ':']).next().unwrap_or("?").trim().to_string(),
            };
            let g = groups.entry(sig).or_insert((0, *seed));
            g.0 += 1;
        }
        for (sig, (n, seed)) in &groups {
            println!("  {n:5} x {sig} (first seed {seed})");
        }
        for (seed, e) in fs.iter().take(3) {
            println!("FAILED seed {seed}: {e}\n   replay: ENGINE_PATHS=1 ENGINE_SEED={seed} cargo test --offline -p setl8-lev-evaluator --test engine_only -- --nocapture");
        }
        panic!("{} of {n} paths failed (first seed {})", fs.len(), fs[0].0);
    }
}
