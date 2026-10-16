# Trust matrix (M10)

Input of the M12 audit report. Generated from `tests-rs/tests/trust_matrix.rs` (the test asserts every row: who catches it, the program's error code, the verifier's finding class; `TRUST_MATRIX_PRINT=1 cargo test --release --test trust_matrix -- --nocapture` prints the table). **68 batch/log tamper classes + 4 chain-level classes = 72 classes.**

## Method

An honest history is scripted (samples and signed orders), run through the reference evaluator, and its batches, sample archive and intents file are edited by a **malicious evaluator that keeps its hash chain and its Meta commitments consistent** (it holds the key, so it always can). Then:

* **Program verdict**: the edited batches are posted in order to the real program and vault in LiteSVM until the first rejection; the exact error code is asserted.
* **Verifier verdict**: the same edited events (as if all had been accepted), archive and intents go through `verify_account` (the public verifier); the finding classes are asserted.
* **WHO**: PROGRAM = the program rejects and the verifier would not notice; VERIFIER = the program accepts and the verifier flags; BOTH = the program rejects and the verifier would also flag the same data; NEITHER = accepted by the program and VALID for the verifier.

Honest outputs of all ten base scenarios are accepted by both (`honest_outputs_are_accepted_by_both`).

Summary: BOTH 25, PROGRAM 6, VERIFIER 33, NEITHER 4, plus 4 chain-level PROGRAM rows. M9b (decisions D1 to D5 of design decision 53) added 14 rows: 5 BOTH, 8 VERIFIER, 1 NEITHER; the NEITHER row is NOT a row that moved: it is a limit the work on D3 made visible (below). No existing row changed class.

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
| fill quantity not a multiple of the step | Both | QuantityInvalid (6060) | [ForgedTrade] | The verifier does not check step/minimum; the changed quantity no longer adds up to the intent (`ForgedTrade`). |
| static-floor fail on an account above the floor | Both | FailStaticFloorNotReached (6097) | [FalseFail] |  |
| quick-limit fail on an account below the limit | Both | FailQuickNotExceeded (6098) | [FalseFail] |  |
| fill on an account that already failed | Both | AccountNotActive (6004) | [StateMismatch] | Program: `AccountNotActive`. The verifier reports events after the account ended. |
| pending order that only reduces: its Close at a price altered by one unit | Both | EventPriceMismatch (6055) | [PriceMismatch] | D1: the Close that executes a pending order is checked like any close. |
| commission fee posted after the slice that overshoots the quick limit | Both | QuickLimitExceeded (6018) | [FeeMismatch] | D2: the program allows nothing but the `Fail` after the overshoot; since M9b the verifier no longer expects the fee and flags it too. |
| breach posted where its closes end at or below the static floor | Both | BreachBelowStaticFloor (6077) | [FalseBreach] | D4 (static floor). The verifier expects `Fail(StaticFloor)` at that sample. |
| Fail omitted (a TightEnd instead) where a breach's closes end at or below the static floor | Both | MustReportFail (6089) | [MissedFail] | D4: a flat batch at or below the static floor must be `report_fail`; the verifier reports the missing `Fail`. |
| breach posted where its closes overshoot the quick limit | Both | QuickLimitExceeded (6018) | [FalseBreach] | D4 (quick limit). |

## Caught by the program only (PROGRAM)

| class | who | program (error code) | verifier (finding class) | note |
|---|---|---|---|---|
| wrong prev_hash | Program | BadPrevHash (6043) | - | `prev_hash` is a batch field, not an event: it is in no log line, so only the program ever sees it. |
| posted by a key that is not the evaluator | Program | Unauthorized (6000) | - | The signer is an account of the transaction, not part of the journal. |
| event dated 30 s after the block time | Program | TimeInFuture (6009) | - | The verifier has no clock; the program compares with the cluster time (I-3, +2 s). |
| fill in a closed session | Program | MarketClosed (6059) | - | Order-time market status and sessions are not re-checked by the verifier (module 9, decision 12). |
| fill on a held account | Program | AccountHeld (6037) | - | `held` is account state set by `hold_account`; it is in no journal event. |
| fill during the 12 h lock after a breach | Program | AccountLocked (6005) | - | The verifier replays the breach but does not judge a later Fill against `lock_until` (only pending triggers use it). A verifier gap, covered by the program. |

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

## Chain-level rows (the pass flow; PROGRAM)

| class | who | program (error code) |
|---|---|---|
| confirm_pass with no pass requested | Program | NoPendingPass (6022) |
| post_journal while a pass is confirmed (the journal is frozen) | Program | PassPending (6019) |
| request_advance below the target (b < 1.08 S) | Program | TargetNotReached (6021) |
| finalize_pass before the delay has elapsed | Program | PassDelayNotElapsed (6023) |

## NEITHER (the honest limits of option C)

| class | who | program (error code) | verifier (finding class) | note |
|---|---|---|---|---|
| a sample that caused a breach dropped from the archive, breach omitted (no second recorder) | Neither | - | - |  |
| market order executed at an older sample inside max_age (worse price) | Neither | - | - |  |
| a trader's market order ignored (no fill, intent on file) | Neither | - | - |  |
| a market fill the account cannot margin | Neither | - | - |  |

## Residual trust

Every NEITHER row, with the reason it stays open. These are what a user of option C trusts the evaluator operator (or the second recorder) for.

* **a sample that caused a breach dropped from the archive, breach omitted (no second recorder)**: A malicious evaluator that rewrites its own sample stream and its own journal consistently leaves nothing to contradict (DESIGN 4.5 "Honest limit", R-2): only an independent second recorder (next row) exposes it.
* **market order executed at an older sample inside max_age (worse price)**: The proof only has to be a real sample no older than `max_age_secs` (A-1, A-7); nothing says it must be the NEWEST one at the order's time, so the evaluator can pick the worst price of the window (up to 900 s in this test's markets). Bounded by `max_age_secs` and the spread; shorten it per market to shrink the slack.
* **a trader's market order ignored (no fill, intent on file)**: A signed market or close intent with no trade under it is indistinguishable from an order the evaluator never received or that was rejected for margin; the intents file proves what was signed, not that it was executed (censorship). Pending orders are different: those are checked (`TriggerMissed`).

* **a market fill the account cannot margin**: neither the program (DESIGN 7.3 puts margin sufficiency off-chain; it checks the margin FIELD of a fill, not that the account can carry it) nor the verifier (module 9 decision 12; D3 checks only the triggered PENDING orders, whose absence of an event needs a ruling) says no. A malicious evaluator can post a market fill for an account that cannot margin it; the signed intent on file shows the trader asked for the trade, the loss from the leverage follows from the formulas, and the account breaches or fails normally. Closing it would be one more verifier check (the same function D3 uses, applied to market fills). Found while writing the D3 rows; not caused by D1 to D5.

Two more limits follow from the table without being separate rows: a verifier that is not run catches nothing (the watchdog is an operational duty, DESIGN 13.3), and the unsigned price sources (decision 46: GMX tickers, Hyperliquid) prove only what the evaluator saw, not what was true.
