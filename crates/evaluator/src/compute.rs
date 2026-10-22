//! The compute contract between the evaluator and the backend that builds its transactions (P27, decision 71).
//!
//! The program's journal events are emitted through `emit_cpi!`, a self-invocation of the program per event, and on a real runtime
//! (Agave 4.0.2 measured) that costs far more than the LiteSVM harness shows: a `report_fail` of 8 events and 3 proofs needs more than
//! the default 200,000 units. So every transaction that carries `post_journal` or `report_fail` MUST carry an explicit
//! `SetComputeUnitLimit`, and its value comes from this module: the worst cost MEASURED on the real runtime for the batch's shape
//! (`handover/cu-runtime.json`, written by `tools/devnet-rehearsal measure`), times 1.20, rounded up to the next 1,000, never above
//! 1,400,000 (the transaction maximum).
//!
//! The two tables below are the measurement reduced to what the evaluator knows when it builds a batch: its kind, its number of events
//! (the `Meta` included) and its number of distinct proofs. Each entry is the maximum over every measured shape that is not larger in
//! events and proofs (monotone), so a cheaper-looking larger batch cannot ask for less. A test recomputes both tables from
//! `cu-runtime.json` and fails if either differs, so a re-measurement that raises a number forces an update here.
use crate::types::{BatchKind, TxFormat};

/// The most compute a transaction may request.
pub const MAX_CU_LIMIT: u64 = 1_400_000;
/// Margin over the measured worst case, as a ratio (12 / 10 = 20 %).
pub const MARGIN_NUM: u64 = 12;
pub const MARGIN_DEN: u64 = 10;
/// Rounding step of a requested limit.
pub const LIMIT_STEP: u64 = 1_000;
/// Bytes a `SetComputeUnitLimit` instruction adds to a legacy transaction: the program key (32) and the instruction
/// (program index 1, no accounts 1, data length 1, data 5).
pub const LIMIT_IX_BYTES: usize = 32 + 8;
/// Bytes a `SetComputeUnitPrice` instruction adds once the program key is already there: 1 + 1 + 1 + 9.
pub const PRICE_IX_BYTES: usize = 12;
/// Rows of the tables: events 0 to 8; columns: proofs 0 to 3.
pub const MAX_EVENTS: usize = 8;
pub const MAX_PROOFS: usize = 3;

// `POST_MEASURED` / `FAIL_MEASURED`: the worst compute units MEASURED for a `post_journal` / `report_fail` of `events` events (row) and
// `proofs` proofs (column), monotone. Generated from handover/cu-runtime.json (see `print_tables` below).
include!("compute_tables.rs");

/// The requested limit for a measured worst case: `ceil(max x 1.20)` up to the next 1,000, never above 1,400,000.
pub fn limit_for(measured_max: u64) -> u64 {
    let raw = measured_max.saturating_mul(MARGIN_NUM).div_ceil(MARGIN_DEN);
    (raw.div_ceil(LIMIT_STEP) * LIMIT_STEP).min(MAX_CU_LIMIT)
}

fn table(kind: BatchKind) -> &'static [[u64; MAX_PROOFS + 1]; MAX_EVENTS + 1] {
    match kind {
        BatchKind::PostJournal => &POST_MEASURED,
        BatchKind::ReportFail(_) => &FAIL_MEASURED,
    }
}

/// The worst compute a batch of this shape was measured to use on the real runtime. A shape outside the batch format (more than 8
/// events or 3 proofs) is not a batch: it gets the transaction maximum.
pub fn measured_cu(kind: BatchKind, events: usize, proofs: usize) -> u64 {
    if events > MAX_EVENTS || proofs > MAX_PROOFS {
        return MAX_CU_LIMIT;
    }
    table(kind)[events][proofs]
}

/// The `SetComputeUnitLimit` a transaction carrying this batch must request.
pub fn recommended_cu_limit(kind: BatchKind, events: usize, proofs: usize) -> u64 {
    limit_for(measured_cu(kind, events, proofs))
}

/// Size of the signed transaction of a batch in the given format, including its compute-budget instructions: always the limit
/// instruction (40 bytes), and with `priority_fee` also a `SetComputeUnitPrice` (12 more bytes, the program key is already there).
pub fn tx_size_with_budget(kind: BatchKind, n_markets: usize, batch_len: usize, format: TxFormat, priority_fee: bool) -> usize {
    let base = match format {
        TxFormat::Legacy => crate::batch::tx_size(kind, n_markets, batch_len, true),
        TxFormat::V0Lookup => crate::batch::tx_size_v0(kind, n_markets, batch_len, true),
    };
    base + if priority_fee { PRICE_IX_BYTES } else { 0 }
}

/// One measured shape of `cu-runtime.json` reduced to the model's axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measured {
    pub post: bool,
    pub events: usize,
    pub proofs: usize,
    pub max_cu: u64,
}

pub type Table = [[u64; MAX_PROOFS + 1]; MAX_EVENTS + 1];

/// The tables the measurements imply: entry (e, p) is the maximum over every measured shape of that kind with at most e events and
/// at most p proofs; an entry no measurement is below (a shape the format cannot have, such as a failing batch of one event) takes the
/// smaller non-zero value of the next larger entries (one more event, one more proof), so the table never has a hole that asks for less.
pub fn fit(measured: &[Measured]) -> (Table, Table) {
    (table_of(measured, true), table_of(measured, false))
}

fn table_of(measured: &[Measured], post: bool) -> Table {
    let mut t: Table = [[0; MAX_PROOFS + 1]; MAX_EVENTS + 1];
    for (e, row) in t.iter_mut().enumerate() {
        for (p, cell) in row.iter_mut().enumerate() {
            *cell = cell_of(measured, post, e, p);
        }
    }
    t
}

fn cell_of(measured: &[Measured], post: bool, e: usize, p: usize) -> u64 {
    let raw = measured.iter().filter(|m| m.post == post && m.events <= e && m.proofs <= p).map(|m| m.max_cu).max().unwrap_or(0);
    if raw > 0 {
        return raw;
    }
    let up = if e < MAX_EVENTS { cell_of(measured, post, e + 1, p) } else { 0 };
    let right = if p < MAX_PROOFS { cell_of(measured, post, e, p + 1) } else { 0 };
    [up, right].into_iter().filter(|x| *x > 0).min().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FailReason;
    use serde_json::Value;

    fn fail() -> BatchKind {
        BatchKind::ReportFail(FailReason::StaticFloor)
    }

    /// The measured shapes of `handover/cu-runtime.json` that are batches.
    pub fn from_json() -> Vec<Measured> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../handover/cu-runtime.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: Value = serde_json::from_str(&text).expect("cu-runtime.json is JSON");
        v["shapes"]
            .as_array()
            .expect("shapes")
            .iter()
            .filter_map(|s| {
                let post = match s["instruction"].as_str()? {
                    "post_journal" => true,
                    "report_fail" => false,
                    _ => return None,
                };
                Some(Measured { post, events: usize::try_from(s["params"]["events"].as_u64()?).ok()?, proofs: usize::try_from(s["params"]["proofs"].as_u64()?).ok()?, max_cu: s["max_cu"].as_u64()? })
            })
            .collect()
    }

    #[test]
    fn the_limit_is_the_measured_worst_case_plus_20_percent_rounded_up_to_1000_and_capped() {
        assert_eq!(limit_for(100_000), 120_000);
        assert_eq!(limit_for(100_001), 121_000, "120,001.2 rounds up to the next 1,000");
        assert_eq!(limit_for(196_812), 237_000, "236,174.4 -> 237,000");
        assert_eq!(limit_for(1), 1_000);
        assert_eq!(limit_for(0), 0);
        assert_eq!(limit_for(1_166_667), 1_400_000);
        assert_eq!(limit_for(5_000_000), 1_400_000, "never above the transaction maximum");
        assert_eq!(limit_for(u64::MAX), 1_400_000);
    }

    #[test]
    fn the_tables_are_monotone_in_events_and_proofs() {
        for t in [&POST_MEASURED, &FAIL_MEASURED] {
            for e in 0..=MAX_EVENTS {
                for p in 0..=MAX_PROOFS {
                    if e > 0 {
                        assert!(t[e][p] >= t[e - 1][p], "events {e} proofs {p}");
                    }
                    if p > 0 {
                        assert!(t[e][p] >= t[e][p - 1], "events {e} proofs {p}");
                    }
                    assert!(t[e][p] > 0, "no hole at events {e} proofs {p}");
                }
            }
        }
    }

    #[test]
    fn the_tables_are_what_cu_runtime_json_implies_so_a_remeasurement_forces_an_update() {
        let measured = from_json();
        assert!(measured.len() > 40, "{} batch shapes measured", measured.len());
        let (post, failt) = fit(&measured);
        assert_eq!(post, POST_MEASURED, "the post_journal table differs from the measurement: regenerate with `cargo test -p setl8-lev-evaluator print_tables -- --ignored --nocapture`");
        assert_eq!(failt, FAIL_MEASURED, "the report_fail table differs from the measurement");
        // and no measured shape is above what the model requests, with the 20 % margin
        for m in &measured {
            let kind = if m.post { BatchKind::PostJournal } else { fail() };
            let limit = recommended_cu_limit(kind, m.events, m.proofs);
            assert!(limit * 10 >= m.max_cu * 12 || limit == MAX_CU_LIMIT, "{m:?}: the limit {limit} is below 1.20 x the measured worst case");
            assert!(measured_cu(kind, m.events, m.proofs) >= m.max_cu, "{m:?}");
            assert!(limit <= MAX_CU_LIMIT);
        }
    }

    #[test]
    #[ignore]
    fn print_tables() {
        let (post, failt) = fit(&from_json());
        let fmt = |name: &str, t: &Table| {
            let rows: Vec<String> = t.iter().map(|r| format!("    [{}],", r.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "))).collect();
            format!("pub const {name}: [[u64; MAX_PROOFS + 1]; MAX_EVENTS + 1] = [\n{}\n];\n", rows.join("\n"))
        };
        println!("// Generated from handover/cu-runtime.json by `print_tables` (P27). Rows: events 0..=8, columns: proofs 0..=3.\n{}\n{}", fmt("POST_MEASURED", &post), fmt("FAIL_MEASURED", &failt));
    }

    #[test]
    fn the_heaviest_shapes_ask_for_more_than_the_default_limit_and_less_than_the_maximum() {
        let heavy = recommended_cu_limit(fail(), 8, 3);
        assert!(heavy > 200_000, "an 8-event report_fail needs more than the default: {heavy}");
        assert!(heavy <= MAX_CU_LIMIT);
        assert!(recommended_cu_limit(BatchKind::PostJournal, 8, 3) > 200_000);
        assert!(recommended_cu_limit(BatchKind::PostJournal, 1, 0) < 100_000);
        // out of format: the maximum
        assert_eq!(recommended_cu_limit(BatchKind::PostJournal, 9, 0), MAX_CU_LIMIT);
        assert_eq!(recommended_cu_limit(fail(), 2, 4), MAX_CU_LIMIT);
    }

    #[test]
    fn the_budget_bytes_are_the_documented_ones() {
        let base = crate::batch::tx_size(BatchKind::PostJournal, 3, 300, false);
        assert_eq!(tx_size_with_budget(BatchKind::PostJournal, 3, 300, TxFormat::Legacy, false), base + 40);
        assert_eq!(tx_size_with_budget(BatchKind::PostJournal, 3, 300, TxFormat::Legacy, true), base + 52);
        assert_eq!((LIMIT_IX_BYTES, PRICE_IX_BYTES), (40, 12));
    }

    /// A batch of `n_close` forced closes, `n_fee` fees (the rest of the events up to `total`), a `Fail` and the `Meta`, with three source-3
    /// proofs (a 32-byte digest each): the bytes the real encoder gives.
    fn fail_batch_len(n_close: usize, n_fee: usize, proofs: usize) -> usize {
        use journal_types::{CloseReason, FeeKind, JournalBatch, JournalEvent, PriceProof, Side};
        let close = JournalEvent::Close {
            market: 0,
            side: Side::Long,
            qty: u64::MAX,
            entry_e12: u64::MAX,
            exit_e12: u64::MAX,
            time: i64::MAX,
            timer_start: i64::MAX,
            pnl: i64::MIN,
            quick: true,
            credited: i64::MIN,
            reason: CloseReason::Fail,
            intent_id: u32::MAX,
            proof: 0,
        };
        let fee = JournalEvent::Fee { kind: FeeKind::Commission, market: 0, amount: u64::MAX, time: i64::MAX };
        let mut events = vec![close; n_close];
        events.extend(vec![fee; n_fee]);
        events.push(JournalEvent::Fail { time: i64::MAX, reason: journal_types::FailReason::PhaseQuick, b_after: i64::MIN, proof: 0 });
        events.push(JournalEvent::Meta { time: i64::MAX, flat: true, samples_hash: [255; 32], intents_hash: [255; 32], engine_version: u16::MAX });
        let proof = PriceProof { source: 3, feed_id: [255; 32], timestamp_ns: u64::MAX, quantized_value: i128::MIN, signed_update: vec![255; 32] };
        JournalBatch { first_seq: u64::MAX, prev_hash: [255; 32], events, proofs: vec![proof; proofs] }.to_bytes().expect("encodes").len()
    }

    #[test]
    fn the_group_that_cannot_be_split_always_fits_a_packet_in_some_format() {
        // the largest failing batches the format allows: at most 8 events (6 closes at most, none with a fee after an overshoot), 3 proofs
        // of source 3, three markets. Legacy does not always fit once the limit instruction is in; the lookup-table form does, with room
        // for the priority-fee instruction too.
        let mut legacy_too_big = 0;
        for (closes, fees) in [(3, 0), (3, 3), (4, 0), (5, 0), (6, 0), (2, 4), (1, 5)] {
            let len = fail_batch_len(closes, fees, 3);
            let kind = BatchKind::ReportFail(engine::FailReason::PhaseQuick);
            let legacy = tx_size_with_budget(kind, 3, len, TxFormat::Legacy, true);
            let v0 = tx_size_with_budget(kind, 3, len, TxFormat::V0Lookup, true);
            if legacy > 1_232 {
                legacy_too_big += 1;
            }
            assert!(v0 <= 1_232, "{closes} closes, {fees} fees: {v0} bytes with a lookup table and a priority fee");
            assert!(v0 < legacy);
        }
        assert!(legacy_too_big >= 3, "the lookup-table form is not a theory: {legacy_too_big} shapes need it");
        // the shape of the campaign seed that found it: three closes, the Fail, the Meta, three source-3 proofs, three markets
        let len = fail_batch_len(3, 0, 3);
        assert_eq!(tx_size_with_budget(BatchKind::ReportFail(engine::FailReason::PhaseQuick), 3, len, TxFormat::Legacy, false), 1_244);
    }

    #[test]
    fn the_lookup_table_form_saves_what_the_formula_says() {
        // post_journal: config, rule set, event authority and the markets leave the static keys (3 + n); the version byte, the lookup
        // count and the lookup itself (32 + 1 + 1 + one byte each) come in
        for n in 0..=3usize {
            let legacy = crate::batch::tx_size(BatchKind::PostJournal, n, 400, true);
            let v0 = crate::batch::tx_size_v0(BatchKind::PostJournal, n, 400, true);
            assert_eq!(legacy - v0, 32 * (3 + n) - (1 + 1 + 32 + 1 + 1 + (3 + n)));
            let f = BatchKind::ReportFail(engine::FailReason::StaticFloor);
            assert_eq!(crate::batch::tx_size(f, n, 400, true) - crate::batch::tx_size_v0(f, n, 400, true), 32 * (6 + n) - (1 + 1 + 32 + 1 + 1 + (6 + n)));
        }
    }

    #[test]
    fn the_model_answers_with_the_table_inside_the_format_and_the_maximum_outside_it() {
        for (e, row) in POST_MEASURED.iter().enumerate() {
            for (p, v) in row.iter().enumerate() {
                assert_eq!(measured_cu(BatchKind::PostJournal, e, p), *v, "events {e} proofs {p}");
                assert_eq!(recommended_cu_limit(BatchKind::PostJournal, e, p), limit_for(*v));
            }
        }
        for (e, row) in FAIL_MEASURED.iter().enumerate() {
            for (p, v) in row.iter().enumerate() {
                assert_eq!(measured_cu(fail(), e, p), *v, "events {e} proofs {p}");
            }
        }
        // the last row and column of the format are inside it, one more of either is not
        assert_ne!(measured_cu(BatchKind::PostJournal, MAX_EVENTS, MAX_PROOFS), MAX_CU_LIMIT);
        assert_eq!(measured_cu(BatchKind::PostJournal, MAX_EVENTS + 1, 0), MAX_CU_LIMIT);
        assert_eq!(measured_cu(BatchKind::PostJournal, 0, MAX_PROOFS + 1), MAX_CU_LIMIT);
    }

    #[test]
    fn fit_takes_the_maximum_below_and_fills_holes_from_the_next_larger_entries() {
        let m = |post, events, proofs, max_cu| Measured { post, events, proofs, max_cu };
        let (post, failt) = fit(&[m(true, 5, 2, 100), m(true, 3, 1, 40), m(true, 8, 3, 300)]);
        // (3,1) is measured; (5,2) the larger of its own and everything below; (8,3) the largest
        assert_eq!((post[3][1], post[5][2], post[8][3]), (40, 100, 300));
        assert_eq!((post[4][1], post[3][3], post[4][2]), (40, 40, 40));
        assert_eq!((post[5][1], post[5][3], post[7][3], post[8][2]), (40, 100, 100, 100));
        // holes: nothing at or below (2,3) is measured, so the smallest non-zero neighbour above or to the right fills it
        assert_eq!((post[0][0], post[2][3], post[2][0]), (40, 40, 40));
        // a kind nobody measured stays zero
        assert!(failt.iter().all(|r| r.iter().all(|x| *x == 0)));
        // the hole at the last row and column is filled from its left and upper neighbours' larger entries
        let (only, _) = fit(&[m(true, 8, 3, 7)]);
        assert!(only.iter().all(|r| r.iter().all(|x| *x == 7)));
    }
}
