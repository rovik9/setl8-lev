//! P26 (decision 69, audit F-16) on the generator's own `MultiQuick` story: three or four positions in profit, the trader closes them
//! one after the other, the second close takes the quick tally over 2.8% of S, so the positions still open are closed with it
//! (`Close{Fail}` after the crossing close) and the account fails (`Fail{PhaseQuick}`). The honest history is VALID, every
//! program batch of it obeys the rules of `report_fail` after an overshoot, and the nearest dishonest histories are caught:
//! a `Close{Fail}` with no overshoot, a `Close{Fail}` for a position that is not open, a `Close{Fail}` after the account is flat.
mod common;
use common::runner::*;
use common::*;
use evaluator::testkit::{generate, Scenario};
use evaluator::{BatchKind, Status};
use journal_types::{CloseReason, FailReason, JournalEvent};
use setl8_verify::FindingClass as C;

/// The first `n` seeds (from 1) whose story is `scenario`.
fn seeds_of(scenario: Scenario, n: usize) -> Vec<u64> {
    (1..100_000u64).filter(|s| generate(*s).scenario == scenario).take(n).collect()
}

fn run(seed: u64) -> (RunResult, evaluator::testkit::PathSpec) {
    let spec = generate(seed);
    let mut chain = SimChain::default();
    let r = run_path(&spec, world_for(&spec), &mut chain);
    assert!(r.world.ev.fault().is_none() && r.notes.is_empty(), "seed {seed}");
    (r, spec)
}

fn events_of(items: &[Item]) -> Vec<&JournalEvent> {
    items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b) } else { None }).flat_map(|b| b.batch.events.iter()).collect()
}

fn dup(items: &[Item]) -> Vec<Item> {
    items
        .iter()
        .map(|i| match i {
            Item::Line(l) => Item::Line(l.clone()),
            Item::Batch(b) => Item::Batch(b.clone()),
        })
        .collect()
}

fn judge(r: &RunResult, items: &[Item]) -> setl8_verify::Report {
    verify_items(&r.world, items, None, None)
}

#[test]
fn the_story_fails_at_a_quick_overshoot_with_forced_closes_and_is_valid() {
    let mut with_forced = 0;
    for seed in seeds_of(Scenario::MultiQuick, 40) {
        let (r, _) = run(seed);
        let st = r.world.ev.stats();
        assert!(matches!(r.world.ev.status(), Status::Failed(_)), "seed {seed}");
        assert_eq!(st.quick_fails, 1, "seed {seed}");
        assert_eq!(st.unpostable, 0, "seed {seed}");
        assert!(st.forced_after_overshoot >= 1, "seed {seed}: no close after the crossing one");
        with_forced += 1;
        // the program's side: the closes after the crossing one are all `Fail` closes, in the batch that ends with the Fail
        let evs = events_of(&r.items);
        let fail_at = evs.iter().position(|e| matches!(e, JournalEvent::Fail { reason: FailReason::PhaseQuick, .. })).unwrap_or_else(|| panic!("seed {seed}: no quick Fail"));
        let forced = evs[..fail_at].iter().rev().take_while(|e| matches!(e, JournalEvent::Close { reason: CloseReason::Fail, .. })).count();
        assert!(forced >= 1, "seed {seed}: the Fail is not preceded by a forced close");
        let rep = judge(&r, &r.items);
        assert!(rep.verdict == setl8_verify::Verdict::Valid, "seed {seed}:\n{}", describe(&rep));
        // every batch obeys the batch limits and the report_fail rule: the Fail batch is a report_fail batch
        for it in &r.items {
            if let Item::Batch(b) = it {
                assert!(b.batch.events.len() <= journal_types::MAX_EVENTS_PER_BATCH && b.batch.proofs.len() <= journal_types::MAX_PROOFS_PER_BATCH, "seed {seed}");
                if b.batch.events.iter().any(|e| matches!(e, JournalEvent::Fail { .. })) {
                    assert!(matches!(b.kind, BatchKind::ReportFail(_)), "seed {seed}");
                }
            }
        }
    }
    assert!(with_forced >= 30, "{with_forced} stories with forced closes");
}

/// The honest items with `f` applied to the first batch event it returns `true` for.
fn mutated(items: &[Item], mut f: impl FnMut(&mut JournalEvent) -> bool) -> Option<Vec<Item>> {
    let mut out = dup(items);
    let mut done = false;
    for it in &mut out {
        if let Item::Batch(b) = it {
            for e in &mut b.batch.events {
                if !done && f(e) {
                    done = true;
                }
            }
        }
    }
    done.then_some(out)
}

#[test]
fn a_forced_close_that_nothing_justifies_is_caught() {
    // the same honest history without its quick Fail: the first Close{Fail} stands alone
    let mut caught = 0;
    for seed in seeds_of(Scenario::MultiQuick, 25) {
        let (r, _) = run(seed);
        // a manual close of the story relabelled as a forced one, in a history without an overshoot: take the first close, which is
        // far below the limit (the credit of the small position), and drop the quick Fail and everything after it
        let mut items = dup(&r.items);
        let mut seen_fail = false;
        let mut kept = Vec::new();
        for it in items.drain(..) {
            match &it {
                Item::Batch(b) if b.batch.events.iter().any(|e| matches!(e, JournalEvent::Fail { .. })) => seen_fail = true,
                _ => {}
            }
            if !seen_fail {
                kept.push(it);
            }
        }
        let Some(items) = mutated(&kept, |e| {
            if let JournalEvent::Close { reason, .. } = e {
                if *reason != CloseReason::Fail {
                    *reason = CloseReason::Fail;
                    return true;
                }
            }
            false
        }) else {
            continue;
        };
        let rep = judge(&r, &items);
        assert!(rep.classes().contains(&C::ForcedCloseUnjustified), "seed {seed}:\n{}", describe(&rep));
        caught += 1;
    }
    assert!(caught >= 10, "{caught} caught");
}

#[test]
fn a_forced_close_of_a_position_that_is_not_open_is_caught() {
    let mut caught = 0;
    for seed in seeds_of(Scenario::MultiQuick, 25) {
        let (r, _) = run(seed);
        // the last forced close names the market of the crossing close, whose position is already closed
        let mut crossing_market = None;
        for e in events_of(&r.items) {
            if let JournalEvent::Close { market, reason, .. } = e {
                if *reason != CloseReason::Fail {
                    crossing_market = Some(*market);
                }
            }
        }
        let Some(cm) = crossing_market else { continue };
        let Some(items) = mutated(&r.items, |e| {
            if let JournalEvent::Close { market, reason: CloseReason::Fail, .. } = e {
                *market = cm;
                return true;
            }
            false
        }) else {
            continue;
        };
        let rep = judge(&r, &items);
        assert!(rep.verdict != setl8_verify::Verdict::Valid, "seed {seed}");
        assert!(rep.classes().contains(&C::PositionMismatch), "seed {seed}:\n{}", describe(&rep));
        caught += 1;
    }
    assert!(caught >= 10, "{caught} caught");
}

#[test]
fn an_extra_forced_close_after_the_account_is_flat_is_caught() {
    let mut caught = 0;
    for seed in seeds_of(Scenario::MultiQuick, 25) {
        let (r, _) = run(seed);
        // the last forced close said twice: the second one closes a position that is no longer there
        let mut items = dup(&r.items);
        let mut done = false;
        for it in items.iter_mut().rev() {
            if let Item::Batch(b) = it {
                if let Some(i) = b.batch.events.iter().rposition(|e| matches!(e, JournalEvent::Close { reason: CloseReason::Fail, .. })) {
                    let dup = b.batch.events[i].clone();
                    b.batch.events.insert(i + 1, dup);
                    done = true;
                    break;
                }
            }
        }
        if !done {
            continue;
        }
        let rep = judge(&r, &items);
        assert!(rep.verdict != setl8_verify::Verdict::Valid, "seed {seed}");
        assert!(rep.classes().contains(&C::PositionMismatch), "seed {seed}:\n{}", describe(&rep));
        caught += 1;
    }
    assert!(caught >= 10, "{caught} caught");
}

#[test]
fn a_forced_close_nothing_justifies_is_reported_when_an_event_of_a_later_second_arrives() {
    // a relabelled first close, then a Fill of a later second with its price off by one unit: the finding of the close comes BEFORE the
    // one of the fill (the close is settled by the first event of a later second, not at the end of the log)
    let mut caught = 0;
    for seed in seeds_of(Scenario::TightClean, 12) {
        let (r, _) = run(seed);
        let mut relabelled = false;
        let Some(items) = mutated(&r.items, |e| match e {
            JournalEvent::Close { reason, .. } if !relabelled && *reason != CloseReason::Fail => {
                *reason = CloseReason::Fail;
                relabelled = true;
                false
            }
            JournalEvent::Fill { price_e12, .. } if relabelled => {
                *price_e12 += 1;
                true
            }
            _ => false,
        }) else {
            continue;
        };
        let rep = judge(&r, &items);
        let order: Vec<C> = rep.findings.iter().map(|f| f.class).collect();
        let forced = order.iter().position(|c| *c == C::ForcedCloseUnjustified).unwrap_or_else(|| panic!("seed {seed}: {order:?}"));
        let later = order.iter().position(|c| *c == C::PriceMismatch).unwrap_or_else(|| panic!("seed {seed}: {order:?}"));
        assert!(forced < later, "seed {seed}: {order:?}");
        caught += 1;
    }
    assert!(caught >= 5, "{caught} caught");
}

#[test]
fn the_story_trades_three_or_four_class_0_markets_with_distinct_prices_and_spreads() {
    let mut sizes = std::collections::BTreeSet::new();
    for seed in seeds_of(Scenario::MultiQuick, 60) {
        let spec = generate(seed);
        let ms = &spec.markets;
        sizes.insert(ms.len());
        assert!(ms.len() == 3 || ms.len() == 4, "seed {seed}");
        for (i, m) in ms.iter().enumerate() {
            assert_eq!((m.id, m.class), (i as u16, 0), "seed {seed}");
            assert_eq!(m.symbol, format!("M{i}C0"), "seed {seed}");
            assert_eq!(m.start_price, ms[0].start_price + i as u64 * 37_000_000_000, "seed {seed}");
            assert_eq!(m.spread_e6, ms[0].spread_e6 + 7 * i as u32, "seed {seed}");
        }
    }
    assert_eq!(sizes.into_iter().collect::<Vec<_>>(), vec![3, 4], "both sizes occur");
}
