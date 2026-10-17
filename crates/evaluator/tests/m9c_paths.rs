//! M9c on the generator's own stories: each story that produces an evaluator refusal (`Edge`, `Cap`, `Remainder`, `Hold`) is VALID
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
    for scenario in [Scenario::Edge, Scenario::Cap, Scenario::Remainder, Scenario::Hold] {
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
