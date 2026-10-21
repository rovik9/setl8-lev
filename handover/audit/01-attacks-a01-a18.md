# M12 item 1: DESIGN 13.2 attacks A-1 to A-18 as tests (A-1 to A-5 by an earlier run, A-6 to A-18 and the class table by this run)

SELF-audit by the builder, not an independent audit. Finding prefix: ATK. No production code was touched.

## Files and how to run

| file | what | status |
|---|---|---|
| `tests-rs/tests/adversarial.rs` | one module per attack `a01_...` to `a18_...`, plus `attack_table_is_complete`. A-1 to A-5 are the earlier run's and are unchanged. A-6 to A-18 and the table are new | 70 tests, 70 pass, 0 ignored |
| `crates/evaluator/tests/adversarial_attacks.rs` (new) | A-9 and A-14 on the evaluator side | 8 tests, 8 pass |
| `crates/verifier/tests/adversarial_attacks.rs` (new) | A-1, A-9, A-14, A-18 on the verifier side | 8 tests, 7 pass, 1 `#[ignore]` FINDING ATK-1 |

```
cd tests-rs && cargo test --offline --test adversarial                       # about 7 s
cd tests-rs && cargo clippy --offline --test adversarial -- -D warnings      # clean
cargo test --offline -p setl8-lev-evaluator --test adversarial_attacks       # 8 pass
cargo test --offline -p setl8-lev-verifier  --test adversarial_attacks       # 7 pass, 1 ignored
cargo test --offline -p setl8-lev-verifier  --test adversarial_attacks -- --ignored   # FAILS (ATK-1), run once, see below
cargo clippy --offline -p setl8-lev-evaluator --test adversarial_attacks -- -D warnings     # clean
cargo clippy --offline -p setl8-lev-verifier  --test adversarial_attacks -- -D warnings      # clean
```

Everything runs against the already built `target/test-deploy` program and the real vault fixture (sha256 42c62670...98c89a6, not rebuilt). No network.

## Class counts (header of `adversarial.rs`, checked by `attack_table_is_complete`)

BLOCKED 6, LIMITED 5, OFF-CHAIN 3, POLICY 2, OPERATIONAL 2 = 18 attacks.

`attack_table_is_complete` holds a const table of (number 1..=18, class, module name, the module's own `CLASS` constant). It fails if a number is missing or repeated, if the table and a module disagree about the class, if a module `aNN_` exists that the table does not list, if a non-OPERATIONAL attack has no test, if an OPERATIONAL attack has no `CHECKLIST`, if A-11 has a test (it must not: no fake test), or if the class counts differ from the numbers it parses out of the header comment of the file. It prints `BLOCKED 6, LIMITED 5, OFF-CHAIN 3, POLICY 2, OPERATIONAL 2` and `69 tests in the modules (0 ignored)` (28 old A-1..A-5 tests + 41 new; the table test itself is the 70th).

## Result table A-1 to A-18

Classes: BLOCKED = the test proves the refusal, the exact error code, and that the account bytes (and the world digest) did not change. LIMITED = the test pins EXACTLY what still passes and where it stops. OFF-CHAIN = the control is in the evaluator, the verifier or the backend; the test is there, and the program-side test pins what the program lets through. POLICY = no code control; the test shows the trace. OPERATIONAL = no test.

| A | attack | class | tests (module `aNN_...`, file) | verdict |
|---|---|---|---|---|
| 1 | evaluator replays an old favourable price | LIMITED + OFF-CHAIN | `a01_old_price_replay`: 7 tests (age 0..60 accepted, 61 refused `ProofTooOld`, whole-second floor, 63 s window pinned, one proof backs many events, time never backwards, +2 s future skew, either admin widens `max_age_secs`). Verifier (`crates/verifier/tests/adversarial_attacks.rs`): `a01_late_breach_with_an_old_sample` (caught: MissedBreach, FalseBreach), `a01_the_same_event_with_a_proof_older_than_the_window_is_a_proof_age_finding` (caught: ProofAge), `a01_the_old_sample_close_is_what_the_program_accepts_and_the_honest_journal_is_valid`, **`a01_finding_atk1_...` (ignored, NOT caught)** | the program bounds the replay by `max_age_secs` only. The verifier catches a late breach but does NOT catch an old sample used for a market Fill/Close: **FINDING ATK-1** |
| 2 | wrong or fake feed id in a proof | BLOCKED | `a02_wrong_feed_id`: 8 tests (6 wrong-feed variants, floating events, remapped feed, unknown sources, value not positive, forged Market account) | `ProofWrongFeed` 6048 etc., world unchanged |
| 3 | impersonate this program to the vault | BLOCKED | `a03_impersonate_the_program_to_the_vault`: 4 tests (the 5 vault sector calls x 3 foreign authorities -> vault `Unauthorized` 6000; control with the real PDA; sigverify ON; own side `ConstraintSeeds` 2006). Real attacker program: `vault_boundary.rs` (item 7, `handover/audit/07-...`) | blocked |
| 4 | anyone but the evaluator posts journal | BLOCKED | `a04_journal_by_anyone_but_the_evaluator`: 4 tests (6 evaluator-only instructions x 5 outsiders -> `Unauthorized`; missing signature 3010; forged config 2006; rotation) | blocked |
| 5 | trader fabricates his own pass | BLOCKED | `a05_trader_fabricates_a_pass`: 5 tests | blocked (the program cannot know `b`/`flat` are TRUE: R-1, see A-17/A-18) |
| 6 | cross-account hedging | OFF-CHAIN | `a06_cross_account_hedging`: 5 tests (below) | detection is NOT in this repo. The program lets opposite positions stand and pays the winner; only the response half (hold, release, violation fail, ban) is on chain |
| 7 | latency arbitrage against a stale oracle | LIMITED | `a07_latency_arbitrage`: 4 tests | still passes: a 59 s old price buys, a 46 s hold books the whole profit. Stops: spread, quick tally cap 0.028 S, admin-tunable `max_age_secs`. Rate limit: backend, no test |
| 8 | keep an account alive or kill it from outside | LIMITED | `a08_keep_alive_or_kill`: 6 tests | outsiders blocked (`Unauthorized`), dead accounts blocked (`AccountNotActive`), throttle pinned. Still passes: the evaluator keeps an idle account alive for 9.95 days; a silent evaluator lets it lapse and anyone may then call the vault's `mark_abandoned` |
| 9 | split a position to dodge the 45 s rule | OFF-CHAIN | program: `a09_split_position`: 3 tests. Evaluator (`crates/evaluator/tests/adversarial_attacks.rs`): 4 tests `a09_*`. Verifier: `a09_a_timer_start_that_ignores_the_restart_on_the_add_is_a_finding` | the evaluator implements the rule exactly (restart on add, partial close keeps, flip new, 45 s/46 s) and the verifier flags a `timer_start` that ignores a restart (QuickMismatch). The program does not hold positions: `timer_start` is a claim |
| 10 | overflow or rounding drift engine vs program | BLOCKED | `a10_overflow_and_rounding_drift`: 2 tests | 128 pseudo-random closes (seed `0x5e78a10d1ff00d12`, long and short, 65 profits / 63 losses) accepted at exactly `engine::pnl_micro`, 256 off-by-one-micro claims refused `PnlMismatch`; extremes get named errors, no panic. Heavy property work: another file |
| 11 | upgrade key compromise | OPERATIONAL | `a11_upgrade_key_compromise::CHECKLIST` (6 items, below). No test | operational |
| 12 | compute or account-size DoS | BLOCKED | `a12_compute_and_account_size_dos`: 3 tests | 9 events, 4 proofs, 9 market accounts -> `BatchTooLarge` 6032; the largest legal batch (8 events, 3 proofs) uses 129,524 CU; the account is 207 bytes before and after every step of a whole lifecycle |
| 13 | wallet cap evasion | OPERATIONAL | `a13_wallet_cap_evasion::CHECKLIST` (4 items) + 1 test of the cap fact (5 active accounts per wallet, `WalletCapReached`; a fresh wallet is not limited) | not blocked, by design |
| 14 | trader presses the button with a position open | OFF-CHAIN | program: `a14_button_with_an_open_position`: 4 tests. Evaluator: 4 tests `a14_*`. Verifier: `a14_a_pass_requested_with_an_open_position_and_a_flat_meta_that_lies_is_invalid` | `NotFlat` on request and on confirm (stored flag re-read); evaluator rejects `NotFlat` whatever the chain flag says; verifier says [StateMismatch, PassInvalid]. A lying `flat=true` passes the program (pinned) |
| 15 | spam `request_advance` | POLICY | `a15_request_spam`: 4 tests | accepted, no rate limit: 25 request/reject cycles in one second leave the account unchanged (45,937 CU per request, 21,688 CU per reject, same signature count on both sides) |
| 16 | veto without cause | POLICY | `a16_veto_without_cause`: 3 tests | 12 request/confirm/veto cycles = 24 days with no payout, `vetoes` = 12, one `PassVetoed` event each; nothing else changes; the config (cap counters) is byte-identical |
| 17 | fake passes starve the daily cap | LIMITED | `a17_fake_passes_starve_the_cap`: 2 tests | with the veto: cap untouched, honest pass pays on time. Without it (R-6): each finalized fake takes a whole window (`DailyPayoutCapReached` 6024 for the honest one); an attacker who calls `finalize_pass` first each window starves the honest pass for as many windows as it has fakes. Two fake payouts reached the vault queue in the test: that is R-1 |
| 18 | mass-fail honest accounts | LIMITED | `a18_mass_fail_honest_accounts`: 4 tests. Verifier: 2 tests `a18_*` | possible and visible; see "A-18 in detail" |

### A-6 in detail (`a06_cross_account_hedging`)

1. `two_wallets_hold_opposite_positions_in_one_market_and_the_program_sees_nothing_wrong`: wallet A long 1M EUR, wallet B short 1M EUR, same second, both accepted; A's journal does not touch B's account bytes; after a 0.5% move A is +4.9k, B -5.1k, and `pnl_A + pnl_B` equals exactly minus the two round-trip spreads.
2. `the_winner_of_a_hedge_pair_is_paid_the_full_formula_amount_by_the_program`: the winner's pass pays $8,000 (0.08 S) through the real vault; B's account is not looked at.
3. `opposite_positions_in_two_accounts_of_one_wallet_are_equally_legal`.
4. `the_on_chain_response_...`: a stranger cannot hold (`Unauthorized`), zero hash refused (`EvidenceMissing`), the evaluator holds; a held account cannot request (`AccountHeld`) and a confirmed one cannot finalize (`AccountHeld`); one admin cannot fail or release (`Unauthorized`); both admins `fail_for_violation`: Failed, marker `PASS_VIOLATION`, wallet banned (`WalletBanned` on open), vault Failed, no payout; wallet B untouched. **LIMITED residual pinned: a fresh wallet opens at once.**
5. `a_false_match_holds_an_honest_trader_until_both_admins_release`: the cost of a false positive.

Not testable here: detection (heuristics on sizes, delays, correlated markets such as EURUSD vs GBPUSD, fresh wallets). That is backend work and is not in this repository. The report must say so.

### A-9, A-14: what exists where

* A-9 evaluator (`crates/evaluator/tests/adversarial_attacks.rs`): tiny order at T plus the big order at T+44 and the close at T+50 is quick (timer = the big add); 10 slices 4 s apart: the timer is the last slice, close 45 s after it is quick, 46 s is not; boundary table 1/44/45 s quick, 46/47 s not; partial close keeps the original timer (second slice not quick); a flip closes the old position (Flip, timer of the old) and the new short has its own timer. Each one is also run through the public verifier and must be VALID.
* A-9 program (`a09_split_position`): the 45 s flag is checked to the second (`QuickMismatch` 6012 at 45 s claiming not-quick, at 46 s claiming quick), `TimerInvalid` for a timer after the close, and, pinned as LIMITED, `timer_start = 10,000,000 s ago` is accepted and a Close with no Fill before it is accepted (`trades_closed` becomes 1): the program has no position book.
* A-14 evaluator: `request_pass` with an open position returns `Reject(NotFlat)` whatever `chain.flat` says; chain says not flat with a flat engine also `NotFlat`; control: flat engine, flat chain, b at target, replay valid -> `Confirm`; queued events -> `JournalBehind` first; watchdog `Mismatch` -> `Reject(Other)`.
* A-14 program: chain not flat -> `request_advance` `NotFlat`; request accepted while flat, a fill posted afterwards (state 1 does not freeze the journal), `confirm_pass` -> `NotFlat` (world unchanged), the evaluator's `reject_pass(NotFlat)` clears it and emits `PassRejected`; a trader cannot reject; **LIMITED pinned: a `Meta.flat = true` after an unclosed fill is accepted, and the pass then requests, confirms and pays $8,000 on chain** (only the evaluator's own check and the verifier's replay stop it); a confirmed pass freezes `post_journal` and `report_fail` (`PassPending`).

### A-18 in detail (`a18_mass_fail_honest_accounts`, program side)

What a compromised evaluator must write to fail an honest $100k account that the program accepts as a failure:

| must be true | otherwise | test |
|---|---|---|
| it goes through `report_fail` with the batch shape `events..., Fail, Meta` | `FailMissing`, `FailNotBeforeMeta`, `FailEventNotAllowed` (via `post_journal`) | `a_bare_fail_event_is_refused_...` |
| `Meta.flat = true` | `FailNotFlat` | same |
| reason code valid and equal to the event's | `FailReasonInvalid`, `FailReasonMismatch` | same |
| `Fail.b_after` equal to the stored balance AFTER the batch's own events | `FailBalanceMismatch` | same |
| that balance at or below 0.92 S (`StaticFloor`) or the quick tally above 0.028 S (`PhaseQuick`) | `FailStaticFloorNotReached`, `FailQuickNotExceeded` | same |
| so the batch must carry a fabricated loss: a **Fee of $8,000** (no rule on chain checks a fee beyond `amount >= 0`) or a **Close of a position nobody opened** with an invented entry price (the program has no position book) | the honest `b` of 100,000 is 8,000 above the floor | `one_fabricated_loss_event_is_enough_...` |
| the account is not in `pass_pending = 2` | `PassPending` (a confirmed pass cannot be failed out from under the delay); state 1 is cleared by the fail | `a_confirmed_pass_cannot_be_failed_but_a_requested_one_can` |

Trace left behind: the `AccountFailed` event (reason, b, seq), the three journal events in the log, vault record Failed, wallet slot freed. Five accounts of one wallet were failed in five transactions (`mass_fail_of_five_accounts_...`): 4 `AccountFailed` events seen, then both admins rotated the evaluator key (`set_evaluator`, one admin alone cannot: A-4); the old key is `Unauthorized` on the fifth account (still Active, untouched); the four failed accounts stay Failed (the new evaluator gets `AccountNotActive`); the only road back is the trader's own paid `reset_account` (the trader's USDC balance drops). Verifier side: the Fee case gives [FeeMismatch]; the Close-without-position case gives [ForgedTrade, PositionMismatch, FloorMismatch, FalseFail].

So the program-side protection against a wrongful fail is: the floor arithmetic, nothing more. Everything else is the verifier after the fact and a human (R-1, OPEN_QUESTIONS #9).

## Checklist items for the operational attacks (no test, copied into the report and the deploy checklist)

**A-11 upgrade key compromise** (`a11_upgrade_key_compromise::CHECKLIST`):
1. After deploy, `solana program show <id>` lists the upgrade authority: it must be the 2-of-2 multisig of SL8 admin + Rov admin, not a hot key.
2. The deploy keypair is destroyed or moved to cold storage after the authority transfer; nobody holds a copy.
3. Each of the two multisig members signs on its own hardware wallet, in different places.
4. The burn decision (make the program immutable) is taken after the independent audit, written down and dated.
5. A monitor alerts both admins on any Upgrade or SetAuthority transaction of the program id (the upgrade is the one lever that can request payouts up to the vault cap; vault boundary item 7 shows the vault itself accepts a claim of up to 2,500,000 USDC per record, so the 0.08 S formula is exactly as strong as this key).
6. The verifiable build (hash of the deployed `.so`) is published; every upgrade is rebuilt and compared before signing.

**A-13 wallet cap evasion** (`a13_wallet_cap_evasion::CHECKLIST`):
1. The backend (KYC, per-device or per-funding-source limits) decides how many wallets one person may use; the program only caps ONE wallet at 5 active accounts.
2. A ban (`ban_wallet`, `fail_for_violation`) is per wallet: the runbook must say how a banned person is recognised again off chain.
3. The sector-wide exposure that matters is the daily payout cap and the vault's record chain bound (R-1), not the wallet count.
4. Review the funding source of new wallets that open accounts in bulk (the program emits `Opened` events that make this a query).

## FINDINGS

### ATK-1 (medium): the verifier does not flag a market Fill or Close priced with an older sample of the `max_age` window

* Where: `crates/verifier/src/replay.rs` (`check_proof`, `on_fill`, `on_close`): a proof must be a real archive record of the market, inside the market's window and inside the batch's committed archive prefix, and the event price must equal the price derived from that proof. Nothing requires the proof to be the NEWEST sample of that market at the event's second.
* Test: `crates/verifier/tests/adversarial_attacks.rs`, `a01_finding_atk1_a_market_close_priced_with_an_older_favourable_sample_must_be_flagged`, `#[ignore = "FINDING ATK-1: ..."]`. Scenario: 200,000 EUR bought at 14:00:00 (mid 1.0800), a spike to 1.0830 at +30 s, back to 1.0800 at +60 s; the honest close at +60 s is a loss by the spread; the dishonest one uses the +30 s sample (genuine, 30 s old, inside the 60 s window, so the PROGRAM accepts it: A-1 tests in `adversarial.rs`) and books a profit. The honest journal is VALID (control, `a01_the_old_sample_close_is_what_the_program_accepts_and_the_honest_journal_is_valid`); the tampered one is internally consistent (price from the proof, pnl from the price).
* Confirmed once: `cargo test --offline -p setl8-lev-verifier --test adversarial_attacks -- --ignored` fails with `an old but genuine sample of the last 60 s was accepted for a close at +60 s: []` (verdict Valid, no finding).
* Why it matters: DESIGN 13.2 A-1 says "Limited: ... The verifier compares against the sample archive." The verifier does compare with the archive for floors, triggers and breaches (`a01_late_breach_with_an_old_sample` is caught), but for a market Fill or Close the slack is the whole window, in BOTH directions (favourable to the trader: fabricated profit and passes; unfavourable: harm). `handover/trust-matrix.md` already lists the unfavourable direction as a NEITHER row ("market order executed at an older sample inside max_age (worse price)"), so the weakness is known; what the matrix does not say is that the favourable direction, which is the A-1 attack as DESIGN words it, is the same hole. Per trade the slack is bounded by `max_age_secs` (launch values in `config/launch.json`: 15 to 45 s) and by the price movement inside it, and the quick tally cap bounds quick trades (A-7), not 46 s+ holds.
* Fix: no on-chain change. The evaluator already prices a market order from the latest sample of the market when the second closes (`crates/evaluator/src/exec.rs`, `latest`), so a deterministic rule exists ("the newest archive record of that market, in stream order, at or before the event's second"); the verifier can enforce it with its archive walker. Classification: a verifier implementation gap against the DESIGN 13.2 wording, but the rule itself is not written down in DESIGN 4.5 (only in M10 notes), so write the rule first, then implement. If the rule is not wanted, DESIGN 13.2 A-1 must stop saying the verifier covers it. Nothing was fixed here.

### Observations that are NOT findings (the program does what the DESIGN says; they size R-1)

* O-1 (A-18, A-10): `Fee` is checked only for `0 <= amount <= i64::MAX` and `b -= amount` (DESIGN I-6). One Fee of $8,000 plus a `Fail` fails any honest account; an amount of `i64::MAX` is accepted and drives `b` to about -9.2e18 without an overflow (`a10_...extreme_quantities...`). The verifier flags it (FeeMismatch); there is no on-chain remedy (R-1, OPEN_QUESTIONS #9).
* O-2 (A-9, A-14, A-18): the program holds no positions. `Close` needs no earlier `Fill`; `entry_e12` and `timer_start` are claims (only `timer_start <= time` and `quick == (time - timer_start <= 45)` are checked); `Meta.flat` is stored as given (DESIGN I-12). A fabricated pass costs one Close with an invented entry price (`a17_...::fabricate`: +$8,000, flat, one transaction).
* O-3 (A-10): the market's `max_notional_usd` is not an on-chain rule (DESIGN 3.2): a Fill of 1.8e19 units (a step multiple, about $18 trillion at 1.0) is accepted by the program. The evaluator refuses and the verifier flags it (`FillNotAllowed`). Pinned in `a10_...`.
* O-4 (A-8): this program skips an activity push inside 86,000 s, the vault's own interval is 86,400 s: a call at 86,000..86,399 s makes a CPI that answers `Throttled` and stores nothing on either side (harmless extra CU). Pinned in `a08_...the_push_is_throttled_twice_here...`.
* O-5 (A-17): `finalize_pass` is permissionless, so with several confirmed passes in the same window whoever calls first takes the cap. In the R-6 case (admins miss the veto window) an attacker holding K fake passes can keep an honest pass waiting for K windows.

## What I could NOT test, and why

* A-6 detection (heuristics across wallets): backend, not in this repo.
* A-7 rate limiting: backend.
* A-11 upgrade authority and A-13 wallet evasion: operational, no fake test (checklists above).
* A-3 with a real attacker SBF program: not in `adversarial.rs` (the earlier run's header says so); `vault_boundary.rs` (item 7) has the real helper program.
* The evaluator's per-account limits (10 pending orders, 8 positions) are off chain and are not in my scope.
* "Would the test fail if the control were removed": I argued it for each test; I did not mutate production code (not allowed). The cargo-mutants run of item 10 is the real check.
