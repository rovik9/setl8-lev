//! The pass flow from the evaluator's side (DESIGN 4.10): after `request_advance` the account is frozen, the evaluator
//! compares its own state with the chain and answers with `confirm_pass` or `reject_pass(reason)`.
use crate::config::PassReject;
use crate::eval::Evaluator;
use crate::types::*;

impl Evaluator {
    /// The trader pressed "request payout and move to the next phase" on chain at second `now`. The caller has drained
    /// and posted every batch (`chain` is the account as stored) and, if it runs the watchdog, passes its `replay` verdict.
    ///
    /// Freezes trading, closes the seconds before `now`, and decides:
    /// `JournalBehind` when events are still queued or the chain head differs from the evaluator's;
    /// `NotFlat` when a position is open or the chain says it is not flat;
    /// `Other` for pending orders left, a balance that differs from the chain's, a failed phase or a watchdog `Mismatch`;
    /// `TargetNotReached` when `B < 1.08 S`; otherwise `Confirm`.
    pub fn request_pass(&mut self, now: i64, chain: &ChainView, replay: ReplayCheck) -> PassDecision {
        self.frozen = true;
        self.tick(now);
        let reject = |r: PassReject| PassDecision::Reject(r);
        if self.fault.is_some() || !matches!(self.status, Status::Active) {
            return reject(PassReject::Other);
        }
        if self.queued_events() > 0 || chain.seq != self.seq || chain.journal_hash != self.head {
            return reject(PassReject::JournalBehind);
        }
        if !self.positions.is_empty() || !chain.flat {
            return reject(PassReject::NotFlat);
        }
        if !self.pending.is_empty() || chain.b != self.st.b || chain.phase != self.phase {
            return reject(PassReject::Other);
        }
        let failed = engine::apply_closed_slice(&self.st, &self.erules, 0, 0).map(|d| d.fail.is_some()).unwrap_or(true)
            || engine::static_floor_micro(&self.st).map(|f| self.st.b <= f).unwrap_or(true);
        match engine::pass_check(&self.st, true, failed) {
            Ok(()) => {}
            Err(engine::PassError::NotFlat) => return reject(PassReject::NotFlat),
            Err(engine::PassError::TargetNotReached) => return reject(PassReject::TargetNotReached),
            Err(engine::PassError::PhaseFailed) => return reject(PassReject::Other),
        }
        if replay == ReplayCheck::Mismatch {
            return reject(PassReject::Other);
        }
        PassDecision::Confirm
    }
}
