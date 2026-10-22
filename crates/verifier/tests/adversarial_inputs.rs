//! M12 item 8 (verifier side): the verifier is fed hostile files. Every case runs under `catch_unwind` and must end as
//! `Err(InputError)` (CLI exit code 2), as a report whose verdict is not `Valid`, or as a `Valid` that this file explains and
//! pins. A panic is a failure of the test. Classes: BLOCKED = the exact error / finding class is asserted; LIMITED = what still
//! passes is pinned exactly; OFF-CHAIN = the verifier is the control (everything here is, by construction); findings are
//! `#[ignore]`d `finding_vrf_n_*` tests that assert the DESIGN behaviour. Base files are the committed vectors under
//! tests/vectors/ and the generator bundles of tests/common.
mod common;
use common::*;
use setl8_verify::{verify_account, FindingClass as C, Inputs, Report, Verdict};
use std::panic::{catch_unwind, AssertUnwindSafe};

const UNCOMMITTED_TAIL: &str = "bytes after the last committed prefix (Meta.intents_hash / samples_hash) are not covered by any hash and no event refers to them: the file may legitimately grow past the last batch";
const RECV_UNUSED: &str = "Sample.recv_ns is never read by the verifier (the proof carries the source time); the evaluator's receive time is the residual oracle risk of decisions 54 to 57";

const VECTORS: [&str; 3] = ["quick_profit", "design_4_9", "holding_night"];

#[derive(Clone)]
struct Fx {
    events: Vec<u8>,
    rules: Vec<u8>,
    archive: Vec<u8>,
    intents: Vec<u8>,
    snapshot: Option<Vec<u8>>,
    second: Option<Vec<u8>>,
    as_of: Option<u64>,
}

impl Fx {
    fn load(name: &str) -> Fx {
        let d = vector_dir(name);
        let r = |f: &str| std::fs::read(d.join(f)).unwrap_or_else(|e| panic!("{name}/{f}: {e}"));
        Fx { events: r("events.jsonl"), rules: r("rules.json"), archive: r("archive.bin"), intents: r("intents.jsonl"), snapshot: Some(r("snapshot.json")), second: None, as_of: None }
    }
    fn of(b: &Bundle) -> Fx {
        let f = b.files();
        Fx { events: f.events, rules: f.rules, archive: f.archive, intents: f.intents, snapshot: f.snapshot, second: f.second, as_of: None }
    }
    fn text(&self) -> String {
        String::from_utf8(self.events.clone()).unwrap()
    }
    fn with_events(&self, s: &str) -> Fx {
        Fx { events: s.as_bytes().to_vec(), ..self.clone() }
    }
    fn with_rules(&self, s: &str) -> Fx {
        Fx { rules: s.as_bytes().to_vec(), ..self.clone() }
    }
    fn with_intents(&self, s: &[u8]) -> Fx {
        Fx { intents: s.to_vec(), ..self.clone() }
    }
    fn with_snapshot(&self, s: &str) -> Fx {
        Fx { snapshot: Some(s.as_bytes().to_vec()), ..self.clone() }
    }
    fn no_snapshot(&self) -> Fx {
        Fx { snapshot: None, ..self.clone() }
    }
}

enum Out {
    Panic,
    Input(String),
    Rep(Box<Report>),
}

fn go(fx: &Fx) -> Out {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let i = Inputs { events: &fx.events, rules: &fx.rules, archive: &fx.archive, intents: &fx.intents, snapshot: fx.snapshot.as_deref(), second_archive: fx.second.as_deref(), as_of_seq: fx.as_of };
        verify_account(&i)
    }));
    match r {
        Err(_) => Out::Panic,
        Ok(Err(e)) => Out::Input(e.0),
        Ok(Ok(rep)) => Out::Rep(Box::new(rep)),
    }
}

impl Out {
    fn valid(&self) -> bool {
        matches!(self, Out::Rep(r) if r.verdict == Verdict::Valid)
    }
    fn desc(&self) -> String {
        match self {
            Out::Panic => "PANIC".into(),
            Out::Input(e) => format!("input error: {e}"),
            Out::Rep(r) => format!("{:?} {:?}", r.verdict, r.classes()),
        }
    }
    fn classes(&self) -> Vec<C> {
        match self {
            Out::Rep(r) => r.classes(),
            _ => vec![],
        }
    }
}

/// What a case must produce.
#[allow(dead_code)]
enum Want {
    /// `Err(InputError)` whose message contains the text.
    Input(&'static str),
    /// Not `Valid` (an input error or a report with a non-Valid verdict).
    NotValid,
    /// A non-Valid report that contains this finding class.
    Has(C),
    /// `Valid`, pinned: the text says why this is correct or a known limit.
    ValidBecause(&'static str),
}

fn verdict_of(label: &str, out: &Out, want: &Want) -> Result<(), String> {
    if matches!(out, Out::Panic) {
        return Err(format!("{label}: the verifier PANICKED"));
    }
    match want {
        Want::Input(needle) => match out {
            Out::Input(e) if e.contains(needle) => Ok(()),
            o => Err(format!("{label}: expected an input error containing {needle:?}, got {}", o.desc())),
        },
        Want::NotValid if out.valid() => Err(format!("{label}: the verifier said VALID")),
        Want::NotValid => Ok(()),
        Want::Has(c) if out.valid() => Err(format!("{label}: expected finding {c:?}, the verifier said VALID")),
        Want::Has(c) if !out.classes().contains(c) => Err(format!("{label}: expected finding {c:?}, got {}", out.desc())),
        Want::Has(_) => Ok(()),
        Want::ValidBecause(_) if !out.valid() => Err(format!("{label}: expected the pinned VALID, got {}", out.desc())),
        Want::ValidBecause(_) => Ok(()),
    }
}

fn check(label: &str, out: &Out, want: &Want) {
    if let Err(e) = verdict_of(label, out, want) {
        panic!("{e}");
    }
}

/// Runs every case and reports ALL the ones that disagree with their expectation.
fn table(cases: Vec<(String, Fx, Want)>) {
    let mut bad = vec![];
    for (label, fx, want) in &cases {
        if let Err(e) = verdict_of(label, &go(fx), want) {
            bad.push(e);
        }
    }
    assert!(bad.is_empty(), "{} of {} cases disagree:\n{}", bad.len(), cases.len(), bad.join("\n"));
}

fn s(x: &str) -> String {
    x.to_string()
}

/// Replaces the FIRST occurrence; panics if absent (so a changed fixture cannot silently turn a case into a no-op).
fn rep1(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "fixture does not contain {from:?}");
    text.replacen(from, to, 1)
}

// ---------------------------------------------------------------------------------------------------------------------
// control

#[test]
fn control_every_committed_vector_is_valid_and_every_case_below_starts_from_one() {
    let mut names: Vec<String> = VECTORS.iter().map(|v| v.to_string()).collect();
    for e in std::fs::read_dir(vector_dir("m9b")).unwrap() {
        names.push(format!("m9b/{}", e.unwrap().file_name().to_string_lossy()));
    }
    assert!(names.len() >= 17, "{names:?}");
    for n in &names {
        let fx = Fx::load(n);
        let out = go(&fx);
        assert!(out.valid(), "{n}: {}", out.desc());
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: truncation at EVERY byte boundary of every input file of three vectors

fn truncated(fx: &Fx, which: usize, n: usize) -> Fx {
    let mut f = fx.clone();
    match which {
        0 => f.events.truncate(n),
        1 => f.rules.truncate(n),
        2 => f.archive.truncate(n),
        3 => f.intents.truncate(n),
        _ => f.snapshot.as_mut().unwrap().truncate(n),
    }
    f
}

fn file_of(fx: &Fx, which: usize) -> &[u8] {
    match which {
        0 => &fx.events,
        1 => &fx.rules,
        2 => &fx.archive,
        3 => &fx.intents,
        _ => fx.snapshot.as_deref().unwrap(),
    }
}

const FILE_NAMES: [&str; 5] = ["events", "rules", "archive", "intents", "snapshot"];

#[test]
fn blocked_truncation_at_every_byte_boundary_never_panics_and_is_never_valid() {
    let mut cases = 0usize;
    for name in VECTORS {
        let fx = Fx::load(name);
        for (which, fname) in FILE_NAMES.iter().enumerate() {
            let full = file_of(&fx, which).to_vec();
            for n in 0..full.len() {
                let out = go(&truncated(&fx, which, n));
                cases += 1;
                let label = format!("{name}/{fname} cut to {n} of {}", full.len());
                assert!(!matches!(out, Out::Panic), "{label}: PANIC");
                // the only truncations that may stay Valid drop nothing but trailing white space (the same document)
                let benign = which != 3 && which != 2 && full[n..].iter().all(|b| b.is_ascii_whitespace()); // an intents file is hashed byte for byte, a trailing newline included
                if benign {
                    assert!(out.valid(), "{label}: only white space was removed, expected the same VALID, got {}", out.desc());
                } else {
                    assert!(!out.valid(), "{label}: a truncated file was called VALID");
                }
            }
        }
    }
    assert!(cases > 8_000, "{cases}");
}

#[test]
fn blocked_truncated_files_are_input_errors_with_a_message_naming_the_file() {
    let fx = Fx::load("design_4_9");
    // a byte-exact cut of the archive in the middle of a record, of the header, of the rules and of the snapshot
    let a = go(&truncated(&fx, 2, fx.archive.len() - 5));
    check("archive mid record", &a, &Want::Input("archive truncated at byte"));
    check("archive mid header", &go(&truncated(&fx, 2, 10)), &Want::Input("archive truncated at byte"));
    check("archive 4 bytes", &go(&truncated(&fx, 2, 4)), &Want::Input("archive does not start with the SL8SAMPL header"));
    check("rules cut", &go(&truncated(&fx, 1, 100)), &Want::Input("rules file:"));
    check("snapshot cut", &go(&truncated(&fx, 4, 50)), &Want::Input("snapshot:"));
    check("events cut mid line", &go(&truncated(&fx, 0, 700)), &Want::Input("events line"));
    check("events empty", &go(&truncated(&fx, 0, 0)), &Want::Input("events file has no header line"));
    check("intents empty", &go(&truncated(&fx, 3, 0)), &Want::Input("intents file has no header line"));
}

// LIMITED: the verifier cannot see that the TAIL of the log is missing unless it gets the snapshot (the account's stored seq and
// head); a log cut at a batch boundary is a valid shorter history. The control is the snapshot / the on-chain head.
#[test]
fn limited_a_log_cut_at_a_line_boundary_is_valid_for_the_prefix_unless_a_snapshot_is_given() {
    for name in VECTORS {
        let fx = Fx::load(name);
        let text = fx.text();
        let full_seq = match go(&fx) {
            Out::Rep(r) => r.state.seq,
            o => panic!("{}", o.desc()),
        };
        let mut boundaries = 0;
        let mut pos = 0;
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        for (i, l) in lines.iter().enumerate() {
            pos += l.len();
            if i < 2 || i + 1 == lines.len() {
                continue; // header, opened, and the complete file
            }
            boundaries += 1;
            let cut = fx.with_events(&text[..pos]);
            // with the snapshot the shortened log is caught (seq / head differ from the account)
            let with = go(&cut);
            assert!(!with.valid(), "{name}: line {i} cut, snapshot given: {}", with.desc());
            assert!(with.classes().contains(&C::StateMismatch), "{name}: {}", with.desc());
            // without it the prefix replays as a valid shorter history, with a smaller seq; or it is flagged by the batch checks
            match go(&cut.no_snapshot()) {
                Out::Rep(r) if r.verdict == Verdict::Valid => assert!(r.state.seq < full_seq, "{name}: line {i}"),
                Out::Rep(_) | Out::Input(_) => {}
                Out::Panic => panic!("{name}: line {i} PANIC"),
            }
        }
        assert!(boundaries >= 3, "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED / LIMITED: single-byte corruption of every byte of every file, three vectors

#[test]
fn blocked_every_byte_of_archive_intents_and_snapshot_is_covered_by_a_check() {
    let mut n = 0usize;
    for name in VECTORS {
        let fx = Fx::load(name);
        for which in [2usize, 3, 4] {
            let len = file_of(&fx, which).len();
            for pos in 0..len {
                for x in [0x01u8, 0x80, 0xff] {
                    let mut f = fx.clone();
                    match which {
                        2 => f.archive[pos] ^= x,
                        3 => f.intents[pos] ^= x,
                        _ => f.snapshot.as_mut().unwrap()[pos] ^= x,
                    }
                    let out = go(&f);
                    n += 1;
                    let label = format!("{name}/{} byte {pos} ^{x:02x}", FILE_NAMES[which]);
                    assert!(!matches!(out, Out::Panic), "{label}: PANIC");
                    assert!(!out.valid(), "{label}: a corrupted byte of an input file was called VALID");
                }
            }
        }
    }
    assert!(n > 8_000, "{n}");
}

/// The JSON role of byte `p` of an events line, for the survivor classification below.
fn role(line: &str, p: usize) -> String {
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("event").to_string();
    if let Some(o) = v.as_object() {
        for k in o.keys() {
            let pat = format!("\"{k}\":");
            if let Some(i) = line.find(&pat) {
                let vs = i + pat.len();
                let ve = line[vs..].find([',', '}']).map(|e| vs + e).unwrap_or(line.len());
                if p >= i && p < ve {
                    return format!("{ty}.{k}.{}", if p < vs { "KEY" } else { "val" });
                }
            }
        }
    }
    format!("{ty}.<syntax>")
}

// LIMITED: which byte changes in the EVENTS file still verify. Everything that is not in this list is detected. The survivors
// are fields the verifier does not use (slot: informational), keys whose loss has a default (`account` absent = line not tied to
// an account, `type` absent = "event": the hash chain still binds the event to the account), the final line break, and four
// `opened` fields that nothing cross-checks (challenge_id, tier_index, time digits; ruleset_version was one until VRF-3 was fixed).
#[test]
fn limited_event_file_single_byte_changes_that_still_verify_are_exactly_these_fields() {
    use std::collections::BTreeSet;
    let allowed: BTreeSet<&str> = [
        "event.slot.val",
        "event.account.KEY",
        "event.type.KEY",
        "event.<syntax>",
        "opened.challenge_id.val",
        "opened.tier_index.val",
        "opened.time.val",
    ]
    .into_iter()
    .collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for name in ["design_4_9", "holding_night"] {
        let fx = Fx::load(name);
        let text = fx.text();
        let mut off = 0;
        for line in text.split_inclusive('\n') {
            for p in 0..line.len() {
                for x in [0x01u8, 0x80, 0xff] {
                    let mut f = fx.clone();
                    f.events[off + p] ^= x;
                    let out = go(&f);
                    assert!(!matches!(out, Out::Panic), "{name}: byte {} ^{x:02x} PANIC", off + p);
                    if out.valid() {
                        let r = role(line, p);
                        assert!(allowed.contains(r.as_str()), "{name}: byte {} ^{x:02x} ({r}) still VALID", off + p);
                        seen.insert(r);
                    }
                }
            }
            off += line.len();
        }
    }
    // and each allowed role really occurs (so a tightening of the verifier makes this list shrink, visibly)
    assert_eq!(seen.iter().map(String::as_str).collect::<BTreeSet<_>>(), allowed);
}

#[test]
fn blocked_every_byte_of_the_rules_file_never_panics() {
    for name in VECTORS {
        let fx = Fx::load(name);
        for pos in 0..fx.rules.len() {
            for x in [0x01u8, 0x80, 0xff] {
                let mut f = fx.clone();
                f.rules[pos] ^= x;
                assert!(!matches!(go(&f), Out::Panic), "{name}: rules byte {pos} ^{x:02x} PANIC");
            }
        }
    }
}

fn leaf_paths(v: &serde_json::Value, path: String, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(m) => m.iter().for_each(|(k, x)| leaf_paths(x, format!("{path}/{k}"), out)),
        serde_json::Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| leaf_paths(x, format!("{path}/{i}"), out)),
        _ => out.push(path),
    }
}

/// Every leaf of the rules file moved by one (numbers +1, hex strings first digit changed); returns the leaves that still verify.
fn rules_leaf_survivors(name: &str) -> Vec<String> {
    let fx = Fx::load(name);
    let base: serde_json::Value = serde_json::from_slice(&fx.rules).unwrap();
    let mut ls = vec![];
    leaf_paths(&base, String::new(), &mut ls);
    let mut surv = vec![];
    for l in ls {
        let mut v = base.clone();
        let t = v.pointer_mut(&l).unwrap();
        *t = match t {
            serde_json::Value::Number(n) => serde_json::json!(n.as_u64().unwrap() + 1),
            serde_json::Value::String(s) => {
                let mut c = s.clone().into_bytes();
                c[0] = if c[0] == b'0' { b'1' } else { b'0' };
                serde_json::json!(String::from_utf8(c).unwrap())
            }
            _ => continue,
        };
        let out = go(&fx.with_rules(&serde_json::to_string(&v).unwrap()));
        assert!(!matches!(out, Out::Panic), "{name}: {l} PANIC");
        if out.valid() {
            surv.push(l);
        }
    }
    surv
}

// LIMITED: for the committed account (EURUSD, no overnight in two of three) the rule fields that can move without the verifier
// noticing are exactly these. fee_bps and tier_sizes are never read (ruleset_version IS compared with the opened line since the VRF-3 fix) by the replay (the 0.4% fee is charged by
// open_account, not journaled); the rest are not exercised by the account (other classes, session edges, a notional far below
// the cap, a proof age far below max_age_secs). Every exercised field (leverage_x[0], commission_e6[0], spread, feed id, qty
// step, status, class, id, and holding_fee_e6[0] when an overnight is held) IS detected.
#[test]
fn limited_rule_fields_that_can_change_without_the_verifier_noticing() {
    let mut common: Vec<String> = ["commission_e6", "holding_fee_e6", "leverage_x"].iter().flat_map(|k| (1..8).map(move |i| format!("/{k}/{i}"))).collect();
    common.extend(["/fee_bps", "/markets/0/max_age_secs", "/markets/0/max_notional_usd", "/markets/0/qty_min", "/markets/0/sessions/0/0", "/markets/0/sessions/0/1", "/tier_sizes/0"].map(s));
    let mut with_hold0 = common.clone();
    with_hold0.push(s("/holding_fee_e6/0"));
    for (name, expect) in [("quick_profit", &with_hold0), ("design_4_9", &with_hold0), ("holding_night", &common)] {
        let mut got = rules_leaf_survivors(name);
        let mut want = expect.clone();
        got.sort();
        want.sort();
        assert_eq!(got, want, "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: events file, structured malformed input

#[test]
fn blocked_events_file_structure() {
    let fx = Fx::load("design_4_9");
    let t = fx.text();
    let hdr = "{\"type\":\"header\",\"format\":\"setl8-lev-events\",\"version\":1}\n";
    let lines: Vec<&str> = t.lines().collect();
    let opened = lines[1];
    let ev1 = lines[2];
    let ev2 = lines[3];
    let cases: Vec<(String, Fx, Want)> = vec![
        ("empty file".into(), fx.with_events(""), Want::Input("no header")),
        ("only white space".into(), fx.with_events(" \n\n  \n"), Want::Input("no header")),
        ("header only".into(), fx.with_events(hdr), Want::Input("no `opened` line")),
        ("header version 2".into(), fx.with_events(&t.replacen("\"version\":1}", "\"version\":2}", 1)), Want::Input("events line 1 must be")),
        ("header format other".into(), fx.with_events(&t.replacen("setl8-lev-events", "setl8-lev-event", 1)), Want::Input("events line 1 must be")),
        ("no header, opened first".into(), fx.with_events(&t.replacen(hdr, "", 1)), Want::Input("events line 1 must be")),
        ("UTF-8 BOM before the header".into(), fx.with_events(&format!("\u{feff}{t}")), Want::Input("events line 1")),
        ("invalid UTF-8".into(), Fx { events: [t.as_bytes(), &[0xff, 0xfe, b'\n']].concat(), ..fx.clone() }, Want::Input("not UTF-8")),
        ("NUL byte in a line".into(), fx.with_events(&format!("{t}{{\"type\":\"event\"\0}}\n")), Want::Input("events line")),
        ("a line that is a JSON array".into(), fx.with_events(&format!("{t}[1,2,3]\n")), Want::NotValid),
        ("a line that is a JSON string".into(), fx.with_events(&format!("{t}\"x\"\n")), Want::NotValid),
        ("a line that is null".into(), fx.with_events(&format!("{t}null\n")), Want::NotValid),
        ("an empty object line".into(), fx.with_events(&format!("{t}{{}}\n")), Want::Input("missing field")),
        ("not JSON line".into(), fx.with_events(&format!("{t}this is not json\n")), Want::Input("events line")),
        ("opened twice".into(), fx.with_events(&t.replacen(opened, &format!("{opened}\n{opened}"), 1)), Want::NotValid),
        ("opened last".into(), fx.with_events(&format!("{hdr}{}\n{opened}\n", lines[2..].join("\n"))), Want::ValidBecause("the opened line is found wherever it is; its position is not enforced (info, VRF-4)")),
        ("two headers (the second is an unknown line type)".into(), fx.with_events(&t.replacen(hdr, &format!("{hdr}{hdr}"), 1)), Want::Has(C::UnknownEvent)),
        ("CRLF line endings are tolerated (same content)".into(), fx.with_events(&t.replace('\n', "\r\n")), Want::ValidBecause("lines are trimmed; the bytes that matter are inside the JSON")),
        ("blank lines between records are tolerated".into(), fx.with_events(&t.replace('\n', "\n\n")), Want::ValidBecause("empty lines are skipped; file order is chain order and is unchanged")),
        ("event line twice (duplicate seq)".into(), fx.with_events(&t.replacen(ev1, &format!("{ev1}\n{ev1}"), 1)), Want::NotValid),
        ("event lines swapped (reordered)".into(), fx.with_events(&t.replacen(&format!("{ev1}\n{ev2}"), &format!("{ev2}\n{ev1}"), 1)), Want::NotValid),
        ("event line deleted (sequence gap)".into(), fx.with_events(&t.replacen(&format!("{ev1}\n"), "", 1)), Want::NotValid),
        ("event line of ANOTHER account".into(), fx.with_events(&rep1(&t, &format!("\"account\":\"{}\",\"seq\":1", "CHTMuHmhkobqJ7mkqtUSbBtPmX8cUcewK9JajUV3mds4"), "\"account\":\"3F5qRPtKg8GhGNnbd3qCj6nVJxWsGxq7pvH84okYLAqf\",\"seq\":1")), Want::Input("belongs to another account")),
        ("unknown line type is reported as UnknownEvent".into(), fx.with_events(&format!("{t}{{\"type\":\"mystery\",\"account\":\"CHTMuHmhkobqJ7mkqtUSbBtPmX8cUcewK9JajUV3mds4\"}}\n")), Want::Has(C::UnknownEvent)),
        ("unknown extra field on an event line is ignored".into(), fx.with_events(&rep1(&t, "\"seq\":1,", "\"seq\":1,\"extra\":{\"a\":[1,2,3]},")), Want::ValidBecause("unknown fields are ignored (forward compatibility); the event bytes and the chain are what is checked")),
        ("event hex with a 0x prefix".into(), fx.with_events(&rep1(&t, "\"event\":\"", "\"event\":\"0x")), Want::Input("field event")),
        ("event hex odd length".into(), fx.with_events(&rep1(&t, "\"event\":\"0", "\"event\":\"00")), Want::Input("field event")),
        ("event hex with a non-hex digit".into(), fx.with_events(&rep1(&t, "\"event\":\"0", "\"event\":\"g")), Want::Input("field event")),
        ("event hex empty".into(), fx.with_events(&rep1(&t, &format!("\"event\":\"{}\"", &ev1[ev1.find("\"event\":\"").unwrap() + 9..ev1.find("\",\"proof\"").unwrap()]), "\"event\":\"\"")), Want::NotValid),
        ("event hex doubled (trailing bytes)".into(), fx.with_events(&{ let h = &ev1[ev1.find("\"event\":\"").unwrap() + 9..ev1.find("\",\"proof\"").unwrap()]; rep1(&t, h, &format!("{h}{h}")) }), Want::NotValid),
        ("proof hex with a wrong length (1 byte)".into(), fx.with_events(&rep1(&t, "\"proof\":\"02c6", "\"proof\":\"02")), Want::NotValid),
        ("proof of 1 MB of zeros".into(), fx.with_events(&rep1(&t, "\"proof\":\"02c6d06ac28a", &format!("\"proof\":\"{}", "00".repeat(1 << 20)))), Want::NotValid),
        ("event of 4 MB of zeros".into(), fx.with_events(&rep1(&t, "\"event\":\"0000", &format!("\"event\":\"{}", "00".repeat(2 << 20)))), Want::NotValid),
    ];
    table(cases);
}

#[test]
fn blocked_events_numeric_fields_nan_inf_negative_overflow() {
    let fx = Fx::load("design_4_9");
    let t = fx.text();
    let mut cases: Vec<(String, Fx, Want)> = vec![];
    // seq of the first event line
    for (label, v) in [
        ("seq 0", "0"),
        ("seq u64::MAX", "18446744073709551615"),
        ("seq 2^64", "18446744073709551616"),
        ("seq -1", "-1"),
        ("seq 1.5", "1.5"),
        ("seq 1e0", "1e0"),
        ("seq 1e999", "1e999"),
        ("seq \"1\"", "\"1\""),
        ("seq \"NaN\"", "\"NaN\""),
        ("seq \"inf\"", "\"inf\""),
        ("seq \"-0\"", "\"-0\""),
        ("seq null", "null"),
        ("seq true", "true"),
        ("seq [1]", "[1]"),
        ("seq {}", "{}"),
        ("seq NaN token", "NaN"),
        ("seq Infinity token", "Infinity"),
        ("seq -0", "-0"),
        ("seq 01 (leading zero)", "01"),
        ("seq 1,", "1,"),
    ] {
        let bad = rep1(&t, "\"seq\":1,", &format!("\"seq\":{v},"));
        let want = match v {
            "1e0" => Want::NotValid, // serde_json parses 1e0 as a float: not an unsigned integer
            _ => Want::NotValid,
        };
        cases.push((label.to_string(), fx.with_events(&bad), want));
    }
    // fields whose domain is narrower than their JSON type
    for (label, from, to, want) in [
        ("slot -1", "\"slot\":1001", "\"slot\":-1", Want::Input("field slot")),
        ("slot u64::MAX (informational field)", "\"slot\":1001", "\"slot\":18446744073709551615", Want::ValidBecause("the slot is informational")),
        ("slot 2^64", "\"slot\":1001", "\"slot\":18446744073709551616", Want::Input("field slot")),
        ("slot \"NaN\"", "\"slot\":1001", "\"slot\":\"NaN\"", Want::Input("field slot")),
        ("size_s u64::MAX", "\"size_s\":100000000000", "\"size_s\":18446744073709551615", Want::Input("size_s does not fit i64")),
        ("size_s 2^63", "\"size_s\":100000000000", "\"size_s\":9223372036854775808", Want::Input("size_s does not fit i64")),
        ("size_s -1", "\"size_s\":100000000000", "\"size_s\":-1", Want::Input("field size_s")),
        ("size_s \"NaN\"", "\"size_s\":100000000000", "\"size_s\":\"NaN\"", Want::Input("field size_s")),
        ("size_s 1e999", "\"size_s\":100000000000", "\"size_s\":1e999", Want::Input("events line")),
        ("size_s 1e11 (a float)", "\"size_s\":100000000000", "\"size_s\":1e11", Want::Input("field size_s")),
        ("tier_index 256", "\"tier_index\":8", "\"tier_index\":256", Want::Input("does not fit u8")),
        ("tier_index -0", "\"tier_index\":8", "\"tier_index\":-0", Want::NotValid),
        ("phase 256", "\"phase\":1,\"ruleset", "\"phase\":256,\"ruleset", Want::Input("does not fit u8")),
        ("ruleset_version 65536", "\"ruleset_version\":1", "\"ruleset_version\":65536", Want::Input("does not fit u16")),
        ("challenge_id 2^64", "\"challenge_id\":0", "\"challenge_id\":18446744073709551616", Want::Input("field challenge_id")),
        ("time i64::MAX", "\"time\":1791810000", "\"time\":9223372036854775807", Want::NotValid),
        ("time i64::MIN", "\"time\":1791810000", "\"time\":-9223372036854775808", Want::ValidBecause("design_4_9 holds no overnight: the opened time only starts the rollover cursor (holding_night covers it)")),
        ("time 0", "\"time\":1791810000", "\"time\":0", Want::ValidBecause("no overnight in this account")),
        ("time -1", "\"time\":1791810000", "\"time\":-1", Want::ValidBecause("no overnight in this account")),
        ("time 2^63", "\"time\":1791810000", "\"time\":9223372036854775808", Want::Input("field time")),
        ("time \"1791810000\"", "\"time\":1791810000", "\"time\":\"1791810000\"", Want::Input("field time")),
        ("time 1791810000.5", "\"time\":1791810000", "\"time\":1791810000.5", Want::Input("field time")),
        ("genesis hash 62 chars", "\"genesis_hash\":\"4c63", "\"genesis_hash\":\"4c", Want::Input("field genesis_hash")),
        ("genesis hash 66 chars", "\"genesis_hash\":\"4c63", "\"genesis_hash\":\"4c6300", Want::Input("field genesis_hash")),
        ("genesis hash empty", "\"genesis_hash\":\"4c63ba5f287e151dd65bb3be53b5e6afb5ce8e84dc1005c808d2539e77c4d2c6\"", "\"genesis_hash\":\"\"", Want::Input("field genesis_hash")),
        ("genesis hash all zero", "\"genesis_hash\":\"4c63ba5f287e151dd65bb3be53b5e6afb5ce8e84dc1005c808d2539e77c4d2c6\"", "\"genesis_hash\":\"0000000000000000000000000000000000000000000000000000000000000000\"", Want::Has(C::BadGenesis)),
        ("account base58 with a 0 (invalid alphabet)", "\"account\":\"CHTMuHmhkobqJ7mkqtUSbBtPmX8cUcewK9JajUV3mds4\",\"trader\"", "\"account\":\"0HTMuHmhkobqJ7mkqtUSbBtPmX8cUcewK9JajUV3mds4\",\"trader\"", Want::Input("field account")),
        ("account base58 one char shorter (decodes to another key)", "\"account\":\"CHTMuHmhkobqJ7mkqtUSbBtPmX8cUcewK9JajUV3mds4\",\"trader\"", "\"account\":\"CHTMuHmhkobqJ7mkqtUSbBtPmX8cUcewK9JajUV3mds\",\"trader\"", Want::NotValid),
        ("account empty string", "\"account\":\"CHTMuHmhkobqJ7mkqtUSbBtPmX8cUcewK9JajUV3mds4\",\"trader\"", "\"account\":\"\",\"trader\"", Want::Input("field account")),
        ("trader base58 of 33 bytes", "\"trader\":\"3F5qRPtKg8GhGNnbd3qCj6nVJxWsGxq7pvH84okYLAqf\"", "\"trader\":\"3F5qRPtKg8GhGNnbd3qCj6nVJxWsGxq7pvH84okYLAqfA\"", Want::Input("field trader")),
    ] {
        cases.push((label.to_string(), fx.with_events(&rep1(&t, from, to)), want));
    }
    table(cases);
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: rules file

#[test]
fn blocked_rules_file_structure_and_values() {
    let fx = Fx::load("design_4_9");
    let r = String::from_utf8(fx.rules.clone()).unwrap();
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let cases: Vec<(String, Fx, Want)> = vec![
        ("empty".into(), fx.with_rules(""), Want::Input("rules file:")),
        ("not JSON".into(), fx.with_rules("nope"), Want::Input("rules file:")),
        ("JSON null".into(), fx.with_rules("null"), Want::Input("rules file:")),
        ("JSON array".into(), fx.with_rules("[]"), Want::Input("rules file:")),
        ("empty object".into(), fx.with_rules("{}"), Want::Input("missing field")),
        ("nesting 100000 deep (recursion limit, not a stack overflow)".into(), fx.with_rules(&deep), Want::Input("rules file:")),
        ("version 2".into(), fx.with_rules(&rep1(&r, "\"version\":1", "\"version\":2")), Want::Input("version 2 is not supported")),
        ("version 0".into(), fx.with_rules(&rep1(&r, "\"version\":1", "\"version\":0")), Want::Input("version 0 is not supported")),
        ("version \"1\"".into(), fx.with_rules(&rep1(&r, "\"version\":1", "\"version\":\"1\"")), Want::Input("rules file:")),
        ("version 1.0".into(), fx.with_rules(&rep1(&r, "\"version\":1", "\"version\":1.0")), Want::Input("rules file:")),
        ("leverage_x with 7 entries".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[50,20,20,20,10,5,5]")), Want::Input("leverage_x must have 8 entries")),
        ("leverage_x with 9 entries".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[50,20,20,20,10,5,5,2,1]")), Want::Input("leverage_x must have 8 entries")),
        ("leverage_x with 1,000,000 entries".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", &format!("[{}]", vec!["1"; 1_000_000].join(",")))), Want::Input("leverage_x must have 8 entries")),
        ("leverage_x entry 65536".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[65536,20,20,20,10,5,5,2]")), Want::Input("rules file:")),
        ("leverage_x entry -1".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[-1,20,20,20,10,5,5,2]")), Want::Input("rules file:")),
        ("leverage_x entry \"NaN\"".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[\"NaN\",20,20,20,10,5,5,2]")), Want::Input("rules file:")),
        ("leverage_x entry 1e999".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[1e999,20,20,20,10,5,5,2]")), Want::Input("rules file:")),
        ("leverage_x entry 5.5".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[5.5,20,20,20,10,5,5,2]")), Want::Input("rules file:")),
        ("leverage_x 65535 (a huge but legal value)".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[65535,20,20,20,10,5,5,2]")), Want::NotValid),
        ("leverage_x 0 (division by zero candidate)".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "[0,0,0,0,0,0,0,0]")), Want::NotValid),
        ("holding_fee_e6 entry u32::MAX+1".into(), fx.with_rules(&rep1(&r, "[100,200,400,200,400,400,1000,1600]", "[4294967296,200,400,200,400,400,1000,1600]")), Want::Input("rules file:")),
        ("commission_e6 u32::MAX".into(), fx.with_rules(&rep1(&r, "\"commission_e6\":[0,", "\"commission_e6\":[4294967295,")), Want::NotValid),
        ("fee_bps 65536".into(), fx.with_rules(&rep1(&r, "\"fee_bps\":40", "\"fee_bps\":65536")), Want::Input("rules file:")),
        ("spread_e6 key \"x\"".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"x\":30}")), Want::Input("is not a market id")),
        ("spread_e6 key \"-0\"".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"-0\":30}")), Want::Input("is not a market id")),
        ("spread_e6 key \"NaN\"".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"NaN\":30}")), Want::Input("is not a market id")),
        ("spread_e6 key \"1e0\"".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"1e0\":30}")), Want::Input("is not a market id")),
        ("spread_e6 key \"65536\"".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"65536\":30}")), Want::Input("is not a market id")),
        ("spread_e6 value 65536".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"0\":65536}")), Want::Input("rules file:")),
        ("spread_e6 value 0 (untradable market, decision 64)".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"0\":0}")), Want::NotValid),
        ("spread_e6 value 65535".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"0\":65535}")), Want::NotValid),
        ("spread_e6 of the market missing".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{}")), Want::NotValid),
        ("spread_e6 duplicate key (last wins in serde_json maps)".into(), fx.with_rules(&rep1(&r, "\"spread_e6\":{\"0\":30}", "\"spread_e6\":{\"0\":30,\"0\":31}")), Want::NotValid),
        ("class 8".into(), fx.with_rules(&rep1(&r, "\"class\":0", "\"class\":8")), Want::Input("class 8 is above 7")),
        ("class 255".into(), fx.with_rules(&rep1(&r, "\"class\":0", "\"class\":255")), Want::Input("class 255 is above 7")),
        ("class 256".into(), fx.with_rules(&rep1(&r, "\"class\":0", "\"class\":256")), Want::Input("rules file:")),
        ("class 7 (another leverage)".into(), fx.with_rules(&rep1(&r, "\"class\":0", "\"class\":7")), Want::NotValid),
        ("market appears twice".into(), {
            let m = &r[r.find("\"markets\":[").unwrap() + 11..r.rfind("],\"ruleset_version\"").unwrap()];
            fx.with_rules(&r.replacen(m, &format!("{m},{m}"), 1))
        }, Want::Input("appears twice")),
        ("feed_id 63 hex chars".into(), fx.with_rules(&rep1(&r, "\"feed_id\":\"c6", "\"feed_id\":\"c")), Want::Input("feed_id")),
        ("feed_id 0x prefixed".into(), fx.with_rules(&rep1(&r, "\"feed_id\":\"", "\"feed_id\":\"0x")), Want::Input("feed_id")),
        ("feed_id empty".into(), fx.with_rules(&rep1(&r, "\"feed_id\":\"c6d06ac28a478e9f1f8763e972081243d937c270314c531f9f290451066650ea\"", "\"feed_id\":\"\"")), Want::Input("feed_id")),
        ("feed_id of another market (no sample matches)".into(), fx.with_rules(&rep1(&r, "\"feed_id\":\"c6", "\"feed_id\":\"c7")), Want::NotValid),
        ("sessions [[1,2,3]] (3 numbers)".into(), fx.with_rules(&rep1(&r, "\"sessions\":[[0,10080]]", "\"sessions\":[[0,10080,1]]")), Want::Input("rules file:")),
        ("sessions [[5]] (1 number)".into(), fx.with_rules(&rep1(&r, "\"sessions\":[[0,10080]]", "\"sessions\":[[5]]")), Want::Input("rules file:")),
        ("sessions end before start".into(), fx.with_rules(&rep1(&r, "\"sessions\":[[0,10080]]", "\"sessions\":[[10080,0]]")), Want::NotValid),
        ("sessions empty (never open)".into(), fx.with_rules(&rep1(&r, "\"sessions\":[[0,10080]]", "\"sessions\":[]")), Want::NotValid),
        ("sessions 65535,65535".into(), fx.with_rules(&rep1(&r, "\"sessions\":[[0,10080]]", "\"sessions\":[[65535,65535]]")), Want::NotValid),
        ("sessions 100000 windows".into(), fx.with_rules(&rep1(&r, "\"sessions\":[[0,10080]]", &format!("\"sessions\":[{}]", vec!["[0,10080]"; 100_000].join(",")))), Want::ValidBecause("duplicate identical windows are the same set of open minutes")),
        ("status 0 (disabled)".into(), fx.with_rules(&rep1(&r, "\"status\":1", "\"status\":0")), Want::NotValid),
        ("status 2 (reduce-only)".into(), fx.with_rules(&rep1(&r, "\"status\":1", "\"status\":2")), Want::NotValid),
        ("status 256".into(), fx.with_rules(&rep1(&r, "\"status\":1", "\"status\":256")), Want::Input("rules file:")),
        ("qty_step 0 (division by zero candidate)".into(), fx.with_rules(&rep1(&r, "\"qty_step\":1000", "\"qty_step\":0")), Want::NotValid),
        ("qty_step u64::MAX".into(), fx.with_rules(&rep1(&r, "\"qty_step\":1000", "\"qty_step\":18446744073709551615")), Want::NotValid),
        ("qty_step 2^64".into(), fx.with_rules(&rep1(&r, "\"qty_step\":1000", "\"qty_step\":18446744073709551616")), Want::Input("rules file:")),
        ("max_notional_usd 0".into(), fx.with_rules(&rep1(&r, "\"max_notional_usd\":1000000000000000", "\"max_notional_usd\":0")), Want::NotValid),
        ("max_age_secs 0".into(), fx.with_rules(&rep1(&r, "\"max_age_secs\":60", "\"max_age_secs\":0")), Want::ValidBecause("every proof of the account is from the second of its event; the bound only bites for older proofs")),
        ("max_age_secs 65535".into(), fx.with_rules(&rep1(&r, "\"max_age_secs\":60", "\"max_age_secs\":65535")), Want::ValidBecause("a longer max age does not invalidate proofs that were younger than the smaller one")),
        ("tier_sizes empty".into(), fx.with_rules(&rep1(&r, "\"tier_sizes\":[100000000000]", "\"tier_sizes\":[]")), Want::ValidBecause("tier_sizes is parsed but never read by the replay: see FINDING VRF-3")),
        ("tier_sizes with a negative".into(), fx.with_rules(&rep1(&r, "\"tier_sizes\":[100000000000]", "\"tier_sizes\":[-1]")), Want::Input("rules file:")),
        ("markets empty".into(), fx.with_rules(&rep1(&r, "\"markets\":[", "\"markets\":[],\"x\":[")), Want::NotValid),
        ("unknown top-level field is ignored".into(), fx.with_rules(&rep1(&r, "{\"commission_e6\"", "{\"extra\":{\"deep\":[1,2,3]},\"commission_e6\"")), Want::ValidBecause("unknown fields are ignored by design (verifier-formats.md: other Market fields are ignored)")),
        ("duplicate top-level key".into(), fx.with_rules(&rep1(&r, "{\"commission_e6\"", "{\"fee_bps\":40,\"commission_e6\"")), Want::Input("duplicate field")),
        ("a field with the wrong type (leverage_x object)".into(), fx.with_rules(&rep1(&r, "[50,20,20,20,10,5,5,2]", "{}")), Want::Input("rules file:")),
        ("market id 65536".into(), fx.with_rules(&rep1(&r, "\"id\":0", "\"id\":65536")), Want::Input("rules file:")),
        ("market id -1".into(), fx.with_rules(&rep1(&r, "\"id\":0", "\"id\":-1")), Want::Input("rules file:")),
    ];
    table(cases);
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: archive

fn archive_header(version: u16, rec_len: u16, reserved: u32) -> Vec<u8> {
    let mut h = b"SL8SAMPL".to_vec();
    h.extend_from_slice(&version.to_le_bytes());
    h.extend_from_slice(&rec_len.to_le_bytes());
    h.extend_from_slice(&reserved.to_le_bytes());
    h
}

#[test]
fn blocked_archive_file_structure() {
    let fx = Fx::load("design_4_9");
    let a = fx.archive.clone();
    let with_archive = |b: Vec<u8>| Fx { archive: b, ..fx.clone() };
    let mut flags = a.clone();
    flags[16 + 3] = 1;
    let mut cases: Vec<(String, Fx, Want)> = vec![
        ("empty".into(), with_archive(vec![]), Want::Input("does not start with the SL8SAMPL header")),
        ("magic only".into(), with_archive(b"SL8SAMPL".to_vec()), Want::Input("archive truncated at byte 0")),
        ("lower-case magic".into(), with_archive([b"sl8sampl".as_slice(), &a[8..]].concat()), Want::Input("does not start with the SL8SAMPL header")),
        ("header only (no samples at all)".into(), with_archive(a[..16].to_vec()), Want::NotValid),
        ("two headers only".into(), with_archive([&a[..16], &a[..16]].concat()), Want::NotValid),
        ("header version 0".into(), with_archive([archive_header(0, 68, 0), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("header version 2".into(), with_archive([archive_header(2, 68, 0), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("header version 65535".into(), with_archive([archive_header(65535, 68, 0), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("record_len 0".into(), with_archive([archive_header(1, 0, 0), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("record_len 67".into(), with_archive([archive_header(1, 67, 0), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("record_len 69".into(), with_archive([archive_header(1, 69, 0), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("record_len 65535 (a huge length field)".into(), with_archive([archive_header(1, 65535, 0), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("reserved 1".into(), with_archive([archive_header(1, 68, 1), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("reserved u32::MAX".into(), with_archive([archive_header(1, 68, u32::MAX), a[16..].to_vec()].concat()), Want::Input("bad archive header")),
        ("a record with flags = 1".into(), with_archive(flags), Want::Input("bad archive record")),
        ("one trailing byte".into(), with_archive([a.clone(), vec![0]].concat()), Want::Input("archive truncated")),
        ("a trailing 67 bytes".into(), with_archive([a.clone(), vec![0; 67]].concat()), Want::Input("archive truncated")),
        ("a trailing all-zero record (flags 0, market 0, value 0)".into(), with_archive([a.clone(), vec![0; 68]].concat()), Want::Has(C::ArchiveRecordInvalid)),
        ("a trailing header (empty extra segment)".into(), with_archive([a.clone(), a[..16].to_vec()].concat()), Want::ValidBecause("an empty segment adds no records; the logical stream (what the hashes cover) is unchanged")),
        ("a header between every record (segments of one record)".into(), with_archive({
            let mut o = a[..16].to_vec();
            for r in a[16..].chunks(68) {
                o.extend_from_slice(r);
                o.extend_from_slice(&a[..16]);
            }
            o
        }), Want::ValidBecause("segment headers are excluded from the logical stream by design (archive.rs)")),
        ("the whole archive twice (second copy as new segment)".into(), with_archive([a.clone(), a.clone()].concat()), Want::Has(C::ArchiveRecordInvalid)),
        ("2 MB of 0xff".into(), with_archive(vec![0xff; 2 << 20]), Want::Input("does not start")),
        ("magic then 2 MB of zeros".into(), with_archive([a[..16].to_vec(), vec![0; 2 << 20]].concat()), Want::NotValid),
    ];
    // the second archive is parsed by the same code
    cases.push(("second archive: garbage".into(), Fx { second: Some(b"garbage".to_vec()), ..fx.clone() }, Want::Input("second archive: archive does not start")));
    cases.push(("second archive: empty".into(), Fx { second: Some(vec![]), ..fx.clone() }, Want::Input("second archive:")));
    cases.push(("second archive: bad record".into(), Fx { second: Some({ let mut b = a.clone(); b[16 + 3] = 7; b }), ..fx.clone() }, Want::Input("second archive: bad archive record")));
    table(cases);
}

/// A bundle with the archive replaced and every commitment recomputed, so the hash checks pass and the SEMANTIC checks have to
/// do the work.
fn with_samples(b: &Bundle, f: impl FnOnce(&mut Vec<setl8_verify::archive::Sample>)) -> Fx {
    let mut b = b.clone();
    f(&mut b.samples);
    b.refresh_commitments();
    Fx::of(&b)
}

#[test]
fn blocked_archive_samples_duplicated_unsorted_negative_with_consistent_hashes() {
    let b = close_case(600);
    let n = b.samples.len();
    assert!(n >= 2);
    let cases: Vec<(String, Fx, Want)> = vec![
        ("control: refreshed commitments, nothing changed".into(), with_samples(&b, |_| {}), Want::ValidBecause("the generator's own output")),
        ("every sample duplicated in place".into(), with_samples(&b, |s| *s = s.iter().flat_map(|x| [*x, *x]).collect()), Want::NotValid),
        ("the first sample duplicated at the end".into(), with_samples(&b, |s| s.push(s[0])), Want::Has(C::ArchiveRecordInvalid)),
        ("samples in reverse order".into(), with_samples(&b, |s| s.reverse()), Want::NotValid),
        ("samples swapped (unsorted)".into(), with_samples(&b, |s| s.swap(0, 1)), Want::NotValid),
        ("one sample removed".into(), with_samples(&b, |s| { s.remove(0); }), Want::NotValid),
        ("all samples removed".into(), with_samples(&b, |s| s.clear()), Want::NotValid),
        ("value -1 (negative price)".into(), with_samples(&b, |s| s[0].value = -1), Want::NotValid),
        ("value 0".into(), with_samples(&b, |s| s[0].value = 0), Want::NotValid),
        ("value i128::MIN".into(), with_samples(&b, |s| s[0].value = i128::MIN), Want::NotValid),
        ("value i128::MAX".into(), with_samples(&b, |s| s[0].value = i128::MAX), Want::NotValid),
        ("value u64::MAX * 1e6 (the largest e12 price)".into(), with_samples(&b, |s| s[0].value = i128::from(u64::MAX) * 1_000_000), Want::NotValid),
        ("value one above the largest e12 price".into(), with_samples(&b, |s| s[0].value = (i128::from(u64::MAX) + 1) * 1_000_000), Want::NotValid),
        ("every value negative".into(), with_samples(&b, |s| s.iter_mut().for_each(|x| x.value = -x.value)), Want::NotValid),
        ("source timestamp 0".into(), with_samples(&b, |s| s[0].ts_ns = 0), Want::NotValid),
        ("source timestamp u64::MAX".into(), with_samples(&b, |s| s[0].ts_ns = u64::MAX), Want::NotValid),
        ("receive time before the source time".into(), with_samples(&b, |s| s[0].recv_ns = 0), Want::ValidBecause(RECV_UNUSED)),
        ("receive time u64::MAX".into(), with_samples(&b, |s| s[0].recv_ns = u64::MAX), Want::ValidBecause(RECV_UNUSED)),
        ("market 65535 (no such market)".into(), with_samples(&b, |s| s[0].market = 65535), Want::NotValid),
        ("source 9".into(), with_samples(&b, |s| s[0].source = 9), Want::NotValid),
        ("a blob digest on a source-2 sample".into(), with_samples(&b, |s| s[0].digest = [7; 32]), Want::ValidBecause("the digest is compared only for source 3 (replay.rs:340); a stray digest on another source is not read")),
        ("a sample of a market the account never traded".into(), with_samples(&b, |s| { let mut x = s[0]; x.market = 7; s.push(x) }), Want::Has(C::ArchiveRecordInvalid)), // P25b: a copy of the FIRST record's second at the end is out of the stream's order
    ];
    table(cases);
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: intents file

#[test]
fn blocked_intents_file_structure() {
    let fx = Fx::load("design_4_9");
    let i = String::from_utf8(fx.intents.clone()).unwrap();
    let hdr = "{\"format\":\"setl8-lev-intents\",\"version\":1}\n";
    let line1 = i.lines().nth(1).unwrap().to_string();
    let sig = &line1[line1.find("\"sig\":\"").unwrap() + 7..line1.rfind("\"}").unwrap()];
    let order = &line1[line1.find("\"order\":\"").unwrap() + 9..line1.find("\",\"sig\"").unwrap()];
    let with = |t: &str| fx.with_intents(t.as_bytes());
    let cases: Vec<(String, Fx, Want)> = vec![
        ("empty".into(), with(""), Want::Input("no header line")),
        ("header only".into(), with(hdr), Want::NotValid),
        ("header version 2".into(), with(&i.replacen("\"version\":1", "\"version\":2", 1)), Want::Input("intents header must be")),
        ("header format other".into(), with(&i.replacen("setl8-lev-intents", "x", 1)), Want::Input("intents header must be")),
        ("header is not JSON".into(), with("hello\n"), Want::Input("intents header:")),
        ("invalid UTF-8 as the header".into(), fx.with_intents(&[0xff, 0xfe, b'\n']), Want::Input("intents header:")),
        ("invalid UTF-8 in a data line after the last committed line".into(), fx.with_intents(&[i.as_bytes(), &[0xff, 0xfe, b'\n']].concat()), Want::ValidBecause(UNCOMMITTED_TAIL)),
        ("nonce 0".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":0", 1)), Want::NotValid),
        ("nonce u32::MAX".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":4294967295", 1)), Want::NotValid),
        ("nonce 2^32".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":4294967296", 1)), Want::NotValid),
        ("nonce -1".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":-1", 1)), Want::NotValid),
        ("nonce 1.5".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":1.5", 1)), Want::NotValid),
        ("nonce \"1\"".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":\"1\"", 1)), Want::NotValid),
        ("nonce \"NaN\"".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":\"NaN\"", 1)), Want::NotValid),
        ("nonce 1e999".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":1e999", 1)), Want::NotValid),
        ("nonce null".into(), with(&i.replacen("\"nonce\":1", "\"nonce\":null", 1)), Want::NotValid),
        ("order of 47 bytes".into(), with(&i.replacen(order, &order[..94], 1)), Want::NotValid),
        ("order of 49 bytes".into(), with(&i.replacen(order, &format!("{order}00"), 1)), Want::NotValid),
        ("order empty".into(), with(&i.replacen(order, "", 1)), Want::NotValid),
        ("order with a non-hex digit".into(), with(&i.replacen(order, &format!("zz{}", &order[2..]), 1)), Want::NotValid),
        ("order kind 9".into(), with(&i.replacen(order, &format!("09{}", &order[2..]), 1)), Want::NotValid),
        ("order side 2".into(), with(&i.replacen(order, &format!("{}02{}", &order[..6], &order[8..]), 1)), Want::NotValid),
        ("order qty u64::MAX".into(), with(&i.replacen(order, &format!("{}ffffffffffffffff{}", &order[..8], &order[24..]), 1)), Want::NotValid),
        ("sig of 63 bytes".into(), with(&i.replacen(sig, &sig[..126], 1)), Want::NotValid),
        ("sig of 65 bytes".into(), with(&i.replacen(sig, &format!("{sig}00"), 1)), Want::NotValid),
        ("sig empty".into(), with(&i.replacen(sig, "", 1)), Want::NotValid),
        ("sig missing".into(), with(&i.replacen(&format!(",\"sig\":\"{sig}\""), "", 1)), Want::NotValid),
        ("unknown extra field is ignored by the parser".into(), with(&i.replacen("\"nonce\":1", "\"extra\":[1],\"nonce\":1", 1)), Want::NotValid), // the line bytes changed: the commitment fails
        ("nonce 1 twice, the copy after the last committed line".into(), with(&format!("{i}{line1}\n")), Want::ValidBecause(UNCOMMITTED_TAIL)),
        ("the same line 1000 times after the last committed line".into(), with(&format!("{i}{}", format!("{line1}\n").repeat(1000))), Want::ValidBecause(UNCOMMITTED_TAIL)),
        ("data line that is not JSON after the last committed line".into(), with(&format!("{i}not json\n")), Want::ValidBecause(UNCOMMITTED_TAIL)),
        ("data line that is an empty object after the last committed line".into(), with(&format!("{i}{{}}\n")), Want::ValidBecause(UNCOMMITTED_TAIL)),
        ("CRLF line endings (bytes changed, the prefix hash no longer matches)".into(), with(&i.replace('\n', "\r\n")), Want::NotValid),
    ];
    table(cases);
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: snapshot

#[test]
fn blocked_snapshot_structure_and_each_field() {
    let fx = Fx::load("design_4_9");
    let snap = String::from_utf8(fx.snapshot.clone().unwrap()).unwrap();
    let mut cases: Vec<(String, Fx, Want)> = vec![
        ("empty".into(), fx.with_snapshot(""), Want::Input("snapshot:")),
        ("{}".into(), fx.with_snapshot("{}"), Want::Input("snapshot:")),
        ("[]".into(), fx.with_snapshot("[]"), Want::Input("snapshot:")),
        ("null".into(), fx.with_snapshot("null"), Want::Input("snapshot:")),
        ("version 2".into(), fx.with_snapshot(&rep1(&snap, "\"version\":1", "\"version\":2")), Want::Input("snapshot version 2 is not supported")),
        ("b as a string".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":\"97935260000\"")), Want::Input("snapshot:")),
        ("b as NaN string".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":\"NaN\"")), Want::Input("snapshot:")),
        ("b 2^63".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":9223372036854775808")), Want::Input("snapshot:")),
        ("b 1e999".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":1e999")), Want::Input("snapshot:")),
        ("b float".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":97935260000.0")), Want::Input("snapshot:")),
        ("b i64::MIN".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":-9223372036854775808")), Want::Has(C::StateMismatch)),
        ("b i64::MAX".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":9223372036854775807")), Want::Has(C::StateMismatch)),
        ("b -0".into(), fx.with_snapshot(&rep1(&snap, "\"b\":97935260000", "\"b\":-0")), Want::Input("snapshot:")),
        ("seq -1".into(), fx.with_snapshot(&rep1(&snap, "\"seq\":9", "\"seq\":-1")), Want::Input("snapshot:")),
        ("seq u64::MAX".into(), fx.with_snapshot(&rep1(&snap, "\"seq\":9", "\"seq\":18446744073709551615")), Want::Has(C::SequenceGap)),
        ("seq 2^64".into(), fx.with_snapshot(&rep1(&snap, "\"seq\":9", "\"seq\":18446744073709551616")), Want::Input("snapshot:")),
        ("latch 256".into(), fx.with_snapshot(&rep1(&snap, "\"latch\":0", "\"latch\":256")), Want::Input("snapshot:")),
        ("trades_closed 2^32".into(), fx.with_snapshot(&rep1(&snap, "\"trades_closed\":1", "\"trades_closed\":4294967296")), Want::Input("snapshot:")),
        ("journal_hash 63 chars".into(), fx.with_snapshot(&rep1(&snap, "\"journal_hash\":\"fe", "\"journal_hash\":\"f")), Want::Input("snapshot journal_hash")),
        ("journal_hash empty".into(), fx.with_snapshot(&rep1(&snap, "\"journal_hash\":\"fe5f3b1c69cb61ec86efdaf165afcef89dcb4fcbf2636e726cba80ba153ddce6\"", "\"journal_hash\":\"\"")), Want::Input("snapshot journal_hash")),
        ("journal_hash all zero".into(), fx.with_snapshot(&rep1(&snap, "fe5f3b1c69cb61ec86efdaf165afcef89dcb4fcbf2636e726cba80ba153ddce6", &"0".repeat(64))), Want::Has(C::HashChain)),
        ("duplicate key".into(), fx.with_snapshot(&rep1(&snap, "{\"b\"", "{\"seq\":9,\"b\"")), Want::Input("duplicate field")),
        ("unknown extra field is ignored".into(), fx.with_snapshot(&rep1(&snap, "{\"b\"", "{\"extra\":1,\"b\"")), Want::ValidBecause("unknown fields are ignored; every stored field of the account is still compared")),
        ("hex in upper case (same 32 bytes)".into(), fx.with_snapshot(&rep1(&snap, "fe5f3b1c69cb61ec86efdaf165afcef89dcb4fcbf2636e726cba80ba153ddce6", "FE5F3B1C69CB61EC86EFDAF165AFCEF89DCB4FCBF2636E726CBA80BA153DDCE6")), Want::ValidBecause("hex is decoded case-insensitively; the 32 bytes are identical, only the text differs")),
    ];
    // every field of the stored account, changed by one, is a StateMismatch (BLOCKED: each field is compared)
    let v: serde_json::Value = serde_json::from_str(&snap).unwrap();
    for (k, val) in v.as_object().unwrap() {
        if k == "version" || k == "journal_hash" {
            continue;
        }
        let mut m = v.clone();
        let n = val.as_i64().unwrap();
        m[k] = serde_json::json!(n + 1);
        // the stored seq is compared with the replayed head: a larger seq is a gap in the log, every other field a state mismatch
        cases.push((format!("field {k} + 1"), fx.with_snapshot(&m.to_string()), Want::Has(if k == "seq" { C::SequenceGap } else { C::StateMismatch })));
    }
    table(cases);
}

// ---------------------------------------------------------------------------------------------------------------------
// as_of_seq and the second archive

#[test]
fn blocked_as_of_seq_extremes() {
    let fx = Fx::load("design_4_9");
    for as_of in [0u64, 1, 2, 8, 9, 10, 1 << 32, u64::MAX] {
        for snap in [true, false] {
            let f = Fx { as_of: Some(as_of), ..if snap { fx.clone() } else { fx.no_snapshot() } };
            let out = go(&f);
            assert!(!matches!(out, Out::Panic), "as_of {as_of} snapshot {snap}: PANIC");
            // a prefix replay is a statement about the prefix; it must never be an input error for a number that parses
            assert!(matches!(out, Out::Rep(_)), "as_of {as_of}: {}", out.desc());
        }
    }
    // seq 0 replays nothing: still a coherent report with state seq 0
    match go(&Fx { as_of: Some(0), ..fx.clone() }) {
        Out::Rep(r) => assert_eq!(r.state.seq, 0),
        o => panic!("{}", o.desc()),
    }
    // beyond the log: the same as the whole log
    match (go(&Fx { as_of: Some(u64::MAX), ..fx.no_snapshot() }), go(&fx.no_snapshot())) {
        (Out::Rep(a), Out::Rep(b)) => assert_eq!((a.state.seq, a.state.journal_hash.clone()), (b.state.seq, b.state.journal_hash.clone())),
        _ => panic!(),
    }
}

#[test]
fn blocked_second_archive_attacks() {
    let b = close_case(600);
    let good = b.archive_bytes();
    let mut f = Fx::of(&b).no_snapshot();
    // control: the same archive twice agrees
    f.second = Some(good.clone());
    check("second = first", &go(&f), &Want::ValidBecause("two recorders that saw the same stream"));
    // a changed price in the second archive is a disagreement
    let mut s2 = b.clone();
    s2.samples[0].value += 1_000_000;
    f.second = Some(s2.archive_bytes());
    check("second differs by 1 e12 unit", &go(&f), &Want::Has(C::ArchiveDisagreement));
    // every truncation of the second archive: never a panic, never a false VALID unless it only dropped white space (it cannot: binary)
    for n in 0..good.len() {
        f.second = Some(good[..n].to_vec());
        let out = go(&f);
        assert!(!matches!(out, Out::Panic), "second archive cut to {n}: PANIC");
        let boundary = n >= 16 && (n - 16) % 68 == 0;
        if !boundary {
            assert!(matches!(out, Out::Input(_)), "second archive cut to {n} (not a record boundary): {}", out.desc());
        }
    }
    // a second archive that stops at a record boundary: fewer samples than the first. This is the "dropped sample" case of DESIGN 13.2
    f.second = Some(good[..good.len() - 68].to_vec());
    let out = go(&f);
    assert!(!matches!(out, Out::Panic));
    let _ = out.desc();
    // negative / huge values in the second archive
    for v in [-1i128, 0, i128::MIN, i128::MAX] {
        let mut s3 = b.clone();
        s3.samples[0].value = v;
        f.second = Some(s3.archive_bytes());
        let out = go(&f);
        assert!(!matches!(out, Out::Panic), "second archive value {v}: PANIC");
        assert!(!out.valid(), "second archive value {v}: VALID");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the CLI: exit code 2 with a message, never a panic message

fn cli(args: &[&str], files: &[(&str, Vec<u8>)]) -> (i32, String) {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let map: std::collections::HashMap<String, Vec<u8>> = files.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let mut out = String::new();
    let code = setl8_verify::cli::run(&argv, &|p| map.get(p).cloned().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file")), &mut out);
    (code, out)
}

#[test]
fn blocked_cli_exit_codes_for_malformed_arguments_and_files() {
    let fx = Fx::load("design_4_9");
    let files = vec![("e", fx.events.clone()), ("r", fx.rules.clone()), ("a", fx.archive.clone()), ("i", fx.intents.clone()), ("s", fx.snapshot.clone().unwrap())];
    let base = ["account", "--events", "e", "--rules", "r", "--archive", "a", "--intents", "i"];
    assert_eq!(cli(&base, &files).0, 0);
    let with = |extra: &[&str]| -> (i32, String) {
        let mut v: Vec<&str> = base.to_vec();
        v.extend_from_slice(extra);
        cli(&v, &files)
    };
    for bad in ["-1", "18446744073709551616", "1e3", " 5", "+5", "0x10", "NaN", "inf", "", "1.0", "-0"] {
        let (code, out) = with(&["--as-of-seq", bad]);
        // "+5" parses as 5 in Rust's u64 parser: accepted; every other string must be refused with exit code 2
        if bad == "+5" {
            assert_eq!(code, 0, "{out}");
        } else {
            assert_eq!(code, 2, "--as-of-seq {bad:?}: {out}");
            assert!(out.starts_with("error: --as-of-seq needs an unsigned integer"), "{out}");
        }
    }
    assert_eq!(with(&["--as-of-seq", "18446744073709551615"]).0, 0);
    assert_eq!(with(&["--snapshot", "s"]).0, 0);
    assert_eq!(with(&["--snapshot", "missing"]).0, 2);
    assert_eq!(with(&["--second-archive", "missing"]).0, 2);
    assert_eq!(with(&["--second-archive", "a"]).0, 0);
    assert_eq!(with(&["--second-archive", "e"]).0, 2); // an events file given as an archive
    assert_eq!(with(&["--bogus"]).0, 2);
    assert_eq!(with(&["--events"]).0, 2);
    assert_eq!(cli(&["watch"], &files).0, 2);
    assert_eq!(cli(&["account"], &files).0, 2);
    // swapped files: every wrong file in every slot
    let names = ["e", "r", "a", "i", "s"];
    for x in 0..4 {
        for (y, file) in names.iter().enumerate().take(4) {
            if x == y {
                continue;
            }
            let mut slots = ["e", "r", "a", "i"];
            slots[x] = file;
            let (code, out) = cli(&["account", "--events", slots[0], "--rules", slots[1], "--archive", slots[2], "--intents", slots[3]], &files);
            assert_eq!(code, 2, "slot {x} holds file {y}: {out}");
            assert!(out.starts_with("error:"), "{out}");
            assert!(!out.contains("panicked"), "{out}");
        }
    }
}

#[test]
fn blocked_the_real_binary_exits_2_with_a_message_and_never_panics() {
    use std::process::Command;
    let fx = Fx::load("design_4_9");
    let dir = std::env::temp_dir().join(format!("sl8-adv-inputs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let w = |n: &str, b: &[u8]| {
        std::fs::write(dir.join(n), b).unwrap();
        dir.join(n).to_string_lossy().to_string()
    };
    let (e, r, a, i, s) = (w("e", &fx.events), w("r", &fx.rules), w("a", &fx.archive), w("i", &fx.intents), w("s", fx.snapshot.as_ref().unwrap()));
    let run = |e: &str, r: &str, a: &str, i: &str, s: &str| {
        let o = Command::new(env!("CARGO_BIN_EXE_setl8-verify")).args(["account", "--events", e, "--rules", r, "--archive", a, "--intents", i, "--snapshot", s]).output().unwrap();
        (o.status.code(), String::from_utf8_lossy(&o.stdout).to_string(), String::from_utf8_lossy(&o.stderr).to_string())
    };
    assert_eq!(run(&e, &r, &a, &i, &s).0, Some(0));
    let trunc_a = w("a2", &fx.archive[..fx.archive.len() - 3]);
    let trunc_e = w("e2", &fx.events[..700]);
    let huge = w("big", &vec![b'['; 5_000_000]);
    let zero = w("zero", &[0u8; 4096]);
    let empty = w("empty", b"");
    for (label, args) in [
        ("truncated archive", run(&e, &r, &trunc_a, &i, &s)),
        ("truncated events", run(&trunc_e, &r, &a, &i, &s)),
        ("5 MB of '[' as rules", run(&e, &huge, &a, &i, &s)),
        ("zeros as events", run(&zero, &r, &a, &i, &s)),
        ("empty snapshot", run(&e, &r, &a, &i, &empty)),
        ("a directory as a file", run(&dir.to_string_lossy(), &r, &a, &i, &s)),
    ] {
        assert_eq!(args.0, Some(2), "{label}: {} {}", args.1, args.2);
        assert!(args.1.starts_with("error:"), "{label}: {}", args.1);
        assert!(!args.2.contains("panicked"), "{label}: {}", args.2);
    }
    // a tampered snapshot is a MISMATCH (exit 1), not an input error
    let bad_snap = w("s2", String::from_utf8_lossy(fx.snapshot.as_ref().unwrap()).replacen("\"b\":97935260000", "\"b\":97935260001", 1).as_bytes());
    let o = run(&e, &r, &a, &i, &bad_snap);
    assert_eq!(o.0, Some(1), "{}", o.1);
    assert!(o.1.contains("[StateMismatch]"), "{}", o.1);
}

// ---------------------------------------------------------------------------------------------------------------------
// resource use: a hostile file may be big; the verifier must stay roughly linear

#[test]
fn blocked_large_hostile_files_finish_quickly() {
    use std::time::Instant;
    let fx = Fx::load("design_4_9");
    let t = Instant::now();
    // 40,000 intent lines with distinct nonces, then the same 40,000 again (every duplicate has to find its first occurrence)
    let line1 = String::from_utf8_lossy(&fx.intents).lines().nth(1).unwrap().to_string();
    let uniq: String = (2..40_002u32).map(|n| line1.replacen("\"nonce\":1,", &format!("\"nonce\":{n},"), 1) + "\n").collect();
    let many = [fx.intents.clone(), uniq.clone().into_bytes(), uniq.into_bytes()].concat();
    let out = go(&fx.with_intents(&many));
    let dup_secs = t.elapsed().as_secs_f64();
    assert!(!matches!(out, Out::Panic), "{}", out.desc());
    // 200,000 archive records (13.6 MB)
    let t = Instant::now();
    let mut big = fx.archive.clone();
    for _ in 0..200_000 {
        big.extend_from_slice(&fx.archive[16..16 + 68]);
    }
    let out = go(&Fx { archive: big, ..fx.clone() });
    let arch_secs = t.elapsed().as_secs_f64();
    assert!(!matches!(out, Out::Panic), "{}", out.desc());
    // 50,000 event lines of an unknown type
    let t = Instant::now();
    let mut ev = fx.text();
    for _ in 0..50_000 {
        ev.push_str("{\"type\":\"mystery\"}\n");
    }
    let out = go(&fx.with_events(&ev));
    let ev_secs = t.elapsed().as_secs_f64();
    assert!(!matches!(out, Out::Panic), "{}", out.desc());
    println!("timings (debug build): 40k + 40k duplicate intents {dup_secs:.2}s, 200k archive records {arch_secs:.2}s, 50k unknown event lines {ev_secs:.2}s");
    assert!(dup_secs < 60.0 && arch_secs < 60.0 && ev_secs < 60.0, "dup {dup_secs} arch {arch_secs} ev {ev_secs}");
}

// ---------------------------------------------------------------------------------------------------------------------
// ED25519: the verifier's check of the trader's intents (intents.rs:118, ed25519-dalek 1.0.1, plain `verify`)

const L_LE: [u8; 32] = [0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10];
const IDENTITY: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 1;
    b
};

fn add_le(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in 0..32 {
        let v = u16::from(a[i]) + u16::from(b[i]) + carry;
        out[i] = (v & 0xff) as u8;
        carry = v >> 8;
    }
    assert_eq!(carry, 0);
    out
}

/// The standard small-order encodings (identity, order 2, order 4 (two), order 8 (four)), plus two non-canonical ones.
fn small_order_keys() -> Vec<[u8; 32]> {
    let hex = |s: &str| -> [u8; 32] { setl8_verify::hexutil::decode_array::<32>(s).unwrap() };
    [
        "0100000000000000000000000000000000000000000000000000000000000000",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    ]
    .iter()
    .map(|h| hex(h))
    .filter(|k| ed25519_dalek::PublicKey::from_bytes(k).is_ok())
    .collect()
}

fn off_curve() -> [u8; 32] {
    (2u16..2000)
        .map(|y| {
            let mut b = [0u8; 32];
            b[..2].copy_from_slice(&y.to_le_bytes());
            b
        })
        .find(|b| ed25519_dalek::PublicKey::from_bytes(b).is_err())
        .expect("an off-curve y")
}

fn with_intent_sigs(b: &Bundle, f: impl FnOnce(&mut Vec<IntentRec>)) -> Fx {
    let mut b = b.clone();
    f(&mut b.intents);
    b.refresh_commitments();
    Fx::of(&b)
}

#[test]
fn blocked_intent_signature_attacks_through_the_whole_verifier() {
    let b = close_case(600);
    assert!(b.intents.len() >= 2);
    let kp = trader_keypair();
    let acct = account_key();
    let sign_with = |seed: u8, nonce: u32, order: &[u8]| -> [u8; 64] {
        use ed25519_dalek::Signer;
        let secret = ed25519_dalek::SecretKey::from_bytes(&[seed; 32]).unwrap();
        let public: ed25519_dalek::PublicKey = (&secret).into();
        ed25519_dalek::Keypair { secret, public }.sign(&setl8_verify::intents::signing_message(&acct, nonce, order)).to_bytes()
    };
    let good = b.intents[0].sig;
    let s: [u8; 32] = good[32..].try_into().unwrap();
    let s_plus_l = add_le(&s, &L_LE);
    assert_eq!(s_plus_l[31] & 0xe0, 0, "S + L must stay below 2^253 so that only a canonical-S check can refuse it");
    let mut malleated = good;
    malleated[32..].copy_from_slice(&s_plus_l);
    let bad_r = off_curve();
    let mut r_off = good;
    r_off[..32].copy_from_slice(&bad_r);
    let mut r_id = [0u8; 64];
    r_id[..32].copy_from_slice(&IDENTITY);
    let order0 = b.intents[0].order.clone();
    let n0 = b.intents[0].nonce;
    let mut top = good;
    top[63] |= 0x80;
    let mut cases: Vec<(String, Fx, Want)> = vec![
        ("control".into(), with_intent_sigs(&b, |_| {}), Want::ValidBecause("honest")),
        ("S + L (malleated), hashes refreshed".into(), with_intent_sigs(&b, |i| i[0].sig = malleated), Want::Has(C::BadIntentSignature)),
        ("S + L on the SECOND intent".into(), with_intent_sigs(&b, |i| {
            let s2: [u8; 32] = i[1].sig[32..].try_into().unwrap();
            let m = add_le(&s2, &L_LE);
            i[1].sig[32..].copy_from_slice(&m);
        }), Want::Has(C::BadIntentSignature)),
        ("S with the top bit set".into(), with_intent_sigs(&b, |i| i[0].sig = top), Want::Has(C::BadIntentSignature)),
        ("all-zero signature".into(), with_intent_sigs(&b, |i| i[0].sig = [0; 64]), Want::Has(C::BadIntentSignature)),
        ("all-0xff signature".into(), with_intent_sigs(&b, |i| i[0].sig = [0xff; 64]), Want::Has(C::BadIntentSignature)),
        ("R = identity, S = 0".into(), with_intent_sigs(&b, |i| i[0].sig = r_id), Want::Has(C::BadIntentSignature)),
        ("R not on the curve".into(), with_intent_sigs(&b, |i| i[0].sig = r_off), Want::Has(C::BadIntentSignature)),
        ("valid signature over a DIFFERENT message (the second intent's signature)".into(), with_intent_sigs(&b, |i| i[0].sig = i[1].sig), Want::Has(C::BadIntentSignature)),
        ("valid signature of ANOTHER KEY over the right message".into(), with_intent_sigs(&b, |i| i[0].sig = sign_with(0x77, n0, &order0)), Want::Has(C::BadIntentSignature)),
        ("valid signature by the trader over the right order but the wrong nonce".into(), with_intent_sigs(&b, |i| i[0].sig = sign_with(0x42, n0 + 100, &order0)), Want::Has(C::BadIntentSignature)),
    ];
    // every single bit of one signature
    for bit in 0..512usize {
        let mut sig = good;
        sig[bit / 8] ^= 1 << (bit % 8);
        cases.push((format!("bit {bit} flipped"), with_intent_sigs(&b, |i| i[0].sig = sig), Want::Has(C::BadIntentSignature)));
    }
    // the trader key in the opened line is not on the curve / is the wrong key
    let mut off_trader = b.clone();
    for l in &mut off_trader.lines {
        if let GLine::Opened(o) = l {
            o.trader = bad_r;
        }
    }
    cases.push(("trader key not on the curve".into(), Fx::of(&off_trader), Want::Has(C::BadIntentSignature)));
    let mut other_trader = b.clone();
    for l in &mut other_trader.lines {
        if let GLine::Opened(o) = l {
            o.trader = [0x33; 32];
        }
    }
    cases.push(("trader key is another (possibly invalid) key".into(), Fx::of(&other_trader), Want::Has(C::BadIntentSignature)));
    let _ = kp;
    table(cases);
}

#[test]
fn blocked_verify_signature_unit_lengths_and_garbage() {
    let kp = trader_keypair();
    let acct = account_key();
    let order = [0u8; 48];
    use ed25519_dalek::Signer;
    let sig = kp.sign(&setl8_verify::intents::signing_message(&acct, 1, &order)).to_bytes();
    let pk = kp.public.to_bytes();
    assert!(setl8_verify::intents::verify_signature(&pk, &acct, 1, &order, &sig));
    for len in [0usize, 1, 32, 63] {
        assert!(!setl8_verify::intents::verify_signature(&pk, &acct, 1, &order, &sig[..len]), "length {len}");
    }
    assert!(!setl8_verify::intents::verify_signature(&pk, &acct, 1, &order, &[sig.as_slice(), &[0u8]].concat()));
    assert!(!setl8_verify::intents::verify_signature(&pk, &acct, 1, &order, &[0u8; 1000]));
    // wrong nonce / account / order
    assert!(!setl8_verify::intents::verify_signature(&pk, &acct, 2, &order, &sig));
    assert!(!setl8_verify::intents::verify_signature(&pk, &[1; 32], 1, &order, &sig));
    assert!(!setl8_verify::intents::verify_signature(&pk, &acct, 1, &[1u8; 48], &sig));
    assert!(!setl8_verify::intents::verify_signature(&pk, &acct, 1, &order[..47], &sig));
}

#[test]
fn pin_the_verifier_refuses_a_non_canonical_s_because_dalek_1_0_1_checks_it_in_plain_verify() {
    let kp = trader_keypair();
    let acct = account_key();
    let order = [0u8; 48];
    use ed25519_dalek::Signer;
    let sig = kp.sign(&setl8_verify::intents::signing_message(&acct, 7, &order)).to_bytes();
    let pk = kp.public.to_bytes();
    let s: [u8; 32] = sig[32..].try_into().unwrap();
    for k in 1..=3 {
        let mut m = sig;
        let mut v = s;
        for _ in 0..k {
            v = add_le(&v, &L_LE);
        }
        m[32..].copy_from_slice(&v);
        assert!(!setl8_verify::intents::verify_signature(&pk, &acct, 7, &order, &m), "S + {k}L");
    }
}

// FINDING VRF-2
#[test]
fn finding_vrf_2_verify_signature_accepts_any_message_for_a_small_order_trader_key() {
    let acct = account_key();
    let order = [0u8; 48];
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&IDENTITY); // R = identity, S = 0: no secret involved
    for key in small_order_keys() {
        let accepted: Vec<u32> = (1u32..500).filter(|n| setl8_verify::intents::verify_signature(&key, &acct, *n, &order, &sig)).collect();
        assert!(accepted.is_empty(), "key {key:02x?}: an unsigned message was accepted for nonces {:?}", &accepted[..accepted.len().min(5)]);
    }
}

// FINDING VRF-2
#[test]
fn finding_vrf_2_the_whole_verifier_says_valid_for_unsigned_intents_of_a_small_order_trader() {
    let b = close_case(600);
    let mut forged = b.clone();
    for l in &mut forged.lines {
        if let GLine::Opened(o) = l {
            o.trader = IDENTITY;
        }
    }
    forged.intents.iter_mut().for_each(|r| {
        r.sig = [0; 64];
        r.sig[..32].copy_from_slice(&IDENTITY);
    });
    forged.refresh_commitments();
    let out = go(&Fx::of(&forged));
    assert!(out.classes().contains(&C::BadIntentSignature), "verdict: {}", out.desc());
}

#[test]
fn vrf_2_fixed_the_verifier_refuses_every_small_order_trader_key() {
    // after the fix (verify_strict in intents.rs) the zero-S "signature" is refused for every nonce and every small-order key
    let acct = account_key();
    let order = [0u8; 48];
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&IDENTITY);
    for n in 1u32..=50 {
        assert!(!setl8_verify::intents::verify_signature(&IDENTITY, &acct, n, &order, &sig), "nonce {n}");
    }
    for key in small_order_keys() {
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&key);
        assert!(!(1u32..2000).any(|n| setl8_verify::intents::verify_signature(&key, &acct, n, &order, &sig)), "{key:02x?} verified");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// FINDING VRF-3: the opened line is not cross-checked with the rules file

// FINDING VRF-3 (FIXED in crates/verifier/src/verify.rs: the rules file's ruleset_version must equal the opened line's)
#[test]
fn finding_vrf_3_a_rules_file_of_another_version_is_not_noticed() {
    let fx = Fx::load("design_4_9");
    let r = String::from_utf8(fx.rules.clone()).unwrap();
    // the account says it was opened under rule set version 1; the file given is version 2
    let out = go(&fx.with_rules(&rep1(&r, "\"ruleset_version\":1", "\"ruleset_version\":2")));
    assert!(!out.valid(), "the verifier replayed rule set 2 against an account of rule set 1 and said VALID");
    // and the other way round: the opened line claims version 9
    let t = fx.text();
    let out = go(&fx.with_events(&rep1(&t, "\"ruleset_version\":1", "\"ruleset_version\":9")));
    assert!(!out.valid(), "an account of rule set 9 was replayed with rule set 1 and said VALID");
}

#[test]
fn pin_vrf_3_what_is_still_not_cross_checked() {
    let fx = Fx::load("design_4_9");
    let t = fx.text();
    let r = String::from_utf8(fx.rules.clone()).unwrap();
    assert!(!go(&fx.with_rules(&rep1(&r, "\"ruleset_version\":1", "\"ruleset_version\":2"))).valid());
    assert!(!go(&fx.with_events(&rep1(&t, "\"ruleset_version\":1", "\"ruleset_version\":9"))).valid());
    // tier_index is read, range-checked as a u8 and then never used; tier_sizes is never read (the committed vectors even have
    // tier_index 8 with a one-entry tier_sizes)
    assert!(go(&fx.with_events(&rep1(&t, "\"tier_index\":8", "\"tier_index\":200"))).valid());
    // a forged size_s is NOT free: the floors and the balance in the log depend on S, so the replay disagrees (here without the snapshot)
    let big = fx.with_events(&rep1(&t, "\"size_s\":100000000000", "\"size_s\":200000000000")).no_snapshot();
    let out = go(&big);
    assert!(!out.valid() && out.classes().contains(&C::FloorMismatch), "{}", out.desc());
}

#[test]
fn blocked_opened_time_after_the_rollover_is_a_fee_mismatch_and_far_past_values_are_pinned() {
    let fx = Fx::load("holding_night");
    let t = fx.text();
    let opened_time = t.lines().nth(1).unwrap().split("\"time\":").nth(1).unwrap().trim_end_matches('}').to_string();
    let with_time = |v: &str| fx.with_events(&rep1(&t, &format!("\"time\":{opened_time}}}"), &format!("\"time\":{v}}}")));
    // an open time AFTER the position was opened / after the rollover hides the night: caught
    for forged in ["9223372036854775807", "1791899999"] {
        let out = go(&with_time(forged));
        assert!(!matches!(out, Out::Panic), "opened.time {forged}: PANIC");
        assert!(out.classes().contains(&C::FeeMismatch), "opened.time {forged}: {}", out.desc());
    }
    // LIMITED: any time before the first event is accepted, including 0, -1 and i64::MIN (the field is a lower bound, never compared
    // with anything else; VRF-3 observation)
    for forged in ["0", "1", "-1", "-9223372036854775808", "1791810001"] {
        let out = go(&with_time(forged));
        assert!(out.valid(), "opened.time {forged}: {}", out.desc());
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Source 3 (GMX signed blob): there is NO signature check on the signed update anywhere in the repo (journal-types/proof.rs: "the
// publisher signature is NOT checked here"; DESIGN 3.3 and R-7). The verifier compares only the sha256 digest of the blob.

fn source3_case() -> Bundle {
    let mut spec = Spec::base();
    spec.markets[0].source = 3;
    let t2 = T_1400 + 600;
    let script = vec![
        Script::Sample { market: 0, t: T_1400, mid: eur(10_800) },
        Script::Order { t: T_1400, intent: market_order(0, true, 200_000_000_000, T_1400) },
        Script::Sample { market: 0, t: t2, mid: eur(10_830) },
        Script::Order { t: t2, intent: order(setl8_verify::intents::IntentKind::Close, 0, 0, 0, 0, 0, 0, 0, t2) },
    ];
    Gen::new(spec).run(&script)
}

#[test]
fn limited_source_3_blob_digest_is_compared_but_the_blob_signature_is_not_checked_anywhere() {
    let b = source3_case();
    assert!(go(&Fx::of(&b)).valid());
    // (1) the archive's digest changed, the proofs not: the proof no longer matches a recorded sample (BLOCKED)
    let tampered_archive = with_samples(&b, |s| s[0].digest = [9; 32]);
    check("archive digest changed", &go(&tampered_archive), &Want::Has(C::ArchiveMissing));
    // (2) a proof whose blob digest is not the archive's: the same finding
    let mut p = b.clone();
    for l in &mut p.lines {
        if let GLine::Event { proof: Some(pr), .. } = l {
            if pr.signed_update.len() == 32 {
                pr.signed_update = vec![8; 32];
                break;
            }
        }
    }
    check("proof digest changed", &go(&Fx::of(&p)), &Want::NotValid);
    // (3) LIMITED: archive digest AND every proof digest replaced by arbitrary bytes, the chain recomputed: VALID. Nothing ties the
    // 32 bytes to a publisher signature (the blob is not even in the archive). Whoever writes the log and the archive chooses them.
    let mut all = b.clone();
    for s in &mut all.samples {
        s.digest = [0xAB; 32];
    }
    for l in &mut all.lines {
        if let GLine::Event { proof: Some(pr), .. } = l {
            if pr.signed_update.len() == 32 {
                pr.signed_update = vec![0xAB; 32];
            }
        }
    }
    all.refresh_commitments();
    let out = go(&Fx::of(&all));
    assert!(out.valid(), "{}", out.desc());
}
