//! `verify_account`: parses the inputs, runs the replay and builds the report. Pure: bytes in, report out.
use std::collections::HashSet;

use journal_types::{genesis_hash, JournalEvent, NativeSha256};

use crate::archive::{self, Archive};
use crate::finding::*;
use crate::intents::{self, IntentFile};
use crate::log::{self, Line, LineAt, OpenedRec};
use crate::replay::{Ctx, Replay};
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
    rp.check_intents(&p.opened.trader);
    drive(&mut rp, &p.lines, as_of);
    rp.finish(as_of.is_some());
    if as_of.is_none() {
        if let Some(s) = &p.snapshot {
            rp.compare_snapshot(s);
        }
    }
    build_report(rp)
}

/// Feeds the log lines to the replay in file order.
pub fn drive(rp: &mut Replay, lines: &[LineAt], as_of: Option<u64>) {
    let mut opened_seen = false;
    for l in lines {
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
            Line::Pass { kind, phase, amount, b, seq, journal_hash, .. } => rp.on_pass_line(kind, *phase, *amount, *b, *seq, journal_hash),
            Line::PassRejected { .. } | Line::PassVetoed { .. } => {}
            Line::PhaseAdvanced { from_phase, to_phase, .. } => rp.on_phase_change(*from_phase, *to_phase, None),
            Line::PassFinalized { phase, to_phase, amount, .. } => rp.on_phase_change(*phase, *to_phase, Some(*amount)),
            Line::AccountFailed { b, seq, journal_hash, .. } => {
                if *b != rp.st.b || *seq != rp.last_seq || journal_hash != &rp.head {
                    rp.f.add(FindingClass::StateMismatch, ctx, format!("b {} seq {}", rp.st.b, rp.last_seq), format!("b {b} seq {seq}"), "AccountFailed disagrees with the replay");
                }
                rp.terminal = true;
            }
            Line::ViolationFail { .. } => rp.terminal = true,
            Line::Unknown { type_name } => rp.f.add(FindingClass::UnknownEvent, ctx, "a known line type", type_name.clone(), "unknown line type in the event log"),
        }
    }
}

pub fn build_report(rp: Replay) -> Report {
    let per_phase = rp.phase_verdicts();
    let state = rp.summary();
    let mut findings = rp.f.list;
    findings.sort_by(|a, b| (a.seq, a.event_index, a.class, &a.message).cmp(&(b.seq, b.event_index, b.class, &b.message)));
    let any_bad_pass = rp.passes.iter().any(|v| !v.valid);
    let verdict = if findings.is_empty() && !rp.f.truncated && !any_bad_pass { Verdict::Valid } else { Verdict::Mismatch };
    Report { verdict, findings, truncated: rp.f.truncated, per_phase, per_pass: rp.passes, state }
}
