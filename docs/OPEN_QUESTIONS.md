# setl8-lev-trading: OPEN QUESTIONS

Ranked: 1 to 6 block a build module, 7 to 15 block launch or change a rule or policy, the rest are confirmations. "Default" is what the build does if you say nothing.
Last updated: 2026-10-09 (architecture C, decisions 32 to 35). Decisions already made are in `handover/design-decisions.md`.
Removed because option C made them obsolete: keeper bounty scope, the 8-open-position cap as an on-chain constraint, two-sample confirmation on-chain.

## A. Blocks building

**1. Stork access and signed-update format (blocks M2 production adapter and the evaluator's price feed).** Stork's signed-update access is contact-sales. Do we have access for the BACKEND, what does it cost, and what exactly is the signed message (layout, signature scheme, size)?
Also unverified: how the proof bytes can be verified on-chain later (`secp256k1_recover` or a CPI), and the per-market update rate.
*Options:* (a) you confirm access and share the docs; (b) build against a mock price source and verify later; (c) Pyth fallback (only if verified free, no phone/KYC signup).
*Default:* (b). Nothing ships to mainnet until (a) or (c) is verified.

**2. Engine and evaluator language / location (blocks the engine crate API in M0 and module M10).** The rules (sections 2, 7, 9) must exist in exactly one implementation. *Options:* (a) **Rust evaluator service** using the `engine` crate directly (recommended: no port, bit-identical with the program and the verifier); (b) your backend stays TypeScript/other and calls the engine compiled to WASM, with differential fuzz tests between both; (c) a separate re-implementation (rejected: two implementations will drift on rounding).
Also: where it runs (your own server vs. a cloud host) and who operates it. *Default:* (a) for the reference harness; production choice yours.

**3. Vault change for the reset price (blocks M8 only).** Prompt is in `handover/vault-change-reset-price.md`. Needs the vault repo change, a shared-interfaces version bump and a new tag by you.
*Default:* M8 is built against a vault fixture with the new formula; the real vault must match before integration tests are meaningful.

**4. Market list and quote conversion (blocks real market data).** You supply the 82+ markets, class of each, session hours, and which are USD-quoted. Non-USD-quoted pairs and indices (EURJPY, DAX, Nikkei) need a conversion feed (3.5).
*Default:* launch with USD-quoted markets only.

**5. Spread, commission and per-market half-spread (placeholders allowed).** Your friend's numbers. Needed before any public test.
*Default:* placeholder fixtures in tests only; no production defaults.

**6. Daily payout cap value (`max_payout_per_window`) and window (blocks nothing in code, set at `init_config`).** The largest single payout is $16,000 (0.08 x $200K). Pick a number you can afford to lose in one day if the evaluator key is stolen.
*Options:* $50,000 / $100,000 / $250,000 / other. *Default for tests:* $100,000 per 86,400 s.

## B. Blocks launch or changes a rule or policy

**7. Define "coordinated hedging" (decision 36: banned, all accounts failed, wallets banned).** The program side is specified (`RuleViolation` fail, `ban_wallet`, `unban_wallet`). Still needed from you: the written definition for the terms (what counts as opposite positions: same market only, or correlated markets; how close in size and time) and the review step before the evaluator fails accounts (automatic, or you look first).
Warning (said once): detection is heuristic, so false positives will fail honest traders with no on-chain remedy except `unban_wallet`, and a new wallet evades a ban. A stolen evaluator key can now fail and ban anyone without fabricated losses.
*Default:* automatic detection flags, you confirm, then the evaluator executes.

**8. Veto aftermath versus "no manual overrides" (policy).** A veto cancels the pending pass and leaves the account as it was. It does not fail the account, refund anything, or approve anything. What do the two admins do next (re-check, ask the trader, let him press again)? Repeated vetoes can stall a trader forever.
*Options:* (a) written policy, each veto needs a public reason code in an off-chain note; (b) cap vetoes per account (after 2 the pass cannot be vetoed again without changing the code); (c) leave open. *Default:* (a), and the `vetoes` counter is public.

**9. Trader recourse when the evaluator is wrong (wrongful fail or a missed pass).** On-chain there is none. *Options:* (a) off-chain process: verifier report shows the error, you issue a free reset-code/refund from the SL8 wallet outside this program; (b) a future on-chain "restore" instruction needing both admins (it would be a manual override, against the "no manual overrides" principle); (c) none. *Default:* (a).

**10. Evaluator outage and journal posting deadline.** If the backend is down, trading stops and vault clocks may run. *Options:* (a) service-level target only (fills posted within 60 s, A-C-3); (b) on-chain `max_post_lag` after which `release_account` or a vault-clock freeze applies; (c) both. *Default:* (a).

**11. Evaluator key custody.** Hot key on a server vs. a KMS/HSM-backed signer, rotation drill, SOL balance alerts. Compromise bound is in 4.7. *Default:* KMS-backed signer if available without KYC; otherwise an encrypted key on a locked-down host plus the rotation runbook.

**12. Admin availability for the 2-day veto.** Both admins must be able to sign a veto inside 2 days, weekends included. If one admin is unreachable the safeguard does not work. *Options:* (a) accept; (b) lengthen the delay to 3 days; (c) a 2-of-3 setup (not in this design). *Default:* (a).

**13. Rule assumptions** (section 15): A-RULE-1 (tight floor is a frozen ratchet), A-RULE-2 (quick limit is strictly greater than 0.028 S, checked when the slice closes). A-RULE-3 is RESOLVED (decision 31). Say yes or change.

**14. Maximum notional per position and per account; bad-print handling.** Not in your rules. Without a cap a trader can use full leverage on a thin crypto alt. And one bad Stork print can fail an account (the engine can require a second sample before a fail; default OFF).
*Default:* per-market `max_notional_usd` set by admins at market creation; unset = leverage only; two-sample confirmation OFF.

**15. Account frozen for 2 days after a phase 2 to 5 pass (A-C-1).** The chain needs the checked state to stay fixed during the delay. The trader cannot start the next phase until `finalize_pass`. *Alternative:* let the next phase start at once and roll back on a veto (more complex, and a veto then voids work the trader already did). *Default:* freeze.

## C. Policy and confirmations

**16. Upgrade-authority burn.** Decided: 2-of-2 now. Burn (immutable program) after the audit: yes or no.
**17. Multisig tooling** for the upgrade authority and the veto. Any crypto-native on-chain multisig with no KYC; choose in BUILD_PLAN M0.
**18. 45 s timer restarts on every same-direction add.** You said later; re-confirm at review.
**19. 0.4% fee on resets as well as purchases** (A-FEE-1). Default yes.
**20. Either admin may change markets and feeds** (A-ADM-1); both for rule sets, limits, veto, evaluator rotation and upgrade. Default yes.
**21. Marketing copy.** "No overnight swap fees, no weekend holding fees" (May plan) is false under decision 22. Needs a copy change; not code. Also: payouts are now "delayed 2 days plus the vault's up-to-5-day heartbeat" and the account is frozen during the first 2 days.
**22. Holding-fee approximation** (A-FEE-2): fee uses the mark at the time it is applied. Default: approximation.
**23. Vault tally enforcement.** As of vault commit `be97396` the vault does not read `PayoutTally` yet. The sector maintains it from day one anyway. Re-check when the vault enforces it.
**24. SOL price and evaluator SOL budget.** $150 is used for rent illustrations; the evaluator pays about 0.005 SOL per 1,000 journal transactions (8.4). Replace both with real figures at launch planning.
**25. `ChallengeSize.cost` units.** I assume it is the same 6-decimal unit as `deposit_fee.amount`; the vault chat should confirm when it applies the reset change.
