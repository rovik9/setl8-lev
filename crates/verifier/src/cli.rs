//! The command-line shell. File access is injected so the whole thing is testable without a process.
use crate::finding::{Report, Verdict};
use crate::log::{Line, PassKind};
use crate::verify::{verify_account, Inputs};

pub const EXIT_VALID: i32 = 0;
pub const EXIT_MISMATCH: i32 = 1;
pub const EXIT_INPUT: i32 = 2;

pub const USAGE: &str = "usage:\n  setl8-verify account --events E.jsonl --rules R.json --archive A.bin --intents I.jsonl [--snapshot S.json] [--second-archive B.bin] [--json] [--as-of-seq N]\n  setl8-verify watch   --events E.jsonl --rules R.json --archive A.bin --intents I.jsonl [--second-archive B.bin]\nexit codes: 0 valid, 1 mismatch, 2 input error\n";

#[derive(Default, Debug)]
struct Args {
    events: Option<String>,
    rules: Option<String>,
    archive: Option<String>,
    intents: Option<String>,
    snapshot: Option<String>,
    second: Option<String>,
    json: bool,
    as_of: Option<u64>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--events" => a.events = Some(value("--events")?),
            "--rules" => a.rules = Some(value("--rules")?),
            "--archive" => a.archive = Some(value("--archive")?),
            "--intents" => a.intents = Some(value("--intents")?),
            "--snapshot" => a.snapshot = Some(value("--snapshot")?),
            "--second-archive" => a.second = Some(value("--second-archive")?),
            "--as-of-seq" => {
                a.as_of = Some(value("--as-of-seq")?.parse::<u64>().map_err(|_| "--as-of-seq needs an unsigned integer".to_string())?);
            }
            "--json" => a.json = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// Runs the CLI. `read` loads a file; the output goes to `out`; the return value is the exit code.
pub fn run(args: &[String], read: &dyn Fn(&str) -> std::io::Result<Vec<u8>>, out: &mut String) -> i32 {
    let Some((cmd, rest)) = args.split_first() else {
        out.push_str(USAGE);
        return EXIT_INPUT;
    };
    if cmd != "account" && cmd != "watch" {
        out.push_str(USAGE);
        return EXIT_INPUT;
    }
    let a = match parse_args(rest) {
        Ok(a) => a,
        Err(e) => {
            out.push_str(&format!("error: {e}\n{USAGE}"));
            return EXIT_INPUT;
        }
    };
    let load = |name: &Option<String>, what: &str| -> Result<Vec<u8>, String> {
        let path = name.as_ref().ok_or_else(|| format!("missing --{what}"))?;
        read(path).map_err(|e| format!("cannot read {path}: {e}"))
    };
    let loaded = (|| -> Result<_, String> {
        let events = load(&a.events, "events")?;
        let rules = load(&a.rules, "rules")?;
        let archive = load(&a.archive, "archive")?;
        let intents = load(&a.intents, "intents")?;
        let snapshot = match &a.snapshot {
            Some(p) => Some(read(p).map_err(|e| format!("cannot read {p}: {e}"))?),
            None => None,
        };
        let second = match &a.second {
            Some(p) => Some(read(p).map_err(|e| format!("cannot read {p}: {e}"))?),
            None => None,
        };
        Ok((events, rules, archive, intents, snapshot, second))
    })();
    let (events, rules, archive, intents, snapshot, second) = match loaded {
        Ok(v) => v,
        Err(e) => {
            out.push_str(&format!("error: {e}\n"));
            return EXIT_INPUT;
        }
    };
    let mut inputs = Inputs { events: &events, rules: &rules, archive: &archive, intents: &intents, snapshot: snapshot.as_deref(), second_archive: second.as_deref(), as_of_seq: a.as_of };
    if cmd == "watch" {
        return watch(&mut inputs, out);
    }
    match verify_account(&inputs) {
        Ok(r) => {
            if a.json {
                match serde_json::to_string_pretty(&r) {
                    Ok(s) => {
                        out.push_str(&s);
                        out.push('\n');
                    }
                    Err(e) => {
                        out.push_str(&format!("error: {e}\n"));
                        return EXIT_INPUT;
                    }
                }
            } else {
                out.push_str(&human(&r));
            }
            exit_code(&r)
        }
        Err(e) => {
            out.push_str(&format!("error: {e}\n"));
            EXIT_INPUT
        }
    }
}

fn exit_code(r: &Report) -> i32 {
    if r.verdict == Verdict::Valid {
        EXIT_VALID
    } else {
        EXIT_MISMATCH
    }
}

/// The chain sequence of the newest pass that is still pending (requested or confirmed, not yet rejected, vetoed,
/// finalized or ended by a fail), if any.
pub fn newest_pending_pass(events: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(events).ok()?;
    let lines = crate::log::parse(text).ok()?;
    let mut pending = None;
    for l in &lines {
        match &l.line {
            Line::Pass { kind: PassKind::Requested | PassKind::Confirmed, seq, .. } => pending = Some(*seq),
            Line::PassRejected { .. } | Line::PassVetoed { .. } | Line::PhaseAdvanced { .. } | Line::PassFinalized { .. } | Line::AccountFailed { .. } | Line::ViolationFail { .. } => pending = None,
            _ => {}
        }
    }
    pending
}

fn watch(inputs: &mut Inputs, out: &mut String) -> i32 {
    let Some(seq) = newest_pending_pass(inputs.events) else {
        out.push_str("NO_PENDING_PASS\n");
        return EXIT_VALID;
    };
    inputs.as_of_seq = Some(seq);
    inputs.snapshot = None;
    match verify_account(inputs) {
        Ok(r) => {
            if r.verdict == Verdict::Valid {
                out.push_str(&format!("VALID seq={seq}\n"));
            } else {
                let classes: Vec<&str> = r.classes().iter().map(|c| c.name()).collect();
                out.push_str(&format!("MISMATCH seq={seq} classes={}\n", classes.join(",")));
            }
            exit_code(&r)
        }
        Err(e) => {
            out.push_str(&format!("error: {e}\n"));
            EXIT_INPUT
        }
    }
}

/// Deterministic human-readable text.
pub fn human(r: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!("VERDICT: {}\n", if r.verdict == Verdict::Valid { "VALID" } else { "MISMATCH" }));
    let st = &r.state;
    s.push_str(&format!(
        "replayed: seq {} | b {} | q_tally {} | latch {} | tight_on {} (r {} g_peak {} l_level {}) | lock_until {} | phase {} | flat {} | trades_closed {} | head {}\n",
        st.seq, st.b, st.q_tally, st.latch, st.tight_on, st.r, st.g_peak, st.l_level, st.lock_until, st.phase, st.flat, st.trades_closed, st.journal_hash
    ));
    s.push_str(&format!("findings: {}{}\n", r.findings.len(), if r.truncated { " (truncated)" } else { "" }));
    for f in &r.findings {
        s.push_str(&format!(
            "  [{}] seq={} event={} source={}: {} (expected {}, found {})\n",
            f.class.name(),
            f.seq.map_or("-".to_string(), |v| v.to_string()),
            f.event_index.map_or("-".to_string(), |v| v.to_string()),
            f.source,
            f.message,
            f.expected,
            f.found
        ));
    }
    for p in &r.per_phase {
        s.push_str(&format!("phase {}: seq {}..{} events {} {}\n", p.phase, p.first_seq, p.last_seq, p.events, if p.valid { "valid" } else { "INVALID" }));
    }
    for p in &r.per_pass {
        s.push_str(&format!("pass {} of phase {} at seq {}: {}{}\n", p.kind, p.phase, p.requested_seq, if p.valid { "valid" } else { "INVALID" }, if p.reasons.is_empty() { String::new() } else { format!(" ({})", p.reasons.join(", ")) }));
    }
    s
}
