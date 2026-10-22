//! S4 (measure) and S5 (the public verifier on the chain's own events), and the results table.
use std::path::PathBuf;

use anchor_lang::solana_program::pubkey::Pubkey;
use setl8_lev_trading::state::Market;

use crate::cluster::TxInfo;
use crate::events;
use crate::launch;
use crate::measure;
use crate::rehearsal::Rehearsal;
use crate::verify::{self, Files};

/// The accounts the verifier is asked about: every honest account (real source format). The mock-proof accounts are verified once
/// to document what the verifier says about source 255; the attack accounts carry junk `Meta` hashes and are not verified.
pub const HONEST: &[&str] = &["A1", "B1", "B2", "B3", "C1", "D1", "E1", "A3"];
pub const MOCK: &[&str] = &["F1"];

impl Rehearsal {
    // ------------------------------------------------------------------------------------------------ S4

    /// The heaviest legal `post_journal` the budget test models, built for a real account: 8 events, 3 proofs in 3 markets (three fills,
    /// three closes, a zero fee, the Meta). The program checks each event's arithmetic and the proofs, not a position book, so a junk
    /// account (G2) takes it. Landed without preflight: what the real runtime makes of 8 emitted events is the measurement.
    /// Re-states the verdict of a heavy batch an earlier run landed (its signature record is in the state).
    fn restate_heavy(&mut self, label: &str, what: &str, limit_text: &str) {
        let rec = self.st.sigs.iter().rev().find(|s| s.label == label).cloned();
        if let Some(r) = rec {
            let ok = r.ok && r.cu <= 200_000;
            self.check(ok, &format!("{what}: {} CU, {} bytes [{}]{} {limit_text}", r.cu, r.size, r.sig, if r.ok { String::new() } else { format!(" FAILED on the cluster: {}", r.err) }));
        }
    }

    fn heavy_batch(&mut self) -> Result<(), String> {
        if self.st.is_done("S4:heavy") {
            self.restate_heavy("post_journal ev=8 pr=3 heavy G2", "the heaviest legal post_journal (8 events, 3 proofs)", "(LiteSVM heaviest mix 125,290)");
            return Ok(());
        }
        if !self.st.accounts.contains_key("G2") {
            return Ok(());
        }
        let g2 = self.acct_ref("G2")?;
        let acct = self.trading_account(&g2.trader, g2.challenge_id)?;
        let now = self.c.now().map_err(|e| e.to_string())?.max(acct.last_batch_time);
        let rs = self.ruleset(acct.ruleset_version)?;
        let mids: [(u16, u64); 3] = [(0, 60_000 * 1_000_000_000_000), (1, 2_600 * 1_000_000_000_000), (2, 150 * 1_000_000_000_000)];
        let mut proofs = Vec::new();
        let mut events = Vec::new();
        for (i, (m, mid)) in mids.iter().enumerate() {
            let mk = self.market(*m)?;
            proofs.push(journal_types::PriceProof { source: 255, feed_id: mk.feed_id, timestamp_ns: u64::try_from(now).unwrap_or(0) * 1_000_000_000, quantized_value: i128::from(*mid) * 1_000_000, signed_update: vec![] });
            let h = u32::from(rs.spread_e6[usize::from(*m)]);
            let qty = mk.qty_min * 10;
            let ask = engine::ask_from_mid(*mid, h).map_err(|e| format!("{e:?}"))?;
            events.push(journal_types::JournalEvent::Fill { market: *m, side: journal_types::Side::Long, order_kind: journal_types::OrderKind::Market, qty, price_e12: ask, time: now, avg_entry_after: ask, margin: 0, sl: 0, tp: 0, intent_id: 1, proof: u8::try_from(i).unwrap_or(0) });
        }
        for (i, (m, mid)) in mids.iter().enumerate() {
            let mk = self.market(*m)?;
            let h = u32::from(rs.spread_e6[usize::from(*m)]);
            let qty = mk.qty_min * 10;
            let ask = engine::ask_from_mid(*mid, h).map_err(|e| format!("{e:?}"))?;
            let bid = engine::bid_from_mid(*mid, h).map_err(|e| format!("{e:?}"))?;
            let pnl = engine::pnl_micro(true, qty, ask, bid).map_err(|e| format!("{e:?}"))?;
            let timer = now - 100;
            let (credited, _) = engine::slice_credit(pnl, false);
            events.push(journal_types::JournalEvent::Close { market: *m, side: journal_types::Side::Long, qty, entry_e12: ask, exit_e12: bid, time: now, timer_start: timer, pnl, quick: false, credited, reason: journal_types::CloseReason::Manual, intent_id: 1, proof: u8::try_from(i).unwrap_or(0) });
        }
        events.truncate(6);
        events.push(journal_types::JournalEvent::Fee { kind: journal_types::FeeKind::Commission, market: 0, amount: 0, time: now });
        events.push(journal_types::JournalEvent::Meta { time: now, flat: true, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION });
        let batch = journal_types::JournalBatch { first_seq: acct.seq + 1, prev_hash: acct.journal_hash, events, proofs };
        let ixn = crate::ix::post_journal(&self.ids, &self.keys.pubkey("evaluator"), &g2.trader, g2.challenge_id, acct.ruleset_version, batch.to_bytes().map_err(|e| format!("{e:?}"))?, &[0, 1, 2]);
        self.c.wait_until(now).map_err(|e| e.to_string())?;
        match self.send("post_journal ev=8 pr=3 heavy G2", &[ixn], "evaluator", &[], true) {
            Ok(s) => {
                self.say(&format!("  heavy batch (8 events, 3 proofs): {} CU, {} bytes [{}]", s.info.cu, s.size, s.info.signature));
                self.check(s.info.cu <= 200_000, &format!("the heaviest legal post_journal (8 events, 3 proofs) fits the 200,000-unit limit on the cluster: {} CU (LiteSVM heaviest mix 125,290)", s.info.cu));
            }
            Err(e) => {
                let sig = e.sent.as_ref().map(|s| s.info.signature.clone()).unwrap_or_default();
                let cu = e.sent.as_ref().map(|s| s.info.cu).unwrap_or(0);
                self.check(false, &format!("the heaviest legal post_journal (8 events, 3 proofs) FAILED on the cluster after {cu} CU: {} [{sig}]", e.text));
                self.st.add_finding("S4-heavy-batch", "high", &sig, &format!("an 8-event, 3-proof post_journal that the LiteSVM budget test accepts at 125,290 CU is refused by the cluster runtime after {cu} CU: {}", e.text));
            }
        }
        self.mark_done("S4:heavy")
    }

    /// The heaviest legal `report_fail`: three losing closes (three proofs), three zero fees, the Fail, the Meta = 8 events. The fills
    /// are posted first (they need their own proofs). A junk account (H1): the program checks arithmetic and proofs, not a position book.
    fn heavy_fail_batch(&mut self, name: &str, trader: &str, fees: usize) -> Result<(), String> {
        let events_n = 5 + fees;
        let label = format!("report_fail ev={events_n} pr=3 heavy {name}");
        let done = format!("S4:heavy-fail:{name}");
        let lite = match events_n {
            8 => "LiteSVM 8 events, 3 proofs: 151,284",
            7 => "LiteSVM 7 events, 3 proofs: 149,821",
            6 => "LiteSVM 6 events, 3 proofs: 138,762",
            _ => "LiteSVM 5 events, 3 proofs: 125,054",
        };
        if self.st.is_done(&done) {
            self.restate_heavy(&label, &format!("a legal report_fail of {events_n} events and 3 proofs"), &format!("({lite})"));
            return Ok(());
        }
        let h1 = self.open(name, trader, true)?;
        let acct = self.trading_account(&h1.trader, h1.challenge_id)?;
        let rs = self.ruleset(acct.ruleset_version)?;
        let mids: [(u16, u64); 3] = [(0, 60_000 * 1_000_000_000_000), (1, 2_600 * 1_000_000_000_000), (2, 150 * 1_000_000_000_000)];
        let evaluator = self.keys.pubkey("evaluator");
        // 1. the fills (one batch: 3 fills, 3 proofs, the Meta)
        let now = self.c.now().map_err(|e| e.to_string())?.max(acct.last_batch_time);
        let mut plan = Vec::new(); // (market, qty, entry, exit)
        let mut fills = Vec::new();
        let mut fill_proofs = Vec::new();
        for (i, (m, mid)) in mids.iter().enumerate() {
            let mk = self.market(*m)?;
            let h = u32::from(rs.spread_e6[usize::from(*m)]);
            let ask = engine::ask_from_mid(*mid, h).map_err(|e| format!("{e:?}"))?;
            let low_mid = *mid / 10 * 3;
            let exit = engine::bid_from_mid(low_mid, h).map_err(|e| format!("{e:?}"))?;
            // a loss of about $75 per close
            let diff = u128::from(ask - exit);
            let qty_raw = 75_000_000u128 * 1_000_000_000_000u128 / diff.max(1);
            let step = u128::from(mk.qty_step.max(1));
            let qty = u64::try_from((qty_raw / step + 1) * step).map_err(|e| e.to_string())?.max(mk.qty_min);
            plan.push((*m, qty, ask, exit, low_mid));
            fill_proofs.push(journal_types::PriceProof { source: 255, feed_id: mk.feed_id, timestamp_ns: u64::try_from(now).unwrap_or(0) * 1_000_000_000, quantized_value: i128::from(*mid) * 1_000_000, signed_update: vec![] });
            fills.push(journal_types::JournalEvent::Fill { market: *m, side: journal_types::Side::Long, order_kind: journal_types::OrderKind::Market, qty, price_e12: ask, time: now, avg_entry_after: ask, margin: 0, sl: 0, tp: 0, intent_id: 1, proof: u8::try_from(i).unwrap_or(0) });
        }
        fills.push(journal_types::JournalEvent::Meta { time: now, flat: false, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION });
        let b1 = journal_types::JournalBatch { first_seq: acct.seq + 1, prev_hash: acct.journal_hash, events: fills, proofs: fill_proofs };
        let ixn = crate::ix::post_journal(&self.ids, &evaluator, &h1.trader, h1.challenge_id, acct.ruleset_version, b1.to_bytes().map_err(|e| format!("{e:?}"))?, &[0, 1, 2]);
        self.c.wait_until(now).map_err(|e| e.to_string())?;
        self.send(&format!("post_journal ev=4 pr=3 heavy-fail fills {name}"), &[ixn], "evaluator", &[], false).map_err(|e| e.text)?;
        // 2. the failing batch
        let acct = self.trading_account(&h1.trader, h1.challenge_id)?;
        let t2 = self.c.now().map_err(|e| e.to_string())?.max(acct.last_batch_time);
        let mut events = Vec::new();
        let mut proofs = Vec::new();
        let mut b_after = acct.b;
        for (i, (m, qty, ask, exit, low_mid)) in plan.iter().enumerate() {
            let mk = self.market(*m)?;
            let pnl = engine::pnl_micro(true, *qty, *ask, *exit).map_err(|e| format!("{e:?}"))?;
            let (credited, _) = engine::slice_credit(pnl, false);
            b_after += credited;
            proofs.push(journal_types::PriceProof { source: 255, feed_id: mk.feed_id, timestamp_ns: u64::try_from(t2).unwrap_or(0) * 1_000_000_000, quantized_value: i128::from(*low_mid) * 1_000_000, signed_update: vec![] });
            events.push(journal_types::JournalEvent::Close { market: *m, side: journal_types::Side::Long, qty: *qty, entry_e12: *ask, exit_e12: *exit, time: t2, timer_start: t2 - 100, pnl, quick: false, credited, reason: journal_types::CloseReason::Manual, intent_id: 1, proof: u8::try_from(i).unwrap_or(0) });
        }
        for _ in 0..fees {
            events.push(journal_types::JournalEvent::Fee { kind: journal_types::FeeKind::Commission, market: 0, amount: 0, time: t2 });
        }
        events.push(journal_types::JournalEvent::Fail { time: t2, reason: journal_types::FailReason::StaticFloor, b_after, proof: 0 });
        events.push(journal_types::JournalEvent::Meta { time: t2, flat: true, samples_hash: [0xAA; 32], intents_hash: [0xBB; 32], engine_version: engine::ENGINE_VERSION });
        let b2 = journal_types::JournalBatch { first_seq: acct.seq + 1, prev_hash: acct.journal_hash, events, proofs };
        let ixn = crate::ix::report_fail(&self.ids, &evaluator, &h1.trader, h1.challenge_id, acct.ruleset_version, b2.to_bytes().map_err(|e| format!("{e:?}"))?, 0, &[0, 1, 2]);
        self.c.wait_until(t2).map_err(|e| e.to_string())?;
        match self.send(&label, &[ixn], "evaluator", &[], true) {
            Ok(s) => {
                self.say(&format!("  report_fail of {events_n} events, 3 proofs: {} CU, {} bytes [{}]", s.info.cu, s.size, s.info.signature));
                self.check(s.info.cu <= 200_000, &format!("a legal report_fail of {events_n} events and 3 proofs fits the 200,000-unit limit on the cluster: {} CU ({lite})", s.info.cu));
            }
            Err(e) => {
                let sig = e.sent.as_ref().map(|s| s.info.signature.clone()).unwrap_or_default();
                let cu = e.sent.as_ref().map(|s| s.info.cu).unwrap_or(0);
                self.check(false, &format!("a legal report_fail of {events_n} events and 3 proofs FAILED on the cluster after {cu} CU: {} [{sig}] ({lite})", e.text));
                self.st.add_finding(&format!("S4-heavy-report-fail-{events_n}"), "high", &sig, &format!("a {events_n}-event, 3-proof report_fail ({lite} CU in LiteSVM) fails on the cluster runtime after {cu} CU: {}", e.text));
            }
        }
        self.mark_done(&done)
    }

    pub fn s4(&mut self) -> Result<(), String> {
        self.begin("S4");
        if let Err(e) = self.heavy_batch() {
            self.fail_with(&format!("heavy batch: {e}"));
        }
        for (name, trader, fees) in [("H1", "trader7", 3usize), ("H2", "trader7", 2), ("H3", "trader5", 1), ("H4", "trader5", 0)] {
            if let Err(e) = self.heavy_fail_batch(name, trader, fees) {
                self.fail_with(&format!("heavy report_fail {name}: {e}"));
            }
        }
        let rows = measure::summarize(&self.st.sigs);
        let lines = measure::compare(&rows);
        let table = measure::table(&lines);
        std::fs::create_dir_all(&self.work).map_err(|e| e.to_string())?;
        std::fs::write(self.work.join("measure.md"), &table).map_err(|e| e.to_string())?;
        for l in &lines {
            self.say(&format!(
                "  {:<20} n={:<3} CU max {:>7} (LiteSVM worst {}, ceiling {}), tx bytes max {:>5} (ceiling {})",
                l.name,
                l.n,
                l.max_cu,
                l.litesvm_cu.map(|x| x.to_string()).unwrap_or_else(|| "-".into()),
                l.ceiling_cu.map(|x| x.to_string()).unwrap_or_else(|| "-".into()),
                l.max_bytes,
                l.ceiling_bytes.map(|x| x.to_string()).unwrap_or_else(|| "-".into())
            ));
        }
        // the hard limits of the runtime: 200,000 units per instruction (no budget instruction is sent) and 1,232 bytes per transaction
        let beyond: Vec<String> = lines.iter().filter(|l| l.max_cu > 200_000 || l.max_bytes > 1_232).map(|l| format!("{} ({} CU, {} bytes)", l.name, l.max_cu, l.max_bytes)).collect();
        self.check(beyond.is_empty(), &format!("every measured instruction is inside the runtime's limits (200,000 CU, 1,232 bytes); {} instructions measured{}", lines.len(), if beyond.is_empty() { String::new() } else { format!(", BEYOND: {}", beyond.join(", ")) }));
        // the ceilings of budget.rs were calibrated on LiteSVM: a line above one is a finding, not a failure of the scenario
        let over: Vec<&measure::Line> = lines.iter().filter(|l| l.over_ceiling).collect();
        if over.is_empty() {
            self.check(true, "no instruction costs more on the cluster than its budget.rs ceiling");
        } else {
            let mut worst_sig = String::new();
            let mut text = Vec::new();
            for l in &over {
                let sig = self.st.sigs.iter().filter(|s| s.ok && measure::instruction_of(&s.label) == l.name).max_by_key(|s| s.cu).map(|s| s.sig.clone()).unwrap_or_default();
                if worst_sig.is_empty() {
                    worst_sig = sig.clone();
                }
                text.push(format!("{} {} CU (ceiling {:?}, LiteSVM worst {:?}) {}", l.name, l.max_cu, l.ceiling_cu, l.litesvm_cu, sig));
            }
            self.say(&format!("  FINDING: {} instructions cost more on the cluster than their budget.rs ceiling: {}", over.len(), text.join("; ")));
            self.st.add_finding(
                "S4-ceilings",
                "medium",
                &worst_sig,
                &format!("on the cluster runtime {} instructions exceed their LiteSVM-calibrated budget.rs ceilings (all far below the 200,000-unit limit): {}. Every one emits an event through the self-CPI of emit_cpi!, which costs about 12,500 CU inside the inner invocation plus the invoke on this runtime", over.len(), text.join("; ")),
            );
        }
        // every instruction the program has should have been measured at least once
        let missing: Vec<&str> = measure::CEILINGS.iter().map(|c| c.0).filter(|n| !rows.contains_key(*n)).collect();
        self.say(&format!("  not exercised by this run: {}", if missing.is_empty() { "none".to_string() } else { missing.join(", ") }));
        self.end("compute units and transaction size of every instruction of the run, against LiteSVM and the ceilings (measure.md)");
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------ S5

    fn chain_txs(&self, addr: &Pubkey) -> Result<Vec<TxInfo>, String> {
        let sigs = self.c.signatures_for(addr).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for (sig, _) in sigs {
            if let Some(t) = self.c.transaction(&sig).map_err(|e| e.to_string())? {
                out.push(t);
            }
        }
        Ok(out)
    }

    fn rules_from_chain(&self, ruleset_version: u16) -> Result<String, String> {
        let rs = self.ruleset(ruleset_version)?;
        let n = self.config()?.market_count;
        let mut markets: Vec<Market> = Vec::new();
        for id in 0..n {
            markets.push(self.market(id)?);
        }
        Ok(launch::rules_json(&rs, &markets))
    }

    /// The five files of one account from the chain and the evaluator's own files.
    pub fn account_files(&self, name: &str) -> Result<(Files, usize, u64), String> {
        let a = self.acct_ref(name)?;
        let addr = self.ids.acct(&a.trader, a.challenge_id);
        let acct = self.trading_account(&a.trader, a.challenge_id)?;
        let txs = self.chain_txs(&addr)?;
        let events_text = events::account_log(&self.ids.lev, &addr, &txs);
        let n_event_lines = events_text.lines().filter(|l| l.contains("\"type\":\"event\"")).count();
        let rules = self.rules_from_chain(acct.ruleset_version)?;
        // an account that never traded (a fresh reset account) has the evaluator's empty files: the headers
        let archive = std::fs::read(self.work.join(format!("{name}.archive.bin"))).unwrap_or_else(|_| evaluator::codec::archive_header().to_vec());
        let intents = std::fs::read(self.work.join(format!("{name}.intents.jsonl"))).unwrap_or_else(|_| evaluator::codec::intents_header_line().as_bytes().to_vec());
        Ok((Files { events: events_text, rules, archive, intents, snapshot: verify::snapshot_json(&acct) }, n_event_lines, acct.seq))
    }

    fn write_files(&self, name: &str, tag: &str, f: &Files) -> Result<PathBuf, String> {
        let dir = self.work.join("verify").join(name).join(tag);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (n, bytes) in [("events.jsonl", f.events.as_bytes()), ("rules.json", f.rules.as_bytes()), ("archive.bin", &f.archive[..]), ("intents.jsonl", &f.intents[..]), ("snapshot.json", f.snapshot.as_bytes())] {
            std::fs::write(dir.join(n), bytes).map_err(|e| e.to_string())?;
        }
        Ok(dir)
    }

    /// The verifier's own command line on the written files, when its binary is available: (exit code, first lines of the output).
    fn run_cli(&self, dir: &std::path::Path) -> Option<(i32, String)> {
        let bin = self.verify_bin.as_ref()?;
        if !bin.exists() {
            return None;
        }
        let out = std::process::Command::new(bin)
            .arg("account")
            .args(["--events", &dir.join("events.jsonl").to_string_lossy()])
            .args(["--rules", &dir.join("rules.json").to_string_lossy()])
            .args(["--archive", &dir.join("archive.bin").to_string_lossy()])
            .args(["--intents", &dir.join("intents.jsonl").to_string_lossy()])
            .args(["--snapshot", &dir.join("snapshot.json").to_string_lossy()])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).lines().take(3).collect::<Vec<_>>().join(" / ");
        Some((out.status.code().unwrap_or(-1), text))
    }

    pub fn s5(&mut self) -> Result<(), String> {
        self.begin("S5");
        if let Err(e) = self.s5_inner() {
            self.fail_with(&e);
        }
        self.end("the chain's own events replayed by the public verifier: every honest account VALID against a snapshot of the chain account; tampered copies flagged");
        Ok(())
    }

    fn s5_inner(&mut self) -> Result<(), String> {
        let mut valid = 0;
        let mut first_honest: Option<Files> = None;
        let names: Vec<&str> = HONEST.iter().copied().filter(|n| self.st.accounts.contains_key(*n)).collect();
        for name in names {
            let (files, event_lines, seq) = match self.account_files(name) {
                Ok(x) => x,
                Err(e) => {
                    self.check(false, &format!("{name}: {e}"));
                    continue;
                }
            };
            self.check(event_lines as u64 == seq, &format!("{name}: the chain delivered {event_lines} journal events and the account says seq {seq} (every event decodes into the verifier's format)"));
            let dir = self.write_files(name, "chain", &files)?;
            let rep = verify::run(&files)?;
            let ok = verify::is_valid(&rep);
            if ok {
                valid += 1;
            }
            let cli = self.run_cli(&dir);
            let cli_text = cli.map(|(code, t)| format!(" | CLI exit {code}: {t}")).unwrap_or_default();
            self.check(ok, &format!("{name}: VALID against the chain snapshot (seq {seq}){}{cli_text}", if ok { String::new() } else { format!(" findings: {}", verify::classes(&rep)) }));
            if !ok {
                let detail = rep.findings.iter().take(4).map(|f| format!("{:?} seq={:?} expected={} found={}", f.class, f.seq, f.expected, f.found)).collect::<Vec<_>>().join("; ");
                self.st.add_finding(&format!("S5-{name}"), "high", "", &format!("the verifier does not call the honest account {name} VALID: {detail}"));
            }
            if first_honest.is_none() && ok && verify::records(&files.archive) > 10 {
                first_honest = Some(files);
            }
        }
        self.say(&format!("  {valid} honest accounts VALID"));

        // the account traded with mock proofs: documented, not an error of the chain
        let mock_names: Vec<&str> = MOCK.iter().copied().filter(|n| self.st.accounts.contains_key(*n)).collect();
        for name in mock_names {
            if let Ok((files, ..)) = self.account_files(name) {
                self.write_files(name, "chain", &files)?;
                let rep = verify::run(&files)?;
                let shape = verify::classes(&rep).contains("ProofShape");
                self.check(!verify::is_valid(&rep) && shape, &format!("{name} (mock proofs, source 255): the verifier flags {} (the localnet-only source is not part of its format)", verify::classes(&rep)));
                self.st.add_finding("S5-mock-source", "low", "", "the verifier reports ProofShape for source 255 (mock) proofs: the localnet build accepts them on chain, the verifier does not know the source; a localnet/devnet rehearsal must trade with the real source format to be verifiable");
            }
        }

        // tampered copies
        if let Some(f) = first_honest {
            let n = verify::records(&f.archive);
            let mk = |archive: Vec<u8>, intents: Vec<u8>, events: String| Files { events, rules: f.rules.clone(), archive, intents, snapshot: f.snapshot.clone() };
            let cases: Vec<(&str, Option<Files>)> = vec![
                ("a sample dropped from the archive", verify::drop_record(&f.archive, n / 2).map(|a| mk(a, f.intents.clone(), f.events.clone()))),
                ("a price in the archive changed", verify::change_price(&f.archive, n / 2, 1_000_000_000).map(|a| mk(a, f.intents.clone(), f.events.clone()))),
                ("a signed intent submitted twice", verify::repeat_first_intent(&f.intents).map(|i| mk(f.archive.clone(), i, f.events.clone()))),
                ("the last journal event of the chain left out", {
                    let mut lines: Vec<&str> = f.events.lines().collect();
                    let last = lines.iter().rposition(|l| l.contains("\"type\":\"event\""));
                    last.map(|i| {
                        lines.remove(i);
                        mk(f.archive.clone(), f.intents.clone(), lines.join("\n") + "\n")
                    })
                }),
            ];
            for (what, files) in cases {
                match files {
                    None => {
                        self.check(false, &format!("tamper: {what}: not applicable"));
                    }
                    Some(t) => match verify::run(&t) {
                        Ok(rep) => {
                            let flagged = !verify::is_valid(&rep);
                            self.check(flagged, &format!("tamper: {what} -> {}", if flagged { format!("MISMATCH ({})", verify::classes(&rep)) } else { "VALID (NOT FLAGGED)".to_string() }));
                        }
                        Err(e) => {
                            self.check(true, &format!("tamper: {what} -> refused as an input error ({e})"));
                        }
                    },
                }
            }
        } else {
            self.check(false, "no honest account with enough samples to tamper with");
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------ results

    pub fn results_table(&self) -> String {
        let mut s = String::from("| scenario | result | signatures | detail |\n|---|---|---|---|\n");
        for id in ["S0", "S1", "S2", "S2-day2", "S2-lock", "S3", "S4", "S5"] {
            if let Some(r) = self.st.scenarios.get(id) {
                let sigs: Vec<String> = r.sigs.iter().take(3).map(|x| format!("{}…", &x[..12.min(x.len())])).collect();
                s.push_str(&format!("| {id} | {} | {} ({} total) | {} |\n", r.result, sigs.join(" "), r.sigs.len(), r.detail.chars().take(160).collect::<String>()));
            } else {
                s.push_str(&format!("| {id} | PENDING | | not run yet |\n"));
            }
        }
        s
    }
}
