//! The transaction model behind the SPLIT rule: how many bytes the signed `post_journal` / `report_fail` transaction
//! takes and how many compute units it may use. Sizes are exact (Solana legacy transaction format, one signature);
//! compute units are a conservative bound whose constants were fitted to the M5b/M7 measurements and are checked
//! against the real program in `tests-rs/tests/differential.rs` for every batch of the gate run.
use crate::types::BatchKind;

/// Maximum size of a serialized Solana transaction.
pub const TX_LIMIT: usize = 1_232;
/// The compute limit a transaction gets WITHOUT a `SetComputeUnitLimit`. The batches of this crate never rely on it (P27).
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

/// Accounts of a `post_journal` transaction that an address lookup table can supply: config, rule set, event authority (and the markets).
pub const POST_LOOKUP_ACCOUNTS: usize = 3;
/// ... of a `report_fail` transaction: config, rule set, sector authority, vault program, product registry, event authority (and the markets).
/// The trading account, the wallet and the vault's trader state belong to one trader and stay in the transaction; so do the fee payer,
/// the program that is called and the compute-budget program (a program id of an instruction must be a static key).
pub const FAIL_LOOKUP_ACCOUNTS: usize = 6;

/// Size in bytes of the same transaction as a VERSION-0 message with one address lookup table that supplies the accounts above and the
/// markets: the version byte, the shorter list of static keys, and the lookup (table key 32, two index lists of one length byte each,
/// one byte per account loaded).
pub fn tx_size_v0(kind: BatchKind, n_markets: usize, batch_len: usize, compute_budget_ix: bool) -> usize {
    let (fixed, lookup_fixed) = match kind {
        BatchKind::PostJournal => (POST_FIXED_ACCOUNTS, POST_LOOKUP_ACCOUNTS),
        BatchKind::ReportFail(_) => (FAIL_FIXED_ACCOUNTS, FAIL_LOOKUP_ACCOUNTS),
    };
    let n_keys = fixed + n_markets + usize::from(compute_budget_ix);
    let loaded = lookup_fixed + n_markets;
    let static_keys = n_keys - loaded;
    tx_size(kind, n_markets, batch_len, compute_budget_ix) - (shortvec_len(n_keys) + 32 * n_keys)
        + (shortvec_len(static_keys) + 32 * static_keys)
        + 1 // the version byte 0x80
        + 1 // the number of lookups (one)
        + 32 + 1 + 1 + loaded // table key, writable index list, read-only index list, one byte per loaded account
}

/// The worst compute units measured on a real runtime for a batch of this shape (`compute::measured_cu`). Kept under its old name: the
/// 200,000-unit default is NOT enough for the largest batches, so a transaction must request `compute::recommended_cu_limit`.
pub fn cu_estimate(kind: BatchKind, n_events: usize, n_proofs: usize) -> u64 {
    crate::compute::measured_cu(kind, n_events, n_proofs)
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
    fn the_cu_model_is_the_runtime_measurement_and_exceeds_the_default_limit_for_the_largest_batches() {
        let p = BatchKind::PostJournal;
        let f = BatchKind::ReportFail(FailReason::PhaseQuick);
        assert_eq!(cu_estimate(p, 4, 2), crate::compute::measured_cu(p, 4, 2));
        // measured on Agave 4.0.2: a report_fail of 7 events and 3 proofs used 196,812 units, 8 events more than the 200,000 default
        assert!(cu_estimate(f, 7, 3) >= 196_812);
        assert!(cu_estimate(f, 8, 3) > CU_LIMIT);
    }
}
