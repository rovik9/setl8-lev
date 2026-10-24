# News spread: design (P28a, decision 73 PROPOSED)

Status: DESIGN ONLY. Nothing under programs/, crates/, tests-rs/ or tools/ was touched; the program is still frozen (localnet sha256 43fdd9356c77...bc1b, default ebbd0786d2d1...2c3b). Decision 73 is "PROPOSED, awaiting Rovik" in handover/design-decisions.md. The implementation is a later prompt that starts only after approval.

Written for: Rovik (approval) and the engineer who implements it afterwards. Numbers are measured from the repo (handover/cu-runtime.json, the program sources) or derived by hand; every estimate says how.

## 0. The decision and the recommended design in one page

The decision (Rovik, 2026-10-23):
1. News spread = 10 x the market's normal half-spread (owner base 100 e6 -> 1,000 e6 = 10 bp per side, 20 bp full).
2. News window = [T - 60 s, T + 60 s] in journal event seconds.
3. It applies to every execution priced inside the window (market Fills and Closes, triggered pending orders, stop-loss/take-profit closes, breach closes, forced Fail closes). It never stacks: overlapping windows give 10x.

Recommended design:
* WHERE: a fixed calendar of 64 slots in the TAIL of the existing `SectorConfig` account (270 -> 1,806 bytes; the first 270 bytes keep their Borsh layout, the tail is read by raw offset). `post_journal` and `report_fail` already receive the config account, so the change costs ZERO extra accounts and ZERO transaction bytes. A wrong or stale calendar account cannot be passed: it is the config PDA the program already seeds-checks.
* SLOT: `news_time i64` + `market_mask u128` = 24 bytes. A market bitmask (bit = market id), not a class mask.
* WHO: `publish_news_window(news_time, market_mask)` and `remove_news_window(slot, news_time)`, either admin (the pattern of `create_market`/`update_market`), through the existing `AdminAction` accounts (config [W], admin_a, admin_b, event_authority, program).
* RULES: publish only if `start = news_time - 60 >= now + 3,600` (and `<= now + 14 days`); remove only while `now + 1,800 <= start`; at most 64 windows (live + future + 7 days of expired ones); the window length is fixed by the program (121 s, both ends included), there is no duration argument.
* PRICING: `Fill` and `Close` in `post_journal` and `report_fail` are checked against `engine::fill_price_e12`/`close_price_e12` with the half-spread `h x 10` when the event `time` is inside a live window whose mask has the market's bit, else `h`. The pure engine functions are called with another argument; `crates/engine` stays UNTOUCHED.
* MARKS: floors, breach, quick tally, holding fee and equity keep the BASE spread (D5, with numbers; this is the item that needs Rovik's confirmation).
* New: error codes 6111 to 6118, events `NewsWindowPublished`/`NewsWindowRemoved`, invariants I-13 (price) and I-14 (calendar horizon), a verifier finding class `NewsSpreadMismatch`, two log line types, two who-may-call rows.
* Cost: about +10,000 CU worst case per `post_journal`/`report_fail` (limits 227,000 -> 239,000 and 270,000 -> 282,000, cap 1,400,000), 0 bytes. No shape stops fitting; no forced close-all becomes unpostable.

Two points need Rovik before approval (section 8): (a) D5, marks keep the base spread; (b) the interaction of the 10x rule with the oracle-noise floor of P24b: if P24b raises a base to 4 to 11 bp, ten times that is 40 to 110 bp per side, up to 21 % of a 20x account per side. A cap is proposed (section 8, choice 6).

## 1. What changes where

### 1.1 Does `crates/engine` stay untouched? YES

The program and the evaluator already call the pure functions `engine::fill_price_e12(mid, half_spread_e6, long)` and `engine::close_price_e12(mid, half_spread_e6, long)` (`crates/engine/src/claims.rs:56,62`), `ask_from_mid`/`bid_from_mid` (`price.rs:19,26`), `check_protective` and `check_pending` (`triggers.rs`), which take the half-spread or a ready-made (bid, ask) pair as ARGUMENTS. The news spread is a different argument: `h_eff = 10 x h`. Overflow: `spread_e6` is a `u16` (at most 65,535), so `10 x h <= 655,350`, below the engine's limit of 1,000,000 (`bid_from_mid` refuses h > 1e6 as `InvalidInput`), and `u32` holds it. Nothing in the engine knows about time or windows, and it should not. The engine version constant (`ENGINE_VERSION`, checked against `Meta.engine_version`) is NOT bumped: no engine function changes (choice 10 in section 8).

The shared PURE rule "which half-spread applies at time t" goes into `crates/journal-types` (next to `in_session`/`market_open` in `session.rs`), as a new module `news.rs`, so that the program, the evaluator and the verifier run the same function:

```rust
pub const NEWS_MULTIPLIER: u32 = 10;
pub const NEWS_PRE_SECS: i64 = 60;     // window = [news_time - 60, news_time + 60], both ends included
pub const NEWS_POST_SECS: i64 = 60;
pub struct NewsSlot { pub news_time: i64, pub market_mask: u128 }   // an empty slot has market_mask == 0
pub fn in_news_window(slots: &[NewsSlot], market: u16, t: i64) -> bool;
pub fn exec_half_spread(base_e6: u32, slots: &[NewsSlot], market: u16, t: i64) -> u32;   // base x 10 inside, base outside, 0 stays 0
```

`journal-types` changes only by this addition: no wire format changes (`Fill`, `Close`, `Fee`, `Meta` and the batch encoding stay byte-identical, so the golden vectors of the codec and the hash chain do not change).

### 1.2 Program (instructions and functions)

| Item | Change |
|---|---|
| `instructions/post_journal.rs`, `Env::half_spread` (:194) and the `Fill` arm (:264-282) and `Close` arm (:283-305) | The two `engine::*_price_e12(mid, env.half_spread(market)?, long)` calls use `env.exec_half_spread(market, time)?` (base from the rule set, x10 inside a live window). `MarketUnpriced` (`half_spread != 0`) still tests the BASE value (10 x 0 = 0: an unpriced market stays unpriced, and the news multiplier on a zero spread is zero). New check at the top of both arms: `ev.time > config.news_retired_before` else `NewsCalendarExpired` (I-14). |
| `run_batch(...)` (shared by `post_journal` and `report_fail`) | Receives the config account info (it is already an account of both instructions) and builds the 64-slot view once per batch (a raw read of 64 x 24 bytes). `report_fail` needs no new account: `Close{Fail}` events are priced by the same `Close` arm. |
| New `instructions/news.rs` | `publish_news_window`, `remove_news_window` (below). |
| New `state/news_calendar.rs` | The tail layout: offsets, `read_slots`, `write_slot`, `reclaimable`. |
| `constants/mod.rs` | `NEWS_*` constants (section 2.1). |
| `errors.rs`, `events.rs`, `lib.rs` | 8 error codes, 2 events, 2 instruction entries. |
| `instructions/init_config.rs:33` | `space = SectorConfig::SIZE + NEWS_TAIL_LEN` (270 + 1,536 = 1,806). `SectorConfig::SIZE` stays 270, the `const_assert!` stays, the Borsh struct stays. |
| `SectorConfig.reserved [u8;64]` | 8 bytes become `news_retired_before i64` (the struct keeps 270 bytes: `reserved` shrinks to 56). |

Pre-mainnet there is no account to migrate: the program was never deployed outside LiteSVM and local validators (decision 72, 5). If a Config exists on a cluster (the devnet rehearsal), it is created again (a throwaway cluster) rather than grown.

### 1.3 Evaluator

| Item | Change |
|---|---|
| `config.rs` | `Calendar` (the 64 slots + `synced_until`), loaded from the chain, fed by the two events. |
| `eval.rs:363` (`on_sample`) | `Stored.bid/ask` stay the BASE quotes: they are the marks. Nothing else here changes. |
| new `exec_quotes(cfg, mid, t)` | The (bid, ask) pair at the EXECUTION second t with `exec_half_spread`. Used where a price becomes a fill or a trigger: `exec.rs:148` (market fill), `:194` (close order), `:538` (`forced_todo`), `decide.rs:269` (`check_protective`), `:280` (`check_pending`), `:336` (forced closes of a decision). |
| stays on BASE quotes | `profit_and_commission` (`decide.rs:209-211`), `charge_night` (`:137`, holding fee), the floor decision `evaluate_sample`, `validate_stop_level` at placement (`exec.rs:121,265`), the margin check's mark of OTHER positions. |
| refusal when the calendar is not current | `calendar_synced_until` rule of section 3.1. |

### 1.4 Verifier

`log.rs` (two line types), `rules.rs`/`snapshot.rs` (the calendar and its anchor), `replay.rs:900,958` (fill and close price check use `exec_half_spread`; the holding-fee mark at :541 stays base), `walk.rs:392,431,718` (the pair used for triggers and for the order-refusal margin check at execution: news pair; the marks: base pair), `finding.rs` (`NewsSpreadMismatch`, appended at the END after `FalseFailMarks`; `FindingClass::ALL` 46 -> 47).

## 2. The program interface

### 2.1 Constants (all in the program, `constants/mod.rs`; the first three also in `journal-types`)

| Constant | Value | Why |
|---|---|---|
| `NEWS_MULTIPLIER` | 10 | the decision |
| `NEWS_PRE_SECS`, `NEWS_POST_SECS` | 60, 60 | the decision |
| `NEWS_MIN_LEAD_SECS` | 3,600 | anti-retroactivity (D2) |
| `NEWS_MAX_LEAD_SECS` | 1,209,600 (14 days) | keeps slots from being hogged and keeps the indexer's required history finite |
| `NEWS_REMOVE_CUTOFF_SECS` | 1,800 | a removal must reach the evaluator before it prices; see 3.1 |
| `NEWS_RETAIN_SECS` | 604,800 (7 days) | expired windows stay readable for late batches; equals the vault's 7-day idle limit |
| `NEWS_SLOTS` | 64 | 14 days ahead + 7 days back = 21 days; at most about 12 distinct news seconds a week x 3 weeks = 36 |

### 2.2 The window: which ends are inclusive

A live window is `start = news_time - 60` to `end = news_time + 60`, BOTH ENDS INCLUDED: an event is in the window iff `start <= ev.time <= end`, which is 121 distinct journal seconds. The second `T - 60` is news-priced; `T - 61` is base; `T + 60` is news-priced; `T + 61` is base. Reason: "from 60 s before to 60 s after" reads as closed, and the whole design treats touching as a hit (DESIGN 2.2, "touching counts"). The tested value is the event `time` (the second the evaluator executed it in; for a triggered order and a breach/fail close that is the trigger or decision sample's second), never the proof's own timestamp and never the block time.

### 2.3 New instructions

| Instruction | Args | Accounts | Signers | Checks |
|---|---|---|---|---|
| `publish_news_window` | `news_time: i64`, `market_mask: u128` | `AdminAction`: config [W], admin_a, admin_b, event_authority [R], program [R] | either admin (`either_admin()`) | `start = news_time - 60`; `now + 3,600 <= start` (`NewsWindowTooSoon`); `start <= now + 1,209,600` (`NewsWindowTooFar`); `market_mask != 0` and no bit at or above `config.market_count` (`NewsWindowInvalid`); no live slot with the same `news_time` and `market_mask` (`NewsWindowDuplicate`); a free slot = empty (`market_mask == 0`) or expired (`news_time + 60 + 604,800 < now`), else `NewsCalendarFull`. Taking an expired slot raises `news_retired_before = max(old, news_time + 60)`. Emits `NewsWindowPublished`. |
| `remove_news_window` | `slot: u8`, `news_time: i64` | the same | either admin | slot < 64, the slot holds a window with that `news_time` (`NewsWindowNotFound`); `now + 1,800 <= start` (`NewsWindowLocked`). Sets `market_mask = 0, news_time = 0`. Emits `NewsWindowRemoved`. |

Wire sizes: `publish` 8 (discriminator) + 8 + 16 = 32 bytes of instruction data, `remove` 8 + 1 + 8 = 17. A transaction with three signatures (fee payer + both admin slots, like `set_evaluator`, 510 bytes, 31,434 CU measured) is about 500 bytes for one publish; each further `publish` in the same transaction adds about 40 bytes (program index, 6 account indexes, length, 32 bytes of data) and about 34,000 CU (it includes the `emit_cpi!`, 14,600 CU, decision 67), so a weekly calendar is 2 to 4 transactions.

### 2.4 Error codes (next free after 6110)

| Code | Name | When |
|---|---|---|
| 6111 | `NewsWindowTooSoon` | publish with `start < now + 3,600` |
| 6112 | `NewsWindowTooFar` | publish with `start > now + 1,209,600` |
| 6113 | `NewsWindowInvalid` | empty mask, a bit for a market that does not exist, `news_time` that overflows |
| 6114 | `NewsWindowDuplicate` | the same `news_time` and `market_mask` already live |
| 6115 | `NewsCalendarFull` | no empty or expired slot |
| 6116 | `NewsWindowNotFound` | remove: empty slot or another `news_time` |
| 6117 | `NewsWindowLocked` | remove with `now + 1,800 > start` (started or too close) |
| 6118 | `NewsCalendarExpired` | `Fill`/`Close` with `ev.time <= news_retired_before` (I-14) |

A fill priced with the wrong spread is the EXISTING `EventPriceMismatch` (6055) in both directions (base inside a window, news outside): the program never prices silently with the wrong spread.

### 2.5 Events (`emit_cpi!`, as the other admin events since decision 67)

```
NewsWindowPublished { slot: u8, news_time: i64, market_mask: u128 }   // 25 bytes + discriminators
NewsWindowRemoved   { slot: u8, news_time: i64, market_mask: u128 }   // 25 bytes
```

### 2.6 Invariants (next free after I-12)

| Id | Invariant |
|---|---|
| I-13 | Every `Fill` and `Close` carries the price `engine::fill_price_e12`/`close_price_e12` of the proof's mid with the half-spread `h x 10` if `ev.time` lies in a live window whose `market_mask` has the market's bit, else `h` (`h` = the account's rule set `spread_e6[market]`). Windows never add up: the multiplier is 10 whatever the number of windows that contain `ev.time`. |
| I-14 | A `Fill` or `Close` with `ev.time <= config.news_retired_before` is refused (`NewsCalendarExpired`): the calendar of that second may have been recycled, and the program never prices with a calendar it no longer holds. Publishing and removing obey the lead, cutoff, capacity and signer rules of 2.3. |

### 2.7 Who-may-call rows (handover/audit/02-who-may-call.md: 26 rows -> 28, 208 cells -> 224)

| instruction | rule | trader | evaluator | SL8 alone | Rov alone | both | random key | authority PDA | no signer |
|---|---|---|---|---|---|---|---|---|---|
| `publish_news_window` | 6.1/14: either admin | refused 6000 | refused 6000 | ALLOWED | ALLOWED | ALLOWED | refused 6000 | refused 6000 | refused 6000 |
| `remove_news_window` | 6.1/14: either admin | refused 6000 | refused 6000 | ALLOWED | ALLOWED | ALLOWED | refused 6000 | refused 6000 | refused 6000 |

The evaluator can NEVER publish: that is what makes "the evaluator cannot declare news by itself" true.

## 3. Evaluator and verifier

### 3.1 Evaluator

How it learns the calendar: at start it reads the config account (the tail) and then follows `NewsWindowPublished`/`NewsWindowRemoved` in the same inner-instruction indexing path as `MarketFeedChanged`/`MarketUpdated`. It keeps `calendar_synced_until = (block time of the newest chain state it has read) + 1,800`.

The rule that makes a stale calendar impossible to use silently: an event at time t can only be affected by a window with `start <= t`. A publish at block time b needs `start >= b + 3,600`, so it affects t only if `b <= t - 3,600`; a removal at block time b needs `start >= b + 1,800`, so it affects t only if `b <= t - 1,800`. Hence a change made after the evaluator's last read (block time c, so b > c) cannot affect any event with `t <= c + 1,800`. So an event at time t may be priced only if `t <= calendar_synced_until`. If the evaluator is behind (RPC lag, outage), it stops executing market orders and stops posting priced events and counts `Stats::calendar_stale`; it never prices with a calendar it may not have.

How it prices: `exec_quotes(cfg, mid, t)` at the execution second, as listed in 1.3. A market order uses the newest sample at or before T as before (decision 68), but the SPREAD is the one of T. A triggered limit/stop order, a stop-loss or take-profit uses the pair of the trigger sample's second for the trigger test AND for the fill, so a limit never fills worse than its level and the price of the fill is the price that triggered it (choice 5 in section 8). The forced closes of a breach/fail decision use, per market, the decision's mark mid with the spread of the decision's second.

A fill during the window: nothing special, one `Fill` + one `Fee{Commission}` (the commission is computed on the news price) = the same two events; event counts do not change, so batching does not change.

A window appearing mid-batch: impossible to matter. A publish lands at least 3,600 s before its first priced second, so a batch whose events are dated before that cannot be affected, and the `synced_until` rule covers removals. A batch that STRADDLES the start or the end is priced event by event (each event has its own time); the evaluator builds each event's price when it creates the event.

Breach and fail interplay: the floor decision uses base marks. The forced closes of the decision execute at news prices, which lowers `b` below the decision's equity by up to `9 bp x leverage`. The evaluator already books the closes with the engine before choosing between `Breach`, `Fail{StaticFloor}` and `Fail{PhaseQuick}` (verifier D4); with news prices a breach decided at base marks can end at or below the static floor and then becomes a `Fail{StaticFloor}` (`BreachBelowStaticFloor` 6077 would reject a `Breach` posted for it). That is correct and must have a test.

### 3.2 Verifier

New log lines (program-wide, no `account` field, like `market_feed_changed`; handover/verifier-formats.md section 1):

| type | fields | mirrors |
|---|---|---|
| `news_window_published` | slot, news_time, market_mask (hex string of the u128), time (block time) | `NewsWindowPublished` |
| `news_window_removed` | slot, news_time, market_mask (hex), time (block time) | `NewsWindowRemoved` |

The replay starts with an empty calendar and applies the lines at their file position (like the market change lines). A publish whose `start < time + 3,600`, a remove whose `start - time < 1,800`, a remove of a slot that does not hold that window, or a duplicate is `StateMismatch` (the program refuses all of them). Because a window can be published at most 14 days ahead and kept 7 days after its end, the indexer must include every calendar line from 15 days before the account's `opened` time on. `--snapshot` (the whole log) additionally compares the live windows of the replayed calendar with the tail of the config account: with it the verdict is ANCHORED for the calendar too; without it `NOT ANCHORED TO THE CHAIN` already covers it.

New finding class, appended at the END of the list after `FalseFailMarks`: `NewsSpreadMismatch` (an executed event priced with the base spread inside a live window, with the news spread outside any window, or with the news spread for a market not in the window's mask). The generic `PriceMismatch` stays for every other wrong price. The class carries the second, the market, the window used (or none) and both expected prices.

Trust-matrix rows added (WHO = BOTH/PROGRAM/VERIFIER/NEITHER, with the reason):

| class | who | program | verifier | reason |
|---|---|---|---|---|
| fill or close priced with the base spread inside a live window (evaluator omits a window) | BOTH | `EventPriceMismatch` 6055 | `NewsSpreadMismatch` | the program re-derives the spread from the calendar it holds; the verifier replays the same lines |
| fill or close priced with the news spread outside every window, or for a market not in the mask (fake window to hurt one trader) | BOTH | 6055 | `NewsSpreadMismatch` | same |
| a published window that the real calendar does not contain (a compromised admin) | NEITHER | accepts | VALID | the chain has no source of truth for news; limited by the 1-hour lead, the cap of 64 slots, removal by either admin, and a watchdog alert (section 6) |
| a window nobody published (unscheduled news) | NEITHER | - | - | by design: not covered |
| publish inside the lead time / beyond 14 days / bad mask / duplicate / full calendar | PROGRAM | 6111 / 6112 / 6113 / 6114 / 6115 | - | never reaches an event, so the log has nothing for the verifier |
| remove a started or about-to-start window | PROGRAM | 6117 | - | same |
| a priced event older than the calendar horizon | PROGRAM | 6118 | - | the verifier replays the full log and could price it, but the program refuses it first |
| calendar lines dropped from the indexed log | VERIFIER | - | `NewsSpreadMismatch` on honest data or a `--snapshot` calendar mismatch | the program accepted honest news-priced fills; the verifier only sees them if the lines are there (the F-36 anchoring caveat) |

## 4. Compute and transaction size

### 4.1 Where the numbers come from

`handover/cu-runtime.json` (102 shapes on Agave 4.0.2, three runs each, a 400,000 limit): `post_journal` of 8 fee events costs 161,294 CU and of 1 event 42,675, a slope of 16,945 CU per event (each event is an `emit_cpi!` self-CPI, 12,539 CU on this runtime). The calendar lookup is plain arithmetic: 64 slots x about 14 BPF instructions (load `news_time`, two compares, branch, loop) at about 1 CU per instruction is about 900 to 1,000 CU per lookup, 6% of one event. A batch has at most 8 events, so at most 8 priced events; one lookup per distinct (market, second) is enough. The raw read of the 1,536-byte tail is a slice copy. Model used below for every limit: `measured + 2,000 (the tail read, generous) + 1,000 x events` of the batch (every event could be a priced one), so +10,000 at 8 events; the real measurement is a step of the implementation (section 7), and the 20 % margin of the limit formula comes on top.

### 4.2 Impact per shape (option (i), tail of the config account)

| Shape | measured CU (validator) | worst CU after | limit now | limit after | tx bytes (legacy, with limit instruction) |
|---|---|---|---|---|---|
| `post_journal`, 1 event, no proof | 42,675 | 42,675 (no priced event) | 52,000 | 52,000 | 480, unchanged |
| `post_journal`, 2 events, 1 proof (fill + close) | 64,798 | 68,798 | 78,000 | 83,000 | 640, unchanged |
| `post_journal`, 5 events, 3 proofs | 129,906 | 136,906 | 156,000 | 165,000 | 1,028, unchanged |
| `post_journal`, 8 events, 3 proofs (the worst) | 188,799 | 198,799 | 227,000 | 239,000 | 1,227, unchanged |
| `report_fail` static floor, 6 closes + Fail + Meta, 3 proofs, V0 | 224,440 | 234,440 | 270,000 | 282,000 | 1,106 (V0), unchanged |
| `report_fail` quick limit, 6 closes, 8 events, V0 | 221,252 | 231,252 | 266,000 | 278,000 | 1,106, unchanged |
| `report_fail` static floor, 3 closes, 3 fees, 8 events | 213,661 | 223,661 | 257,000 | 269,000 | 1,208, unchanged |
| `publish_news_window` / `remove_news_window` | n/a (new) | about 34,000 / about 33,000 | 400,000 not needed | default 200,000 | about 500 / 487 |
| any other instruction that loads config | unchanged | unchanged (the tail is never deserialized) | | | |

(`recommended_cu_limit` = measured worst x 1.20 rounded UP to 1,000: 198,799 x 1.2 = 238,559 -> 239,000; 234,440 x 1.2 = 281,328 -> 282,000.) The cap of 1,400,000 is never close; the default 200,000 was already insufficient for these shapes. NO shape exceeds its limit, the cap or 1,232 bytes. No forced close-all becomes unpostable: event counts per Fill/Close do not change (a Fill is 2 events, a Close 2, a flip 4), bytes do not change, and the largest group limit stays under 300,000. The `Stats::unpostable == 0` campaigns must be re-run (section 7).

### 4.3 D6: where the calendar is stored (the four ways)

Account sizes and rent use `(128 + bytes) x 6,960 lamports` (DESIGN 5).

| | (i-b) tail of `SectorConfig` (recommended) | (i-a) typed array in `SectorConfig` | (ii) one `NewsWindow` PDA per window | (iii) one `NewsCalendar` PDA |
|---|---|---|---|---|
| bytes / rent | 270 -> 1,806; 13,460,640 lamports = 0.01346064 SOL (+0.0107 SOL once) | same size | about 40 B each (8 + 8 + 16 + bump, rounded): 1,169,280 lamports = 0.00116928 SOL each, 64 of them 0.0748 SOL (returnable) | 8 + 8 + 1 + 1,536 = about 1,553 B: 11,699,760 lamports = 0.01169976 SOL, plus the config unchanged at 270 |
| extra accounts in `post_journal`/`report_fail` | 0 | 0 | 0 to k (as many as the evaluator chooses to pass) | 1 |
| tx bytes against 1,232 (legacy) | +0 | +0 | +33 per account | +33 (32-byte key + 1 index) |
| with the address lookup table of decision 71 | +0 | +0 | +1 to +2 per account if in the table | +2 if the calendar PDA is in the table; the table must be extended once |
| added CU | about 1,000 per lookup, +0 for every other instruction | the same + about 3,800 on EVERY instruction that loads config (Borsh of 64 x (i64 + u128)) | one account load (about 1,500) per window + lookups | one account load (about 1,500) + lookups |
| wrong or stale account | impossible: it is the config PDA, already seeds-checked | same | the evaluator can OMIT a window: the program cannot see an account that was not passed; fails the requirement "evaluator omitting a window" unless an index of windows is kept in config (which is option (i) again) | a wrong key fails the seeds constraint (loud, `ConstraintSeeds` 2006); stale is impossible (a PDA is the current state) |
| layout / derivation | config PDA `["config"]`; tail at byte offset 270: `[news_time i64 LE][market_mask u128 LE]` x 64; `news_retired_before` inside `reserved` | slots as a field | `["news", id u32 LE]` | `["news"]`, header `retired_before i64`, `bump u8`, then 64 slots |
| verdict | WINS: bytes are the scarce resource (5 of the 71 legacy shapes measured are within 33 bytes of the limit) | acceptable, but pays CU everywhere | REJECTED: silent omission | works, but 5 of 71 measured legacy `post_journal`/`report_fail` shapes would no longer fit 1,232 bytes: `post_journal` 8 events 3 proofs (1,227 -> 1,260) and four `report_fail` shapes of 1,208 to 1,220; the evaluator would split or use V0 for them, and the lookup table needs one more entry |

(i-b) works because Anchor's `Account<SectorConfig>` deserializes only the declared struct and its exit writes only the struct's bytes: the tail is untouched by every other instruction. That has to be a test (the tail is byte-identical after every instruction that writes config: `finalize_pass`, `set_*`).

## 5. D1 to D6, one by one

* **D1, the calendar on chain, published as `emit_cpi!` events: AGREE.** Evidence: both `Fill` and `Close` arms already recompute the price from the proof's mid and the rule set's spread (`post_journal.rs:264-305`) and refuse a different price with `EventPriceMismatch`; the calendar only chooses which spread. Admin events moved to `emit_cpi!` in decision 67 (F-27), so the verifier sees them as it sees the market lines. The evaluator has no instruction that writes the calendar.
* **D2, anti-retroactivity: AGREE, with three changes.** Lead 3,600 s as the default; the DURATION cap is not needed because the window length is a constant of the program (121 s), there is no duration argument to abuse; the number of windows is capped at 64 slots; a removal needs a cutoff (1,800 s before the start), not just "before the start", so that an evaluator that is a few minutes behind can never price a second with a window that was removed (3.1); the future is capped at 14 days. Who may publish: EITHER admin, remove: EITHER admin. Reason: the harm of a hostile publish is bounded and visible (at most 64 windows x 121 s, announced at least 1 hour ahead, removable by the other admin, 9 bp x leverage extra per execution: 4.5 % of a 50x account per side, 1.8 % at 20x), and it is the same trust as `update_market` (decision 24, DESIGN 14: either admin can already re-point a feed, which is more dangerous). Both-admins would put a 2-of-2 hardware-wallet ceremony on every weekly calendar and push the calendar toward a hot key; a dedicated publisher key is the alternative (choice 3).
* **D3, granularity: a MARKET bitmask.** A class mask is 1 byte but cannot say "BoJ day: JP225 only" (SPY, QQQ, JP225, KR200 are all class 3) or "AAPL earnings" (ten stocks share class 5). A u128 mask (bit = market id, `MAX_MARKETS` = 128) is 16 bytes: slot 24 bytes against 9, 1,536 bytes of tail against 576, +0.0067 SOL rent. One window per market would need up to 21 slots per news second. Recommended: the market mask.
* **D4, the multiplier 10 is a protocol constant: AGREE.** No rule-set layout change is needed (the rule set stays 552 bytes, `create_ruleset` and the launch JSON keep their ABI). Rolling out a different multiplier later: a program upgrade (the 2-of-2 multisig) that changes `NEWS_MULTIPLIER` in `journal-types` and the program together with the evaluator and the verifier; because the program keeps no multiplier per window, the safe procedure is to ship it when no live or future window exists (or to remove and republish them), and to cut the verifier version at the upgrade slot. If Rovik wants the multiplier to change without an upgrade, a `multiplier u8` per slot (+1 byte, 64 bytes total) is the cheap alternative; not recommended for v1.
* **D5, marks keep the BASE spread: AGREE, and STOP for Rovik's decision (see 5.1).**
* **D6, storage: option (i-b)** (4.3).

### 5.1 D5 in numbers

The engine's marks DO use the half-spread: a long is marked at the bid `mid x (1 - h)`, a short at the ask (`decide.rs:209-211`, `eval.rs:363`; the verifier `mid_pair` in `walk.rs`). So the question is real. Two options:

* **A (recommended): marks, floors, breach, quick tally, holding fee, equity keep the base spread; the news spread applies to executions only.**
* **B: inside the window marks use the news spread too.**

Effect on a position opened at FULL leverage at the news ask and marked immediately, mid unchanged, in % of the account (entry spread 10 bp at news; mark 1 bp under A, 10 bp under B; the quote of an existing position is hit the same way under B):

| class (leverage) | launch markets | A: P right after the fill | B: P right after the fill | B: hit on an EXISTING full-leverage position when the window starts |
|---|---|---|---|---|
| 0 FX majors (50x) | none | -5.5 % | -10.0 % (past the 8 % static floor: instant fail) | -4.5 % |
| 2, 3 metals, indices (20x) | GOLD, SILVER, SPY, QQQ, JP225, KR200 | -2.2 % | -4.0 % | -1.8 % |
| 4 energy (10x) | WTIOIL, BRENTOIL | -1.1 % | -2.0 % | -0.9 % |
| 5, 6 stocks, crypto majors (5x) | 10 stocks, BTC, ETH, SOL | -0.55 % | -1.0 % | -0.45 % |
| 7 crypto alts (2x) | none | -0.22 % | -0.4 % | -0.18 % |

Under B an artifact alone can move equity by up to 4.5 % of the account for 121 seconds and back, which can breach a tight floor whose cushion is smaller than that (L_start is capped at R, so the cushion can be near zero) and can falsely start tight mode or raise `g_peak` (the ratchet never falls, so a peak made of spread artifact is permanent). Under A the cost is real and paid once, at execution; nothing is created by marking. A has one visible consequence: the account is marked at base spread right after a news-priced fill, so its first equity is 9 bp x leverage better than an exit would realize; the exit pays it. Recommendation: A. If Rovik wants B, the evaluator and the verifier change in the marks (a second pair for marks), the program does not.

Related consequence of the decision itself (not of A or B): a FULL-leverage round trip entirely inside a window costs 2 x 10 bp x leverage in spread plus 2 x 0.02 % x leverage in commission: class 2/3 (20x): 4.0 % + 0.8 % = 4.8 % of the account (the same round trip at the base spread: 0.4 % + 0.8 % = 1.2 %); class 4 (10x): 2.4 %; class 5/6 (5x): 1.2 %; class 0 (50x): 10 % + 2 % = 12 % (more than the 8 % floor). Class 0 has no market at launch; if one is added, the first fill of a 50x position inside a window nearly uses the floor under option A as well (E = 100 - 1.0 - 5.5 - 1.0 = 92.5 %).

## 6. Backend duties (not code)

* WHO publishes: one of the two admins, from their normal signer, in a WEEKLY batch (2 to 4 transactions of 5 to 18 `publish_news_window` each, 40 bytes and about 34,000 CU per instruction; set an explicit compute limit of 400,000 to 1,400,000 for the batch). The evaluator key never publishes. The backend prepares the unsigned transactions from the calendar and shows the admin what it will publish; the admin signs.
* HOW FAR AHEAD: the program minimum is 1 hour and the maximum 14 days. Policy: publish a week ahead every week, so a calendar line announced a few days earlier still has margin and an emergency add is still possible down to 1 hour.
* THE CALENDAR SOURCE must satisfy (no vendor named): scheduled macro releases (to the second, UTC) and central-bank decisions; machine-readable with a stable schema and a change history (revisions and cancellations are visible); a time that is the release time, not a rounded display time; coverage of crypto-specific scheduled events and of the exchange hours events the markets care about; reachable and payable without KYC, phone or face verification (crypto-native signup and payment); an archive back at least a year for the verifier's audits; and a SECOND independent source for cross-checking the same week.
* UNSCHEDULED NEWS IS NOT COVERED: an event nobody announced (a hack, a surprise statement) is priced at the base spread. The 1-hour lead makes an emergency window impossible by design.
* MONITORING: the backend compares the source calendar with the chain every 5 minutes. Alert level 1 when a source event with a market mapping starts in less than 180 minutes and no window with that `news_time` and mask is on chain; level 2 at 90 minutes (the last publish possible is 60 minutes before the window start, i.e. 61 minutes before T); alert on any window on chain that the source does not contain (a possible hostile publish: either admin removes it); alert when fewer than 8 free slots remain; alert when `calendar_synced_until` of the evaluator falls more than 600 s behind.

## 7. Attacks, tests, and what must be redone

### 7.1 Attacks and tests the audit must add

| Attack / edge | Expected |
|---|---|
| fake window to hurt one trader (compromised admin) | NEITHER; limited by the lead, the 64-slot cap, removal by the other admin, the alert |
| window spam (64 publishes) | `NewsCalendarFull` 6115 afterwards; harmless to traders; either admin removes |
| evaluator omits a window | fill priced at base inside a window: `EventPriceMismatch` 6055 and `NewsSpreadMismatch` |
| window edge seconds | `T-61` base, `T-60` news, `T+60` news, `T+61` base, hand prices |
| overlapping windows | 10x, never 100x (two windows contain the second; the hand price is the 10x price) |
| window during a closed market | no Fill (`MarketClosed`); a `Close` of an existing position is news-priced; hand case |
| window on an unpriced market (h = 0) | `MarketUnpriced` still refuses a Fill; a `Close` stays at 0 (10 x 0) |
| publish inside the lead time | `start = now + 3,599` refused 6111, `now + 3,600` accepted |
| remove a started or too-close window | `start - now = 1,799` refused 6117, `1,800` accepted, a started window refused |
| replayed publish | the same `(news_time, mask)` again: 6114 |
| bad mask | zero mask, a bit at `market_count` and above: 6113 |
| a batch that straddles the start and the end | one batch, events at `T-61, T-60, T+60, T+61`, each priced by its own second |
| quick-trade interplay | open at `T-70` (base), close at `T-30` (news): `quick` true (40 s), pnl from the news exit, `credited` half of it; and the opposite |
| same-second breach in a window | base-mark decision, news-priced forced closes: if the closes end at or below the static floor the batch must be `Fail{StaticFloor}`; `Breach` is refused 6077 |
| quick-limit failure with forced closes in a window | the closes carry no fee and are news-priced; `Fail.b_after` is the news-priced balance |
| calendar horizon | `ev.time <= news_retired_before` refused 6118 |
| trigger quotes (choice 5) | a limit buy in a window fills at `ask_news <= limit`, never above it |
| the tail survives other writers | the tail bytes are identical after `finalize_pass`, `set_limits`, `set_paused` |
| `init_config` size | account is 1,806 bytes, tail zero, a second `init_config` still refused |

Hand-written tests that come FIRST (before any implementation, from the numbers, not from the code): (1) the price vectors: mid 1.0800 (1,080,000,000,000 e12): base ask 1,080,108,000,000, news ask 1,081,080,000,000, base bid 1,079,892,000,000, news bid 1,078,920,000,000 (h = 100 and 1,000); (2) the window edges `T-61, T-60, T+60, T+61`; (3) the overlap; (4) the lead and cutoff boundaries (3,599/3,600 and 1,799/1,800); (5) the eight error codes; (6) the two who-may-call rows; (7) `EventPriceMismatch` in both directions; (8) the D4 interplay; (9) the golden file `news_prices.json` produced by an independent Python check (`check.py`), as for the other nine golden files.

### 7.2 Everything that must be redone after the program changes, in order

1. Approval of the design (Rovik) and the two points of section 0 / 8.
2. `journal-types/news.rs` + the golden file and its Python check; hand-written tests of 7.1 first.
3. The program: publish/remove, tail, `Env`, errors, events, `init_config`; unit tests; `scripts/build-test-so.sh` and `scripts/anchor-build-checked.sh`: BOTH program hashes change (localnet 43fdd935... and default ebbd0786...); a new freeze note in STATE.md, AUDIT_REPORT "Gates" and deploy-checklist. A program change also ends the "program frozen" condition: `git diff 64629c1 HEAD -- programs ...` is no longer empty by design.
4. The evaluator (calendar, `exec_quotes`, `synced_until`) and the verifier (lines, replay, finding class), each with its own tests; the trust matrix (new rows, `trust_matrix.rs`, one named test each) and verifier-formats.md.
5. The 10,000-path differential (LiteSVM, with random calendars and with `SETL8_LAUNCH_COSTS`) and the 100,000-path engine-only campaign: `Stats::unpostable` 0, event counts, largest limit and transaction.
6. Compute: `budget.rs` (LiteSVM ceilings of `post_journal`, `report_fail` and the two new instructions), then `devnet-rehearsal.sh selftest measure` on the validator for the new shapes -> `handover/cu-runtime.json` and `.md`, `compute_tables.rs`, `budget_runtime.rs`, `backend-compute-contract.md` (the 227,000 -> 239,000 and 270,000 -> 282,000 limits and the table); the "runtime model" tests fail until the table is regenerated.
7. The M12 audit report sections to redo: 2 (A-1 to A-18, plus new attacks), 3 (the who-may-call matrix: 28 rows; account hygiene: config size and tail; journal; state machine; arithmetic extremes: 10x of a u16; fuzz), 4 (findings), 6 (the NEITHER rows and the money bound), 8 (the auditor's ranked list: the calendar lines in `post_journal`), 11 (mutation), 12 (unsafe/panic/cast review of the raw tail read), 14 (gates).
8. Mutation runs: the program instructions (the 143 mutants plus the new `news.rs` and the changed lines of `post_journal.rs`), and `cargo mutants --in-diff` on the evaluator and verifier lines.
9. The devnet rehearsal (P23) again, with a story that publishes a window, trades inside it and verifies; the 3,600-second lead makes a real-time story impossible unless the scratch build lowers `NEWS_MIN_LEAD_SECS` (never deployed), as the rehearsal's scratch build already lowers the pass-delay minimum (decision 70).
10. Docs: DESIGN (5.3, 6.1, 4.4 I-13 and I-14, 12, 13, 14), trust-matrix.md, verifier-formats.md, deploy-checklist (the Config rent, the calendar duties), design-decisions 73 -> APPROVED.
11. Only then: THE INDEPENDENT HUMAN REVIEW. It must start AFTER this change, on the final program; a review of the current frozen program would be reviewing code that is about to change in exactly its trust boundary (`post_journal.rs` price checks, the first item of the auditor's list).

Is the vault fixture (`tests-rs/fixtures/core_vault.so`, sha256 42c62670...) affected? NO. The vault never sees a spread or a price; the program's CPIs into it (`deposit_fee`, `deposit_reset`, `record_activity`, `flag_trader_failed`, `request_payout`) and the `sector_authority` PDA keep their ABI, and the new instructions make no vault call. The program id and the registry entry are unchanged. Re-run `vault_boundary.rs`/`vault_layout_pin.rs` anyway (they exercise the new build against the fixture).

## 8. Choices where the design could go either way, and the recommendation

| # | Choice | Options | Recommendation |
|---|---|---|---|
| 1 | Window ends | both included (121 s) / half-open `[T-60, T+60)` (120 s) | both included: "touching counts" everywhere else |
| 2 | Granularity | market mask u128 (24 B/slot) / class mask u8 (9 B) | market mask: classes cannot separate JP225 from SPY or AAPL from MSFT |
| 3 | Who publishes | either admin / both / a dedicated `news_key` in config (set by both admins, 32 bytes from `reserved`) | either admin by hand, weekly; add a dedicated key only if the weekly signing becomes a burden |
| 4 | Removal | before the start / with a 1,800 s cutoff | cutoff: the evaluator can then never price with a removed window |
| 5 | Trigger quotes | triggers and fills on the news pair / triggers on base, fills on news | news pair: a limit never fills beyond its level and the price of a fill is the price that triggered it |
| 6 | The 10x rule against the oracle floor of P24b | pure `10 x h` / `min(10 x h, cap)` | a cap. With the owner base of 100 e6 the news half-spread is 1,000 e6 (10 bp). P24b will raise Hyperliquid-sourced bases where the oracle noise is larger (the oracle gaps in config/launch.md reach a median of 5.13 bp and a p95 of 10.72 bp; the floor formula is P24b's, so these are illustrations): a base of 397 e6 (KR200 at its median) gives 3,970 e6 = 39.7 bp per side = 7.9 % of a 20x account per side; at its p95 (1,072 e6) 10,720 e6 = 107 bp = 21.4 % per side, an instant fail. Proposed cap: 2,000 e6 (20 bp per side, 4 % of a 20x account) as `NEWS_HALF_SPREAD_CAP_E6`; Rovik's call, because it deviates from "10 x the normal half-spread" for the noisiest markets |
| 7 | Calendar storage | tail by raw offset / typed Borsh array | tail: no CU on other instructions; the typed array is acceptable (+3,800 CU on every config load) |
| 8 | Lead | 1,800 / 3,600 / 7,200 | 3,600: an hour to see and to remove, short enough for a calendar changed the same morning |
| 9 | Multiplier | program constant / one byte per slot | constant (D4) |
| 10 | `ENGINE_VERSION` | keep / bump | keep: no engine function changes; the program build, evaluator and verifier versions carry the change |
| 11 | Retention and horizon | 7 days + `news_retired_before` / never recycle | 7 days: equals the vault's idle limit; an older batch is refused loudly (I-14) |
| 12 | Marks (D5) | A base / B news | A; this is the one that needs Rovik |

## 9. Where DESIGN contradicts or is touched by the decision

* DESIGN 3.2 and decision 14: the spread is part of the per-account parameter snapshot ("publishing version N+1 never changes an account created under version N"). The calendar is the first admin-written parameter that changes the COST of accounts that already exist, with no new rule set version. It is bounded (lead 1 h, 121 s per window, 64 windows) and visible, but it breaks the snapshot principle and DESIGN 3.2/9.1/14 must say so. If Rovik wants the snapshot kept, the multiplier and the calendar would have to be part of the rule set, which is immutable, and the calendar cannot be.
* DESIGN 7.1 (execution price) and I-4/I-5: the price formula gains the window; 4.4 gains I-13 and I-14; 5.3 (`SectorConfig` 270 bytes) becomes 270 + a 1,536-byte tail; 6.1 gains two admin rows; 12 gains 6111 to 6118; 14 gains the row "publish/remove a news window: either admin, indirect cost impact, no money".
* DESIGN 1.3/13: the sentence "the chain uses the half-spread of the market of a Fill" now means the half-spread at the event second.
* The owner's base (100 e6) and the P24b floors interact with the 10x rule (choice 6).
* No contradiction with decision 24 (the admin levers) beyond the above; no admin key touches money.
