# setl8-lev-trading: OPEN QUESTIONS

Ranked: 1 to 5 block a build module, 6 to 10 block launch or change a rule, the rest are confirmations. "Default" is what the build does if you say nothing.
Last updated: 2026-10-09. Decisions already made are in `handover/design-decisions.md`.

## A. Blocks building

**1. Stork access (blocks M2 and the keeper).** Stork's signed-update access is contact-sales. Do we have it, what does it cost, is there a per-update fee on Solana, and can Setl8's keeper and any outside keeper
get the signed stream? Also unverified: the account byte layout, how many updates fit in one transaction, and whether Stork's program rejects an update older than the stored one.
*Options:* (a) you confirm access and share the docs; (b) build against a mock adapter and verify later; (c) Pyth fallback (only if verified free for non-index markets, and without phone/KYC signup).
*Default:* (b). Nothing ships to mainnet until (a) or (c) is verified.

**2. Cross-account hedge farming (blocks launch, may change rules).** Two wallets trade opposite sides; one passes a phase and collects $8,000 on $100K, the other resets for $859 to $1,117. Roughly 7x to 9x return per pass. The 5-accounts-per-wallet cap does not help; there is no KYC.
*Options (can be combined):* (a) minimum trading days per phase (for example 5 distinct UTC days); (b) consistency rule (no single day or trade above X% of the phase profit); (c) minimum risk per trade (stop-loss distance at least Y%); (d) payout delay beyond the 5-day heartbeat; (e) accept the risk and cap total payouts per day per product at the vault.
*Default:* none; the design documents the hole (A-6). Needs your decision before launch.

**3. Vault change for the reset price (blocks M8 only).** Prompt is in `handover/vault-change-reset-price.md`. Needs the vault repo change, a shared-interfaces version bump and a new tag by you.
*Default:* M8 is built against a vault fixture with the new formula; the real vault must match before integration tests are meaningful.

**4. Market list and quote conversion (blocks real market data).** You supply the 82+ markets, class of each, session hours, and which are USD-quoted. Non-USD-quoted pairs and indices (EURJPY, DAX, Nikkei) need a conversion feed (3.5).
*Default:* launch with USD-quoted markets only; others added when you provide the conversion feed.

**5. Spread, commission and per-market half-spread (placeholders allowed).** Your friend's numbers. Needed before any public test.
*Default:* placeholder fixtures in tests only; no production defaults.

## B. Blocks launch or changes a rule

**6. Rule assumptions** (section 15): A-RULE-1 (tight floor is a frozen ratchet), A-RULE-2 (quick limit is strictly greater than 0.028 S, checked when the slice closes), A-RULE-3 (the tight-mode start sample is not checked against the new floor; a trader within 3% of the static floor cannot hold more than about 1% of open profit). Say yes or change.

**7. Bounty scope.** Decision 21 says paid keepers. I limited the bounty to floor settlements (static fail, tight breach, phase fail) so bounties cannot be farmed (A-4, A-5). SL, TP and pending fills are served unpaid by Setl8's bot. Confirm or ask for more.

**8. Cap of 8 open positions per account.** Needed because each open market adds a feed to the transaction. Raising it costs more transactions per snapshot. Default 8.

**9. Two-sample confirmation for FAIL and breach.** Protects against a single bad print failing an account (R-4) but delays settlement by about 2 s and gives the trader one more sample. Default OFF.

**10. Maximum notional per position and per account.** Not in your rules. Without a cap a trader can open the full leverage on a thin crypto alt (notional 2x equity). Default: per-market `max_notional_usd` set by admins at market creation; unset = leverage only.

## C. Policy and confirmations

**11. Upgrade-authority burn.** Decided: 2-of-2 now. Burn (immutable program) after the audit: yes or no.
**12. Multisig tooling** for the upgrade authority. Any crypto-native on-chain multisig with no KYC; choose in BUILD_PLAN M0.
**13. 45 s timer restarts on every same-direction add.** You said later; re-confirm at review.
**14. 0.4% fee on resets as well as purchases** (A-FEE-1). Default yes.
**15. Either admin may change markets and feeds** (A-ADM-1); both for rule sets, bounty and upgrade. Default yes.
**16. Marketing copy.** "No overnight swap fees, no weekend holding fees" (May plan) is false under decision 22. Needs a copy change; not code.
**17. Holding-fee approximation** (A-FEE-2): fee uses the mark at the time it is applied. Alternative: store the 22:00 mark per market (more accounts, more compute). Default: approximation.
**18. Vault tally enforcement.** As of vault commit `be97396` the vault does not read `PayoutTally` yet. The sector maintains it from day one anyway. Re-check when the vault enforces it.
**19. SOL price assumption** ($150) used for rent illustrations; replace with the real figure at launch planning.
**20. `ChallengeSize.cost` units.** I assume it is the same 6-decimal unit as `deposit_fee.amount`; the vault chat should confirm when it applies the reset change.
