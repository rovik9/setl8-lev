//! The histories of the rehearsal, each a short Rust function over a [`Live`] account: a pass (+10% of S, held longer than 45 s), a
//! static-floor fail, a hold followed by a fail, a breach (12 h lock) and the forced close-all after a quick-limit overshoot
//! (decision 69). Prices are the mock oracle's (source 255); the numbers are chosen so that the engine's rules are hit exactly where
//! the comments say. Every story is run offline by the unit tests (and again before a live run) against [`OfflineNet`].
use evaluator::{Batch, BatchKind, ChainView};
use journal_types::JournalEvent;

use crate::live::{AcctRef, Live, Net};

/// Starting mock prices (e12): BTC $60,000, ETH $2,600, SOL $150.
pub const BTC: (u16, u64) = (0, 60_000 * 1_000_000_000_000);
pub const ETH: (u16, u64) = (1, 2_600 * 1_000_000_000_000);
pub const SOL: (u16, u64) = (2, 150 * 1_000_000_000_000);

/// Notional of a position as thousandths of S, and the price move (ppm) that makes it gain `gain_ppm` of S.
fn move_ppm(gain_ppm: i64, k_milli: i64) -> i64 {
    gain_ppm * 1000 / k_milli
}

fn open_long(l: &mut Live, m: u16, size: i64, k_milli: i64) -> Result<u64, String> {
    let qty = l.qty(m, size * k_milli / 1000);
    l.market_order(m, true, qty)?;
    Ok(qty)
}

/// A pass: a long of 3 S in `m`, +10% of S over 60 s (so held well over 45 s: a full-credit slice), closed flat. The balance ends
/// above 1.08 S with tight mode ended; the caller presses the button.
pub fn pass_story(l: &mut Live, net: &mut dyn Net, a: &AcctRef, m: u16, size: i64) -> Result<(), String> {
    l.set_active(&[m]);
    l.hold_prices(10, 5);
    open_long(l, m, size, 3_000)?;
    l.step(5);
    let top = l.scaled(m, move_ppm(100_000, 3_000));
    l.ramp(&[(m, top)], 60, 5);
    l.hold_prices(5, 5);
    l.close_order(m)?;
    l.step(5);
    l.hold_prices(10, 5);
    l.flush(net, a)?;
    l.note(format!("{}: pass story done, balance {}", a.name, l.ev.state().b));
    Ok(())
}

/// A static-floor fail: a long of 4 S, the price falls 2.3% (-9.2% of S), the equity touches 0.92 S. The evaluator closes the
/// position at the sample and posts `Fail{StaticFloor}` with `report_fail`.
pub fn static_fail_story(l: &mut Live, net: &mut dyn Net, a: &AcctRef, m: u16, size: i64) -> Result<(), String> {
    l.set_active(&[m]);
    l.hold_prices(10, 5);
    open_long(l, m, size, 4_000)?;
    l.step(5);
    let low = l.scaled(m, -23_000);
    l.ramp(&[(m, low)], 40, 5);
    l.hold_prices(10, 5);
    l.flush(net, a)?;
    if !l.failed() {
        return Err("the static-floor story did not fail the account".into());
    }
    Ok(())
}

/// The review case (DESIGN 2.9): a long of 2 S, the evaluator HOLDS the account (`hold_account`), then the price falls 6.5% (-13% of S)
/// and the held account falls to its static floor: `report_fail` on a held account leaves it Failed + held (decision 67 amendment).
pub fn violation_story(l: &mut Live, net: &mut dyn Net, a: &AcctRef, m: u16, size: i64) -> Result<(), String> {
    l.set_active(&[m]);
    l.hold_prices(10, 5);
    open_long(l, m, size, 2_000)?;
    l.step(5);
    l.hold_prices(10, 5);
    l.hold(net, a)?;
    let low = l.scaled(m, -65_000);
    l.ramp(&[(m, low)], 40, 5);
    l.hold_prices(10, 5);
    l.flush(net, a)?;
    if !l.failed() {
        return Err("the held account did not fall to its floor".into());
    }
    Ok(())
}

/// A breach (DESIGN 4.9): a long of 3 S, +1.5% (+4.5% of S: tight mode starts above 1%, the tight floor rises to 0.995 S), then the
/// price falls back to 0.3% below the start (-0.9% of S): the equity touches the tight floor, the evaluator closes the position and
/// posts `Breach`, and the account is locked for 12 hours. The phase is NOT failed.
pub fn breach_story(l: &mut Live, net: &mut dyn Net, a: &AcctRef, m: u16, size: i64) -> Result<(), String> {
    l.set_active(&[m]);
    l.hold_prices(10, 5);
    let start = l.price(m);
    open_long(l, m, size, 3_000)?;
    l.step(5);
    let peak = l.scaled(m, 15_000);
    l.ramp(&[(m, peak)], 30, 5);
    l.hold_prices(5, 5);
    let low = u64::try_from(i128::from(start) - i128::from(start) * 3_000 / 1_000_000).unwrap_or(1);
    l.ramp(&[(m, low)], 25, 5);
    l.hold_prices(10, 5);
    l.flush(net, a)?;
    Ok(())
}

/// The forced close-all after a quick-limit overshoot (decision 69, audit F-16): three longs of 1.2 S in BTC, ETH and SOL, all younger
/// than 45 s. BTC gains 1.4% of S (credit 0.7% S), ETH and SOL 6% of S each (credit 3% S, over the limit of 2.8% S ALONE). The
/// trader closes BTC (fits), then ETH: that close takes the quick tally over the limit, so SOL is closed with it as a
/// `Close{Fail}` and the account fails with `Fail{PhaseQuick}`, all in one `report_fail` batch.
pub fn quick_story(l: &mut Live, net: &mut dyn Net, a: &AcctRef, size: i64) -> Result<(), String> {
    l.set_active(&[BTC.0, ETH.0, SOL.0]);
    l.hold_prices(9, 3);
    for m in [BTC.0, ETH.0, SOL.0] {
        open_long(l, m, size, 1_200)?;
    }
    l.step(3);
    let (b, e, s) = (l.scaled(BTC.0, move_ppm(14_000, 1_200)), l.scaled(ETH.0, move_ppm(60_000, 1_200)), l.scaled(SOL.0, move_ppm(60_000, 1_200)));
    l.ramp(&[(BTC.0, b), (ETH.0, e), (SOL.0, s)], 30, 3);
    l.hold_prices(3, 3);
    l.close_order(BTC.0)?;
    l.step(3);
    l.close_order(ETH.0)?;
    l.step(3);
    l.hold_prices(9, 3);
    l.flush(net, a)?;
    if !l.failed() {
        return Err("the quick-limit story did not fail the account".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------- offline net

/// A net that posts nothing: it records the batches and the pass calls, and answers the way an honest chain would. The stories run
/// against it in the unit tests and before a live run (the preflight of the rehearsal).
#[derive(Default)]
pub struct OfflineNet {
    pub batches: Vec<Batch>,
    pub calls: Vec<String>,
    pub view: Option<ChainView>,
}

impl Net for OfflineNet {
    fn post(&mut self, _a: &AcctRef, b: &Batch) -> Result<(), String> {
        self.view = Some(ChainView { seq: b.account_after.seq, journal_hash: b.account_after.journal_hash, b: b.account_after.b, flat: b.account_after.flat, phase: self.view.map(|v| v.phase).unwrap_or(1) });
        self.batches.push(b.clone());
        Ok(())
    }
    fn view(&mut self, a: &AcctRef) -> Result<ChainView, String> {
        let _ = a;
        self.view.ok_or_else(|| "nothing posted yet".to_string())
    }
    fn request_advance(&mut self, a: &AcctRef, t: i64) -> Result<bool, String> {
        self.calls.push(format!("request_advance {} {t}", a.name));
        Ok(true)
    }
    fn confirm_pass(&mut self, a: &AcctRef, t: i64) -> Result<(), String> {
        self.calls.push(format!("confirm_pass {} {t}", a.name));
        if let Some(v) = self.view.as_mut() {
            if v.phase == 1 {
                v.phase = 2;
            }
        }
        Ok(())
    }
    fn reject_pass(&mut self, a: &AcctRef, t: i64, reason: u8) -> Result<(), String> {
        self.calls.push(format!("reject_pass {} {t} {reason}", a.name));
        Ok(())
    }
    fn hold(&mut self, a: &AcctRef, t: i64) -> Result<(), String> {
        self.calls.push(format!("hold {} {t}", a.name));
        Ok(())
    }
}

/// The events of a list of batches, in order.
pub fn events_of(batches: &[Batch]) -> Vec<JournalEvent> {
    batches.iter().flat_map(|b| b.batch.events.iter().cloned()).collect()
}

pub fn batch_kinds(batches: &[Batch]) -> Vec<&'static str> {
    batches.iter().map(|b| match b.kind {
        BatchKind::PostJournal => "post_journal",
        BatchKind::ReportFail(_) => "report_fail",
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch;
    use anchor_lang::solana_program::pubkey::Pubkey;
    use evaluator::{AccountParams, PassDecision, Status};
    use journal_types::{CloseReason, FailReason};
    use solana_keypair::Keypair;
    use solana_signer::Signer;

    const SIZE: i64 = 2_500_000_000;
    const T0: i64 = 1_800_000_000;

    fn launch() -> launch::Launch {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/launch.json");
        launch::parse(&std::fs::read_to_string(path).expect("launch.json")).expect("parses")
    }

    fn setup() -> (Live, AcctRef) {
        let l = launch();
        let trader = Keypair::new();
        let account = Pubkey::new_unique();
        let params = AccountParams { account: account.to_bytes(), trader: trader.pubkey().to_bytes(), challenge_id: 0, size_micro: SIZE, phase: 1, tier_index: 0, opened_time: T0 };
        let live = Live::new(l.rules_cfg(true), params, &trader.to_bytes(), &[BTC, ETH, SOL]).expect("live");
        (live, AcctRef { name: "t".into(), trader: trader.pubkey(), challenge_id: 0 })
    }

    #[test]
    fn a_pass_story_ends_flat_above_the_target_and_phase_one_advances_at_once() {
        let (mut l, a) = setup();
        let mut net = OfflineNet::default();
        pass_story(&mut l, &mut net, &a, BTC.0, SIZE).expect("story");
        let st = l.ev.state();
        assert!(st.b >= SIZE / 100 * 108, "balance {} is below 1.08 S", st.b);
        assert!(l.ev.positions().is_empty() && !st.tight_on, "flat, tight mode ended");
        assert!(matches!(l.ev.status(), Status::Active));
        let evs = events_of(&net.batches);
        assert!(evs.iter().any(|e| matches!(e, JournalEvent::TightStart { .. })), "tight mode started");
        assert!(evs.iter().any(|e| matches!(e, JournalEvent::Close { quick: false, .. })), "a full-credit slice");
        assert!(!evs.iter().any(|e| matches!(e, JournalEvent::Breach { .. } | JournalEvent::Fail { .. })));
        let d = l.pass(&mut net, &a).expect("pass");
        assert_eq!(d, PassDecision::Confirm);
        assert_eq!(l.ev.phase(), 2);
        assert_eq!(net.calls.len(), 2);
        // phase 2: the same story passes again from the fresh state
        pass_story(&mut l, &mut net, &a, BTC.0, SIZE).expect("story 2");
        assert!(l.ev.state().b >= SIZE / 100 * 108);
        assert_eq!(l.pass(&mut net, &a).expect("pass 2"), PassDecision::Confirm);
        assert_eq!(l.ev.phase(), 2, "a phase-2 confirmation leaves the pass waiting out its delay");
    }

    #[test]
    fn a_static_fail_story_posts_report_fail_with_the_static_floor() {
        let (mut l, a) = setup();
        let mut net = OfflineNet::default();
        static_fail_story(&mut l, &mut net, &a, BTC.0, SIZE).expect("story");
        assert!(matches!(l.ev.status(), Status::Failed(engine::FailReason::StaticFloor)));
        assert_eq!(batch_kinds(&net.batches).last(), Some(&"report_fail"));
        let evs = events_of(&net.batches);
        assert!(evs.iter().any(|e| matches!(e, JournalEvent::Fail { reason: FailReason::StaticFloor, .. })));
    }

    #[test]
    fn a_violation_story_holds_first_and_then_fails_on_the_floor() {
        let (mut l, a) = setup();
        let mut net = OfflineNet::default();
        violation_story(&mut l, &mut net, &a, BTC.0, SIZE).expect("story");
        assert_eq!(net.calls.iter().filter(|c| c.starts_with("hold")).count(), 1);
        assert!(matches!(l.ev.status(), Status::Failed(engine::FailReason::StaticFloor)));
    }

    #[test]
    fn a_breach_story_breaches_locks_for_12_hours_and_does_not_fail() {
        let (mut l, a) = setup();
        let mut net = OfflineNet::default();
        breach_story(&mut l, &mut net, &a, BTC.0, SIZE).expect("story");
        let evs = events_of(&net.batches);
        let breach_at = evs.iter().find_map(|e| if let JournalEvent::Breach { time, .. } = e { Some(*time) } else { None }).expect("a Breach event");
        assert!(matches!(l.ev.status(), Status::Active));
        assert_eq!(l.ev.state().lock_until, breach_at + 12 * 3_600, "locked for 12 hours from the breach");
        assert!(evs.iter().any(|e| matches!(e, JournalEvent::Close { reason: CloseReason::Breach, .. })));
        assert!(l.ev.state().b > SIZE / 100 * 92, "above the static floor");
    }

    #[test]
    fn the_quick_story_closes_the_last_position_as_a_forced_close_after_the_overshoot() {
        let (mut l, a) = setup();
        let mut net = OfflineNet::default();
        quick_story(&mut l, &mut net, &a, SIZE).expect("story");
        assert!(matches!(l.ev.status(), Status::Failed(engine::FailReason::PhaseQuick)));
        let evs = events_of(&net.batches);
        let reasons: Vec<CloseReason> = evs.iter().filter_map(|e| if let JournalEvent::Close { reason, .. } = e { Some(*reason) } else { None }).collect();
        assert_eq!(reasons, vec![CloseReason::Manual, CloseReason::Manual, CloseReason::Fail], "BTC fits, ETH crosses, SOL is closed with it");
        assert_eq!(l.ev.stats().forced_after_overshoot, 1);
        assert_eq!(l.ev.stats().unpostable, 0);
        // the failing batch is a report_fail whose closes after the overshoot are all Close{Fail}
        let last = net.batches.last().expect("batches");
        assert!(matches!(last.kind, BatchKind::ReportFail(engine::FailReason::PhaseQuick)));
        assert!(last.batch.events.len() <= 8 && last.batch.proofs.len() <= 3);
    }
}
