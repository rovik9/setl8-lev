//! The program's events as the chain delivers them (`emit_cpi!` self-calls: inner instructions to our own program with the event
//! tag, the event discriminator and the Borsh body) and their translation into the verifier's event-log lines
//! (handover/verifier-formats.md section 1). Nothing here knows the evaluator: the log is built from the chain alone.
use anchor_lang::{AnchorDeserialize, Discriminator};
use setl8_lev_trading::events::{
    AccountAbandoned, AccountFailed, AccountHeld, AccountReset, EventLog, HoldReleased, Opened, PassCancelled, PassConfirmed, PassFinalized,
    PassRejected, PassRequested, PassVetoed, PhaseAdvanced, ViolationFail,
};
use anchor_lang::solana_program::pubkey::Pubkey;
use setl8_verify::{hexutil, log};

use crate::cluster::TxInfo;

pub enum Ev {
    Opened(Opened),
    Reset(AccountReset),
    Log(EventLog),
    Requested(PassRequested),
    Cancelled(PassCancelled),
    Confirmed(PassConfirmed),
    Rejected(PassRejected),
    Vetoed(PassVetoed),
    Phase(PhaseAdvanced),
    Finalized(PassFinalized),
    Abandoned(AccountAbandoned),
    Failed(AccountFailed),
    Held(AccountHeld),
    Released(HoldReleased),
    Violation(ViolationFail),
    /// An event of the program this tool does not turn into a log line (admin events); its discriminator.
    Other([u8; 8]),
}

fn de<T: AnchorDeserialize>(body: &[u8]) -> Option<T> {
    T::deserialize(&mut &body[..]).ok()
}

/// Decodes the self-CPI events of `program` in one transaction, in execution order of the inner instructions.
pub fn decode(program: &Pubkey, tx: &TxInfo) -> Vec<Ev> {
    let mut out = Vec::new();
    for (_, group) in &tx.inner {
        for ix in group {
            if ix.program != *program || ix.data.len() < 16 || ix.data[..8] != *anchor_lang::event::EVENT_IX_TAG_LE {
                continue;
            }
            let disc: [u8; 8] = ix.data[8..16].try_into().unwrap_or_default();
            let body = &ix.data[16..];
            let ev = if disc == *Opened::DISCRIMINATOR {
                de(body).map(Ev::Opened)
            } else if disc == *AccountReset::DISCRIMINATOR {
                de(body).map(Ev::Reset)
            } else if disc == *EventLog::DISCRIMINATOR {
                de(body).map(Ev::Log)
            } else if disc == *PassRequested::DISCRIMINATOR {
                de(body).map(Ev::Requested)
            } else if disc == *PassCancelled::DISCRIMINATOR {
                de(body).map(Ev::Cancelled)
            } else if disc == *PassConfirmed::DISCRIMINATOR {
                de(body).map(Ev::Confirmed)
            } else if disc == *PassRejected::DISCRIMINATOR {
                de(body).map(Ev::Rejected)
            } else if disc == *PassVetoed::DISCRIMINATOR {
                de(body).map(Ev::Vetoed)
            } else if disc == *PhaseAdvanced::DISCRIMINATOR {
                de(body).map(Ev::Phase)
            } else if disc == *PassFinalized::DISCRIMINATOR {
                de(body).map(Ev::Finalized)
            } else if disc == *AccountAbandoned::DISCRIMINATOR {
                de(body).map(Ev::Abandoned)
            } else if disc == *AccountFailed::DISCRIMINATOR {
                de(body).map(Ev::Failed)
            } else if disc == *AccountHeld::DISCRIMINATOR {
                de(body).map(Ev::Held)
            } else if disc == *HoldReleased::DISCRIMINATOR {
                de(body).map(Ev::Released)
            } else if disc == *ViolationFail::DISCRIMINATOR {
                de(body).map(Ev::Violation)
            } else {
                Some(Ev::Other(disc))
            };
            if let Some(e) = ev {
                out.push(e);
            }
        }
    }
    out
}

fn kv(ty: &str, account: &[u8; 32], fields: &str) -> String {
    format!("{{\"type\":\"{ty}\",\"account\":\"{}\",{fields}}}\n", log::base58(account))
}

/// One event as a line of the verifier's log, with the account it belongs to. `block_time` and `slot` are the transaction's.
pub fn line(ev: &Ev, block_time: i64, slot: u64) -> Option<([u8; 32], String)> {
    let h = |x: &[u8; 32]| hexutil::encode(x);
    Some(match ev {
        Ev::Opened(o) => {
            let rec = log::OpenedRec {
                account: o.account.to_bytes(),
                trader: o.trader.to_bytes(),
                challenge_id: o.challenge_id,
                size_s: o.size_s,
                tier_index: o.tier_index,
                phase: o.phase,
                ruleset_version: o.ruleset_version,
                genesis_hash: o.genesis_hash,
                time: o.time,
            };
            (rec.account, log::opened_line(&rec))
        }
        Ev::Log(l) => (l.account.to_bytes(), log::event_line(&l.account.to_bytes(), l.seq, &l.event_bytes, &l.proof_bytes, slot)),
        Ev::Requested(e) => {
            let a = e.account.to_bytes();
            (a, kv("pass_requested", &a, &format!("\"phase\":{},\"amount\":{},\"b\":{},\"seq\":{},\"journal_hash\":\"{}\",\"time\":{}", e.phase, e.amount, e.b, e.seq, h(&e.journal_hash), e.time)))
        }
        Ev::Confirmed(e) => {
            let a = e.account.to_bytes();
            (a, kv("pass_confirmed", &a, &format!("\"phase\":{},\"amount\":{},\"b\":{},\"seq\":{},\"journal_hash\":\"{}\",\"eligible_at\":{}", e.phase, e.amount, e.b, e.seq, h(&e.journal_hash), e.eligible_at)))
        }
        Ev::Rejected(e) => {
            let a = e.account.to_bytes();
            (a, kv("pass_rejected", &a, &format!("\"phase\":{},\"reason\":{}", e.phase, e.reason)))
        }
        Ev::Vetoed(e) => {
            let a = e.account.to_bytes();
            (a, kv("pass_vetoed", &a, &format!("\"phase\":{},\"admin_a\":\"{}\",\"admin_b\":\"{}\",\"slot\":{}", e.phase, e.admin_a, e.admin_b, e.slot)))
        }
        Ev::Phase(e) => {
            let a = e.account.to_bytes();
            (a, kv("phase_advanced", &a, &format!("\"from_phase\":{},\"to_phase\":{},\"time\":{}", e.from_phase, e.to_phase, e.time)))
        }
        Ev::Finalized(e) => {
            let a = e.account.to_bytes();
            (
                a,
                kv(
                    "pass_finalized",
                    &a,
                    &format!(
                        "\"phase\":{},\"to_phase\":{},\"amount\":{},\"request_id\":{},\"tally_count\":{},\"tally_total\":{},\"window_total\":{},\"time\":{}",
                        e.phase, e.to_phase, e.amount, e.request_id, e.tally_count, e.tally_total, e.window_total, e.time
                    ),
                ),
            )
        }
        Ev::Failed(e) => {
            let a = e.account.to_bytes();
            (a, kv("account_failed", &a, &format!("\"reason\":{},\"b\":{},\"seq\":{},\"journal_hash\":\"{}\"", e.reason, e.b, e.seq, h(&e.journal_hash))))
        }
        Ev::Violation(e) => {
            let a = e.account.to_bytes();
            (a, kv("violation_fail", &a, &format!("\"trader\":\"{}\",\"evidence_hash\":\"{}\"", e.trader, h(&e.evidence_hash))))
        }
        Ev::Held(e) => (e.account.to_bytes(), log::hold_line(&e.account.to_bytes(), block_time, true)),
        Ev::Released(e) => (e.account.to_bytes(), log::hold_line(&e.account.to_bytes(), block_time, false)),
        Ev::Cancelled(e) => {
            let a = e.account.to_bytes();
            (a, kv("pass_cancelled", &a, &format!("\"phase\":{},\"time\":{}", e.phase, e.time)))
        }
        Ev::Abandoned(e) => {
            let a = e.account.to_bytes();
            (a, kv("account_abandoned", &a, &format!("\"trader\":\"{}\",\"time\":{}", e.trader, e.time)))
        }
        Ev::Reset(_) | Ev::Other(_) => return None,
    })
}

/// The events log text of ONE account from the transactions that touched it (in chain order): header, then its lines.
pub fn account_log(program: &Pubkey, account: &Pubkey, txs: &[TxInfo]) -> String {
    let mut text = log::header_line();
    for tx in txs.iter().filter(|t| t.ok()) {
        for ev in decode(program, tx) {
            if let Some((a, l)) = line(&ev, tx.block_time.unwrap_or(0), tx.slot) {
                if a == account.to_bytes() {
                    text.push_str(&l);
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::RawIx;
    use anchor_lang::Event;

    fn tx_with(program: Pubkey, evs: Vec<Vec<u8>>) -> TxInfo {
        let inner: Vec<RawIx> = evs.into_iter().map(|d| RawIx { program, accounts: vec![0], data: [anchor_lang::event::EVENT_IX_TAG_LE.to_vec(), d].concat() }).collect();
        TxInfo { slot: 77, block_time: Some(1_700_000_123), inner: vec![(0, inner)], ..Default::default() }
    }

    #[test]
    fn events_decode_from_inner_instructions_and_become_lines_the_verifier_parses() {
        let program = Pubkey::new_unique();
        let account = Pubkey::new_unique();
        let trader = Pubkey::new_unique();
        let opened = Opened { account, trader, challenge_id: 0, size_s: 2_500_000_000, tier_index: 0, phase: 1, ruleset_version: 1, genesis_hash: [3; 32], time: 1_700_000_000 };
        let elog = EventLog { account, seq: 1, event_bytes: vec![9, 9], proof_bytes: vec![], };
        let req = PassRequested { account, phase: 2, amount: 7, b: 8, seq: 1, journal_hash: [4; 32], time: 5 };
        let conf = PassConfirmed { account, phase: 2, amount: 7, b: 8, seq: 1, journal_hash: [4; 32], eligible_at: 99 };
        let rej = PassRejected { account, phase: 2, reason: 1 };
        let veto = PassVetoed { account, phase: 2, admin_a: Pubkey::new_unique(), admin_b: Pubkey::new_unique(), slot: 6 };
        let adv = PhaseAdvanced { account, from_phase: 1, to_phase: 2, time: 7 };
        let fin = PassFinalized { account, phase: 2, to_phase: 3, amount: 7, request_id: 1, tally_count: 1, tally_total: 7, window_total: 7, time: 8 };
        let failed = AccountFailed { account, reason: 0, b: 5, seq: 6, journal_hash: [5; 32] };
        let viol = ViolationFail { account, trader, evidence_hash: [6; 32] };
        let held = AccountHeld { account, trader, evidence_hash: [7; 32] };
        let rel = HoldReleased { account, trader };
        let canc = PassCancelled { account, phase: 2, time: 11 };
        let aband = AccountAbandoned { account, trader, time: 12 };
        let tx = tx_with(
            program,
            vec![opened.data(), elog.data(), req.data(), conf.data(), rej.data(), veto.data(), adv.data(), fin.data(), failed.data(), viol.data(), held.data(), rel.data(), canc.data(), aband.data()],
        );
        let evs = decode(&program, &tx);
        assert_eq!(evs.len(), 14);
        let mut text = log::header_line();
        for e in &evs {
            let (a, l) = line(e, 1_700_000_123, 77).expect("a line");
            assert_eq!(a, account.to_bytes());
            text.push_str(&l);
        }
        // every line is accepted by the verifier's own parser, as the types the format promises
        let parsed = log::parse(&text).expect("the verifier parses the lines");
        assert_eq!(parsed.len(), 14);
        assert!(matches!(parsed[0].line, log::Line::Opened(_)));
        assert!(matches!(parsed[1].line, log::Line::Event { seq: 1, slot: 77, .. }));
        assert!(matches!(parsed[2].line, log::Line::Pass { kind: log::PassKind::Requested, phase: 2, .. }));
        assert!(matches!(parsed[3].line, log::Line::Pass { kind: log::PassKind::Confirmed, time_or_eligible: 99, .. }));
        assert!(matches!(parsed[5].line, log::Line::PassVetoed { phase: 2, slot: 6 }));
        assert!(matches!(parsed[7].line, log::Line::PassFinalized { to_phase: 3, request_id: 1, .. }));
        assert!(matches!(parsed[10].line, log::Line::Held { time: 1_700_000_123 }), "the hold is dated with the block time");
        assert!(matches!(parsed[13].line, log::Line::AccountAbandoned { time: 12 }));
    }

    #[test]
    fn a_foreign_program_or_a_plain_log_is_not_an_event() {
        let program = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        let account = Pubkey::new_unique();
        let ev = HoldReleased { account, trader: account }.data();
        // the same bytes from another program, and a truncated body
        assert!(decode(&program, &tx_with(other, vec![ev.clone()])).is_empty());
        let mut short = ev.clone();
        short.truncate(20);
        assert!(decode(&program, &tx_with(program, vec![short])).is_empty());
        // an unknown discriminator is kept as Other and gives no line
        let evs = decode(&program, &tx_with(program, vec![vec![1, 2, 3, 4, 5, 6, 7, 8, 0]]));
        assert!(matches!(evs[0], Ev::Other(_)));
        assert!(line(&evs[0], 0, 0).is_none());
    }

    #[test]
    fn the_log_of_one_account_keeps_only_its_lines_and_skips_failed_transactions() {
        let program = Pubkey::new_unique();
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let good = tx_with(program, vec![HoldReleased { account: a, trader: a }.data(), HoldReleased { account: b, trader: b }.data()]);
        let mut bad = tx_with(program, vec![HoldReleased { account: a, trader: a }.data()]);
        bad.err = serde_json::json!({"InstructionError": [0, {"Custom": 1}]});
        let text = account_log(&program, &a, &[good, bad]);
        assert_eq!(text.lines().count(), 2, "header and one line:\n{text}");
    }
}
