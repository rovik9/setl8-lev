# Trust matrix (M10)

Input of the M12 audit report. Generated from `tests-rs/tests/trust_matrix.rs` (the test asserts every row: who catches it, the program's error code, the verifier's finding class; `TRUST_MATRIX_PRINT=1 cargo test --release --test trust_matrix -- --nocapture` prints the table). **73 batch/log tamper classes + 4 chain-level classes = 77 classes (+ 11 Reset rows). Since M12 every row has exactly ONE named test** (`tests-rs/tests/trust_matrix.rs`: `row_NNN_<slug>` and `chain_row_NN_<slug>`; `tests-rs/tests/trust_reset.rs`: `reset_row_NN_<slug>`; `every_row_has_exactly_one_named_test` and `trust_matrix_document_matches_rows` keep the document and the tests in step). Rows without a test: 0.

## Method

An honest history is scripted (samples and signed orders), run through the reference evaluator, and its batches, sample archive and intents file are edited by a **malicious evaluator that keeps its hash chain and its Meta commitments consistent** (it holds the key, so it always can). Then:

* **Program verdict**: the edited batches are posted in order to the real program and vault in LiteSVM until the first rejection; the exact error code is asserted.
* **Verifier verdict**: the same edited events (as if all had been accepted), archive and intents go through `verify_account` (the public verifier); the finding classes are asserted.
* **WHO**: PROGRAM = the program rejects and the verifier would not notice; VERIFIER = the program accepts and the verifier flags; BOTH = the program rejects and the verifier would also flag the same data; NEITHER = accepted by the program and VALID for the verifier.

Honest outputs of all ten base scenarios are accepted by both (`honest_outputs_are_accepted_by_both`).

Summary (after P21; P25b moved one row from NEITHER to VERIFIER; P26 added one BOTH row): BOTH 32, PROGRAM 2, VERIFIER 36, NEITHER 3, plus 4 chain-level PROGRAM rows and the 11 Reset (M8) rows below. P21 (decision 64, unpriced markets are untradable) added three rows and moved none: two BOTH ("a market fill in a market the account's rule set does not price" and "the fill of a triggered pending order in a market the rule set does not price") and one NEITHER that is the rule itself, not an attack ("a triggered pending order in an unpriced market that the evaluator does not fill"). Before that, after M9c: BOTH 28, PROGRAM 3, VERIFIER 35, NEITHER 3, plus 4 chain-level PROGRAM rows and the 11 Reset (M8) rows below. M9c moved four rows: "fill in a closed session", "fill on a held account" and "fill during the 12 h lock after a breach" from PROGRAM to BOTH, and "a market fill the account cannot margin" from NEITHER to VERIFIER; it added "a market fill above the market's notional cap" (VERIFIER). Before that (M9b): BOTH 25, PROGRAM 6, VERIFIER 33, NEITHER 4. M9b (decisions D1 to D5 of design decision 53) added 14 rows: 5 BOTH, 8 VERIFIER, 1 NEITHER; the NEITHER row is NOT a row that moved: it is a limit the work on D3 made visible (below). No existing row changed class.

## Caught by the program, and also by the verifier (BOTH)

| class | who | program (error code) | verifier (finding class) | note |
|---|---|---|---|---|
| fill price altered by one unit | Both | EventPriceMismatch (6055) | [PriceMismatch] |  |
| close exit price altered (pnl recomputed) | Both | EventPriceMismatch (6055) | [PriceMismatch] |  |
| close pnl wrong by one micro-USD | Both | PnlMismatch (6011) | [PnlMismatch] |  |
| quick flag flipped on a quick slice | Both | QuickMismatch (6012) | [QuickMismatch] |  |
| quick profit credited in full | Both | CreditMismatch (6062) | [CreditMismatch] |  |
| fee amount beyond i64 | Both | FeeInvalid (6013) | [FeeMismatch] |  |
| tight start: l_level one micro off | Both | TightStartLLevel (6069) | [FloorMismatch] |  |
| floor raise: l_level one micro off | Both | FloorLLevel (6071) | [FloorMismatch] |  |
| floor raise dropped (the breach then names a stale level) | Both | BreachLLevel (6074) | [FloorMismatch] |  |
| tight start and floor raise swapped | Both | TightNotOn (6064) | [FloorMismatch] |  |
| meta: engine version wrong | Both | EngineVersionMismatch (6083) | [EngineVersion] |  |
| batch replayed | Both | BadSequence (6007) | [SequenceGap] |  |
| event time goes backwards | Both | TimeBackwards (6008) | [StateMismatch] |  |
| proof carries another market's feed | Both | ProofWrongFeed (6048) | [ProofFeed] |  |
| proof older than the market's max_age | Both | ProofTooOld (6049) | [ProofAge] |  |
| batch of nine events | Both | BatchTooLarge (6032) | [FeeMismatch] | Encoded by hand (the encoder refuses it). The verifier only sees the extra fees. |
| fill quantity not a multiple of the step | Both | QuantityInvalid (6060) | [ForgedTrade, FillNotAllowed] | M9c: the verifier now also checks step and minimum, so it names the quantity itself (`FillNotAllowed`) as well as the intent total that no longer adds up (`ForgedTrade`). |
| fill in a closed session | Both | MarketClosed (6059) | [FillNotAllowed] | M9c (was PROGRAM): the verifier knows the sessions and the price's last change as of the order's sample. |
| fill on a held account | Both | AccountHeld (6037) | [FillNotAllowed] | M9c (was PROGRAM): the log now carries `account_held` / `hold_released` lines dated with the block time. |
| fill during the 12 h lock after a breach | Both | AccountLocked (6005) | [FillNotAllowed] | M9c (was PROGRAM): the verifier judges a market fill against the replayed `lock_until`. |
| a market fill in a market the account's rule set does not price | Both | MarketUnpriced (6109) | [FillNotAllowed] | P21, decision 64: the half-spread of the market in the account's own (immutable) rule set is 0, so an exposure-increasing `Fill` there is refused by the program (checked right after the open-allowed checks, before the proof and the quantity) and by the verifier (`open_refusal`, reason "unpriced market"). Closes are never refused by this rule. |
| the fill of a triggered pending order in a market the rule set does not price | Both | MarketUnpriced (6109) | [FillNotAllowed] | P21: the same for a `Limit`/`Stop` fill whose order is on file (an honest evaluator would not have accepted the order at all). The verifier's walk finds the refusal at the trigger second and reports the fill as an execution the evaluator's own rules refuse. |
| static-floor fail on an account above the floor | Both | FailStaticFloorNotReached (6097) | [FalseFail] |  |
| quick-limit fail on an account below the limit | Both | FailQuickNotExceeded (6098) | [FalseFail] |  |
| fill on an account that already failed | Both | AccountNotActive (6004) | [StateMismatch] | Program: `AccountNotActive`. The verifier reports events after the account ended. |
| pending order that only reduces: its Close at a price altered by one unit | Both | EventPriceMismatch (6055) | [PriceMismatch] | D1: the Close that executes a pending order is checked like any close. |
| commission fee posted after the slice that overshoots the quick limit | Both | QuickLimitExceeded (6018) | [FeeMismatch] | D2: the program allows nothing but the `Fail` after the overshoot; since M9b the verifier no longer expects the fee and flags it too. |
| breach posted where its closes end at or below the static floor | Both | BreachBelowStaticFloor (6077) | [FalseBreach] | D4 (static floor). The verifier expects `Fail(StaticFloor)` at that sample. |
| Fail omitted (a TightEnd instead) where a breach's closes end at or below the static floor | Both | MustReportFail (6089) | [MissedFail] | D4: a flat batch at or below the static floor must be `report_fail`; the verifier reports the missing `Fail`. |
| breach posted where its closes overshoot the quick limit | Both | QuickLimitExceeded (6018) | [FalseBreach] | D4 (quick limit). |
| forced close after the quick-limit overshoot relabelled as the trader's own close (reason Manual) | Both | QuickLimitExceeded (6018) | [ForgedTrade] | P26 (decision 69, F-16): after the overshoot the program takes nothing but `Close` with reason `Fail` in the failing `report_fail` batch; a forced close dressed as a trader's close is refused, and the verifier finds no order for it. |
| event dated 30 s after the block time | Both | TimeInFuture (6009) | [PriceSampleMismatch] | The program compares with the cluster time (I-3, +2 s). P25b (was PROGRAM): the verifier has no clock, but the shifted event second no longer matches the sample the price rule names (the fill is priced with an OLDER sample than the newest at the new second). |

## Caught by the program only (PROGRAM)

| class | who | program (error code) | verifier (finding class) | note |
|---|---|---|---|---|
| wrong prev_hash | Program | BadPrevHash (6043) | - | `prev_hash` is a batch field, not an event: it is in no log line, so only the program ever sees it. |
| posted by a key that is not the evaluator | Program | Unauthorized (6000) | - | The signer is an account of the transaction, not part of the journal. |

## Caught by the verifier only (VERIFIER)

These are the classes the chain cannot see (DESIGN 1.6 right-hand column): prices, omissions, fees, positions, intents and the archive. Their safety rests on someone running `setl8-verify` (the watchdog) inside the 2-day window.

| class | who | program (error code) | verifier (finding class) | note |
|---|---|---|---|---|
| commission fee missing after a close | Verifier | - | [FeeMismatch] |  |
| commission fee one micro-USD too high | Verifier | - | [FeeMismatch] |  |
| commission fee placed before its fill | Verifier | - | [FeeMismatch] | The program only needs `amount >= 0` (I-6). |
| holding fee omitted at the rollover | Verifier | - | [FeeMismatch] |  |
| holding fee one micro-USD too low | Verifier | - | [FeeMismatch] |  |
| Meta.samples_hash is not a prefix of the archive | Verifier | - | [SamplesHashMismatch] |  |
| Meta.intents_hash is not a prefix of the intents file | Verifier | - | [IntentsHashMismatch] |  |
| Meta.flat says flat while a position is open | Verifier | - | [StateMismatch] |  |
| breach posted late with the old sample (replayed price) | Verifier | - | [MissedBreach, FalseBreach] | DESIGN A-1: an old, genuine sample is still inside `max_age`, so the program accepts the proof; the archive walk finds the breach at its own second and finds none at the posted one. |
| breach built from the worst sample of each market (never simultaneous) | Verifier | - | [FalseBreach] | Cherry-picked samples of different instants make a breach the program accepts (every claim is arithmetically true); only the archive walk knows the equity was never that low at any one instant. |
| breach omitted (the trader keeps the position) | Verifier | - | [MissedBreach] |  |
| fail omitted at the static floor | Verifier | - | [MissedFail] |  |
| pass requested and confirmed on an account that is not flat | Verifier | - | [PassInvalid, StateMismatch] | `flat` is the evaluator's assertion (I-12); the chain cannot check it. The watchdog replay (`setl8-verify watch`) catches it inside the 2-day window. |
| slow profitable slice booked as quick (half credited) | Verifier | - | [QuickMismatch] |  |
| close names another entry price (pnl consistent with it) | Verifier | - | [AvgEntryMismatch] |  |
| fill reports a wrong average entry | Verifier | - | [AvgEntryMismatch] |  |
| fill reports a wrong margin | Verifier | - | [MarginMismatch] |  |
| trade with no signed intent (forged trade) | Verifier | - | [ForgedTrade] |  |
| intent signed by a key that is not the trader's | Verifier | - | [BadIntentSignature] |  |
| archive: the price of the proof's sample altered | Verifier | - | [ArchiveMissing] |  |
| archive: the sample behind a fill dropped | Verifier | - | [ArchiveMissing] |  |
| stop-loss triggered but not executed | Verifier | - | [TriggerMissed] |  |
| limit order triggered but never filled | Verifier | - | [TriggerMissed] |  |
| stop-loss close with no trigger | Verifier | - | [TriggerFalse] | An evaluator-originated close needs no intent; only the walk knows no sample triggered it. |
| the same, with an independent second recorder | Verifier | - | [MissedBreach] |  |
| pending order that only reduces: the Close that executes it is missing | Verifier | - | [TriggerMissed] | D1: the trigger passes and nothing executes the order. |
| Close under the nonce of a pending order that no sample triggers | Verifier | - | [TriggerFalse] | D1: nothing triggered that order at that second. |
| Close under the nonce of a set_sl_tp | Verifier | - | [ForgedTrade] | D1: not an order of the trader that can be executed by a close. |
| flip that drops its opening remainder although its closing slice did not overshoot | Verifier | - | [ForgedTrade] | D2: the trades under the intent do not add up. |
| pending order the account could not margin is filled anyway | Verifier | - | [PendingFillWithoutMargin] | D3: the program does not check margin at all (DESIGN 7.3 puts it off-chain). The same order with enough margin and no fill is the existing row "limit order triggered but never filled". |
| phase_advanced to another phase after pass_finalized | Verifier | - | [StateMismatch] | D5: log lines are indexer output; the program never sees them. |
| phase_advanced repeated after pass_finalized | Verifier | - | [StateMismatch] | D5. |
| phase_advanced with no pass line before it | Verifier | - | [StateMismatch] | D5. |
| a market fill the account cannot margin | Verifier | - | [FillWithoutMargin] | M9c, B1 (was NEITHER): neither the program (DESIGN 7.3 puts margin off-chain) nor, until M9c, the verifier said no. The verifier now judges every exposure-increasing execution with the evaluator's own formula. Exactly enough margin is accepted, one micro-USD short is not (`crates/verifier/src/walk.rs refusal_tests`, `tests/m9c.rs`). |
| a market fill above the market's notional cap | Verifier | - | [FillNotAllowed] | M9c, B2: the cap is an evaluator rule (`max_notional_usd`, micro-USD); the program does not check it. |
| market order executed at an older sample inside max_age (worse price) | Verifier | - | [PriceSampleMismatch] | P25b (decision 68, F-08, was NEITHER): the verifier prices every market Fill/Close with the NEWEST archive sample of the market at or before the event second (DESIGN 4.5); an older sample, favourable or not, and a newer one are `PriceSampleMismatch`. |

## Chain-level rows (the pass flow; PROGRAM)

| class | who | program (error code) |
|---|---|---|
| confirm_pass with no pass requested | Program | NoPendingPass (6022) |
| post_journal while a pass is confirmed (the journal is frozen) | Program | PassPending (6019) |
| request_advance below the target (b < 1.08 S) | Program | TargetNotReached (6021) |
| finalize_pass before the delay has elapsed | Program | PassDelayNotElapsed (6023) |

## NEITHER (the honest limits of option C)

Unchanged by M9c except for one row that left it ("a market fill the account cannot margin", now VERIFIER). Each row that stays is one sentence under "Residual trust": a dropped sample is invisible without a second recorder; the older-sample price is inside `max_age_secs`; a silent market order leaves no trace the files can contradict.

| class | who | program (error code) | verifier (finding class) | note |
|---|---|---|---|---|
| a sample that caused a breach dropped from the archive, breach omitted (no second recorder) | Neither | - | - |  |
| a trader's market order ignored (no fill, intent on file) | Neither | - | - |  |
| a triggered pending order in an unpriced market that the evaluator does not fill (the rule itself) | Neither | - | - | P21: not an attack. The order is on file, the samples trigger it, the evaluator does not fill it; the verifier computes the same refusal (`Refusal::Unpriced`) and expects no fill, so there is no `TriggerMissed`. The same silence in a PRICED market is a miss (`TriggerMissed`, rows above). |

## Reset (M8)

Rows of `reset_account`, asserted in `tests-rs/tests/trust_reset.rs`. A reset has no price argument and no phase argument, so most of what a cheating trader or evaluator could try is not expressible; these are the rows that are. WHO as above, plus VAULT = the vault rejects (its own error code).

| class | who | caught by | note |
|---|---|---|---|
| reset: a price below the registry's tier price paid to the vault (a fake reset price) | Vault | WrongAmount (6007) | Not expressible through our instruction (the amount is computed from the registry, not passed); in production only our program can sign as `sector_authority`. The harness calls `deposit_reset` directly with the PDA flagged as a signer. |
| reset: the registry replaced by a copy with cheaper tiers at another address | Program | InvalidVaultAccount (6103) | The registry must be the vault-owned PDA `["product_registry", this program]`; address, owner and header are checked before a byte of it is used. |
| reset: an account that has not failed (an Active account of a high phase) | Program | ResetNotAllowed (6027) | |
| reset: a second reset of the same failed record | Program | AccountAlreadyReset (6107) | Before any CPI. |
| reset: a second reset of the same failed record with the local mark wiped (the vault is the backstop) | Vault | ResetNotAllowed (6011) | Only reachable by editing the account, which no instruction does; shows the vault holds the line on its own. |
| reset: by a banned wallet | Program | WalletBanned (6035) | |
| reset: a violation-failed account after the wallet was unbanned (DESIGN 2.9: no reset) | Program | ViolationNoReset (6108) | P19 / decision 60. `fail_for_violation` leaves `pass_pending = 3` on the Failed account; `reset_account` refuses it before any CPI, banned wallet or not (while the ban lasts `WalletBanned` answers first). `report_fail`, Abandoned and Graduated never set it. |
| reset: while the sector is paused | Program | Paused (6001) | |
| reset: while the vault pauses this product | Vault | ProductNotActive | The whole transaction reverts, nothing is marked. |
| reset into a higher phase: the new account copies the failed phase (there is no phase argument) | Program | phase = failed phase | For every phase 1 to 5 the new account's phase is the failed account's. The ladder entry the vault prices with is the failed phase minus one, so a wrong phase would also be a wrong price (WrongAmount). |
| reset: an evaluator log that opens the new account in a higher phase than the chain's account | Verifier | StateMismatch | The `opened` line of the log says phase 4, the chain's account (the snapshot) says 3. The link between the failed account and its successor is the `AccountReset` event (previous account, phase); the verifier of one account does not follow it, an auditor compares `Opened.phase` with the last phase of the previous account's log. |

A policy point that used to be open here is closed (P19, decision 60): a violation-failed account can no longer be reset once the wallet is unbanned; the row above is the proof. `unban_wallet` still lets the trader open NEW accounts (and reset accounts that failed normally); it never revives a failed one.

## Residual trust

Every NEITHER row, with the reason it stays open. These are what a user of option C trusts the evaluator operator (or the second recorder) for.

* **a sample that caused a breach dropped from the archive, breach omitted (no second recorder)**: A malicious evaluator that rewrites its own sample stream and its own journal consistently leaves nothing to contradict (DESIGN 4.5 "Honest limit", R-2): only an independent second recorder (next row) exposes it.
* **a trader's market order ignored (no fill, intent on file)**: A signed market or close intent with no trade under it is indistinguishable from an order the evaluator never received or that was rejected for margin; the intents file proves what was signed, not that it was executed (censorship). Pending orders are different: those are checked (`TriggerMissed`). M9c recomputes the reasons that depend on the account and the rules (margin, hold, lock, status, step and minimum, caps, a closed market) for every execution and for every triggered pending order; what stays out of reach for a market order is why NOTHING happened: the evaluator may refuse at arrival or at execution for reasons that depend on the moment (the newest sample older than `max_age_secs`, an invalid stop or take-profit level, the order sample it saw), none of which the files record.
* **a triggered pending order in an unpriced market that the evaluator does not fill (the rule itself)**: Listed so that the table is complete, not because trust is needed: decision 64 makes the non-fill REQUIRED, and the verifier agrees with it (the account's rule set has no half-spread for the market, so no exposure can be opened there). What an operator could do instead, fill it, is the BOTH row above. An honest evaluator refuses such an order at placement and never writes it to the intents file; the row exists for the order a dishonest evaluator publishes in the file.

Two more limits follow from the table without being separate rows: a verifier that is not run catches nothing (the watchdog is an operational duty, DESIGN 13.3), and the unsigned price sources (decision 46: GMX tickers, Hyperliquid) prove only what the evaluator saw, not what was true.

## M12 additions (self-audit, handover/AUDIT_REPORT.md)
* No row moved class. The dishonest-evaluator table (`tests-rs/tests/adversarial_evaluator.rs`) re-asserts 17 rows from the evaluator's side and found ONE case that belongs in no row: a `report_fail(StaticFloor)` built from the real lows of two markets that never coexisted is accepted by the program and VALID for the verifier (finding F-09 / TMX-1, a GAP, not a NEITHER row: DESIGN A-18 says a mass fail is "visible", and for this shape it is not). It is an open finding with a failing ignored test, not an accepted limit.
* The row "market order executed at an older sample inside max_age (worse price)" and its favourable twin (F-08 / ATK-1) moved from NEITHER to VERIFIER in P25b (see below).
* The verifier now refuses (input error) a rules file whose version differs from the account's (F-04) and verifies intent signatures with `verify_strict` (F-03); neither changes a row.

## P25a changes (decision 67, handover/AUDIT_REPORT.md "Changes after the audit")
* No row moved class and no row was added. The "finalize_pass before the delay has elapsed" row is unchanged; the delay itself is now limited to 1 to 6 days by the program (`InvalidLimits`), so the vault's 7-day idle limit cannot swallow a confirmed pass if the evaluator refreshes the activity during the delay (the evaluator's duty, deploy checklist).
* The admin and cancel events (`EvaluatorChanged`, `LimitsChanged`, `PausedChanged`, `RuleSetCreated`, `MarketCreated`, `MarketUpdated`, `MarketFeedChanged`, `WalletBanned`, `WalletUnbanned`, `PassCancelled`) are CPI events now, the same delivery as the journal events. Who can see them changed from "anyone who reads the transaction log text" to "anyone who reads inner instructions"; the verifier still does not model them (F-19, F-24 stay open).
* F-32 is FIXED by P25c (see below).

## P25c additions (decision 67 amendment, audit F-32): an account failed while held
A held account is not frozen for losses (`Fee` and `Close` events still post), so the evaluator's `report_fail` stays allowed on it; the account then stays Failed + held until both admins decide. Two new rows (the rest of the matrix is unchanged); each has a named test in `tests-rs/tests/held_fail_review.rs`:

| class | who | caught by | note |
|---|---|---|---|
| reset: a trader resets an account that was failed while held (hedge farming: pay the reset price and carry on before the review) | Program | ReviewPending (6110) | `reset_of_a_failed_account_that_is_still_held_is_refused_with_review_pending`. Before any CPI, bytes untouched; permanent until both admins `release_hold` (then an ordinary reset) or `fail_for_violation` (then ViolationNoReset, 6108, for ever). The evaluator cannot make the review disappear by failing the account: `report_fail` keeps the hold. |
| close: a trader closes a Failed + held account to erase the evidence and leave the wallet's `held_count` above zero | Program | ReviewPending (6110) | `close_account_refuses_a_failed_held_account_until_the_hold_is_resolved`. Without this a closed Failed + held account would block `ban_wallet` for the wallet for good (`WalletHasHeldAccount`). |

Unchanged: a Failed + held account cannot post any journal event (`AccountNotActive`), so its balance cannot move any more; only the two admins can resolve it (`release_hold`, `fail_for_violation`), both-admin instructions in the who-may-call matrix. A stolen evaluator key can still `hold_account` anyone (a pause, DESIGN 4.7), and with this change can also leave a held account Failed + held by failing it at its real floor; that is a delay for the trader until both admins release, not a loss beyond the failure the key could already cause.

## P25b changes (decision 68, handover/AUDIT_REPORT.md "Changes after the audit, part 3")
* Rows moved: "market order executed at an older sample inside max_age (worse price)" NEITHER -> VERIFIER (`PriceSampleMismatch`; its favourable twin, ATK-1, is the same class); "event dated 30 s after the block time" PROGRAM -> BOTH (the shifted event no longer matches the sample the price rule names). Counts: BOTH 31, PROGRAM 2, VERIFIER 36, NEITHER 3 (was 30 / 3 / 35 / 4), plus the 4 chain-level rows and the 11 Reset rows.
* Cases that stay the same class but gained a finding: the late-breach case (DE-03a) also reports `ForcedCloseUnjustified`; the mass fail from non-simultaneous lows (DE-04d, TMX-1, was a GAP) is now a VERIFIER case: `ForcedCloseUnjustified` + `FalseFailMarks`.
* New finding classes (appended): `ArchiveRecordInvalid`, `PriceSampleMismatch`, `ForcedCloseUnjustified`, `FalseFailMarks`.
* A poison archive record, a replayed or out-of-window intent, a Meta that goes back in time, an account size that is no tier of the rules and a second recorder that differs by one e18 unit are now findings of the verifier; they were limits (pinned VALID) before. A cancelled pass, an abandoned account and an admin market change are understood by the verifier.
* Still NEITHER (unchanged): a sample that caused a breach dropped from the archive without a second recorder; a trader's market order ignored; the unpriced-market rule itself.
* Deferred (F-25): lifecycle lines (hold, release, phase) are not part of the hash chain; the verifier orders them by file position and block time. A Fill posted after a release but dated before it can be judged wrongly (a mis-ordered judgement, not a theft). A fix is a v2 log format with a sequence number on those events.

## P26 changes (decision 69, handover/AUDIT_REPORT.md "Changes after the audit, part 4")
* One BOTH row added: "forced close after the quick-limit overshoot relabelled as the trader's own close (reason Manual)" (`row_073_*`, scenario `sc_two_quick`: two positions, the first close crosses the quick limit, the other position is closed with it as `Close{Fail}`). Counts: BOTH 32, PROGRAM 2, VERIFIER 36, NEITHER 3 (was 31 / 2 / 36 / 3), plus the 4 chain-level rows and the 11 Reset rows; 73 batch/log rows + 4 chain rows = 77 classes.
* What the program now accepts that it refused before (F-16): in a `report_fail(PhaseQuick)` batch, once a close has taken `q_tally` over 0.028 S, `Close` events with reason `Fail` may follow, up to the `Fail`. They are a larger set than the other rows assume: a compromised evaluator can add `Close{Fail}` events there that the program does not check against open positions (the program keeps none); the verifier does (`PositionMismatch` for a close of a position that is not open, `ForcedCloseUnjustified` for a `Close{Fail}` no quick-limit `Fail` of its own second justifies). Each such close is checked like any other close (price proof, arithmetic of its pnl and credit) and adds to `q_tally`; the batch must still end in `Fail{PhaseQuick}` and a flat `Meta` with `b_after` equal to the balance after the closes, so the extra closes change only the balance of an account that fails in the same batch (a Failed account cannot post, pass, be paid or advance; it can only be reset into a new account or, once the hold is resolved, closed). Pinned by `crates/evaluator/tests/multi_quick.rs` (honest VALID; `Close{Fail}` without an overshoot, for a position that is not open, after the account is flat: all caught) and by `tests-rs/tests/quick_overshoot_closes.rs` (program refusals with exact errors).
