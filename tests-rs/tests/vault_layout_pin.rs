//! Audit F-33 (decision 67): the program reads two vault accounts by BYTE OFFSET (`vault_view.rs`: the product registry and the
//! trader-state record). If the vault ever changes its layout, the program would read garbage. These tests pin (1) the exact
//! vault fixture by sha256 and (2) every offset and discriminator `vault_view.rs` relies on, against real accounts made by the
//! real vault. A different vault build must fail HERE first, not in production.
mod common;

use common::vault::*;
use common::*;
use setl8_lev_trading::vault_view as vv;
use sha2::{Digest, Sha256};
use solana_signer::Signer;

#[test]
fn the_vault_fixture_is_the_pinned_build() {
    let bytes = std::fs::read(fixture_path()).expect("vault fixture");
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(got, VAULT_FIXTURE_SHA256);
    assert_eq!(VAULT_FIXTURE_SHA256, "42c6267008ca25cfd7b71792f3797307292049d91420dbba8fe07dc3098c89a6");
}

#[test]
fn the_offsets_in_vault_view_match_the_real_vault_trader_state() {
    assert_eq!(vv::VAULT_ACCOUNT_SIZE_OFFSET, 80);
    assert_eq!(vv::VAULT_PAYOUT_COUNT_OFFSET, 88);
    let mut v = VaultEnv::new();
    let t = v.new_trader();
    let tk = t.pubkey();
    assert_ok(v.open(&t, 0));
    let size = v.ruleset.tier_size[0];
    let raw = v.e.svm.get_account(&vault_trader_state_pda(&tk, 0)).expect("vault record");
    assert_eq!(raw.owner, VAULT_ID);
    let d = &raw.data;
    // discriminator, wallet (8..40), product program (40..72), challenge id (72..80), then the two read-by-offset fields
    assert_eq!(d[..8], vv::TRADER_STATE_DISCRIMINATOR);
    assert_eq!(d[8..40], tk.to_bytes());
    assert_eq!(d[40..72], PROGRAM_ID.to_bytes());
    assert_eq!(u64::from_le_bytes(d[72..80].try_into().unwrap()), 0);
    let at = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
    assert_eq!(at(vv::VAULT_ACCOUNT_SIZE_OFFSET), size, "account_size is at the pinned offset");
    assert_eq!(at(vv::VAULT_PAYOUT_COUNT_OFFSET), 0, "payout_count is at the pinned offset");
    assert_eq!(d[96], vv::VAULT_STATUS_ACTIVE, "status byte follows payout_count");
    // the decoder agrees with the bytes
    let view = vv::parse_trader_state(d, &tk, &PROGRAM_ID, 0).expect("decodes");
    assert_eq!((view.account_size, view.payout_count, view.status), (size, 0, vv::VAULT_STATUS_ACTIVE));
    // a change at the pinned payout_count offset is what the decoder sees (and nothing else moves)
    v.edit_vault_record(&tk, 0, None, Some(3));
    let raw = v.e.svm.get_account(&vault_trader_state_pda(&tk, 0)).unwrap();
    let view = vv::parse_trader_state(&raw.data, &tk, &PROGRAM_ID, 0).unwrap();
    assert_eq!((view.account_size, view.payout_count), (size, 3));
}

#[test]
fn the_decoder_reads_the_real_vault_product_registry() {
    let mut v = VaultEnv::new();
    let raw = v.e.svm.get_account(&registry_pda()).expect("registry");
    assert_eq!(raw.owner, VAULT_ID);
    assert_eq!(raw.data[..8], vv::PRODUCT_REGISTRY_DISCRIMINATOR);
    let view = vv::parse_registry(&raw.data, &PROGRAM_ID).expect("decodes");
    let reg = v.registry();
    assert_eq!(view.tiers, reg.challenge_sizes);
    assert_eq!(view.ladder, reg.reset_price_bps);
    assert_eq!(view.ladder, RESET_LADDER_BPS.to_vec());
    assert_eq!(view.tiers.len(), 10);
    // the decoder also follows an edit of the registry (tiers and ladder are read from the real offsets, not cached)
    let mut tiers = reg.challenge_sizes.clone();
    tiers[0].1 += 1;
    assert_ok(v.vault_set_tiers(tiers.clone()));
    let raw = v.e.svm.get_account(&registry_pda()).unwrap();
    assert_eq!(vv::parse_registry(&raw.data, &PROGRAM_ID).unwrap().tiers, tiers);
    // a registry of another product is refused
    assert!(vv::parse_registry(&raw.data, &anchor_lang::prelude::Pubkey::new_unique()).is_err());
}
