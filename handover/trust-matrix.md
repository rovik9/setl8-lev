# Trust matrix (M10)

Input of the M12 audit report. Generated from `tests-rs/tests/trust_matrix.rs` (the test asserts every row: who catches it, the program's error code, the verifier's finding class; `TRUST_MATRIX_PRINT=1 cargo test --release --test trust_matrix -- --nocapture` prints the table). **54 batch/log tamper classes + 4 chain-level classes = 58 classes.**

## Method

An honest history is scripted (samples and signed orders), run through the reference evaluator, and its batches, sample archive and intents file are edited by a **malicious evaluator that keeps its hash chain and its Meta commitments consistent** (it holds the key, so it always can). Then:

* **Program verdict**: the edited batches are posted in order to the real program and vault in LiteSVM until the first rejection; the exact error code is asserted.
* **Verifier verdict**: the same edited events (as if all had been accepted), archive and intents go through `verify_account` (the public verifier); the finding classes are asserted.
* **WHO**: PROGRAM = the program rejects and the verifier would not notice; VERIFIER = the program accepts and the verifier flags; BOTH = the program rejects and the verifier would also flag the same data; NEITHER = accepted by the program and VALID for the verifier.

Honest outputs of all ten base scenarios are accepted by both (`honest_outputs_are_accepted_by_both`).

Summary: BOTH 20, PROGRAM 6, VERIFIER 25, NEITHER 3, plus 4 chain-level PROGRAM rows.

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

## Residual trust

Every NEITHER row, with the reason it stays open. These are what a user of option C trusts the evaluator operator (or the second recorder) for.

* **a sample that caused a breach dropped from the archive, breach omitted (no second recorder)**: A malicious evaluator that rewrites its own sample stream and its own journal consistently leaves nothing to contradict (DESIGN 4.5 "Honest limit", R-2): only an independent second recorder (next row) exposes it.
* **market order executed at an older sample inside max_age (worse price)**: The proof only has to be a real sample no older than `max_age_secs` (A-1, A-7); nothing says it must be the NEWEST one at the order's time, so the evaluator can pick the worst price of the window (up to 900 s in this test's markets). Bounded by `max_age_secs` and the spread; shorten it per market to shrink the slack.
* **a trader's market order ignored (no fill, intent on file)**: A signed market or close intent with no trade under it is indistinguishable from an order the evaluator never received or that was rejected for margin; the intents file proves what was signed, not that it was executed (censorship). Pending orders are different: those are checked (`TriggerMissed`).

Two more limits follow from the table without being separate rows: a verifier that is not run catches nothing (the watchdog is an operational duty, DESIGN 13.3), and the unsigned price sources (decision 46: GMX tickers, Hyperliquid) prove only what the evaluator saw, not what was true.
