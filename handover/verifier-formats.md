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
Any other `type` is an `UnknownEvent` finding. A malformed line (not JSON, wrong field type, bad hex) is an input error (exit code 2). All lines must carry the account of the `opened` line.

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

Journal link: a `Fill` or `Close` event's `intent_id` is the intent's nonce. `intent_id == 0` is valid only for closes the evaluator originates (reasons StopLoss, TakeProfit, Breach, Fail); Manual and Flip closes and every Fill need an intent; the evaluator-originated reasons must NOT claim one. A Fill's `order_kind` (Market, Limit, Stop) must match the intent kind, its market and side the intent's, its `sl`/`tp` the intent's levels; a Close's intent must be an order of the opposite side or a `close` intent of the same market; the quantities executed under one nonce within a batch add up to the intent's quantity (a flip is a Close plus a Fill under one nonce in ONE batch); the intent must be dated at or before the trade and lie inside the batch's committed intents prefix.

## 6. Conventions the replay assumes of the evaluator (normative for M10)
- Prices: `Fill` = ask (long) / bid (short) of the proof's mid with the market's half-spread; `Close` of a long = bid, of a short = ask (`engine::claims`). Every proof is a record of the archive (same market, source, timestamp, value, digest) inside the batch's committed prefix.
- `Fill.margin` = `margin_micro(|position qty after|, avg_entry_after, leverage of the class)`: the margin of the WHOLE position after the fill. `Fill.sl/tp` = the intent's levels; a non-zero value replaces the position's level, zero keeps it.
- Commission: every `Fill` and `Close` whose class has `commission_e6 > 0` is followed (same time) by `Fee{Commission}` with `fee_ceil_micro(margin_micro(qty, price, 1), rate)`; none when the rate is 0, and NONE after the closing slice that pushes the phase quick tally over its limit (DESIGN I-8: only the `Fail` may follow it; M9b decision D2, section 8).
- Holding fee: one `Fee{Holding}` per night per position held at the 22:00 UTC rollover R, `time == R`, amount `holding_fee_micro(|qty|, mark, rate, 1)` with the mark = the bid (long) or ask (short) of the LAST archive sample of the market with source second <= R. It must appear before any event dated after R; events dated exactly R may come before or after it. A position opened at or after R is not charged for R. A fee for a position that does not exist, at the wrong time or market, or twice, is a `FeeMismatch`.
- Time: an event's `time` is the source second of the sample that caused it; event times never decrease along the chain.
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

## 8. M9b: the rules that resolve the five findings of M10 (decision 53)
* **D1 pending orders that only reduce.** A pending (limit or stop) order that meets a position of the other side executes as the netting says: a `Close` for what it reduces (reason `Manual`, `intent_id` = the order's nonce) and, if it is larger than the position, a `Fill` of kind Limit/Stop under the same nonce for the remainder (the closing part of that flip carries reason `Flip`). A `Close` with reason `Manual` or `Flip` under the nonce of a pending order of its market is the execution of that order: it consumes the order's trigger expectation exactly like the `Fill` does. The stop order uses `Manual` too (there is no other reason). Still checked: a trigger with neither a `Fill` nor such a `Close` is `TriggerMissed`; a second execution, or one at a second no sample triggers, is `TriggerFalse`; the quantities under the nonce must add up to the order's quantity and the price/pnl claims are the usual ones (`ForgedTrade`, `PriceMismatch`, `PnlMismatch`).
* **D2 the quick-limit overshoot.** The closing slice after which the phase quick tally is over 0.028 S carries no commission `Fee` and is followed directly by `Fail{PhaseQuick}`. A flip whose CLOSING slice overshoots never executes its opening remainder: the trades under its intent end at the `Fail` (no `ForgedTrade` for the missing quantity). A `Fee` after the overshooting slice is a `FeeMismatch` (and the program refuses it, `QuickLimitExceeded`).
* **D3 margin of a pending fill.** For every pending order the samples of a second trigger, the verifier computes the evaluator's margin check itself (`b' + P' - closing commission of the positions left - this fill's commission >= sum of margins`, with the order netted into its market's position and the trigger sample as that market's mark), in the evaluator's order: the second's stop-loss and take-profit closes first, then the orders by (market, nonce), each against the account the earlier ones left. An order that fails the check stays pending and NO event is expected for it (the next trigger is judged afresh). An order that passes is expected to execute (`TriggerMissed` otherwise). Executing an order that failed the check is the new class `PendingFillWithoutMargin`. Not checked here (the evaluator may refuse for them, the verifier cannot see them): an account hold, a market that is reduce-only, the cap on positions or notional.
* **D4 what a breach turns into.** The verifier books the forced closes of a breach sample with the engine (the slice that adds least to the quick tally first, commission after every slice that does not overshoot) and expects: `Fail{StaticFloor}` when the resulting B is at or below the static floor; else `Fail{PhaseQuick}` when the closes pushed the quick tally over its limit; else `Breach`. The program accepts either `Fail` reason when both hold (`report_fail` checks the reason it is given against its own condition); the verifier names the static floor first, as `check_breach` does, and accepts whichever reason is justified by the replayed state. A posted `Breach` where a `Fail` is expected is `FalseBreach` (and `FloorMismatch` for `BreachBelowStaticFloor`); an omitted `Fail` is `MissedFail`.
* **D5 `phase_advanced` after `pass_finalized`.** A `phase_advanced` line that directly follows a `pass_finalized` line (the previous line of the file) with the same new phase number confirms it and changes nothing. Any other `phase_advanced` is the phase-1 confirmation: it needs an unresolved `pass_requested` line of its `from_phase` (a later `pass_rejected`, `pass_vetoed`, `pass_finalized` or `phase_advanced` resolves it), else `StateMismatch`; a phase advance to a wrong phase, a duplicate, or one with another line in between is `StateMismatch` as before. An indexer may therefore mirror both events of a finalization.
