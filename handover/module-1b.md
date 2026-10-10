# Module M1b: rule sets, markets, wallets (DONE, verified 2026-10-10)

Toolchain as in module-0.md / module-1a.md (anchor-lang resolves to 0.32.2, `litesvm =0.7.0`). `anchor-lang` now has the `init-if-needed` feature (used only by `ban_wallet`).

## Built
- Item 0 (M1a review fix): `init_config` rejects `vault_program == default` and `sl8_wallet == default` with `Unauthorized`. No setters added.
- `RuleSet` (`state/rule_set.rs`): 552 bytes, DESIGN 5.4 field order, PDA `["ruleset", version u16 LE]`, const assert + offset test (version 8, fee_bps 10, tier_size[0] 94, reset_ladder_bps[0] 254, spread_e6[0] 264, spread_e6[127] 518, bump 520).
- `Market` (`state/market.rs`): 128 bytes, layout from the M1b prompt, PDA `["market", id u16 LE]`, const assert + offset test.
- `WalletProfile` (`state/wallet_profile.rs`): 56 bytes, PDA `["wallet", trader]`, const assert + offset test.
- `create_ruleset` (both admins), `create_market` / `update_market` (either admin), `ban_wallet` / `unban_wallet` (both admins). `instructions/market_params.rs` holds the validation shared by create and update. `admin_action.rs` now also exposes `require_both_admins` / `require_either_admin` (free functions; the `AdminAction` methods call them) for instructions whose accounts struct has extra accounts.
- Events: `RuleSetCreated`, `MarketCreated`, `MarketUpdated`, `MarketFeedChanged{market_id, old_feed, new_feed, slot}`, `WalletBanned`, `WalletUnbanned` (plain `emit!`).
- tests-rs harness extended in place (`tests/common/mod.rs`): `Admins` (who is named / who signs), instruction builders, `launch_ruleset_args()`, `market_params()`, `events_of::<T>(logs)` (decodes Anchor events from "Program data:" log lines).

## Test counts
- Workspace (`scripts/test-engine.sh`): engine 4, journal-types 1, program 9 (adds three layout tests) = 14.
- tests-rs: 72 (admin 13, admin_keys 1, error_codes 1, init_config 16, market 16, ruleset 16, smoke 1, wallet 8).
- Mutation checks (temporary, reverted): `create_ruleset` `version == next` -> `>=`: killed by 2 tests (`create_ruleset_rejects_version_two_before_one_exists`, `version_two_can_follow_one_and_version_one_is_untouched`). `create_market` `market_id == market_count` -> `>=`: killed by 1 test (`create_market_rejects_skipped_ids`).

## Decisions (design silent)
Error-code choices (no new variants, codes fixed by DESIGN 12):
1. Rule set tier / ladder / leverage / fee failures AND a wrong `version` (not `current + 1`): `InvalidTier`. Version increment overflow: `MathOverflow`. Non-admin: `Unauthorized`.
2. `create_market`: wrong id (not `market_count`), id >= 128: `MarketUnknown`. Counter overflow: `MathOverflow`. Non-admin: `Unauthorized`.
3. ALL other market-parameter failures (class, status, zero feed, max_age, quantity fields, notional, sessions, quote conversion) in `create_market` and `update_market`: `MarketUnknown` (DESIGN 12 has no better code).
4. `ban_wallet`: zero evidence `EvidenceMissing`. `unban_wallet` without a profile: Anchor's `AccountNotInitialized` (3012). `update_market` for a market that does not exist: Anchor account error (3012), not a program error.
5. `version 1 twice`: fails in account creation (system program "already in use") before the handler runs; stored bytes unchanged (tested).
Other:
6. `quote_conv_market` must be `0xFFFF` or STRICTLY EARLIER than the market's own id (`< own_id`). At creation this is exactly "an existing market, not itself"; on update it keeps the conversion graph acyclic (a market cannot be pointed at a later market, even an existing one). Found by a failing test of mine that assumed otherwise.
7. `create_ruleset`, `create_market`, `ban_wallet` take a separate `payer` signer (as `init_config`); the trader never pays for a ban.
8. `update_market` always emits `MarketUpdated`, even if nothing changed (an audit trail of admin actions); `MarketFeedChanged` only when the feed id differs.
9. `ban_wallet` uses `init_if_needed` (feature `init-if-needed`); a profile is treated as new when `trader == default`; existing counters are never reset (tested).
10. `create_ruleset` validates ALL 8 leverage entries (>= 1), including classes unused so far; unused tier/ladder entries must be zero; commission, spread and holding fee are unchecked (PENDING numbers).

## Open hooks
- `ban_wallet`: DESIGN 6.1 says "for a linked wallet with no held account". That check needs `TradingAccount` (M5/M7) and is NOT built; today an admin pair can ban any wallet.
- `WalletProfile` is created only by `ban_wallet` so far; `open_account` (M5) must create it for new traders with the same PDA and treat an existing banned profile as `WalletBanned`.
- `update_market` has no delay or lock by decision 24; nothing in M1b limits how often or how far a feed can be changed (events are the only trail).
- `Market.status` is stored but not interpreted on chain yet (used from M5).
- Rule set spread array is indexed by market id (128); `create_market` does not check that the rule set has a spread entry (it always does: the array is fixed at 128).
