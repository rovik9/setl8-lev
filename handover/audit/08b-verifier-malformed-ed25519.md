# Item 8, second part: malformed verifier inputs and the ed25519 audit (finding prefix VRF)

SELF-audit by the builder. Nothing here was reviewed by anyone else. No production code was changed.

Files added:

* `crates/verifier/tests/adversarial_inputs.rs` (30 tests: 27 pass, 3 ignored findings)
* `crates/evaluator/tests/adversarial_signatures.rs` (16 tests: 14 pass, 2 ignored findings)

Run with:

* `cargo test --offline -p setl8-lev-verifier --test adversarial_inputs` (about 5 s in debug)
* `cargo test --offline -p setl8-lev-evaluator --test adversarial_signatures`
* Add `-- --ignored` to see the findings fail. I ran them: all five fail for the reason stated below.
* `cargo clippy --offline -p <crate> --test <file> -- -D warnings` is clean for both files.

## 1. Every place that verifies an ed25519 signature

Searched with `grep -rnE 'ed25519|verify_strict|\.verify\(|Signature|PublicKey|VerifyingKey|SigningKey'` over `programs/`, `crates/*/src`, `crates/*/tests`, `tools/*/src`, `tests-rs/` (Rust and Cargo.toml).

| # | file:line | crate and version (from Cargo.lock) | strict? | what it checks |
|---|---|---|---|---|
| 1 | `crates/verifier/src/intents.rs:116-118` `verify_signature` (`PublicKey::from_bytes`, `Signature::from_bytes`, `pk.verify`) | ed25519-dalek 1.0.1, ed25519 1.5.3, curve25519-dalek 3.2.0 (root `Cargo.lock`) | NO: plain `Verifier::verify` | the trader's signature on every order intent in the intents file. Called from `crates/verifier/src/replay.rs:266` (`check_intents`). This is the only production verification site in the repo |
| 2 | `crates/evaluator/tests/common/mod.rs:32-34` `Ed25519Check` | same versions | NO: plain `verify` | the test harness's `IntentVerifier`, the only implementation that does real crypto. It is test code, but it is the pattern a backend would copy |
| 3 | `crates/evaluator/src/eval.rs:423` `self.verifier.verify(...)` in `accept_order` | none (trait `IntentVerifier`, eval.rs:15) | depends on the implementation injected | the call site. The core has no crypto dependency |
| 4 | `crates/evaluator/src/eval.rs:25` `AcceptAll` | none | no check at all | tests of the book only. A backend that used it would produce an intents file the verifier flags `BadIntentSignature` (test `off_chain_an_evaluator_that_skips_the_signature_check_is_flagged_by_the_verifier`) |
| 5 | `tests-rs/Cargo.lock`: `solana-signature` 2.3.0 (`verify_verbose` uses `verify_strict`), `solana-ed25519-program` 2.2.3, `ring` 0.17.14 | pulled in by litesvm and the Solana crates | `solana-signature` is strict | not our code and not on any path of this repo's checks. The LiteSVM harness runs with signature verification OFF anyway (tests-rs/tests/common/vault.rs header) |

Not present anywhere (stated so nobody assumes it exists):

* **The on-chain program verifies no ed25519 signature.** `programs/` has no ed25519, no precompile call and no instruction-sysvar read for signatures. DESIGN 3.3 and R-7 say so. The order-intent signatures are checked only by the evaluator and the verifier.
* **Source 3 (GMX signed blob) has no signature check.** `journal-types/src/proof.rs` says "the publisher signature is NOT checked here". The verifier compares only the sha256 digest of the blob with the archive (`replay.rs:340`). Test `limited_source_3_blob_digest_is_compared_but_the_blob_signature_is_not_checked_anywhere` pins this: archive digest and proof digests replaced by `0xAB..` everywhere, chain recomputed, the verifier says VALID.
* **There is no other signed-update check** (Stork, Pyth not built). Nothing to test.
* Other Cargo.lock files (`tools/session-observer`, `gmx-probe`, `hl-probe`) contain `ring` 0.17.14 only through TLS, not for signatures we check. `tools/launch-config` has no crypto crate at all. No ed25519-dalek in any tool.

Which verifier a production backend must use: `PublicKey::verify_strict` (ed25519-dalek 1.0.1 has it, public.rs:283). It rejects a small-order R or public key. Both non-strict sites above should be changed to it.

Facts measured on dalek 1.0.1, default features (no `legacy_compatibility`), pinned by tests:

* A malleated S+L is REFUSED even by plain `verify`. 1.0.1 without `legacy_compatibility` checks S < L at parse time (`check_scalar` calls `Scalar::from_canonical_bytes`). So the malleability attack is BLOCKED, not a finding. A test asserts that S+L stays below 2^253, so the legacy "top 3 bits clear" check alone would not have refused it. If anyone enables the `legacy_compatibility` feature, those tests fail.
* A small-order PUBLIC KEY is accepted by plain `verify`. With the identity point as key, R = identity and S = 0, the equation holds for any message and any nonce: no secret is involved. For the other small-order points (orders 2, 4, 8, and two non-canonical encodings) a nonce exists within 2000 tries. `verify_strict` rejects all of them.

## 2. Findings

| id | severity | where | description | fix kind |
|---|---|---|---|---|
| VRF-1 | info (harness only) | `crates/evaluator/tests/common/mod.rs:34` `Ed25519Check` | The only real `IntentVerifier` in the repo uses plain `verify`. With a small-order trader key the evaluator accepts an order with signature (R = identity, S = 0) for any nonce. Test: `finding_vrf_1_plain_verify_accepts_an_unsigned_order_for_a_small_order_trader_key`. The same file shows `StrictEd25519` (verify_strict) refusing the same forgeries and accepting honest orders. Impact: none today, because it is test code and a small-order key cannot sign `open_account`. It matters because a production backend written from this pattern would inherit the hole | plain implementation error. Use `verify_strict` and document it in the `IntentVerifier` trait docs |
| VRF-2 | low | `crates/verifier/src/intents.rs:118` | The public verifier accepts an unsigned intent when the account's trader key (from the `opened` line) has small order. Three tests: `finding_vrf_2_verify_signature_accepts_any_message_for_a_small_order_trader_key` (unit), `finding_vrf_2_the_whole_verifier_says_valid_for_unsigned_intents_of_a_small_order_trader` (whole verifier, consistent hashes: verdict VALID), and `finding_vrf_2_verifier_calls_an_unsigned_trade_valid_for_a_small_order_trader_key` in the evaluator crate (end to end through the evaluator's output). `pin_vrf_2_what_the_verifier_does_today_with_the_identity_trader` pins the current behaviour. Impact is limited because the program's `open_account` takes the trader as a `Signer`, and a Solana transaction signature is checked strictly (`solana-signature` 2.3.0 uses `verify_strict`), so no honest or dishonest trader can open an account under a small-order key. It is still wrong for a verifier: "forged trade" is the one class the verifier exists to detect, and a malicious `opened` line in a log given to a third party would hide forged trades. The `opened` line is NOT bound to the chain (the genesis hash binds only the account address), so a third party cannot rely on the trader field being the real one unless they compare it with the chain | plain implementation error: `pk.verify_strict`, and refuse a small-order trader key as an input error |
| VRF-3 | info | `crates/verifier/src/verify.rs` (parse) | `opened.ruleset_version` is never compared with `rules.json` `ruleset_version`; both are read and neither is used. A rules file of another version replays without a word. Test: `finding_vrf_3_a_rules_file_of_another_version_is_not_noticed`; the current behaviour is pinned in `pin_vrf_3_current_behaviour`. In practice a wrong rule set mostly shows up as FeeMismatch or FloorMismatch, but only if its numbers differ for what the account exercised (see section 4, rule fields). Related, not part of the test: `tier_index` and `tier_sizes` are parsed and never used (the committed vectors even have `tier_index` 8 with a one-entry `tier_sizes`) | plain implementation error for the version check (small). A tier check would need the vectors regenerated |

Observations that are NOT findings (each is pinned by a test):

* **Uncommitted tail.** Bytes in the intents file or the archive AFTER the last committed prefix (`Meta.intents_hash` / `samples_hash`) are not covered by any hash and are ignored: garbage lines, duplicated nonces, extra samples, a whole second copy of the archive, invalid UTF-8, all stay VALID. This is by design (the files grow after the last batch). Nothing a committed event refers to can be changed this way. LIMITED.
* **Missing tail of the log.** A log cut at a line boundary is a valid shorter history unless the snapshot or the on-chain head is given. With the snapshot it is a `StateMismatch` or `SequenceGap`. The control is the comparison with the chain head. LIMITED.
* **`Sample.recv_ns` is never read** by the verifier, and a stray digest on a non-source-3 sample is not read either (the digest is compared for source 3 only, replay.rs:340). The evaluator's receive time is the known oracle weakness of decisions 54 to 57; no verifier check exists for it.
* **`opened.time`** may be any value before the first event, including 0, -1 and `i64::MIN` (one pinned test). A value after the first overnight rollover is a FeeMismatch.
* **The `opened` line may appear anywhere** in the events file (even after the events). The replay finds it first. Info.
* **Unknown JSON fields** in the rules, the snapshot and event lines are ignored (forward compatibility). Unknown line TYPES are a finding (`UnknownEvent`). Hex is accepted in either case. CRLF and blank lines are tolerated. Duplicate JSON keys are an input error (`duplicate field`).
* **Performance.** 40,000 unique plus 40,000 duplicate intent lines: 2.9 s in a debug build (a duplicate scan is O(n) per duplicate, so O(n squared) for a hostile file). 200,000 extra archive records: 0.45 s. 50,000 unknown event lines: 0.06 s. Not a denial of service worth a finding, but the duplicate scan is quadratic.

## 3. What the verifier-input tests cover (test -> attack -> class -> result)

The helper `go()` runs `verify_account` under `catch_unwind`. Any panic fails the test with the case label. Valid outcomes are listed with the reason in the source (`Want::ValidBecause`).

| test | attack | class | expected result |
|---|---|---|---|
| `control_every_committed_vector_is_valid_...` | all 3 main vectors and 14 m9b vectors | control | VALID, so every other case starts from a valid base |
| `blocked_truncation_at_every_byte_boundary_...` | every file (events, rules, archive, intents, snapshot) of 3 vectors cut at EVERY byte (11,441 cases) | BLOCKED | never a panic; VALID only when a trailing whitespace byte was removed from events, rules or snapshot (the same document) |
| `blocked_truncated_files_are_input_errors_...` | cuts mid record, mid header, 4 bytes, mid line, empty | BLOCKED | `Err` with a message naming the file ("archive truncated at byte", "rules file:", "snapshot:", "events line", "no header line") |
| `limited_a_log_cut_at_a_line_boundary_...` | events cut at each line boundary | LIMITED | with snapshot: a non-Valid report with StateMismatch (the shorter log differs from the stored account); without: a shorter valid history or a flagged one. Pinned |
| `blocked_every_byte_of_archive_intents_and_snapshot_...` | every byte of archive, intents, snapshot x 3 xor masks, 3 vectors (8,886 cases) | BLOCKED | no panic, never VALID: every byte of these three files is covered |
| `limited_event_file_single_byte_changes_...` | every byte of the events file of 2 vectors x 3 masks, classified by JSON field | LIMITED | the only fields where a one-byte change still verifies are exactly: `slot` values, the `account` key name, the `type` key name, the final line break, and `opened` challenge_id / tier_index / ruleset_version / time. The test fails on any other survivor AND if one of these disappears from the list |
| `blocked_every_byte_of_the_rules_file_never_panics` | every byte of rules.json x 3 masks x 3 vectors | BLOCKED | no panic |
| `limited_rule_fields_that_can_change_...` | every leaf of rules.json moved by +1 (numbers) or first hex digit changed | LIMITED | exact survivor sets per vector. Not read by the replay: `fee_bps`, `tier_sizes`, `ruleset_version`. Not exercised by the account: other classes' leverage, holding fee and commission, `max_age_secs`, `max_notional_usd`, `qty_min`, session edges, and `holding_fee_e6[0]` when no night is held. Everything else is detected |
| `blocked_events_file_structure` (33 cases) | empty, header-only, wrong header, BOM, invalid UTF-8, NUL, JSON array/string/null lines, opened twice, duplicate seq, swapped lines, deleted line, event of another account, 0x-prefixed / odd / non-hex / doubled / empty event hex, 1 MB proof, 4 MB event | BLOCKED | exact error text or non-Valid; unknown line type is `UnknownEvent` |
| `blocked_events_numeric_fields_...` (50 cases) | seq 0, u64::MAX, 2^64, -1, 1.5, 1e999, "NaN", "inf", "-0", null, NaN/Infinity tokens, leading zero; slot, size_s, tier_index, phase, ruleset_version, challenge_id, time at overflow values; genesis hash lengths; base58 account and trader of wrong length or alphabet | BLOCKED | `Err` naming the field (exit code 2) or a finding |
| `blocked_rules_file_structure_and_values` (65 cases) | empty, null, nesting 100,000 deep, version 0/2/"1"/1.0, arrays of 7/9/1,000,000 entries, entries -1 / 65536 / "NaN" / 1e999 / 5.5, spread keys "x", "-0", "NaN", "1e0", "65536", duplicate keys, class 8/255/256, duplicate market, feed_id lengths and 0x prefix, sessions of wrong arity / reversed / empty / 100,000 windows, qty_step 0 / u64::MAX / 2^64, status 256, unknown field | BLOCKED | `Err` or non-Valid; the 100,000-deep nesting is an `Err`, not a stack overflow |
| `blocked_archive_file_structure` (26 cases) | empty, magic only, lower-case magic, header only, version 0/2/65535, record_len 0/67/69/65535, reserved 1/u32::MAX, flags=1, trailing 1 / 67 / 68 bytes, headers between records, archive twice, 2 MB of 0xff, 2 MB of zeros; second archive garbage / empty / bad record | BLOCKED | `Err` with "bad archive header", "archive truncated at byte", "does not start with the SL8SAMPL header", "bad archive record", "second archive:" |
| `blocked_archive_samples_..._with_consistent_hashes` (22 cases) | the commitments (samples_hash, intents_hash) are RECOMPUTED so only the semantic checks can catch it: every sample duplicated, reversed, swapped, removed, value -1 / 0 / i128::MIN / i128::MAX / u64::MAX x 1e6 / one above, every value negated, ts 0 / u64::MAX, market 65535, source 9 | BLOCKED | never a panic; non-Valid for each. The tail cases (a duplicate at the end, recv_ns, a stray digest) are the pinned VALIDs of section 2 |
| `blocked_intents_file_structure` (34 cases) | empty, bad header, nonce 0 / u32::MAX / 2^32 / -1 / 1.5 / "1" / "NaN" / 1e999 / null, order of 47 / 49 / 0 bytes, non-hex, kind 9, side 2, qty u64::MAX, sig of 63 / 65 / 0 bytes or missing, extra field, duplicates, CRLF | BLOCKED | non-Valid (the line bytes are hashed); tail-only cases pinned as VALID |
| `blocked_snapshot_structure_and_each_field` (37 cases) | `{}`, `[]`, null, version 2, b as string / "NaN" / 2^63 / 1e999 / float / i64::MIN / i64::MAX / -0, seq -1 / u64::MAX / 2^64, latch 256, trades_closed 2^32, hash of 63 chars / empty / zero, duplicate key, every field +1 | BLOCKED | `Err("snapshot:")` for malformed; a seq above the replay is `SequenceGap`, a wrong hash `HashChain`, every other field `StateMismatch` |
| `blocked_as_of_seq_extremes` | as_of_seq 0, 1, 2, 8, 9, 10, 2^32, u64::MAX, with and without snapshot | BLOCKED | never a panic, always a report; 0 replays nothing (seq 0); beyond the log equals the whole log |
| `blocked_second_archive_attacks` | second archive equal / one e12 unit different / truncated at EVERY byte / one record short / values -1, 0, i128::MIN, i128::MAX | BLOCKED | equal: VALID; different: `ArchiveDisagreement`; any non-record-boundary cut: `Err`; bad values: non-Valid; never a panic |
| `blocked_cli_exit_codes_...` | `--as-of-seq` of "-1", 2^64, "1e3", " 5", "0x10", "NaN", "inf", "", "1.0", "-0"; missing files; unknown flags; each of 4 files swapped into each other slot (12 combinations) | BLOCKED | exit code 2 with a message starting "error:", none contains "panicked". ("+5" parses as 5: pinned, harmless) |
| `blocked_the_real_binary_exits_2_...` | the built `setl8-verify` binary on a truncated archive, truncated events, 5 MB of `[` as rules, 4 KB of zeros as events, empty snapshot, a directory as a file | BLOCKED | exit 2, stdout starts with "error:", stderr has no "panicked". A tampered snapshot gives exit 1 and `[StateMismatch]` |
| `blocked_large_hostile_files_finish_quickly` | 80,000 duplicate-nonce intent lines, 200,000 extra archive records, 50,000 unknown event lines | BLOCKED | no panic, each under 60 s (measured 2.9 / 0.45 / 0.06 s, debug) |
| `blocked_intent_signature_attacks_through_the_whole_verifier` (about 530 cases) | with all hashes recomputed: S+L on intent 1, S+L on intent 2, S with the top bit set, zero signature, 0xff signature, R = identity with S = 0, R not on the curve, the second intent's valid signature in the first's line, a valid signature of another key, a valid signature of the trader over the wrong nonce, EVERY one of the 512 single-bit flips of a signature, a trader key not on the curve, a different trader key | BLOCKED | each is `BadIntentSignature` (non-Valid) |
| `blocked_verify_signature_unit_lengths_and_garbage` | signature of 0 / 1 / 32 / 63 / 65 / 1000 bytes, wrong nonce, account, order, order of 47 bytes | BLOCKED | `false` |
| `pin_the_verifier_refuses_a_non_canonical_s_...` | S + L, S + 2L, S + 3L | BLOCKED (pin of the dalek 1.0.1 behaviour) | refused |
| `limited_source_3_blob_digest_...` | digest changed in the archive only / in a proof only / everywhere consistently | BLOCKED for the first two (`ArchiveMissing`, non-Valid), LIMITED for the third (VALID: no blob signature check exists) | as stated |
| `blocked_opened_time_after_the_rollover_...` | opened.time after the night, and far-past values | BLOCKED / LIMITED | FeeMismatch; far-past values VALID (pinned) |
| `finding_vrf_2_*` (2), `finding_vrf_3_*` (1), `pin_vrf_2_*`, `pin_vrf_3_*` | see section 2 | FINDING | the three findings are `#[ignore]` and fail as described |

Evaluator side, `crates/evaluator/tests/adversarial_signatures.rs`, run for both `Ed25519Check` (plain, the harness) and `StrictEd25519` (verify_strict, defined in the file):

| test | attack | class | expected result |
|---|---|---|---|
| `control_an_honest_signature_is_accepted_by_both_implementations` | honest order | control | `Ok` |
| `blocked_malleated_s_plus_l_is_refused_with_bad_signature` | S+L (below 2^253) | BLOCKED | `Err(OrderReject::BadSignature)`; nothing recorded, the honest original is accepted afterwards |
| `blocked_other_non_canonical_s_values_are_refused` | S = L, S = 2L, S+2L, bit 253 set, top bit set, S = 2^256-1 | BLOCKED | `BadSignature` |
| `blocked_all_zero_all_ff_and_half_zero_signatures_are_refused` | zero, 0xff, R zero, S zero, R identity with S zero (for the honest trader) | BLOCKED | `BadSignature` |
| `blocked_every_single_bit_flip_of_the_signature_is_refused` | 512 bit flips | BLOCKED | `BadSignature` |
| `blocked_a_signature_for_another_message_is_refused` | changed order bytes (5 positions), another nonce, another account, no domain prefix, another key | BLOCKED | `BadSignature`; the honest order still passes afterwards |
| `blocked_r_not_on_the_curve_is_refused` | R off the curve | BLOCKED | `BadSignature` |
| `blocked_a_trader_key_that_is_not_on_the_curve_...` | trader key off the curve | BLOCKED | every order is `BadSignature` |
| `pin_dalek_1_0_1_plain_verify_accepts_a_small_order_key_and_verify_strict_does_not` | raw crypto: all 8 small-order points | pin | plain accepts for some message, strict refuses |
| `pin_dalek_1_0_1_rejects_a_non_canonical_s_in_plain_verify` | S+L at the crypto level | pin | refused |
| `finding_vrf_1_*` (ignored), `pin_vrf_1_*` (2) | small-order trader keys | FINDING / pin | plain: accepted; strict: refused for every nonce 1..200 and every key |
| `off_chain_an_evaluator_that_skips_the_signature_check_is_flagged_by_the_verifier` | `AcceptAll` with a zero signature | OFF-CHAIN | evaluator executes the order, the verifier reports `BadIntentSignature` (Mismatch) |
| `finding_vrf_2_verifier_calls_an_unsigned_trade_valid_...` (ignored) | unsigned trade under an identity trader, through the evaluator's output | FINDING | verdict Valid |
| `control_signing_message_binds_account_nonce_and_order` | the signed message | control | changes with account, nonce and order; starts with `SL8LEV|` |

## 4. Counts

* Verifier file: 30 tests (27 pass, 3 ignored). About 50,000 verify runs per `cargo test` (11,441 truncations, 8,886 byte flips of three files, about 17,000 events-file flips, about 4,000 rules flips, about 800 table cases, about 530 signature cases).
* Evaluator file: 16 tests (14 pass, 2 ignored).
* Findings: VRF-1 (info), VRF-2 (low), VRF-3 (info). Each ignored test was run once with `--ignored` and fails for the stated reason.
* Panics found: none in any case.
* Verdict VALID for a tampered input: only in the pinned LIMITED cases above (uncommitted tail, informational fields, small-order trader key = VRF-2).

## 5. What I could not test, and why

* **A production `IntentVerifier`.** None exists in the repo; the evaluator takes it by injection. I tested the two that exist (harness, `AcceptAll`) plus a strict one written in the test file. The backend's own check cannot be tested here.
* **The Solana runtime's own signature check** (that a small-order key cannot sign `open_account`). I read that `solana-signature` 2.3.0 uses `verify_strict`; the validator's sigverify path is not in this repo and the LiteSVM harness has verification off. So the claim "a small-order trader cannot exist" rests on Solana, not on a test here.
* **A different dalek version or feature set.** Only 1.0.1 with default features is in the lockfiles; the canonical-S result depends on `legacy_compatibility` staying off.
* **Real GMX/Stork/Pyth signatures.** Nothing verifies them (see section 1); no authentic blob is available offline.
* **Memory exhaustion by a multi-GB file.** I tested up to 13.6 MB. The CLI reads whole files into memory, so a huge hostile file is bounded by the reader's RAM; I did not try to exhaust it.
