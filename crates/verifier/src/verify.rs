//! `verify_account`: parses the inputs, runs the replay and builds the report. Pure: bytes in, report out.
use std::collections::HashSet;

use journal_types::{genesis_hash, JournalEvent, NativeSha256};

use crate::archive::{self, Archive};
use crate::finding::*;
use crate::intents::{self, IntentFile};
use crate::log::{self, Line, LineAt, OpenedRec, PassKind};
use crate::replay::{Ctx, Findings, Replay};
use crate::rules::Rules;
use crate::snapshot::Snapshot;

/// The raw bytes of every input. Nothing here reads a file.
#[derive(Clone, Debug, Default)]
pub struct Inputs<'a> {
    pub events: &'a [u8],
    pub rules: &'a [u8],
    pub archive: &'a [u8],
    pub intents: &'a [u8],
    pub snapshot: Option<&'a [u8]>,
    pub second_archive: Option<&'a [u8]>,
    /// Replay only the events up to this chain sequence (the watchdog use of DESIGN 4.6).
    pub as_of_seq: Option<u64>,
}

/// The inputs are not in the documented formats (CLI exit code 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputError(pub String);

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub struct Parsed {
    pub lines: Vec<LineAt>,
    pub rules: Rules,
    pub archive: Archive,
    pub second: Option<Archive>,
    pub intents: IntentFile,
    pub snapshot: Option<Snapshot>,
    pub opened: OpenedRec,
}

pub fn parse_inputs(inputs: &Inputs) -> Result<Parsed, InputError> {
    let text = std::str::from_utf8(inputs.events).map_err(|_| InputError("events file is not UTF-8".into()))?;
    let lines = log::parse(text).map_err(InputError)?;
    let rules = Rules::parse(inputs.rules).map_err(InputError)?;
    let archive = Archive::parse(inputs.archive).map_err(|e| InputError(format!("archive: {e}")))?;
    let second = match inputs.second_archive {
        Some(b) => Some(Archive::parse(b).map_err(|e| InputError(format!("second archive: {e}")))?),
        None => None,
    };
    let intents = IntentFile::parse(inputs.intents).map_err(|e| InputError(e.0))?;
    let snapshot = match inputs.snapshot {
        Some(b) => Some(Snapshot::parse(b).map_err(InputError)?),
        None => None,
    };
    let opened = lines
        .iter()
        .find_map(|l| if let Line::Opened(o) = &l.line { Some(o.clone()) } else { None })
        .ok_or_else(|| InputError("the events file has no `opened` line".into()))?;
    for l in &lines {
        if let Some(a) = l.account {
            if a != opened.account {
                return Err(InputError(format!("events line {} belongs to another account", l.line_no)));
            }
        }
    }
    if i64::try_from(opened.size_s).is_err() {
        return Err(InputError("opened.size_s does not fit i64".into()));
    }
    // the rules file must be the rule set the account was opened under (M12 finding VRF-3): a file of another version would be
    // replayed without a word
    if opened.ruleset_version != rules.ruleset_version {
        return Err(InputError(format!("the account was opened under rule set {} but the rules file is rule set {}", opened.ruleset_version, rules.ruleset_version)));
    }
    Ok(Parsed { lines, rules, archive, second, intents, snapshot, opened })
}

/// Meta hashes of the events that will be replayed (to search the archive and intents prefixes in one pass).
fn meta_hashes(lines: &[LineAt], as_of: Option<u64>) -> (HashSet<[u8; 32]>, HashSet<[u8; 32]>) {
    let (mut s, mut i) = (HashSet::new(), HashSet::new());
    for l in lines {
        if let Line::Event { seq, event, .. } = &l.line {
            if as_of.is_some_and(|n| *seq > n) {
                break;
            }
            if let Ok(JournalEvent::Meta { samples_hash, intents_hash, .. }) = JournalEvent::decode(event) {
                s.insert(samples_hash);
                i.insert(intents_hash);
            }
        }
    }
    (s, i)
}

pub fn verify_account(inputs: &Inputs) -> Result<Report, InputError> {
    let p = parse_inputs(inputs)?;
    Ok(run(&p, inputs.as_of_seq))
}

pub fn run(p: &Parsed, as_of: Option<u64>) -> Report {
    let (sh, ih) = meta_hashes(&p.lines, as_of);
    let sample_prefix = archive::find_prefixes(&p.archive.stream, &sh);
    let intent_prefix = intents::find_prefixes(&p.intents.raw, &ih);
    let Some(mut rp) = Replay::new(&p.rules, &p.archive, &p.intents, p.opened.clone(), "primary", sample_prefix, intent_prefix) else {
        return Report { verdict: Verdict::Mismatch, findings: vec![], truncated: false, per_phase: vec![], per_pass: vec![], state: StateSummary::default() };
    };
    let genesis = genesis_hash(&NativeSha256, &p.opened.account);
    if genesis != p.opened.genesis_hash {
        rp.f.add(
            FindingClass::BadGenesis,
            Ctx { seq: None, idx: None, time: p.opened.time },
            crate::hexutil::encode(&genesis),
            crate::hexutil::encode(&p.opened.genesis_hash),
            "genesis hash is not sha256(\"SL8LEV\" || account)",
        );
    }
    rp.check_archive();
    rp.check_intents(&p.opened.trader);
    rp.init_walk();
    drive(&mut rp, &p.lines, as_of);
    rp.finish(as_of.is_some());
    if as_of.is_none() {
        if let Some(s) = &p.snapshot {
            rp.compare_snapshot(s);
        }
    }
    let extra = match &p.second {
        Some(b) => second_recorder(p, b, as_of),
        None => vec![],
    };
    build_report(rp, extra)
}

/// Two samples of the same market and source timestamp may differ by at most this many e12 units (after the floor of
/// `value / 10^6`). Two recorders of one stream see the same numbers, so the tolerance is NONE.
pub const ARCHIVE_TOLERANCE_E12: u64 = 0;

/// The classes the second recorder can add: what the walk finds on the second archive.
const WALK_CLASSES: [FindingClass; 17] = [
    FindingClass::MissedBreach,
    FindingClass::MissedFail,
    FindingClass::FalseBreach,
    FindingClass::MissedTightStart,
    FindingClass::FalseTightStart,
    FindingClass::MissedFloorRaise,
    FindingClass::FalseFloorRaise,
    FindingClass::WrongLLevel,
    FindingClass::TriggerMissed,
    FindingClass::TriggerFalse,
    FindingClass::PendingFillWithoutMargin,
    FindingClass::FillWithoutMargin,
    FindingClass::FillNotAllowed,
    FindingClass::ArchiveRecordInvalid,
    FindingClass::PriceSampleMismatch,
    FindingClass::ForcedCloseUnjustified,
    FindingClass::FalseFailMarks,
];

/// DESIGN 4.5 / 13.2 "dropped sample": replays the walk on the second archive and reports (a) the samples the two archives
/// disagree on, (b) every floor, breach or trigger finding the SECOND archive supports that the primary does not.
fn second_recorder(p: &Parsed, b: &Archive, as_of: Option<u64>) -> Vec<Finding> {
    let mut out = Vec::new();
    let ctx = Ctx { seq: None, idx: None, time: p.opened.time };
    let mut by_key = std::collections::HashMap::new();
    for s in &p.archive.samples {
        by_key.entry((s.market, s.ts_ns)).or_insert(s.value);
    }
    let mut dis = Findings::new("second");
    for s in &b.samples {
        if let Some(a) = by_key.get(&(s.market, s.ts_ns)) {
            let diff = a.abs_diff(s.value) / 1_000_000;
            if diff > u128::from(ARCHIVE_TOLERANCE_E12) {
                dis.add(
                    FindingClass::ArchiveDisagreement,
                    ctx,
                    format!("market {} second {} value {}", s.market, s.ts_ns / 1_000_000_000, a),
                    s.value.to_string(),
                    format!("the archives differ by {diff} e12 units at source timestamp {} ns", s.ts_ns),
                );
            }
        }
    }
    out.extend(dis.list);
    let Some(mut rp) = Replay::new(&p.rules, b, &p.intents, p.opened.clone(), "second", Default::default(), Default::default()) else { return out };
    rp.check_archive();
    rp.check_intents(&p.opened.trader);
    rp.init_walk();
    drive(&mut rp, &p.lines, as_of);
    rp.finish(as_of.is_some());
    for f in rp.f.list {
        if WALK_CLASSES.contains(&f.class) {
            out.push(f);
        }
    }
    out
}

/// Feeds the log lines to the replay in file order.
pub fn drive(rp: &mut Replay, lines: &[LineAt], as_of: Option<u64>) {
    let mut opened_seen = false;
    // the new phase of the `pass_finalized` line directly before the current line (decision D5)
    let mut finalized_to: Option<u8> = None;
    // the phase of the newest `pass_requested` that no later line has resolved: a `phase_advanced` needs one
    let mut open_pass: Option<u8> = None;
    for l in lines {
        let just_finalized = finalized_to.take();
        let ctx = Ctx { seq: Some(rp.last_seq), idx: None, time: rp.last_time };
        match &l.line {
            Line::Opened(_) => {
                if opened_seen {
                    rp.f.add(FindingClass::StateMismatch, ctx, "one opened line", "a second one", "the account is opened twice");
                }
                opened_seen = true;
            }
            Line::Event { seq, event, proof, .. } => {
                if as_of.is_some_and(|n| *seq > n) {
                    break;
                }
                rp.on_event_line(*seq, event, proof);
            }
            Line::Pass { kind, phase, amount, b, seq, journal_hash, time_or_eligible } => {
                if *kind == PassKind::Requested {
                    open_pass = Some(*phase);
                }
                rp.on_pass_line(kind, *phase, *amount, *b, *seq, journal_hash, *time_or_eligible)
            }
            Line::PassRejected { .. } | Line::PassVetoed { .. } => open_pass = None,
            // The program emits `PhaseAdvanced` as well as `PassFinalized` for one `finalize_pass`: the one that directly
            // follows a `pass_finalized` with the same new phase confirms it and changes nothing (decision D5).
            Line::PhaseAdvanced { to_phase, .. } if just_finalized == Some(*to_phase) => {}
            Line::PhaseAdvanced { from_phase, to_phase, .. } => {
                // any other phase advance is the phase-1 confirmation: it needs the pass request of that phase
                if open_pass != Some(*from_phase) {
                    rp.f.add(FindingClass::StateMismatch, ctx, format!("a pass_requested line of phase {from_phase} before the phase change"), "none", "phase_advanced without a pending pass request of that phase");
                }
                open_pass = None;
                rp.on_phase_change(*from_phase, *to_phase, None)
            }
            Line::PassFinalized { phase, to_phase, amount, .. } => {
                open_pass = None;
                rp.on_phase_change(*phase, *to_phase, Some(*amount));
                finalized_to = Some(*to_phase);
            }
            Line::AccountFailed { b, seq, journal_hash, .. } => {
                if *b != rp.st.b || *seq != rp.last_seq || journal_hash != &rp.head {
                    rp.f.add(FindingClass::StateMismatch, ctx, format!("b {} seq {}", rp.st.b, rp.last_seq), format!("b {b} seq {seq}"), "AccountFailed disagrees with the replay");
                }
                rp.terminal = true;
            }
            Line::ViolationFail { .. } => rp.terminal = true,
            Line::Held { time } => rp.on_hold(ctx, *time, true),
            Line::HoldReleased { time } => rp.on_hold(ctx, *time, false),
            Line::Unknown { type_name } => rp.f.add(FindingClass::UnknownEvent, ctx, "a known line type", type_name.clone(), "unknown line type in the event log"),
        }
    }
}

/// Adds the findings of the second recorder that the primary run did not already report (same class and same expectation).
pub fn merge_findings(mut primary: Vec<Finding>, extra: Vec<Finding>) -> Vec<Finding> {
    for f in extra {
        let dup = primary.iter().any(|g| g.class == f.class && g.expected == f.expected);
        if !dup {
            primary.push(f);
        }
    }
    primary
}

pub fn build_report(rp: Replay, extra: Vec<Finding>) -> Report {
    let per_phase = rp.phase_verdicts();
    let state = rp.summary();
    let mut findings = rp.f.list;
    findings = merge_findings(findings, extra);
    findings.sort_by(|a, b| (a.seq, a.event_index, a.class, &a.message).cmp(&(b.seq, b.event_index, b.class, &b.message)));
    let any_bad_pass = rp.passes.iter().any(|v| !v.valid);
    let verdict = if findings.is_empty() && !rp.f.truncated && !any_bad_pass { Verdict::Valid } else { Verdict::Mismatch };
    Report { verdict, findings, truncated: rp.f.truncated, per_phase, per_pass: rp.passes, state }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(class: FindingClass, expected: &str, source: &'static str) -> Finding {
        Finding { class, seq: None, event_index: None, expected: expected.into(), found: String::new(), message: String::new(), source }
    }

    #[test]
    fn the_second_recorder_adds_only_what_the_primary_lacks() {
        let primary = vec![f(FindingClass::MissedBreach, "at 10", "primary")];
        let merged = merge_findings(
            primary,
            vec![
                f(FindingClass::MissedBreach, "at 10", "second"), // the same miss
                f(FindingClass::MissedBreach, "at 20", "second"), // another miss of the same class
                f(FindingClass::MissedFail, "at 10", "second"),   // another class, same words
            ],
        );
        assert_eq!(merged.len(), 3);
        assert_eq!(merged.iter().filter(|x| x.source == "second").count(), 2);
        assert!(merged.iter().any(|x| x.class == FindingClass::MissedBreach && x.expected == "at 20"));
    }
}
