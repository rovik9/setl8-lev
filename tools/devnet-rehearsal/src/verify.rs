//! S5: the public verifier on the chain's own events. The five input files are built here: the events log from the program's
//! `emit_cpi!` events of every transaction that touched the account, the rules file from the chain's `RuleSet` and `Market`
//! accounts, the snapshot from the stored `TradingAccount`, and the evaluator's sample archive and intents file. Then copies are
//! tampered with and the verifier must say so.
use setl8_lev_trading::state::TradingAccount;
use setl8_verify::{hexutil, Inputs, Report, Verdict};

pub const ARCHIVE_HEADER: usize = 16;
pub const RECORD: usize = 68;

pub struct Files {
    pub events: String,
    pub rules: String,
    pub archive: Vec<u8>,
    pub intents: Vec<u8>,
    pub snapshot: String,
}

/// The snapshot file (verifier-formats.md section 2) from the stored account.
pub fn snapshot_json(a: &TradingAccount) -> String {
    serde_json::json!({
        "version": 1, "b": a.b, "q_tally": a.q_tally, "latch": a.latch, "tight_on": a.tight_on, "r": a.r, "g_peak": a.g_peak, "l_level": a.l_level,
        "lock_until": a.lock_until, "seq": a.seq, "journal_hash": hexutil::encode(&a.journal_hash), "flat": a.flat, "phase": a.phase,
        "status": a.status, "trades_closed": a.trades_closed
    })
    .to_string()
}

pub fn run(f: &Files) -> Result<Report, String> {
    setl8_verify::verify_account(&Inputs {
        events: f.events.as_bytes(),
        rules: f.rules.as_bytes(),
        archive: &f.archive,
        intents: &f.intents,
        snapshot: Some(f.snapshot.as_bytes()),
        second_archive: None,
        as_of_seq: None,
    })
    .map_err(|e| format!("verifier input: {e}"))
}

pub fn is_valid(r: &Report) -> bool {
    r.verdict == Verdict::Valid
}

pub fn classes(r: &Report) -> String {
    r.classes().iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(",")
}

/// The archive with the record at index `idx` removed (the evaluator "forgot" a sample).
pub fn drop_record(archive: &[u8], idx: usize) -> Option<Vec<u8>> {
    let n = records(archive);
    if idx >= n {
        return None;
    }
    let at = ARCHIVE_HEADER + idx * RECORD;
    let mut out = archive[..at].to_vec();
    out.extend_from_slice(&archive[at + RECORD..]);
    Some(out)
}

/// The archive with the price of record `idx` changed by `delta_e12` (the quantized value is in e18: 1e6 per e12 unit).
pub fn change_price(archive: &[u8], idx: usize, delta_e12: i128) -> Option<Vec<u8>> {
    if idx >= records(archive) {
        return None;
    }
    let at = ARCHIVE_HEADER + idx * RECORD + 20;
    let v = i128::from_le_bytes(archive[at..at + 16].try_into().ok()?);
    let mut out = archive.to_vec();
    out[at..at + 16].copy_from_slice(&(v + delta_e12 * 1_000_000).to_le_bytes());
    Some(out)
}

pub fn records(archive: &[u8]) -> usize {
    archive.len().saturating_sub(ARCHIVE_HEADER) / RECORD
}

/// The intents file with the first intent line repeated (the same signed order submitted again).
pub fn repeat_first_intent(intents: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(intents).ok()?;
    let mut lines = text.lines();
    let header = lines.next()?;
    let first = lines.next()?;
    let mut out = String::new();
    out.push_str(header);
    out.push('\n');
    out.push_str(first);
    out.push('\n');
    out.push_str(first);
    out.push('\n');
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    Some(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::{AcctRef, Live};
    use crate::stories::{self, OfflineNet, BTC, ETH, SOL};
    use anchor_lang::solana_program::pubkey::Pubkey;
    use evaluator::AccountParams;
    use setl8_verify::log;
    use solana_keypair::Keypair;
    use solana_signer::Signer;

    const SIZE: i64 = 2_500_000_000;
    const T0: i64 = 1_800_000_000;

    /// A locally produced account: the honest events text, rules file, archive, intents and snapshot of a pass story.
    fn local(mock: bool) -> Files {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/launch.json");
        let launch = crate::launch::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        let trader = Keypair::new();
        let account = Pubkey::new_unique();
        let params = AccountParams { account: account.to_bytes(), trader: trader.pubkey().to_bytes(), challenge_id: 0, size_micro: SIZE, phase: 1, tier_index: 0, opened_time: T0 };
        let mut live = Live::new(launch.rules_cfg(mock), params.clone(), &trader.to_bytes(), &[BTC, ETH, SOL]).unwrap();
        let a = AcctRef { name: "t".into(), trader: trader.pubkey(), challenge_id: 0 };
        let mut net = OfflineNet::default();
        stories::pass_story(&mut live, &mut net, &a, BTC.0, SIZE).unwrap();
        let rec = log::OpenedRec {
            account: params.account,
            trader: params.trader,
            challenge_id: 0,
            size_s: SIZE as u64,
            tier_index: 0,
            phase: 1,
            ruleset_version: 1,
            genesis_hash: journal_types::genesis_hash(&journal_types::NativeSha256, &params.account),
            time: T0,
        };
        let mut events = log::header_line();
        events.push_str(&log::opened_line(&rec));
        let mut slot = 1000u64;
        for b in &net.batches {
            let mut seq = b.batch.first_seq;
            for ev in &b.batch.events {
                let proof = ev.proof_index().and_then(|i| b.batch.proofs.get(i)).map(|p| p.to_bytes().unwrap()).unwrap_or_default();
                events.push_str(&log::event_line(&params.account, seq, &ev.to_bytes(), &proof, slot));
                seq += 1;
                slot += 1;
            }
        }
        // the rules file from the launch rules (the live run builds it from the chain's accounts; same fields)
        let rules = {
            let rs = &launch.ruleset;
            let spread: serde_json::Map<String, serde_json::Value> = launch.markets.iter().map(|m| (m.args.market_id.to_string(), serde_json::json!(rs.spread_e6[usize::from(m.args.market_id)]))).collect();
            let markets: Vec<serde_json::Value> = launch
                .markets
                .iter()
                .map(|m| {
                    let p = &m.args.params;
                    serde_json::json!({"id": m.args.market_id, "class": m.args.class, "feed_id": hexutil::encode(&p.feed_id), "max_age_secs": p.max_age_secs, "status": p.status,
                        "qty_step": p.qty_step, "qty_min": p.qty_min, "max_notional_usd": p.max_notional_usd,
                        "sessions": p.sessions[..usize::from(p.session_count)].iter().map(|s| serde_json::json!([s.start_min, s.end_min])).collect::<Vec<_>>()})
                })
                .collect();
            serde_json::json!({"version": 1, "ruleset_version": 1, "fee_bps": rs.fee_bps, "leverage_x": rs.leverage_x, "holding_fee_e6": rs.holding_fee_e6,
                "commission_e6": rs.commission_e6, "tier_sizes": &rs.tier_size[..usize::from(rs.tier_count)], "spread_e6": spread, "markets": markets})
            .to_string()
        };
        let st = live.ev.state();
        let last = net.batches.last().unwrap();
        let snapshot = serde_json::json!({"version": 1, "b": st.b, "q_tally": st.q_tally, "latch": u8::from(st.latch), "tight_on": u8::from(st.tight_on), "r": st.r, "g_peak": st.g_peak,
            "l_level": st.l_level, "lock_until": st.lock_until, "seq": live.ev.seq(), "journal_hash": hexutil::encode(&live.ev.head()), "flat": u8::from(last.account_after.flat),
            "phase": 1, "status": 0, "trades_closed": live.ev.trades_closed()})
        .to_string();
        Files { events, rules, archive: live.ev.archive_bytes().to_vec(), intents: live.ev.intents_bytes().to_vec(), snapshot }
    }

    #[test]
    fn the_honest_local_account_is_valid_and_each_tampered_copy_is_flagged() {
        let f = local(false);
        let rep = run(&f).expect("inputs parse");
        assert!(is_valid(&rep), "honest: {}", classes(&rep));
        let n = records(&f.archive);
        assert!(n > 10, "{n} samples");
        // a sample dropped from the middle: the samples hash the Meta committed no longer matches
        let dropped = Files { archive: drop_record(&f.archive, n / 2).unwrap(), events: f.events.clone(), rules: f.rules.clone(), intents: f.intents.clone(), snapshot: f.snapshot.clone() };
        let r = run(&dropped).expect("parses");
        assert!(!is_valid(&r), "a dropped sample must be flagged");
        // a price changed
        let changed = Files { archive: change_price(&f.archive, n / 2, 1_000_000_000).unwrap(), events: f.events.clone(), rules: f.rules.clone(), intents: f.intents.clone(), snapshot: f.snapshot.clone() };
        let r = run(&changed).expect("parses");
        assert!(!is_valid(&r), "a changed price must be flagged");
        // an intent submitted twice
        let again = Files { intents: repeat_first_intent(&f.intents).unwrap(), events: f.events.clone(), rules: f.rules.clone(), archive: f.archive.clone(), snapshot: f.snapshot.clone() };
        // (an input error is also a detection)
        if let Ok(r) = run(&again) {
            assert!(!is_valid(&r), "a repeated intent must be flagged");
        }
    }

    /// The finding of the rehearsal's first run: the verifier does not know the mock source (255) that the `localnet` build accepts
    /// on chain, so an account traded with mock proofs is never VALID. The rehearsal therefore trades its verification-grade accounts
    /// with the markets' real source format and keeps mock proofs for the accounts it does not verify.
    #[test]
    fn mock_proofs_are_not_judged_valid_by_the_verifier() {
        let f = local(true);
        let rep = run(&f).expect("inputs parse");
        assert!(!is_valid(&rep));
        assert!(classes(&rep).contains("ProofShape"), "{}", classes(&rep));
    }

    #[test]
    fn the_tampering_helpers_refuse_out_of_range_records() {
        let f = local(false);
        let n = records(&f.archive);
        assert!(drop_record(&f.archive, n).is_none());
        assert!(change_price(&f.archive, n, 1).is_none());
        assert!(repeat_first_intent(b"only a header\n").is_none());
        assert_eq!(drop_record(&f.archive, 0).unwrap().len(), f.archive.len() - RECORD);
    }
}
