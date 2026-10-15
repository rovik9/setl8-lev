//! `DEBUG_SEED=n cargo test -p setl8-lev-evaluator --test debug_seed -- --nocapture` prints one history and its journal.
mod common;
use common::runner::*;
use common::*;
use evaluator::testkit::{generate, Step};

#[test]
fn debug_seed() {
    let Some(seed) = std::env::var("DEBUG_SEED").ok().and_then(|v| v.parse::<u64>().ok()) else { return };
    let spec = generate(seed);
    println!("scenario {:?} tier {} phase {} markets {:?} commission {:?} batch_max {}", spec.scenario, spec.tier, spec.phase, spec.markets.iter().map(|m| (m.id, m.class, m.start_price)).collect::<Vec<_>>(), spec.commission_e6, spec.batch_max_events);
    if std::env::var("SHOW_STEPS").is_ok() {
        for s in &spec.steps {
            match s {
                Step::Sample { .. } => {
                    if std::env::var("SHOW_SAMPLES").is_ok() {
                        println!("  {s:?}");
                    }
                }
                _ => println!("  {s:?}"),
            }
        }
    }
    let mut chain = SimChain::default();
    let r = run_path(&spec, world_for(&spec), &mut chain);
    let mut seq = 0;
    for it in &r.items {
        match it {
            Item::Batch(b) => {
                println!("-- batch kind {:?} meta_time {} tx {}B cu {} markets {:?}", b.kind, b.meta_time, b.tx_bytes, b.cu, b.markets);
                for (i, e) in b.batch.events.iter().enumerate() {
                    println!("   #{} {:?}", b.batch.first_seq + i as u64, e);
                    seq += 1;
                }
            }
            Item::Line(l) => println!("-- line {l}"),
        }
    }
    println!("stats {:?}", r.world.ev.stats());
    println!("state {:?}  positions {:?} pending {:?}", r.world.ev.state(), r.world.ev.positions(), r.world.ev.pending());
    let _ = seq;
    let rep = verify_items(&r.world, &r.items, None, None);
    println!("verdict {:?}\n{}", rep.verdict, describe(&rep));
}
