# Module M2: journal types, hash chain, price proofs (DONE, verified 2026-10-10)

Crate `crates/journal-types` (package `setl8-lev-journal-types`, lib `journal_types`). Touched nothing outside `crates/journal-types`, `scripts/gen-hash-vectors.py`, `Cargo.lock` and this file. `crates/engine`, `programs/` and `tests-rs/` are untouched.

## What was built
- `types.rs`: `JournalBatch`, `JournalEvent` (9 variants, DESIGN 8.2 order and widths), `PriceProof`. `proof_ref()` (raw field), `proof_index()` (resolved), `time()`.
- `enums.rs`: `Side`, `OrderKind`, `CloseReason`, `FeeKind`, `FailReason` (`#[repr(u8)]`, `TryFrom<u8>` rejects unknown values).
- `codec.rs`: hand-written canonical Borsh (no `borsh` dependency). Encode is infallible for events, fallible (`EncodeError`) for proofs and batches so that everything that encodes also decodes. Decode is strict and panic-free.
- `hash.rs`: `Sha256` trait (`hashv(&[&[u8]])`, the shape of `sol_sha256`; closures implement it), `NativeSha256` (feature `native-sha2`), `genesis_hash`, `proof_digest`, `next_hash`, `apply_batch`, `ChainEntry`/`verify_chain`/`ChainBreak`, `feed_id`.
- `proof.rs`: `price_e12_from_quantized`, `proof_time_secs`, `check_proof`, source constants.
- `session.rs`: `minute_of_week`, `in_session`, `market_open`.
- `scripts/gen-hash-vectors.py` writes `crates/journal-types/tests/vectors.json` (committed). It is an independent Python encoder (hashlib + struct), written from DESIGN 8.2/8.3.

## Byte layouts (all little endian; fixed arrays raw; Vec = u32 length + items; bool = 0/1)
`JournalBatch`: first_seq u64 | prev_hash [32] | events (u32 count 1..=8, then events) | proofs (u32 count 0..=3, then proofs). Total fixed part 8+32+4+4 = 48 bytes.

| Event (tag) | fields in order (width) | encoded bytes |
|---|---|---|
| Fill (0) | market u16, side u8, order_kind u8, qty u64, price_e12 u64, time i64, avg_entry_after u64, margin u64, sl u64, tp u64, intent_id u32, proof u8 | 66 |
| Close (1) | market u16, side u8, qty u64, entry_e12 u64, exit_e12 u64, time i64, timer_start i64, pnl i64, quick u8, credited i64, reason u8, intent_id u32, proof u8 | 67 |
| Fee (2) | kind u8, market u16, amount u64, time i64 | 20 |
| TightStart (3) | time, p_open, r, l_start, g_peak, l_level (6 x i64), proof u8 | 50 |
| FloorRaised (4) | time, g_peak, l_level (3 x i64), proof u8 | 26 |
| TightEnd (5) | time i64 | 9 |
| Breach (6) | time, l_level, b_after (3 x i64), proof u8 | 26 |
| Fail (7) | time i64, reason u8, b_after i64, proof u8 | 19 |
| Meta (8) | time i64, flat u8, samples_hash [32], intents_hash [32], engine_version u16 | 76 |

`PriceProof`: source u8 | feed_id [32] | timestamp_ns u64 | quantized_value i128 | signed_update (u32 length + bytes, at most 512). Fixed part 61 bytes; a source-2 or source-4 proof is exactly 61 bytes, a source-3 proof is 61 + the size of the signed report (up to 573).

| enum | values |
|---|---|
| order_kind | Market 0, Limit 1, Stop 2 |
| side | Long 0, Short 1 |
| Close reason | Manual 0, StopLoss 1, TakeProfit 2, Flip 3, Breach 4, Fail 5 |
| Fee kind | Holding 0, Commission 1 |
| Fail reason | StaticFloor 0, PhaseQuick 1 |
| PriceProof source | Stork 0 (not built), Pyth 1 (not built), GMX tickers 2, GMX signed 3, Hyperliquid 4, mock 255 |
| proof field | index into batch proofs, `NO_PROOF` = 255 |

## Public API
Types: `JournalBatch`, `JournalEvent`, `PriceProof`, `ChainEntry`, `ChainBreak`, `ChainBreakKind`, enums above, `UnknownValue`, errors `DecodeError`, `EncodeError`, `ChainError`, `ProofError`.
Constants: `MAX_EVENTS_PER_BATCH = 8`, `MAX_PROOFS_PER_BATCH = 3`, `MAX_SIGNED_UPDATE_BYTES = 512`, `NO_PROOF = 255`, `MINUTES_PER_WEEK = 10080`, `MAX_PROOF_FUTURE_SECS = 2`, `SOURCE_*`, `GENESIS_PREFIX`, `EVENT_PREFIX`, `FEED_PREFIX`.
Functions: `JournalBatch::{decode, to_bytes, validate}`, `JournalEvent::{decode, to_bytes, encode_into, proof_ref, proof_index, time}`, `PriceProof::{decode, to_bytes, encode_into}`, `genesis_hash(h, pubkey)`, `proof_digest(h, proof)`, `next_hash(h, prev, seq, event, proof)`, `apply_batch(h, prev, first_seq, batch)`, `verify_chain(h, start, first_seq, entries)`, `feed_id(h, source, symbol)`, `price_e12_from_quantized`, `proof_time_secs`, `check_proof(proof, market_feed_id, max_age_secs, event_time, event_price_e12, allow_mock)`, `minute_of_week`, `in_session`, `market_open`.

## Hashing and features
`default = ["native-sha2"]`. The Solana program must depend on the crate with `default-features = false` and pass `sol_sha256` through the `Sha256` trait (a closure `|parts: &[&[u8]]| -> [u8; 32]` works), so no hashing code is linked into the .so. The evaluator, verifier, WASM build and the tests use the default. The crate builds and is clippy-clean with `--no-default-features`.

## Tests
- Unit 1 (limits). Integration 71 plus 5 proptest properties (512 cases each) = 77 total.
- vectors (11): genesis hash; every event encodes to / decodes from the Python bytes; no-proof variants; proofs for sources 2, 3, 4, 255 (bytes, decode, digest); full 3-event batch; `next_hash` for every event with and without a proof; 3-event chain via `apply_batch` and `verify_chain`; feed ids (BTC/2, xyz:AAPL/4, EURUSD/0); feed id depends on source and symbol; closure hasher equals native hasher.
- decode (18 + 5 proptest): round trips (events, proofs, batch); tags in declaration order; encoded lengths equal the design widths; enum tables; rejection of trailing bytes, event count 0 / 9 (8 accepted), proof count 4 (3 accepted), `signed_update` 513 (512 accepted), bad proof indexes (255 ok, index == len rejected), unknown enum values and tags, non-0/1 booleans, length prefixes larger than the input (`u32::MAX` events, proofs, `signed_update`), every truncation of a valid batch; proof helper accessors; property tests: random events/proofs/batches round-trip, random bytes never panic, mutated valid batches never panic and anything that decodes re-encodes to the same bytes.
- chain (17): honest chain; empty chain; tamper tests (insert, drop middle / first / last, swap, alter event byte and each Fill field, alter seq, alter proof fields, remove/add proof, change prev_hash, wrong recorded hash) each check the first break index and that the recomputed head changes; seq 0 and overflow; proof/event consistency; `apply_batch` errors; two batches chain like one.
- proof (12) and session (13): wrong feed, price +/-1, exactly max_age old vs one second older, +2 s vs +3 s, sub-second flooring, unknown sources (0, 1, 5, 100, 254), mock with and without `allow_mock`, 512 vs 513, check order, value conversion boundaries; Monday 00:00 = minute 0, Sunday 23:59 = 10079, epoch is Thursday, negative times, window start inclusive / end exclusive, zero sessions closed, stale feed closed, no wrap.

## Mutation gate
`cd crates/journal-types && cargo mutants --no-shuffle` (cargo-mutants 27.1.0, installed with `cargo install`): **187 mutants, 179 caught, 8 unviable, 0 missed, 0 timeouts: 100% of the 179 viable mutants killed** (gate: 95%). Unviable (do not compile, so nothing to kill): `Ok(Default::default())` replacements of `JournalEvent::decode`, `PriceProof::decode`, `JournalBatch::decode`, `Reader::side`, `Reader::event`, `Reader::proof`; `enum_err -> Default::default()`; `From<EncodeError> for ChainError -> Default::default()`. Survivors: none. (The run was made before two cosmetic test-only clippy fixes.)

## Checks
`scripts/test-engine.sh` passes. `cargo clippy --offline -p setl8-lev-journal-types -- -D warnings` passes (also `--all-targets`, and `--no-default-features`). `cargo check --target wasm32-unknown-unknown` was NOT run: the target is not installed (`rustup target list --installed` shows only aarch64-apple-darwin) and installing toolchain components was not part of the task. The crate has no std::time, floats or unsafe and only `alloc`-style collections, so it is expected to build for WASM.

## Decisions (design silent)
1. Hand-written Borsh instead of the `borsh` crate: the decoder checks every length before allocating and the wire format is pinned by independently generated vectors.
2. Hashing is a trait (`hashv(parts)`), closures implement it; `native-sha2` is on by default, the program opts out.
3. `quick` and `flat` are `bool` in Rust; decode rejects any byte other than 0 or 1 (stricter than raw `u8`). Encoding writes 0/1.
4. `PriceProof.source` is a plain `u8` and decode accepts any value; policy is in `check_proof` (supported: 2, 3, 4, and 255 with `allow_mock`; 0 and 1 are `UnknownSource` because they are "not built").
5. Proofs that no event references are allowed in a batch. Events pointing at the same proof are allowed.
6. `next_hash`/`verify_chain` demand a proof exactly when the event's `proof` field is not 255 (`ProofMismatch` / `BadProof` otherwise): a proof cannot be smuggled in or left out.
7. `verify_chain` needs the recorded head after each event (`ChainEntry.hash`) to localize an altered event; without recorded heads only the final head can be compared. Dropping the LAST entry leaves a valid shorter chain, so the caller must compare the returned head with the account's `journal_hash`.
8. `apply_batch` takes the stored head as `prev_hash` and does not compare it with `batch.prev_hash` (the program does that in I-2 and returns its own error).
9. `check_proof` takes `max_age_secs: u32` (the Market field is u16, which widens). Check order: source, `signed_update` length, feed, price, too old, in the future. Sources 2 and 4 with a non-empty `signed_update` are not rejected (no error variant in the spec).
10. `market_open`: fresh means `now - max_age <= publish <= now + 2` (same 2 s skew as proofs), inclusive. `in_session` with `session_count` above the slice length only looks at the slots that exist.
11. `price_e12_from_quantized`: a positive value that floors to 0 (below 10^6) is `ValueNotPositive` (a zero price is never valid).
12. All time arithmetic is done in `i128` with saturating operations (clippy `arithmetic_side_effects` is denied in non-test code, together with unwrap/expect/panic/indexing/casts).

## Findings for later modules
- Sizes are now measured, not estimated: events are 9 to 76 bytes (Fill 66, Close 67), far below the "90 to 110" in DESIGN 8.2; an unsigned proof is 61 bytes, not "about 300". Only source 3 (GMX signed Chainlink blob, up to 512 bytes of `signed_update`) approaches the old estimate, and a full batch of 3 such proofs (up to 3 x 573 bytes) does not fit a 1,232-byte transaction. The probe (P5) must measure the real report size; M11 should size batches from these numbers.
- No on-chain signature check exists for any source in v1 (DESIGN 3.3, decision 46).
