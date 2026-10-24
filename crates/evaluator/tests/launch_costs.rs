//! P24a (decision 72) - the event-count check of the owner's fixed costs. Until now the crypto and FX classes carried no commission, so their
//! Fills and Closes were single events; now EVERY Fill and Close carries a `Fee{Commission}` (0.02 % per side). This file measures what that
//! does to the batch limits (8 events, 3 proofs, 1,232 bytes, the compute limit): the events of a Fill and of a Close, and the close-all
//! failures for N = 1 to 8 open positions, with and without commission, for the static-floor failure (every close carries its fee) and the
//! quick-limit failure (decision 69: the crossing slice and the forced closes carry none). Run with `--nocapture` for the tables.
mod common;
use common::runner::*;
use evaluator::testkit::{close_all_story, generate, launch_costs_requested, LAUNCH_COMMISSION_E6};
use evaluator::Status;
use journal_types::{FeeKind, JournalEvent};

/// One row of the close-all table: what the evaluator did with `n` positions.
struct Row {
    n: usize,
    quick: bool,
    launch: bool,
    failed: bool,
    unpostable: u32,
    /// (events, proofs, markets, cu, limit, bytes, format) of every batch from the first close on
    batches: Vec<(usize, usize, usize, u64, u64, usize, &'static str)>,
    valid: bool,
}

fn run_story(n: usize, quick: bool, launch: bool, source: u8) -> Row {
    let spec = close_all_story(7, n, quick, launch, source);
    let mut chain = SimChain::default();
    let r = run_path(&spec, world_for(&spec), &mut chain);
    assert!(r.world.ev.fault().is_none() && r.notes.is_empty(), "n {n} quick {quick} launch {launch}");
    let st = r.world.ev.stats();
    let mut batches = Vec::new();
    let mut seen_close = false;
    for it in &r.items {
        if let Item::Batch(b) = it {
            if b.batch.events.iter().any(|e| matches!(e, JournalEvent::Close { .. })) {
                seen_close = true;
            }
            if seen_close {
                let fmt = if b.tx_format == evaluator::TxFormat::V0Lookup { "lookup table" } else { "legacy" };
                batches.push((b.batch.events.len(), b.batch.proofs.len(), b.markets.len(), b.cu, b.cu_limit, b.tx_bytes, fmt));
            }
        }
    }
    let rep = verify_items(&r.world, &r.items, None, None);
    if std::env::var_os("LC_DEBUG").is_some() {
        println!("n {n} quick {quick} launch {launch}: positions {} refusals {:?} status {:?} stats {:?}", r.world.ev.positions().len(), r.world.ev.refusals().iter().map(|x| format!("{:?}", x.reason)).collect::<Vec<_>>(), r.world.ev.status(), r.world.ev.stats());
    }
    Row { n, quick, launch, failed: matches!(r.world.ev.status(), Status::Failed(_)), unpostable: st.unpostable, batches, valid: rep.verdict == setl8_verify::Verdict::Valid }
}

#[test]
fn close_all_for_one_to_eight_positions_with_and_without_the_owner_commission() {
    println!("\nclose-all failures, source-3 proofs (the biggest), tier $200K; per row the batches from the first close on:");
    println!("| story | N | costs | failed | unpostable | verifier | batches (events/proofs/markets, CU, limit, bytes, format) |");
    println!("|---|---|---|---|---|---|---|");
    let mut new_unpostable = Vec::new();
    for quick in [false, true] {
        for n in 1..=8usize {
            let zero = run_story(n, quick, false, 3);
            let owner = run_story(n, quick, true, 3);
            for r in [&zero, &owner] {
                let list: Vec<String> = r.batches.iter().map(|(e, p, m, cu, l, b, f)| format!("{e}/{p}/{m} {cu} {l} {b}B {f}")).collect();
                println!("| {} | {} | {} | {} | {} | {} | {} |", if r.quick { "quick-limit" } else { "static floor" }, r.n, if r.launch { "owner" } else { "none" }, r.failed, r.unpostable, if r.valid { "VALID" } else { "NOT VALID" }, list.join("; "));
            }
            // the decision must not make a forced close-all unpostable that was postable without it
            if owner.unpostable > zero.unpostable {
                new_unpostable.push((quick, n, zero.unpostable, owner.unpostable));
            }
            assert!(owner.valid, "quick {quick} n {n}");
            // what the table shows, pinned: a static-floor failure of 1 to 8 positions always fails the account and is postable, with every
            // close carrying its commission (the closes are separate units, so no batch of the story holds more than 7 events); a quick-limit failure is postable up to four positions (crossing close and
            // two forced closes in three markets) and NOT from five on, with and without the commission: the three-proof limit of decision 69
            if !quick {
                assert!(owner.failed && owner.unpostable == 0 && zero.unpostable == 0, "static floor n {n}");
                assert!(owner.batches.iter().all(|b| b.0 <= 7), "static floor n {n}: {:?}", owner.batches);
            } else if n >= 2 {
                assert!(owner.failed, "quick n {n}");
                assert_eq!((owner.unpostable, zero.unpostable), if n <= 4 { (0, 0) } else { (1, 1) }, "quick n {n}");
            }
            if owner.failed && owner.unpostable == 0 {
                // every batch is inside the chain's limits, in some form
                for (e, p, _, cu, l, b, _) in &owner.batches {
                    assert!(*e <= 8 && *p <= 3 && *b <= 1_232 && *l >= *cu && *l <= 1_400_000, "quick {quick} n {n}: {e} events {p} proofs {b} bytes");
                }
            }
        }
    }
    assert!(new_unpostable.is_empty(), "forced close-alls that no longer fit: {new_unpostable:?}");
}

/// The events of a Fill and of a Close with commission, counted over real paths with the owner's costs: a Fill is `Fill, Fee`, a Close is
/// `Close, Fee` except the quick-limit crossing slice and the forced closes after it (decision 69), which carry none.
#[test]
fn every_fill_and_every_close_carries_a_commission_with_the_owner_costs() {
    let (mut fills, mut fills_fee, mut closes, mut closes_fee, mut closes_no_fee_paths) = (0u64, 0u64, 0u64, 0u64, 0u64);
    std::env::set_var("SETL8_LAUNCH_COSTS", "1");
    assert!(launch_costs_requested());
    for seed in 1..=400u64 {
        let spec = generate(seed);
        if spec.scenario == evaluator::testkit::Scenario::Edge {
            continue;
        }
        assert_eq!(spec.commission_e6, [LAUNCH_COMMISSION_E6; 8], "seed {seed}");
        let mut chain = SimChain::default();
        let r = run_path(&spec, world_for(&spec), &mut chain);
        let evs: Vec<&JournalEvent> = r.items.iter().filter_map(|i| if let Item::Batch(b) = i { Some(b) } else { None }).flat_map(|b| b.batch.events.iter()).collect();
        let st = r.world.ev.stats();
        let mut path_closes_no_fee = 0;
        for (k, e) in evs.iter().enumerate() {
            let next_is_fee = matches!(evs.get(k + 1), Some(JournalEvent::Fee { kind: FeeKind::Commission, .. }));
            match e {
                JournalEvent::Fill { .. } => {
                    fills += 1;
                    fills_fee += u64::from(next_is_fee);
                }
                JournalEvent::Close { .. } => {
                    closes += 1;
                    closes_fee += u64::from(next_is_fee);
                    path_closes_no_fee += u64::from(!next_is_fee);
                }
                _ => {}
            }
        }
        // a Close without a Fee exists only in a quick-limit failure (the crossing slice, the forced closes) or when the notional rounds to a zero fee
        if path_closes_no_fee > 0 {
            closes_no_fee_paths += 1;
            assert!(st.quick_fails > 0 || st.breaches > 0 || st.static_fails > 0, "seed {seed}: a close without commission outside a failure/breach");
        }
    }
    std::env::remove_var("SETL8_LAUNCH_COSTS");
    println!("\nover 399 paths with the owner costs: {fills} fills, {fills_fee} followed by a commission fee; {closes} closes, {closes_fee} followed by one; {closes_no_fee_paths} paths have closes without one");
    assert_eq!(fills, fills_fee, "every Fill is followed by its commission Fee");
    assert!(closes_fee * 100 >= closes * 80, "most closes carry one");
}

/// The same N positions as ONE failing batch (the shape of decision 69), computed from the real encoder with source-3 proofs: events, proofs,
/// the measured compute and the limit the model asks for, the transaction bytes legacy and with the lookup table, and whether it is a legal
/// batch. `static`: every close carries a commission fee (2N + 2 events); `quick`: the crossing close and the forced closes carry none (N + 2).
#[test]
fn the_single_batch_shape_of_a_close_all_for_one_to_eight_positions() {
    use evaluator::batch::{tx_size, tx_size_v0};
    use evaluator::BatchKind;
    use evaluator::compute::{measured_cu, recommended_cu_limit};
    use journal_types::{CloseReason, FailReason, JournalBatch, PriceProof, Side};
    let close = |m: u16, p: u8| JournalEvent::Close { market: m, side: Side::Long, qty: u64::MAX, entry_e12: u64::MAX, exit_e12: u64::MAX, time: i64::MAX, timer_start: i64::MAX, pnl: i64::MIN, quick: true, credited: i64::MIN, reason: CloseReason::Fail, intent_id: u32::MAX, proof: p };
    let fee = |m: u16| JournalEvent::Fee { kind: FeeKind::Commission, market: m, amount: u64::MAX, time: i64::MAX };
    let proof = |i: u8| PriceProof { source: 3, feed_id: [i; 32], timestamp_ns: u64::MAX, quantized_value: i128::MIN, signed_update: vec![255; 32] };
    println!("\none failing batch for N positions (source-3 proofs):");
    println!("| story | N | events | proofs | measured CU | limit | legacy bytes | lookup-table bytes | legal batch |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for quick in [false, true] {
        for n in 1..=8usize {
            let mut events = Vec::new();
            for i in 0..n {
                let m = u16::try_from(i).unwrap();
                events.push(close(m, u8::try_from(i.min(254)).unwrap()));
                if !quick {
                    events.push(fee(m));
                }
            }
            events.push(JournalEvent::Fail { time: i64::MAX, reason: FailReason::PhaseQuick, b_after: i64::MIN, proof: 0 });
            events.push(JournalEvent::Meta { time: i64::MAX, flat: true, samples_hash: [255; 32], intents_hash: [255; 32], engine_version: u16::MAX });
            let proofs: Vec<PriceProof> = (0..n).map(|i| proof(u8::try_from(i).unwrap())).collect();
            let (ne, np) = (events.len(), proofs.len());
            let len = JournalBatch { first_seq: u64::MAX, prev_hash: [255; 32], events, proofs }.to_bytes().map(|b| b.len());
            let kind = BatchKind::ReportFail(engine::FailReason::PhaseQuick);
            let legal = ne <= 8 && np <= 3;
            // the format has 8 events and 3 proofs; beyond it the encoder refuses (len is Err) and the model gives the transaction maximum
            let (cu, lim, leg, v0) = match &len {
                Ok(l) => (measured_cu(kind, ne, np).to_string(), recommended_cu_limit(kind, ne, np).to_string(), tx_size(kind, np, *l, true).to_string(), tx_size_v0(kind, np, *l, true).to_string()),
                Err(_) => ("-".into(), "-".into(), "-".into(), "-".into()),
            };
            println!("| {} | {n} | {ne} | {np} | {cu} | {lim} | {leg} | {v0} | {} |", if quick { "quick limit (no fee on the closes)" } else { "static floor (fee on every close)" }, if legal { "yes" } else { "NO: over 8 events or 3 proofs" });
            if legal {
                let l = len.unwrap();
                assert!(tx_size_v0(kind, np, l, true) <= 1_232 && recommended_cu_limit(kind, ne, np) <= 1_400_000, "quick {quick} n {n}");
            }
        }
    }
}

/// P24a-r1: the rounding corner at 200 e6. The commission is `ceil(notional x 200 / 1e6)`, so any positive notional pays at least 1 micro-USD and
/// the evaluator (which emits the `Fee` only when the amount is above zero) never drops one. The smallest minimum order of config/launch.md is SOL,
/// 100,000 units at 103.312365 = $10.331236 -> 2,067 micro-USD; the next two are MSFT 2,106 and AMD 2,137. Hand numbers, ceil edges included.
#[test]
fn the_smallest_legal_order_still_pays_a_commission_of_at_least_one_micro_usd() {
    let rate = LAUNCH_COMMISSION_E6;
    assert_eq!(rate, 200);
    for (notional_micro, fee) in [(10_331_236, 2_067), (10_526_800, 2_106), (10_684_800, 2_137), (10_000_000, 2_000), (5_000, 1), (5_001, 2), (1, 1)] {
        assert_eq!(engine::fee_ceil_micro(notional_micro, rate), Ok(fee), "notional {notional_micro}");
    }
    // zero only for a zero notional, which no order has (the minimum quantity is positive)
    assert_eq!(engine::fee_ceil_micro(0, rate), Ok(0));
    // a price 4 orders of magnitude below the launch price of SOL would still pay: 10_331_236 / 10_000 = 1,033 micro-USD notional -> 1 micro-USD
    assert_eq!(engine::fee_ceil_micro(1_034, rate), Ok(1));
}
