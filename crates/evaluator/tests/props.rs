//! Property tests: batching does not change what happens (only how it is cut into transactions), and everything is a
//! pure function of the inputs.
//!
//! Note on what "the same" means: every batch carries its own `Meta`, so two batching policies give DIFFERENT hash chains
//! and sequence numbers. What must be identical is the account state (`b`, `q_tally`, tight state, lock, trades, flat), the
//! book, the archive and intents bytes, and the CONTENT of the events (all events except the `Meta`s, with their proofs).
mod common;
use common::runner::*;
use common::*;
use evaluator::testkit::{generate, Step};
use evaluator::*;
use journal_types::JournalEvent;
use proptest::prelude::*;

fn has_pass(seed: u64) -> bool {
    generate(seed).steps.iter().any(|s| matches!(s, Step::Pass { .. }))
}

/// The events without their `Meta`s and with the proof resolved (batch-local proof indexes are not content).
fn content(items: &[Item]) -> Vec<(JournalEvent, Vec<u8>)> {
    let mut out = Vec::new();
    for it in items {
        if let Item::Batch(b) = it {
            for ev in &b.batch.events {
                if matches!(ev, JournalEvent::Meta { .. }) {
                    continue;
                }
                let proof = ev.proof_index().and_then(|i| b.batch.proofs.get(i)).map(|p| p.to_bytes().unwrap()).unwrap_or_default();
                let mut e = ev.clone();
                match &mut e {
                    JournalEvent::Fill { proof, .. }
                    | JournalEvent::Close { proof, .. }
                    | JournalEvent::TightStart { proof, .. }
                    | JournalEvent::FloorRaised { proof, .. }
                    | JournalEvent::Breach { proof, .. }
                    | JournalEvent::Fail { proof, .. } => *proof = 0,
                    _ => {}
                }
                out.push((e, proof));
            }
        }
    }
    out
}

fn run_with(seed: u64, max_events: usize) -> RunResult {
    let mut spec = generate(seed);
    spec.batch_max_events = max_events;
    let mut chain = SimChain::default();
    run_path(&spec, world_for(&spec), &mut chain)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(120))]

    /// Two batching policies, one history: the same state, the same events, the same files.
    #[test]
    fn batching_policy_does_not_change_the_outcome(seed in 0u64..200_000, e1 in 2usize..=8, e2 in 2usize..=8) {
        prop_assume!(!has_pass(seed));
        let a = run_with(seed, e1);
        let b = run_with(seed, e2);
        prop_assert_eq!(a.world.ev.state(), b.world.ev.state());
        prop_assert_eq!(a.world.ev.trades_closed(), b.world.ev.trades_closed());
        prop_assert_eq!(a.world.ev.positions(), b.world.ev.positions());
        prop_assert_eq!(a.world.ev.pending(), b.world.ev.pending());
        prop_assert_eq!(a.world.ev.archive_bytes(), b.world.ev.archive_bytes());
        prop_assert_eq!(a.world.ev.intents_bytes(), b.world.ev.intents_bytes());
        prop_assert_eq!(content(&a.items), content(&b.items));
        prop_assert_eq!(a.world.ev.status(), b.world.ev.status());
        // and the verifier is as happy with one as with the other
        let ba: Vec<Batch> = a.items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b.clone()) } else { None }).collect();
        let bb: Vec<Batch> = b.items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b.clone()) } else { None }).collect();
        prop_assert!(verify(&a.world, &ba).verdict == setl8_verify::Verdict::Valid);
        prop_assert!(verify(&b.world, &bb).verdict == setl8_verify::Verdict::Valid);
    }

    /// A history is a pure function of its seed.
    #[test]
    fn a_seed_gives_the_same_bytes_twice(seed in 0u64..200_000) {
        let a = run_with(seed, 8);
        let b = run_with(seed, 8);
        let bytes = |r: &RunResult| -> Vec<Vec<u8>> { r.items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b.batch.to_bytes().unwrap()) } else { None }).collect() };
        prop_assert_eq!(bytes(&a), bytes(&b));
        prop_assert_eq!(a.world.ev.head(), b.world.ev.head());
    }

    /// Whatever the policy, every batch obeys the chain's limits and ends with exactly one Meta whose time never goes back.
    #[test]
    fn every_batch_obeys_the_limits(seed in 0u64..200_000, e in 2usize..=8) {
        let r = run_with(seed, e);
        let mut last_meta = i64::MIN;
        let mut last_event = i64::MIN;
        for it in &r.items {
            if let Item::Batch(b) = it {
                prop_assert!(b.batch.events.len() <= 8 && b.batch.proofs.len() <= 3 && b.tx_bytes <= 1_232 && b.cu <= 200_000);
                let ends_with_meta = matches!(b.batch.events.last(), Some(JournalEvent::Meta { .. }));
                prop_assert!(ends_with_meta);
                for ev in &b.batch.events {
                    let t = ev.time();
                    if !matches!(ev, JournalEvent::Meta { .. }) {
                        prop_assert!(t >= last_event, "event times never go back: {} after {}", t, last_event);
                        last_event = t;
                    }
                }
                prop_assert!(b.meta_time >= last_meta && b.meta_time >= last_event);
                last_meta = b.meta_time;
            }
        }
    }
}
