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
- `setl8-shared-interfaces` is pinned at **v0.5.0** (tag commit 3b0580c4; was v0.4.0 until 2026-10-13). `request_payout` queues a claim and needs
  7 accounts; `flag_trader_failed` takes the registry read-only. Vault commit `be97396` does not yet enforce the payout tally,
  but the sector must maintain it from day one (`PayoutTally`, section 10.5).
- Vault `INACTIVITY_LIMIT_SECS` = 604,800 (7 days), `ACTIVITY_THROTTLE_SECS` = 86,400, `MAX_RESET_PHASES` = 8.
- Vault `deposit_reset` (core commit 9a5c179, shared-interfaces v0.5.0) prices a reset as `ceil(CURRENT tier cost x reset_price_bps[failed phase] / 10000)`,
  a percent of the tier PRICE as it is registered NOW, not of account size and not of what the trader paid (decisions 17, 18, 51). No longer build-blocking.
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

### 2.9 Prohibited conduct: coordinated hedging (decision 36, Rovik 2026-10-09)
**Rule:** holding opposite positions in the SAME market across accounts or wallets controlled by one person or one group is not allowed.
**Process:** (1) the backend detects it and **holds** the flagged accounts (`hold_account`): no new exposure, no pass request, confirmation or payout finalization for them until review; (2) both admins review the evidence;
(3a) confirmed: both admins sign `fail_for_violation` for each flagged account (account Failed, vault `flag_trader_failed`, wallet banned) and, if needed, `ban_wallet` for linked wallets; no refund, no reset, no payout (a violation-failed account carries a marker, `pass_pending = 3`, and `reset_account` refuses it for good with `ViolationNoReset` 6108, also after `unban_wallet`; decision 60);
(3b) not confirmed: both admins `release_hold` and the account carries on exactly as it was.
Correlated markets (EURUSD against GBPUSD) are NOT covered (Rovik's choice). The numeric thresholds (minimum size, time overlap, wallet-link evidence) are an evaluator policy to be written in the public terms. ASSUMPTION A-RULE-4 (OPEN_QUESTIONS #7).

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
  account created under version N. The evaluator reads the rule set of each account's version; the chain does not use most of it (it uses the tier table, ladder, fee rate, the half-spread of the market of a `Fill` and the constants of section 2).
  **Unpriced markets are untradable (decision 64).** `spread_e6` is indexed by market id and the rule set is immutable, so a market created after version N has a half-spread of 0 in version N and in every older version. A market whose half-spread in the ACCOUNT'S OWN rule set is 0 takes no new exposure: an exposure-increasing `Fill` there is refused by the program (`MarketUnpriced`, 6109), the evaluator and the public verifier (`FillNotAllowed`, "unpriced market"); closes are never refused by this rule. Accounts of an older rule set version therefore cannot trade a later-added market; they keep trading every market their rule set prices. Adding a market after launch = a new rule set version that prices it, for NEW accounts.
- **`Market`** (PDA `["market", market_id u16]`): class, `status` (0 disabled, 1 enabled, 2 reduce-only), `feed_id [u8;32]`
  (**either admin can change it at any time, decision 24**), `max_age_secs`, quantity step and minimum, `max_notional_usd` (per-position
  cap, see OPEN_QUESTIONS), up to 6 weekly session windows, optional `quote_conv_market` (see 3.5). The chain uses a market only to check that a journal event
  names an existing market and that a price proof carries that market's current `feed_id` (and, for a `Fill`, that the market is priced in the account's rule set, 3.2 above). Changing `feed_id` emits `MarketFeedChanged{old,new,slot}`.

Rate units: all rates are fixed-point with 6 decimals ("e6") of the notional. Example: holding fee 0.10% per night = 1,000 e6.

### 3.3 Price source and the `PriceProof`
**SUPERSEDED (decisions 43 to 50): the launch sources are the GMX public API and the Hyperliquid xyz oracle, see 8.2 and handover/design-decisions.md. The Stork text below is kept for the later licensed-feed path.**
**Where prices come from.** The evaluator subscribes to Stork's signed price stream (Stork's own API/WebSocket; the Solana feed account holds only the latest value, so it is not the source).
Each update it receives is a signed message: feed id, timestamp (ns), value (18 decimals), publisher signature(s). The evaluator:
1. keeps **every update it uses** in an append-only sample file per market (`samples-<market>-<date>.bin`), and publishes the file hash in each journal batch (4.5);
2. converts to `price_e12 = floor(quantized_value / 10^6)` and applies the half-spread (section 7.1; a `Fill` in a market whose half-spread is 0 in the account's rule set is refused before this step, decision 64);
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
  AND the time of the last price CHANGE of the feed is within `max_age_secs` of `now`. The windows alone decide open or closed, because neither GMX nor Hyperliquid reports a closed market (decision 49); the staleness test never uses `updatedAt`. Holidays need an admin window change.
- **Closed market:** no new orders and no fills of pending orders; existing positions **may be held** with a frozen mark (the last sample); the evaluator does not trigger floors, SL, TP or pending orders on a frozen mark. The holding fee keeps accruing every calendar night (section 9.3).
- **Reopen:** the first fresh sample is the next price. If it is past an SL, a floor or a pending order, they trigger at that sample's price
  (a gap is a gap; the trader bears it, consistent with the confirmed rule "even through a gap").
- **Reduce-only status (2):** admin sets it before a feed migration or delisting; opens are rejected, closes allowed.
- **Index markets** (SPX, NDX, DAX, Nikkei, FTSE) are created with `status = 0` until a custom feed exists. Enabling one is an admin action.

### 3.5 Quote currency
**Decision 48:** JPY is quoted as yen per USD by the source; the evaluator inverts it (bid = 1/ask, ask = 1/bid, mid = 1/mid, rounded against the trader) and the market trades JPYUSD. EUR and GBP are USD per unit. No conversion market is needed at launch.
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
| I-1 | Signer is `config.evaluator`. Account status is `Active`. An exposure-increasing `Fill` also needs `held == 0` (`AccountHeld`) and a market that is priced in the account's rule set, `spread_e6[market] != 0` (`MarketUnpriced`, decision 64; checked right after the open-allowed checks and before the proof and quantity checks, in `post_journal` and `report_fail` alike). A `Close` never needs it. `pass_pending != 2` (a confirmed pass freezes the journal; `pass_pending == 1` still allows posting so the evaluator can bring the journal up to date before confirming). |
| I-2 | `batch.first_seq == acct.seq + 1` and `batch.prev_hash == acct.journal_hash`; each event extends the hash (8.3). `acct.seq` and `acct.journal_hash` are updated at the end. |
| I-3 | Time: `ev.time >= acct.last_batch_time` (never backwards) and `ev.time <= now + 2 s`. |
| I-4 | Each referenced market exists, and every `PriceProof` carries that market's current `feed_id`, a value equal to the event price (3.3), and `ev.time - max_age_secs <= proof.time <= ev.time + 2 s` (a price older than the market's `max_age_secs` cannot justify an event). |
| I-5 | `Close`: `pnl == floor(sign x qty x (exit - entry) / DEN)` computed by the program; `quick == (ev.time - timer_start <= 45)`; `credited` is `pnl` for a loss or a non-quick slice, `floor(pnl/2)` for a quick profit (7.4); `b += credited`; if `quick && pnl > 0` then `q_tally += floor(pnl/2)`. |
| I-6 | `Fee`: `amount >= 0`; `b -= amount`. (Holding fee and commission, both rounded up by the engine.) |
| I-7 | After `b` changes: if `b >= 1.04 S` then `latch = 1`. The static floor is `0.96 S` if latched, else `0.92 S`. |
| I-8 | `q_tally > 0.028 S` after any event: the batch is rejected unless its last event is `Fail{PhaseQuick}`; use `report_fail`. |
| I-9 | `TightStart`: `p_open > 0.01 S`, `r == b`, `l_start == min(static floor + 0.04 S, r)`, `l_level == l_start + max(0, g_peak - 0.01 S)` and `tight_on == 0` before. `FloorRaised`: `tight_on == 1`, new `g_peak > old`, `l_level` recomputed by the formula. `TightEnd`: `flat == true`. |
| I-10 | `Breach`: `tight_on == 1`, `flat == true` after the batch, `b <= l_level` (position closed at the sample, so balance equals equity), `lock_until = ev.time + 12 h`, `tight_on = 0`. Any `Fill` that opens or adds has `ev.time >= acct.lock_until`. |
| I-11 | `Fail` (only through `report_fail`; M7: the batch is `events..., Fail, Meta` with `Meta.flat == 1`, the `Fail` right before `Meta`, `Fail.b_after == b`, and `post_journal` rejects a batch that ends flat at or below the static floor with `MustReportFail`): reasons `StaticFloor` (needs `flat == true` and `b <= static floor`) or `PhaseQuick` (needs I-8). Sets status Failed and calls the vault. Violation fails are NOT journal events: they go through `fail_for_violation` (6.1). |
| I-12 | `flat` is stored as given. The chain cannot check it. |
The program also re-derives, never trusts: the static floor, the quick limit, the pass target, the payout amount. None of these are fields the evaluator can set.

### 4.5 Decisive samples, the sample archive and signed order intents
**Decisive samples** (each carries a `PriceProof` in the journal): every fill (open, add, reduce, close, flip), every SL/TP/limit/stop trigger, the tight-mode start sample,
every sample that raised `g_peak`, the breach sample, the fail sample. Ordinary mark-to-market samples are not posted.
**Sample archive.** The evaluator writes every price update it used, per market, to an append-only file and stores the SHA-256 of the file prefix used in each batch (`samples_hash`). Setl8 publishes the archive (object storage with a public URL, plus a copy
the admins keep). A verifier can recompute the path: given the account's fills (known from the journal) and the sample file it must reproduce every floor, ratchet and trigger the evaluator claims, and find any breach that the evaluator did not claim.
**Signed order intents.** Each trader order is signed by the trader wallet (ed25519, message `SL8LEV|account|nonce|order bytes`). The evaluator stores them and commits `intents_hash` per batch. A fill without a matching intent
(or SL/TP/limit/stop already on file) is visible to the verifier as a forged trade. On-chain verification of these signatures (ed25519 precompile via the instructions sysvar) is an optional hardening, not in v1.
**M9 settles the formats** (handover/verifier-formats.md): ONE logical stream of 68-byte records for all markets (not a file per market), `samples_hash`/`intents_hash` are prefix hashes found by one hashing pass over the record/line boundaries, intents are ed25519-signed 48-byte `OrderIntent`s.
**M10 settles the evaluator's side of these formats** (handover/module-10.md): a second is processed once, when it closes, from the account as it stood at its start; orders execute when their second closes; a holding fee dated R is written before anything dated R and a `Meta` never passes it; the batch rules (units, flat endings) are listed there, with five disagreements between this design, the program and the verifier that the differential campaign found.
**Honest limit.** If the evaluator drops a sample that would have caused a breach, nothing on-chain shows it; a verifier with its own copy of the Stork stream can. Setl8 should run an independent second recorder (watchdog) for this reason.

### 4.6 Public verifier and watchdog
- `setl8-verify` (a CLI, module M9): reads the program's event log (via any RPC/archive/indexer), the sample archive and the intents file; replays with the `engine` crate; prints a verdict per account, per phase, and per pending pass. Open source so traders can check their own accounts. **M9 as built:** `setl8-verify account|watch --events --rules --archive --intents [--snapshot] [--second-archive] [--json] [--as-of-seq N]`, exit codes 0 valid, 1 mismatch, 2 input error.
- **Duty during a pending pass (decision 67):** the vault abandons a record idle for 7 days and the longest allowed pass delay is 6 days; the evaluator should still call `touch_activity` (the trader `release_account`) at least once during the delay so that a delay set at the maximum plus the idle time before the confirmation cannot reach 7 days.
- Setl8 runs it as a **watchdog**: on every `PassRequested` event it replays that account and alerts both admins with `VALID` or `MISMATCH` inside the first hour, so the 2-day veto window has operational meaning (13.3, OPEN_QUESTIONS).

### 4.7 What happens if the evaluator is offline, wrong, or hacked
| Case | Effect | Protection |
|---|---|---|
| Evaluator offline | No new journal; traders cannot trade (the backend is the venue); passes cannot be requested if the last batch left the account non-flat; vault activity clocks may run (touch_activity stops) | `release_account` by the trader refreshes the vault clock; backend runbook. No on-chain recourse for a long outage (OPEN_QUESTIONS #10). |
| Evaluator bug miscomputes a floor or pnl | Either a wrong fail/breach or a wrong pass | I-5..I-10 catch arithmetic classes of error; verifier catches the rest; veto catches a wrong pass; a wrong fail has no automatic remedy (OPEN_QUESTIONS #9). |
| Key stolen: attacker fabricates a pass for his own accounts | He needs a `request_advance` from a trader wallet, then 2 days | Watchdog alerts, both admins veto, daily cap bounds the rest, vault caps at 32% of size per record chain. Rotation: both admins `set_evaluator`. |
| Key stolen: attacker fails honest traders | Possible only through floor or quick-limit fails, which need fabricated losses (I-11). A stolen key can also `hold_account` anyone (a pause, not a loss); only both admins can fail-for-violation, ban, release or unban. | Visible in the journal; no automatic remedy. Reset price is paid again by the trader (OPEN_QUESTIONS #9). |
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

**What lands on Solana (posted by the evaluator about 14:03; the table shows three transactions for readability. Measured in M11: a proof is 61 bytes, not 300, so size would allow fewer; a batch carries at most 3 proofs and the four proofs of this example need two transactions):**
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
| `RuleSet` | `["ruleset", version u16]` | 552 | 0.0047328 | admins, immutable after creation |
| `Market` | `["market", market_id u16]` | 128 | 0.00178176 | admins |
| `WalletProfile` | `["wallet", trader]` | 56 | 0.00128064 | program |
| `TradingAccount` | `["acct", trader, challenge_id u64 le]` | 207 | 0.0023316 | program only (via evaluator or trader instructions) |
| `PayoutTally` (no discriminator) | `["payout_tally"]` | 25 | 0.00106488 | program only (vault reads it) |
| `sector_authority` (address only, holds no data) | `["setl8_sector_authority"]` | none | none | signs vault CPIs |

There is no bounty vault (decision 21 superseded). `TradingAccount` seeds mirror the vault's `TraderState` (`["trader_state", product_program_id, trader_wallet, challenge_id]`), so the two records of one challenge are always found from the same three inputs.
Rent for a new `TradingAccount` (about $0.35 at an assumed $150/SOL; SOL price is an assumption) is paid by the trader as `payer` and is **returned** by `close_account` once the account is terminal for 7 days.

### 5.1 `TradingAccount` layout (207 bytes, Borsh, fields in this order)
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
| `pass_pending` | u8 | 1 | 0 none, 1 requested by the trader and awaiting the evaluator, 2 confirmed and waiting out the delay, 3 = the violation marker (written only by `fail_for_violation` on the Failed account it ends; no pass state; decision 60) |
| `vetoes` | u8 | 1 | count of vetoed passes (saturating) |
| `bump` | u8 | 1 | |
| `tight_on` | u8 | 1 | |
| `held` | u8 | 1 | 1 while paused for hedging review (2.9) |
| `r` | i64 | 8 | B recorded when tight mode began |
| `g_peak` | i64 | 8 | highest `E - R` claimed since tight mode began |
| `l_level` | i64 | 8 | tight floor L (0 when off) |
| `pass_requested_at` | i64 | 8 | time of `request_advance`. **M8 note: on a Failed account the field holds `reset_at`, the time `reset_account` consumed the record (0 = not reset yet); `end_account` zeroes it, nothing else reads it once the account is terminal.** |
| `pass_proposed_at` | i64 | 8 | time of `confirm_pass` (the delay counts from here). **M7 note: once the account is Failed, Abandoned or Graduated nothing can be pending, so the field then holds `terminal_at`, the time of that transition; `close_account` counts 7 days from it.** |
| **Total** | | **207** | |

The static floor is not stored: it is `0.96 S` if `latch == 1`, else `0.92 S`. Positions, pending orders and the last trades live in the evaluator's database and in the journal; the chain never needs them.

### 5.2 `WalletProfile` (56 bytes)
`trader Pubkey, next_challenge_id u64, active_count u8, bump u8, banned u8, held_count u8, pad [u8;4]` (M7: `held_count` = number of this wallet's accounts currently held; `ban_wallet` is refused while it is above 0). `banned = 1` blocks `open_account` and `reset_account` for that wallet (`WalletBanned`). `active_count <= 5` is enforced at `open_account` and `reset_account` (decision 28). It is decremented when an account becomes Failed,
Graduated or Abandoned. `next_challenge_id` hands out unused vault challenge ids (the vault rejects a reused id anyway).

### 5.3 `SectorConfig` (270 bytes)
`admin_a, admin_b, evaluator, vault_program, sl8_wallet` (5 x Pubkey), `current_ruleset u16`, `market_count u16`, `paused u8`, `bump u8`,
`pass_delay_secs u32` (default 172,800), `max_payout_per_window u64` (micro-USD, value open), `window_secs u32` (default 86,400), `window_start i64`, `window_total u64`, `reserved [u8;64]`.

### 5.4 `RuleSet` (552 bytes, Borsh, immutable after creation)
`version u16`, `fee_bps u16` (swap-cost fee, 40), `tier_count u8` (10), `ladder_len u8` (5), `leverage_x [u16;8]` (per class), `holding_fee_e6 [u32;8]` (per class), `commission_e6 [u32;8]` (per class, PENDING),
`tier_size [u64;10]` (micro-USD), `tier_cost [u64;10]` (micro-USD), `reset_ladder_bps [u16;5]`, `spread_e6 [u16;128]` (half-spread per market id, up to 128 markets; PENDING), `bump u8`, `reserved [u8;31]`.
8 + 2+2+1+1 + 16 + 32 + 32 + 80 + 80 + 10 + 256 + 1 + 31 = 552. (The 904 in the first draft was an estimate before the layout existed.)
Launch values (decisions 13, 15, 17, 26): `leverage_x` = 50, 20, 20, 20, 10, 5, 5, 2; `holding_fee_e6` = 100, 200, 400, 200, 400, 400, 1000, 1600; `tier_size` $2.5K, 5K, 10K, 15K, 20K, 25K, 50K, 75K, 100K, 200K; `tier_cost` $30, 55, 99, 145, 189, 235, 489, 689, 859, 1,599; `reset_ladder_bps` = 10000, 11500, 12000, 12750, 13000.

---

## 6. Instruction list

Notation: `[S]` signer, `[W]` writable, `[R]` read-only. Compute figures are **estimates** to be measured in the test workspace (module M11); see section 11.

**Event delivery (decision 67, audit F-27).** Every program event is an `emit_cpi!` self-CPI instruction, including the admin ones (`LimitsChanged`, `EvaluatorChanged`, `PausedChanged`, `RuleSetCreated`, `MarketCreated`, `MarketFeedChanged`, `MarketUpdated`, `WalletBanned`, `WalletUnbanned`, `PassCancelled`): names and fields are unchanged, only the delivery moved from a truncatable program log to an inner instruction. Nine instructions therefore end their account list with `event_authority` (PDA `["__event_authority"]`, read-only) and `program` (this program, read-only): `set_evaluator`, `set_limits`, `set_paused` (the shared `AdminAction` struct), `create_ruleset`, `create_market`, `update_market`, `ban_wallet`, `unban_wallet`, `cancel_pass`. A client built from the old IDL must append the two accounts. Cost: about +14,600 compute units and +32 to 64 bytes per call of these nine instructions (handover/module-11.md).

### 6.1 Admin
| Instruction | Signers | Effect |
|---|---|---|
| `init_config` | SL8 admin [S], Rov admin [S], payer | Creates `SectorConfig` (evaluator, delay, window, cap) with `current_ruleset = 0` and `PayoutTally` (0/0, magic `SL8TALLY`). Only the two compiled-in admin keys may call it (no one else can front-run the singleton). Rule set 1 is a separate `create_ruleset` call (its arguments are too big to share a transaction). |
| `create_ruleset` | both admins | Creates `RuleSet` version N+1 (immutable) and sets `current_ruleset`. Applies only to NEW accounts. |
| `create_market` / `update_market` | either admin | Create or change a market: status, feed id, sessions, caps. **Free feed remap per decision 24.** Emits `MarketFeedChanged`. |
| `set_evaluator(new_key)` | both admins | Rotates the evaluator key. Effective immediately. |
| `set_limits(pass_delay_secs, max_payout_per_window, window_secs)` | both admins | Delay is clamped to `[1 day, 6 days]` (decision 67: the vault abandons a record that is idle for 7 days, so a longer delay could lose a confirmed pass; a 6-day delay leaves a day of margin) so no admin pair can set a zero delay by accident; cap > 0. |
| `set_paused(bool)` | pause: either admin; unpause: both | Blocks `open_account`, `reset_account`, `request_advance`, `finalize_pass`. `post_journal`, `report_fail`, `confirm_pass`, `reject_pass` keep working. |
| `release_hold` | both admins | Clears `held` after review (3b in 2.9). |
| `fail_for_violation(evidence_hash)` | both admins | Requires `held == 1`. Sets status Failed, CPI `flag_trader_failed`, `active_count - 1`, sets the wallet's `banned = 1`, clears any pending pass, emits `ViolationFail{evidence_hash}`. Needs the same accounts as `report_fail`. |
| `ban_wallet(evidence_hash)` | both admins | Sets `banned = 1` (creates the profile if missing) for a linked wallet with no held account; emits `WalletBanned`. |
| `unban_wallet` | both admins | Clears `banned` (the way to correct a wrongful ban). Does not revive failed accounts. |
| `veto_pass` | both admins | Cancels a pending pass, state 1 or 2 (10.7). Cannot do anything else. |

### 6.2 Trader
| Instruction | Signers | Accounts (R/W) | Effect |
|---|---|---|---|
| `open_account(tier_index, mint)` | trader [S], payer (= trader) [S] | trader; `WalletProfile` [W, init if needed]; `TradingAccount` [W, init]; config [R]; rule set [R]; `sector_authority` [R]; vault: program [R], `product_registry` [W], `trader_state` [W, created by vault], `vault_state` [R], `trader_token_account` [W], mint [R], pool token account [W], SL8 token account [W], token program [R], system program [R] | Checks cap 5 and not paused; transfers the 0.4% fee (trader -> SL8 token account, round UP); CPI `deposit_fee(amount = tier cost)` signed by `sector_authority`; initialises the account (`b = S`, phase 1, rule set snapshot); emits `Opened` (not part of the hash chain; `seq = 0`, `journal_hash = h_0`, see 8.3). The evaluator learns of it from the event. |
| `reset_account(prev_challenge_id)` (+ the mint as an account) | trader [S], payer (= trader) [S] | the accounts of `open_account` in their order, plus the previous `TradingAccount` [W] (after the wallet) and the previous vault `trader_state` [W] (after the new one); no new `WalletProfile` (it must exist) | Checks in order: sector not paused; payer is the trader; rule set is the current one; wallet not banned; the previous account is the one named, locally `Failed` (not Active, not Held, not Abandoned) and not reset yet; fewer than 5 active accounts (the failed one is not counted, the new one will be); the registry is the vault-owned `product_registry` PDA of THIS product (address, owner and header checked, never trusted from the caller); the price `reset_price::reset_amount_micro(tiers, size, phase-1, ladder)` read from that registry NOW. Then CPI `deposit_reset(amount, trader_wallet, product_program_id, prev_challenge_id, new_challenge_id, reset_phase = phase-1)` signed by `sector_authority`; the sector passes NEITHER `payout_count` NOR `account_size` (the vault copies both from the failed record) and reads the new vault record back (its `payout_count + 1` is the new `next_request_id`, its `account_size` must equal the failed account's). Then the 0.4% fee on the reset price (assumption A-FEE-1, still open; the same `ruleset.fee_bps` as `open_account`, rounded up, trader -> SL8 wallet). The new account starts at the SAME phase with fresh state (`B = S`, no positions, tight off, latch off, `q = 0`, no lock, genesis `sha256("SL8LEV" || new account)`, `seq 0`) under the CURRENT rule set; the previous account is marked reset (`reset_at` in `pass_requested_at`). Emits `Opened` (as a purchase, with the failed phase) and `AccountReset`. No free-reset path: there is no price, phase or payout argument. Vault side: decision 51; this side: decision 58, handover/module-8.md. |
| `request_advance` (the "request payout and move to next phase" button) | trader [S], payer (= trader) [S] | `TradingAccount` [W]; config [R]; `sector_authority` [R]; vault `product_registry` [R], `trader_state` [W] | Requires: Active, not held, not paused, `flat == 1`, `tight_on == 0`, `pass_pending == 0`, `b >= 1.08 S` (program-maintained), phase not failed. Sets `pass_pending = 1`, `pass_requested_at = now`, CPI `record_activity`, emits `PassRequested{phase, amount 0.08 S (0 in phase 1), b, seq, journal_hash}`. No money moves and the phase does not change yet. |
| `cancel_pass` | trader [S] | `TradingAccount` [W] | Only if `pass_pending == 1` and `now - pass_requested_at >= 3,600 s` (the evaluator never answered). Sets `pass_pending = 0`. |
| `release_account` | trader [S] | `TradingAccount` [W], vault `trader_state` [W], registry [R] | Only calls `record_activity`; if the vault says `Abandoned` the local account is marked Abandoned and the wallet slot is freed. Otherwise it is a harmless activity refresh and the trader's own way to keep the vault clock alive if the evaluator stops touching it. |
| `close_account` | trader [S] | `TradingAccount` [W, closed] | After 7 days in a terminal state: returns rent to the trader. The event stream remains the record. |

Trading itself (orders, SL/TP, cancels, closes) is NOT an instruction. It is an API call to the evaluator, signed by the trader wallet (4.5).

### 6.3 Evaluator
| Instruction | Signers | Accounts | Effect |
|---|---|---|---|
| `post_journal(batch)` | evaluator [S] (also fee payer) | `TradingAccount` [W]; config [R]; markets referenced [R]; event accounts | Applies a `JournalBatch` (8.2) under invariants I-1 to I-12, updates `b`, `q`, `latch`, tight state, `lock_until`, `seq`, `journal_hash`, `flat`; emits each event via `emit_cpi!`. |
| `hold_account(evidence_hash)` | evaluator [S] | `TradingAccount` [W] | Requires Active and a non-zero `evidence_hash`. Sets `held = 1`, emits `AccountHeld{evidence_hash}`. Reversible, moves nothing. Blocks exposure-increasing `Fill`, `request_advance`, `confirm_pass` and `finalize_pass` (a pending pass stays pending but cannot finalize while held). |
| `report_fail(batch, reason)` | evaluator [S] | as `post_journal` plus `WalletProfile` [W], `sector_authority` [R], vault program [R], `product_registry` [R], `trader_state` [W] | Applies the final batch (must end with `Fail{reason}`), checks I-11, sets status Failed, CPI `flag_trader_failed`, `active_count - 1`. |
| `confirm_pass` | evaluator [S] | `TradingAccount` [W]; config [R] | Requires `pass_pending == 1`, `held == 0`, `flat == 1`, `b >= 1.08 S`. **Phase 1:** starts phase 2 at once (`b = S`, `q = 0`, latch 0, tight off) and emits `PhaseAdvanced`. **Phases 2 to 5:** `pass_pending = 2`, `pass_proposed_at = now`, emits `PassConfirmed`. The evaluator calls it only after its own engine agrees (4.10). |
| `reject_pass(reason)` | evaluator [S] | `TradingAccount` [W] | Requires `pass_pending == 1`. Sets it to 0 and emits `PassRejected{reason}` (NotFlat, JournalBehind, TargetNotReached, Other). Cannot touch money. |
| `touch_activity` | evaluator [S] | `TradingAccount` [W], `sector_authority` [R], vault `product_registry` [R], `trader_state` [W] | CPI `record_activity` (skipped if `now - last_activity_pushed < 86,000 s`), handles `Abandoned` as in 10.2. The backend calls it when the trader sent a signed order. |

### 6.4 Anyone
| Instruction | Signers | Accounts | Effect |
|---|---|---|---|
| `finalize_pass` | any payer [S] | `TradingAccount` [W]; config [W]; `payout_tally` [W]; `WalletProfile` [W]; `sector_authority` [R]; vault: program, `product_registry` [W], `trader_state` [W], `vault_state` [W], `payout_claim` [W, created by vault], system program | Requires `pass_pending == 2`, `held == 0`, `now >= pass_proposed_at + pass_delay_secs`, not paused, daily cap not exceeded (10.6). CPI `request_payout(amount = 0.08 S, proposed_request_id = next_request_id)`, reads return data: `Paid` -> tally bump, `next_request_id + 1`, window counter, next phase starts fresh (or Graduated after phase 5); `Abandoned` -> status Abandoned. |

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
Buy fills at `ask = ceil(mid x (1 + h))`, sell at `bid = floor(mid x (1 - h))`, with `h` the half-spread of the market in the account's rule set (e6 units). `h = 0` is not a price: an exposure-increasing fill in such a market is refused (decision 64, 3.2); a closing fill never is, and its price at `h = 0` would be the mid.

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
`AccountHeld`, `HoldReleased`, `ViolationFail`, `WalletBanned`, `WalletUnbanned`, `PhaseAdvanced`, `Graduated`, `Abandoned`, `MarketFeedChanged`. The first group (`Fill` to `Meta`) is posted by the evaluator inside a `JournalBatch` and extends the hash chain; the rest are emitted by the program itself with `emit_cpi!` and carry the account's current `seq` as a reference (`at_seq`) but are NOT part of the hash chain, so the evaluator's sequence numbers are never disturbed by lifecycle events. Reason codes are enums, not free text.
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
  source: u8,            // 0 Stork (not built), 1 Pyth (not built), 2 GMX tickers (unsigned), 3 GMX signed report (Chainlink blob), 4 Hyperliquid (unsigned), 255 mock (tests only). Decisions 43, 44, 46.
  feed_id: [u8;32],      // = sha256("SL8FEED" || source || utf8 symbol), e.g. symbol "BTC" for GMX, "xyz:AAPL" for Hyperliquid (decision 46)
  timestamp_ns: u64,
  quantized_value: i128, // 18 decimals, converted from the source's own units by the evaluator
  signed_update: Vec<u8> // source 3: the 32-byte sha256 of the signed Chainlink blob (the blob itself is in the archive, decision 47); EMPTY for sources 2 and 4; at most 512 bytes
}
```
Measured (M11, Borsh): a proof is 61 bytes (sources 2 and 4) or 93 bytes (source 3); `Fill` is 66 bytes, `Close` 67, `Fee` 20, `TightStart` 50, `FloorRaised` 26, `TightEnd` 9, `Breach` 26, `Fail` 19, `Meta` 76; a batch adds 48 bytes. The largest transaction (3 proofs of source 3, 8 market accounts) is 1,181 bytes of the 1,232. *Was: "about 300 bytes per proof (UNVERIFIED, M2), about two proofs per transaction".*
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
build uses placeholders marked `PENDING` in the test fixtures, never in production data. A market must have `h > 0` in the rule set an account trades under (decision 64); the test fixtures use the named constant `TEST_HALF_SPREAD_E6 = 1` where they need the smallest priced market.

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

### 9.5 Reset price (decisions 17, 18, 51)
`reset price = ceil(CURRENT tier cost x ladder[failed_phase_idx] / 10000)` through the shared `setl8_shared_interfaces::reset_price` (u128, rounded up), ladder (phase 1 to 5, index 0 to 4) = 10000, 11500, 12000, 12750, 13000 bps. The tier is the registered `challenge_sizes` entry whose size equals the failed account's size, read as it is NOW: a tier repriced or removed by `update_product_config` changes or blocks every pending reset of that size (never reprice or remove a tier while Failed records exist), and tier sizes must be unique. The program computes the amount with `reset_price::reset_amount_micro`. The 0.4% fee applies on top (assumption A-FEE-1, confirm). The account resumes in the same phase it failed. The table below is for the launch tier costs.
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
- A held account (2.9) cannot be requested, confirmed or finalized; a pending pass stays pending until `release_hold` (the delay clock keeps running) or `fail_for_violation` (which clears it).
- A reject, cancel or veto leaves phase, `b`, `q`, latch and tight state exactly as they were.

### 10.4 Payout flow against the queued-claim vault
1. `request_advance` (trader), then `confirm_pass` (evaluator) as in 10.3.
2. After `pass_delay_secs` (default 2 days, clamped to 1 to 6 days, decision 67) anyone calls `finalize_pass`; the caller pays the `PayoutClaim` rent as `payer`.
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
`veto_pass` requires both admin signatures in one transaction and `pass_pending` 1 or 2 (the violation marker 3 of a failed account is not a pending pass and stays). It sets `pass_pending = 0`, increments `vetoes` (saturating), emits `PassVetoed{admin keys, slot}`. It never touches `b`, phase, payouts or the vault, and it cannot be used to confirm or finalize anything.
The trader may press the button again. **What admins do after a veto (investigate, reset, ban) is policy, not code, and it is in tension with "no manual overrides"; see OPEN_QUESTIONS #8.** A veto can delay a trader indefinitely; the `vetoes` counter and event make that visible.

### 10.8 Vault pause (decision 25)
While the vault pauses this product: `open_account`, `reset_account` and `finalize_pass` fail at the vault (their CPIs revert), so the caller retries later. Journal posts, `report_fail`, `confirm_pass` and `reject_pass` continue (the vault allows `flag_trader_failed` while paused).
The vault freezes inactivity clocks during a pause. The sector `set_paused` is a separate, additional switch.

---

## 11. Compute, transaction size and stack budget

**MEASURED (M11, P19 part C; handover/module-11.md has the full table and how it is produced; `tests-rs/tests/budget.rs` asserts a ceiling per instruction: the measured worst case + 15 %, never above 200,000 CU or 1,232 bytes).** Worst cases, compute units / signed-transaction bytes, with traders of fixed keys and the worst bump-search keys found:
| Instruction | CU | bytes | Notes |
|---|---|---|---|
| `post_journal` | 125,276 | 1,181 | worst CU: 8 events, 3 proofs (2 fills, 2 closes, tight start, floor raise, fee, meta); worst bytes: 3 proofs of source 3 and 8 market accounts. One signature (evaluator = fee payer), no budget instruction. A 1-event batch is about 31,000 CU. |
| `report_fail` | 151,317 | 1,149 | 8 events, 3 proofs; the `flag_trader_failed` call and 5 more fixed accounts on top of `post_journal`. |
| `open_account` | 135,595 | 838 | first account of a wallet, worst key (typical 100,000 to 120,000); the vault's `deposit_fee` creates the `TraderState`. |
| `reset_account` | 158,200 | 911 | worst key (typical 125,000 to 145,000); the vault's `deposit_reset` is about 42,000 of it. |
| `request_advance` | 45,937 | 539 | one `record_activity` call. |
| `confirm_pass` / `reject_pass` / `cancel_pass` / `veto_pass` | 23,971 / 21,688 / 4,699 / 22,643 | 374 / 375 / 307 / 471 | |
| `finalize_pass` | 84,594 | 671 | `request_payout` creates the `PayoutClaim`; worst key. |
| `hold_account` / `release_hold` / `fail_for_violation` | 24,633 / 24,600 / 44,035 | 439 / 504 / 668 | |
| `touch_activity` / `release_account` | 52,145 / 52,151 | 539 | with the vault call, worst key (throttled: about 22,000). |
| `close_account` | 3,337 | 307 | |
| admin: `init_config`, `create_market`, `update_market`, `create_ruleset`, `ban_wallet`, `unban_wallet`, `set_*` | 4,285 to 31,823 | 405 to 984 | three signatures (fee payer + both admins). |
Every instruction is below the default 200,000 units, so no compute-budget instruction is needed anywhere; `post_journal` (1,181 B) and `report_fail` (1,149 B) are the ones near the byte limit. *Was (estimates): `post_journal` 120k to 220k "requests 250k", `report_fail` +20k over it, `open_account` 90k to 130k, pass instructions 15k to 40k, `finalize_pass` 70k to 100k; "default 200k suffices except `post_journal`". The estimate for `post_journal` was too high (the real worst case fits the default) and the one for `report_fail` too low (+26k, not +20k).*

**Transaction size.** 1,232 bytes. Measured wire sizes (Borsh): a proof is **61 bytes** (sources 2 and 4) or 93 (source 3, with the 32-byte digest); `Fill` 66, `Close` 67, `Fee` 20, `TightStart` 50, `FloorRaised` 26, `TightEnd` 9, `Breach` 26, `Fail` 19, `Meta` 76; a batch adds 48. A full batch (8 events, 3 proofs) is well inside one transaction; what splits batches is the proof count (3), the event count (8) and the market count (8), not the bytes. *Was: "a signed proof is about 300 bytes (UNVERIFIED, M2), an event about 100: about two proofs per transaction".* This costs fees (8.4) but no safety: every transaction extends the same hash chain and a later transaction cannot be applied before an earlier one.

**Stack (4,096 bytes per frame).** Accounts are small Borsh structs; instruction account structs with more than a few accounts (`open_account`, `finalize_pass`, `report_fail`) wrap their big members in `Box<>`; `RuleSet` is read with `AccountLoader` (zero-copy, 552 bytes). Measured (`scripts/stack-frames.py`, compiler-reported frames of the localnet build): the largest is `handle_reset_account` at **2,944 bytes**, then the `ResetAccount` and `OpenAccount` account structs at 2,752 and 2,560, `handle_finalize_pass` 1,984; none is over 4,096.
**Build check:** `scripts/build-test-so.sh` (and `scripts/anchor-build-checked.sh`, same approach as the vault's script of that name) runs `cargo build-sbf`, fails on any "Stack offset of ... exceeded max offset of 4096" message, and the program has `const_assert!`s on every account size in section 5.

**Rent** (rent-exempt minimum, (128 + length) x 6,960 lamports): `TradingAccount` 207 B 2,331,600; `WalletProfile` 56 B 1,280,640; `Market` 128 B 1,781,760; `RuleSet` 552 B 4,732,800; `SectorConfig` 270 B 2,770,080; `PayoutTally` 25 B 1,064,880; vault `TraderState` 115 B 1,691,280. Opening the first account of a wallet costs 5,303,520 lamports of rent + 5,000 fee and 862.436 USDC at the $100K tier (module-11.md).

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
| 6019 | `PassPending` | `post_journal` or `report_fail` while `pass_pending == 2` |
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
| 6034 | `InvalidLimits` | delay outside 1 to 6 days, zero cap or zero window |
| 6035 | `WalletBanned` | `open_account` or `reset_account` by a banned wallet |
| 6036 | `EvidenceMissing` | hold, violation fail or ban without a non-zero `evidence_hash` |
| 6037 | `AccountHeld` | exposure-increasing `Fill`, `request_advance`, `confirm_pass` or `finalize_pass` on a held account |
| 6038 | `NotHeld` | `fail_for_violation` or `release_hold` on an account that is not held |
| 6039 to 6103 | | defined in the module notes (`handover/module-5a.md` to `module-7.md`): `InvalidFlag`, `InvalidTokenAccount`, `InvalidVaultProgram`, `EngineStateMismatch`, the journal errors 6043 to 6088, `MustReportFail` and the fail errors 6089 to 6099, `AlreadyHeld`, `WalletHasHeldAccount`, `AccountNotTerminal`, `InvalidVaultAccount` |
| 6104 | `ResetTierInvalid` | `reset_account`: not exactly one registry tier has the failed account's size |
| 6105 | `ResetPhaseOutOfRange` | `reset_account`: the failed phase is beyond the registered reset ladder |
| 6106 | `ResetPriceOverflow` | `reset_account`: the reset price does not fit in a u64 |
| 6107 | `AccountAlreadyReset` | `reset_account`: the previous account was already reset (`ResetNotAllowed` 6027 stays the error for an account that is not Failed) |
| 6108 | `ViolationNoReset` | `reset_account`: the previous account was failed by `fail_for_violation` (DESIGN 2.9: no reset), checked before any CPI |
| 6109 | `MarketUnpriced` | `post_journal` / `report_fail`: an exposure-increasing `Fill` in a market whose half-spread in the account's rule set is 0 (decision 64; a `Close` is never refused by it) |

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
| A-6 | Cross-account hedging: wallet A long, wallet B short, one passes | **Prohibited (decision 36, 2.9): backend detects and holds, both admins review, then fail and ban.** Residual: detection is heuristic (different sizes, delays, correlated markets such as EURUSD vs GBPUSD, and fresh wallets can slip through; a false match holds an honest trader until review); a ban is per wallet and a new wallet evades it. Original economics follow. HIGH severity economics: the loser costs a reset of 100% to 130% of the tier price (about $859 to $1,117 on $100K); the winner collects $8,000 per phase. **Option C improves detection**: the backend sees every order of every account in real time and can flag opposite positions of the same size and time across wallets. Detection output feeds a veto (policy, OPEN_QUESTIONS #7). |
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
- **Deploy rule for a new market (decision 64).** A market added after launch is traded only by accounts whose rule set prices it. Create it as `status = 0`, publish the rule set version that gives it a non-zero half-spread, enable the market when that version is current. `launch-config verify` refuses a launch rule set with a zero half-spread for any market id.

---

## 14. Admin powers, upgrade authority and the "no admin key on money" rule
| Power | Held by | Money impact |
|---|---|---|
| Create or change markets, **re-point feeds, enable or disable** | either admin (EXCEPTION: no delay, decision 24) | Indirect: a rigged feed can mislead the evaluator; passes still face delay, veto and cap |
| Publish a new rule set | both admins | Only new accounts; existing accounts keep their snapshot |
| Release a hold, fail an account for violation, ban or unban a wallet | both admins | Can fail an account (no refund) but cannot approve or move a payout |
| Rotate the evaluator key | both admins | None directly; a new key can post journals |
| Set pass delay (1 to 6 days), daily payout cap and window | both admins | Can slow or throttle payouts; cannot create one |
| Veto a pending pass | both admins together | Can cancel a payout request; cannot approve, create or move anything |
| Pause sector | either (unpause: both) | Stops new purchases and finalization; never moves money |
| Upgrade program | 2-of-2 multisig (SL8 admin + Rov admin) | Code could request payouts up to the vault cap; this is the biggest lever; burn decision pending |
The evaluator key can post journals, report floor/quick-limit fails, hold accounts for review, confirm or reject passes and refresh activity. Failing for violation, banning, releasing a hold and unbanning need both admins. It cannot request or finalize a payout, open accounts, change config, or pass the delay.
No instruction in this program takes an admin-supplied or evaluator-supplied amount into a vault payout or transfers trader or payout tokens. Payout amounts are the fixed formula in 10.4.

---

## 15. Assumptions register (each is also in OPEN_QUESTIONS.md)
| Id | Assumption | Where |
|---|---|---|
| A-RULE-1 | Tight floor L is a frozen ratchet; effective floor = max(F_s, L) | 2.3 |
| A-RULE-2 | Quick tolerance: strictly greater than 0.028 S, checked when the quick slice closes | 2.4 |
| A-RULE-4 | Hedging = opposite positions in the same market, detected off-chain, held until both admins review (decision 36); thresholds to be written | 2.9 |
| A-RULE-3 | RESOLVED (decision 31): `l_start = min(f_static + 0.04 S, R)` | 2.3, 4.4 |
| A-ORA-1 | Stork signed-update format, size, fee and access for non-enterprise users (all UNVERIFIED) | 3.3, 11 |
| A-ORA-2 | The backend can obtain Stork's signed stream without an enterprise contract | 3.3 |
| A-FEE-1 | CONFIRMED by Rovik 2026-10-17 (decision 63): the 0.4% fee also applies on resets | 9.5 |
| A-FEE-2 | Holding fee uses the mark at application time, not at 22:00 | 9.3 |
| A-MKT-1 | Launch with USD-quoted markets; cross pairs and non-USD indices wait for conversion feeds | 3.5 |
| A-CAP-1 | At most 10 pending orders per account and 8 open positions (engine limits, enforced off-chain; the open-position cap exists to bound valuation work and per-account proofs) | 16 |
| A-ADM-1 | Either admin may change markets and feeds (no delay); both admins for rule sets, limits, veto, evaluator rotation and upgrade | 14 |
| A-VLT-1 | RESOLVED 2026-10-13 (decision 51): the vault (core commit 9a5c179, shared-interfaces v0.5.0) prices resets as ceil(current tier cost x ladder / 10000) | 1.4, 9.5 |
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
| `crates/evaluator` (M10) | The reference evaluator as a deterministic library: order book, valuation per second, rules, journal batches (the SPLIT rule), sample archive, intents file, pass answers. No I/O, no clock, no dependencies beyond `engine` and `journal-types`. Golden vectors in `crates/evaluator/tests/golden/`. | The production evaluator is Setl8's backend. **Language decision is open (OPEN_QUESTIONS #2):** recommended Rust (same engine, no port). A backend in another language reproduces the golden files bit for bit; the three crates build for `wasm32-unknown-unknown` (checked in M10). Findings of the differential campaign: `handover/module-10.md`. |
| `tests/` (separate workspace, LiteSVM) | Isolated test workspace, same convention as the vault, loading the built program `.so`; no dependency on the program crate's dev-dependencies. | Section BUILD_PLAN. |
Engine limits (off-chain): 8 open positions and 10 pending orders per account; margin check at order time and at fill time; orders deleted on fail, breach lock, phase change (decision 27).
