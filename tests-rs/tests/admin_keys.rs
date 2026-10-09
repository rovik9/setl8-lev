mod common;

use common::*;
use setl8_lev_trading::constants as c;
use solana_signer::Signer;

/// The localnet constants are the public keys of seeds [0x11;32] and [0x22;32].
#[test]
fn test_admin_constants_equal_the_keys_derived_from_the_fixed_seeds() {
    assert_eq!(sl8_test_admin().pubkey(), c::SL8_ADMIN);
    assert_eq!(rov_test_admin().pubkey(), c::ROV_ADMIN);
    assert_eq!(SL8_TEST_SEED, [0x11; 32]);
    assert_eq!(ROV_TEST_SEED, [0x22; 32]);
    assert_ne!(c::SL8_ADMIN, c::ROV_ADMIN);
}
