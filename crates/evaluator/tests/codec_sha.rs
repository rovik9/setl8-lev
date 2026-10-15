//! The hand-written SHA-256 against the `sha2` crate, and the evaluator's byte formats against the verifier's.
mod common;
use evaluator::sha256::{sha256, sha256_parts, Sha256State};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]
    #[test]
    fn sha256_equals_the_crate(data in proptest::collection::vec(any::<u8>(), 0..400)) {
        let want: [u8; 32] = Sha256::digest(&data).into();
        prop_assert_eq!(sha256(&data), want);
    }

    #[test]
    fn incremental_equals_one_shot(data in proptest::collection::vec(any::<u8>(), 0..300), cut in 0usize..300) {
        let cut = cut.min(data.len());
        let mut s = Sha256State::new();
        s.update(&data[..cut]);
        let mid = s.finalize();
        let want_mid: [u8; 32] = Sha256::digest(&data[..cut]).into();
        prop_assert_eq!(mid, want_mid, "finalize does not consume or disturb the state");
        s.update(&data[cut..]);
        let want: [u8; 32] = Sha256::digest(&data).into();
        prop_assert_eq!(s.finalize(), want);
        prop_assert_eq!(sha256_parts(&[&data[..cut], &data[cut..]]), want);
    }
}

#[test]
fn the_chain_hasher_equals_the_native_one() {
    let h = evaluator::sha256::chain_hasher();
    let acct = [3u8; 32];
    assert_eq!(journal_types::genesis_hash(&h, &acct), journal_types::genesis_hash(&journal_types::NativeSha256, &acct));
}
