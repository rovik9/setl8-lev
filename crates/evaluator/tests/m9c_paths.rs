//! M9c on the generator's own stories: each story that produces an evaluator refusal (`Edge`, `Cap`, `Remainder`, `Hold`, `Unpriced`) is VALID
//! as generated, and the nearest dishonest history is caught: the same journal judged under rules or a log that make the refusal
//! unjustified (a higher cap, a lower minimum, no hold lines) misses the fill the order would have got; a hold the log does not
//! have is a `TriggerMissed`, an `Edge` fill one step too large is a `FillWithoutMargin`.
mod common;
use common::runner::*;
use common::*;
use evaluator::testkit::{generate, PathSpec, Scenario};
use setl8_verify::FindingClass as C;

/// The first `n` seeds (from 1) whose story is `scenario`.
fn seeds_of(scenario: Scenario, n: usize) -> Vec<u64> {
    (1..100_000u64).filter(|s| generate(*s).scenario == scenario).take(n).collect()
}

struct Run {
    spec: PathSpec,
    r: RunResult,
}

fn run(seed: u64) -> Run {
    let spec = generate(seed);
    let mut chain = SimChain::default();
    let r = run_path(&spec, world_for(&spec), &mut chain);
    assert!(r.world.ev.fault().is_none() && r.notes.is_empty(), "seed {seed}");
    Run { spec, r }
}

/// The verifier over the run's items, with the rules text replaced when `rules` is given and the lines of `type` dropped.
fn judge(x: &Run, rules: Option<(&str, &str)>, drop_lines: &[&str]) -> setl8_verify::Report {
    let w = &x.r.world;
    let items: Vec<Item> = x
        .r
        .items
        .iter()
        .filter(|i| match i {
            Item::Line(l) => !drop_lines.iter().any(|t| l.contains(&format!("\"type\":\"{t}\""))),
            Item::Batch(_) => true,
        })
        .map(|i| match i {
            Item::Line(l) => Item::Line(l.clone()),
            Item::Batch(b) => Item::Batch(b.clone()),
        })
        .collect();
    let events = items_text(&w.opened, &items);
    let mut rules_text = rules_json(&w.rules, w.opened.size_micro);
    if let Some((from, to)) = rules {
        assert!(rules_text.contains(from), "{from} is not in the rules");
        rules_text = rules_text.replacen(from, to, 1);
    }
    setl8_verify::verify_account(&setl8_verify::Inputs {
        events: events.as_bytes(),
        rules: rules_text.as_bytes(),
        archive: w.ev.archive_bytes(),
        intents: w.ev.intents_bytes(),
        snapshot: None,
        second_archive: None,
        as_of_seq: None,
    })
    .expect("inputs parse")
}

#[test]
fn every_story_is_valid_as_generated() {
    for scenario in [Scenario::Edge, Scenario::Cap, Scenario::Remainder, Scenario::Hold, Scenario::Unpriced] {
        for seed in seeds_of(scenario, 25) {
            let x = run(seed);
            let rep = judge(&x, None, &[]);
            assert!(rep.verdict == setl8_verify::Verdict::Valid, "{scenario:?} seed {seed}:\n{}", describe(&rep));
        }
    }
}

#[test]
fn a_cap_that_was_not_there_makes_the_refusal_a_miss() {
    // the evaluator refused the pending order for the notional cap; under rules without that cap the order had to fill
    let mut caught = 0;
    for seed in seeds_of(Scenario::Cap, 25) {
        let x = run(seed);
        if !x.r.world.ev.refusals().iter().any(|r| r.reason == evaluator::OrderReject::NotionalCap) {
            continue;
        }
        let cap = x.spec.markets[0].max_notional_micro;
        let rep = judge(&x, Some((&format!("\"max_notional_usd\":{cap}"), "\"max_notional_usd\":1000000000000000")), &[]);
        assert!(rep.classes().contains(&C::TriggerMissed), "seed {seed}:\n{}", describe(&rep));
        caught += 1;
    }
    assert!(caught >= 10, "{caught} stories refused for the cap");
}

#[test]
fn a_minimum_that_was_not_there_makes_the_refusal_a_miss() {
    let mut caught = 0;
    for seed in seeds_of(Scenario::Remainder, 25) {
        let x = run(seed);
        if !x.r.world.ev.refusals().iter().any(|r| r.reason == evaluator::OrderReject::InvalidRemainder) {
            continue;
        }
        let rep = judge(&x, Some(("\"qty_min\":1000000", "\"qty_min\":1000")), &[]);
        assert!(rep.classes().contains(&C::TriggerMissed), "seed {seed}:\n{}", describe(&rep));
        caught += 1;
    }
    assert!(caught >= 10, "{caught} stories refused for the remainder");
}

#[test]
fn a_hold_the_log_does_not_have_makes_the_refusal_a_miss() {
    let mut caught = 0;
    for seed in seeds_of(Scenario::Hold, 25) {
        let x = run(seed);
        if !x.r.world.ev.refusals().iter().any(|r| r.reason == evaluator::OrderReject::AccountHeld) {
            continue;
        }
        // with its hold lines the history is valid (above); without them the order the evaluator "refused" had to fill
        let rep = judge(&x, None, &["account_held", "hold_released"]);
        assert!(rep.classes().contains(&C::TriggerMissed), "seed {seed}:\n{}", describe(&rep));
        // a hold that is never released: the fill after the release is an execution on a held account
        let rep = judge(&x, None, &["hold_released"]);
        assert!(rep.classes().contains(&C::FillNotAllowed), "seed {seed}:\n{}", describe(&rep));
        caught += 1;
    }
    assert!(caught >= 10, "{caught} stories refused for the hold");
}

#[test]
fn an_edge_story_fills_exactly_at_the_margin_and_a_lower_leverage_makes_it_a_fill_without_margin() {
    let mut exact = 0;
    for seed in seeds_of(Scenario::Edge, 25) {
        let x = run(seed);
        if x.r.world.ev.stats().margin_exact == 0 {
            continue;
        }
        exact += 1;
        // the same history under a rule set whose leverage is one less: the fill that used exactly all the margin now cannot be margined
        let rep = judge(&x, Some(("\"leverage_x\":[50,", "\"leverage_x\":[49,")), &[]);
        assert!(rep.classes().contains(&C::FillWithoutMargin), "seed {seed}:\n{}", describe(&rep));
    }
    assert!(exact >= 20, "{exact} stories at the margin limit");
}

/// Decision 64 on the generator's story: the trader tries a market order, a limit order and a stop order in the market the rule
/// set does not price (market 1, half-spread 0). The evaluator refuses each at arrival and leaves no trace of it: no line in the
/// intents file, no event; the priced market goes on trading and the whole history is VALID (the walk expects no fill there).
#[test]
fn an_unpriced_attempt_is_refused_at_arrival_and_leaves_no_trace() {
    let mut checked = 0;
    for seed in seeds_of(Scenario::Unpriced, 25) {
        let x = run(seed);
        let w = &x.r.world;
        assert_eq!(x.spec.markets[1].spread_e6, 0, "seed {seed}: the second market is the unpriced one");
        assert_eq!(w.ev.stats().unpriced_rejects, 3, "seed {seed}: market, limit and stop order");
        let file = setl8_verify::intents::IntentFile::parse(w.ev.intents_bytes()).expect("intents parse");
        assert!(file.lines.iter().all(|l| l.intent.as_ref().map(|it| it.market != 1).unwrap_or(true)), "seed {seed}: an order of the unpriced market is on file");
        assert!(w.ev.refusals().iter().all(|r| r.reason != evaluator::OrderReject::MarketUnpriced), "seed {seed}: nothing is refused later");
        // the priced market traded: the story is not trivially empty
        assert!(w.ev.stats().orders_accepted >= 3, "seed {seed}");
        let rep = judge(&x, None, &[]);
        assert!(rep.verdict == setl8_verify::Verdict::Valid, "seed {seed}:\n{}", describe(&rep));
        checked += 1;
    }
    assert!(checked >= 20, "{checked} unpriced stories");
}

/// The `Edge` story has three shapes (a single order, an add to a smaller position, a flip) and in every one of them the order that
/// uses the whole margin is accepted with `equity - margin == 0` exactly and the one unit more is refused for margin. The generator
/// finds the exact size by search (`edge_max_order`), so this pins that it does, for all three shapes, on every story.
#[test]
fn every_edge_story_ends_exactly_at_the_margin_in_each_of_its_three_shapes() {
    use journal_types::{CloseReason, JournalEvent};
    let (mut single, mut add, mut flip) = (0, 0, 0);
    for seed in seeds_of(Scenario::Edge, 90) {
        let x = run(seed);
        let evs = events(&x.r.items);
        let flips = evs.iter().any(|e| matches!(e, JournalEvent::Close { reason: CloseReason::Flip, .. }));
        let fills = evs.iter().filter(|e| matches!(e, JournalEvent::Fill { .. })).count();
        match (flips, fills) {
            (true, 2) => flip += 1,
            (false, 2) => add += 1,
            (false, 1) => single += 1,
            other => panic!("seed {seed}: {other:?} is none of the three shapes"),
        }
        assert_eq!(x.r.world.ev.stats().margin_exact, 1, "seed {seed}: the whole margin is used exactly, once");
        let refused: Vec<_> = x.r.world.ev.refusals().iter().filter(|r| r.reason == evaluator::OrderReject::InsufficientMargin).collect();
        assert_eq!(refused.len(), 1, "seed {seed}: the unit more is refused for margin, once");
    }
    assert!(single >= 10 && add >= 10 && flip >= 10, "shapes: single {single}, add {add}, flip {flip}");
}
