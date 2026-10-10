You are building module M1b of setl8-lev-trading in THIS repo (branch main). M1a is done (commit 1f6ea2a). Read first, in this order: handover/module-1a.md; docs/DESIGN.md sections 3 (3.1, 3.2, 3.4, 3.5), 5 (5.2 WalletProfile, 5.4 RuleSet, table in 5), 6.1 (admin table), 12, 14; docs/BUILD_PLAN.md section 0 and the M1b row; handover/design-decisions.md (decisions 13, 15, 17, 24, 26, 28, 36). Read existing code in programs/setl8-lev-trading/src (admin_action.rs has both_admins / either_admin; reuse them) and tests-rs/tests/common/mod.rs (reuse the harness; extend it, do not fork it).

RULES
- Same rules as P2: work only inside this repo; never touch ../setl8-vault or ../setl8-shared-interfaces; NEVER read, print, open or commit private keys, .env files or keypair JSONs; no push, tag or git-config change; commit only with explicit paths after scripts/secret-scan-staged.sh; author env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com; commit message ends with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Build exactly what is listed. If the design looks wrong or contradictory, STOP and report. Where silent, take the smallest safe option and list it under DECISIONS.
- All arithmetic checked; no unwrap/expect in program code; no floats; no truncating `as` casts.

SCOPE (nothing else)
0. FIX from the M1a review: in `init_config`, reject `vault_program == Pubkey::default()` and `sl8_wallet == Pubkey::default()` (error `Unauthorized`, same as the evaluator check). Add two tests. Do not add setters.
1. `RuleSet` account exactly as DESIGN 5.4: 552 bytes including discriminator, fields in the listed order, PDA `["ruleset", version u16 LE]`, Borsh, const-asserted size (8 + payload == 552) plus a byte-layout test (serialize a known value, compare offsets of version, fee_bps, tier_size[0], reset_ladder_bps[0], spread_e6[127], bump).
2. `create_ruleset(args)`: both admins. `version` MUST equal `config.current_ruleset + 1` (first call creates version 1). Validates: tier_count 1..=10 and ladder_len 1..=5 (arrays beyond the counts must be zero); tier_size and tier_cost strictly increasing over the first tier_count entries and non-zero; reset_ladder_bps non-zero and non-decreasing over the first ladder_len entries; fee_bps <= 1000; every leverage_x >= 1; spread_e6 / commission_e6 / holding_fee_e6 any value (commission and spread are PENDING numbers; zeros allowed). Sets `config.current_ruleset = version`. Immutable afterwards: there is NO update instruction, and a test proves a second create with the same version fails and the stored bytes of version 1 are unchanged after creating version 2. Errors: use `InvalidTier` for tier/ladder/leverage/fee validation failures, `RuleSetMissing` is not used here, `Unauthorized` for admin failure, `MathOverflow` for the version increment. Emit `RuleSetCreated{version}`.
3. `Market` account, 128 bytes including discriminator, PDA `["market", market_id u16 LE]`. Layout (this is the design for the 128 bytes, DESIGN gave the fields but not the order): `market_id u16, class u8, status u8 (0 disabled, 1 enabled, 2 reduce-only), feed_id [u8;32], max_age_secs u16, qty_step u64, qty_min u64, max_notional_usd u64 (micro-USD), session_count u8, sessions [(u16 start_min, u16 end_min); 6], quote_conv_market u16 (0xFFFF = none), bump u8, reserved [u8;30]`. 8 + 2+1+1+32+2+8+8+8+1+24+2+1+30 = 128. Const-assert it and write a byte-layout test.
4. `create_market(args)`: EITHER admin. `market_id` MUST equal `config.market_count` (sequential, no gaps) and < 128 (id or bounds failure: `MarketUnknown`; counter overflow: `MathOverflow`; signer failure: `Unauthorized`; do not add error variants; list in DECISIONS). Validates: class 0..=7, status 0..=2, feed_id not all zero, max_age_secs > 0, qty_step > 0, qty_min >= qty_step, max_notional_usd > 0, session_count <= 6 and every session start < end <= 10080 (minutes in a week), unused session slots zero, quote_conv_market is 0xFFFF or < market_count (an earlier market) and not itself. Increments `config.market_count`.
5. `update_market(args)`: EITHER admin. Can change status, feed_id, max_age_secs, qty_step, qty_min, max_notional_usd, sessions, quote_conv_market; CANNOT change market_id or class (class is fixed at creation). Same validations. If `feed_id` changes emit `MarketFeedChanged{market_id, old_feed, new_feed, slot}` (slot from Clock). Any change emits `MarketUpdated{market_id}`. No delay, no lock (decision 24 exception, documented in the code comment).
6. `WalletProfile`: 56 bytes including discriminator, PDA `["wallet", trader]`, fields per DESIGN 5.2: `trader Pubkey, next_challenge_id u64, active_count u8, bump u8, banned u8, pad [u8;5]`. 8+32+8+1+1+1+5 = 56. Const-assert plus layout test. (Creation by `open_account` comes in M5; here it is only created by `ban_wallet`.)
7. `ban_wallet(evidence_hash [u8;32])` with a `trader` pubkey argument: both admins; `evidence_hash` non-zero else `EvidenceMissing`; creates the profile if missing (admin pays), sets `banned = 1`; idempotent if already banned; emits `WalletBanned{trader, evidence_hash}`. NOTE for the report: the DESIGN says "no held account" - that check needs TradingAccount (M5/M7) and is NOT built here; state this in module-1b.md as an open hook.
8. `unban_wallet(trader)`: both admins; the profile must exist (else Anchor's own account-not-initialized error; list in DECISIONS); sets `banned = 0`; emits `WalletUnbanned{trader}`.
9. Events (plain `emit!`, same as M1a): RuleSetCreated, MarketCreated{market_id, class, feed_id}, MarketUpdated, MarketFeedChanged, WalletBanned, WalletUnbanned.
10. Launch rule set helper: in tests-rs/tests/common add `launch_ruleset_args()` returning the DESIGN 5.4 launch values: leverage_x = 50,20,20,20,10,5,5,2; holding_fee_e6 = 100,200,400,200,400,400,1000,1600; tier_size micro-USD $2.5K,5K,10K,15K,20K,25K,50K,75K,100K,200K; tier_cost $30,55,99,145,189,235,489,689,859,1599; reset_ladder_bps 10000,11500,12000,12750,13000; fee_bps 40; tier_count 10; ladder_len 5; commission and spread zeros.

TESTS (tests-rs, LiteSVM, real signatures; each item is a test)
- create_ruleset succeeds with the launch values; account length 552; every field read back equals the args; config.current_ruleset == 1.
- create_ruleset fails: one admin only; non-admins; version 0; version 2 before 1 exists; version 1 twice; tier_count 0 and 11; non-increasing tier_size; non-increasing tier_cost; zero tier_size; ladder_len 0 and 6; decreasing reset ladder; non-zero array entry beyond tier_count; fee_bps 1001; leverage 0.
- version 2 can be created after 1; version 1 bytes unchanged; current_ruleset == 2.
- create_market: id 0 succeeds then id 1; skipping to id 5 fails; id 128 fails; every validation failure listed in item 4 (each its own assertion); one admin alone (either) succeeds; no signer fails; non-admin fails; market_count increments; account length 128.
- update_market: changes persist; class cannot change (instruction has no class field - assert by data equality); feed change emits MarketFeedChanged (decode the event from the logs, with old and new feed) and a non-feed change does not; either admin can update; non-admin cannot; validations re-applied.
- WalletProfile: layout test for the 56 bytes (via ban_wallet's account).
- ban_wallet: both admins required; zero evidence fails with EvidenceMissing; creates the profile; idempotent; banned == 1; one admin fails. unban_wallet: both admins required; clears banned; fails on a wallet with no profile.
- The two init_config checks from item 0.
- Layout/offset tests for RuleSet and Market as described.
- A mutation check: temporarily change `create_ruleset`'s `version == current + 1` to `>=`, confirm tests fail, revert. Then temporarily change `create_market` id check the same way. Report both.

DEFINITION OF DONE
`scripts/test-engine.sh`, `scripts/anchor-build-checked.sh` (default and localnet clean) and `cd tests-rs && cargo test` pass. Write handover/module-1b.md (built, test counts, decisions, open hooks). Commit with explicit paths; do NOT push.

REPORT BACK (paste this block, nothing longer):
- test counts: workspace, tests-rs (list new test names, grouped)
- both build scripts' final lines and the two .so sha256 values
- both mutation results
- every file added/changed, one line each
- DECISIONS where the design was silent (including each error-code choice)
- anything you refused or stopped on
- `git log --oneline -3` and `git status --short`
