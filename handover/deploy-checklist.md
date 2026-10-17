# Deploy checklist (consolidated)

Values and checks that live outside the code; none is set by this repo. The per-module lists (`module-7.md` "Deploy checklist" and the others) stay as the history of where each item came from. Last updated 2026-10-13 (decision 51).

## Vault and shared-interfaces
- Vault: core commit **9a5c179 or later** (the reset price is a percentage of the TIER price from this commit on). Older vaults price a reset off the account size.
- `setl8-shared-interfaces`: tag **v0.5.0** (tag commit `3b0580c47979387e03f11c280d93a5870c66cd3e`) in `programs/setl8-lev-trading/Cargo.toml` and `tests-rs/Cargo.toml`, both lock files resolved to that commit.
- Test fixture `tests-rs/fixtures/core_vault.so` (git-ignored): the vault's localnet build (`scripts/build-test-so.sh` in a clean clone at 9a5c179), sha256 `42c6267008ca25cfd7b71792f3797307292049d91420dbba8fe07dc3098c89a6`, asserted by `VAULT_FIXTURE_SHA256` in `tests-rs/tests/common/vault.rs`.

## `register_product` / `update_product_config` (both vault admins)
- `product_program_id` = the real program id (placeholder `9NAdnJ7ySQqPYuPKu6d6wCbcaHHMnzwFT6xEcRfXX2a7` replaced, keypair generated locally, never printed or committed).
- `reset_price_bps = [10000, 11500, 12000, 12750, 13000]`: basis points of the CURRENT tier price, indexed by the 0-based phase the trader failed in (type `Vec<u16>`, values above 10000 are intended). Never a size-based table such as `[100, 150]`.
- `challenge_sizes` = the rule set's ten tiers (size, cost) exactly, with **unique sizes**. Two tiers of one size with different costs make the vault answer `InvalidChallengeTier` forever for that size; the sector helper refuses any duplicate size.
- **Never reprice or remove a tier while Failed records exist.** A reset follows the tier cost registered now, not what the trader paid: repricing changes, and removing blocks, every pending reset of that size. (Restated for M8: `reset_account` reads the registry at the moment of the call. A removed tier is refused by the sector with `ResetTierInvalid` before any CPI, a duplicate size with the same error, a ladder shorter than the failed phase with `ResetPhaseOutOfRange`; a repriced tier is simply followed, the trader pays the new price. The ladder must have FIVE entries or the phases beyond it cannot be reset.)
- `reset_account` needs about 125,000 to 157,000 compute units (the vault's `deposit_reset` is about 42,000 of it; the spread comes from the PDA bump searches of the trader's keys). It fits the default 200,000-unit limit; clients should still ask for 250,000.
- `max_payout_count` = **4** (one payout per phase 2 to 5; the local graduation assumes it).
- `fee_split_bps` (the harness uses 6500; the real number is Rovik's); mints = the vault's USDC and USDT.

## Program and config
- Deploy only the DEFAULT build (real admin keys); never `--features localnet`. `scripts/anchor-build-checked.sh` clean, record the sha256.
- `constants/admin.rs` real SL8 and Rov keys (equal to the vault's); upgrade authority 2-of-2 (decision 30).
- `init_config` (both admins): `evaluator`, `vault_program` = real vault id, `sl8_wallet` (needs a token account of each accepted mint for the 0.4% fee), `pass_delay_secs` 172,800, `window_secs` 86,400, `max_payout_per_window` = 100,000,000,000 micro-USD (decision 37).
- Rule set 1 and markets: see `module-7.md` ("Rule set 1", "Markets"); PENDING numbers `commission_e6` and `spread_e6`.
- Evaluator operations, rent and fees, open policy items: see `module-7.md`.

## Still open (not decided by this change)
- The 0.4% fee on top of resets (A-FEE-1; `reset_account` charges it with the same `ruleset.fee_bps` as `open_account`, one constant to change if it is dropped); the reset price after a wrongful fail (OPEN_QUESTIONS #9); what admins do after a veto (#8); hedging thresholds (#7).
