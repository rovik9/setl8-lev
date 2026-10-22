//! M10 differential campaign: seeded random account histories through the reference evaluator, every batch posted through
//! the REAL program and vault in LiteSVM; after EVERY transaction the stored `TradingAccount` (all 207 bytes) must equal the
//! evaluator's state; an honest batch is never rejected; at the end of each path the public verifier replays the emitted
//! EventLogs with the archive and the intents and must say VALID.
//!
//! `DIFF_PATHS=N` (default 300) and `DIFF_SEED=S` (default 1) select the paths. A failure prints the seed and a replay command.
//! The gate run is `DIFF_PATHS=10000`; it also asserts the coverage floors below.
mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use common::evaluator::shared::runner::{completed_a_pass, fail8_commission_of, m9b_cover, m9c_cover, run_path, spec_rules, M9bCover, M9cCover, ResetCover, RunResult};
use common::evaluator::shared::World;
use common::evaluator::*;
use common::launch_ruleset_args;
use common::vault::RESET_LADDER_BPS;
use ::evaluator::testkit::{generate, generate_after_reset, reset_story, PathSpec};
use ::evaluator::Status;
use solana_signer::Signer;

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
    transactions: u64,
    passes_vetoed: u64,
    graduated: u64,
    m9b: M9bCover,
    resets: ResetCover,
    m9c: M9cCover,
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
        self.transactions += o.transactions;
        self.passes_vetoed += o.passes_vetoed;
        self.graduated += o.graduated;
        self.m9b.add(&o.m9b);
        self.resets.add(&o.resets);
        self.m9c.add(&o.m9c);
    }
}

struct Outcome {
    cover: Cover,
    shapes: BTreeMap<String, ShapeMax>,
}

/// What one finished segment counts (`transactions` is set by the caller: the chain counts both segments of a reset story).
fn cover_of(spec: &PathSpec, chain: &LiteChain, r: &RunResult) -> Cover {
    let w = &r.world;
    let st = w.ev.stats();
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
    c.passes_vetoed = u64::from(r.pass.vetoed > 0);
    c.graduated = u64::from(r.pass.graduated);
    c.m9b = m9b_cover(spec, w, &r.items, &chain.lines);
    c.m9c = m9c_cover(w);
    c.m9c.fail8_commission = fail8_commission_of(&r.items);
    c
}

/// ceil(tier cost x ladder[phase - 1] / 10_000), computed from the launch table independently of the program's helper.
fn expected_reset_price(tier: usize, phase: u8) -> u64 {
    let cost = u128::from(launch_ruleset_args().tier_cost[tier]);
    let bps = u128::from(RESET_LADDER_BPS[usize::from(phase) - 1]);
    u64::try_from((cost * bps).div_ceil(10_000)).unwrap()
}

fn run_seed(seed: u64) -> Result<Outcome, String> {
    let spec = generate(seed);
    let mut chain = LiteChain::new(&spec);
    let world = World::from_params(spec_rules(&spec), chain.params.clone());
    let r = run_path(&spec, world, &mut chain);
    verify_segment(&spec, &chain, &r)?;
    let mut c = cover_of(&spec, &chain, &r);
    // M8: the failed account goes through the REAL `reset_account` and the story goes on in the new account; both accounts
    // are compared byte for byte after every transaction and verified at the end of their own segment
    if reset_story(&spec) && matches!(r.world.ev.status(), Status::Failed(_)) {
        let failed_phase = r.world.ev.phase();
        let tk = chain.j.trader.pubkey();
        let tt = chain.j.v.trader_token(&tk);
        let bal0 = chain.j.v.token_balance(&tt);
        let (params2, ev) = chain.reset(chain.clock.max(spec.end) + 1_000);
        let price = expected_reset_price(spec.tier, failed_phase);
        let fee = u64::try_from((u128::from(price) * 40).div_ceil(10_000)).unwrap();
        if (ev.price, ev.fee, ev.phase) != (price, fee, failed_phase) || bal0 - chain.j.v.token_balance(&tt) != price + fee {
            return Err(format!("reset of tier {} phase {failed_phase}: event price {} fee {}, trader paid {}, want {price} + {fee}", spec.tier, ev.price, ev.fee, bal0 - chain.j.v.token_balance(&tt)));
        }
        let spec2 = generate_after_reset(&spec, params2.opened_time);
        let world2 = World::from_params(spec_rules(&spec2), params2);
        let r2 = run_path(&spec2, world2, &mut chain);
        verify_segment(&spec2, &chain, &r2).map_err(|e| format!("after the reset (second segment {:?}, phase {failed_phase}): {e}", spec2.scenario))?;
        c.add(&cover_of(&spec2, &chain, &r2));
        c.paths = 1;
        c.resets = ResetCover::one(failed_phase, completed_a_pass(&r2, failed_phase));
    }
    c.transactions = chain.txs;
    Ok(Outcome { cover: c, shapes: chain.shapes })
}

/// Coverage floors of the gate run (10,000 paths); scaled with the number of paths when fewer are run (never below 1).
fn floors(n: u64) -> Vec<(&'static str, u64)> {
    let scale = |x: u64| ((x * n).div_ceil(10_000)).max(1);
    vec![
        ("breach", scale(500)),
        ("static_fail", scale(300)),
        ("quick_fail", scale(200)),
        ("quick_trade", scale(500)),
        ("holding_night", scale(500)),
        ("limit_fill", scale(300)),
        ("sl", scale(300)),
        ("tp", scale(300)),
        ("flip", scale(200)),
        ("tight_clean", scale(200)),
        ("pass_finalized", scale(150)),
        ("pass_reject_cancel", scale(100)),
        ("closed_market", scale(100)),
    ]
}

fn value_of(c: &Cover, name: &str) -> u64 {
    match name {
        "breach" => c.breach,
        "static_fail" => c.static_fail,
        "quick_fail" => c.quick_fail,
        "quick_trade" => c.quick_trade,
        "holding_night" => c.holding_night,
        "limit_fill" => c.limit_fill,
        "sl" => c.sl,
        "tp" => c.tp,
        "flip" => c.flip,
        "tight_clean" => c.tight_clean,
        "pass_finalized" => c.pass_finalized,
        "pass_reject_cancel" => c.pass_reject_cancel,
        "closed_market" => c.closed_market,
        _ => 0,
    }
}

#[test]
fn differential_campaign() {
    let n: u64 = std::env::var("DIFF_PATHS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let start: u64 = std::env::var("DIFF_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let threads = std::env::var("DIFF_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map(|x| x.get()).unwrap_or(4).min(12) as u64);
    let next = AtomicU64::new(0);
    let total = Mutex::new(Cover::default());
    let shapes = Mutex::new(BTreeMap::<String, ShapeMax>::new());
    let failures = Mutex::new(Vec::<(u64, String)>::new());
    let t0 = std::time::Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                let mut local = Cover::default();
                let mut local_shapes: BTreeMap<String, ShapeMax> = BTreeMap::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let seed = start + i;
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_seed(seed))) {
                        Ok(Ok(o)) => {
                            local.add(&o.cover);
                            for (k, v) in o.shapes {
                                let e = local_shapes.entry(k).or_default();
                                e.count += v.count;
                                e.max_bytes = e.max_bytes.max(v.max_bytes);
                                e.max_cu = e.max_cu.max(v.max_cu);
                                e.model_bytes_max = e.model_bytes_max.max(v.model_bytes_max);
                                e.model_cu_max = e.model_cu_max.max(v.model_cu_max);
                            }
                        }
                        Ok(Err(e)) => failures.lock().unwrap().push((seed, e)),
                        Err(p) => {
                            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panic".into());
                            failures.lock().unwrap().push((seed, format!("panic: {msg}")));
                        }
                    }
                }
                total.lock().unwrap().add(&local);
                let mut g = shapes.lock().unwrap();
                for (k, v) in local_shapes {
                    let e = g.entry(k).or_default();
                    e.count += v.count;
                    e.max_bytes = e.max_bytes.max(v.max_bytes);
                    e.max_cu = e.max_cu.max(v.max_cu);
                    e.model_bytes_max = e.model_bytes_max.max(v.model_bytes_max);
                    e.model_cu_max = e.model_cu_max.max(v.model_cu_max);
                }
            });
        }
    });
    let c = *total.lock().unwrap();
    let f = failures.lock().unwrap();
    let secs = t0.elapsed().as_secs_f64();
    println!("differential: {n} paths from seed {start} in {secs:.1}s on {threads} threads; {} transactions", c.transactions);
    println!("{c:#?}");
    println!("batch shapes (max signed transaction bytes / compute units seen; model bound):");
    for (k, v) in shapes.lock().unwrap().iter() {
        println!("  {k}: n={} max {} B / {} CU (model {} B / {} CU)", v.count, v.max_bytes, v.max_cu, v.model_bytes_max, v.model_cu_max);
    }
    if !f.is_empty() {
        let mut fs: Vec<_> = f.iter().collect();
        fs.sort_by_key(|x| x.0);
        for (seed, e) in fs.iter().take(5) {
            println!("FAILED seed {seed}: {e}\n   replay: DIFF_PATHS=1 DIFF_SEED={seed} cargo test --offline --test differential -- --nocapture");
        }
        panic!("{} of {n} paths failed (first seed {})", fs.len(), fs[0].0);
    }
    if n >= 2_000 {
        let mut short = Vec::new();
        for (name, floor) in floors(n) {
            let v = value_of(&c, name);
            println!("coverage {name}: {v} (floor {floor})");
            if v < floor {
                short.push(format!("{name} {v} < {floor}"));
            }
        }
        for (name, floor) in M9bCover::floors(n) {
            let v = c.m9b.value_of(name);
            println!("coverage {name}: {v} (floor {floor})");
            if v < floor {
                short.push(format!("{name} {v} < {floor}"));
            }
        }
        for (name, floor) in M9cCover::floors(n) {
            let v = c.m9c.value_of(name);
            println!("coverage {name}: {v} (floor {floor})");
            if v < floor {
                short.push(format!("{name} {v} < {floor}"));
            }
        }
        // P25b (audit F-16): every forced-close sequence the evaluator emitted is one the program accepts
        println!("coverage unpostable: {} (must be 0); samples refused: future {}, stale {}, out of order {}", c.m9c.unpostable, c.m9c.samples_future, c.m9c.samples_stale, c.m9c.samples_out_of_order);
        assert_eq!(c.m9c.unpostable, 0, "Stats::unpostable must stay 0 over the campaign");
        for (name, floor) in ResetCover::floors(n) {
            let v = c.resets.value_of(name);
            println!("coverage {name}: {v} (floor {floor})");
            if v < floor {
                short.push(format!("{name} {v} < {floor}"));
            }
        }
        assert!(short.is_empty(), "under-exercised classes: {short:?}");
    }
}
