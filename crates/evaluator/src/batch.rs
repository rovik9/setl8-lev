//! The transaction model behind the SPLIT rule: how many bytes the signed `post_journal` / `report_fail` transaction
//! takes and how many compute units it may use. Sizes are exact (Solana legacy transaction format, one signature);
//! compute units are a conservative bound whose constants were fitted to the M5b/M7 measurements and are checked
//! against the real program in `tests-rs/tests/differential.rs` for every batch of the gate run.
use crate::types::BatchKind;

/// Maximum size of a serialized Solana transaction.
pub const TX_LIMIT: usize = 1_232;
/// Default compute limit per instruction.
pub const CU_LIMIT: u64 = 200_000;

/// Unique accounts of a `post_journal` transaction without markets: evaluator, trading account, config, rule set,
/// event authority, program.
pub const POST_FIXED_ACCOUNTS: usize = 6;
/// Unique accounts of a `report_fail` transaction without markets: evaluator, trading account, config, rule set,
/// wallet, sector authority, vault program, product registry, vault trader state, event authority, program.
pub const FAIL_FIXED_ACCOUNTS: usize = 11;

/// Length of Solana's compact-u16 ("shortvec") encoding of `n`.
pub fn shortvec_len(n: usize) -> usize {
    if n < 0x80 {
        1
    } else if n < 0x4000 {
        2
    } else {
        3
    }
}

/// Size in bytes of the signed legacy transaction that carries one `post_journal` or `report_fail` instruction whose
/// `JournalBatch` encodes to `batch_len` bytes and that names `n_markets` market accounts. One signature (the
/// evaluator, also the fee payer); with `compute_budget_ix` a second instruction `SetComputeUnitLimit` is counted.
pub fn tx_size(kind: BatchKind, n_markets: usize, batch_len: usize, compute_budget_ix: bool) -> usize {
    let (fixed, data_len) = match kind {
        // 8-byte discriminator + borsh Vec<u8> (u32 length + bytes)
        BatchKind::PostJournal => (POST_FIXED_ACCOUNTS, 8 + 4 + batch_len),
        // + the reason byte
        BatchKind::ReportFail(_) => (FAIL_FIXED_ACCOUNTS, 8 + 4 + batch_len + 1),
    };
    let ix_accounts = fixed + n_markets;
    let n_keys = ix_accounts + usize::from(compute_budget_ix);
    let ix = 1 + shortvec_len(ix_accounts) + ix_accounts + shortvec_len(data_len) + data_len;
    // SetComputeUnitLimit: program index, no accounts, 5 data bytes
    let budget = if compute_budget_ix { 1 + 1 + 1 + 5 } else { 0 };
    1 + 64 // signature count and the signature
        + 3 // message header
        + shortvec_len(n_keys) + 32 * n_keys
        + 32 // recent blockhash
        + 1 /* instruction count: one or two, always one byte */ + ix + budget
}

/// Conservative compute-unit bound of a batch with `n_events` events (the `Meta` included) and `n_proofs` proofs.
/// Fitted to: 1 event 30,760; 4 events 65,056; 8 events 107,685; 8 events with 3 proofs 124,445; `report_fail` with
/// 8 events and 3 proofs 158,394 (handover/module-5b.md, module-7.md).
pub fn cu_estimate(kind: BatchKind, n_events: usize, n_proofs: usize) -> u64 {
    let base = 30_000u64;
    let per_event = 12_500u64;
    let per_proof = 8_000u64;
    let fail_extra = match kind {
        BatchKind::PostJournal => 0,
        BatchKind::ReportFail(_) => 40_000,
    };
    base + per_event * n_events as u64 + per_proof * n_proofs as u64 + fail_extra
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FailReason;

    #[test]
    fn shortvec_boundaries() {
        assert_eq!([shortvec_len(0), shortvec_len(127), shortvec_len(128), shortvec_len(16_383), shortvec_len(16_384)], [1, 1, 2, 2, 3]);
    }

    /// A post_journal transaction with a 100-byte batch and no market: 1+64+3+(1+192)+32+1+(1+1+6+1+112) = 415.
    #[test]
    fn tx_size_by_hand() {
        assert_eq!(tx_size(BatchKind::PostJournal, 0, 100, false), 1 + 64 + 3 + 1 + 6 * 32 + 32 + 1 + (1 + 1 + 6 + 1 + 112));
        // one market adds a key, an account index and nothing else; 200 bytes of data take a 2-byte length
        assert_eq!(tx_size(BatchKind::PostJournal, 1, 116, false), 1 + 64 + 3 + 1 + 7 * 32 + 32 + 1 + (1 + 1 + 7 + 2 + 128));
        // the budget instruction adds its key (32) and 8 bytes of instruction
        assert_eq!(tx_size(BatchKind::PostJournal, 0, 100, true), tx_size(BatchKind::PostJournal, 0, 100, false) + 32 + 8);
        // report_fail: 11 fixed accounts and one more data byte
        assert_eq!(tx_size(BatchKind::ReportFail(FailReason::StaticFloor), 0, 100, false), 1 + 64 + 3 + 1 + 11 * 32 + 32 + 1 + (1 + 1 + 11 + 1 + 113));
    }

    #[test]
    fn cu_bound_covers_the_measurements() {
        let p = BatchKind::PostJournal;
        assert!(cu_estimate(p, 1, 0) >= 30_760);
        assert!(cu_estimate(p, 4, 0) >= 65_056);
        assert!(cu_estimate(p, 8, 0) >= 107_685);
        assert!(cu_estimate(p, 8, 3) >= 124_445);
        assert!(cu_estimate(BatchKind::ReportFail(FailReason::PhaseQuick), 8, 3) >= 158_394);
        assert!(cu_estimate(BatchKind::ReportFail(FailReason::PhaseQuick), 8, 3) <= CU_LIMIT);
        // the bound is exactly its three constants
        assert_eq!(cu_estimate(p, 0, 0), 30_000);
        assert_eq!(cu_estimate(p, 1, 0) - cu_estimate(p, 0, 0), 12_500);
        assert_eq!(cu_estimate(p, 0, 1) - cu_estimate(p, 0, 0), 8_000);
        assert_eq!(cu_estimate(BatchKind::ReportFail(FailReason::StaticFloor), 0, 0) - 30_000, 40_000);
        // the largest batch the chain allows fits the default limit, so the limit never binds before 8 events and 3 proofs
        assert!(cu_estimate(BatchKind::ReportFail(FailReason::StaticFloor), 8, 3) <= CU_LIMIT);
    }
}
