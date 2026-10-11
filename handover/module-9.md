# Module M9: the public verifier `setl8-verify` (DONE, verified 2026-10-12)

Crate `crates/verifier` (package `setl8-lev-verifier`, library `setl8_verify` + binary `setl8-verify`), in the root workspace. Formats and the evaluator conventions it assumes: `handover/verifier-formats.md`. Two commits: stage 1 (formats, chain, event replay, intents) and stage 2 (archive walk, second recorder, pass verdicts, `watch`). The program, tests-rs, engine and journal-types were NOT touched. Dependencies (CLI shell and parsing only): `serde`, `serde_json`, `bs58`, `sha2`, `ed25519-dalek 1.0` (the only version in the offline registry); `proptest` for tests. The replay core is pure (no I/O, no clock).

## CLI
```
setl8-verify account --events E.jsonl --rules R.json --archive A.bin --intents I.jsonl [--snapshot S.json] [--second-archive B.bin] [--json] [--as-of-seq N]
setl8-verify watch   --events E.jsonl --rules R.json --archive A.bin --intents I.jsonl [--second-archive B.bin]
```
Exit codes: 0 valid, 1 mismatch, 2 input error (missing file, wrong format, bad arguments). Human text by default (`VERDICT`, the replayed state, one line per finding, per-phase and per-pass lines), stable JSON with `--json` (fields in declaration order, findings sorted by (seq, event index, class, message); no timestamps). `--as-of-seq N` replays only the events up to sequence N (the watchdog use of DESIGN 4.6). `watch` prints one line for the newest pending pass: `VALID seq=N`, `MISMATCH seq=N classes=A,B` or `NO_PENDING_PASS` (a pass is pending until it is rejected, vetoed, finalized or ended by a fail); no network.

## Library
`verify_account(&Inputs) -> Result<Report, InputError>`; `Report { verdict: Valid | Mismatch, findings (at most 1,000, then `truncated`), per_phase: [PhaseVerdict], per_pass: [PassVerdict], state: StateSummary }`; `Finding { class, seq, event_index, expected, found, message, source }`. The replay (`replay.rs`) rebuilds positions and the `AccountState` event by event with `engine::apply_fill`, `check_close`, `apply_closed_slice`, `check_tight_start`/`check_floor_raised`/`check_breach`/`check_tight_end`, the cost functions and `engine::claims` prices; `walk.rs` is the stage-2 archive walk.

## Finding classes
| class | meaning |
|---|---|
| HashChain | the replayed chain head differs from the stored `journal_hash` |
| SequenceGap | an event line's sequence is not the previous + 1 (dropped, repeated, reordered), or the stored account is at another sequence |
| BadGenesis | `opened.genesis_hash` is not `sha256("SL8LEV" ++ account)` |
| EventDecode | event or proof bytes do not decode, or the engine refuses the event |
| UnknownEvent | a log line of an unknown `type` |
| ProofFeed | the proof's feed is not the market's feed (or belongs to no market) |
| ProofAge | the proof is outside `event time - max_age ..= event time + 2` |
| ProofShape | `signed_update` shape wrong for the source, bad value, a proof missing where required or present where not referenced |
| PriceMismatch | the event price differs from the proof mid with the half-spread (`engine::claims`) |
| ArchiveMissing | a proof is not a record of the archive (market, source, timestamp, value, digest) or lies outside the batch's committed prefix; no mark for a holding fee |
| SamplesHashMismatch | `Meta.samples_hash` matches no record boundary, or the committed prefix shrank |
| IntentsHashMismatch | same for `intents_hash` and line boundaries |
| ForgedTrade | no matching signed intent (absent, other market/side/kind/levels/quantity, dated after the trade, outside the committed prefix, wrong origin for the close reason) |
| BadIntentSignature | the ed25519 signature is not the trader's over `SL8LEV | account | nonce | order` |
| IntentDecode | an intent line is unusable (not JSON, bad hex, bad order bytes, nonce 0, duplicate nonce) |
| UnknownMarket | the event names a market the rules do not list |
| PositionMismatch | Close without a position, on the other side, or larger than the position; a Fill that reduces |
| PnlMismatch / CreditMismatch / QuickMismatch | `check_close` against the REPLAYED position (timer and quick flag included) |
| FeeMismatch | holding or commission fee wrong, missing, unexpected (flat day, wrong time or market) or too many nights between two events to check |
| MarginMismatch | `Fill.margin` is not the margin of the whole position |
| AvgEntryMismatch | `avg_entry_after` or a Close's `entry_e12` differs from the replayed average |
| FloorMismatch | TightStart/FloorRaised/Breach/TightEnd claims fail the engine formulas; breach with open positions; missing TightEnd |
| StateMismatch | snapshot field differs; `Meta.flat` disagrees; time goes backwards; events after the account ended; second `opened`; log ends inside a batch |
| EngineVersion | `Meta.engine_version` is not the engine's |
| PassInvalid | a pass request/confirmation or a phase change that the replay does not support (reasons in `per_pass`) |
| FalseFail | a `Fail` the replayed state does not justify |
| MissedBreach / MissedFail / MissedTightStart / MissedFloorRaise | the samples require it, the journal has none |
| FalseBreach / FalseTightStart / FalseFloorRaise | the journal has it, the samples do not support it |
| WrongLLevel | formula-consistent TightStart/FloorRaised whose `p_open`/`g_peak`/`l_level` differ from the samples' |
| TriggerMissed / TriggerFalse | a stop-loss, take-profit, limit or stop the samples trigger and the journal lacks / the journal has and no sample triggers |
| ArchiveDisagreement | the two archives differ at the same market and source timestamp |
Classes added to the prompt's list: `ProofShape`, `IntentDecode`, `UnknownMarket`, `PositionMismatch`, `PassInvalid`, `FalseFail`, `FalseBreach`-family (`FalseTightStart`, `FalseFloorRaise`).

## Tests
Total 190: unit 45, integration 145 (cli 6, design_4_9 2, vectors 3, stage1 51, stage2 42, coverage 38) of which 3 proptest (`fuzz.rs`, 200 cases each: random bytes in every file, byte flips/truncation of valid files, flips inside the event hex). Stage 1 at its commit: 84 (unit 19, integration 65). Every test generator uses the REAL engine and journal-types (`tests/common/mod.rs`, a test-only evaluator); the committed vectors in `tests/vectors/` (`design_4_9`, `quick_profit`, `holding_night`) are its output and `tests/vectors.rs` regenerates them byte for byte (`SL8_WRITE_VECTORS=1` rewrites).
Tamper tests by class (each changes one thing and must give exactly that class; in `stage1.rs`, `stage2.rs`, `coverage.rs`): HashChain (`hash_chain_flipped_bit_in_the_stored_head`, `hash_chain_altered_event_bytes_change_the_head`), SequenceGap (`sequence_gap_dropped_event`, `_reordered_events`, `_snapshot_beyond_the_log`), BadGenesis (`bad_genesis`), EventDecode (`event_decode`), UnknownEvent (`unknown_event_type`), ProofFeed/ProofAge/ProofShape (`proof_feed_is_not_the_markets_feed`, `proof_age_is_outside_the_markets_window`, `proof_shape_signed_update_on_an_unsigned_source`, `the_proof_of_a_floor_event_is_checked_like_any_other`), PriceMismatch (`price_mismatch_*_off_by_one_unit`), ArchiveMissing (`archive_missing_proof_value_not_in_the_archive`, `archive_record_altered_after_the_commitment`, `signed_proofs_carry_the_blob_digest_and_it_must_match_the_archive`), SamplesHashMismatch/IntentsHashMismatch (`samples_hash_mismatch`, `intents_hash_mismatch`, `*_prefix_must_not_shrink`), ForgedTrade (`forged_trade_*`, `only_one_protective_level_differing_is_a_forged_trade`), BadIntentSignature (`bad_intent_signature*`), IntentDecode (`intent_decode_duplicate_nonce`), UnknownMarket (`unknown_market`), PositionMismatch (`position_mismatch_*`), PnlMismatch/CreditMismatch/QuickMismatch (`pnl_mismatch_by_one_micro`, `credit_mismatch_by_one_micro`, `quick_mismatch_flag_and_timer`, `quick_boundary_45_seconds`), FeeMismatch (`fee_mismatch_*`, `holding_fee_is_charged_at_the_rollover_and_not_before`), MarginMismatch (`margin_mismatch`), AvgEntryMismatch (`avg_entry_mismatch_and_entry_of_a_close`), FloorMismatch (`floor_mismatch_level_off_by_one_micro`, `a_wrong_floor_claim_does_not_poison_what_follows`), StateMismatch (`state_mismatch_snapshot_fields_and_meta_flat`, `all_snapshot_fields_are_compared`), EngineVersion (`engine_version`), FalseFail (`false_fail`), PassInvalid (`pass_requested_one_micro_short_is_invalid`, `pass_requested_with_the_journal_behind`, `pass_request_claims_are_compared_with_the_replay`, `the_next_phase_must_be_the_one_the_rules_name`), MissedBreach (`missed_breach_the_breach_is_removed_from_the_journal`, `missed_breach_found_only_by_the_second_archive`, `breach_boundary_equity_equals_the_tight_floor`), MissedFail (`static_floor_boundary_equity_equals_the_floor`, `missed_fail_the_quick_limit_is_exceeded_and_nothing_fails_the_account`), FalseBreach/FalseTightStart/FalseFloorRaise (`false_*_when_the_primary_archive_was_manipulated`), MissedTightStart (`tight_start_boundary_p_open_equals_one_percent`), MissedFloorRaise (`missed_floor_raise`), WrongLLevel (`wrong_l_level_numbers_consistent_with_the_formulas_but_not_with_the_samples`), TriggerMissed/TriggerFalse (`trigger_missed_stop_loss`, `trigger_missed_limit_fill`, `trigger_false_stop_loss_the_signed_level_is_lower`, `trigger_false_limit_fill_that_no_sample_triggers`), ArchiveDisagreement (`archive_disagreement_alone_and_no_disagreement_for_identical_archives`).
Also: the honest 3.5-hour two-market account (`honest_long_path_verifies_valid_and_has_every_feature`: quick trade, cancelled and filled limits, stop-loss, take-profit, two tight modes, raises, a holding-fee night, commissions, set_sl_tp, breach; 2,520 samples), DESIGN 4.9 (`design_4_9_verifies_valid_with_the_design_numbers`), boundary pairs at the verifier level (E == L, E == F_s, P == 0.01 S and each plus one micro), the closed-market rule, determinism (`stage_2_output_is_byte_identical_between_runs`, `output_is_deterministic`), the CLI exit codes (`cli.rs`), `as_of_seq`, `watch`.

## Mutation gate (`cargo mutants --no-shuffle -j 3 --timeout 150 --exclude src/main.rs`, crates/verifier)
Final full run: 635 mutants: 595 caught, 1 missed, 29 unviable, 10 timeouts. The one missed (`walk.rs Walker::push`, `==` to `!=` in the pending-order key) was fixed with a test and rechecked (all 18 mutants of `Walker::push` caught), so the gate stands at 596 caught of 606 viable (98.3 %) plus 10 timeouts. The 10 timeouts are mutants that stop the program from terminating (`archive.rs` header offset `+=` to `*=`; `next_rollover_after` returning a constant or a wrong product; the rollover loop condition; `walk_advance`/`walk_step` cursor `+=` to `*=`): a hang is a detection, none is a survivor, so all 606 viable mutants are detected (100 %). Exclusion: `src/main.rs` (one mutant: the three lines that read files and call `process::exit`; its behaviour is tested through the process in `tests/cli.rs`). Earlier runs found 67 survivors; each got a test, a refactor that removed a redundant condition (`before_event`, `holding_expectations`, the commission rate test, `is_positive` for `qty > 0`, the unused `quote_conv_market` default) or a unit test of the helper. No survivor is left unexplained.

## DESIGN edits
Two one-line notes (committed with stage 1): 4.5 (one logical record stream for all markets, prefix hashes found in one hashing pass, signed 48-byte `OrderIntent`s) and 4.6 (`setl8-verify account|watch`, exit codes).

## DECISIONS (design silent)
1. **Time model.** An event's `time` is the source second of the sample that caused it. The decision for a sample uses the account state BEFORE the first event of its second; an evaluator acts at most once per second and kind (the last sample's numbers are expected); intents are effective from the first sample STRICTLY after their `client_time`.
2. **`Meta.time` = the last sample the evaluator processed for the batch**; the walk runs to the largest event/Meta time, and to the `time` of a `pass_requested` line. A log that ends before a breach sample is not wrong until something later (a Meta, a pass request) shows the evaluator had seen that second.
3. **Holding fee**: one `Fee{Holding}` per night per position at the rollover instant 22:00:00 UTC (`time == R`), mark = bid (long) / ask (short) of the LAST sample of the market with second <= R, rate 0 means no fee. A fee dated R is applied before the samples of seconds >= R are judged. Missing fees are noticed at the first event dated after R, or at the end of the log. A gap of more than 3,660 nights between two events with positions open is reported, not walked.
4. **Commission**: a `Fee{Commission}` follows each Fill/Close of a class with a non-zero rate, amount `ceil(rate x margin_micro(qty, price, 1))`.
5. **`Fill.margin` is the margin of the whole position after the fill**; `Fill.sl/tp` equal the intent's levels (non-zero replaces, zero keeps).
6. **Intent rules**: Manual/Flip closes and all Fills need an intent; StopLoss/TakeProfit/Breach/Fail closes must carry 0; kinds must match; quantities under one nonce add up to the intent's within one batch; intent lines after the last committed prefix are not judged; a line with a bad signature is reported once (no extra `ForgedTrade` for trades that cite it).
7. **Sample archive**: one stream for all markets; headers per segment, excluded from the hash; flags byte 0 distinguishes a record from a header; samples must be in non-decreasing source second across the stream (the walk stops at the first sample later than the target).
8. **Closed market**: no floor, trigger or pending decision on a sample of a closed market; its mark stays frozen for other markets' equity.
9. **Fail**: `Fail{StaticFloor}` is justified by the replayed state (flat, `b <= floor`); the walk only reports a MISSED fail; `Fail{PhaseQuick}` must follow the overshooting close directly (`MissedFail` otherwise). Events after the account ended are `StateMismatch`, a closing `Meta` is allowed.
10. **Second recorder**: only walk classes can come from the second archive; a finding is added when the primary run has none of the same class and expectation; `ArchiveDisagreement` tolerance is 0 e12 units (two recorders of one stream see the same numbers).
11. **Pass lines**: file order = chain order; a pass line is judged at its position; `PassInvalid` is raised for any invalid pass or phase change and makes the verdict Mismatch.
12. **Not re-checked** (the program enforces them): quantity step and minimum, margin sufficiency, market status and sessions at order time, daily limits.
13. Source 255 (mock) proofs are never accepted by the verifier.
