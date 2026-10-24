# Runtime hosting: design and handoff plan (P29a, decision 74 PROPOSED)

Status: DESIGN ONLY. Nothing under programs/, crates/, tests-rs/ or tools/ was touched; every reproduction below ran in a scratch copy under /private/tmp that was deleted afterwards. The program is frozen and this design needs no program change (section 9). Decision 74 is "PROPOSED, awaiting Rovik" in handover/design-decisions.md. The implementation is a list of later prompts (section 10).

Written for: Rovik (approval) and the engineers who implement it, and for the backend team that hosts the evaluator (they read `~/Documents/setl8-back/docs/UPSTREAM.md`; nothing there was edited).

## 0. Verdicts in one table

Each requirement was checked against the code at 2c70883 (the revision the backend imported, unchanged) before anything was proposed.

| # | Claim | Verdict | One line of evidence |
|---|---|---|---|
| R1 | No way to apply a historical market feed/status/config change | CONFIRMED (and narrower than stated: the verifier models only feed and status) | `Evaluator.rules` is a private `Rc<RulesCfg>` (`eval.rs:112`), `rules()` returns `&RulesCfg` (`eval.rs:228`); no method changes it. Reproduction: `ev.apply_market_change(..)` does not compile (E0599). The verifier replays `market_feed_changed` and `market_status_changed` only (`replay.rs:341-376`); every other market field comes from the rules file at the END of the log (`verifier-formats.md` section 1, "Admin market changes"). |
| R2 | No versioned complete checkpoint/restore, no retention | CONFIRMED | 35 private fields (section 4.1); `Sha256State` fields are private (`sha256.rs:19-24`); no `checkpoint` method (E0599). Memory grows at 188 bytes per sample per account (section 4.7). |
| R3 | `refuse()` keeps only 256 refusals; cancel, SL/TP and triggered pending orders have no outcome | PARTLY | 256-cap CONFIRMED (`eval.rs:316-321`; reproduction: 257 accepted market orders refused after a hold arrives, `exec_refused = 257`, `refusals().len() = 256`). Cancel and SL/TP leave no trace CONFIRMED (`exec.rs:154`, `exec.rs:161-168`; reproduction: after a cancel, `pending` empty, `queued_events` 0, `refusals` empty, 0 batches). A triggered pending order that FILLS does have a journal event, REFUTED: it is a `Fill` with `intent_id` = the order's nonce (`exec.rs:347-360`; reproduction: one `Fill{intent_id: 5}`). One that is triggered but refused (margin, lock, hold, cap) has none and is kept pending (`exec.rs:216-219`). |
| R4 | Stale public documentation | CONFIRMED, and larger than stated | `evaluator/src/lib.rs:10,15,16` (200,000 CU, arrival cut, "order" moving the clock) plus 9 more places (section 6). |
| R5 | Lifecycle entry points mutate without preconditions | CONFIRMED, plus one hazard the report did not name | `phase_started` (`eval.rs:477-484`) accepts any phase (reproduction: `phase_started(9)` with an open position leaves `phase = 9`, position kept, `b = S`); `pass_resolved`, `set_held`, `end_externally` (`eval.rs:487-500`) take no argument and check nothing (`end_externally` with a queued event ends the account). The hazard: an order accepted in the second of the pass request executes while the account is frozen (section 7.4, reproduction). |
| Send | `Evaluator` is not `Send` | CONFIRMED, not needed by the backend's current design | `Rc<RulesCfg>` and `Box<dyn IntentVerifier>` (reproduction: E0277 for both). The backend builds the evaluator on its own OS thread (`setl8-back/crates/execution/src/lib.rs:13-28`) and never moves it. Recommendation: make it `Send` anyway (section 8). |

Two choices need a decision from you, not just an implementation: the feed-remap procedure (choice 1) and whether the frozen-execution hazard is fixed as a decision change in its own prompt (choice 2). Neither is a program change.

## 1. What was read

* This repo: `crates/evaluator/src/*.rs` (all), `crates/verifier/src/{replay,walk,verify,log,rules}.rs`, `crates/journal-types/src/{hash,lib}.rs`, `crates/engine/src/{triggers,account,position}.rs`, `programs/setl8-lev-trading/src/instructions/{confirm_pass,pass_common,finalize_pass,post_journal}.rs`, `handover/{verifier-formats,module-10,design-decisions,news-spread-design,backend-compute-contract}.md`, `docs/DESIGN.md` 4.5.
* The backend, read-only: `docs/UPSTREAM.md`, `SERVICE_CONTRACTS.md`, `IMPORT.md`, `DESIGN.md`, `crates/evaluator-service/src/lib.rs` (the adapter that drives the evaluator), `crates/execution/src/lib.rs` (threading and recovery).
* The SDK, read-only: it depends on `journal-types` only (a byte-identical vendored copy, `setl8-sdk/crates/setl8-sdk/Cargo.toml:10`); its `EvaluatorBatchInput` (`journal_instructions.rs:26`) mirrors four fields of `Batch`. It does not use `Evaluator`, `ChainView`, `phase_started` or anything else of this design.

How the backend uses the evaluator (this fixes what must stay compatible): `Runtime::apply` (`evaluator-service/src/lib.rs`) maps each durable input to one call (`on_order`, `on_sample`, `tick`, `drain_batches`, `set_held`, `request_pass`, `pass_resolved`, `phase_started`, `end_externally`), compares the whole result on recovery, and replays EVERY input from the beginning on every start (`execution/src/lib.rs:36-68`). It reads `archive_bytes()[start..]` and `intents_bytes()[start..]` after each input, `refusals()[start..]`, and asserts `exec_refused == refusals().len()` (`evaluator-service/src/lib.rs:247-252`). One of its comments cites a symbol `price_at` in exec.rs (`setl8-back/docs/DESIGN.md`); the function is `order_sample` (`exec.rs:18`). That is a stale reference on their side, noted for them.

## 2. The constraints this design keeps

* **C1 byte equality.** Every journal batch, the archive bytes, the intents file and every hash stay byte-identical for the same inputs. The gate is defined in section 10.1 and is run by every implementation prompt.
* **C2 off-chain only.** None of the five needs the program (section 9).
* **C3 one configuration stream.** R1's API is one time-ordered stream of configuration changes with a reserved variant for the news windows of decision 73 (section 3.1).
* **C4 compatibility and versions** in section 11.

## 3. R1: historical configuration changes

### 3.1 The API (signature level)

```rust
/// One change to a market's configuration. `#[non_exhaustive]`: new variants are additive.
#[non_exhaustive]
pub enum ConfigChange {
    /// An admin `update_market` that remapped the feed. `new_source` is the PriceProof.source of the new feed (the host knows it; the
    /// verifier line carries only the feed id). `old_feed` must equal the feed the evaluator holds (the chain rule of the verifier).
    MarketFeed { market: u16, old_feed: [u8; 32], new_feed: [u8; 32], new_source: u8 },
    /// An admin `update_market` that changed the status (0 disabled, 1 enabled, 2 reduce-only).
    MarketStatus { market: u16, old_status: u8, new_status: u8 },
    // RESERVED, not implemented in P29 (C3): MarketLimits { .. } (max_age_secs, quantity step/minimum, cap, sessions): needs
    // verifier lines first, see 3.6.
    // RESERVED for P28c: NewsWindowPublished { slot, news_time, market_mask }, NewsWindowRemoved { slot, news_time }.
}

pub struct ChangeId(pub u64);   // strictly increasing per account; the host derives it from the chain position (slot << 16 | index)

pub enum ConfigApplied { Applied, Duplicate }

pub enum ConfigError {
    /// `at` is before the evaluator's clock: closed seconds are never rewritten.
    TimeInPast { clock: i64, at: i64 },
    UnknownMarket(u16),
    /// `old_*` differs from what the evaluator holds (the verifier's `StateMismatch`).
    StaleBase,
    /// The journal is not settled: a drained batch is not confirmed on chain, or a queued unit references the market (FEED only, 3.4).
    NotSettled,
    InvalidValue,                  // status above 2, an all-zero feed
    OutOfOrder { last: ChangeId }, // an id below the last applied one
    Conflict,                      // the same id with different content
    Ended,                         // the account is over
    Unsupported,                   // a reserved variant before it is implemented
}

impl Evaluator {
    /// Closes every second before `at` under the OLD configuration (exactly like `tick(at)`), then applies the change; `at` is the
    /// chain second of the change (the block time of the admin transaction). `posted_through` is the journal seq the host knows to be
    /// on chain (`ChainView.seq` semantics); it must equal `self.seq()` for a FEED change.
    pub fn apply_config_change(&mut self, id: ChangeId, at: i64, posted_through: u64, change: ConfigChange) -> Result<ConfigApplied, ConfigError>;
    /// The configuration in force at second `t` (t >= the first change retained), from the time-indexed history.
    pub fn rules_at(&self, t: i64) -> &RulesCfg;
    pub fn rules(&self) -> &RulesCfg;          // unchanged: the configuration now
    pub fn last_change_id(&self) -> ChangeId;
}
```

### 3.2 Ordering against samples, orders and the second-close processing

* The evaluator has no wall clock and no log positions. The host owns the order; the stream of its durable inputs IS the order. `apply_config_change` is one more input in that stream, applied where it sits.
* It first calls the same close routine as `tick(at)` (`eval.rs:408-414`, `decide.rs:59-78`): every second BEFORE `at` is processed (rollover fees, decisions, triggers, pending fills, queued orders) under the OLD configuration. Then the change is applied. So the new configuration governs the whole of second `at` and later.
* Same second, samples: a sample of second `at` recorded BEFORE the call is stored as received (its `Stored` numbers depend on the half-spread, which never changes for an account) and is DECIDED at the close of second `at`, after the change. A sample recorded after the call sees the new configuration too. Same second, orders: an order accepted before the call had its ARRIVAL checks made under the old configuration and they are not redone; its EXECUTION at the close of `at` uses the new one (status, feed). An order that arrives after the call sees the new configuration at arrival. This is the existing split between arrival and execution (`eval.rs:435-471` against `exec.rs:133-158`), made explicit.
* `at < clock` is `TimeInPast` (not silently reordered). A change at `at == clock` is the normal case for a change observed in the current second.

### 3.3 Agreement with the verifier (derived from `replay.rs` and `walk.rs`, not invented)

| Situation | Verifier (file, line) | Evaluator today | Evaluator after P29 |
|---|---|---|---|
| Status becomes 0 or 2 | `status_of(market) != 1` makes `open_refusal` answer `NotTradable` for an exposure increase (`walk.rs:601`); closes, floors, SL/TP and marks are NOT conditioned on status (`market_open` uses sessions and price changes only, `walk.rs` header) | arrival refuses limit/stop (`exec.rs:71`), execution refuses an opening part (`exec.rs:248`); decisions ignore status (`decide.rs:237`) | unchanged; the same rules read `rules()` after the change |
| Open positions of that market | stay open, marked, valued, SL/TP and floors keep working | same | same; nothing is closed by a status change |
| Pending orders of that market | an order that cannot fill stays pending and no event is expected (`verifier-formats.md` 8 D3, `walk.rs` `open_refusal`) | stays pending, `refused_fills` +1 each trigger second (`exec.rs:216-219`) | same; the refusal is reported once per reason change (section 5.5) |
| Reducing/closing orders | never refused by status | same | same |
| Market closed (sessions) | triggers nothing, mark frozen | same | no change |
| Feed remapped | proofs are judged against the feed at the log position (`replay.rs:423`: `mk.feed_id = self.feed_of(mid)`); the archive record is found by `(market, ts, value)` and has no feed field (`replay.rs:456-470`), so old samples remain valid prices of the market | proof feed id = the CURRENT feed at drain time (`drain.rs:59-69`) | tracks, marks and `last_change` are RETAINED (the verifier derives them from the archive without any epoch), `proof_for` stamps the feed in force when the batch is sealed. See 3.4. |
| Same change twice | a second `market_feed_changed` whose `old_feed` is not the current feed: `StateMismatch` (`replay.rs:358-361`) | n/a | `Duplicate` for the same `ChangeId` and content; `StaleBase` otherwise |

### 3.4 Old-history proof identity across a feed change (the archive format does NOT change)

The archive record (`codec.rs:123-136`, 68 bytes) has `market`, `source`, timestamps, value, digest and no feed id; it stays exactly as it is. What a proof's `feed_id` means is fixed as follows.

1. The program checks a proof's feed id against the market's CURRENT feed at posting time (`post_journal.rs:179-182` via `check_proof_without_price`), and the verifier against the feed at the log position (`replay.rs:423`). Both agree: the feed id of a proof is the feed in force when the batch is POSTED. The evaluator stamps at SEAL time (`drain.rs:59-69`, `proof_for`), which is the closest it can get.
2. Therefore a feed change must not land between sealing and posting of a batch that references the market, or the program answers `ProofWrongFeed` (6048) and the sealed batch (its hash is in the chain) can never be posted. This is a property of the program (decision 24 lets an admin remap at any time), not of the evaluator, and it is NOT fixed by any evaluator API. What the evaluator can do: refuse a FEED change while it is not settled (`NotSettled`: `posted_through != seq`, or a queued unit references the market), and recover with a checkpoint taken before the drain (section 4.6): the host restores it, applies the change, drains again and gets batches with the new feed id.
3. Operational rule (to go into the admin runbook and `deploy-checklist.md`): a feed remap is made in a quiet window: set the market to disabled or reduce-only, wait until the backend has posted and confirmed every batch of every account that references it, remap, wait for the backend to apply the change, enable. Because the program offers no way to accept a previous feed for events dated before the change, a remap during trading strands in-flight batches. That is a real limit of the on-chain design; hardening it would need a `Market` layout change (old feed id plus an effective time; the 30 reserved bytes of `Market` cannot hold a 32-byte feed id) and is NOT proposed (choice 1).
4. After the change, a sample recorded before it stays a sample of the market (tracks are retained). It may justify a new event; the proof then carries the NEW feed id with the OLD sample's value and source byte. The program and the verifier accept that (they cannot tell), and for a remap that points the same instrument at an equivalent feed it is the right economics. If Rovik prefers that old-feed prices never justify new events (retire them), the evaluator would need a feed epoch per sample and the verifier an epoch from the line's time; that is choice 1 option B, not recommended.

### 3.5 History of the rules inside the evaluator

`rules: Rc<RulesCfg>` becomes `rules: RulesHistory` = `Vec<RulesEntry { from: i64, id: ChangeId, cfg: Arc<RulesCfg> }>`, newest last; `rules()` is the last entry, `rules_at(t)` a binary search by `from`. It is small (one entry per admin change) and serializes into the checkpoint. It is used by `rules_at` (hosts and tests cross-checking against the verifier), by the checkpoint, and to make `Duplicate` detection exact. No decision reads the history: decisions read the newest entry.

### 3.6 Verifier-side equivalent

* Feed and status lines already exist and are applied by log position (`verify.rs:247-248`). No change is needed for the evaluator to agree on an honest history, because the program itself orders an accepted batch after the change tx that preceded it.
* One additive improvement for the day a position/time mismatch matters (F-25): an optional `time` (block time) field on `market_feed_changed` and `market_status_changed` lines, ignored by version 1 readers; the replay may then also check that no event dated at or after `time` was judged under the old value. Not required for P29; listed in the plan as an optional step.
* Config beyond feed and status (sessions, `max_age_secs`, quantity step/minimum, cap) is NOT modelled by the verifier: the rules file shows the end state for the whole history. An evaluator API for it without a verifier line would let the two disagree, so `MarketLimits` stays RESERVED until `market_limits_changed` lines exist (a verifier prompt of its own).
* C3: the news windows of decision 73 arrive as two more variants of this enum (`NewsWindowPublished`, `NewsWindowRemoved`) and two more log lines; the same `apply_config_change` closes the seconds before `at`, applies the window, and keeps the `ChangeId` order. `non_exhaustive` makes adding them additive.

## 4. R2: complete checkpoint, restore and retention

### 4.1 Inventory: every field of `Evaluator` and everything it points to

`serialize` = written into the checkpoint; `re-derive` = recomputed on restore and compared; `re-inject` = supplied by the host again.

| Field (eval.rs) | Type / what it points to | Treatment |
|---|---|---|
| `rules` (:112) | `Rc<RulesCfg>` -> `RulesCfg{ruleset_version, leverage_x[8], holding_fee_e6[8], commission_e6[8], markets: Vec<MarketCfg{id, class, source, feed_id, max_age_secs, status, qty_step, qty_min, max_notional_micro, sessions: Vec<(u16,u16)>, half_spread_e6}>}` | serialize (the history, 3.5) |
| `acct` (:113) | `AccountParams` (account, trader, challenge_id, size_micro, phase, tier_index, opened_time) | serialize |
| `verifier` (:114) | `Box<dyn IntentVerifier>` | RE-INJECT (`restore(bytes, verifier)`) |
| `erules` (:115) | `engine::Rules{frozen_ratchet, quick_strict}` | serialize (2 bytes; always the default today) |
| `st` (:116) | `AccountState` (11 fields) | serialize |
| `phase` (:117), `trades_closed` (:120), `status` (:121: Active or Failed(reason)), `frozen`, `held` (:122-123), `ended_externally` (:126) | scalars | serialize |
| `positions` (:118) | `BTreeMap<u16, Pos{p: Position{qty, avg_entry_e12, timer_start, opened_at, fee_day}, sl, tp}>` | serialize |
| `pending` (:119) | `Vec<Pending{nonce, intent: OrderIntent}>` | serialize (order matters) |
| `fault` (:124) | `Option<EvalError>` with `Invariant(&'static str)` | REFUSE to checkpoint a faulted evaluator (`CheckpointError::Faulted`): the backend discards a faulted runtime and replays; a `&'static str` cannot be restored |
| `samples` (:128) | `Vec<Stored{rec: SampleRec, mid, bid, ask, last_change}>` | serialize `rec` and `last_change`; `mid`, `bid`, `ask` are re-derived from `rec.value` and the market's half-spread and COMPARED (a mismatch is `Corrupt`) |
| `archive` (:129) | `Vec<u8>` (segment headers and records) | serialize only the UNDRAINED tail, plus the absolute length already drained (4.5) |
| `tracks` (:130) | `BTreeMap<u16, Track{idxs: Vec<usize>, last_value, last_change, last_ts_ns}>` | serialize; `idxs` are absolute sample ids (4.4) |
| `marks` (:131) | `BTreeMap<u16, usize>` | serialize (absolute ids) |
| `last_archive_day` (:132) | `Option<i64>` | serialize |
| `clock`, `processed_through`, `roll_cursor` (:134-136) | `i64` | serialize |
| `buckets` (:137) | `BTreeMap<i64, Bucket{samples: Vec<usize>, orders: Vec<QOrder{nonce, intent}>}>` | serialize |
| `queue` (:139) | `VecDeque<Unit{events: Vec<QEvent{ev: JournalEvent, proof: Option<usize>, after: Snap{st, trades_closed, last_time, flat}}>}>` | serialize (`JournalEvent` as its canonical Borsh bytes; `after` fully; `proof` as an absolute id) |
| `seq`, `head`, `last_meta_time` (:140-142) | `u64`, `[u8;32]`, `i64` | serialize |
| `nonces` (:144) | `BTreeSet<u32>` | serialize (4 bytes each; needed forever, nonces are unique across the log) |
| `intents` (:145) | `Vec<u8>` | like `archive`: the undrained tail plus the drained absolute length |
| `samples_hasher` (:147), `intents_hasher` (:149) | `Sha256State{h[8], block[64], filled, total}` | serialize with the block buffer, `filled` and `total` (new `Sha256State::to_bytes/from_bytes`, 112 bytes) |
| `samples_hashed`, `intents_hashed` (:148, :150) | prefix counters (indexes into `samples`, bytes of `intents`) | serialize as ABSOLUTE counters |
| `stats` (:151) | `Stats` (26 `u32` counters) | serialize (they feed the backend's `unpostable == 0` check) |
| `refusals` (:152) | `Vec<Refusal>` (<= 256) | serialize |
| NEW: `rules history`, `last_change_id` | 3.5 | serialize |
| NEW: outcome queue, `outcome_seq`, `pending_last_refusal` map, `level_source` map (5.4) | section 5 | serialize |
| NEW: `sample_base`, `archive_base`, `intents_base` | section 4.4 | serialize |

Nothing is left to chance: a struct added later breaks `restore` by construction because the writer enumerates the fields, and a test (`checkpoint_covers_every_field`) compares the field list of `Evaluator` (via a macro-generated list) with the writer's.

### 4.2 Byte format (version 1)

```
offset  size  field
0       8     magic "SL8EVCK1"
8       2     checkpoint format version (u16 LE) = 1
10      2     RUNTIME_API_VERSION (u16 LE)
12      2     ENGINE_VERSION (u16 LE) of the engine the writer was built with
14      2     reserved = 0
16      8     account (first 8 bytes) - fast mismatch check, the full key is in the body
24      8     body length (u64 LE)
32      ..    body: sections in a FIXED order, each = tag u8, length u32 LE, content; integers little endian, i128 as 16 bytes LE,
              Vec/Map = u32 LE count + items, Option = 0/1 + value, enums = u8 tag; JournalEvent = its canonical Borsh bytes
end-32  32    sha256 of everything before it
```

### 4.3 API

```rust
impl Evaluator {
    /// A complete, deterministic checkpoint. Err(Faulted) for a faulted evaluator.
    pub fn checkpoint(&self) -> Result<Vec<u8>, CheckpointError>;
    /// Rebuilds an evaluator from `bytes`. Never panics on hostile input: every length is checked against the remaining bytes BEFORE
    /// allocation (a declared count above the remaining bytes is `Corrupt`), every index is range-checked, the trailing hash is verified first.
    pub fn restore(bytes: &[u8], verifier: Box<dyn IntentVerifier + Send>) -> Result<Evaluator, CheckpointError>;
}
pub enum CheckpointError { BadMagic, UnsupportedFormat(u16), UnsupportedRuntime(u16), EngineVersionMismatch{have: u16, want: u16},
                           Truncated, BadHash, Corrupt(&'static str), Faulted }
```

`UnsupportedRuntime`: a checkpoint written by a different `RUNTIME_API_VERSION` is refused, not migrated (a migration is a prompt of its own if ever needed; the backend can always replay inputs, which it already does).

### 4.4 The guarantee and the property test

Guarantee: for any history H and any cut point k, `restore(checkpoint(after k inputs), v)` followed by the remaining inputs yields exactly the outputs of the uninterrupted run: every `Batch` (all fields), the archive and intents bytes, the outcome stream, `stats`, and the final `checkpoint()` bytes.

Test (property, over the campaigns): for 2,000 engine-only paths and 300 differential paths of the default campaigns, with 1 to 5 random cut points each (a seeded generator, the seed printed), compare byte for byte. Cut points are forced to include: inside a queued unit, between a sample and its second's close, between `drain_batches` and the next sample, with a pending order, with an open position, right after a `Fail`, and right after a `Meta`. Hostile bytes: every single-byte flip and every truncation of 20 checkpoints must return `Err` and never panic (a `catch_unwind` harness plus `#![deny(clippy::unwrap_used)]` already in the crate).

### 4.5 Retention (what history is needed after a time, and for what)

A boundary `B` (a second) is valid when `B <= processed_through` and every batch holding events dated at or before `B` has been DRAINED. What the evaluator still needs after `B`:

| History | Needed for | Rule |
|---|---|---|
| for each market, the newest sample with second <= B | `order_sample` (`exec.rs:18`) of an order in an unprocessed second, the staleness test (`exec.rs:27`), `last_change`/`market_is_open`, the mark of an open position | keep exactly one per market |
| every sample with second > B | unprocessed seconds, `order_sample` for a bucket second, hashing | keep |
| samples referenced by `marks` of an open position | valuation, forced closes (`exec.rs:536`) | keep |
| samples referenced by a queued unit (`QEvent.proof`) | the proof of an event not yet drained (`drain.rs:59`) | keep until drained |
| samples not yet hashed (`index >= samples_hashed`) | the next `Meta.samples_hash` prefix (`drain.rs:210-216`) | keep |
| the max_age window of each market | NOT needed by the evaluator: freshness is decided from the newest sample's second, not from older ones; the program checks a proof against the event time, and the only proof a new event can use is the newest sample at or before it | not kept |
| samples at or before B, hashed, older than the newest per market, not referenced | nothing | RELEASED |
| archive bytes | the host's evidence; the evaluator needs only `last_archive_day` and the hasher | handed to the host by `take_archive()`, then dropped |
| intents bytes | the host's evidence; the evaluator needs the hasher and `intents_hashed` | handed over by `take_intents()`, then dropped |
| hash states | continuity of `Meta.samples_hash` / `intents_hash` | kept in full (112 bytes each) |
| `nonces` | `BadNonce` across the whole log | kept (4 bytes per order) |

The index remapping problem: `Track.idxs`, `Bucket.samples`, `marks`, `QEvent.proof`, `samples_hashed` and `Batch.samples_committed` all point into `samples` (`eval.rs:90-96, 104-108, 131, 148`; `types.rs:56`). The design avoids remapping by making every such index ABSOLUTE: `samples` becomes a `VecDeque<Stored>` with `sample_base: u64`; the entry of absolute id `a` is `samples[a - sample_base]`; releasing the first `n` entries only raises `sample_base`. Nothing else changes, `Batch.samples_committed` stays an absolute count (so it equals what the uninterrupted run reports) and `Meta` is unchanged. The same trick for `archive_base` and `intents_base`.

```rust
impl Evaluator {
    /// Releases samples under the rules above. Returns how many were released. Err(NotSettled) if `boundary > processed_through`.
    pub fn release_through(&mut self, boundary: i64) -> Result<usize, RetentionError>;
    /// The archive bytes written since the last call, in order (segment headers included). Concatenating every result gives exactly
    /// the bytes `archive_bytes()` would have returned without any draining; hash continuity is kept in the evaluator.
    pub fn take_archive(&mut self) -> Vec<u8>;
    pub fn take_intents(&mut self) -> Vec<u8>;
    pub fn archive_total_len(&self) -> u64;   // absolute
    pub fn intents_total_len(&self) -> u64;
    pub fn sample_count(&self) -> usize;      // absolute (unchanged meaning)
    pub fn retained_samples(&self) -> usize;
}
```

`archive_bytes()`/`intents_bytes()` keep working and return what has not been taken; a host that never calls `take_*` and `release_through` sees today's behaviour exactly. `sample(idx)` returns `None` for a released id; `sample_stream()` returns the retained records only (documented; BREAKING only for callers that expected the whole stream; the backend does not call either).

### 4.6 Checkpoint plus drain, the recovery recipe

The host keeps the last checkpoint taken before its last `drain_batches`. If a sealed batch cannot be posted (a feed change landed, an RPC fork), it restores that checkpoint, replays its durable inputs after it, applies any new configuration, and drains again: the new batches carry the new proofs and continue the same hash chain from the posted head. That is the use of "restore then continue equals uninterrupted" for the in-flight problem of 3.4; no separate "undo drain" API is needed.

### 4.7 Memory per account, before and after

Measured sizes (a throwaway `size_of` test in the scratch copy): `Stored` 112 bytes, `SampleRec` 80, `QEvent` 184, `Pos` 56, `Sha256State` 112. Per recorded sample the evaluator holds `Stored` 112 + the archive record 68 + the track index 8 = **188 bytes**, and never frees any of it.

| samples per second (all markets of one account) | per day | bytes per day per account |
|---|---|---|
| 1 | 86,400 | 16.2 MB |
| 5 | 432,000 | 81 MB |
| 21 (one per second per launch market) | 1,814,400 | 341 MB |

After retention the steady state is, per market, one anchor sample plus the samples of the unprocessed and not-yet-hashed seconds (the host drains, posts and releases about every second): at the launch size (21 markets) about 21 x (1 anchor + up to 5 seconds of samples at 1 per second) x (112 + 8) = 15 KB, plus the queue, `nonces`, the pending/positions maps and the hash states: **about 20 to 150 KB per account** (the upper figure assumes the host releases once every 60 seconds with 21 samples per second: 1,260 samples x 120 bytes); the undrained archive tail is the host's to take (68 bytes per sample, taken every input). The checkpoint is the same size (a few tens of KB) plus the queue.

### 4.8 A feed store shared between accounts

An honest view. The samples of one market are the same for every account whose rule set lists it; only the archive hash differs per account (it commits to that account's own prefix from its own start). The evaluator does NOT need a trait for this: a host that holds the global ordered stream in one store can call `on_sample` on each account's evaluator, and after retention each evaluator keeps only about 20 to 150 KB, so memory no longer scales with history (about 20 to 150 KB each). The remaining per-account cost is CPU: one `Stored` push, one `SampleRec::to_bytes` and one SHA-256 update of 68 bytes per sample (estimated at about 1 to 3 microseconds; to be measured in the implementation prompt) times accounts times samples per second; at 5,000 accounts and 21 samples per second that is about 105,000 samples per second, a fraction of one core. A zero-copy trait (`SampleSource`) would remove the copies but touches `Stored`, every index and the checkpoint; not recommended before the retained-memory numbers are measured at the target account count (choice 5).

## 5. R3: complete outcomes

### 5.1 The completion paths (every place an order or a position reaches an end state)

| # | Path | Where | Today | Outcome kind (5.2) |
|---|---|---|---|---|
| 1 | Order rejected at arrival (signature, nonce, decode, time, frozen/ended, market, quantity, level, cap, pending limit, price, session) | `accept_order` `eval.rs:435-471`, `validate_arrival` `exec.rs:45-128` | returned `Err(OrderReject)`, nothing stored | `Rejected{reason}` |
| 2 | Order accepted | `eval.rs:462-469` | returned `Ok`, intents line written | `Accepted{kind}` |
| 3 | Market order: no sample / stale / closed | `exec.rs:137-146` | `refuse()` (first 256) | `Refused{reason}` |
| 4 | Market order: refused in `exec_trade` (hold, lock, status, unpriced, remainder, positions, levels, cap, margin) | `exec.rs:149-151`, `229-307` | `refuse()` | `Refused{reason}` |
| 5 | Market order executed (open, add, reduce, flip) | `exec.rs:309-329` | `Fill`/`Close` events with `intent_id` | `Executed` + link |
| 6 | Close order: no position / no sample / closed / quantity | `exec.rs:170-192` | `refuse()` | `Refused{reason}` |
| 7 | Close order executed | `exec.rs:195` | `Close{Manual}` event | `Executed` + link |
| 8 | Limit/stop triggered and filled | `exec.rs:201-215` | `Fill{intent_id}` (or `Close` for a reducing order) | `Executed` + link (and `Triggered` first) |
| 9 | Limit/stop triggered but refused (stays pending) | `exec.rs:216-219` | `refuse()` AND `refused_fills` every trigger second | `TriggerRefused{reason}` (once per reason change) |
| 10 | Stop-loss / take-profit fires | `decide.rs:332-341` -> `close_checked` | `Close{StopLoss|TakeProfit}`, `intent_id 0` | `LevelTriggered{market, kind}` + link (+ the nonce that set the level, 5.4) |
| 11 | Cancel executed | `exec.rs:154` | silent `retain` | `Cancelled{target}` or `CancelNoop{target}` (already filled or gone) |
| 12 | SetSlTp applied | `exec.rs:161-168` | silent | `SlTpApplied` or `SlTpNoop` (position opened after the order's `client_time`) |
| 13 | Breach: forced closes + `Breach`/`TightEnd` | `decide.rs:343-376` | `Close{Breach}` events | `BreachDecided` + links of the closes |
| 14 | Static fail / quick fail: forced closes + `Fail` | `decide.rs:378-396`, `exec.rs:472-519` | `Close{Fail}` + `Fail` events | `AccountFailed{reason}` + links |
| 15 | Pending orders dropped by a fail | `decide.rs:411` (`self.pending.clear()`) | silent | `Cancelled{reason: AccountEnded}` per order |
| 16 | Orders discarded by `end_externally` | `eval.rs:497` | silent | `Cancelled{reason: AccountEnded}` per pending order and queued order |
| 17 | Order still queued/pending when the account is frozen, unfrozen | `eval.rs:439` | n/a | no outcome (state, not completion) |

### 5.2 The `Outcome` schema (versioned)

```rust
pub const OUTCOME_SCHEMA_VERSION: u16 = 1;

pub struct Outcome {
    pub seq: u64,              // 1, 2, 3 ... per evaluator, assigned in emission order; deterministic
    pub time: i64,             // the evaluator second the thing happened in (arrival clock, execution second, trigger second)
    pub nonce: u32,            // 0 when there is none (a forced close, a decision)
    pub market: Option<u16>,
    pub kind: OutcomeKind,     // Rejected, Accepted, Refused, Executed, Triggered, TriggerRefused, LevelTriggered, Cancelled, CancelNoop,
                               // SlTpApplied, SlTpNoop, BreachDecided, AccountFailed
    pub status: OutcomeStatus, // Final | Pending (an accepted order that is still alive) | Informational
    pub reason: Option<OrderReject>,
    pub detail: u64,           // kind-specific (the target nonce of a cancel, the level kind, the fail reason), documented per kind
}

pub enum OutcomeRecord { Outcome(Outcome), Linked { outcome_seq: u64, journal_seq: u64, event_index: u8 } }
```

Journal link: an event gets its chain `seq` only when `drain_batches` seals its batch (`drain.rs:227`), so an outcome cannot know it at emission. Every `QEvent` therefore carries the `seq` of its outcome (a new `Option<u64>` field of `QEvent`), and `seal` emits one `Linked` record per event that has one, in event order. A host reading the stream sees `Outcome#41 Executed(nonce 7)` and later `Linked{41 -> journal seq 1203, index 2}`. `event_index` is the index inside the batch (the `Meta` is last, links never point at it). Where no event exists (rows 1 to 4, 6, 9, 11, 12, 15, 16) there is no link, by definition.

### 5.3 Where each outcome is emitted (no decision changes)

Emission is a pure append at points that already branch: `Rejected`/`Accepted` at the two exits of `accept_order` (`eval.rs:423-433`); `Refused` inside `refuse()` (`eval.rs:316-321`), which also keeps its capped `refusals` vector derived from the same call; `Executed` where `exec_trade`/`exec_close_order` return success (`exec.rs:149, 195, 205`); `Triggered`/`LevelTriggered` in `execute_trigger` and `execute_pending_fill`; `Cancelled`/`SlTpApplied` in the two arms of `exec_order`/`apply_sl_tp`; `BreachDecided`/`AccountFailed` in `do_breach`/`do_static_fail`/`close_checked`; the drops at `decide.rs:411` and `eval.rs:497`. No outcome call reads state that a decision then uses, and none returns a value into a branch; the byte-equality gate (10.1) proves it. The outcome vector is not part of any hash.

### 5.4 Two small bookkeeping additions

* `pending_last_refusal: BTreeMap<u32 /*nonce*/, OrderReject>`: a pending order that is refused at every trigger second would flood the stream (it floods `refused_fills` today, which is why the 257th refusal arrives so easily). `TriggerRefused` is emitted when the reason differs from the last one for that nonce, so a stuck order produces one record per reason change. `Stats.refused_fills` keeps counting every second exactly as today.
* `level_source: BTreeMap<u16, (u32, u32)>`: the nonce that set the current stop-loss and take-profit of a position (the `Fill` or the `SetSlTp`), so `LevelTriggered.nonce` can name it; `Close{StopLoss}` keeps `intent_id 0` in the journal (unchanged bytes).

### 5.5 `drain_outcomes()`, boundedness, and the fate of `refusals()`

```rust
impl Evaluator {
    pub fn drain_outcomes(&mut self) -> Vec<OutcomeRecord>;      // everything since the last call, in seq order; empties the queue
    pub fn outcomes_pending(&self) -> usize;
    pub const OUTCOME_HIGH_WATER: usize = 65_536;
    pub fn backpressure(&self) -> bool;                          // outcomes_pending() >= OUTCOME_HIGH_WATER
}
```

Never drop. At the high-water mark the evaluator REFUSES NEW INPUT that would create outcomes and mutates nothing: `on_order` -> `Err(OrderReject::HostBackpressure)`, `on_sample` -> `Err(SampleError::HostBackpressure)`, `tick` is never refused (the clock must be able to advance, and `tick` returns nothing today); the host sees the mark with `backpressure()` and drains. A single input can still add a bounded burst (the orders already accepted in the seconds it closes, each producing at most two records), so the mark is a soft limit with a documented overshoot of at most twice the accepted-but-uncompleted orders (<= 10 pending + the buckets). The outcome queue is part of the checkpoint (undrained records survive a restore).

`refusals()` stays, documented as "the first 256 `Refused`/`TriggerRefused` reasons, kept for compatibility, derived from the same calls; use `drain_outcomes()`". The unit test `only_the_first_256_refusals_are_kept` (`eval.rs:550-558`) stays as it is; new tests pin that the stream has all 300. `Stats.exec_refused` is unchanged (the backend's `exec_refused == refusals().len()` assertion keeps meaning "no more than 256", which is exactly why they need the stream).

Interaction with the intents file: an accepted order's line stays (the file is the signed record, not a status log). A rejected order leaves no line and exactly one `Rejected` outcome. An accepted order that is later refused, cancelled or dropped has its line and one final outcome. The verifier reads only the file and the journal and is not affected.

## 6. R4: stale statements, with corrected text

| # | Where | Stale text | Corrected text |
|---|---|---|---|
| 1 | `crates/evaluator/src/lib.rs:10-11` | "the evaluator's clock (the newest second of any sample, order or `tick`)" | "the evaluator's clock (the newest second of any accepted sample or `tick`; an order never moves the clock, decision 68, F-18)" |
| 2 | `lib.rs:15` | "Orders execute when their second closes, at the newest sample of their market that existed when they arrived." | "Orders execute when their second closes. A market or close order is priced with the newest sample of its market, in stream order, whose source second is at or before the second it executes in (`exec.rs` `order_sample`, decision 68, D-F08); a triggered limit/stop and a stop-loss/take-profit with the TRIGGER sample (`decide.rs`); the forced closes of a breach or fail with the marks of the decision sample." |
| 3 | `lib.rs:16` | "at most 8 events (the `Meta` last), 3 proofs, 1,232 bytes, 200,000 CU" | "at most 8 events (the `Meta` last), 3 proofs, 1,232 bytes WITH the `SetComputeUnitLimit` instruction, and the compute limit `compute::recommended_cu_limit(kind, events, proofs)` (the worst case measured on Agave 4.0.2 x 1.20, at most 1,400,000; the 200,000 default is not enough for 7 or 8 events, handover/backend-compute-contract.md)" |
| 4 | `crates/evaluator/src/batch.rs:3-4` | "compute units are a conservative bound whose constants were fitted to the M5b/M7 measurements and are checked against the real program in tests-rs/tests/differential.rs" | "compute units are the worst case measured on the validator (`compute::measured_cu`, `handover/cu-runtime.json`); the limit to request is `compute::recommended_cu_limit`; `tests-rs/tests/budget_runtime.rs` asserts the table" |
| 5 | `crates/evaluator/src/config.rs:109` (`SampleError::OutOfOrder`) | "The source second is earlier than a sample already recorded" | "The source second is earlier than the evaluator's clock (the newest accepted sample second or the last `tick`)" |
| 6 | `crates/evaluator/src/eval.rs:475-476` (`phase_started`) | "Positions and pending orders must be gone (the pass check guarantees it)." | "The pass check does not guarantee it: an order accepted in the second of the request can still execute, and the check cannot see orders in `buckets` (section 7.4). After P29 this function checks and refuses." |
| 7 | `crates/verifier/src/log.rs:6` | "... `market_feed_changed`, `market_updated`" | "... `market_feed_changed`, `market_status_changed`" (the type the parser reads, `log.rs:148`) |
| 8 | `handover/verifier-formats.md:3` | "(`design_4_9`, `quick_profit`, `holding_night`)" | add `m9b` (the directory exists) |
| 9 | `handover/verifier-formats.md:90` (section 6, Time) | "an event's `time` is the source second of the sample that caused it" | "an event's `time` is the second the evaluator acted in: for a market or close order the evaluator's own execution second T (its sample is the newest one at or before T); for a trigger the trigger sample's second; for a breach or fail close the decision sample's second" |
| 10 | `handover/verifier-formats.md:132` (section 9, Unseen) | "the order sample it saw" | "the newest sample at or before the execution second" |
| 11 | `handover/module-10.md` "The model" (Time, Orders, Batching bullets) | the clock of "any sample, order or tick"; "the newest sample of its market that existed when it arrived"; "modelled compute <= 200,000 CU" | the corrected texts of rows 1 to 3 |
| 12 | `setl8-back/docs/DESIGN.md` (backend's file, not edited here) | "exec.rs `price_at`" | `order_sample` (`exec.rs:18`) |

Also checked and found accurate: `engine/src/lib.rs` and `consts.rs` headers (milestone words only), `journal-types/src/lib.rs` table, `session.rs`, `hash.rs`, `verifier/src/walk.rs` header, `compute.rs` header, `types.rs` `BatchPolicy::MAX`, the 256-refusal doc of `refusals()` (`eval.rs:260`). Counts in the `module-*.md` history files (golden case counts, mutant counts) are history and were not re-audited.

## 7. R5: lifecycle entry points, preconditions, and the phase-end contract

### 7.1 What the evaluator can check itself and what only the host can authenticate

| Entry point | Evaluator can check | Only the host can authenticate |
|---|---|---|
| `phase_started(phase)` | not ended; `frozen` (a pass is pending); `status == Active`; flat (`positions` empty); no pending orders; `queued_events() == 0`; no orders in `buckets`; `phase == self.phase + 1`; `chain.phase == phase`; `chain.seq == self.seq`; `chain.journal_hash == self.head`; `chain.b == size_micro` (a fresh phase) | that `PhaseAdvanced`/`PassFinalized` was finalized (not reorged), its order against other lifecycle events, the identity of the chain account |
| `pass_resolved()` | not ended; `frozen` | that `PassRejected`/`PassVetoed`/`PassCancelled` was finalized |
| `set_held(held)` | not ended; `held != new` (the verifier's `StateMismatch` for a double hold/release, `replay.rs`) | that `AccountHeld`/`HoldReleased` was finalized and its order |
| `end_externally()` | the account is not already ended | that `AccountAbandoned`/`ViolationFail`/`PassFinalized to_phase 0` was finalized |
| `request_pass(now, chain, replay)` | everything it checks today (`pass.rs:16-44`) plus orders in `buckets` (7.4) | that `PassRequested` was finalized, and the verifier verdict |
| `tick(now)` | monotonic (an earlier `now` is ignored, `eval.rs:402-406`) | that `now` is not far ahead of the host's real clock: `tick` closes every second up to `now` and charges every 22:00 UTC rollover in between (`decide.rs:104-121`), so a wrong host clock charges holding fees for the whole gap; the evaluator has no clock to compare with. An optional `HostLimits.max_tick_jump_secs` (default none) is proposed. |

### 7.2 Typed errors instead of mutation

```rust
pub enum LifecycleError {
    Ended, NotFrozen, NotActive, NotFlat, PendingOrders, QueuedEvents, OrdersQueued,
    WrongPhase { have: u8, want: u8 },
    ChainMismatch { what: &'static str },  // seq | journal_hash | b | phase
    AlreadyHeld, NotHeld, AlreadyEnded,
}
impl Evaluator {
    pub fn phase_started(&mut self, phase: u8, chain: &ChainView) -> Result<(), LifecycleError>;
    pub fn pass_resolved(&mut self) -> Result<(), LifecycleError>;
    pub fn set_held(&mut self, held: bool) -> Result<(), LifecycleError>;
    pub fn end_externally(&mut self) -> Result<(), LifecycleError>;
}
```

A violated precondition changes NOTHING and returns the error. The backend maps it to a refused lifecycle input with evidence (it already stores `evidence` and refuses inputs without it).

### 7.3 The phase-end contract

Which on-chain event ends a phase, what the host passes, in what order, and what the evaluator then does:

| Chain event (finalized) | Host calls, in this order | The evaluator then |
|---|---|---|
| `PassRequested{phase}` | `request_pass(now, chain, replay)`; on `Confirm` the host sends `confirm_pass`; on `Reject` it sends `reject_pass(code)` | frozen; arrival refuses orders (`AccountFrozen`) |
| `PassRejected` / `PassCancelled` / `PassVetoed` | `pass_resolved()` | unfrozen, same phase |
| `PhaseAdvanced{from 1, to 2}` (from `confirm_pass`, phase 1 only; no delay) | `phase_started(2, chain)` | engine state = `reset_phase_state(S)` with the active breach lock KEPT (matches `start_phase`, `pass_common.rs:187-198`); `phase = 2`; unfrozen; positions and pending orders were already empty (checked); `seq`, `head`, archive, intents, hash states, `nonces`, `trades_closed` continue untouched |
| `PassConfirmed{phase 2..5}` | nothing (the account stays frozen through the delay) | frozen |
| `PassFinalized{phase, to_phase}` immediately followed by `PhaseAdvanced{to_phase}` (the program emits both for one `finalize_pass`, verifier-formats 8 D5), `to_phase` 3 to 5 | `phase_started(to_phase, chain)` once (on the `PassFinalized`; the second event is the same transition) | as above |
| `PassFinalized{to_phase 0}` (graduation after phase 5) | `end_externally()` | ended |
| `AccountAbandoned`, `ViolationFail` | `end_externally()` | ended |
| `AccountFailed` (from the evaluator's own `report_fail`) | nothing | the evaluator already holds `Status::Failed` |
| `AccountHeld` / `HoldReleased` | `set_held(true/false)` | exposure-increasing orders refused/allowed |

The 12-hour breach lock across a phase change is KEPT, as today and as on chain (`start_phase` keeps `lock_until`; verifier `on_phase_change` keeps it, `replay.rs:1360`): a trader locked at the end of a phase cannot open in the first hours of the next. The contract says so explicitly and a test pins it on both sides.

### 7.4 The hazard found on the way: an accepted order executes while frozen

`accept_order` refuses orders once `frozen` (`eval.rs:439`), but an order accepted EARLIER in the same second sits in `buckets` and `exec_order` (`exec.rs:133-158`) has no frozen check. Reproduction (scratch test): accept a market order at T0, call `request_pass(T0, ...)` (it answers `Reject(TargetNotReached)` because `b = S`, and the account stays frozen), then `tick(T0 + 1)`: `frozen = true`, `positions = 1`, `queued_events = 1`. With a passing balance the answer would be `Confirm` (`positions` is empty at that instant), and the order then opens a position during the delay; a `Fill` posted while `pass_pending == 2` is refused by the program (`PassPending`), so the account would be stuck. The new `phase_started` precondition (`OrdersQueued`) catches the second half; the first half (the pass answer is wrong) is a DECISION change: `request_pass` should answer `Other`/`JournalBehind` when `buckets` holds an unexecuted order, or `exec_order` should refuse with `AccountFrozen`. It changes bytes only in histories where an order and a pass request share a second, so it is proposed as a separate prompt with its own gate and Rovik's explicit approval (choice 2), not folded into the hosting prompts.

## 8. Threading: should `Evaluator` be `Send`?

How the backend hosts it today: one OS thread per `work` call builds the `Runtime`, replays all inputs and processes the new ones inside `handle.block_on` (`execution/src/lib.rs:13-28`); only owned results cross the channel. So `Send` is not needed. It becomes useful when the backend moves to many accounts: a work-stealing runtime (`tokio::spawn`, `spawn_blocking`, rayon), moving an evaluator between shard threads for rebalancing, or keeping it across `.await` in a spawned task. Today those require one pinned thread per shard and no migration.

Cost of making it `Send`: `Rc<RulesCfg>` is never cloned (`grep`: constructed at `eval.rs:187` only), so it becomes plain `Arc<RulesCfg>` entries of the history (3.5) at no runtime cost; `Box<dyn IntentVerifier>` becomes `Box<dyn IntentVerifier + Send>`, which BREAKS any caller passing a non-`Send` verifier (the backend's `WalletVerifier` holds no non-`Send` state; the test `AcceptAll` and `Ed25519Check` are `Send`). The type stays `!Sync` (not needed). WASM builds are unaffected. A compile-time test `assert_send::<Evaluator>()` pins it. Recommendation: do it in the first implementation prompt (P29c), as part of the signature change of `Evaluator::new` that `restore` needs anyway (both take `Box<dyn IntentVerifier + Send>`).

## 9. On-chain change: none

* R1: the program already enforces the feed at posting; the in-flight limit of 3.4 is a property of decision 24, hardening it would need a `Market` layout extension and is NOT proposed. Nothing in R1 needs the program.
* R2, R3, R4, R5, Send: the evaluator library and the verifier's documentation/lines only.
* Can anything ride the news-spread program change (P28b to P28d)? Not needed. The only candidate is the feed-remap hardening; if Rovik wants it later, it belongs in the same program prompt as the calendar (both change `Market`/`SectorConfig` layouts and both end the freeze). It does not need to wait for it.
* `crates/engine` and `crates/journal-types` are untouched by every step of the plan below. The program, both build hashes, the vault fixture, the SDK and its vendored `journal-types` copy are unaffected.

## 10. The plan

### 10.1 The byte-equality gate (C1), run by every implementation prompt

1. **Baseline first, from the unchanged code.** Prompt P29b adds a test-only harness (`crates/evaluator/tests/p29_bytes_gate.rs`, no `src` change) that runs the campaigns and, for every path, hashes in a fixed order: each `Batch` (the `JournalBatch` bytes, `kind`, `markets`, `meta_time`, `samples_committed`, `intents_committed_bytes`, `account_after`, `tx_bytes`, `cu`, `cu_limit`, `tx_format`), the final `archive_bytes()`, the final `intents_bytes()`, `seq()`, `head()`, `state()`, `positions()` and `pending()`. It writes one SHA-256 per path and per 1,000 paths into `handover/audit/p29-baseline.json`, produced at 2c70883 (engine-only 100,000 paths seed 1; engine-only 10,000 paths with `SETL8_LAUNCH_COSTS`; the differential's 10,000 paths via `tests-rs`, which already executes every batch on the real program).
2. **Every later prompt** re-runs the harness and requires EVERY hash to equal the baseline. A difference is a STOP for that prompt.
3. **Golden vectors** (`crates/evaluator/tests/golden/*.json`, `check.py` 0 mismatches) unchanged and byte-identical (`git diff` on the directory is empty).
4. **Properties** added by the plan: restore-equals-uninterrupted (4.4), release-equals-no-release (outputs and concatenated `take_*` equal the uninterrupted bytes), outcomes-do-not-change-decisions (the byte gate itself).

### 10.2 Prompts, in order

| Prompt | Scope | Tests first | Gate / mutation |
|---|---|---|---|
| **P29b** baseline and documentation | the byte gate harness and `p29-baseline.json`; the comment/doc fixes of section 6 (rows 1 to 5 and 7 to 11; row 6 belongs to P29c, when `phase_started` really checks), no behaviour change | the harness itself; a test that fails on a stale sentence is not needed | the harness at 2c70883; `git diff` shows only comments/docs; all existing tests; clippy |
| **P29c** `Send` and lifecycle | `Box<dyn IntentVerifier + Send>`, `Arc`; `LifecycleError` and the typed lifecycle methods (7.2, BREAKING); `request_pass` stays as is | `assert_send::<Evaluator>()`; one test per precondition and per error (a refused call changes nothing: byte-compare the checkpoint-equivalent state before and after); the phase-end contract table as a test over a scripted pass flow including the lock kept | byte gate; mutants of the changed lines (`cargo mutants --in-diff`) |
| **P29d** outcomes | `Outcome`, `drain_outcomes`, `backpressure`, the emission points, `level_source`, `pending_last_refusal`, links in `seal` | the 17 paths of 5.1 each with a hand-written scenario; the 300-refusal case; the cancel; the SL/TP; high-water behaviour (refuse, never drop); ordering/determinism (same inputs, same stream) | byte gate (decisions unchanged); mutation on the emission points |
| **P29e** configuration stream | `ConfigChange`, `ChangeId`, `apply_config_change`, `RulesHistory`, `rules_at`, `NotSettled`; the verifier's optional `time` field on the two market lines (additive); ordering tests against the verifier | scripted histories with a status change and a feed change in the same second as samples and orders, run through evaluator AND verifier (VALID); idempotency/conflict/out-of-order; reserved variants return `Unsupported` | byte gate with an empty change stream; verifier tests; mutation |
| **P29f** checkpoint | `Sha256State` bytes, the format of 4.2, `checkpoint`, `restore`, `CheckpointError`, field-coverage test | restore-equals-uninterrupted over 2,000 + 300 campaign paths with random cuts; the hostile-bytes sweep; format golden file | byte gate; the property test is the gate; mutation |
| **P29g** retention | `sample_base`/absolute ids, `release_through`, `take_archive`, `take_intents`, totals; the memory measurement at 21 markets | release-equals-no-release over the campaigns; every retained-set rule of 4.5 as a unit test; the memory formula asserted; checkpoint after release restores | byte gate; memory numbers recorded |
| **P29h** (OPTIONAL, needs Rovik's explicit approval) frozen-execution fix | `request_pass`/`exec_order` behaviour for an order accepted in the pass second (7.4) | the reproduction; the verifier agrees | the byte gate is EXPECTED to differ only on histories with that coincidence; the prompt lists any path that differs and why |
| **P29i** close | `RUNTIME_API_VERSION = 2`, `OUTCOME_SCHEMA_VERSION = 1`, `CHECKPOINT_FORMAT_VERSION = 1`, crate version 0.2.0; `handover/runtime-api.md` (the reference the backend needs: every public item, its errors, the host contract of sections 3.4, 7.3); `design-decisions` 74 -> APPROVED; STATE; the candidate-revision manifest (7.3.1); the 100,000-path engine-only and 10,000-path differential campaigns; full mutation of the changed files; gates | the whole suite | all gates; the manifest lists the commit hash, the file list and the sha256 of each exported file |

**Estimated number of Claude Code prompts: 8** (P29b to P29g, P29i, plus P29h if approved: 9), each a normal-size prompt of the P25 to P27 kind. Add one review round by Rovik after P29b (baseline) and one before P29i.

### 10.3 Where it sits relative to P28b to P28d (the news-spread program, evaluator and verifier steps of `news-spread-design.md` 7.2)

* P29 touches the evaluator and the verifier but neither the program nor `journal-types`; P28b changes the program, P28c the evaluator/verifier/`journal-types` for the calendar, P28d the audit and campaigns. With an EMPTY calendar P28c must be byte-identical, so its gate IS the P29 baseline.
* Recommended order: **P29b to P29g first, then P28c on top of them**, because P28c needs exactly what P29e provides (one time-ordered configuration stream with a reserved news variant) and what P29d provides (an outcome for a news-priced execution is just an outcome), and because doing P29 first keeps P28c's diff small and reviewable. P28b (the program) is independent and can run in parallel with P29.
* **The handoff: the backend re-exports ONCE, from a reviewed candidate revision.** The reason is cost: an export means a new reviewed commit, new file hashes in `interfaces/*-pin.json`, a new evidence run (`tools/evidence.py`), and, if `journal-types` changes, a new SDK pin (the SDK vendors a byte-identical copy). The decision point is the end of P29g: if decision 73 has been approved and P28b/P28c are scheduled to start, wait and export after P28c (one export, `RUNTIME_API_VERSION` 3 = P29 + the calendar, and the SDK re-pin for the new program happens at the same time); if decision 73 is still waiting for Rovik's open items (D5, the cap, the broken snapshot), export P29 at P29i as `RUNTIME_API_VERSION = 2` (it needs no SDK change because it does not touch `journal-types`) so the backend is not blocked on production hosting, and treat P28c as a later additive revision (`ConfigChange::News*` was reserved with `non_exhaustive`). Recommendation: export at P29i unless decision 73 is approved before P29g ends.

#### 10.3.1 The candidate-revision manifest

`handover/runtime-export-manifest.md`, written by P29i: the commit hash; `RUNTIME_API_VERSION`, `CHECKPOINT_FORMAT_VERSION`, `OUTCOME_SCHEMA_VERSION`, `ENGINE_VERSION`; for the four exported crates the list of source files with their sha256; the list of public items added, changed (BREAKING) and unchanged; the byte-gate result (the baseline hashes and the new hashes, equal); a statement that the program build hashes are unchanged. The backend verifies it with its existing `tools/evidence.py` procedure and never patches `vendor/`.

### 10.4 What stays UNCHANGED

`programs/` (both build hashes, 43fdd935... and ebbd0786...), `crates/engine`, `crates/journal-types` (including `ENGINE_VERSION` and every wire format), the journal batch bytes, the sample archive record and segment format, the intents file format, every hash and the hash-chain rules, the golden vectors, the trust matrix classes, the vault and its fixture, the SDK and its vendored `journal-types`, `config/launch.*`, the rule set and the launch costs, the verifier's findings and verdicts for every existing history (the verifier changes only by one optional field and, later, by decision 73), the decisions of the evaluator (floors, triggers, pricing, batching, the pass answers) except the optional P29h.

## 11. Compatibility and versions (C4)

### 11.1 Every change, additive or breaking for the backend

| Change | Kind | The backend must |
|---|---|---|
| `drain_outcomes`, `OutcomeRecord`, `backpressure`, `outcomes_pending` | ADDITIVE | adopt (replaces its 256-refusal workaround) |
| `OrderReject::HostBackpressure`, `SampleError::HostBackpressure` (new variants of exhaustive enums) | BREAKING only for exhaustive `match` (the backend uses `format!("{e:?}")`) | recompile |
| `refusals()` documented as the first 256 | ADDITIVE (docs) | stop asserting `exec_refused == refusals().len()` |
| `apply_config_change`, `ConfigChange` (`non_exhaustive`), `ChangeId`, `rules_at`, `RulesHistory` | ADDITIVE | implement its `MarketConfigurationChanged` input |
| `rules()` still returns `&RulesCfg` | ADDITIVE (unchanged) | none |
| `checkpoint()`, `restore()`, `CheckpointError`, `Sha256State::{to_bytes,from_bytes}` | ADDITIVE | adopt for recovery |
| `Evaluator::new(.., Box<dyn IntentVerifier>)` -> `Box<dyn IntentVerifier + Send>` | BREAKING for non-`Send` verifiers | none (its verifier is `Send`) |
| `Evaluator: Send` | ADDITIVE | optional |
| `phase_started(phase, &ChainView) -> Result`, `pass_resolved`, `set_held`, `end_externally` return `Result<(), LifecycleError>` | BREAKING (signature) | adapt the four call sites in `evaluator-service/src/lib.rs` (:182, :210, :216, :221) |
| `release_through`, `take_archive`, `take_intents`, `archive_total_len`, `intents_total_len`, `retained_samples` | ADDITIVE | adopt |
| `sample(idx)` returns `None` for a released id; `sample_stream()` returns retained records only | BEHAVIOUR change only after `release_through` is used | none (it calls neither) |
| `sample_count()`, `Batch.samples_committed`, `intents_committed_bytes` | UNCHANGED (absolute) | none |
| `LifecycleError` replaces silent mutation of a violated precondition | BEHAVIOUR change for invalid use only | surface the error as a refused input |
| the doc fixes of section 6 | ADDITIVE | none |
| optional P29h | DECISION change in one coincidence | review |

There is NO change in the journal bytes, the archive, the intents file or any hash for the same inputs.

### 11.2 Version constants and how they relate

| Constant | Where | Starts at | Moves when |
|---|---|---|---|
| `RUNTIME_API_VERSION: u32` | `evaluator` crate root | 1 = the revision the backend imported (2c70883, implicit) ; **2 = after P29i** | any change to the public API or its documented behaviour (breaking or additive) of `Evaluator` and its types; recorded in the checkpoint header |
| `CHECKPOINT_FORMAT_VERSION: u16` | `evaluator::checkpoint` | 1 | any change to the byte layout of 4.2 (a restore of another version is refused) |
| `OUTCOME_SCHEMA_VERSION: u16` | `evaluator::outcome` | 1 | any change to the fields/meaning of `Outcome` (new `OutcomeKind` values are additive and keep the version; a changed meaning bumps it) |
| crate version `setl8-lev-evaluator` | `Cargo.toml` | 0.1.0 | 0.2.0 at P29i (breaking), semver thereafter |
| `ENGINE_VERSION` | `engine` | unchanged | an engine function changes (decision 73 choice 10: not for the news spread). P29 changes no engine function: UNCHANGED |
| program build hashes | `scripts/build-test-so.sh`, `anchor-build-checked.sh` | 43fdd935..., ebbd0786... | the program changes (P28b). UNCHANGED by P29 |

The relations: `RUNTIME_API_VERSION` is about the library's API and host contract, independent of the wire formats; `ENGINE_VERSION` is stamped in every `Meta` and is about the arithmetic; the program build hash is about the deployed code. A change in one does not force a bump in the others; the manifest (10.3.1) records all of them together so the backend can pin the tuple. `RUNTIME_API_VERSION` 3 (the calendar) will be an additive step over 2.

### 11.3 The re-export procedure for the backend

1. The reviewed candidate is a commit of THIS repo named in the manifest. 2. The backend runs its owner-authorized export (`tools/evidence.py export`, IMPORT.md) against that commit as a NEW snapshot beside the old one, never over it, and `check --lev` verifies the working copy against the manifest hashes. 3. It adapts its application code (outside `vendor/`) to the table above and reruns its recovery tests: the replay of all old inputs must reproduce the old results byte for byte (the backend's own "deterministic replay mismatch" check is exactly the C1 gate on their side). 4. It switches the pin. It never edits `vendor/`.

### 11.4 What the SDK must know

Nothing of this design. It depends on `journal-types` only (byte-identical copy), uses neither `Evaluator` nor `ChainView`, and P29 does not touch `journal-types`. P28c will (the calendar module), and the SDK vendored copy must then be re-pinned.

## 12. Choices where the design could go either way

| # | Choice | Options | Recommendation |
|---|---|---|---|
| 1 | Feed remap | A: retain tracks, stamp the feed in force at seal, a quiet-window runbook, `NotSettled` guard (this design). B: per-sample feed epoch, old-feed prices never justify new events (needs a verifier epoch and leaves positions unpriceable until a new sample). C: program hardening (old feed + effective time in `Market`) | A. C only if remaps during trading are expected; it would ride the calendar's layout work |
| 2 | The frozen-execution hazard (7.4) | fix as a decision change (`request_pass` answers on `buckets`, or `exec_order` refuses when frozen) / document only | fix, as the separate optional P29h, with your approval; the typed `phase_started` precondition protects the phase change either way |
| 3 | Typed lifecycle API | replace the four methods (BREAKING, one consumer, 4 call sites) / add `try_*` and deprecate | replace: the only consumer is not in production and `#[must_use]` makes the error impossible to ignore |
| 4 | Outcome link | `Linked` records at seal / a field on `Batch` | `Linked` records: `Batch` keeps its fields |
| 5 | Shared feed store | a `SampleSource` trait / the host feeds each account | the host feeds each account; revisit with measured CPU at the target account count |
| 6 | Checkpoint of a faulted evaluator | refuse / serialize the fault | refuse: faulted runtimes are discarded and replayed |
| 7 | Retention boundary owner | the host passes `release_through(B)` / the evaluator releases by itself after every drain | the host: the evaluator cannot know what the host has stored |
| 8 | `tick` jump guard | none / `HostLimits.max_tick_jump_secs` | add it, default none (no behaviour change) |
| 9 | Verifier lines for sessions/limits | now / later | later, with its own prompt, only when an admin actually changes them mid-history; until then the evaluator API reserves the variant |
| 10 | Export timing | after P29i / after P28c | after P29i, unless decision 73 is approved before P29g ends (10.3) |

STOP items: none. Two decisions are Rovik's (choices 1 and 2); the design is complete under the recommendations.

## Appendix: the reproductions (scratch copy, deleted afterwards)

Scratch crate copy of `crates/{engine,journal-types,verifier,evaluator}` (the program and tests-rs excluded), tests added only there:
* `p29_repro::r3_the_257th_refusal_has_no_detail`: 257 accepted market orders, `set_held(true)`, `tick(T0+1)` -> `exec_refused=257 refusals().len()=256`.
* `r3_cancel_leaves_no_trace_a_host_can_read`: limit then cancel -> `pending=0 queued_events=0 refusals=0 batches=0`.
* `r3_a_triggered_pending_fill_does_have_a_journal_event`: -> one `Fill{intent_id:5}`.
* `r5_phase_started_with_an_open_position_and_a_nonsense_phase`: -> `positions=1 b=S phase=9 frozen=false`; `end_externally` with 1 queued event -> `is_ended=true`.
* `r5_an_order_accepted_in_the_second_of_the_pass_request_executes_while_frozen`: -> `Reject(TargetNotReached) frozen=true positions=0`, then `frozen=true positions=1 queued_events=1`.
* `p29_send_fail`: E0277 `Rc<RulesCfg>` and `dyn IntentVerifier` cannot be sent between threads.
* `p29_api_fail`: E0599 no method `apply_market_change`, no method `checkpoint`.
* `p29_sizes`: `Stored` 112, `SampleRec` 80, `Pos` 56, `QEvent` 184, `Sha256State` 112, `MarketCfg` 96, `Stats` 104.
