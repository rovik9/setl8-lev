mod common;
use common::*;
use journal_types::*;

const T: i64 = 1_760_000_000;
const MAX_AGE: u32 = 60;
const PRICE: u64 = 67_250_000_000_000_000; // $67,250.00 in 1e-12 USD

fn good() -> PriceProof {
    PriceProof {
        source: 2,
        feed_id: feed(2, "BTC"),
        timestamp_ns: u64::try_from(T).unwrap() * 1_000_000_000,
        quantized_value: i128::from(PRICE) * 1_000_000,
        signed_update: vec![],
    }
}

fn check(p: &PriceProof) -> Result<(), ProofError> {
    check_proof(p, &feed(2, "BTC"), MAX_AGE, T, PRICE, false)
}

#[test]
fn a_matching_proof_is_accepted_for_every_real_source() {
    assert_eq!(check(&good()), Ok(()));
    for source in [2u8, 3, 4] {
        let p = PriceProof { source, ..good() };
        assert_eq!(check_proof(&p, &p.feed_id, MAX_AGE, T, PRICE, false), Ok(()), "source {source}");
    }
}

#[test]
fn wrong_feed_is_rejected() {
    let mut p = good();
    p.feed_id[31] ^= 1;
    assert_eq!(check(&p), Err(ProofError::WrongFeed));
    // the same symbol under another source is another feed
    let q = PriceProof { feed_id: feed(3, "BTC"), ..good() };
    assert_eq!(check(&q), Err(ProofError::WrongFeed));
}

#[test]
fn a_price_off_by_one_is_rejected_in_both_directions() {
    assert_eq!(check_proof(&good(), &feed(2, "BTC"), MAX_AGE, T, PRICE + 1, false), Err(ProofError::PriceMismatch));
    assert_eq!(check_proof(&good(), &feed(2, "BTC"), MAX_AGE, T, PRICE - 1, false), Err(ProofError::PriceMismatch));
    // the conversion floors: a value 999_999 above the multiple still means the same price
    let p = PriceProof { quantized_value: i128::from(PRICE) * 1_000_000 + 999_999, ..good() };
    assert_eq!(check(&p), Ok(()));
    let p = PriceProof { quantized_value: i128::from(PRICE) * 1_000_000 + 1_000_000, ..good() };
    assert_eq!(check(&p), Err(ProofError::PriceMismatch));
}

#[test]
fn exactly_max_age_old_is_accepted_and_one_second_older_is_rejected() {
    let at = |t: i64| PriceProof { timestamp_ns: u64::try_from(t).unwrap() * 1_000_000_000, ..good() };
    assert_eq!(check(&at(T - i64::from(MAX_AGE))), Ok(()));
    assert_eq!(check(&at(T - i64::from(MAX_AGE) - 1)), Err(ProofError::TooOld));
    // sub-second part: 59.999999999 s old still counts as 60 s after flooring the proof time
    let p = PriceProof { timestamp_ns: u64::try_from(T - i64::from(MAX_AGE)).unwrap() * 1_000_000_000 + 999_999_999, ..good() };
    assert_eq!(check(&p), Ok(()));
    // max_age 0: only the same second (or up to 2 s ahead) is fine
    assert_eq!(check_proof(&at(T), &feed(2, "BTC"), 0, T, PRICE, false), Ok(()));
    assert_eq!(check_proof(&at(T - 1), &feed(2, "BTC"), 0, T, PRICE, false), Err(ProofError::TooOld));
}

#[test]
fn two_seconds_in_the_future_is_accepted_and_three_is_rejected() {
    let at = |t: i64| PriceProof { timestamp_ns: u64::try_from(t).unwrap() * 1_000_000_000, ..good() };
    assert_eq!(check(&at(T + 2)), Ok(()));
    assert_eq!(check(&at(T + 3)), Err(ProofError::InFuture));
    // 2.999999999 s ahead floors to 2 s: accepted
    let p = PriceProof { timestamp_ns: u64::try_from(T + 2).unwrap() * 1_000_000_000 + 999_999_999, ..good() };
    assert_eq!(check(&p), Ok(()));
}

#[test]
fn unknown_sources_are_rejected() {
    for source in [0u8, 1, 5, 100, 254] {
        let p = PriceProof { source, ..good() };
        assert_eq!(check(&p), Err(ProofError::UnknownSource), "source {source}");
        assert_eq!(check_proof(&p, &feed(2, "BTC"), MAX_AGE, T, PRICE, true), Err(ProofError::UnknownSource), "source {source} with mocks allowed");
    }
}

#[test]
fn mock_source_needs_allow_mock() {
    let p = PriceProof { source: 255, ..good() };
    assert_eq!(check_proof(&p, &feed(2, "BTC"), MAX_AGE, T, PRICE, false), Err(ProofError::MockNotAllowed));
    assert_eq!(check_proof(&p, &feed(2, "BTC"), MAX_AGE, T, PRICE, true), Ok(()));
    // mocks still go through the other checks
    let wrong = PriceProof { source: 255, feed_id: [1; 32], ..good() };
    assert_eq!(check_proof(&wrong, &feed(2, "BTC"), MAX_AGE, T, PRICE, true), Err(ProofError::WrongFeed));
    assert_eq!(check_proof(&p, &feed(2, "BTC"), MAX_AGE, T, PRICE + 1, true), Err(ProofError::PriceMismatch));
}

#[test]
fn signed_update_512_is_accepted_and_513_rejected() {
    let p = PriceProof { source: 3, signed_update: vec![1; 512], ..good() };
    assert_eq!(check(&p), Ok(()));
    let p = PriceProof { source: 3, signed_update: vec![1; 513], ..good() };
    assert_eq!(check(&p), Err(ProofError::SignedUpdateTooLong));
}

#[test]
fn check_order_is_source_then_length_then_feed_then_price_then_time() {
    let p = PriceProof { source: 7, signed_update: vec![0; 600], feed_id: [0; 32], quantized_value: 1, timestamp_ns: 0 };
    assert_eq!(check(&p), Err(ProofError::UnknownSource));
    let p = PriceProof { signed_update: vec![0; 600], feed_id: [0; 32], quantized_value: 1, timestamp_ns: 0, ..good() };
    assert_eq!(check(&p), Err(ProofError::SignedUpdateTooLong));
    let p = PriceProof { feed_id: [0; 32], quantized_value: 1, timestamp_ns: 0, ..good() };
    assert_eq!(check(&p), Err(ProofError::WrongFeed));
    let p = PriceProof { quantized_value: 5_000_000, timestamp_ns: 0, ..good() };
    assert_eq!(check(&p), Err(ProofError::PriceMismatch));
    let p = PriceProof { timestamp_ns: 0, ..good() };
    assert_eq!(check(&p), Err(ProofError::TooOld));
}

#[test]
fn non_positive_or_huge_values_are_rejected_by_the_check() {
    for v in [0i128, -1, -1_000_000_000_000] {
        let p = PriceProof { quantized_value: v, ..good() };
        assert_eq!(check(&p), Err(ProofError::ValueNotPositive), "{v}");
    }
    let p = PriceProof { quantized_value: i128::MAX, ..good() };
    assert_eq!(check(&p), Err(ProofError::ValueTooLarge));
}

#[test]
fn proof_time_floors_the_nanoseconds() {
    let p = |ns: u64| PriceProof { timestamp_ns: ns, ..good() };
    assert_eq!(proof_time_secs(&p(0)), 0);
    assert_eq!(proof_time_secs(&p(999_999_999)), 0);
    assert_eq!(proof_time_secs(&p(1_000_000_000)), 1);
    assert_eq!(proof_time_secs(&p(1_999_999_999)), 1);
    assert_eq!(proof_time_secs(&p(u64::MAX)), 18_446_744_073);
}

#[test]
fn price_conversion_floors_and_rejects_zero_negative_and_overflow() {
    assert_eq!(price_e12_from_quantized(1_000_000), Ok(1));
    assert_eq!(price_e12_from_quantized(1_999_999), Ok(1));
    assert_eq!(price_e12_from_quantized(2_000_000), Ok(2));
    assert_eq!(price_e12_from_quantized(999_999), Err(ProofError::ValueNotPositive));
    assert_eq!(price_e12_from_quantized(1), Err(ProofError::ValueNotPositive));
    assert_eq!(price_e12_from_quantized(0), Err(ProofError::ValueNotPositive));
    assert_eq!(price_e12_from_quantized(-1), Err(ProofError::ValueNotPositive));
    assert_eq!(price_e12_from_quantized(i128::MIN), Err(ProofError::ValueNotPositive));
    let max = i128::from(u64::MAX) * 1_000_000;
    assert_eq!(price_e12_from_quantized(max), Ok(u64::MAX));
    assert_eq!(price_e12_from_quantized(max + 999_999), Ok(u64::MAX));
    assert_eq!(price_e12_from_quantized(max + 1_000_000), Err(ProofError::ValueTooLarge));
    assert_eq!(price_e12_from_quantized(i128::MAX), Err(ProofError::ValueTooLarge));
    // an 18-decimal $67,250.123456789012 -> 1e-12 USD
    assert_eq!(price_e12_from_quantized(67_250_123_456_789_012_345_678), Ok(67_250_123_456_789_012));
}

// ---------------------------------------------------------------- M5b additions

#[test]
fn signed_update_shape_follows_decision_47() {
    let with = |source: u8, len: usize| PriceProof { source, signed_update: vec![1; len], ..good() };
    // sources 2 and 4: empty only
    for source in [2u8, 4] {
        assert_eq!(check_signed_update_shape(&with(source, 0)), Ok(()), "source {source} empty");
        assert_eq!(check_signed_update_shape(&with(source, 1)), Err(ProofError::BadSignedUpdate), "source {source} 1 byte");
        assert_eq!(check_signed_update_shape(&with(source, 32)), Err(ProofError::BadSignedUpdate), "source {source} 32 bytes");
    }
    // source 3: exactly 32 bytes
    for len in [0usize, 1, 31, 33, 64, 512] {
        assert_eq!(check_signed_update_shape(&with(3, len)), Err(ProofError::BadSignedUpdate), "source 3 len {len}");
    }
    assert_eq!(check_signed_update_shape(&with(3, 32)), Ok(()));
    assert_eq!(SIGNED_UPDATE_DIGEST_BYTES, 32);
    // mock and unsupported sources: the shape rule does not apply (check_proof enforces the 512 limit)
    assert_eq!(check_signed_update_shape(&with(255, 512)), Ok(()));
    assert_eq!(check_signed_update_shape(&with(255, 0)), Ok(()));
    assert_eq!(check_signed_update_shape(&with(7, 100)), Ok(()));
}

#[test]
fn proof_check_without_a_price_still_checks_everything_else() {
    let chk = |p: &PriceProof, allow_mock: bool| check_proof_without_price(p, &feed(2, "BTC"), MAX_AGE, T, allow_mock);
    assert_eq!(chk(&good(), false), Ok(()));
    // no event price to mismatch: any positive value is fine
    assert_eq!(chk(&PriceProof { quantized_value: 5_000_000, ..good() }, false), Ok(()));
    assert_eq!(chk(&PriceProof { quantized_value: 0, ..good() }, false), Err(ProofError::ValueNotPositive));
    assert_eq!(chk(&PriceProof { quantized_value: -1, ..good() }, false), Err(ProofError::ValueNotPositive));
    assert_eq!(chk(&PriceProof { quantized_value: 999_999, ..good() }, false), Err(ProofError::ValueNotPositive));
    assert_eq!(chk(&PriceProof { quantized_value: i128::MAX, ..good() }, false), Err(ProofError::ValueTooLarge));
    let mut wrong_feed = good();
    wrong_feed.feed_id[0] ^= 1;
    assert_eq!(chk(&wrong_feed, false), Err(ProofError::WrongFeed));
    assert_eq!(chk(&PriceProof { source: 7, ..good() }, false), Err(ProofError::UnknownSource));
    assert_eq!(chk(&PriceProof { source: 255, ..good() }, false), Err(ProofError::MockNotAllowed));
    assert_eq!(chk(&PriceProof { source: 255, ..good() }, true), Ok(()));
    assert_eq!(chk(&PriceProof { signed_update: vec![0; 513], ..good() }, false), Err(ProofError::SignedUpdateTooLong));
    // time window, both ends inclusive
    let at = |t: i64| PriceProof { timestamp_ns: u64::try_from(t).unwrap() * 1_000_000_000, ..good() };
    assert_eq!(chk(&at(T - i64::from(MAX_AGE)), false), Ok(()));
    assert_eq!(chk(&at(T - i64::from(MAX_AGE) - 1), false), Err(ProofError::TooOld));
    assert_eq!(chk(&at(T + 2), false), Ok(()));
    assert_eq!(chk(&at(T + 3), false), Err(ProofError::InFuture));
}

#[test]
fn check_proof_with_a_price_is_unchanged_by_the_refactor() {
    assert_eq!(check(&good()), Ok(()));
    assert_eq!(check_proof(&good(), &feed(2, "BTC"), MAX_AGE, T, PRICE + 1, false), Err(ProofError::PriceMismatch));
    assert_eq!(check_proof(&good(), &feed(2, "BTC"), MAX_AGE, T, PRICE - 1, false), Err(ProofError::PriceMismatch));
}
