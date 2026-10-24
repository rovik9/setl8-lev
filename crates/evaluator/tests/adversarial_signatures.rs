//! M12 item 8 (ed25519, evaluator side): the evaluator takes its signature check as an injected `IntentVerifier`. This file
//! attacks the three implementations that exist in the repo and states which one a production backend must use:
//!   - `Ed25519Check` (tests/common/mod.rs): ed25519-dalek 1.0.1 `PublicKey::verify` (plain, NOT strict). The harness verifier.
//!   - `StrictEd25519` (below): the same crate, `PublicKey::verify_strict`. What a production backend must use.
//!   - `AcceptAll` (src/eval.rs): accepts everything; for tests of the book itself only. The public verifier catches its output.
//!
//!
//! Classes: BLOCKED = the refusal and its exact reason are asserted; LIMITED = what still passes is pinned; OFF-CHAIN = the
//! control lives in the verifier or backend. Findings carry the id VRF-n and are `#[ignore]`d (they assert the DESIGN behaviour).
mod common;
use common::*;
use ed25519_dalek::{PublicKey, Signature, Signer, Verifier};
use evaluator::codec::{signing_message, OrderIntent};
use evaluator::{AcceptAll, Evaluator, IntentVerifier, OrderReject as R, SignedIntent};

/// The group order L of the ed25519 base point, little endian.
const L_LE: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

/// 256-bit little-endian addition; `None` on carry out of 256 bits.
fn add_le(a: &[u8; 32], b: &[u8; 32]) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in 0..32 {
        let v = u16::from(a[i]) + u16::from(b[i]) + carry;
        out[i] = (v & 0xff) as u8;
        carry = v >> 8;
    }
    if carry == 0 {
        Some(out)
    } else {
        None
    }
}

/// The identity point, the order-2 point, the two order-4 points (sign bit 0 and 1) and the order-8 points (all with both
/// sign bits). All 32-byte encodings of small-order points that decompress (the standard list of 8 points, plus the
/// non-canonical encodings of the identity and of the order-2 point).
fn small_order_encodings() -> Vec<[u8; 32]> {
    let hex = |s: &str| -> [u8; 32] {
        let mut o = [0u8; 32];
        for (i, b) in o.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        o
    };
    let cands = [
        "0100000000000000000000000000000000000000000000000000000000000000", // identity (0,1)
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f", // order 2 (0,-1)
        "0000000000000000000000000000000000000000000000000000000000000000", // order 4
        "0000000000000000000000000000000000000000000000000000000000000080", // order 4
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a", // order 8
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa", // order 8
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05", // order 8
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85", // order 8
        // non-canonical encodings (y + p) of the identity and of the order-2 point
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    ];
    cands.iter().map(|c| hex(c)).collect()
}

const IDENTITY: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 1;
    b
};

/// What a production backend must use: strict verification (rejects a non-canonical S, and a small-order R or public key).
struct StrictEd25519;
impl IntentVerifier for StrictEd25519 {
    fn verify(&self, trader: &[u8; 32], message: &[u8], sig: &[u8; 64]) -> bool {
        let Ok(pk) = PublicKey::from_bytes(trader) else { return false };
        let Ok(sig) = Signature::from_bytes(sig) else { return false };
        pk.verify_strict(message, &sig).is_ok()
    }
}

fn keypair_from_seed(seed: u8) -> ed25519_dalek::Keypair {
    let secret = ed25519_dalek::SecretKey::from_bytes(&[seed; 32]).expect("seed");
    let public: PublicKey = (&secret).into();
    ed25519_dalek::Keypair { secret, public }
}

fn honest(account: &[u8; 32], nonce: u32) -> SignedIntent {
    sign(account, nonce, &market_order(0, true, MEUR, T_1400))
}

/// A world whose evaluator uses `v`, with one fresh sample so an order can be validated against a priced market.
fn world_with(v: Box<dyn IntentVerifier + Send>, trader: [u8; 32]) -> World {
    let mut p = params(S100K, 2, T_1400 - HOUR);
    p.trader = trader;
    let mut w = World::from_params(base_rules(vec![fx_market()]), p.clone());
    w.ev = Evaluator::new(w.rules.clone(), p, v).expect("config");
    w.sample(0, T_1400, p4(10_000));
    w
}

fn fx_market() -> evaluator::MarketCfg {
    let mut m = eur_market(0);
    m.half_spread_e6 = TEST_HALF_SPREAD_E6;
    m.max_age_secs = 3_600;
    m
}

fn honest_world(v: Box<dyn IntentVerifier + Send>) -> World {
    world_with(v, trader_keypair().public.to_bytes())
}

type MakeVerifier = fn() -> Box<dyn IntentVerifier + Send>;

fn both() -> Vec<(&'static str, MakeVerifier)> {
    vec![("Ed25519Check (plain verify)", || Box::new(Ed25519Check)), ("StrictEd25519 (verify_strict)", || Box::new(StrictEd25519))]
}

// ---------------------------------------------------------------------------------------------------------------------
// control: the honest path works with both implementations, so the refusals below are not an artefact

#[test]
fn control_an_honest_signature_is_accepted_by_both_implementations() {
    for (name, mk) in both() {
        let mut w = honest_world(mk());
        let acct = w.opened.account;
        assert_eq!(w.ev.on_order(&honest(&acct, 1)), Ok(()), "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: malleated S (S + L) is refused. ed25519-dalek 1.0.1 without the `legacy_compatibility` feature checks that S is
// reduced (Scalar::from_canonical_bytes); this test breaks if that feature is ever switched on.

#[test]
fn blocked_malleated_s_plus_l_is_refused_with_bad_signature() {
    for (name, mk) in both() {
        let mut w = honest_world(mk());
        let acct = w.opened.account;
        let good = honest(&acct, 1);
        let s: [u8; 32] = good.sig[32..].try_into().unwrap();
        let s_plus_l = add_le(&s, &L_LE).expect("S + L fits in 256 bits");
        // S + L is below 2^253, so the "top 3 bits clear" legacy check alone would let it through
        assert_eq!(s_plus_l[31] & 0xe0, 0, "the malleated S must be a value the legacy check accepts, else this test proves nothing");
        let mut bad = good;
        bad.sig[32..].copy_from_slice(&s_plus_l);
        assert_ne!(bad.sig, good.sig);
        assert_eq!(w.ev.on_order(&bad), Err(R::BadSignature), "{name}");
        // nothing was recorded: the nonce is still free and the honest original is accepted afterwards
        assert_eq!(w.ev.on_order(&good), Ok(()), "{name}");
    }
}

#[test]
fn blocked_other_non_canonical_s_values_are_refused() {
    for (name, mk) in both() {
        let mut w = honest_world(mk());
        let acct = w.opened.account;
        let good = honest(&acct, 1);
        let s: [u8; 32] = good.sig[32..].try_into().unwrap();
        let mut cases: Vec<(&str, [u8; 32])> = vec![("S = L", L_LE), ("S = 2L", add_le(&L_LE, &L_LE).unwrap()), ("S + 2L", add_le(&s, &add_le(&L_LE, &L_LE).unwrap()).unwrap())];
        let mut top = s;
        top[31] |= 0x80;
        cases.push(("top bit set", top));
        let mut top3 = s;
        top3[31] |= 0x20;
        cases.push(("bit 253 set", top3));
        cases.push(("S = 2^256 - 1", [0xff; 32]));
        for (label, bad_s) in cases {
            let mut bad = good;
            bad.sig[32..].copy_from_slice(&bad_s);
            assert_eq!(w.ev.on_order(&bad), Err(R::BadSignature), "{name}: {label}");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: the other malformed signatures

#[test]
fn blocked_all_zero_all_ff_and_half_zero_signatures_are_refused() {
    for (name, mk) in both() {
        let mut w = honest_world(mk());
        let acct = w.opened.account;
        let good = honest(&acct, 1);
        let mut variants: Vec<(&str, [u8; 64])> = vec![("all zero", [0; 64]), ("all 0xff", [0xff; 64])];
        let mut r_zero = good.sig;
        r_zero[..32].fill(0);
        variants.push(("R zero", r_zero));
        let mut s_zero = good.sig;
        s_zero[32..].fill(0);
        variants.push(("S zero", s_zero));
        let mut r_id = good.sig;
        r_id[..32].copy_from_slice(&IDENTITY);
        r_id[32..].fill(0);
        variants.push(("R identity, S zero", r_id));
        for (label, sig) in variants {
            let s = SignedIntent { sig, ..good };
            assert_eq!(w.ev.on_order(&s), Err(R::BadSignature), "{name}: {label}");
        }
    }
}

#[test]
fn blocked_every_single_bit_flip_of_the_signature_is_refused() {
    for (name, mk) in both() {
        let mut w = honest_world(mk());
        let acct = w.opened.account;
        let good = honest(&acct, 1);
        for bit in 0..512usize {
            let mut sig = good.sig;
            sig[bit / 8] ^= 1 << (bit % 8);
            assert_eq!(w.ev.on_order(&SignedIntent { sig, ..good }), Err(R::BadSignature), "{name}: bit {bit}");
        }
    }
}

#[test]
fn blocked_a_signature_for_another_message_is_refused() {
    for (name, mk) in both() {
        let mut w = honest_world(mk());
        let acct = w.opened.account;
        let good = honest(&acct, 1);
        // same signature, one byte of the order changed (a different market and a different quantity)
        for pos in [0usize, 1, 4, 12, 40] {
            let mut order = good.order;
            order[pos] ^= 1;
            assert_eq!(w.ev.on_order(&SignedIntent { order, ..good }), Err(R::BadSignature), "{name}: order byte {pos}");
        }
        // same signature, another nonce
        assert_eq!(w.ev.on_order(&SignedIntent { nonce: 2, ..good }), Err(R::BadSignature), "{name}: nonce");
        // a valid signature for another ACCOUNT (a replay of an order signed for a different challenge)
        let other = sign(&[0x55; 32], 1, &market_order(0, true, MEUR, T_1400));
        assert_eq!(w.ev.on_order(&other), Err(R::BadSignature), "{name}: other account");
        // a valid signature of the right order and nonce but over a message without the domain prefix
        let kp = trader_keypair();
        let mut raw = signing_message(&acct, 1, &good.order);
        raw.drain(..7); // drop "SL8LEV|"
        let no_prefix = SignedIntent { sig: kp.sign(&raw).to_bytes(), ..good };
        assert_eq!(w.ev.on_order(&no_prefix), Err(R::BadSignature), "{name}: no domain prefix");
        // a valid signature by ANOTHER key over the right message
        let other_key = keypair_from_seed(0x77);
        let by_other = SignedIntent { sig: other_key.sign(&signing_message(&acct, 1, &good.order)).to_bytes(), ..good };
        assert_eq!(w.ev.on_order(&by_other), Err(R::BadSignature), "{name}: other key");
        // the honest one still passes (no state leaked from the refusals)
        assert_eq!(w.ev.on_order(&good), Ok(()), "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// BLOCKED: points that are not on the curve

/// A 32-byte string with the sign bit clear that is not the y coordinate of any curve point.
fn off_curve_encoding() -> [u8; 32] {
    for y in 2u16..2000 {
        let mut b = [0u8; 32];
        b[..2].copy_from_slice(&y.to_le_bytes());
        // PublicKey::from_bytes decompresses the point: an error means "not on the curve"
        if PublicKey::from_bytes(&b).is_err() {
            return b;
        }
    }
    panic!("no off-curve y found below 2000");
}

#[test]
fn blocked_r_not_on_the_curve_is_refused() {
    let bad_point = off_curve_encoding();
    for (name, mk) in both() {
        let mut w = honest_world(mk());
        let acct = w.opened.account;
        let good = honest(&acct, 1);
        let mut sig = good.sig;
        sig[..32].copy_from_slice(&bad_point);
        assert_eq!(w.ev.on_order(&SignedIntent { sig, ..good }), Err(R::BadSignature), "{name}");
    }
}

#[test]
fn blocked_a_trader_key_that_is_not_on_the_curve_can_never_sign_anything() {
    let bad_point = off_curve_encoding();
    for (name, mk) in both() {
        let mut w = world_with(mk(), bad_point);
        let acct = w.opened.account;
        // every signature, including one made with the honest harness key and an all-zero one, is refused
        assert_eq!(w.ev.on_order(&honest(&acct, 1)), Err(R::BadSignature), "{name}");
        let zero = SignedIntent { sig: [0; 64], ..honest(&acct, 2) };
        assert_eq!(w.ev.on_order(&zero), Err(R::BadSignature), "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the library facts the findings rest on, pinned at the crypto level (ed25519-dalek 1.0.1, curve25519-dalek 3.2.0)

#[test]
fn pin_dalek_1_0_1_plain_verify_accepts_a_small_order_key_and_verify_strict_does_not() {
    let msg = b"any message at all";
    let mut sig_bytes = [0u8; 64];
    sig_bytes[..32].copy_from_slice(&IDENTITY); // R = identity, S = 0
    let sig = Signature::from_bytes(&sig_bytes).expect("S = 0 is canonical");
    let pk = PublicKey::from_bytes(&IDENTITY).expect("the identity decompresses");
    assert!(pk.verify(msg, &sig).is_ok(), "plain verify accepts the identity key with (R = identity, S = 0) on ANY message");
    assert!(pk.verify_strict(msg, &sig).is_err(), "verify_strict rejects a small-order key / R");
    // the same for every small-order encoding: for each there is a message the zero-S forgery verifies for (the hash k must be 0
    // mod the point's order: about 1 message in 8 at worst)
    let mut seen = 0;
    for enc in small_order_encodings() {
        let Ok(pk) = PublicKey::from_bytes(&enc) else { continue };
        seen += 1;
        let found = (0u32..2000).any(|n| {
            let m = n.to_le_bytes();
            let ok = pk.verify(&m, &sig).is_ok();
            if ok {
                assert!(pk.verify_strict(&m, &sig).is_err(), "strict must refuse what plain accepts for a small-order key");
            }
            ok
        });
        assert!(found, "no message found for small-order key {enc:02x?}");
    }
    assert!(seen >= 8, "expected at least the 8 small-order points to decompress, got {seen}");
}

#[test]
fn pin_dalek_1_0_1_rejects_a_non_canonical_s_in_plain_verify() {
    // this is why the malleability test above is BLOCKED rather than a finding: plain verify checks S < L
    let kp = trader_keypair();
    let msg = b"m";
    let sig = kp.sign(msg);
    let s: [u8; 32] = sig.to_bytes()[32..].try_into().unwrap();
    let mut b = sig.to_bytes();
    b[32..].copy_from_slice(&add_le(&s, &L_LE).unwrap());
    // either the signature does not even parse, or verify refuses it
    match Signature::from_bytes(&b) {
        Err(_) => {}
        Ok(m) => assert!(kp.public.verify(msg, &m).is_err()),
    }
    assert!(kp.public.verify(msg, &sig).is_ok());
}

// ---------------------------------------------------------------------------------------------------------------------
// FINDING VRF-1 (low): the plain verifier accepts a forged order for an account whose trader key is a small-order point.
// A production backend must use verify_strict; the harness verifier (and crates/verifier, see VRF-2) use plain verify.

// FINDING VRF-1
#[test]
fn finding_vrf_1_plain_verify_accepts_an_unsigned_order_for_a_small_order_trader_key() {
    for enc in small_order_encodings().into_iter().filter(|e| PublicKey::from_bytes(e).is_ok()) {
        let mut w = world_with(Box::new(Ed25519Check), enc);
        // the forger signs nothing: R = identity, S = 0 over a message whose nonce is ground until the hash vanishes
        let mut accepted = false;
        for nonce in 1u32..200 {
            let intent = market_order(0, true, MEUR, T_1400);
            let mut sig = [0u8; 64];
            sig[..32].copy_from_slice(&IDENTITY);
            let s = SignedIntent { nonce, order: intent.to_bytes(), sig };
            if w.ev.on_order(&s) != Err(R::BadSignature) {
                accepted = true;
                break;
            }
        }
        assert!(!accepted, "an unsigned order was accepted for the small-order trader key {enc:02x?}");
    }
}

#[test]
fn pin_vrf_1_the_strict_implementation_refuses_the_same_forgeries() {
    for enc in small_order_encodings().into_iter().filter(|e| PublicKey::from_bytes(e).is_ok()) {
        let mut w = world_with(Box::new(StrictEd25519), enc);
        for nonce in 1u32..200 {
            let mut sig = [0u8; 64];
            sig[..32].copy_from_slice(&IDENTITY);
            let s = SignedIntent { nonce, order: market_order(0, true, MEUR, T_1400).to_bytes(), sig };
            assert_eq!(w.ev.on_order(&s), Err(R::BadSignature), "strict must refuse nonce {nonce} for {enc:02x?}");
        }
    }
}

#[test]
fn vrf_1_fixed_the_harness_verifier_refuses_the_zero_s_forgery_of_the_identity_key() {
    // after the fix (verify_strict in Ed25519Check) the identity-key forgery is refused for every nonce
    let mut w = world_with(Box::new(Ed25519Check), IDENTITY);
    for nonce in 1u32..=5 {
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&IDENTITY);
        let s = SignedIntent { nonce, order: market_order(0, true, MEUR, T_1400).to_bytes(), sig };
        assert_eq!(w.ev.on_order(&s), Err(R::BadSignature), "nonce {nonce}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// OFF-CHAIN: AcceptAll in the evaluator is caught by the verifier

#[test]
fn off_chain_an_evaluator_that_skips_the_signature_check_is_flagged_by_the_verifier() {
    let mut w = honest_world(Box::new(AcceptAll));
    // an order with a zero signature is accepted by AcceptAll and executed
    let s = SignedIntent { nonce: 1, order: market_order(0, true, MEUR, T_1400).to_bytes(), sig: [0; 64] };
    assert_eq!(w.ev.on_order(&s), Ok(()));
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    let bs = w.drain();
    let rep = verify(&w, &bs);
    assert_eq!(rep.verdict, setl8_verify::Verdict::Mismatch, "{}", describe(&rep));
    assert!(rep.findings.iter().any(|f| f.class == setl8_verify::FindingClass::BadIntentSignature), "{}", describe(&rep));
}

// ---------------------------------------------------------------------------------------------------------------------
// FINDING VRF-2 (low), seen through the evaluator's output: the PUBLIC VERIFIER says VALID for a fabricated trade when the
// account's trader key is a small-order point (crates/verifier/src/intents.rs:118 uses plain verify). The unit-level version of
// this test lives in crates/verifier/tests/adversarial_inputs.rs.

// FINDING VRF-2
#[test]
fn finding_vrf_2_verifier_calls_an_unsigned_trade_valid_for_a_small_order_trader_key() {
    let mut w = world_with(Box::new(AcceptAll), IDENTITY);
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&IDENTITY);
    let s = SignedIntent { nonce: 1, order: market_order(0, true, MEUR, T_1400).to_bytes(), sig };
    assert_eq!(w.ev.on_order(&s), Ok(()));
    w.sample(0, T_1400 + 5, p4(10_000));
    w.ev.tick(T_1400 + 6);
    let bs = w.drain();
    let rep = verify(&w, &bs);
    assert!(
        rep.findings.iter().any(|f| f.class == setl8_verify::FindingClass::BadIntentSignature),
        "the verifier accepted a zero-S signature by a small-order key: verdict {:?}\n{}",
        rep.verdict,
        describe(&rep)
    );
}

#[test]
fn control_signing_message_binds_account_nonce_and_order() {
    // the message the signature covers (what the verifier recomputes): a change of any of the three changes it
    let o = OrderIntent::from_bytes(&market_order(0, true, MEUR, T_1400).to_bytes()).unwrap().to_bytes();
    let base = signing_message(&[1; 32], 1, &o);
    assert_ne!(base, signing_message(&[2; 32], 1, &o));
    assert_ne!(base, signing_message(&[1; 32], 2, &o));
    let mut o2 = o;
    o2[47] ^= 1;
    assert_ne!(base, signing_message(&[1; 32], 1, &o2));
    assert!(base.starts_with(b"SL8LEV|"));
}
