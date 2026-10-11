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
| `phase_advanced` | account, from_phase, to_phase, time | `PhaseAdvanced` |
| `pass_finalized` | account, phase, to_phase (0 = graduated), amount, request_id, tally_count, tally_total, window_total, time | `PassFinalized` |
| `account_failed` | account, reason, b, seq, journal_hash | `AccountFailed` |
| `violation_fail` | account, trader, evidence_hash | `ViolationFail` |
Any other `type` is an `UnknownEvent` finding. A malformed line (not JSON, wrong field type, bad hex) is an input error (exit code 2). All lines must carry the account of the `opened` line.

## 2. Account snapshot (`snapshot.json`, optional)
`{"version":1,"b":..,"q_tally":..,"latch":0|1,"tight_on":0|1,"r":..,"g_peak":..,"l_level":..,"lock_until":..,"seq":..,"journal_hash":hex,"flat":0|1,"phase":..,"status":..,"trades_closed":..}` = the stored `TradingAccount` fields (DESIGN 5.1). Compared with the replay at the end.

## 3. Rules file (`rules.json`)
`{"version":1,"ruleset_version":u16,"fee_bps":u16,"leverage_x":[8 x u16],"holding_fee_e6":[8 x u32],"commission_e6":[8 x u32],"tier_sizes":[u64..],"spread_e6":{"<market id>":u16,..},"markets":[{"id","class","feed_id":hex32,"max_age_secs","status","qty_step","qty_min","max_notional_usd","sessions":[[start_min,end_min],..],"quote_conv_market"?}]}`.
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
- Commission: every `Fill` and `Close` whose class has `commission_e6 > 0` is followed (same time) by `Fee{Commission}` with `fee_ceil_micro(margin_micro(qty, price, 1), rate)`; none when the rate is 0.
- Holding fee: one `Fee{Holding}` per night per position held at the 22:00 UTC rollover R, `time == R`, amount `holding_fee_micro(|qty|, mark, rate, 1)` with the mark = the bid (long) or ask (short) of the LAST archive sample of the market with source second <= R. It must appear before any event dated after R; events dated exactly R may come before or after it. A position opened at or after R is not charged for R. A fee for a position that does not exist, at the wrong time or market, or twice, is a `FeeMismatch`.
- Time: an event's `time` is the source second of the sample that caused it; event times never decrease along the chain.
