# Prompt for the setl8-vault chat: reset price = tier price x percent

Paste everything below the line into the vault chat.

---

Change request for `core-vault` (and a doc note for `setl8-shared-interfaces`). Do NOT change any account layout, instruction signature, discriminator or the `reset_price_bps` type. Only the meaning of the table and the price formula change.

## Why
The lev-trading product prices a phase reset as a percentage of the ORIGINAL PURCHASE PRICE of that account tier, not of account size. Purchase prices are not proportional to size ($30 for 2.5K, $859 for 100K), so one size-based table cannot express it.

Locked ladder (resume at the phase the trader failed in):
- phase 1 (index 0) = 100% of tier price
- phase 2 (index 1) = 115%
- phase 3 (index 2) = 120%
- phase 4 (index 3) = 127.5%
- phase 5 (index 4) = 130%
So `reset_price_bps = [10000, 11500, 12000, 12750, 13000]` (10000 = 100% of the tier price).

## Current behaviour
`deposit_reset` (programs/core-vault/src/instructions/sector/deposit_reset.rs) computes
`expected = prev.account_size * reset_price_bps[reset_phase] / 10_000` and requires `amount == expected`.

## Required behaviour
1. In `deposit_reset`, look up the tier in `registry.challenge_sizes` whose `size == prev.account_size` (same lookup `deposit_fee` uses to validate `(account_size, amount)`). If none: `VaultError::InvalidChallengeTier`.
2. `expected = ceil(tier.cost * bps / 10_000)` using u128 math with checked ops. Round UP (in the protocol's favour). `amount` must equal `expected` exactly (keep `WrongAmount`).
3. `tier.cost` is in the same unit as `amount` in `deposit_fee`; do not rescale.
4. Update the doc comments on `ProductRegistry.reset_price_bps`, `RegisterProductArgs.reset_price_bps`, `UpdateProductConfigArgs.reset_price_bps` and `DepositResetArgs.amount` in shared-interfaces ("basis points of the tier price, 10000 = 100%, not of account size") and add a CHANGELOG entry. Because the meaning changes, bump the version (your call whether 0.3.3 or 0.4.0) and say which.
5. Keep `MAX_RESET_PHASES = 8`; keep all other checks (Failed only, once per record, payout_count copied, pause handling).
6. Edge: if the tier set is changed by `update_product_config` after a trader bought, a record whose `account_size` is no longer a tier must fail with `InvalidChallengeTier`, never fall back to a size-based price. Tell me if you think this should be handled differently.

## Tests (LiteSVM, same style as the existing ones)
- 100K tier with cost 859: phase indices 0..4 must require 859.000000, 987.850000, 1030.800000, 1095.225000, 1116.700000 (in the cost unit; show the exact base-unit integers you use).
- A tier with a cost that makes `cost * bps` not divisible by 10_000: confirm round-up.
- amount one unit too low and one unit too high: `WrongAmount`.
- `prev.account_size` not in `challenge_sizes`: `InvalidChallengeTier`.
- `reset_phase` beyond the table: `InvalidResetPhase`.
- Mutation-check the rounding direction (flip ceil to floor; a test must fail).

## Report back
Files changed, new test names, the version you chose, and anything in my spec that looks unsafe. Do not push or tag.
