# Module M8 (reset_account): requirements fixed by the vault change (P16, decision 51)

DONE: built in P18, see `handover/module-8.md` (every test item below is in `tests-rs/tests/reset.rs`).

Vault core commit 9a5c179, shared-interfaces v0.5.0. M8 is no longer blocked by the vault.

## What M8 must do
- `reset_account` computes the amount ONLY with `reset_price::reset_amount_micro(tiers, account_size, failed_phase_idx, ladder_bps)` (it calls `setl8_shared_interfaces::reset_price`; no other rounding copy anywhere).
- `tiers` = the `challenge_sizes` of the vault's `ProductRegistry`, read from the account passed in and checked as the vault-owned registry PDA of this product (the same account the CPI uses), as it is NOW. `ladder_bps` = its `reset_price_bps`. `account_size` = the failed `TradingAccount.size_s`; `failed_phase_idx` = the phase it failed in minus 1.
- The vault `DepositResetArgs` carries `amount`, `trader_wallet`, `product_program_id`, `prev_challenge_id`, `new_challenge_id`, `reset_phase`. The sector passes NEITHER `payout_count` NOR `account_size`: the vault copies both from the failed record. `reset_phase` is used by the vault only to pick the ladder entry.
- The 0.4% fee goes on top of the reset price (A-FEE-1, confirmed 2026-10-17, decision 63), `ceil(price x fee_bps / 10_000)` as in `open_account`.
- Errors: the helper's `ResetTierInvalid` (6104, no tier or a duplicate size), `ResetPhaseOutOfRange` (6105), `ResetPriceOverflow` (6106). Vault errors the CPI can return: `InvalidChallengeTier` 6006, `WrongAmount` 6007, `ResetNotAllowed` 6011 (not Failed or already reset), `InvalidResetPhase` 6012.
- The new account starts in the SAME phase with fresh state; the previous account must be locally `Failed`.

## End-to-end tests M8 must contain (through the REAL vault fixture, via our `reset_account`)
`tests-rs/tests/reset_price.rs` already proves the vault's side by calling `deposit_reset` directly with the `sector_authority` PDA flagged as a signer (possible because the harness runs with signature verification off). It cannot prove that OUR instruction builds the CPI correctly, so M8 needs:
1. All five phases at the 100K tier (859.000000 / 987.850000 / 1,030.800000 / 1,095.225000 / 1,116.700000) and at the 2.5K tier (30 / 34.5 / 36 / 38.25 / 39), each: trader pays price plus the 0.4% fee, the pool and SL8 split is the vault's, the new local account has the same phase, the vault record has `payout_count` and `account_size` of the failed one.
2. The same-phase resume: a phase-3 failure resets into phase 3, with the engine state fresh.
3. A tier repriced or removed between the fail and the reset: the reset follows the new cost, or fails with `InvalidChallengeTier` (documents the deploy rule).
4. Duplicate tier sizes in the registry: `ResetTierInvalid` before any CPI.
5. `reset_account` while the sector or the vault product is paused, by a banned wallet, on a non-Failed account, twice for one failed record.
6. The wrongful-fail policy is still open (OPEN_QUESTIONS #9): M8 must not add a free-reset path.

## What P16 changed (for the record)
- Pins: v0.4.0 to v0.5.0 in both Cargo.toml files; only `setl8-shared-interfaces` changed in both lock files.
- Fixture rebuilt from vault 9a5c179 (sha256 `42c6267008ca25cfd7b71792f3797307292049d91420dbba8fe07dc3098c89a6`; old `ed4c8c7c...` from 591ced6).
- New pure helper `programs/setl8-lev-trading/src/reset_price.rs` with 10 unit tests; 3 new errors 6104 to 6106; 9 harness tests in `tests-rs/tests/reset_price.rs`.
- Decisions taken where the prompt was silent: a ladder entry or tier cost of 0 gives price 0 and `Ok(0)` (the formula's honest result, documented); any duplicate size is refused even with equal costs (the vault accepts equal-cost duplicates, the sector is stricter on purpose); checks run in the order tier, phase, overflow.
