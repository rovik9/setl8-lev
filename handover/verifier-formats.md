# setl8-verify: input formats (version 1)

These formats are used verbatim by the evaluator reference harness (M10). Every file has a version; the test vectors live in `crates/verifier/tests/vectors/<name>/` (`design_4_9`, `quick_profit`, `holding_night`), written by the test-only generator in `crates/verifier/tests/common/mod.rs` (real `engine` and `journal-types`) and re-checked byte for byte by `tests/vectors.rs`. The archive writer and the intent encoder live in the crate (`setl8_verify::archive::ArchiveWriter`, `setl8_verify::intents`, `setl8_verify::log::{header_line, event_line, opened_line}`).

## 1. Event log (`events.jsonl`)
JSON Lines, UTF-8, file order = chain order. Line 1 is the header:
`{"type":"header","format":"setl8-lev-events","version":1}`. Every other line has a `type` (default `event`). Accounts and wallets are base58, hashes and raw bytes lower-case hex, integers are JSON numbers.

| type | fields | mirrors |
|---|---|---|
| `opened` | account, trader, challenge_id, size_s, tier_index, phase, ruleset_version, genesis_hash, time | `Opened` (the first line the verifier needs) |
| `event` | account, seq u64, event hex, proof hex or "", slot u64 | `EventLog`: the raw canonical Borsh bytes the program emitted (no re-encoding) |
| `pass_requested` | account, phase, amount, b, seq, journal_hash, time | `PassRequested` |
| `pass_confirmed` | account, phase, amount, b, seq, journal_hash, eligible_at | `PassConfirmed` |
| `pass_rejected` | account, phase, reason | `PassRejected` |
| `pass_vetoed` | account, phase, admin_a, admin_b, slot | `PassVetoed` |
| `phase_advanced` | account, from_phase, to_phase, time | `PhaseAdvanced` (the program emits it as well as `pass_finalized` for one `finalize_pass` to a phase above 0; section 8, D5) |
| `pass_finalized` | account, phase, to_phase (0 = graduated), amount, request_id, tally_count, tally_total, window_total, time | `PassFinalized` |
| `account_failed` | account, reason, b, seq, journal_hash | `AccountFailed` |
| `violation_fail` | account, trader, evidence_hash | `ViolationFail` |
| `account_held` | account, time | `AccountHeld` (M9c): `hold_account`, dated with the block time of its transaction; the account is held from that second on |
| `hold_released` | account, time | `HoldReleased` (M9c): `release_hold`; the account is no longer held from that second on |
| `pass_cancelled` | account, phase, time | `PassCancelled` (P25b, F-24): the trader withdrew an unanswered request (`cancel_pass`); the pending pass is resolved (`watch` stops reporting it) |
| `account_abandoned` | account, trader, time | `AccountAbandoned` (P25b, F-24): the vault flipped the record to Abandoned after 7 idle days; the account is over, outside the journal, whatever was open or pending |
| `market_feed_changed` | market_id, old_feed hex32, new_feed hex32 | `MarketFeedChanged` (P25b, F-19): an admin remapped the market's feed. **No `account` field**: market events are program-wide, the indexer puts the line at the log position it saw it |
| `market_status_changed` | market_id, old_status, new_status | an admin `update_market` that changed the status (0 disabled, 1 enabled, 2 reduce-only); the indexer reads old and new off the Market account (the program's `MarketUpdated` carries only the id). No `account` field |

**Admin market changes (P25b, F-19).** `rules.json` shows the markets as they stand at the END of the log. The replay starts every market the log changes with the OLD value of its FIRST change and applies each change line at its position: proofs are checked against the feed the market had at that position, and a market order against the status it had. A change must start from the value the market had (`StateMismatch`), a change of a market the rules do not list is `UnknownMarket`, and after the whole log the rules file must show the last change of every market (`StateMismatch`). A log without change lines is judged against the rules file throughout, as before.
An `account_held` line on a held account, and a `hold_released` line on an account that is not held, are `StateMismatch` (the program refuses both). The program's `AccountHeld` / `HoldReleased` events carry no time, so the indexer dates the lines with the block time of the transaction. Any other `type` is an `UnknownEvent` finding. A malformed line (not JSON, wrong field type, bad hex) is an input error (exit code 2). All lines must carry the account of the `opened` line.

## 2. Account snapshot (`snapshot.json`, optional)
`{"version":1,"b":..,"q_tally":..,"latch":0|1,"tight_on":0|1,"r":..,"g_peak":..,"l_level":..,"lock_until":..,"seq":..,"journal_hash":hex,"flat":0|1,"phase":..,"status":..,"trades_closed":..}` = the stored `TradingAccount` fields (DESIGN 5.1). Compared with the replay at the end.

## 3. Rules file (`rules.json`)
`{"version":1,"ruleset_version":u16,"fee_bps":u16,"leverage_x":[8 x u16],"holding_fee_e6":[8 x u32],"commission_e6":[8 x u32],"tier_sizes":[u64..],"spread_e6":{"<market id>":u16,..},"markets":[{"id","class","feed_id":hex32,"max_age_secs","status","qty_step","qty_min","max_notional_usd","sessions":[[start_min,end_min],..],...}]}` (other `Market` fields, such as `quote_conv_market`, are ignored).
From the program's accounts: `RuleSet` gives `fee_bps`, `leverage_x`, `holding_fee_e6`, `commission_e6`, the first `tier_count` of `tier_size`, and `spread_e6[id]` for each existing market; every `Market` account gives one market object (`sessions` = its first `session_count` windows). A market missing from `spread_e6` has half-spread 0.

## 4. Sample archive (`archive.bin`)
One append-only logical stream; the file is one or more SEGMENTS (for example per UTC day), each `header + records`. The stream, and what `Meta.samples_hash` commits to, is the concatenation of the RECORD bytes (headers excluded).

Header (16 bytes): magic `SL8SAMPL` (8) | version u16 LE = 1 | record_len u16 LE = 68 | reserved u32 = 0.

Record (68 bytes, little-endian):
| offset | width | field |
|---|---|---|
| 0 | 2 | market id u16 |
| 2 | 1 | source u8 (2 GMX tickers, 3 GMX signed, 4 Hyperliquid, 255 mock) |
| 3 | 1 | flags, must be 0 (this is what tells a record from a header) |
| 4 | 8 | evaluator receive time, ns, u64 |
| 12 | 8 | source timestamp, ns, u64 (= `PriceProof.timestamp_ns`) |
| 20 | 16 | `quantized_value` i128, 18 decimals (= `PriceProof.quantized_value`) |
| 36 | 32 | blob digest: source 3 = sha256 of the signed blob (= `PriceProof.signed_update`), zeros otherwise |
Stream order = receive order; per market the source timestamps do not decrease. The mid of a sample is `floor(value / 10^6)` e12.

**Validation up front (P25b, F-10, F-11).** A record the verifier will not use is reported as a finding `ArchiveRecordInvalid` (one per record, with the reason) and SKIPPED everywhere else (not found as a proof, not in a market's list, not walked), so one poison record cannot switch the walk off for the others: (1) a `quantized_value` that is not a positive price (`floor(value / 10^6)` must be a positive u64); (2) a record outside the longest non-decreasing run of source SECONDS across the whole stream (where two longest runs exist the one that keeps the EARLIER records wins: a future-dated record, a back-dated one, a whole copy of the archive appended again are the poison, not the honest records after them); (3) a record whose market timestamp (ns) is older than the market's previous accepted record. The raw bytes still count for `samples_hash` (the stream is hashed as written). The second recorder's archive is validated the same way.

### How `samples_hash` / `intents_hash` are matched (prefix search)
`Meta` carries only a hash, no length. The verifier collects the distinct hashes of all `Meta` events, then does ONE pass over the stream: it keeps a running sha256, and after every record boundary (0 records included) tests `hash.clone().finalize()` against the wanted set (`HashSet`, O(1) per test), stopping when all are found. Cost: O(n) hashing of the stream plus one finalize per record, memory O(number of distinct Meta hashes). Hashes that match no boundary are a `SamplesHashMismatch`. The intents file is searched the same way over LINE boundaries (offset 0, after each `\n`, end of file). A later Meta must commit a prefix at least as long as an earlier one (a shrinking prefix is a mismatch), and every sample a batch's proofs use must lie inside that batch's committed prefix.

## 5. Intents file (`intents.jsonl`) and `OrderIntent`
Line 1: `{"format":"setl8-lev-intents","version":1}`. Each other line: `{"nonce":u32,"order":hex,"sig":hex}`. Nonce 0 is reserved ("no intent"); nonces are unique. `Meta.intents_hash` = sha256 of a prefix of the file bytes ending at a line boundary (the header line counts).

`order` = canonical `OrderIntent`, 48 bytes little-endian:
| offset | width | field |
|---|---|---|
| 0 | 1 | kind: 0 market, 1 limit, 2 stop, 3 close, 4 cancel, 5 set_sl_tp |
| 1 | 2 | market id u16 |
| 3 | 1 | side: 0 buy/long, 1 sell/short (0 for close, cancel, set_sl_tp) |
| 4 | 8 | qty u64, 1e-6 base units (kind close: 0 = the whole position) |
| 12 | 8 | price_e12: limit or stop level, else 0 |
| 20 | 8 | sl_e12 (0 = none) |
| 28 | 8 | tp_e12 (0 = none) |
| 36 | 4 | target_nonce: the order a cancel refers to, else 0 |
| 40 | 8 | client_time i64, unix seconds |
The offsets are pinned by the `order_byte_table` test.

Signed message: `b"SL8LEV|" ++ account (32) ++ b"|" ++ nonce u32 LE ++ b"|" ++ order bytes`; ed25519, by the TRADER's key (the `trader` of the `opened` line).

**An intent is executed once, in its window (P25b, F-14, F-18).** Nonces are unique across the WHOLE log: a nonce used by the trades of one batch cannot be used by a later batch (`ForgedTrade`), so a signed market intent or a close-all intent cannot be replayed. A market or close intent executes in the evaluator's own second, which is within **5 s** of the `client_time` the trader signed, either side (`ForgedTrade` outside); a limit or stop intent is placed before it fills (`client_time` at or before the fill).

Journal link: a `Fill` or `Close` event's `intent_id` is the intent's nonce. `intent_id == 0` is valid only for closes the evaluator originates (reasons StopLoss, TakeProfit, Breach, Fail); Manual and Flip closes and every Fill need an intent; the evaluator-originated reasons must NOT claim one. A Fill's `order_kind` (Market, Limit, Stop) must match the intent kind, its market and side the intent's, its `sl`/`tp` the intent's levels; a Close's intent must be an order of the opposite side or a `close` intent of the same market; the quantities executed under one nonce within a batch add up to the intent's quantity (a flip is a Close plus a Fill under one nonce in ONE batch); the intent must be dated at or before the trade and lie inside the batch's committed intents prefix.

## 6. Conventions the replay assumes of the evaluator (normative for M10)
- Prices: `Fill` = ask (long) / bid (short) of the proof's mid with the market's half-spread; `Close` of a long = bid, of a short = ask (`engine::claims`). Every proof is a record of the archive (same market, source, timestamp, value, digest) inside the batch's committed prefix.
- **Which sample prices an event (P25b, F-08, F-12, F-21; DESIGN 4.5 "The price rule")**: a market order (`Fill{Market}`, `Close{Manual}` and `Close{Flip}` under a market or close intent) executed in second T uses the NEWEST sample of its market, in stream order, with source second at or before T (the last of several samples of one second); a triggered pending order and a stop-loss or take-profit close use the TRIGGER sample (the last triggering sample of the second); the forced closes of a Breach or Fail decision use, per market, the mark the decision used (the newest sample of that market at or before the decision's own sample). Another record, older or newer, is `PriceSampleMismatch` (two records with the same market, source timestamp, value and digest are the same price). The evaluator follows the same rule (it does not matter which of two samples of one second arrived first).
- **Forced closes (P25b, F-13, F-09).** A `Close{Breach}` or `Close{Fail}` needs a Breach or Fail decision of the archive at its second; the closes of the OTHER positions that come before a quick-limit overshoot (reason `Fail`) are justified by the `Fail{PhaseQuick}` that follows in the same second. Otherwise `ForcedCloseUnjustified`; a `Fail` of a second with such closes is also `FalseFailMarks` (the Fail rests on marks that were never simultaneous). When a breach-shaped sample and a static-fail sample fall in the SAME second, the more severe outcome, the Fail, is expected and the Breach is not (F-15).
- `Fill.margin` = `margin_micro(|position qty after|, avg_entry_after, leverage of the class)`: the margin of the WHOLE position after the fill. `Fill.sl/tp` = the intent's levels; a non-zero value replaces the position's level, zero keeps it.
- Commission: every `Fill` and `Close` whose class has `commission_e6 > 0` is followed (same time) by `Fee{Commission}` with `fee_ceil_micro(margin_micro(qty, price, 1), rate)`; none when the rate is 0, and NONE after the closing slice that pushes the phase quick tally over its limit (DESIGN I-8: only the `Fail` may follow it; M9b decision D2, section 8).
- Holding fee: one `Fee{Holding}` per night per position held at the 22:00 UTC rollover R, `time == R`, amount `holding_fee_micro(|qty|, mark, rate, 1)` with the mark = the bid (long) or ask (short) of the LAST archive sample of the market with source second <= R. It must appear before any event dated after R; events dated exactly R may come before or after it. A position opened at or after R is not charged for R. A fee for a position that does not exist, at the wrong time or market, or twice, is a `FeeMismatch`.
- Time: an event's `time` is the source second of the sample that caused it; event times never decrease along the chain. **`Meta` is an event for this rule (P25b, F-26):** a `Meta` dated before the previous event, or an event dated before the previous batch's `Meta`, is `StateMismatch` ("event time goes backwards"), exactly as the program refuses it (`TimeBackwards`).
- Meta: `Meta.time` is the time of the last sample the evaluator processed for the batch. The replay judges the archive up to the largest event or Meta time seen (and up to the `time` of a `pass_requested` line); an event the samples call for in an EARLIER second that the journal lacks is a miss.

## 7. What the archive walk expects (stage 2; the evaluator must act exactly like this)
For each sample, in stream order, of a market that is OPEN (inside a session window AND a price change within `max_age_secs`, `journal_types::market_open`, with the last price CHANGE as the feed time; DESIGN 3.4, decision 49):
1. the account state is the one the journal left BEFORE the first event of the sample's second; the marks are the latest sample of every market with an open position (longs at bid, shorts at ask, half-spread from the rules file);
2. `engine::evaluate_sample` with `E = b + P - estimated closing commission` (commission ceil on `margin_micro(|q|, mark, 1)`) gives TightStart, FloorRaised, Breach or Fail; touching counts (`<=`), P exactly 0.01 S is not tight mode, `P = 0.01 S + 1 micro` is;
3. unless the decision is a Breach or Fail: `check_protective` (stop-loss before take-profit) for the position of the sample's market, then `check_pending` for the live limit/stop orders of the market (not while locked).
A closed market (or one whose price has not changed for `max_age_secs`) triggers nothing; its mark stays frozen for the other markets' equity.
Each decision becomes an expectation dated with the sample's second; the journal events of that second consume them: TightStart/FloorRaised/Breach (kind match; TightStart and FloorRaised numbers `p_open`, `g_peak`, `l_level` must equal the samples'), Close with reason StopLoss/TakeProfit (market and kind), Fill with order kind Limit/Stop (market and intent nonce) or a Close with reason Manual/Flip under that nonce (section 8, D1). Unsupported events are `FalseTightStart`, `FalseFloorRaise`, `FalseBreach`, `TriggerFalse`; expectations left over when the walk passes their second are `MissedTightStart`, `MissedFloorRaise`, `MissedBreach`, `MissedFail`, `TriggerMissed`.
- An evaluator acts at most once per second and kind; if several samples of one second give the same kind of decision, the LAST one's numbers are expected. An action changes the state for the NEXT second.
- Intents (pending orders, cancels, set_sl_tp) are effective from the first sample whose second is STRICTLY greater than their `client_time`; a set_sl_tp only applies to a position that was already open at its `client_time`; a filled or cancelled order is never expected again.
- A quick tally above 0.028 S must be followed immediately by `Fail{PhaseQuick}` (only `Meta` may come between; a commission `Fee` there is tolerated by this rule but is itself a `FeeMismatch`), else `MissedFail`.
- A pass line (`pass_requested`, `pass_confirmed`) is judged against the replay at its sequence (reasons: `journal_behind`, `seq_mismatch`, `hash_mismatch`, `phase_mismatch`, `b_mismatch`, `target_not_reached`, `not_flat`, `phase_failed`, `amount_mismatch`, `history_invalid`).
- Second recorder (`--second-archive`): the same walk on the second archive; its stage-2 findings that the primary run did not produce (same class and same expectation) are added with `source: "second"`; samples with the same market and source timestamp whose values differ by more than 0 e12 units are `ArchiveDisagreement`.

- **Other checks added by P25b.** `opened.size_s` must be one of the rules file's `tier_sizes` (`StateMismatch`, F-20). The second recorder compares the e18 `quantized_value`s exactly (`ArchiveDisagreement`, no floor to e12, tolerance 0 e18 units, F-22). The notional cap is compared on the exact product `qty x price` against `max_notional_usd` (the JSON key keeps the program's field name; the value is MICRO-USD; `max_notional_micro` is accepted as a synonym; F-35, F-37), by the verifier and the evaluator alike. A hostile intents file is parsed in linear time (F-23).
- **The verdict says whether it is anchored (P25b, F-36).** `account` without `--snapshot` prints `VERDICT: VALID (NOT ANCHORED TO THE CHAIN: ...)` in the same line; `watch` prints `VALID seq=N NOT ANCHORED TO THE CHAIN` (it replays a prefix and never has a snapshot). With `--snapshot` of the whole log the report is anchored (`"anchored": true` in the JSON). A MISMATCH needs no such note.

## 8. M9b: the rules that resolve the five findings of M10 (decision 53)
* **D1 pending orders that only reduce.** A pending (limit or stop) order that meets a position of the other side executes as the netting says: a `Close` for what it reduces (reason `Manual`, `intent_id` = the order's nonce) and, if it is larger than the position, a `Fill` of kind Limit/Stop under the same nonce for the remainder (the closing part of that flip carries reason `Flip`). A `Close` with reason `Manual` or `Flip` under the nonce of a pending order of its market is the execution of that order: it consumes the order's trigger expectation exactly like the `Fill` does. The stop order uses `Manual` too (there is no other reason). Still checked: a trigger with neither a `Fill` nor such a `Close` is `TriggerMissed`; a second execution, or one at a second no sample triggers, is `TriggerFalse`; the quantities under the nonce must add up to the order's quantity and the price/pnl claims are the usual ones (`ForgedTrade`, `PriceMismatch`, `PnlMismatch`).
* **D2 the quick-limit overshoot.** The closing slice after which the phase quick tally is over 0.028 S carries no commission `Fee` and is followed directly by `Fail{PhaseQuick}`. A flip whose CLOSING slice overshoots never executes its opening remainder: the trades under its intent end at the `Fail` (no `ForgedTrade` for the missing quantity). A `Fee` after the overshooting slice is a `FeeMismatch` (and the program refuses it, `QuickLimitExceeded`).
* **D3 margin of a pending fill.** For every pending order the samples of a second trigger, the verifier computes the evaluator's margin check itself (`b' + P' - closing commission of the positions left - this fill's commission >= sum of margins`, with the order netted into its market's position and the trigger sample as that market's mark), in the evaluator's order: the second's stop-loss and take-profit closes first, then the orders by (market, nonce), each against the account the earlier ones left. An order that fails the check stays pending and NO event is expected for it (the next trigger is judged afresh). An order that passes is expected to execute (`TriggerMissed` otherwise). Executing an order that failed the check is the new class `PendingFillWithoutMargin`. Not checked here (the evaluator may refuse for them, the verifier cannot see them): an account hold, a market that is reduce-only, the cap on positions or notional.
* **D4 what a breach turns into.** The verifier books the forced closes of a breach sample with the engine (the slice that adds least to the quick tally first, commission after every slice that does not overshoot) and expects: `Fail{StaticFloor}` when the resulting B is at or below the static floor; else `Fail{PhaseQuick}` when the closes pushed the quick tally over its limit; else `Breach`. The program accepts either `Fail` reason when both hold (`report_fail` checks the reason it is given against its own condition); the verifier names the static floor first, as `check_breach` does, and accepts whichever reason is justified by the replayed state. A posted `Breach` where a `Fail` is expected is `FalseBreach` (and `FloorMismatch` for `BreachBelowStaticFloor`); an omitted `Fail` is `MissedFail`.
* **D5 `phase_advanced` after `pass_finalized`.** A `phase_advanced` line that directly follows a `pass_finalized` line (the previous line of the file) with the same new phase number confirms it and changes nothing. Any other `phase_advanced` is the phase-1 confirmation: it needs an unresolved `pass_requested` line of its `from_phase` (a later `pass_rejected`, `pass_vetoed`, `pass_finalized` or `phase_advanced` resolves it), else `StateMismatch`; a phase advance to a wrong phase, a duplicate, or one with another line in between is `StateMismatch` as before. An indexer may therefore mirror both events of a finalization.

## 9. M9c: the evaluator's own refusals, recomputed (decision 59)
The evaluator refuses an order that opens or adds exposure for a list of reasons and leaves NO event for it. A verifier that does not know the reasons cannot tell a legitimate refusal from a silent drop, and one that ignores them cannot tell an execution the rules forbid. Since M9c the verifier recomputes every reason it has the data for, in the evaluator's order (`walk.rs::open_refusal`):

| # | reason | data | verifier |
|---|---|---|---|
| 0 | the account is held | `account_held` / `hold_released` lines (section 1) | from the second of the hold line up to (excluding) the second of the release line |
| 1 | the 12 h breach lock | the replayed `lock_until` | `engine::can_increase_exposure(state, second)`; for market orders only (a pending order is not triggered while locked: the walk leaves it out) |
| 2 | the market is disabled or reduce-only | `rules.json` `status` (1 = enabled) | any other status takes no new exposure; reducing and closing are fine |
| 3 | quantity | `qty_step`, `qty_min` | a market order: the whole quantity, and the remainder of a flip; a triggered pending order: the remainder of a flip (the order itself was validated when it was placed); a `Close` the trader named a quantity for |
| 4 | too many positions | the replayed positions | a new market while 8 positions are open (`MAX_POSITIONS`, the evaluator's cap; the program has none) |
| 5 | notional cap | `max_notional_usd`, compared with the notional in MICRO-USD as the evaluator does | the position the order leaves behind |
| 6 | margin | the engine's `margin_micro`, the walk's marks | `b' + P' - closing commission of the positions left - this fill's commission >= sum of margins`; the order is netted into its market's position (a flip credits its closing slice first), the order's own sample is its market's mark, the others use the latest sample at or before the second |
| - | the market is closed | sessions and the sample's last-change time | outside the session windows, or no price change within `max_age_secs` as of the order's sample (decision 49) |

* **A market order** is judged when its first event arrives, with the account as the journal has left it before that event (every change of state is an event, so this is the state the evaluator had): the `Fill` of a plain open or add; for a flip the `Close{Flip}` (the evaluator checks the whole order before it books the closing slice; the `Fill` that follows is the other half and is not judged again); a `Close{Manual}` under a market or close intent only needs the market open and the quantity named. A margin shortfall is `FillWithoutMargin`; every other reason, and a closed market, is `FillNotAllowed` (the finding names the reason).
* **A triggered pending order** is judged when its second is complete (section 8, D3), with the same function: a refusal for ANY of the reasons above means it stays pending and no event is expected (`TriggerMissed` is for an order that the rules let fill and that did not). Executing a refused one is `PendingFillWithoutMargin` for margin and `FillNotAllowed` for the others.
* **Unseen.** The verifier does not know why a market order has no fill. The evaluator may refuse it at arrival or at execution for reasons that depend on the moment (the newest sample older than `max_age_secs`, an invalid stop or take-profit level, the order sample it saw), so a signed market intent without any event is not a finding (trust matrix NEITHER row "a trader's market order ignored"). Nor does it know the cap on pending orders (a refused placement leaves no intent line).

## 10. Finding classes added after the audit (P25b, decision 68)
Appended to the end of the class list so that the earlier classes keep their order: `ArchiveRecordInvalid` (a poison archive record, skipped), `PriceSampleMismatch` (an event priced with another sample than the price rule names), `ForcedCloseUnjustified` (a Breach/Fail close no decision supports), `FalseFailMarks` (a Fail resting on such closes). The second recorder adds these to the classes it can report (`source: "second"`). The log lines `pass_cancelled`, `account_abandoned`, `market_feed_changed` and `market_status_changed` are new in the same version 1 of the format: a log without them is judged as before.

## 11. Mock proofs (P23 finding D-03, left strict on purpose, P27)
The verifier does not know proof source 255 (mock): an account traded with mock proofs is never VALID (`ProofShape`), and it stays that way, so a mock proof can never read VALID in production; the devnet rehearsal trades its verification-grade accounts with the markets' real source format (2) on the localnet build and keeps mock proofs for the shapes it only measures (`handover/cu-runtime.md`).
