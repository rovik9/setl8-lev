//! P27 (decision 71) on the generator's own `CommissionFail` story: three long positions in three class-0 markets with a commission
//! slide together through the static floor, so the failing batch is the heaviest legal `report_fail` the evaluator emits with fees: three
//! closes, their three commission fees, the `Fail` and the `Meta` (8 events, 3 proofs). The honest history is VALID.
mod common;
use common::runner::*;
use common::*;
use evaluator::testkit::{generate, Scenario};
use evaluator::{BatchKind, Status};
use journal_types::JournalEvent;

/// The first `n` seeds (from 1) whose story is `scenario`.
fn seeds_of(scenario: Scenario, n: usize) -> Vec<u64> {
    (1..100_000u64).filter(|s| generate(*s).scenario == scenario).take(n).collect()
}

#[test]
fn three_closes_with_their_fees_fail_the_account_at_the_static_floor() {
    let mut eight = 0;
    for seed in seeds_of(Scenario::CommissionFail, 40) {
        let spec = generate(seed);
        // three markets, all class 0, and a commission on that class
        assert_eq!(spec.markets.len(), 3, "seed {seed}");
        assert!(spec.markets.iter().all(|m| m.class == 0), "seed {seed}");
        assert!(spec.commission_e6[0] > 0, "seed {seed}: no commission");
        let mut chain = SimChain::default();
        let r = run_path(&spec, world_for(&spec), &mut chain);
        assert!(r.world.ev.fault().is_none() && r.notes.is_empty(), "seed {seed}");
        let st = r.world.ev.stats();
        assert!(matches!(r.world.ev.status(), Status::Failed(_)), "seed {seed}");
        assert_eq!((st.static_fails, st.quick_fails, st.unpostable), (1, 0, 0), "seed {seed}");
        // the journal: three forced closes with a commission each, then the Fail. When the policy lets a batch hold 8 events (3 seeds of 7) and
        // the three closes fit a legacy transaction with the limit instruction they are ONE batch of 3 closes, 3 fees, Fail, Meta over 3
        // proofs; otherwise the units are split (only a unit that cannot be split uses a lookup table) and the failing batch is smaller
        let batches: Vec<_> = r.items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b) } else { None }).collect();
        let all: Vec<&JournalEvent> = batches.iter().flat_map(|b| b.batch.events.iter()).collect();
        let n = |f: fn(&JournalEvent) -> bool| all.iter().filter(|e| f(e)).count();
        let commissions = all.iter().filter(|e| matches!(e, JournalEvent::Fee { kind: journal_types::FeeKind::Commission, .. })).count();
        assert_eq!((n(|e| matches!(e, JournalEvent::Close { .. })), commissions, n(|e| matches!(e, JournalEvent::Fail { .. }))), (3, 6, 1), "seed {seed}");
        let failing: Vec<_> = batches.iter().filter(|b| matches!(b.kind, BatchKind::ReportFail(_))).collect();
        assert_eq!(failing.len(), 1, "seed {seed}");
        let evs = &failing[0].batch.events;
        assert!(matches!(evs[evs.len() - 2], JournalEvent::Fail { .. }) && matches!(evs[evs.len() - 1], JournalEvent::Meta { .. }), "seed {seed}");
        if evs.len() == 8 {
            assert!(spec.batch_max_events >= 8, "seed {seed}");
            eight += 1;
            assert_eq!((evs.iter().filter(|e| matches!(e, JournalEvent::Close { .. })).count(), evs.iter().filter(|e| matches!(e, JournalEvent::Fee { .. })).count()), (3, 3), "seed {seed}");
            assert_eq!(failing[0].batch.proofs.len(), 3, "seed {seed}");
        }
        // the slide is a slide, not a blow-up: every close is within 5% of its entry (the loss is shared by three positions)
        for e in &all {
            if let JournalEvent::Close { entry_e12, exit_e12, .. } = e {
                assert!(*exit_e12 < *entry_e12 && *exit_e12 * 100 > *entry_e12 * 95, "seed {seed}: entry {entry_e12} exit {exit_e12}");
            }
        }
        // the compute contract: the limit covers the measured shape, the transaction fits a packet
        let b = failing[0];
        assert!(b.cu_limit >= b.cu && b.cu_limit <= evaluator::compute::MAX_CU_LIMIT && b.tx_bytes <= 1_232, "seed {seed}");
        let rep = verify_items(&r.world, &r.items, None, None);
        assert!(rep.verdict == setl8_verify::Verdict::Valid, "seed {seed}:\n{}", describe(&rep));
    }
    assert!(eight >= 5, "{eight} of 40 stories ended in one batch of eight events");
}

#[test]
fn the_story_has_its_slice_of_the_seeds_and_the_other_stories_keep_theirs() {
    // the scenario mix is part of the campaigns' coverage floors: 4% commission stories, 12% nights, 5% pass stories, 2% multi-quick
    // stories drawn from the first 5,000 seeds (a window of 1,000 would be too noisy)
    let n = 5_000u64;
    let count = |s: Scenario| (1..=n).filter(|x| generate(*x).scenario == s).count();
    let near = |got: usize, per_mille: usize| {
        let want = per_mille * n as usize / 1000;
        got * 100 >= want * 85 && got * 100 <= want * 115
    };
    let (c, nights, pass, mq) = (count(Scenario::CommissionFail), count(Scenario::Night), count(Scenario::Pass), count(Scenario::MultiQuick));
    assert!(near(c, 40), "commission stories {c}");
    assert!(near(nights, 120), "nights {nights}");
    assert!(near(pass, 50), "pass stories {pass}");
    assert!(near(mq, 20), "multi-quick stories {mq}");
}
