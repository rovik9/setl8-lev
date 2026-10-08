# setl8-lev-trading — DESIGN

Status: **DRAFT v2 FOR REVIEW (architecture C). Complete (sections 1 to 16). No program code is written until Rovik approves this file.**
Last updated: 2026-10-09. Companion files: `OPEN_QUESTIONS.md`, `BUILD_PLAN.md`, `../handover/design-decisions.md` (decisions 32 to 35 are the architecture change).

v1 of this document (fully on-chain keeper design, "option B") is kept in git commit `29221e7`. It stays available as a later hardening path; this file replaces it.

Sections marked CONFIRMED were restated to Rovik and confirmed. Sections marked ASSUMPTION
are my reading of an unstated edge; each one is also tracked in OPEN_QUESTIONS.md.

---

## 1. Scope, architecture, trust model

### 1.1 What this program is
`setl8-lev-trading` is a Solana sector program (Anchor 0.32.1) for Setl8's leveraged-trading challenge product.
A trader buys a challenge account (10 sizes, $2.5K to $200K), trades 82+ **synthetic markets** priced by Stork,
and passes phases for fixed payouts. There is **no matching engine, no order book and no token custody on-chain**.
**The trading itself runs on a Setl8 backend (the "evaluator").** Solana holds three things only:
1. the **journal**: every fill, close and floor change of every account, with the signed Stork price that decided it, hash-chained;
2. the **gate** to money: purchase and reset fees in, payout claims out, through the core vault by CPI, with a 2-day delay and a veto on payouts;
3. the **limits**: a fixed payout formula, a daily payout cap, a fixed set of phases.

### 1.2 Components
```
 trader ──signed orders──► EVALUATOR (Setl8 backend, hot key)           PUBLIC VERIFIER (anyone, open source)
                             │  netting, floors, SL/TP, fees, orders        ▲ replays the journal with the same
                             │  uses Stork prices, keeps all price samples  │ `engine` crate, checks every proof
                             │                                              │
                             ├─ post_journal / report_fail / touch_activity ▼
 trader ─open/reset/request──►  setl8-lev-trading (Solana program)  ──CPI, signed by sector_authority PDA──► core vault
 anyone ──finalize_pass────►    journal hash chain, PendingPass, daily cap     (deposit_fee, deposit_reset, record_activity,
 both admins ──veto_pass───►                                                    request_payout, flag_trader_failed)
```
- `sector_authority` = PDA `["setl8_sector_authority"]` under THIS program id; it signs every vault CPI with `invoke_signed`.
- This program never holds trader or payout money. Token movement happens inside the vault. The one exception is the 0.4% swap-cost fee
  (section 9.2), a plain SPL transfer signed by the trader straight to the SL8 wallet.
- The `engine` crate (Rust, section 16) is the single definition of the rules in sections 2, 7 and 9. The evaluator runs it, the public verifier runs it, and the
  program links the small part of it that re-checks arithmetic (section 4.4).

### 1.3 Trust model (who must be trusted for what) — READ THIS ONE
| Party | Trusted for | Not trusted for |
|---|---|---|
| Stork publishers | The signed price and its timestamp | Nothing else |
| Core vault | Holds money; caps exposure (8% of size per pass, max 4 passes per record = 32%) | n/a |
| **Evaluator (backend + its hot key)** | **The truth of every trading outcome**: fills, floors, breaches, the pass condition. The chain cannot recompute the price path. | Moving money directly (it has no payout power); changing config |
| This program's code and its upgrade authority (2-of-2: SL8 admin + Rov admin) | Everything below | n/a |
| Admins (both together) | Vetoing a pending pass (requested or confirmed); rule sets; limits; rotating the evaluator | Moving or approving money: no instruction does it |
| Either admin alone | Markets and feeds (**EXCEPTION, decision 24**), pausing | Same |
| Traders | Their own signed orders and their own wallet transactions | The evaluation of their trades |
| Anyone | Nothing | Can call `finalize_pass` after the delay, and can replay the journal |

**Plain statement of the risk (decision 32).** Under option C a wrong or hacked evaluator can write a false history. It can fail a clean account, miss a breach, or fabricate a pass.
The program does not hide this. It bounds the damage and makes it visible:
- every pass is a public, replayable record, and a payout cannot leave before **2 days** have passed and **both admins** have had the chance to veto;
- total payouts are throttled by `max_payout_per_window`, and by the vault (32% of size per record chain);
- the evaluator key has no payout power and no config power;
- the journal carries the signed Stork price behind every decisive fill and trigger, and a commitment to the full sample file, so a third party can detect a lie.
What C cannot do: prove a breach that the evaluator left out, or stop the evaluator from failing an honest trader (section 4.7, 13).

### 1.4 Facts this design is built on (verified in the read-only repos, 2026-10-09)
- `setl8-shared-interfaces` is at **v0.4.0** (the earlier note of v0.3.2 was stale). `request_payout` queues a claim and needs
  7 accounts; `flag_trader_failed` takes the registry read-only. Vault commit `be97396` does not yet enforce the payout tally,
  but the sector must maintain it from day one (`PayoutTally`, section 10.5).
- Vault `INACTIVITY_LIMIT_SECS` = 604,800 (7 days), `ACTIVITY_THROTTLE_SECS` = 86,400, `MAX_RESET_PHASES` = 8.
- Vault `deposit_reset` today prices a reset as a percent of account SIZE. The locked ladder (decision 17) is a percent of
  tier PRICE. **Vault change required** (prompt in `handover/vault-change-reset-price.md`). BUILD-BLOCKING for the reset module only.
- Stork's Solana feed account stores only the latest value (`timestamp_ns: u64`, `quantized_value: i128`, 18 decimals). Neither
  Stork nor Pyth provides verified high/low/OHLC. The evaluator therefore must record its own sample stream (section 4.5).
- The vault pays queued claims through a permissionless heartbeat (about every 5 days, pro rata). Total time from "button" to money is 2 days (ours) plus up to 5 days (vault).

### 1.5 Locked decisions this document implements
Numbers refer to `handover/design-decisions.md`: rules 1 to 12; leverage 13; prices 15; 0.4% fee 16; reset ladder 17 and
vault change 18; 12 h lock 19; margin 20; closed markets 22; holding fee 23 and 26; admin exception 24; vault pause 25;
order types 27; 5 accounts per wallet 28; 2-of-2 upgrade authority 30; tight floor cap 31; **architecture C 32; delay and veto 33;
daily cap 34; evaluator powers 35**. Decision 21 (keepers and bounty) is **superseded** and no longer part of the design.

### 1.6 What the chain enforces versus what it only records
| Rule | On-chain | Off-chain only (verifier can check) |
|---|---|---|
| Journal sequence, hash chain, time never goes backwards | ENFORCED | |
| P&L formula of each closed slice, quick-trade half credit, fee debits, balance continuity | ENFORCED (integer re-computation) | |
| Quick tally Q and the phase-fail threshold 0.028 S | ENFORCED | |
| Static floor latch (B >= 1.04 S) and the static-floor fail rule when flat | ENFORCED | |
| Tight-floor start and ratchet arithmetic (`l_start`, `l_level`) and "breach only when balance <= level" | ENFORCED (arithmetic on claimed values) | The claimed open profit and peak are not provable |
| 12 h lock after a breach (no new opens in the journal) | ENFORCED | |
| Payout amount (0.08 S), once per phase, phases 2 to 5, 2-day delay, daily cap, veto | ENFORCED | |
| Phase pass condition: B >= 1.08 S, flat, no open pass | ENFORCED on the chain-tracked B; "flat" is the evaluator's assertion | Flatness against the real positions |
| Prices used are the real Stork prices; samples were complete; no breach was omitted | not provable | Verifier replays proofs and sample file |
| Fills match what the trader actually ordered | not provable | Trader-signed order intents (section 4.5) |

---

## 2. Challenge rules (CONFIRMED by Rovik, 2026-10-09)

Notation (S = account size, e.g. $100,000). All money is 6-decimal USDC base units (u64/i64/i128 as
specified later). Examples below use dollars.

### 2.1 Balance and equity
- **B (closed balance):** starts at S each phase. A closed slice with profit P adds P if it is a normal
  slice and P/2 if it is a quick slice (rounded DOWN, i.e. against the trader). A loss is always credited in full.
- **P (open profit):** sum of unrealized PnL over all open net positions (one per market), marked at the signed Stork prices the evaluator samples (section 4.5).
- **E (equity) = B + P.**

### 2.2 Floors
| Floor | Level | Effect when equity touches it (E <= floor) |
|---|---|---|
| Static floor F_s | 0.92 S, and permanently 0.96 S once B >= 1.04 S has ever been true (latched) | **Account FAILED** (vault `flag_trader_failed`) |
| Tight floor L | only while "tight mode" is on, see 2.3 | **Close all positions at that sample's price, lock account 12 h.** Phase is NOT failed. |

- Touching counts as a hit (E <= level, not E < level).
- If one sample shows E <= F_s, the account is FAILED even when tight mode was on and L was skipped by a gap.
- After the latch, F_s stays 0.96 S even if B later falls below 1.04 S.

### 2.3 Tight mode
- **Starts** at the first accepted sample where P > 0.01 S (strictly more than 1% of S, all markets combined).
  At start we record R = B (closed balance at that instant) and **L_start = min(F_s(at that moment) + 0.04 S, R)** (decision 31: the tight floor is never set above the closed balance, so a trader close to the static floor is not breached by the very sample that made him a winner).
- **While on**, G = E - R (profit since the mode began, realized and open; closing a winner does not reduce G).
  G_peak = highest G over accepted samples since the mode began.
  **L = L_start + max(0, G_peak - 0.01 S).** L only rises. It does not fall when P drops below 1% or goes negative.
- **Ends** only when every position is closed (flat). The next time P > 0.01 S a fresh mode starts.
- ASSUMPTION A-RULE-1: L is a frozen ratchet level. If F_s latches to 0.96 S in the middle of a mode (by realizing
  profit), L is NOT recomputed from the new F_s (otherwise realizing profit could instantly breach). The effective
  floor is max(F_s, L). Confirm with Rovik.
- **A-RULE-3 RESOLVED (decision 31, Rovik 2026-10-09):** L_start is capped at R. Example: B = $93K, F_s = $92K, P = +$1.1K: L_start = min($96K, $93K) = $93K, so L = $93K + max(0, 1.1K - 1K) = $93.1K, not $96K. With B >= F_s + 0.04 S the cap does nothing (all earlier examples).

### 2.4 Quick trades
- A closing slice (partial or full) is **quick** if (close time - position timer start) <= 45 s.
- Position timer: starts when a net position opens from flat; **restarts on every same-direction add**;
  a partial reduction does not reset it; a flip closes the old position and the remainder opens a new position with a fresh timer.
- Credit: a quick slice with profit adds half the profit to B (rounded down). Quick losses are credited in full.
- **Quick tally Q** = sum of the credited half-profits of profitable quick slices. Quick losses never reduce Q.
- **Phase FAILS immediately** when Q > 0.35 x 0.08 S = **0.028 S** ($2,800 on $100k, i.e. up to $5,600 raw quick profit).
  ASSUMPTION A-RULE-2: "crosses" means strictly greater than, checked at the moment the quick slice closes, not at phase pass.

### 2.5 Phases and payouts
| Phase | Target | Payout on pass |
|---|---|---|
| 1 | B >= 1.08 S | none (evaluation) |
| 2, 3, 4, 5 | B >= 1.08 S | exactly 0.08 S per pass ($8,000 on $100k). Profit above 8% is forfeited |

- **Pass condition (checked when the trader presses "request payout and move to next phase"):** B >= 1.08 S and no open positions
  and the phase is not failed. Until pressed, the trader may keep trading; all floors and the quick rule still apply.
- **Pressing the button** (`request_advance`) starts the move to the next phase. Phase 1 to 2 (no money) happens at once. For phases 2 to 5 it creates a **pending pass**:
  the account is frozen for trading for the 2-day delay, either both admins veto it or anyone finalizes it, and only then (`finalize_pass`) the vault payout claim of 0.08 S is queued and
  the next phase starts: fresh B = S, F_s = 0.92 S, latch cleared, tight mode off, Q = 0 (decisions 33, section 10).
- After the phase 5 payout claim is queued the account is Graduated (vault `max_payout_count` = 4).
- No daily drawdown rule.

### 2.6 Worked example R1 (S = $100,000)
| Step | P (open) | E | Tight mode | L | Result |
|---|---|---|---|---|---|
| t1 | +0.6k | 100.6k | off | n/a | F_s = 92k |
| t2 | +1.2k | 101.2k | **on**: R = 100k, L_start = 96k, G = 1.2k | 96k + max(0, 1.2k - 1k) = 96.2k | |
| t3 | +3.0k | 103.0k | on, G_peak = 3.0k | 96k + 2.0k = 98.0k | L raised |
| t4 | +1.0k | 101.0k | on | stays 98.0k | below 1% but L does not fall |
| t5 | -2.0k | 98.0k | on | 98.0k | E <= L: **breach**. Close all at this sample price, B = 98.0k, locked 12 h, tight mode ends |

### 2.7 Worked example R2: permanent shift
B goes 100k -> 104.0k (closed): F_s becomes 96k forever. Later B = 101k: F_s stays 96k; a sample with E <= 96k fails the account.

### 2.8 Worked example R3: quick credit
Quick slice with +$1,000 raw profit: B += $500, Q += $500. Another quick slice with -$400: B -= $400, Q unchanged.
Phase fails once Q > $2,800.

---

## 3. Markets, price source, sessions

### 3.1 Market classes and leverage (CONFIRMED, decision 13)
| Class id | Class | Max leverage |
|---|---|---|
| 0 | FX majors | 1:50 |
| 1 | FX minors / exotics | 1:20 |
| 2 | Metals | 1:20 |
| 3 | Indices | 1:20 |
| 4 | Energy | 1:10 |
| 5 | Stocks | 1:5 |
| 6 | Crypto majors | 1:5 |
| 7 | Crypto alts | 1:2 |

Leverage means required margin = notional / leverage. It is a per-class value stored in the account's **rule set** (3.2), so a later
change never touches an existing account. The market list itself (82+ markets, class of each) is supplied by Rovik later; the
program takes it as data (`create_market`), nothing market-specific is hardcoded.


### 3.2 Accounts that describe the market (all admin-written, all read-only for traders)
- **`SectorConfig`** (singleton, PDA `["config"]`): admin keys, **evaluator key**, vault program id, SL8 wallet, `current_ruleset`, `market_count`, `paused`,
  `pass_delay_secs`, `max_payout_per_window`, `window_secs` and the running window counters (section 10.6).
- **`RuleSet`** (PDA `["ruleset", version u16]`, **immutable once created**): leverage per class, half-spread per market,
  commission per class (pending numbers), holding-fee rate per class, tier price table, reset ladder, fee rate. A `TradingAccount`
  stores its `ruleset_version` at purchase. This is the "per-account parameter snapshot": publishing version N+1 never changes an
  account created under version N. The evaluator reads the rule set of each account's version; the chain does not use most of it (it uses the tier table, ladder, fee rate and the constants of section 2).
- **`Market`** (PDA `["market", market_id u16]`): class, `status` (0 disabled, 1 enabled, 2 reduce-only), `feed_id [u8;32]`
  (**either admin can change it at any time, decision 24**), `max_age_secs`, quantity step and minimum, `max_notional_usd` (per-position
  cap, see OPEN_QUESTIONS), up to 6 weekly session windows, optional `quote_conv_market` (see 3.5). The chain uses a market only to check that a journal event
  names an existing market and that a price proof carries that market's current `feed_id`. Changing `feed_id` emits `MarketFeedChanged{old,new,slot}`.

Rate units: all rates are fixed-point with 6 decimals ("e6") of the notional. Example: holding fee 0.10% per night = 1,000 e6.

### 3.3 Price source and the `PriceProof`
**Where prices come from.** The evaluator subscribes to Stork's signed price stream (Stork's own API/WebSocket; the Solana feed account holds only the latest value, so it is not the source).
Each update it receives is a signed message: feed id, timestamp (ns), value (18 decimals), publisher signature(s). The evaluator:
1. keeps **every update it uses** in an append-only sample file per market (`samples-<market>-<date>.bin`), and publishes the file hash in each journal batch (4.5);
2. converts to `price_e12 = floor(quantized_value / 10^6)` and applies the half-spread (section 7.1);
3. attaches to every **decisive** event the original signed update as a `PriceProof` (format in section 8.2).

**What the chain does with a `PriceProof` (v1):** checks that `feed_id` equals the market's current feed, that `price_e12` equals the proof's own value field, that the proof time is not in the future and not older than the market's `max_age_secs` before the event time,
and stores the proof in the event log. **It does NOT verify the publisher signature on-chain in v1**, because Stork's exact signed-message format and a cheap on-chain verification path are
not verified (UNVERIFIED, module M2). The signature is verified off-chain by the public verifier. Hardening paths, in order of cost: (a) `secp256k1_recover` check inside `post_journal` once the format is confirmed;
(b) a CPI to Stork's own verification instruction if one exists.

**Pyth fallback.** Only if a free, verified Pyth feed exists for a market that Stork lacks. Same `PriceProof` shape with a different `source` byte. Not built until verified.

**UNVERIFIED, must be checked against the real Stork documentation in module M2:** the exact signed-message layout, the size of one update on the wire (assumed 200 to 250 bytes),
whether non-enterprise customers can get the signed stream at all (contact-sales; OPEN_QUESTIONS #1), the price, and the per-market update frequency.

### 3.4 Sessions and closed markets (decision 22)
- Each market has up to 6 weekly windows `(start_minute_of_week, end_minute_of_week)` in UTC. `market_open(now)` = now falls inside a window
  AND the feed's `publish_time` is within `max_age_secs` of `now`. Holidays need no admin action: the feed goes stale, so the market reads as closed.
- **Closed market:** no new orders and no fills of pending orders; existing positions **may be held** with a frozen mark (the last sample); the evaluator does not trigger floors, SL, TP or pending orders on a frozen mark. The holding fee keeps accruing every calendar night (section 9.3).
- **Reopen:** the first fresh sample is the next price. If it is past an SL, a floor or a pending order, they trigger at that sample's price
  (a gap is a gap; the trader bears it, consistent with the confirmed rule "even through a gap").
- **Reduce-only status (2):** admin sets it before a feed migration or delisting; opens are rejected, closes allowed.
- **Index markets** (SPX, NDX, DAX, Nikkei, FTSE) are created with `status = 0` until a custom feed exists. Enabling one is an admin action.

### 3.5 Quote currency
P&L is in USD. A pair quoted in another currency (EURJPY quoted in JPY, DAX in EUR) needs a USD conversion. Design: `Market.quote_conv_market`
points to another market whose price is USD per one unit of the quote currency; every valuation of the first market uses a fresh sample of the second; both proofs are attached to the event. **Open question (OPEN_QUESTIONS):** simplest launch
is USD-quoted markets only; cross pairs and non-USD indices are added when the conversion feed exists.

---

## 4. How outcomes are checked when the backend trades

### 4.1 The problem, restated for option C
The rules in section 2 are about the **path**: "the first moment equity touched the floor", "SL hit", "open profit passed 1% and the floor switched".
Solana cannot watch ticks, so the path is evaluated off-chain by the evaluator. A dishonest or hacked evaluator could fail traders or save them.
This section says exactly what protects against that, and what does not.

### 4.2 What the oracle can and cannot prove (checked, 2026-10-09)
| Question | Answer | Consequence |
|---|---|---|
| Does Stork or Pyth give a verified high/low/OHLC for an interval? | **No.** Only point prices with timestamps. | The path is only observable through discrete samples. The evaluator's sampling density defines "the path". |
| Does the Stork Solana feed account keep history? | **No**, only the latest value. | Setl8 must keep its own sample archive; it is part of the product (4.5). |
| Is a point price verifiable? | Yes: publisher-signed. Checked by Stork's program when pushed on-chain. Our v1 journal does NOT verify it on-chain (3.3). | A third party can verify each proof off-chain. |

### 4.3 Who does what
| Step | Who | Where |
|---|---|---|
| Trader sends an order (market, limit, stop, SL/TP, close) with a wallet signature | Trader | to the evaluator API |
| Fill, netting, margin check, marks, floors, tight-floor ratchet, quick rule, holding fees, SL/TP/pending triggers | Evaluator, using the `engine` crate | off-chain |
| Keep every used price update in an append-only archive; keep every signed trader order | Evaluator | off-chain, published (4.5) |
| Post the events with proofs, in order | Evaluator (`post_journal`) | on-chain |
| Re-check arithmetic, balances, quick tally, fail thresholds, sequence, hash chain | Program | on-chain (4.4) |
| Replay the whole journal and archive; flag any mismatch | Public verifier / Setl8 watchdog | off-chain (4.6) |
| Press "request payout and move to next phase" | Trader (`request_advance`) | on-chain |
| Freeze the account's trading, bring the journal up to date, check flat and balance against the engine, then `confirm_pass` or `reject_pass` | Evaluator | off-chain check, on-chain call |
| Veto, within 2 days of the confirmation | Both admins together | on-chain |
| Finalize after the delay | Anyone (`finalize_pass`) | on-chain, vault CPI |

### 4.4 What the program re-checks on every `post_journal` (invariants I-1 to I-12)
Let `acct` be the stored account and `ev` an event of the batch, processed in order. Any failure reverts the whole batch.
| Id | Invariant |
|---|---|
| I-1 | Signer is `config.evaluator`. Account status is `Active`. `pass_pending != 2` (a confirmed pass freezes the journal; `pass_pending == 1` still allows posting so the evaluator can bring the journal up to date before confirming). |
| I-2 | `batch.first_seq == acct.seq + 1` and `batch.prev_hash == acct.journal_hash`; each event extends the hash (8.3). `acct.seq` and `acct.journal_hash` are updated at the end. |
| I-3 | Time: `ev.time >= acct.last_batch_time` (never backwards) and `ev.time <= now + 2 s`. |
| I-4 | Each referenced market exists, and every `PriceProof` carries that market's current `feed_id`, a value equal to the event price (3.3), and `ev.time - max_age_secs <= proof.time <= ev.time + 2 s` (a price older than the market's `max_age_secs` cannot justify an event). |
| I-5 | `Close`: `pnl == floor(sign x qty x (exit - entry) / DEN)` computed by the program; `quick == (ev.time - timer_start <= 45)`; `credited` is `pnl` for a loss or a non-quick slice, `floor(pnl/2)` for a quick profit (7.4); `b += credited`; if `quick && pnl > 0` then `q_tally += floor(pnl/2)`. |
| I-6 | `Fee`: `amount >= 0`; `b -= amount`. (Holding fee and commission, both rounded up by the engine.) |
| I-7 | After `b` changes: if `b >= 1.04 S` then `latch = 1`. The static floor is `0.96 S` if latched, else `0.92 S`. |
| I-8 | `q_tally > 0.028 S` after any event: the batch is rejected unless its last event is `Fail{PhaseQuick}`; use `report_fail`. |
| I-9 | `TightStart`: `p_open > 0.01 S`, `r == b`, `l_start == min(static floor + 0.04 S, r)`, `l_level == l_start + max(0, g_peak - 0.01 S)` and `tight_on == 0` before. `FloorRaised`: `tight_on == 1`, new `g_peak > old`, `l_level` recomputed by the formula. `TightEnd`: `flat == true`. |
| I-10 | `Breach`: `tight_on == 1`, `flat == true` after the batch, `b <= l_level` (position closed at the sample, so balance equals equity), `lock_until = ev.time + 12 h`, `tight_on = 0`. Any `Fill` that opens or adds has `ev.time >= acct.lock_until`. |
| I-11 | `Fail` (only through `report_fail`): reasons `StaticFloor` (needs `flat == true` and `b <= static floor`) or `PhaseQuick` (needs I-8). Sets status Failed and calls the vault. |
| I-12 | `flat` is stored as given. The chain cannot check it. |
The program also re-derives, never trusts: the static floor, the quick limit, the pass target, the payout amount. None of these are fields the evaluator can set.

### 4.5 Decisive samples, the sample archive and signed order intents
**Decisive samples** (each carries a `PriceProof` in the journal): every fill (open, add, reduce, close, flip), every SL/TP/limit/stop trigger, the tight-mode start sample,
every sample that raised `g_peak`, the breach sample, the fail sample. Ordinary mark-to-market samples are not posted.
**Sample archive.** The evaluator writes every price update it used, per market, to an append-only file and stores the SHA-256 of the file prefix used in each batch (`samples_hash`). Setl8 publishes the archive (object storage with a public URL, plus a copy
the admins keep). A verifier can recompute the path: given the account's fills (known from the journal) and the sample file it must reproduce every floor, ratchet and trigger the evaluator claims, and find any breach that the evaluator did not claim.
**Signed order intents.** Each trader order is signed by the trader wallet (ed25519, message `SL8LEV|account|nonce|order bytes`). The evaluator stores them and commits `intents_hash` per batch. A fill without a matching intent
(or SL/TP/limit/stop already on file) is visible to the verifier as a forged trade. On-chain verification of these signatures (ed25519 precompile via the instructions sysvar) is an optional hardening, not in v1.
**Honest limit.** If the evaluator drops a sample that would have caused a breach, nothing on-chain shows it; a verifier with its own copy of the Stork stream can. Setl8 should run an independent second recorder (watchdog) for this reason.

### 4.6 Public verifier and watchdog
- `setl8-verify` (a CLI, module M9): reads the program's event log (via any RPC/archive/indexer), the sample archive and the intents file; replays with the `engine` crate; prints a verdict per account, per phase, and per pending pass. Open source so traders can check their own accounts.
- Setl8 runs it as a **watchdog**: on every `PassRequested` event it replays that account and alerts both admins with `VALID` or `MISMATCH` inside the first hour, so the 2-day veto window has operational meaning (13.3, OPEN_QUESTIONS).

### 4.7 What happens if the evaluator is offline, wrong, or hacked
| Case | Effect | Protection |
|---|---|---|
| Evaluator offline | No new journal; traders cannot trade (the backend is the venue); passes cannot be requested if the last batch left the account non-flat; vault activity clocks may run (touch_activity stops) | `release_account` by the trader refreshes the vault clock; backend runbook. No on-chain recourse for a long outage (OPEN_QUESTIONS #10). |
| Evaluator bug miscomputes a floor or pnl | Either a wrong fail/breach or a wrong pass | I-5..I-10 catch arithmetic classes of error; verifier catches the rest; veto catches a wrong pass; a wrong fail has no automatic remedy (OPEN_QUESTIONS #9). |
| Key stolen: attacker fabricates a pass for his own accounts | He needs a `request_advance` from a trader wallet, then 2 days | Watchdog alerts, both admins veto, daily cap bounds the rest, vault caps at 32% of size per record chain. Rotation: both admins `set_evaluator`. |
| Key stolen: attacker fails honest traders | Possible: `report_fail` is valid if the batch satisfies I-11 (needs fabricated losses that bring `b <= static floor`) | Visible in the journal; no automatic remedy. Reset price is paid again by the trader (OPEN_QUESTIONS #9). |
| Key stolen: attacker stops journaling | Liveness only | Rotation. |
| Evaluator omits a breach | Trader keeps a position he should have lost; the verifier with an independent recorder finds it | Not preventable on-chain. A pass built on it can be vetoed. |

### 4.8 Sampling cadence
The evaluator values every account against the newest sample of each open market at least once per second of backend time and on every incoming Stork update for markets with open positions.
Floors are tested on every valuation. "Touching counts" (E <= level). Gap handling is as in 3.4: the first sample past a trigger fills at that sample's price. The archive makes the cadence auditable;
the cadence itself is a service setting, not a chain rule.

### 4.9 Worked example: +1.2%, tight floor switches on, price reverses through it
Setup: S = $100,000, phase 2, B = $100,000.000000, static floor $92,000, EURUSD (class 0, leverage 1:50), half-spread 0.003% (30 e6), no commission, no holding fee yet.
Trader sends a signed market buy of 1,000,000 EUR at 14:00:00 UTC. The evaluator fills at `ask = ceil(1.08000 x 1.00003)` = **1.0800324**. Notional = $1,080,032.40. Margin = notional / 50 = **$21,600.648**.
(All values were produced by integer math with the rules of section 7.4; module M3's test vectors must reproduce them exactly.)

| Time | Mid | Bid (mark) | P (open) | E | What the evaluator does (off-chain) |
|---|---|---|---|---|---|
| 14:00:00 | 1.08000 | 1.0799676 | -64.80 | 99,935.20 | Fill: qty +1,000,000 EUR @ 1.0800324, timer starts. Intent signed by trader stored. |
| 14:00:30 | 1.08060 | 1.080567582 | +535.18 | 100,535.18 | Mark only. P below 1% (1,000). |
| 14:01:00 | 1.08130 | 1.081267561 | +1,235.16 | 101,235.16 | **Tight mode ON** (P > 1,000): R = 100,000.000, `l_start = min(96,000, R) = 96,000`, G = 1,235.161, `l_level = 96,000 + 235.161 = 96,235.161`. |
| 14:01:30 | 1.08310 | 1.083067507 | +3,035.11 | 103,035.11 | New high: `g_peak = 3,035.107`, `l_level = 96,000 + 2,035.107 = 98,035.107`. |
| 14:02:00 | 1.08110 | 1.081067567 | +1,035.17 | 101,035.17 | Nothing falls: `g_peak` and `l_level` unchanged. |
| 14:02:30 | 1.07800 | 1.07796766 | -2,064.74 | 97,935.26 | `E = 97,935.26 <= l_level 98,035.107` -> **breach**. Close at the sample bid 1.07796766: pnl = -2,064.74, slice age 150 s so a normal slice, `B = 97,935.26`. Account flat, locked until 02:02:30 next day. |

**What lands on Solana (posted by the evaluator about 14:03, as three transactions because a signed proof is about 300 bytes in the journal format of 8.2 and a 1,232-byte transaction carries about two; sizes are estimates until M2):**
| Tx | Events (seq) | Proofs attached | What the program checks |
|---|---|---|---|
| A | `Fill`(1) buy 1,000,000 EUR @ 1.0800324; `TightStart`(2) `r=100,000`, `l_start=96,000`, `g_peak=1,235.161`, `l_level=96,235.161` | 2 (14:00:00, 14:01:00) | seq 1 and 2 continue the stored seq 0; I-9: `l_start = min(92,000 + 4,000, 100,000)`, `p_open 1,235.161 > 1,000`; hash chain. |
| B | `FloorRaised`(3) `g_peak=3,035.107`, `l_level=98,035.107` | 1 (14:01:30) | I-9: `l_level = 96,000 + (3,035.107 - 1,000)`; raised. |
| C | `Close`(4) qty 1,000,000 EUR, entry 1.0800324, exit 1.07796766, `pnl = -2,064.740000`, quick=false, credited = -2,064.74; `Breach`(5) `l_level=98,035.107`, `flat=true`, `lock_until = 14:02:30 + 12 h` | 1 (14:02:30) | I-5: `1e12 x (1.07796766e12 - 1.0800324e12) / 1e12 = -2.06474e9` micro-USD matches; `b = 97,935.26`; I-10: `b <= l_level`; lock set. No vault CPI (the phase is not failed). |

**If the evaluator had been late** with the 14:02:30 sample (next sample at mid 1.06000 at 14:02:40): `P = -20,064.20`, `E = 79,935.80 <= static floor 92,000` -> **FAIL**: the evaluator closes at that sample, `B = 79,935.80`, and calls `report_fail(StaticFloor)`.
The program checks `flat` and `b <= 92,000`, sets Failed, CPI `flag_trader_failed`, `active_count - 1`. The trader bears the gap, as the confirmed rule says.

### 4.10 Worked example: a pass, the delay and a veto (phase 3, S = $100,000)
| When | Event | Result |
|---|---|---|
| Day 0, 10:00 | Journal shows `b = 108,150.00`, `flat = true`, `tight_on = 0`, last batch 09:58. | |
| Day 0, 10:01 | Trader presses the button: `request_advance` | Program checks Active, `flat`, `b >= 108,000`, no pass pending. Writes `pass_pending = 1`, `pass_requested_at = 10:01`; emits `PassRequested{amount 8,000, b 108,150, seq, hash}`. Vault `record_activity` CPI. **No money moved.** The evaluator freezes the account's trading at once. |
| Day 0, 10:02 | Evaluator compares its engine state with the chain (flat, same `b`, no pending orders left) | If equal: `confirm_pass` -> `pass_pending = 2`, `pass_proposed_at = 10:02`, event `PassConfirmed`. If not (a position was open, or the journal lagged): `reject_pass(NotFlat)` -> `pass_pending = 0`, the trader is told and may press again after closing. If the evaluator stays silent for 1 hour the trader can `cancel_pass`. |
| Day 0, 10:20 | Watchdog replays the account | Posts `VALID` to the admins' channel. |
| Day 2, 10:02 | Delay over | Anyone calls `finalize_pass` (payer pays the `PayoutClaim` rent). Program checks the daily cap, then CPI `request_payout(8,000)`; on `Paid`: tally +1 / +8,000, `next_request_id + 1`, phase 4 starts fresh (`b = 100,000`, `q = 0`, latch 0, tight off). |
| Day 2 to Day 7 | Vault heartbeat | The vault pays the claim pro rata. |
| **Veto case:** Day 1, 16:00 | Watchdog posts `MISMATCH` | Both admins sign `veto_pass`. `pass_pending = 0`, `vetoes + 1`, event `PassVetoed`. The account unfreezes in the same phase with the same `b`. No money moved. The trader may request again. What happens next is policy, not code (OPEN_QUESTIONS #8). |

---

## 5. Accounts and byte sizes

Sizes include the 8-byte Anchor discriminator. Rent = (128 + bytes) x 6,960 lamports (rent-exempt minimum; the formula is Solana's current default, to be re-checked on the target cluster).
Under option C no account holds positions or orders, so every account is small and plain Borsh (no zero-copy needed; the stack budget in section 11 is still checked).
All money is `i64` micro-USD (6 decimals); prices are `u64` in 1e-12 USD; quantities are `u64` in 1e-6 base units.

| Account | PDA seeds (this program unless noted) | Bytes | Rent (SOL) | Written by |
|---|---|---|---|---|
| `SectorConfig` | `["config"]` | 270 | 0.00277008 | admins (the window counters by `finalize_pass`) |
| `RuleSet` | `["ruleset", version u16]` | 904 | 0.00718272 | admins, immutable after creation |
| `Market` | `["market", market_id u16]` | 128 | 0.00178176 | admins |
| `WalletProfile` | `["wallet", trader]` | 56 | 0.00128064 | program |
| `TradingAccount` | `["acct", trader, challenge_id u64 le]` | 206 | 0.00232464 | program only (via evaluator or trader instructions) |
| `PayoutTally` (no discriminator) | `["payout_tally"]` | 25 | 0.00106488 | program only (vault reads it) |
| `sector_authority` (address only, holds no data) | `["setl8_sector_authority"]` | none | none | signs vault CPIs |

There is no bounty vault (decision 21 superseded). `TradingAccount` seeds mirror the vault's `TraderState` (`["trader_state", product_program_id, trader_wallet, challenge_id]`), so the two records of one challenge are always found from the same three inputs.
Rent for a new `TradingAccount` (about $0.35 at an assumed $150/SOL; SOL price is an assumption) is paid by the trader as `payer` and is **returned** by `close_account` once the account is terminal for 7 days.

### 5.1 `TradingAccount` layout (206 bytes, Borsh, fields in this order)
| Field | Type | Bytes | Meaning |
|---|---|---|---|
| discriminator | | 8 | |
| `trader` | Pubkey | 32 | owner |
| `challenge_id` | u64 | 8 | vault challenge id |
| `size_s` | u64 | 8 | S in micro-USD |
| `created_at` | i64 | 8 | |
| `b` | i64 | 8 | closed balance B, **maintained by the program from the journal** |
| `q_tally` | i64 | 8 | credited quick profit this phase |
| `lock_until` | i64 | 8 | unix seconds, 0 = none (set by `Breach`) |
| `last_batch_time` | i64 | 8 | time of the last journal event |
| `last_activity_pushed` | i64 | 8 | last `record_activity` CPI time |
| `next_request_id` | u64 | 8 | mirrors vault `payout_count + 1` |
| `seq` | u64 | 8 | journal sequence |
| `journal_hash` | [u8;32] | 32 | rolling hash head (8.3) |
| `trades_closed` | u32 | 4 | |
| `ruleset_version` | u16 | 2 | snapshot of the rule set |
| `status` | u8 | 1 | 0 Active, 1 Failed, 2 Abandoned, 3 Graduated |
| `phase` | u8 | 1 | 1 to 5 |
| `latch` | u8 | 1 | static floor shifted to 0.96 S |
| `flat` | u8 | 1 | evaluator's assertion after the last batch (I-12) |
| `pass_pending` | u8 | 1 | 0 none, 1 requested by the trader and awaiting the evaluator, 2 confirmed and waiting out the delay |
| `vetoes` | u8 | 1 | count of vetoed passes (saturating) |
| `bump` | u8 | 1 | |
| `tight_on` | u8 | 1 | |
| `r` | i64 | 8 | B recorded when tight mode began |
| `g_peak` | i64 | 8 | highest `E - R` claimed since tight mode began |
| `l_level` | i64 | 8 | tight floor L (0 when off) |
| `pass_requested_at` | i64 | 8 | time of `request_advance` |
| `pass_proposed_at` | i64 | 8 | time of `confirm_pass` (the delay counts from here) |
| **Total** | | **206** | |

The static floor is not stored: it is `0.96 S` if `latch == 1`, else `0.92 S`. Positions, pending orders and the last trades live in the evaluator's database and in the journal; the chain never needs them.

### 5.2 `WalletProfile` (56 bytes)
`trader Pubkey, next_challenge_id u64, active_count u8, bump u8, pad`. `active_count <= 5` is enforced at `open_account` and `reset_account` (decision 28). It is decremented when an account becomes Failed,
Graduated or Abandoned. `next_challenge_id` hands out unused vault challenge ids (the vault rejects a reused id anyway).

### 5.3 `SectorConfig` (270 bytes)
`admin_a, admin_b, evaluator, vault_program, sl8_wallet` (5 x Pubkey), `current_ruleset u16`, `market_count u16`, `paused u8`, `bump u8`,
`pass_delay_secs u32` (default 172,800), `max_payout_per_window u64` (micro-USD, value open), `window_secs u32` (default 86,400), `window_start i64`, `window_total u64`, `reserved [u8;64]`.

---

## 6. Instruction list

Notation: `[S]` signer, `[W]` writable, `[R]` read-only. Compute figures are **estimates** to be measured in the test workspace (module M11); see section 11.

### 6.1 Admin
| Instruction | Signers | Effect |
|---|---|---|
| `init_config` | SL8 admin [S], Rov admin [S], payer | Creates `SectorConfig` (evaluator, delay 172,800, window 86,400, cap) and `PayoutTally` (0/0, magic `SL8TALLY`), rule set 1. |
| `create_ruleset` | both admins | Creates `RuleSet` version N+1 (immutable) and sets `current_ruleset`. Applies only to NEW accounts. |
| `create_market` / `update_market` | either admin | Create or change a market: status, feed id, sessions, caps. **Free feed remap per decision 24.** Emits `MarketFeedChanged`. |
| `set_evaluator(new_key)` | both admins | Rotates the evaluator key. Effective immediately. |
| `set_limits(pass_delay_secs, max_payout_per_window, window_secs)` | both admins | Delay is clamped to `[1 day, 14 days]` so no admin pair can set a zero delay by accident; cap > 0. |
| `set_paused(bool)` | pause: either admin; unpause: both | Blocks `open_account`, `reset_account`, `request_advance`, `finalize_pass`. `post_journal`, `report_fail`, `confirm_pass`, `reject_pass` keep working. |
| `veto_pass` | both admins | Cancels a pending pass, state 1 or 2 (10.7). Cannot do anything else. |

### 6.2 Trader
| Instruction | Signers | Accounts (R/W) | Effect |
|---|---|---|---|
| `open_account(tier_index, mint)` | trader [S], payer (= trader) [S] | trader; `WalletProfile` [W, init if needed]; `TradingAccount` [W, init]; config [R]; rule set [R]; `sector_authority` [R]; vault: program [R], `product_registry` [W], `trader_state` [W, created by vault], `vault_state` [R], `trader_token_account` [W], mint [R], pool token account [W], SL8 token account [W], token program [R], system program [R] | Checks cap 5 and not paused; transfers the 0.4% fee (trader -> SL8 token account, round UP); CPI `deposit_fee(amount = tier cost)` signed by `sector_authority`; initialises the account (`b = S`, phase 1, rule set snapshot); emits `Opened` (not part of the hash chain; `seq = 0`, `journal_hash = h_0`, see 8.3). The evaluator learns of it from the event. |
| `reset_account(prev_challenge_id, mint)` | trader [S], payer [S] | as above plus the previous `TradingAccount` [W] and the previous vault `trader_state` [W] | Previous account must be locally `Failed`; CPI `deposit_reset(amount = ceil(tier cost x ladder[phase-1]), reset_phase = phase-1)`; new account starts at the SAME phase with fresh state; 0.4% fee on the reset price (assumption A-FEE-1). Needs the vault change (decision 18). |
| `request_advance` (the "request payout and move to next phase" button) | trader [S], payer (= trader) [S] | `TradingAccount` [W]; config [R]; `sector_authority` [R]; vault `product_registry` [R], `trader_state` [W] | Requires: Active, not paused, `flat == 1`, `tight_on == 0`, `pass_pending == 0`, `b >= 1.08 S` (program-maintained), phase not failed. Sets `pass_pending = 1`, `pass_requested_at = now`, CPI `record_activity`, emits `PassRequested{phase, amount 0.08 S (0 in phase 1), b, seq, journal_hash}`. No money moves and the phase does not change yet. |
| `cancel_pass` | trader [S] | `TradingAccount` [W] | Only if `pass_pending == 1` and `now - pass_requested_at >= 3,600 s` (the evaluator never answered). Sets `pass_pending = 0`. |
| `release_account` | trader [S] | `TradingAccount` [W], vault `trader_state` [W], registry [R] | Only calls `record_activity`; if the vault says `Abandoned` the local account is marked Abandoned and the wallet slot is freed. Otherwise it is a harmless activity refresh and the trader's own way to keep the vault clock alive if the evaluator stops touching it. |
| `close_account` | trader [S] | `TradingAccount` [W, closed] | After 7 days in a terminal state: returns rent to the trader. The event stream remains the record. |

Trading itself (orders, SL/TP, cancels, closes) is NOT an instruction. It is an API call to the evaluator, signed by the trader wallet (4.5).

### 6.3 Evaluator
| Instruction | Signers | Accounts | Effect |
|---|---|---|---|
| `post_journal(batch)` | evaluator [S] (also fee payer) | `TradingAccount` [W]; config [R]; markets referenced [R]; event accounts | Applies a `JournalBatch` (8.2) under invariants I-1 to I-12, updates `b`, `q`, `latch`, tight state, `lock_until`, `seq`, `journal_hash`, `flat`; emits each event via `emit_cpi!`. |
| `report_fail(batch, reason)` | evaluator [S] | as `post_journal` plus `WalletProfile` [W], `sector_authority` [R], vault program [R], `product_registry` [R], `trader_state` [W] | Applies the final batch (must end with `Fail{reason}`), checks I-11, sets status Failed, CPI `flag_trader_failed`, `active_count - 1`. |
| `confirm_pass` | evaluator [S] | `TradingAccount` [W]; config [R] | Requires `pass_pending == 1`, `flat == 1`, `b >= 1.08 S`. **Phase 1:** starts phase 2 at once (`b = S`, `q = 0`, latch 0, tight off) and emits `PhaseAdvanced`. **Phases 2 to 5:** `pass_pending = 2`, `pass_proposed_at = now`, emits `PassConfirmed`. The evaluator calls it only after its own engine agrees (4.10). |
| `reject_pass(reason)` | evaluator [S] | `TradingAccount` [W] | Requires `pass_pending == 1`. Sets it to 0 and emits `PassRejected{reason}` (NotFlat, JournalBehind, TargetNotReached, Other). Cannot touch money. |
| `touch_activity` | evaluator [S] | `TradingAccount` [W], `sector_authority` [R], vault `product_registry` [R], `trader_state` [W] | CPI `record_activity` (skipped if `now - last_activity_pushed < 86,000 s`), handles `Abandoned` as in 10.2. The backend calls it when the trader sent a signed order. |

### 6.4 Anyone
| Instruction | Signers | Accounts | Effect |
|---|---|---|---|
| `finalize_pass` | any payer [S] | `TradingAccount` [W]; config [W]; `payout_tally` [W]; `WalletProfile` [W]; `sector_authority` [R]; vault: program, `product_registry` [W], `trader_state` [W], `vault_state` [W], `payout_claim` [W, created by vault], system program | Requires `pass_pending == 2`, `now >= pass_proposed_at + pass_delay_secs`, not paused, daily cap not exceeded (10.6). CPI `request_payout(amount = 0.08 S, proposed_request_id = next_request_id)`, reads return data: `Paid` -> tally bump, `next_request_id + 1`, window counter, next phase starts fresh (or Graduated after phase 5); `Abandoned` -> status Abandoned. |

### 6.5 Signer rule for vault CPIs
Every vault CPI is signed by `sector_authority` through `invoke_signed` with seeds `["setl8_sector_authority", bump]`. Vault-side extra signers: `deposit_fee`/`deposit_reset` need the trader and a payer;
`request_payout` needs a payer; `record_activity` and `flag_trader_failed` need only `sector_authority`. No admin key and no evaluator key appears in any money path.

---

## 7. Netting math

**Where this runs.** This section is the specification of the `engine` crate's `apply_fill` (section 16). The evaluator and the public verifier run it in full. The program does NOT run netting; it re-checks only the
closing slice (I-5: pnl formula, quick flag and credit) and the fees. Words like "slot" below mean the engine's per-market position record.

Units: `qty` is a signed `i64` in 1e-6 base units (long > 0). Prices are `u64` in 1e-12 USD. Money is micro-USD. `DEN = 10^12 x 10^6 / 10^6 = 10^12` (notional micro-USD = `qty x price / 10^12`), all products in `i128`.
Tradovate style: **one net position per (account, market)**.

### 7.1 Execution price
Buy fills at `ask = ceil(mid x (1 + h))`, sell at `bid = floor(mid x (1 - h))`, with `h` the half-spread of the market in the account's rule set (e6 units).

### 7.2 Fill algorithm (`apply_fill(side, dq, p, T)`; `q` current signed qty, `A` avg entry)
1. `q == 0` -> open: `q = dq`, `A = p`, `timer_start = T`, `opened_at = T`.
2. Same direction (`q` and `dq` same sign) -> **merge**: `A' = round(( |q| A + |dq| p ) / ( |q| + |dq| ))` with the rounding below; `q' = q + dq`; **`timer_start = T`** (restart on every add, decision 10).
3. Opposite direction: `c = min(|dq|, |q|)` is the closing slice.
   - `pnl = floor( sign(q) x c x (p - A) / DEN )`.
   - Quick if `T - timer_start <= 45`. Credit: loss in full; profit `pnl` -> `floor(pnl / 2)` if quick, else `pnl`. `B += credit`. If quick and `pnl > 0`: `q_tally += floor(pnl / 2)`.
   - `|dq| < |q|`: partial reduce; `q' = q + dq`, `A` and `timer_start` unchanged.
   - `|dq| == |q|`: flat, slot cleared.
   - `|dq| > |q|`: **flip**: slot closes fully, remainder `|dq| - |q|` opens the opposite way at price `p` with a fresh timer; SL/TP of the old position are dropped, the order's SL/TP (if any) attach to the new position.
4. Every closing slice becomes a `Close` journal event. A flip emits two events (`Close`, then `Fill` for the remainder). `Fill` always means exposure goes up (open or add).

### 7.3 Margin
`margin(qty, price) = ceil( |qty| x price / DEN / leverage )`. An order is accepted if, after applying it, `E - sum(margin of all positions) >= 0`. For a flip only the remainder needs new margin. Reduce-only orders skip the check.
At a breach or a fail all margin is released.

### 7.4 Rounding (always against the trader)
| Quantity | Rule |
|---|---|
| Realized and unrealized P&L | floor to micro-USD (profit rounds down, loss rounds to larger loss) |
| Fees (holding, commission, swap-cost) and margin | ceil |
| Average entry | long: ceil; short: floor |
| Quick credit | floor(profit / 2) |
| Oracle price to e12 | floor; ask = ceil, bid = floor |

### 7.5 Worked examples
- **Merge:** long 3 @ 1.10 then buy 4 @ 1.13 (units of 1,000,000 EUR): numerator 7.82e12 / 7 = 1.117142857142857e12 -> long rounds UP to **1.117142857143**.
- **Partial reduce:** long 700,000 @ 1.117142857143, sell 300,000 at bid 1.12: `pnl = floor(300,000 x 0.002857142857 ...)` = **+$857.142857**; position now 400,000 @ 1.117142857143, timer unchanged.
- **Flip:** long 1,000,000 @ 1.0800324, sell 1,500,000 at bid 1.0820000: closing slice pnl = 1,000,000 x 0.0019676 = **+$1,967.60**; remainder **short 500,000 @ 1.0820000** (fresh timer), margin on remainder = 500,000 x 1.082 / 50 = $10,820.
- **Quick:** the same flip 30 s after the long opened: slice is quick, credit = floor(1,967.60 / 2) = $983.80, `q_tally += 983.80`.

### 7.6 Edge cases (each needs a test, module M3)
Error names in this table (`InvalidQuantity`, `InvalidStopLevel`) are the engine's rejection codes returned to the trader by the evaluator API. They are not program errors (section 12).
| # | Case | Rule |
|---|---|---|
| N-1 | Zero quantity, quantity not a multiple of `qty_step`, or below `qty_min` | `InvalidQuantity` |
| N-2 | Fill exactly flattens | Slot cleared, SL/TP cleared, `open_count -= 1` |
| N-3 | Flip when only the closing slice fits margin | Allowed; remainder is margin-checked |
| N-4 | SL above bid for a long (or TP below) | `InvalidStopLevel` |
| N-5 | SL and TP both satisfied by one gap sample | SL takes precedence (conservative) |
| N-6 | A fill that makes `q_tally > 0.028 S` | Phase fails immediately (I-8; the evaluator reports it with `report_fail(PhaseQuick)`) |
| N-7 | Add to a position 44 s after open, then close 20 s later | Timer restarted at the add: closing slice age 20 s -> quick |
| N-8 | Same-tx multiple fills | Sequential, each with its own `seq`; one sample, one `T` |
| N-9 | Pending order fills in a market where a position exists | Netting applies normally (merge, reduce or flip) |
| N-10 | Fill price rounds to exactly the trigger | Counts as triggered (touching counts) |

---

## 8. Journal and trade history

### 8.1 What is recorded (decision 3, restated for option C)
Every fill and every closing slice, every floor or mode change, every fee, and the batch metadata, in order. The trader's full trade history is therefore the event stream of one account.
Event kinds: `Opened`, `Fill`, `Close`, `Fee`, `TightStart`, `FloorRaised`, `TightEnd`, `Breach`, `Fail`, `Meta`, `PassRequested`, `PassConfirmed`, `PassRejected`, `PassCancelled`, `PassVetoed`, `PassFinalized`,
`PhaseAdvanced`, `Graduated`, `Abandoned`, `MarketFeedChanged`. The first group (`Fill` to `Meta`) is posted by the evaluator inside a `JournalBatch` and extends the hash chain; the rest are emitted by the program itself with `emit_cpi!` and carry the account's current `seq` as a reference (`at_seq`) but are NOT part of the hash chain, so the evaluator's sequence numbers are never disturbed by lifecycle events. Reason codes are enums, not free text.
`Fill` always means exposure goes up (open or add). Reductions, closes and the closing half of a flip are `Close` events.

### 8.2 Formats (Borsh, canonical)
```text
JournalBatch {
  first_seq: u64,                  // must equal account.seq + 1
  prev_hash: [u8;32],              // must equal account.journal_hash
  events: Vec<JournalEvent>,       // 1..=8
  proofs: Vec<PriceProof>,         // 0..=3, referenced by index
}
enum JournalEvent {
  Fill      { market: u16, side: u8 /*0 long,1 short*/, order_kind: u8 /*market,limit,stop*/, qty: u64, price_e12: u64, time: i64,
              avg_entry_after: u64, margin: u64, sl: u64, tp: u64, intent_id: u32, proof: u8 /*index or 255*/ },
  Close     { market: u16, side: u8, qty: u64, entry_e12: u64, exit_e12: u64, time: i64, timer_start: i64,
              pnl: i64, quick: u8, credited: i64, reason: u8 /*Manual,StopLoss,TakeProfit,Flip,Breach,Fail*/, intent_id: u32, proof: u8 },
  Fee       { kind: u8 /*Holding,Commission*/, market: u16, amount: u64, time: i64 },
  TightStart{ time: i64, p_open: i64, r: i64, l_start: i64, g_peak: i64, l_level: i64, proof: u8 },
  FloorRaised{ time: i64, g_peak: i64, l_level: i64, proof: u8 },
  TightEnd  { time: i64 },
  Breach    { time: i64, l_level: i64, b_after: i64, proof: u8 },
  Fail      { time: i64, reason: u8 /*StaticFloor,PhaseQuick*/, b_after: i64, proof: u8 },
  Meta      { time: i64, flat: u8, samples_hash: [u8;32], intents_hash: [u8;32], engine_version: u16 },   // last event of every batch
}
PriceProof {
  source: u8,            // 0 Stork, 1 Pyth (not built)
  feed_id: [u8;32],
  timestamp_ns: u64,
  quantized_value: i128, // 18 decimals, as Stork publishes it
  signed_update: Vec<u8> // Stork's signed update, opaque to the program (3.3); about 200 to 250 bytes assumed
}
```
Size of one proof in a transaction is about 300 bytes (UNVERIFIED, M2); a `Close` or `Fill` event is about 90 to 110 bytes. Hence about two proofs per 1,232-byte transaction; larger batches use an address lookup table or several transactions.
`MAX_EVENTS_PER_BATCH = 8` and `MAX_PROOFS_PER_BATCH = 3` bound compute and event size.

### 8.3 Hash chain
Genesis `h_0 = sha256("SL8LEV" ‖ account pubkey)`, written at `open_account` together with `seq = 0`; the first evaluator batch has `first_seq = 1`.
For each event with sequence number `n`: `h_n = sha256("SL8LEV1" ‖ h_(n-1) ‖ n as u64 LE ‖ borsh(event) ‖ proof_digest)`, where `proof_digest = sha256(borsh(proof))` if the event references a proof and 32 zero bytes otherwise.
The program computes this with the `sol_sha256` syscall, stores only `h_n` and `n`, and emits `EventLog{account, seq, event, proof}` through `emit_cpi!` (an inner instruction, not a text log: logs truncate at 10 KB per transaction).
Any missing, reordered or altered event breaks the chain from that point; the chain makes the order and completeness of the posted record checkable, nothing more.

### 8.4 Storage choices compared
| Option | How | Cost | Verdict |
|---|---|---|---|
| A. One record account per closed trade | PDA per trade, about 192 bytes | rent about $0.33 per trade at $150/SOL | Rejected: costs more than a $30 account earns |
| B. Events + rolling hash (chosen) | `emit_cpi!` per event, `journal_hash` in the account | no rent; transaction fee about 5,000 lamports (about $0.00075) per transaction | Cheap; full detail; tamper-evident |
| C. In-account ring buffer of recent trades | inside the account | extra rent | Dropped: the UI reads the evaluator's API and verifies against the hash |
Cost sketch: a trade that needs 3 transactions costs the evaluator about 15,000 lamports (about $0.002). A scalper with 500 trades per phase costs about 0.0075 SOL (about $1.1). **The evaluator pays; its wallet needs a SOL budget and a low-balance alert (OPEN_QUESTIONS).**
Retention depends on RPC/archive providers. Setl8 keeps its own full copy of events, the sample archive and the intents file, and publishes them; the hash makes loss or tampering detectable.

### 8.5 When the evaluator posts
- Every `Fill` batch within `T_post` of the fill, so `flat` on the chain tracks reality closely (the chain value only changes with a `Meta` event). Target `T_post` = 60 s. It is a service level, not a chain rule in v1 (OPEN_QUESTIONS #10).
- Closing details, fees and floor changes at the latest when the position closes.
- A `Meta` event ends every batch and sets `flat`.

---

## 9. Prices, fees and costs

### 9.1 Spread (decision 14, numbers PENDING from Rovik's friend)
Oracles give one price. The program synthesises bid and ask: `ask = mid x (1 + h)`, `bid = mid x (1 - h)`, `h` = per-market half-spread in the rule set (e6 units). Longs are marked at bid, shorts at ask, so a position
starts slightly negative (see the -$64.80 in 4.9, which is 0.003% half-spread each side on $1.08M). Spread and commission are simulated balance deductions; no tokens move. Until the friend's numbers arrive the
build uses placeholders marked `PENDING` in the test fixtures, never in production data.

### 9.2 Purchase price and the 0.4% fee (decisions 15, 16)
The vault charges exactly the tier `cost` (`deposit_fee.amount` must equal it). The 0.4% fee is a **separate SPL transfer** by the trader to the SL8 wallet's token account of the same mint, rounded UP to a micro-dollar, charged to every buyer on top of the price.
| Size | Price | 0.4% fee | Buyer pays |
|---|---|---|---|
| $2,500 | $30.00 | $0.12 | $30.12 |
| $5,000 | $55.00 | $0.22 | $55.22 |
| $10,000 | $99.00 | $0.396 | $99.396 |
| $15,000 | $145.00 | $0.58 | $145.58 |
| $20,000 | $189.00 | $0.756 | $189.756 |
| $25,000 | $235.00 | $0.94 | $235.94 |
| $50,000 | $489.00 | $1.956 | $490.956 |
| $75,000 | $689.00 | $2.756 | $691.756 |
| $100,000 | $859.00 | $3.436 | $862.436 |
| $200,000 | $1,599.00 | $6.396 | $1,605.396 |

The vault then splits `cost` by its `fee_split_bps` between the payout pool and the SL8 wallet. The 0.4% fee is outside that split. A trader paying with another Solana token swaps to USDC/USDT first in the same transaction
(client-side router such as Jupiter); the program only sees the stablecoin amounts. Rent for the new accounts is a separate SOL cost to the trader (section 5).

### 9.3 Holding fee (decisions 22, 23, 26)
Charged for every calendar night a position is held through **22:00 UTC**, on notional at the mark, both long and short, ceil-rounded, deducted from `B`. A weekend is three charges (Fri, Sat, Sun nights); there is no triple-swap day.
| Class | Per night | Worst case per night on a fully used $100K account |
|---|---|---|
| FX majors | 0.01% | $500 (0.5%) |
| FX minors / exotics | 0.02% | $400 (0.4%) |
| Metals | 0.04% | $800 (0.8%) |
| Indices | 0.02% | $400 (0.4%) |
| Energy | 0.04% | $400 (0.4%) |
| Stocks | 0.04% | $200 (0.2%) |
| Crypto majors | 0.10% | $500 (0.5%) |
| Crypto alts | 0.16% | $320 (0.32%) |

Mechanics (engine, section 16): each position stores `fee_day` (index of the last 22:00 rollover already charged; a position opened after the day's rollover starts with today's index, so it is charged from the next rollover). The engine charges
`n = today_index - fee_day` nights at once: `fee = ceil(n x rate_e6 x notional_at_stored_mark / 10^6)`. **Approximation:** the notional uses the mark at the time the fee is applied, not at 22:00 exactly. This is documented, bounded by the
price move in the gap, and tested. Each application becomes a `Fee{HoldingFee}` journal event; the program only checks that `b` is debited by exactly that amount (I-6). The rates are broker-derived (IG formulas) and doubled; benchmark rates for October 2026 were not verified.

### 9.4 Commission
Pending (decision 14). The rule set has a per-class `commission_e6` charged on notional at open and at close (ceil). The estimated closing commission is included in `E`, so a floor decision never ignores a cost the trader will pay on closing.

### 9.5 Reset price (decision 17; needs the vault change of decision 18)
`reset price = ceil(tier cost x ladder[failed_phase] / 10000)`, ladder (phase 1 to 5) = 10000, 11500, 12000, 12750, 13000 bps. The 0.4% fee applies on top (assumption, confirm). The account resumes in the same phase it failed.
| Size | Phase 1 (100%) | Phase 2 (115%) | Phase 3 (120%) | Phase 4 (127.5%) | Phase 5 (130%) |
|---|---|---|---|---|---|
| $2,500 | $30.00 | $34.50 | $36.00 | $38.25 | $39.00 |
| $5,000 | $55.00 | $63.25 | $66.00 | $70.125 | $71.50 |
| $10,000 | $99.00 | $113.85 | $118.80 | $126.225 | $128.70 |
| $15,000 | $145.00 | $166.75 | $174.00 | $184.875 | $188.50 |
| $20,000 | $189.00 | $217.35 | $226.80 | $240.975 | $245.70 |
| $25,000 | $235.00 | $270.25 | $282.00 | $299.625 | $305.50 |
| $50,000 | $489.00 | $562.35 | $586.80 | $623.475 | $635.70 |
| $75,000 | $689.00 | $792.35 | $826.80 | $878.475 | $895.70 |
| $100,000 | $859.00 | $987.85 | $1,030.80 | $1,095.225 | $1,116.70 |
| $200,000 | $1,599.00 | $1,838.85 | $1,918.80 | $2,038.725 | $2,078.70 |

The vault copies `payout_count` from the failed record, so a phase-5 reset cannot earn more than the payouts that remain (max 4 per record chain, 32% of size).

---

## 10. Lifecycle, CPI map and payout flow

### 10.1 Lifecycle table
| Event | Our instruction | Vault CPI (signed by `sector_authority`) | Extra signers | Local state change |
|---|---|---|---|---|
| Purchase | `open_account` | `deposit_fee` | trader, payer | account created, `active_count + 1` |
| Trader activity (a signed order reached the evaluator) | `touch_activity` (evaluator) or `release_account` (trader) | `record_activity` (skipped if called within the last 86,000 s) | none | `last_activity_pushed` |
| Static-floor or quick-limit fail | `report_fail` | `flag_trader_failed` | evaluator | status Failed, `active_count - 1` |
| Tight-floor breach | `post_journal` (`Breach`) | none | evaluator | lock 12 h, tight mode off |
| Reset after fail | `reset_account` | `deposit_reset` | trader, payer | new account at the same phase |
| Pass, phase 1 | `request_advance` then `confirm_pass` | `record_activity` (at the request) | trader, then evaluator | fresh phase 2 at the confirmation |
| Pass, phases 2 to 5 | `request_advance`, `confirm_pass`, wait 2 days, `finalize_pass` | `record_activity` (request), `request_payout` (finalize) | trader, evaluator, any payer | tally bumped, next phase |
| Graduation (after phase 5 finalize) | `finalize_pass` | `request_payout` (the vault also auto-graduates at `max_payout_count`) | any payer | status Graduated, `active_count - 1` |
| Abandonment (7 days idle) | vault flips it on `record_activity`/`request_payout`/`mark_abandoned`; we learn via return data in `release_account`, `touch_activity` or `finalize_pass` | `record_activity` | | status Abandoned, wallet slot freed |

### 10.2 Return-data discipline
After every `record_activity` and `request_payout` CPI the program reads `get_return_data()` and checks `program_id == vault_program`. `Abandoned` is handled as above; a missing or wrong-program return value aborts the transaction.
`ActivityOutcome::Throttled` and `Recorded` are both fine. Nothing tells the trader a payout was queued unless the outcome was `Paid`.

### 10.3 The pass state machine (`pass_pending`)
```
          request_advance            confirm_pass (phase 2..5)             finalize_pass (after delay, cap ok)
   0 ──────────────────────► 1 ─────────────────────────────► 2 ───────────────────────────────► 0 (next phase / Graduated)
   ▲                         │ reject_pass (evaluator)          │ veto_pass (both admins)
   │◄────────────────────────┤ cancel_pass (trader, after 1 h)  │
   └─────────────────────────┴──────────────────────────────────┘
   confirm_pass in phase 1 goes 1 ─► 0 and starts phase 2 at once (no money).
```
- State 1: the evaluator may still post journal batches (to catch the chain up); the trader cannot trade (the evaluator freezes the account; the chain cannot enforce this, only the rule that nothing can be confirmed unless `flat`).
- State 2: `post_journal` and `report_fail` are rejected (`PassPending`), so the state that was checked cannot change. The account is frozen for trading for the whole delay (decision 33).
- A reject, cancel or veto leaves phase, `b`, `q`, latch and tight state exactly as they were.

### 10.4 Payout flow against the queued-claim vault
1. `request_advance` (trader), then `confirm_pass` (evaluator) as in 10.3.
2. After `pass_delay_secs` (default 2 days, clamped to 1 to 14 days) anyone calls `finalize_pass`; the caller pays the `PayoutClaim` rent as `payer`.
3. The program checks: pending is 2, delay elapsed, not paused, daily cap (10.6).
4. CPI `request_payout(trader_wallet, amount = 0.08 S, product_program_id, challenge_id, proposed_request_id = next_request_id)`; the return data must be `Paid`.
5. On `Paid`: tally checked-add (`requested_count + 1`, `requested_total + amount`), `next_request_id += 1`, window counter add, then the next phase starts fresh (or the account is Graduated after phase 5). On any error the whole transaction reverts, including the tally.
6. The vault's permissionless 5-day heartbeat pays queued claims pro rata; this program never learns when. Nothing in this program can move that money.
7. Fixed formula: `amount = 0.08 S`, once per phase 2 to 5, at most 4 per record chain (vault cap). No instruction takes an amount from any caller.

### 10.5 `PayoutTally`
PDA `["payout_tally"]` under this program, 25 bytes, layout `b"SL8TALLY" | version 1 | requested_count u64 LE | requested_total u64 LE`, no Anchor discriminator, held as `UncheckedAccount`, parsed and written with `setl8_shared_interfaces::PayoutTally::{parse, write_into}`.
Increases only on `Paid`, checked arithmetic (fail on overflow), never decreased, closed or re-initialised, including across upgrades. Initialised to 0/0 in `init_config`.

### 10.6 Daily payout cap (decision 34)
`finalize_pass` does, in this order: if `now >= window_start + window_secs` then `window_start = now`, `window_total = 0`. Then `window_total + amount <= max_payout_per_window` or the instruction fails with `DailyPayoutCapReached`
(the trader or anyone retries after the window rolls). Then `window_total += amount`. Known property: fixed windows allow up to twice the cap across a window boundary. `max_payout_per_window` is a micro-USD value set by both admins (open question).
For scale: the largest single payout is $16,000 (0.08 x $200K), so any cap below that would block that tier; the largest sum of the vault cap per account is $64,000.

### 10.7 Veto (decision 33)
`veto_pass` requires both admin signatures in one transaction and `pass_pending != 0`. It sets `pass_pending = 0`, increments `vetoes` (saturating), emits `PassVetoed{admin keys, slot}`. It never touches `b`, phase, payouts or the vault, and it cannot be used to confirm or finalize anything.
The trader may press the button again. **What admins do after a veto (investigate, reset, ban) is policy, not code, and it is in tension with "no manual overrides"; see OPEN_QUESTIONS #8.** A veto can delay a trader indefinitely; the `vetoes` counter and event make that visible.

### 10.8 Vault pause (decision 25)
While the vault pauses this product: `open_account`, `reset_account` and `finalize_pass` fail at the vault (their CPIs revert), so the caller retries later. Journal posts, `report_fail`, `confirm_pass` and `reject_pass` continue (the vault allows `flag_trader_failed` while paused).
The vault freezes inactivity clocks during a pause. The sector `set_paused` is a separate, additional switch.

---

## 11. Compute, transaction size and stack budget

**ESTIMATES. Measured values replace them in module M11.**
| Instruction | CU estimate | Notes |
|---|---|---|
| `post_journal` (8 events, 3 proofs) | 120k to 220k | per event about 12k to 25k CU: Borsh decode, i128 pnl re-check, `sol_sha256` over about 150 bytes, `emit_cpi!` self-CPI (about 5k). Requests 250k. |
| `report_fail` | +20k over `post_journal` | `flag_trader_failed` CPI |
| `open_account` | 90k to 130k | `deposit_fee` CPI creates `TraderState` and moves tokens; account init 206 bytes |
| `request_advance` / `confirm_pass` / `reject_pass` / `cancel_pass` / `veto_pass` | 15k to 40k | `request_advance` includes one `record_activity` CPI |
| `finalize_pass` | 70k to 100k | `request_payout` CPI creates `PayoutClaim` |
Default 200k per instruction suffices except `post_journal`.

**Transaction size.** 1,232 bytes. A signed proof is about 300 bytes (UNVERIFIED, M2), an event about 100: about two proofs per transaction, so a trade with entry, tight start, peak and exit is 2 to 3 transactions (section 4.9).
This costs fees (8.4) but no safety: every transaction extends the same hash chain and a later transaction cannot be applied before an earlier one.

**Stack (4,096 bytes per frame).** Accounts are small Borsh structs; instruction account structs with more than a few accounts (`open_account`, `finalize_pass`, `report_fail`) wrap their big members in `Box<>`; `RuleSet` is read with `AccountLoader` (zero-copy, 904 bytes).
**Build check:** `scripts/anchor-build-checked.sh` (same approach as the vault's script of that name) runs `anchor build`/`cargo build-sbf`, fails on any "Stack offset of ... exceeded max offset of 4096" message, and runs `const_assert!`s on every account size in section 5.

---

## 12. Error list
| Code | Name | Raised when |
|---|---|---|
| 6000 | `Unauthorized` | wrong signer for an admin or evaluator instruction |
| 6001 | `Paused` | sector paused |
| 6002 | `WalletCapReached` | 5 active accounts already |
| 6003 | `InvalidTier` | tier index not in the rule set |
| 6004 | `AccountNotActive` | status is not Active |
| 6005 | `AccountLocked` | an exposure-increasing `Fill` with `time < lock_until` |
| 6006 | `MarketUnknown` | market account missing or wrong |
| 6007 | `BadSequence` | `first_seq` or `prev_hash` mismatch |
| 6008 | `TimeBackwards` | event time before `last_batch_time` or before the previous event |
| 6009 | `TimeInFuture` | event time beyond `now + 2 s` |
| 6010 | `BadPriceProof` | feed id, value, or age check failed (I-4) |
| 6011 | `PnlMismatch` | `Close.pnl` differs from the program's computation (I-5) |
| 6012 | `QuickMismatch` | `quick` or `credited` wrong (I-5) |
| 6013 | `FeeInvalid` | negative fee amount (I-6) |
| 6014 | `TightStartInvalid` | I-9 failed |
| 6015 | `FloorFormulaMismatch` | `l_level` does not follow the formula, or fell |
| 6016 | `BreachInvalid` | I-10 failed |
| 6017 | `FailInvalid` | I-11 failed |
| 6018 | `QuickLimitExceeded` | Q above 0.028 S without a `Fail{PhaseQuick}` (use `report_fail`) |
| 6019 | `PassPending` | `post_journal`/`report_fail` while `pass_pending == 2` |
| 6020 | `NotFlat` | button or confirm with `flat == 0` |
| 6021 | `TargetNotReached` | `b < 1.08 S` |
| 6022 | `NoPendingPass` | `finalize`, `confirm`, `reject`, `cancel` or `veto` with nothing pending in the right state |
| 6023 | `PassDelayNotElapsed` | `finalize_pass` too early |
| 6024 | `DailyPayoutCapReached` | window total would exceed the cap |
| 6025 | `PayoutNotAccepted` | vault returned anything other than `Paid` where it was required |
| 6026 | `ReturnDataInvalid` | return data missing or from the wrong program |
| 6027 | `ResetNotAllowed` | previous account not locally Failed |
| 6028 | `WrongPhase` | operation not valid in this phase |
| 6029 | `MathOverflow` | checked arithmetic failed |
| 6030 | `CloseTooEarly` | `close_account` before 7 days terminal |
| 6031 | `RuleSetMissing` | version not found |
| 6032 | `BatchTooLarge` | more than 8 events or 3 proofs |
| 6033 | `CancelTooEarly` | `cancel_pass` before 1 hour |
| 6034 | `InvalidLimits` | delay outside 1 to 14 days, zero cap or zero window |

---

## 13. Failure and attack analysis

### 13.1 Residual risks accepted by the design (not solved)
| # | Risk | Size | Where discussed |
|---|---|---|---|
| R-1 | **Evaluator is wrong or compromised** and writes a false history (fabricated pass, wrongful fail, omitted breach) | Money out is bounded by the 2-day delay + veto, the daily cap and the vault (32% of size per record chain); trader harm from a wrongful fail has no on-chain remedy | 1.3, 4.7 |
| R-2 | Omitted breach or dropped sample cannot be proven on-chain | Only a verifier with an independent price record can find it | 4.5 |
| R-3 | Evaluator outage stops trading, journaling and activity refresh | Liveness only; vault clocks can run | 4.7 |
| R-4 | Oracle error or compromise pays out wrong prices | bounded by the same limits as R-1 | 1.3 |
| R-5 | Admin free feed remap | **EXCEPTION (decision 24)**: a rigged feed can make the evaluator produce passes; the delay, veto and cap apply | 14 |
| R-6 | Both admins unreachable for 2 days: a bad pass is finalized | Bounded by the cap and vault | 13.3 |
| R-7 | Signatures on price proofs and trader intents are not verified on-chain in v1 | The public verifier checks them; hardening paths in 3.3 and 4.5 | 3.3 |

### 13.2 Attacks considered
| # | Attack | Status |
|---|---|---|
| A-1 | Evaluator replays an old favourable price | Limited: `proof.time` must be within `max_age_secs` of the event time and event time never goes backwards. The verifier compares against the sample archive. |
| A-2 | Wrong or fake feed id in a proof | Blocked: I-4 checks the market's current feed id. |
| A-3 | Impersonate this program to the vault | Blocked by the vault: `sector_authority` must be a PDA signed by this program id. |
| A-4 | Anyone but the evaluator posts journal | Blocked: I-1. |
| A-5 | Trader fabricates his own pass | Blocked: only the evaluator posts the journal that gives him `b` and `flat`. |
| A-6 | Cross-account hedging: wallet A long, wallet B short, one passes | **NOT solved by rules.** HIGH severity economics: the loser costs a reset of 100% to 130% of the tier price (about $859 to $1,117 on $100K); the winner collects $8,000 per phase. **Option C improves detection**: the backend sees every order of every account in real time and can flag opposite positions of the same size and time across wallets. Detection output feeds a veto (policy, OPEN_QUESTIONS #7). |
| A-7 | Latency arbitrage against a stale oracle | Partly mitigated: spread, tight `max_age_secs`, the quick-trade rule; the backend can also rate-limit. Not eliminated. |
| A-8 | Keep an account alive or kill it from outside | Evaluator-only `touch_activity` follows real signed orders; the trader's own `release_account` also refreshes. A silent evaluator can let an account lapse (R-3). |
| A-9 | Split a position to dodge the 45 s rule | Timer restarts on every add; partial closes do not reset; flip makes a new position. |
| A-10 | Overflow or rounding drift between engine and program | `i128` intermediates, checked arithmetic, the program re-computes pnl with the same `engine` function the evaluator uses, differential tests, mutation tests on netting and ratchet. |
| A-11 | Upgrade key compromise | 2-of-2 multisig (decision 30); burn decision after the audit. |
| A-12 | Compute or account-size DoS | Hard caps: 8 events and 3 proofs per batch; accounts fixed-size. |
| A-13 | Wallet cap evasion | Not blocked (new wallet). The cap limits convenience only. |
| A-14 | Trader presses the button while a position is open | Chain `flat` can be stale; blocked by `confirm_pass`/`reject_pass` (the evaluator compares with its engine) and by the replay of the watchdog. |
| A-15 | Trader spams `request_advance` to burn evaluator transactions | Cost parity (both pay a base fee); `reject_pass` blocks nothing else. Accepted. |
| A-16 | Admins veto without cause | They cannot move money, but can delay a trader without limit. Visible (`vetoes`, event). Policy item, OPEN_QUESTIONS #8. |
| A-17 | Compromised evaluator fills the daily cap with fake passes to starve honest ones | Fake passes are vetoed (the cap counts only finalized passes, so vetoed ones do not consume it); honest passes finalize after the next window if needed. |
| A-18 | Compromised evaluator mass-fails honest accounts | Possible and visible; rotation by both admins; no automatic remedy (OPEN_QUESTIONS #9). |

### 13.3 Operational duties that make the safeguards real
- A **watchdog** that replays every `PassRequested`/`PassConfirmed` within the hour and alerts both admins (4.6). Without it the veto is decorative.
- Both admins must be able to sign a veto within 2 days (including weekends). A pair of mobile-ready signing setups is required (OPEN_QUESTIONS #12).
- Evaluator key in a hardware-backed signer or a managed KMS, with a rotation runbook (OPEN_QUESTIONS #11).
- A second, independent price recorder (4.5).

---

## 14. Admin powers, upgrade authority and the "no admin key on money" rule
| Power | Held by | Money impact |
|---|---|---|
| Create or change markets, **re-point feeds, enable or disable** | either admin (EXCEPTION: no delay, decision 24) | Indirect: a rigged feed can mislead the evaluator; passes still face delay, veto and cap |
| Publish a new rule set | both admins | Only new accounts; existing accounts keep their snapshot |
| Rotate the evaluator key | both admins | None directly; a new key can post journals |
| Set pass delay (1 to 14 days), daily payout cap and window | both admins | Can slow or throttle payouts; cannot create one |
| Veto a pending pass | both admins together | Can cancel a payout request; cannot approve, create or move anything |
| Pause sector | either (unpause: both) | Stops new purchases and finalization; never moves money |
| Upgrade program | 2-of-2 multisig (SL8 admin + Rov admin) | Code could request payouts up to the vault cap; this is the biggest lever; burn decision pending |
The evaluator key can post journals, report fails, confirm or reject passes and refresh activity. It cannot request or finalize a payout, open accounts, change config, or pass the delay.
No instruction in this program takes an admin-supplied or evaluator-supplied amount into a vault payout or transfers trader or payout tokens. Payout amounts are the fixed formula in 10.4.

---

## 15. Assumptions register (each is also in OPEN_QUESTIONS.md)
| Id | Assumption | Where |
|---|---|---|
| A-RULE-1 | Tight floor L is a frozen ratchet; effective floor = max(F_s, L) | 2.3 |
| A-RULE-2 | Quick tolerance: strictly greater than 0.028 S, checked when the quick slice closes | 2.4 |
| A-RULE-3 | RESOLVED (decision 31): `l_start = min(f_static + 0.04 S, R)` | 2.3, 4.4 |
| A-ORA-1 | Stork signed-update format, size, fee and access for non-enterprise users (all UNVERIFIED) | 3.3, 11 |
| A-ORA-2 | The backend can obtain Stork's signed stream without an enterprise contract | 3.3 |
| A-FEE-1 | The 0.4% fee also applies on resets | 9.5 |
| A-FEE-2 | Holding fee uses the mark at application time, not at 22:00 | 9.3 |
| A-MKT-1 | Launch with USD-quoted markets; cross pairs and non-USD indices wait for conversion feeds | 3.5 |
| A-CAP-1 | At most 10 pending orders per account and 8 open positions (engine limits, enforced off-chain; the open-position cap exists to bound valuation work and per-account proofs) | 16 |
| A-ADM-1 | Either admin may change markets and feeds (no delay); both admins for rule sets, limits, veto, evaluator rotation and upgrade | 14 |
| A-VLT-1 | Vault changes the reset price formula (decision 18) before module M8 is built | 1.4, 9.5 |
| A-C-1 | A pass needs the evaluator's `confirm_pass`; the account is frozen during the 2-day delay | 10.3 |
| A-C-2 | Price-proof and intent signatures are verified off-chain only in v1 | 3.3, 4.5 |
| A-C-3 | Evaluator posts fills within about 60 s; not a chain rule in v1 | 8.5 |
| A-C-4 | `max_payout_per_window` value set by Rovik | 10.6 |

---

## 16. Off-chain components and repository layout

| Component | What it is | Notes |
|---|---|---|
| `crates/engine` | Pure Rust, integer-only (no floats), deterministic. Implements sections 2, 7 and 9: fill/netting, margin, spread, floors, ratchet, quick rule, holding fees, SL/TP/limit/stop triggers, pass check. Versioned (`engine_version`). | The ONE definition of the rules. Linked by the program (only the checking subset), the evaluator and the verifier. |
| `crates/journal-types` | Event, batch and proof types; Borsh encoding; hashing. | Shared by program, evaluator and verifier. |
| `programs/setl8-lev-trading` | The Anchor program. | Sections 5, 6, 10. |
| `crates/verifier` (+ CLI `setl8-verify`) | Replays journal + sample archive + intents; checks proofs; reports a verdict and mismatches. | Module M9. Open source. |
| `services/evaluator` | Reference harness that drives the engine with recorded samples and orders and posts batches through the program. | The production evaluator is Setl8's backend. **Language decision is open (OPEN_QUESTIONS #2):** recommended Rust (same engine, no port). If the backend is TypeScript, the engine is compiled to WASM and both are fuzzed against each other bit for bit. |
| `tests/` (separate workspace, LiteSVM) | Isolated test workspace, same convention as the vault, loading the built program `.so`; no dependency on the program crate's dev-dependencies. | Section BUILD_PLAN. |
Engine limits (off-chain): 8 open positions and 10 pending orders per account; margin check at order time and at fill time; orders deleted on fail, breach lock, phase change (decision 27).
