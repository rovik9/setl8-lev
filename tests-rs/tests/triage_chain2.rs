//! Triage of the M12 review claims V12 and V20 (program side). Tests marked `#[ignore = "FINDING ..."]` assert the CORRECT
//! behaviour and fail today; run them with `-- --ignored`.
mod common;

use common::journal::*;
use common::*;
use setl8_lev_trading::{constants as c, events::MarketFeedChanged, instructions::UpdateMarketArgs};
use solana_signer::Signer;

fn hash(n: u8) -> [u8; 32] {
    [n; 32]
}

// ======================================================================================== V12

// FINDING V12
/// A held, idle account: the vault's permissionless `mark_abandoned` flips the vault record to Abandoned after 7 days while the
/// local account is still Active + held. Both admins then call `fail_for_violation`: the `flag_trader_failed` CPI is refused by
/// the vault (InvalidTraderStatus 6005) and the whole instruction reverts, so neither the violation marker nor the ban is applied.
/// The correct behaviour: the admins' decision (marker + ban) is recorded whatever the vault already knows.
#[test]
#[ignore = "FINDING V12: fail_for_violation reverts when the vault record is already Abandoned, so the violation marker and ban cannot be applied"]
fn fail_for_violation_still_records_the_violation_when_the_vault_already_abandoned_the_challenge() {
    let mut w = JEnv::new();
    let t = w.t0 + 1_000;
    w.ok(|w| w.hold(t, hash(1)));
    let limit = w.t0 + 604_800;
    assert_ok(w.vault_mark_abandoned(limit + 1));
    assert_eq!(w.vault_status().status, 3, "vault: Abandoned");
    assert_eq!((w.account().status, w.account().held), (c::STATUS_ACTIVE, 1), "local: still Active and held");
    let r = w.violation(limit + 2, hash(2));
    assert!(r.is_ok(), "the violation fail reverted: {:?}", r.err().map(|f| (f.err, f.meta.logs.last().cloned())));
    let a = w.account();
    assert!(a.violation_failed());
    assert_eq!(w.wallet_profile().banned, 1);
}

// ======================================================================================== V20

/// Whether the transaction carried an `emit_cpi!` event (an inner instruction with the Anchor event tag).
fn has_cpi_event(meta: &litesvm::types::TransactionMetadata) -> bool {
    meta.inner_instructions.iter().flatten().any(|i| i.instruction.data.len() > 8 && i.instruction.data[..8] == *anchor_lang::event::EVENT_IX_TAG_LE)
}

// FINDING V20 / F-27 (FIXED, decision 67: the admin events are emit_cpi! events)
/// DESIGN 8.1: every lifecycle event is emitted "with `emit_cpi!`" because program logs truncate at 10 KB per transaction. The
/// admin-signed `MarketFeedChanged` (the only guard on the single-admin feed remap, R-5) is a plain `emit!` log line: a rogue admin
/// can put a log-heavy instruction before it in the same transaction and a log-based indexer never sees the remap.
#[test]
fn market_feed_changed_is_emitted_as_a_cpi_event_not_only_as_a_log_line() {
    let mut e = Env::initialised();
    e.create_market_ok(&create_market_args(0, 0, 0xA0));
    let admins = Admins::both(&e);
    let ix = update_market_ix(admins, &UpdateMarketArgs { market_id: 0, params: market_params(0xC7) });
    let meta = e.send_admins(ix, admins).expect("update succeeds");
    assert_eq!(events_of::<MarketFeedChanged>(&meta.logs).len() + usize::from(has_cpi_event(&meta)), 1, "one event either way");
    assert!(has_cpi_event(&meta), "MarketFeedChanged is only a 'Program data:' log line");
}

// FINDING V20 / F-27 (FIXED, decision 67: the admin events are emit_cpi! events)
#[test]
fn wallet_banned_is_emitted_as_a_cpi_event_not_only_as_a_log_line() {
    let mut e = Env::initialised();
    let admins = Admins::both(&e);
    let trader = solana_pubkey::Pubkey::new_unique();
    let ix = ban_wallet_ix(e.payer.pubkey(), admins, trader, [0xE5; 32]);
    let meta = e.send_admins(ix, admins).expect("ban succeeds");
    assert!(has_cpi_event(&meta), "WalletBanned is only a 'Program data:' log line");
}
