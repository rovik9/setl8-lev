//! The event log (format version 1): JSON Lines mirroring what the program emits.
//!
//! Line 1 is the header `{"type":"header","format":"setl8-lev-events","version":1}`. Every other line has a `"type"`:
//! `opened`, `event` (the default when `type` is absent), `pass_requested`, `pass_confirmed`, `pass_rejected`,
//! `pass_vetoed`, `phase_advanced`, `pass_finalized`, `account_failed`, `violation_fail`, and (M9c) `account_held` /
//! `hold_released`, and (P25b, decision 68) `pass_cancelled`, `account_abandoned`, `market_feed_changed`, `market_status_changed`.
//! File order is chain order.
//! Hashes and raw bytes are lower-case hex; accounts and wallets are base58.
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedRec {
    pub account: [u8; 32],
    pub trader: [u8; 32],
    pub challenge_id: u64,
    pub size_s: u64,
    pub tier_index: u8,
    pub phase: u8,
    pub ruleset_version: u16,
    pub genesis_hash: [u8; 32],
    pub time: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PassKind {
    Requested,
    Confirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Opened(OpenedRec),
    /// An `EventLog` record: the raw canonical bytes.
    Event { seq: u64, event: Vec<u8>, proof: Vec<u8>, slot: u64 },
    /// `PassRequested` / `PassConfirmed`: the numbers the program wrote into the event.
    Pass { kind: PassKind, phase: u8, amount: u64, b: i64, seq: u64, journal_hash: [u8; 32], time_or_eligible: i64 },
    PassRejected { phase: u8, reason: u8 },
    PassVetoed { phase: u8, slot: u64 },
    PhaseAdvanced { from_phase: u8, to_phase: u8, time: i64 },
    PassFinalized { phase: u8, to_phase: u8, amount: u64, request_id: u64, time: i64 },
    AccountFailed { reason: u8, b: i64, seq: u64, journal_hash: [u8; 32] },
    ViolationFail { evidence_hash: [u8; 32] },
    /// M9c: `hold_account` / `release_hold` (the program's `AccountHeld` / `HoldReleased`), dated with the block time of the
    /// transaction. From that second on the account is held (no exposure increase) / no longer held.
    Held { time: i64 },
    HoldReleased { time: i64 },
    /// P25b (F-24): the program's `PassCancelled` (the trader withdrew an unanswered request): the pending pass is resolved.
    PassCancelled { phase: u8, time: i64 },
    /// P25b (F-24): the program's `AccountAbandoned` (the vault flipped the record after 7 idle days): the account ended outside the journal.
    AccountAbandoned { time: i64 },
    /// P25b (F-19): the program's `MarketFeedChanged` (an admin remapped a market's feed). Market lines carry no `account`: they are
    /// program-wide and sit in the log at the position the indexer saw them.
    MarketFeedChanged { market: u16, old_feed: [u8; 32], new_feed: [u8; 32] },
    /// P25b (F-19): an admin `update_market` that changed a market's status (the indexer reads the old and new status off the Market account).
    MarketStatusChanged { market: u16, old_status: u8, new_status: u8 },
    /// A line whose `type` this version does not know.
    Unknown { type_name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAt {
    /// 1-based line number in the file.
    pub line_no: usize,
    pub account: Option<[u8; 32]>,
    pub line: Line,
}

fn field<'a>(v: &'a Value, name: &str) -> Result<&'a Value, String> {
    v.get(name).ok_or_else(|| format!("missing field {name}"))
}
fn u64f(v: &Value, name: &str) -> Result<u64, String> {
    field(v, name)?.as_u64().ok_or_else(|| format!("field {name} is not an unsigned integer"))
}
fn i64f(v: &Value, name: &str) -> Result<i64, String> {
    field(v, name)?.as_i64().ok_or_else(|| format!("field {name} is not an integer"))
}
fn u8f(v: &Value, name: &str) -> Result<u8, String> {
    u8::try_from(u64f(v, name)?).map_err(|_| format!("field {name} does not fit u8"))
}
fn u16f(v: &Value, name: &str) -> Result<u16, String> {
    u16::try_from(u64f(v, name)?).map_err(|_| format!("field {name} does not fit u16"))
}
fn strf<'a>(v: &'a Value, name: &str) -> Result<&'a str, String> {
    field(v, name)?.as_str().ok_or_else(|| format!("field {name} is not a string"))
}
fn hexf<const N: usize>(v: &Value, name: &str) -> Result<[u8; N], String> {
    crate::hexutil::decode_array::<N>(strf(v, name)?).map_err(|e| format!("field {name}: {e}"))
}
fn keyf(v: &Value, name: &str) -> Result<[u8; 32], String> {
    let s = strf(v, name)?;
    let bytes = bs58::decode(s).into_vec().map_err(|e| format!("field {name}: {e}"))?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| format!("field {name}: expected 32 bytes"))
}

pub fn base58(key: &[u8; 32]) -> String {
    bs58::encode(key).into_string()
}

fn parse_line(v: &Value) -> Result<(Option<[u8; 32]>, Line), String> {
    let ty = match v.get("type") {
        None => "event",
        Some(t) => t.as_str().ok_or("type is not a string")?,
    };
    let account = v.get("account").map(|_| keyf(v, "account")).transpose()?;
    let line = match ty {
        "opened" => Line::Opened(OpenedRec {
            account: keyf(v, "account")?,
            trader: keyf(v, "trader")?,
            challenge_id: u64f(v, "challenge_id")?,
            size_s: u64f(v, "size_s")?,
            tier_index: u8f(v, "tier_index")?,
            phase: u8f(v, "phase")?,
            ruleset_version: u16f(v, "ruleset_version")?,
            genesis_hash: hexf::<32>(v, "genesis_hash")?,
            time: i64f(v, "time")?,
        }),
        "event" => {
            let event = crate::hexutil::decode(strf(v, "event")?).map_err(|e| format!("field event: {e}"))?;
            let proof = crate::hexutil::decode(strf(v, "proof")?).map_err(|e| format!("field proof: {e}"))?;
            Line::Event { seq: u64f(v, "seq")?, event, proof, slot: u64f(v, "slot")? }
        }
        "pass_requested" | "pass_confirmed" => Line::Pass {
            kind: if ty == "pass_requested" { PassKind::Requested } else { PassKind::Confirmed },
            phase: u8f(v, "phase")?,
            amount: u64f(v, "amount")?,
            b: i64f(v, "b")?,
            seq: u64f(v, "seq")?,
            journal_hash: hexf::<32>(v, "journal_hash")?,
            time_or_eligible: i64f(v, if ty == "pass_requested" { "time" } else { "eligible_at" })?,
        },
        "pass_rejected" => Line::PassRejected { phase: u8f(v, "phase")?, reason: u8f(v, "reason")? },
        "pass_vetoed" => Line::PassVetoed { phase: u8f(v, "phase")?, slot: u64f(v, "slot")? },
        "phase_advanced" => Line::PhaseAdvanced { from_phase: u8f(v, "from_phase")?, to_phase: u8f(v, "to_phase")?, time: i64f(v, "time")? },
        "pass_finalized" => Line::PassFinalized {
            phase: u8f(v, "phase")?,
            to_phase: u8f(v, "to_phase")?,
            amount: u64f(v, "amount")?,
            request_id: u64f(v, "request_id")?,
            time: i64f(v, "time")?,
        },
        "account_failed" => Line::AccountFailed { reason: u8f(v, "reason")?, b: i64f(v, "b")?, seq: u64f(v, "seq")?, journal_hash: hexf::<32>(v, "journal_hash")? },
        "violation_fail" => Line::ViolationFail { evidence_hash: hexf::<32>(v, "evidence_hash")? },
        "account_held" => Line::Held { time: i64f(v, "time")? },
        "hold_released" => Line::HoldReleased { time: i64f(v, "time")? },
        "pass_cancelled" => Line::PassCancelled { phase: u8f(v, "phase")?, time: i64f(v, "time")? },
        "account_abandoned" => Line::AccountAbandoned { time: i64f(v, "time")? },
        "market_feed_changed" => Line::MarketFeedChanged { market: u16f(v, "market_id")?, old_feed: hexf::<32>(v, "old_feed")?, new_feed: hexf::<32>(v, "new_feed")? },
        "market_status_changed" => Line::MarketStatusChanged { market: u16f(v, "market_id")?, old_status: u8f(v, "old_status")?, new_status: u8f(v, "new_status")? },
        other => Line::Unknown { type_name: other.to_string() },
    };
    Ok((account, line))
}

/// Parses the whole log. A malformed line is a hard input error (exit code 2): the file is not in this format.
pub fn parse(text: &str) -> Result<Vec<LineAt>, String> {
    let mut out = Vec::new();
    let mut header_seen = false;
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(t).map_err(|e| format!("events line {line_no}: {e}"))?;
        if !header_seen {
            let ok = v.get("type").and_then(Value::as_str) == Some("header")
                && v.get("format").and_then(Value::as_str) == Some("setl8-lev-events")
                && v.get("version").and_then(Value::as_u64) == Some(1);
            if !ok {
                return Err("events line 1 must be {\"type\":\"header\",\"format\":\"setl8-lev-events\",\"version\":1}".into());
            }
            header_seen = true;
            continue;
        }
        let (account, line) = parse_line(&v).map_err(|e| format!("events line {line_no}: {e}"))?;
        out.push(LineAt { line_no, account, line });
    }
    if !header_seen {
        return Err("events file has no header line".into());
    }
    Ok(out)
}

pub fn header_line() -> String {
    "{\"type\":\"header\",\"format\":\"setl8-lev-events\",\"version\":1}\n".to_string()
}

pub fn event_line(account: &[u8; 32], seq: u64, event: &[u8], proof: &[u8], slot: u64) -> String {
    format!(
        "{{\"type\":\"event\",\"account\":\"{}\",\"seq\":{seq},\"event\":\"{}\",\"proof\":\"{}\",\"slot\":{slot}}}\n",
        base58(account),
        crate::hexutil::encode(event),
        crate::hexutil::encode(proof)
    )
}

/// The `account_held` / `hold_released` line of an indexer.
pub fn hold_line(account: &[u8; 32], time: i64, held: bool) -> String {
    format!("{{\"type\":\"{}\",\"account\":\"{}\",\"time\":{time}}}\n", if held { "account_held" } else { "hold_released" }, base58(account))
}

/// The `market_feed_changed` line of an indexer (no `account`: market events are program-wide).
pub fn market_feed_line(market: u16, old_feed: &[u8; 32], new_feed: &[u8; 32]) -> String {
    format!("{{\"type\":\"market_feed_changed\",\"market_id\":{market},\"old_feed\":\"{}\",\"new_feed\":\"{}\"}}\n", crate::hexutil::encode(old_feed), crate::hexutil::encode(new_feed))
}

/// The `market_status_changed` line of an indexer.
pub fn market_status_line(market: u16, old_status: u8, new_status: u8) -> String {
    format!("{{\"type\":\"market_status_changed\",\"market_id\":{market},\"old_status\":{old_status},\"new_status\":{new_status}}}\n")
}

pub fn opened_line(o: &OpenedRec) -> String {
    format!(
        "{{\"type\":\"opened\",\"account\":\"{}\",\"trader\":\"{}\",\"challenge_id\":{},\"size_s\":{},\"tier_index\":{},\"phase\":{},\"ruleset_version\":{},\"genesis_hash\":\"{}\",\"time\":{}}}\n",
        base58(&o.account),
        base58(&o.trader),
        o.challenge_id,
        o.size_s,
        o.tier_index,
        o.phase,
        o.ruleset_version,
        crate::hexutil::encode(&o.genesis_hash),
        o.time
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_and_opened_round_trip() {
        let acct = [9u8; 32];
        let o = OpenedRec { account: acct, trader: [3; 32], challenge_id: 4, size_s: 100, tier_index: 8, phase: 1, ruleset_version: 7, genesis_hash: [5; 32], time: 1000 };
        let text = format!("{}{}{}", header_line(), opened_line(&o), event_line(&acct, 1, &[1, 2, 3], &[], 77));
        let lines = parse(&text).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line, Line::Opened(o));
        assert_eq!(lines[0].line_no, 2);
        assert_eq!(lines[1].line, Line::Event { seq: 1, event: vec![1, 2, 3], proof: vec![], slot: 77 });
        assert_eq!(lines[1].account, Some(acct));
    }

    #[test]
    fn integer_fields_are_range_checked() {
        let acct = base58(&[9; 32]);
        let h = crate::hexutil::encode(&[1u8; 32]);
        let line = |tier: &str, rv: &str| {
            format!(
                "{}{{\"type\":\"opened\",\"account\":\"{acct}\",\"trader\":\"{acct}\",\"challenge_id\":0,\"size_s\":1,\"tier_index\":{tier},\"phase\":1,\"ruleset_version\":{rv},\"genesis_hash\":\"{h}\",\"time\":1}}\n",
                header_line()
            )
        };
        assert!(parse(&line("255", "65535")).is_ok());
        assert!(parse(&line("256", "1")).is_err());
        assert!(parse(&line("1", "65536")).is_err());
        assert!(parse(&line("-1", "1")).is_err());
    }

    #[test]
    fn typed_lines_and_unknown() {
        let h = crate::hexutil::encode(&[1u8; 32]);
        let acct = base58(&[9; 32]);
        let text = format!(
            "{}{{\"type\":\"pass_requested\",\"account\":\"{acct}\",\"phase\":2,\"amount\":8,\"b\":9,\"seq\":3,\"journal_hash\":\"{h}\",\"time\":5}}\n\
             {{\"type\":\"pass_confirmed\",\"account\":\"{acct}\",\"phase\":2,\"amount\":8,\"b\":9,\"seq\":3,\"journal_hash\":\"{h}\",\"eligible_at\":6}}\n\
             {{\"type\":\"pass_rejected\",\"account\":\"{acct}\",\"phase\":2,\"reason\":1}}\n\
             {{\"type\":\"pass_vetoed\",\"account\":\"{acct}\",\"phase\":2,\"admin_a\":\"x\",\"admin_b\":\"y\",\"slot\":4}}\n\
             {{\"type\":\"phase_advanced\",\"account\":\"{acct}\",\"from_phase\":1,\"to_phase\":2,\"time\":7}}\n\
             {{\"type\":\"pass_finalized\",\"account\":\"{acct}\",\"phase\":2,\"to_phase\":3,\"amount\":8,\"request_id\":1,\"tally_count\":1,\"tally_total\":8,\"window_total\":8,\"time\":9}}\n\
             {{\"type\":\"account_failed\",\"account\":\"{acct}\",\"reason\":0,\"b\":5,\"seq\":6,\"journal_hash\":\"{h}\"}}\n\
             {{\"type\":\"violation_fail\",\"account\":\"{acct}\",\"trader\":\"{acct}\",\"evidence_hash\":\"{h}\"}}\n\
             {{\"type\":\"mystery\",\"account\":\"{acct}\"}}\n",
            header_line()
        );
        let l = parse(&text).unwrap();
        assert_eq!(l.len(), 9);
        assert!(matches!(l[0].line, Line::Pass { kind: PassKind::Requested, phase: 2, seq: 3, .. }));
        assert!(matches!(l[1].line, Line::Pass { kind: PassKind::Confirmed, time_or_eligible: 6, .. }));
        assert!(matches!(l[5].line, Line::PassFinalized { to_phase: 3, request_id: 1, .. }));
        assert!(matches!(l[8].line, Line::Unknown { ref type_name } if type_name == "mystery"));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(parse("").is_err());
        assert!(parse("{\"type\":\"header\",\"format\":\"x\",\"version\":1}\n").is_err());
        assert!(parse(&format!("{}not json\n", header_line())).is_err());
        assert!(parse(&format!("{}{{\"type\":\"event\"}}\n", header_line())).is_err());
        assert!(parse(&format!("{}{{\"type\":1}}\n", header_line())).is_err());
        assert!(parse(&format!("{}{{\"seq\":1,\"event\":\"zz\",\"proof\":\"\",\"slot\":0,\"account\":\"{}\"}}\n", header_line(), base58(&[1; 32]))).is_err());
    }
}
