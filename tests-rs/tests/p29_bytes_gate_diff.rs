//! P29b: the differential twin of the byte-equality gate. The same hash (`crates/evaluator/tests/common/bytes_gate.rs`) over the
//! differential campaign's paths, which post EVERY batch through the real program and vault in LiteSVM, so a baseline equal means the
//! program also took the same batches. `#[ignore]`: `scripts/p29-bytes-gate.sh` runs it.
//! Environment: `P29_NAME`, `P29_PATHS` (default 300), `P29_SEED` (default 1), `P29_MODE` (`check` or `write`), `P29_FILE`, `P29_THREADS`.
mod common;

use common::evaluator::shared::bytes_gate::*;
use common::evaluator::shared::runner::{run_path, spec_rules};
use common::evaluator::shared::World;
use common::evaluator::*;
use ::evaluator::testkit::{generate, generate_after_reset, reset_story};
use ::evaluator::Status;

fn segments_of(seed: u64) -> Result<Vec<SegmentView>, String> {
    let spec = generate(seed);
    let mut chain = LiteChain::new(&spec);
    let world = World::from_params(spec_rules(&spec), chain.params.clone());
    let r = run_path(&spec, world, &mut chain);
    if !r.notes.is_empty() || r.world.ev.fault().is_some() {
        return Err(format!("seed {seed}: notes {:?} fault {:?}", r.notes, r.world.ev.fault()));
    }
    let mut out = vec![SegmentView::of(&r)];
    if reset_story(&spec) && matches!(r.world.ev.status(), Status::Failed(_)) {
        let (params2, _ev) = chain.reset(chain.clock.max(spec.end) + 1_000);
        let spec2 = generate_after_reset(&spec, params2.opened_time);
        let world2 = World::from_params(spec_rules(&spec2), params2);
        let r2 = run_path(&spec2, world2, &mut chain);
        if !r2.notes.is_empty() || r2.world.ev.fault().is_some() {
            return Err(format!("seed {seed} (second segment): notes {:?}", r2.notes));
        }
        out.push(SegmentView::of(&r2));
    }
    Ok(out)
}

#[test]
#[ignore]
fn p29_bytes_gate_differential() {
    let name = std::env::var("P29_NAME").unwrap_or_else(|_| "differential_default".into());
    let n = env_u64("P29_PATHS", 300) as usize;
    let seed = env_u64("P29_SEED", 1);
    let threads = env_u64("P29_THREADS", std::thread::available_parallelism().map(|x| x.get() as u64).unwrap_or(4).min(12)) as usize;
    let t0 = std::time::Instant::now();
    let results = par_map(n, threads, |i| segments_of(seed + i as u64).map(|s| parts_of(&s)));
    let mut parts = Vec::with_capacity(n);
    for (i, r) in results.into_iter().enumerate() {
        match r {
            Ok(p) => parts.push(p),
            Err(e) => panic!("path {i}: {e}"),
        }
    }
    let params = serde_json::json!({"harness": "differential (tests-rs/tests/p29_bytes_gate_diff.rs), every batch posted through the real program in LiteSVM",
        "paths": n, "first_seed": seed, "launch_costs": ::evaluator::testkit::launch_costs_requested(), "generator": "testkit::generate + reset story"});
    let c = Campaign { name, params, parts };
    println!("P29: {} paths hashed in {:.1}s on {threads} threads", c.parts.len(), t0.elapsed().as_secs_f64());
    finish_campaign(&baseline_file(env!("CARGO_MANIFEST_DIR"), ".."), &c, seed);
}
