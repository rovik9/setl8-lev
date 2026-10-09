//! The two admin keys behind the 2-of-2 that guards every admin instruction.
//! Same approach as the vault's `constants/admin.rs`; `localnet` picks the keys:
//!
//! | build                    | keys                                                       |
//! |--------------------------|------------------------------------------------------------|
//! | default (`anchor build`) | the REAL founder-held keys (identical to the vault's)      |
//! | `--features localnet`    | PUBLIC TEST keys derived from fixed seed bytes (see below) |
//!
//! `localnet` must never be in `default`: a deployable build must not carry keys anyone can sign with.
//! Test key seeds (secret halves are public by design): SL8 = `[0x11; 32]`, ROV = `[0x22; 32]`;
//! tests-rs re-derives them with `Keypair::new_from_array` and compares with these constants.
use anchor_lang::prelude::*;

/// SL8's half of the 2-of-2. REAL key (same as the vault's `SL8_ADMIN_PUBKEY`).
#[cfg(not(feature = "localnet"))]
pub const SL8_ADMIN: Pubkey = pubkey!("SL89fcsKAuWYtkEJah86WLLBxCiHd1DeNczpsDjSHDJ");
/// SL8's half of the 2-of-2. PUBLIC TEST key, seed `[0x11; 32]`.
#[cfg(feature = "localnet")]
pub const SL8_ADMIN: Pubkey = pubkey!("F25s3DdjXdCxYBhh2z8FBusVEMT4b9bGNFVKJi3wFoF4");

/// Rov's half of the 2-of-2. REAL key (same as the vault's `ROV_ADMIN_PUBKEY`).
#[cfg(not(feature = "localnet"))]
pub const ROV_ADMIN: Pubkey = pubkey!("RovSQZxURnNgNkf7WkzRJTnhSBuN6tdfG2CQxp3rzKZ");
/// Rov's half of the 2-of-2. PUBLIC TEST key, seed `[0x22; 32]`.
#[cfg(feature = "localnet")]
pub const ROV_ADMIN: Pubkey = pubkey!("Bow1CGKGDB9mNxeWdw85E2aCthQ1oZX4oFEe7fYT17ew");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_admin_keys_differ() {
        assert_ne!(SL8_ADMIN, ROV_ADMIN);
    }

    #[cfg(not(feature = "localnet"))]
    #[test]
    fn default_build_has_the_real_admin_keys() {
        assert_eq!(SL8_ADMIN.to_string(), "SL89fcsKAuWYtkEJah86WLLBxCiHd1DeNczpsDjSHDJ");
        assert_eq!(ROV_ADMIN.to_string(), "RovSQZxURnNgNkf7WkzRJTnhSBuN6tdfG2CQxp3rzKZ");
    }

    #[cfg(feature = "localnet")]
    #[test]
    fn localnet_build_has_the_test_admin_keys() {
        assert_eq!(SL8_ADMIN.to_string(), "F25s3DdjXdCxYBhh2z8FBusVEMT4b9bGNFVKJi3wFoF4");
        assert_eq!(ROV_ADMIN.to_string(), "Bow1CGKGDB9mNxeWdw85E2aCthQ1oZX4oFEe7fYT17ew");
    }
}
