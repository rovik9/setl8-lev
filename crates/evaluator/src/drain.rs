//! From queued events to batches: the SPLIT rule, `Meta`, the proof table and the prefix hashes.
use std::collections::BTreeSet;

use journal_types::{apply_batch, JournalBatch, JournalEvent, PriceProof, NO_PROOF};

use crate::batch::{cu_estimate, tx_size};
use crate::eval::Evaluator;
use crate::sha256::chain_hasher;
use crate::types::*;

/// Length of an encoded `Meta` event.
const META_LEN: usize = 76;

/// `ev` with its proof field set to `idx`.
fn with_proof(ev: &JournalEvent, idx: u8) -> JournalEvent {
    let mut e = ev.clone();
    match &mut e {
        JournalEvent::Fill { proof, .. }
        | JournalEvent::Close { proof, .. }
        | JournalEvent::TightStart { proof, .. }
        | JournalEvent::FloorRaised { proof, .. }
        | JournalEvent::Breach { proof, .. }
        | JournalEvent::Fail { proof, .. } => *proof = idx,
        JournalEvent::Fee { .. } | JournalEvent::TightEnd { .. } | JournalEvent::Meta { .. } => {}
    }
    e
}

fn fail_reason_of(units: &[&Unit]) -> Option<engine::FailReason> {
    for u in units {
        for q in &u.events {
            if let JournalEvent::Fail { reason, .. } = &q.ev {
                return Some(match reason {
                    journal_types::FailReason::StaticFloor => engine::FailReason::StaticFloor,
                    journal_types::FailReason::PhaseQuick => engine::FailReason::PhaseQuick,
                });
            }
        }
    }
    None
}

struct Measure {
    /// A `TightEnd`, `Breach` or `Fail` is in the batch: the program demands that the batch ends flat (`Meta.flat`).
    needs_flat: bool,
    ends_flat: bool,
    kind: BatchKind,
    events: usize,
    proofs: usize,
    markets: BTreeSet<u16>,
    tx: usize,
    cu: u64,
}

impl Evaluator {
    fn proof_for(&self, idx: usize) -> Option<PriceProof> {
        let s = self.samples.get(idx)?;
        let m = self.rules.market(s.rec.market)?;
        Some(PriceProof {
            source: s.rec.source,
            feed_id: m.feed_id,
            timestamp_ns: s.rec.ts_ns,
            quantized_value: s.rec.value,
            signed_update: if s.rec.source == 3 { s.rec.digest.to_vec() } else { Vec::new() },
        })
    }

    fn measure(&self, units: &[&Unit], policy: &BatchPolicy) -> Measure {
        let kind = match fail_reason_of(units) {
            Some(r) => BatchKind::ReportFail(r),
            None => BatchKind::PostJournal,
        };
        let mut needs_flat = false;
        let mut ends_flat = true;
        let mut events = 1usize; // the Meta
        let mut ev_bytes = META_LEN;
        let mut proof_set = BTreeSet::new();
        let mut markets = BTreeSet::new();
        for u in units {
            for q in &u.events {
                events += 1;
                ev_bytes += q.ev.to_bytes().len();
                ends_flat = q.after.flat;
                if matches!(q.ev, JournalEvent::TightEnd { .. } | JournalEvent::Breach { .. } | JournalEvent::Fail { .. }) {
                    needs_flat = true;
                }
                match &q.ev {
                    JournalEvent::Fill { market, .. } | JournalEvent::Close { market, .. } | JournalEvent::Fee { market, .. } => {
                        markets.insert(*market);
                    }
                    _ => {}
                }
                if let Some(j) = q.proof {
                    proof_set.insert(j);
                    if let Some(s) = self.samples.get(j) {
                        markets.insert(s.rec.market);
                    }
                }
            }
        }
        let proof_bytes: usize = proof_set.iter().filter_map(|j| self.proof_for(*j)).map(|p| 61 + p.signed_update.len()).sum();
        let batch_len = 8 + 32 + 4 + ev_bytes + 4 + proof_bytes;
        let tx = tx_size(kind, markets.len(), batch_len, policy.compute_budget_ix);
        let cu = cu_estimate(kind, events, proof_set.len());
        Measure { needs_flat, ends_flat, kind, events, proofs: proof_set.len(), markets, tx, cu }
    }

    fn fits(m: &Measure, p: &BatchPolicy) -> bool {
        (!m.needs_flat || m.ends_flat)
            && m.events <= p.max_events.min(journal_types::MAX_EVENTS_PER_BATCH)
            && m.proofs <= p.max_proofs.min(journal_types::MAX_PROOFS_PER_BATCH)
            && m.markets.len() <= 8
            && m.tx <= p.max_tx_bytes
            && m.cu <= p.max_cu
    }

    /// Turns the queued events into batches under `policy`, in order, each ending with a `Meta`. Units (a flip, the last
    /// close with its `Breach`/`Fail`/`TightEnd`) are never split; a unit that cannot fit alone is emitted alone.
    /// The batches are final: the evaluator's sequence number and chain head move past them.
    pub fn drain_batches(&mut self, policy: &BatchPolicy) -> Vec<Batch> {
        let mut out = Vec::new();
        if self.fault.is_some() {
            return out;
        }
        while !self.queue.is_empty() {
            let mut take = 0usize;
            loop {
                if take >= self.queue.len() {
                    break;
                }
                let cand: Vec<&Unit> = self.queue.iter().take(take + 1).collect();
                let m = self.measure(&cand, policy);
                if Self::fits(&m, policy) || take == 0 {
                    take += 1;
                } else {
                    break;
                }
                // a failing unit ends the batch
                if cand.last().is_some_and(|u| u.events.iter().any(|q| matches!(q.ev, JournalEvent::Fail { .. }))) {
                    break;
                }
            }
            let units: Vec<Unit> = (0..take).filter_map(|_| self.queue.pop_front()).collect();
            let b = self.seal(units, policy);
            out.push(b);
        }
        if policy.heartbeat && self.processed_through > self.last_meta_time && matches!(self.status, Status::Active) && !self.ended_externally {
            out.push(self.seal(Vec::new(), policy));
        }
        out
    }

    fn seal(&mut self, units: Vec<Unit>, policy: &BatchPolicy) -> Batch {
        let unit_refs: Vec<&Unit> = units.iter().collect();
        let m = self.measure(&unit_refs, policy);
        let flat_events: Vec<&QEvent> = units.iter().flat_map(|u| u.events.iter()).collect();
        // proof table in order of first use
        let mut table: Vec<usize> = Vec::new();
        let mut events: Vec<JournalEvent> = Vec::new();
        for q in &flat_events {
            let idx = match q.proof {
                Some(j) => {
                    let pos = table.iter().position(|x| *x == j).unwrap_or_else(|| {
                        table.push(j);
                        table.len() - 1
                    });
                    u8::try_from(pos).unwrap_or(NO_PROOF)
                }
                None => NO_PROOF,
            };
            events.push(with_proof(&q.ev, idx));
        }
        let proofs: Vec<PriceProof> = table.iter().filter_map(|j| self.proof_for(*j)).collect();
        let last_snap = flat_events.last().map(|q| q.after);
        let last_event_time = last_snap.map(|s| s.last_time).unwrap_or(self.last_meta_time);
        // `Meta.time` tells the verifier how far the journal has caught up: it judges the samples up to that second with the
        // account as the journal left it. A holding fee dated R changes the account BEFORE the samples of second R are judged, so
        // while it is still queued the Meta stops at R - 1.
        let next_head = self.queue.front().and_then(|u| u.events.first()).map(|q| (q.after.last_time, matches!(q.ev, JournalEvent::Fee { kind: journal_types::FeeKind::Holding, .. })));
        let mut meta_time = match next_head {
            Some((h, true)) => self.processed_through.min(h.saturating_sub(1)),
            Some((h, false)) => self.processed_through.min(h),
            None => self.processed_through,
        };
        meta_time = meta_time.max(last_event_time).max(self.last_meta_time);
        let (st, trades, flat) = match last_snap {
            Some(s) => (s.st, s.trades_closed, s.flat),
            None => (self.st, self.trades_closed, self.positions.is_empty()),
        };
        // commitments: samples with a source second up to the Meta time, and every intent accepted so far
        let n_samples = self.samples.partition_point(|s| s.sec() <= meta_time).max(self.samples_hashed);
        while self.samples_hashed < n_samples {
            if let Some(s) = self.samples.get(self.samples_hashed) {
                self.samples_hasher.update(&s.rec.to_bytes());
            }
            self.samples_hashed += 1;
        }
        let intents_len = self.intents.len();
        self.intents_hasher.update(self.intents.get(self.intents_hashed..).unwrap_or(&[]));
        self.intents_hashed = intents_len;
        events.push(JournalEvent::Meta {
            time: meta_time,
            flat,
            samples_hash: self.samples_hasher.finalize(),
            intents_hash: self.intents_hasher.finalize(),
            engine_version: engine::ENGINE_VERSION,
        });
        let batch = JournalBatch { first_seq: self.seq + 1, prev_hash: self.head, events, proofs };
        let (head, last_seq) = apply_batch(&chain_hasher(), &self.head, batch.first_seq, &batch).unwrap_or((self.head, self.seq));
        self.head = head;
        self.seq = last_seq;
        self.last_meta_time = meta_time;
        let account_after = AccountSnapshot {
            b: st.b,
            q_tally: st.q_tally,
            lock_until: st.lock_until,
            last_batch_time: meta_time,
            seq: last_seq,
            journal_hash: head,
            trades_closed: trades,
            latch: st.latch,
            flat,
            tight_on: st.tight_on,
            r: st.r,
            g_peak: st.g_peak,
            l_level: st.l_level,
            failed: matches!(m.kind, BatchKind::ReportFail(_)),
        };
        Batch {
            kind: m.kind,
            batch,
            markets: m.markets.into_iter().collect(),
            meta_time,
            samples_committed: n_samples,
            intents_committed_bytes: intents_len,
            account_after,
            tx_bytes: m.tx,
            cu: m.cu,
        }
    }
}
