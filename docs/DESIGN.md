# setl8-lev-trading — DESIGN

Status: **DRAFT FOR REVIEW. Complete first pass (sections 1 to 15). No program code is written until Rovik approves this file.**
Last updated: 2026-10-09. Companion files: `OPEN_QUESTIONS.md`, `BUILD_PLAN.md`, `../handover/design-decisions.md`.

Sections marked CONFIRMED were restated to Rovik and confirmed. Sections marked ASSUMPTION
are my reading of an unstated edge; each one is also tracked in OPEN_QUESTIONS.md.

---

## 1. Scope, architecture, trust model

### 1.1 What this program is
`setl8-lev-trading` is a Solana sector program (Anchor 0.32.1) for Setl8's leveraged-trading challenge product.
A trader buys a challenge account (10 sizes, $2.5K to $200K), trades 82+ **synthetic markets** priced by an oracle,
and passes phases for fixed payouts. There is **no matching engine, no order book and no token custody here**.
Trades are simulated balance changes. The only real money moves through the core vault by CPI:
purchase and reset fees in, queued payout claims out.

### 1.2 Hub and spoke
```
 trader wallet ──signs──► setl8-lev-trading (this program) ──CPI, signed by sector_authority PDA──► core vault
                               │  ▲                                                                (deposit_fee, deposit_reset,
   anyone (keepers) ──crank───►│  │ reads                                                           record_activity, request_payout,
                               │  └── Stork feed accounts (price, publish time)                     flag_trader_failed)
                               └── emits journal events (emit_cpi!) + keeps a rolling hash in the account
```
- `sector_authority` = PDA `["setl8_sector_authority"]` under THIS program id; it signs every vault CPI with `invoke_signed`.
- This program never holds trader or payout money. Token movement happens inside the vault, from the trader's own token account.
  The one exception is the 0.4% swap-cost fee (section 9.2), a plain SPL transfer signed by the trader straight to the SL8 wallet.

### 1.3 Trust model (who must be trusted for what)
| Party | Trusted for | Not trusted for |
|---|---|---|
| Oracle publishers (Stork) | The signed price and its timestamp | Nothing else |
| Core vault | Holds money, caps total payout exposure (8% of size per pass, max 4 passes per record = 32%) | n/a |
| This program's code and its upgrade authority (2-of-2: SL8 admin + Rov admin) | Everything below | n/a |
| Admins | Creating/disabling markets and re-pointing feeds (**EXCEPTION, decision 24**), publishing new rule sets for NEW accounts | Moving trader or payout money: no instruction does it |
| Setl8's keeper bot | Liveness only (sampling density) | Correctness: any wallet can submit the same evidence |
| Traders | Their own signatures | Choosing which prices count (the program decides, see section 4) |

### 1.4 Facts this design is built on (verified in the read-only repos, 2026-10-09)
- `setl8-shared-interfaces` is at **v0.4.0** (the earlier note of v0.3.2 was stale). `request_payout` queues a claim and needs
  7 accounts; `flag_trader_failed` takes the registry read-only. Vault commit `be97396` does not yet enforce the payout tally,
  but the sector must maintain it from day one (`PayoutTally`, section 10.5).
- Vault `INACTIVITY_LIMIT_SECS` = 604,800 (7 days), `ACTIVITY_THROTTLE_SECS` = 86,400, `MAX_RESET_PHASES` = 8.
- Vault `deposit_reset` today prices a reset as a percent of account SIZE. The locked ladder (decision 17) is a percent of
  tier PRICE. **Vault change required** (prompt in `handover/vault-change-reset-price.md`). BUILD-BLOCKING for the reset module only.
- Stork's Solana feed account stores only the latest value (`timestamp_ns: u64`, `quantized_value: i128`, 18 decimals). Neither
  Stork nor Pyth provides verified high/low/OHLC. Both facts drive section 4.

### 1.5 Locked decisions this document implements
Numbers refer to `handover/design-decisions.md`: rules 1 to 12; leverage 13; prices 15; 0.4% fee 16; reset ladder 17 and
vault change 18; 12 h lock 19; margin 20; keepers 21; closed markets 22; holding fee 23 and 26; admin exception 24; vault pause 25;
order types 27; 5 accounts per wallet 28; 2-of-2 upgrade authority 30.

---

## 2. Challenge rules (CONFIRMED by Rovik, 2026-10-09)

Notation (S = account size, e.g. $100,000). All money is 6-decimal USDC base units (u64/i64/i128 as
specified later). Examples below use dollars.

### 2.1 Balance and equity
- **B (closed balance):** starts at S each phase. A closed slice with profit P adds P if it is a normal
  slice and P/2 if it is a quick slice (rounded DOWN, i.e. against the trader). A loss is always credited in full.
- **P (open profit):** sum of unrealized PnL over all open net positions (one per market), marked at accepted oracle prices.
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
  At start we record R = B (closed balance at that instant) and L_start = F_s(at that moment) + 0.04 S.
- **While on**, G = E - R (profit since the mode began, realized and open; closing a winner does not reduce G).
  G_peak = highest G over accepted samples since the mode began.
  **L = L_start + max(0, G_peak - 0.01 S).** L only rises. It does not fall when P drops below 1% or goes negative.
- **Ends** only when every position is closed (flat). The next time P > 0.01 S a fresh mode starts.
- ASSUMPTION A-RULE-1: L is a frozen ratchet level. If F_s latches to 0.96 S in the middle of a mode (by realizing
  profit), L is NOT recomputed from the new F_s (otherwise realizing profit could instantly breach). The effective
  floor is max(F_s, L). Confirm with Rovik.
- ASSUMPTION A-RULE-3: the sample that starts tight mode is not checked against the new floor; the floor applies from the next sample (section 4.5).
  Consequence of the confirmed rule: a trader within 3% of the static floor cannot hold more than about 1% of open profit. Confirm with Rovik.

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
- **Pressing the button** moves to the next phase: fresh B = S, F_s = 0.92 S, latch cleared, tight mode off, Q = 0.
  For phases 2 to 5 it also queues a vault payout claim of 0.08 S.
- After the phase 5 payout the account is Graduated (vault `max_payout_count` = 4).
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

## 3. Markets, oracle adapter, sessions

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
- **`SectorConfig`** (singleton, PDA `["config"]`): admin keys, vault program id, Stork program id, SL8 wallet, bounty parameters,
  `paused` flag, `current_ruleset`, `market_count`.
- **`RuleSet`** (PDA `["ruleset", version u16]`, **immutable once created**): leverage per class, half-spread per market,
  commission per class (pending numbers), holding-fee rate per class, tier price table, reset ladder, fee rate. A `TradingAccount`
  stores its `ruleset_version` at purchase. This is the "per-account parameter snapshot": publishing version N+1 never changes an
  account created under version N.
- **`Market`** (PDA `["market", market_id u16]`): class, `status` (0 disabled, 1 enabled, 2 reduce-only), `feed_id [u8;32]`
  (**admin can change it at any time, decision 24**), `max_age_secs`, quantity step and minimum, `max_notional_usd` (per-position
  cap, see OPEN_QUESTIONS), up to 6 weekly session windows, optional `quote_conv_market` (see 3.5). Changing `feed_id` emits
  `MarketFeedChanged{old,new,slot}`.

Rate units: all rates are fixed-point with 6 decimals ("e6") of the notional. Example: holding fee 0.10% per night = 1,000 e6.

### 3.3 Oracle adapter interface
The program never reads a vendor account directly. It calls one function:

```rust
/// Price in 1e-12 USD units per 1 unit of the base asset. 1.0800 -> 1_080_000_000_000.
pub struct OraclePrice { pub price_e12: u64, pub publish_time: i64 /* unix seconds */ }

pub trait OracleAdapter {
    /// `feed` is the vendor account passed in the transaction. Must verify owner, address derivation from `feed_id`,
    /// positivity, `publish_time <= now + 2`, and `now - publish_time <= max_age_secs`. Never trusts caller-supplied fields.
    fn read(feed: &AccountInfo, feed_id: &[u8; 32], now: i64, max_age_secs: u16) -> Result<OraclePrice>;
}
```
- **Stork implementation (primary).** Check `feed.owner == config.stork_program`; check `feed.key == find_program_address(["stork_feed", feed_id], stork_program)`;
  read `timestamp_ns` and `quantized_value` from the `TemporalNumericValueFeed`; `price_e12 = quantized_value / 10^6` (round DOWN for
  bids, the program applies spread afterwards); `publish_time = timestamp_ns / 10^9`. Pull-based: the client puts the Stork update
  instruction(s) **before** ours in the same transaction.
  **UNVERIFIED, must be checked against the real SDK in module M2:** the exact account byte offsets, how many feed updates fit in one
  1,232-byte transaction (I assume 3 to 4 with a signed-update blob of about 200 to 250 bytes each), Stork's per-update fee on Solana,
  and whether non-enterprise keepers can get the signed stream at all (contact-sales, OPEN_QUESTIONS #1).
- **Pyth implementation (fallback, only if verified free for non-index markets).** Same trait. Not built until verified.
- **Optional hardening.** Self-verify a signed blob inside our program with the `secp256k1_recover` syscall behind the same trait. It would let
  a keeper present an older signed price (see 4.9), but needs Stork's exact signed-message format, which is not verified.

### 3.4 Sessions and closed markets (decision 22)
- Each market has up to 6 weekly windows `(start_minute_of_week, end_minute_of_week)` in UTC. `market_open(now)` = now falls inside a window
  AND the feed's `publish_time` is within `max_age_secs` of `now`. Holidays need no admin action: the feed goes stale, so the market reads as closed.
- **Closed market:** no new orders and no fills of pending orders; existing positions **may be held** with a frozen mark (the last accepted mark);
  floors, SL, TP and pending orders do not trigger on a frozen mark. The holding fee keeps accruing every calendar night (section 9.3).
- **Reopen:** the first fresh sample is the next price. If it is past an SL, a floor or a pending order, they trigger at that sample's price
  (a gap is a gap; the trader bears it, consistent with the confirmed rule "even through a gap").
- **Reduce-only status (2):** admin sets it before a feed migration or delisting; opens are rejected, closes allowed.
- **Index markets** (SPX, NDX, DAX, Nikkei, FTSE) are created with `status = 0` until a custom feed exists. Enabling one is an admin action.

### 3.5 Quote currency
P&L is in USD. A pair quoted in another currency (EURJPY quoted in JPY, DAX in EUR) needs a USD conversion. Design: `Market.quote_conv_market`
points to another market whose price is USD per one unit of the quote currency; every sample of the first market must be accompanied by a fresh
sample of the second, and the second's `publish_time` joins the snapshot coherence check (4.3). **Open question (OPEN_QUESTIONS):** simplest launch
is USD-quoted markets only; cross pairs and non-USD indices are added when the conversion feed exists.

---

## 4. Core problem: verifying the price path without trusting a backend

### 4.1 The problem
The rules in section 2 are about the **path**: "the first moment equity touched the floor", "SL hit at 03:00", "open profit passed 1% and the floor switched".
A normal exchange sees every tick. Solana does not: the program runs only when someone sends a transaction, and then it sees one number per market.
If a Setl8 server decided "the floor was hit" and told the chain, a hacked or dishonest server could fail traders or save them. We want the chain to
decide, from evidence anyone can check.

### 4.2 What the oracle can and cannot prove (checked, 2026-10-09)
| Question | Answer | Consequence |
|---|---|---|
| Does Stork or Pyth give a verified high/low/OHLC for an interval? | **No.** Only point prices with timestamps. | The path is only observable through discrete samples. |
| Does the Stork feed account keep history? | **No**, only the latest value. | A late reader cannot go back and fetch the old price from the chain. |
| Is a point price verifiable? | Yes: publisher-signed (secp256k1/EIP-712), checked by Stork's program when the update is pushed. | We trust samples, not our backend. |
| Can an old signed price be replayed later? | Not for Stork through the feed account. Only if our program verifies the signed blob itself (unverified format). | Section 4.9: we do NOT depend on replay. |

**Therefore:** the on-chain truth is "the rules applied to the sequence of accepted samples". Sampling density decides how close that is to the true path.
No design can make the chain see a spike that no one sampled. The honest goal is: (1) nobody can profit by choosing which samples count,
(2) sampling is dense and funded, (3) every settlement price is a real signed price, (4) the residual risk is bounded and written down.

### 4.3 Definitions
- **Sample**: `(market, price_e12, publish_time)` returned by the oracle adapter inside the current transaction (age at most `max_age_secs`, default 10 s, tunable per market).
- **Mark**: bid for a long, ask for a short, where `bid = mid x (1 - half_spread)`, `ask = mid x (1 + half_spread)`. A position is valued at the price it could be closed at.
- **Snapshot**: one sample for every market with an open position (plus the traded market, plus quote-conversion markets). Its time `T = min(publish_time)` over all samples used
  (a snapshot is as old as its oldest component).
- **Coherence**: all samples in a snapshot must have `publish_time` within `W = 30 s` of each other. Positions whose stored mark is within `W` of the newest sample may reuse the stored mark,
  so a keeper with 8 open positions can spread the price updates over two or three consecutive transactions (`refresh_marks`) and finish with `crank`.
- **Sample clock** `t_last`: per account, the `T` of the last accepted snapshot. **Every new snapshot must have `T > t_last`.** It only moves forward.
- **Equity** `E = B + P`, with `P` from marks (spread already inside the mark) minus the estimated closing commission once commission numbers exist. `B` already includes holding fees applied in step 2.

### 4.4 Monotonic state (what may only ever move one way)
| Variable | Direction | Why this matters |
|---|---|---|
| `t_last` (sample clock) | only forward | Stops replay of an old, favourable price. Order of events is the order of publish times. |
| `g_peak` (peak profit since tight mode began) | only up while the mode is on | The tight floor `L` is a function of it, so `L` can never fall. |
| `l_level` (tight floor) | only up while on; reset only when the mode ends (flat) | Matches the confirmed rule. |
| `f_static` (static floor) | 0.92 S, then 0.96 S forever in the phase once latched; reset only by a new phase | The latch is one-way. |
| `q_tally` (credited quick profit) | only up within a phase | Quick losses never reduce it (confirmed). |
| `seq` (journal sequence) and `journal_hash` | seq only up; hash chains | Any missing event shows up as a hash mismatch. |
| `phase` | only up (1 to 5) | A reset creates a NEW account record, never rewinds an old one. |
| `lock_until` | set only by a breach, never shortened | A second breach cannot happen while locked (no positions can open). |

Everything else (equity, `P`, margin used) is **derived on demand** from stored marks, never trusted from a caller.

### 4.5 One pipeline, not three instructions
The brief suggested `checkpoint`, `trigger_sl_tp` and `trigger_breach` as separate keeper instructions. **Recommendation: one instruction, `crank`, with a fixed internal order**, because with
three separate entry points a caller chooses the ORDER in which events are applied, and order changes outcomes (e.g. take the TP before the breach, or the SL before the floor).
With one pipeline the caller chooses only which evidence to submit; the program decides everything else. Order inside `crank` (and inside every trader action, which runs steps 1 to 4 first):

1. **Validate evidence**: adapter reads, freshness, session open, coherence, `T > t_last`. Failure aborts the transaction (`StaleSample`, `IncoherentSnapshot`, `MarketClosed`).
2. **Accrue holding fees** for every 22:00 UTC rollover crossed since each position's `fee_day`.
3. **Compute `P`, `E`** from the snapshot marks.
4. **Static floor**: if `E <= f_static` then close all at the marks, status `Failed`, CPI `flag_trader_failed`. Stop.
5. **Tight floor breach** (only if tight mode was already on before this sample): if `E <= l_level` (the level as of the PREVIOUS sample) then close all at the marks, `lock_until = T + 43,200`, end tight mode. Stop.
6. **Per-position SL then TP** (only if the market is open): fill at the mark if the mark is at or through the level. SL and TP are closes, so they can be quick slices and update `B`, `q_tally`.
7. **Pending orders** in markets covered by the snapshot: fill if the trigger condition holds, checking margin at fill time (reject silently if it fails, leaving the order pending, and emit `OrderRejected`).
8. **Tight-mode bookkeeping**: if off and `P > 0.01 S` then start (record `R = B`, `l_level = f_static + 0.04 S`, `g_peak = E - R`). If on, update `g_peak = max(g_peak, E - R)` and `l_level = max(l_level, L_start + max(0, g_peak - 0.01 S))`.
   If flat, tight mode ends.
9. **Phase-fail rule**: if a quick slice in step 6 pushed `q_tally > 0.028 S` the phase fails (same effect as step 4).
10. Store marks, set `t_last = T`, emit journal events, pay the keeper bounty only if step 4, 5 or 9 settled something.

**A-RULE-3 (new edge case, needs Rovik's confirmation; default shown).** Step 5 uses the level as of the previous sample. So the sample that STARTS tight mode is not checked against the new floor.
Reason: if an account is within 3% of its static floor (for example `B = 0.93 S`), `l_level = f_static + 0.04 S = 0.96 S` is above its own equity at the moment the mode starts, which would close a winning position on the
same sample that made it a winner. With the default the breach is declared at the next sample (`E <= l_level`). Either way a trader sitting less than 3% above the static floor can hold at most about 1% of open profit. This is a
consequence of the confirmed rule, not something I added; it is flagged so you can choose.

### 4.6 Which price settles what
| Event | Price used | Who bears slippage |
|---|---|---|
| Market order | Ask (buy) or bid (sell) of the sample in the trader's own transaction | Trader |
| Limit order | The first accepted sample at or better than the limit, filled AT the sample (price improvement kept) | Neither (trader benefits) |
| Stop (entry) order | The first accepted sample at or through the trigger, filled at the sample | Trader |
| Stop-loss | Mark of the first accepted sample at or through the SL | Trader (gap) |
| Take-profit | Mark of the first accepted sample at or through the TP (price improvement kept) | Trader benefits |
| Tight-floor breach | Marks of the sample that showed `E <= L` (confirmed: "that sample price") | Trader |
| Static-floor fail | Marks of the sample that showed `E <= F_s`, even through a gap | Trader |
| Holding fee | Mark at the time the fee is applied (see 9.3 for the approximation) | Trader |

### 4.7 What happens when no price arrives in time
| Scenario | What the program does | Honest consequence |
|---|---|---|
| Keeper outage, price moves through a floor and partly back | Nothing is declared; no sample showed the breach | Trader benefits (a rare lucky path). Bounded by the real price move inside the gap. |
| Keeper outage, price keeps going | First accepted sample after the gap decides; closes at that price | Trader bears the gap (could be far past the floor). Equity may be far below `F_s`: the account just fails. |
| Weekend or holiday | Market closed, marks frozen, triggers off, holding fee accrues (3.4) | First fresh sample on reopen settles everything. |
| Feed stops updating on an OPEN market (outage) | Samples fail `StaleSample`; cranks revert. Trader actions in that market revert. | Nothing can trigger. Admin can set `reduce-only`. Positions are held at the last mark. Residual risk R-3. |
| Bad print (single wrong price) | A single price that breaches a floor closes the account | Risk R-4. Optional mitigation: require two consecutive samples at least 2 s apart for a FAIL or breach (config flag, default OFF, OPEN_QUESTIONS #9). |
| Stale pending limit order | It fills at the first accepted sample that crosses it, price improvement kept | Standard. |

### 4.8 Keeper economics (decision 21)
- **Who:** Setl8's bot runs `crank` per account with open positions every 5 s (target) and every 60 s with pending orders only. Anyone else may submit the same instruction.
- **Evidence supply:** the keeper builds the transaction `[Stork update ix(s), crank ix]`. The program checks the feed account, not who pushed it.
- **Bounty:** `config.bounty_lamports` (default 1,000,000 lamports = 0.001 SOL) is moved from the program-owned `bounty_vault` PDA to the caller of any `crank` that performed step 4, 5 or 9
  (a **floor settlement**: static fail, tight breach or quick-limit phase fail). SL, TP and pending-order fills pay NO bounty (Setl8's bot serves them; they only affect the trader's own result, never the vault), and plain mark refreshes pay nothing,
  so spamming cranks or self-farming earns nothing (A-4, A-5). **Refinement of decision 21 for Rovik to confirm.** First valid transaction wins; later ones fail with `StaleSample` (their `T` is not greater than `t_last`)
  and only cost their own fee.
- **Funding:** `fund_bounty` accepts SOL from anyone. SL8 wallet tops it up. It is one-way: traders pay nothing, and the pool can only be spent through the bounty path or returned to the SL8 wallet by 2-of-2.
- **Pool exhausted:** cranks still work, just unpaid. The pool balance is public. Alert threshold lives in the bot, not on-chain.
- **Sizing example:** 1,000 settlements a month x 0.001 SOL = 1 SOL a month. Even at 100,000 settlements it is 100 SOL. Cheap compared with the vault's payout exposure.
- **Griefing:** a griefer cannot keep an account "alive" or "dead": `crank` never calls `record_activity`, so only the trader's own actions refresh the 7-day inactivity clock.

### 4.9 The race: a later favourable sample beats an earlier adverse one
Scenario: at 10:00:00 the price touched the tight floor; a keeper holds that signed price. At 10:00:03 the trader sends a transaction with the price now back above the floor.
If the trader lands first, `t_last = 10:00:03` and the keeper's 10:00:00 sample is rejected (`T <= t_last`). The breach is never recorded.
- **Why not allow older evidence?** Because then every state would have to be re-wound when a late sample arrives (positions, balance, journal), and an attacker could race to reorder history. Forward-only is simple and auditable.
- **What bounds the damage?** The trader can only choose WHEN to send, among real prices. The keeper's cadence (5 s) limits how long an adverse state can go unsampled. The attack gain is limited to price moves that
  start and reverse inside one sampling gap. For FX majors that is a few hundredths of a percent; for crypto alts it can be several tenths.
- **Optional hardening (not in v1, UNVERIFIED format):** self-verified signed blobs with a short dispute window would let a keeper submit a slightly older adverse price. Revisit after the audit if bounty data shows a real leak.
- **Selective sampling in the other direction:** a trader running his own fast bot samples only favourable spikes for a TP. Same bound, same mitigation (keepers sample both ways; the TP fill still uses a real price). Residual risk R-1.

### 4.10 The five questions from the brief
| Question | Answer |
|---|---|
| (a) Permissionless keeper instructions with incentives? | Yes: one `crank` with a settlement bounty (4.8) plus Setl8's own bot. |
| (b) Which events must be monotonic? | Table 4.4. |
| (c) No price in time? | Table 4.7: first accepted sample settles; trader bears gaps; nothing triggers on stale or closed markets. |
| (d) Does the oracle give verified high/low? | **No** (checked). Design does not assume it. |
| (e) Honest failure and attack analysis? | Section 13. The unsolved items are written there, not hidden. |

### 4.11 Recommendation
**"Sampled ratchet with a forward-only clock":** rules run on accepted samples only; the sample clock, tight-floor ratchet, static latch and quick tally are monotonic; one `crank` pipeline applies events in a fixed order;
a funded permissionless keeper set provides density; every settlement uses a real signed price; no historical replay is needed. Residual risks R-1 to R-4 (section 13) are accepted and listed for your review.

### 4.12 Worked example: +1.2%, tight floor switches on, price reverses through it
Setup: S = $100,000, phase 2, B = $100,000.000, `f_static` = $92,000, EURUSD (class 0, leverage 1:50), half-spread 0.003% (30 e6), no commission, no holding fee yet.
Trader buys 1,000,000 EUR at market. Entry = ask = 1.08000 x 1.00003 = **1.0800324**. Notional = $1,080,032.40. Margin = notional / 50 = **$21,600.648**. `T0 = 14:00:00 UTC`.
(All values below are produced by integer math with the rounding rules of section 7.4; the module M3 test vectors reproduce them exactly.)

| Time | Who | Mid | Bid (mark) | P | E | State change written on-chain |
|---|---|---|---|---|---|---|
| 14:00:00 | Trader, market buy | 1.08000 | 1.0799676 | -64.80 | 99,935.20 | Position slot 0 created: qty +1,000,000 EUR, avg_entry 1.0800324, margin 21,600.648, timer_start = T0, mark = 1.08000 @ T0, fee_day set. `t_last = T0`. Event `Fill` seq 1 (floor_before 92,000, mode Static), hash updated. CPI `record_activity` -> Recorded, `last_activity_pushed = T0`. |
| 14:00:30 | Keeper | 1.08060 | 1.080567582 | +535.18 | 100,535.18 | Mark and `t_last` only. P is below 1% (1,000), so no mode change. No event. No bounty. |
| 14:01:00 | Keeper | 1.08130 | 1.081267561 | +1,235.16 | 101,235.16 | **Tight mode ON** (P > 1,000): `tight_on = 1`, `R = 100,000.000`, `l_start = 96,000`, `g = 1,235.161`, `g_peak = 1,235.161`, `l_level = 96,000 + 235.161 = 96,235.161`. This sample is not checked against `l_level` (A-RULE-3). Event `ModeSwitch{TightStart, floor 92,000 -> 96,235.161, price 1.081267561}` seq 2. |
| 14:01:30 | Keeper | 1.08310 | 1.083067507 | +3,035.11 | 103,035.11 | New high: `g_peak = 3,035.107`, `l_level = 96,000 + 2,035.107 = 98,035.107`. Event `FloorRaised{96,235.161 -> 98,035.107}` seq 3. |
| 14:02:00 | Keeper | 1.08110 | 1.081067567 | +1,035.17 | 101,035.17 | E is above `l_level`; `g` fell below `g_peak`. **Nothing falls**: `g_peak` and `l_level` unchanged. Mark and `t_last` only. |
| 14:02:30 | Keeper | 1.07800 | 1.07796766 | -2,064.74 | 97,935.26 | Step 5: `E = 97,935.26 <= l_level 98,035.107` -> **breach**. Close slot 0 at the mark: realized -2,064.74, slice age 150 s > 45 s so a normal slice, `B = 97,935.26`. Slot cleared, `open_count = 0`, margin freed, pending orders (none) deleted, `tight_on = 0`, `r = g_peak = l_level = 0`, `lock_until = 14:02:30 + 12 h = 02:02:30 next day`, closed trade written to the ring buffer, `trades_closed = 1`. Events `Close` seq 4 and `ModeSwitch{Breach, floor 98,035.107 -> 92,000, reason E<=L}` seq 5. Bounty 0.001 SOL to the keeper. No vault CPI (phase not failed). |

**If the keeper had been late** (no sample between 14:02:00 and a later one):
- Next sample at mid 1.07700: `P = -3,064.71`, `E = 96,935.29 <= l_level` -> breach at THAT price, `B = 96,935.29`.
- Next sample at mid 1.06000: `P = -20,064.20`, `E = 79,935.80 <= f_static 92,000` -> **FAIL**: close at that price, `B = 79,935.80`, status `Failed`, CPI `flag_trader_failed`, `WalletProfile.active_count - 1`, events `Close` and `Fail`. The trader bears the gap.

**If the trader raced:** at 14:02:31 the trader submits his own transaction with mid 1.08110 (E back above the floor) before the keeper's 14:02:30 transaction lands. Then `t_last = 14:02:31` and the keeper's sample is rejected: no breach is recorded.
This is risk R-1: it needs a favourable price within seconds of an adverse one, and the 5 s keeper cadence plus the bounty keep that gap small.

---

## 5. Accounts and byte sizes

Sizes include the 8-byte Anchor discriminator. Rent = (128 + bytes) x 6,960 lamports (rent-exempt minimum; the formula is Solana's current default, to be re-checked on the target cluster).
Large accounts use **zero-copy** (`#[account(zero_copy)]`, `#[repr(C)]`, no `Vec`, explicit padding, `AccountLoader`), so they are never copied onto the 4,096-byte BPF stack.
All money is `i64` micro-USD (6 decimals); prices are `u64` in 1e-12 USD; quantities are `u64`/`i64` in 1e-6 base units.

| Account | PDA seeds (this program unless noted) | Bytes | Rent (SOL) | Written by |
|---|---|---|---|---|
| `SectorConfig` | `["config"]` | 248 | 0.00261696 | admins |
| `RuleSet` | `["ruleset", version u16]` | 904 | 0.00718272 | admins, immutable after creation |
| `Market` | `["market", market_id u16]` | 128 | 0.00178176 | admins |
| `WalletProfile` | `["wallet", trader]` | 56 | 0.00128064 | program |
| `TradingAccount` (zero-copy) | `["acct", trader, challenge_id u64 le]` | 2,608 | 0.01904256 | program only |
| `BountyVault` (SOL only, no data) | `["bounty"]` | 8 | 0.00094656 + balance | program, anyone may fund |
| `PayoutTally` (no discriminator) | `["payout_tally"]` | 25 | 0.00106488 | program only (vault reads it) |
| `sector_authority` (address only, holds no data) | `["setl8_sector_authority"]` | none | none | signs vault CPIs |

`TradingAccount` seeds mirror the vault's `TraderState` (`["trader_state", product_program_id, trader_wallet, challenge_id]`), so the two records of one challenge are always found from the same three inputs.
Rent for a new `TradingAccount` (about $2.9 at an assumed $150/SOL; SOL price is an assumption) is paid by the trader as `payer` and is **returned** by `close_account` once the account is terminal for 7 days.

### 5.1 `TradingAccount` layout (2,608 bytes)
| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | discriminator | 8 | |
| 8 | `trader` | Pubkey 32 | owner |
| 40 | `challenge_id` | u64 | vault challenge id |
| 48 | `size_s` | u64 | S in micro-USD |
| 56 | `b` | i64 | closed balance B |
| 64 | `r` | i64 | B recorded when tight mode began |
| 72 | `g_peak` | i64 | highest `E - R` since tight mode began |
| 80 | `l_level` | i64 | tight floor L (0 when off) |
| 88 | `f_static` | i64 | current static floor level |
| 96 | `q_tally` | i64 | credited quick profit this phase |
| 104 | `lock_until` | i64 | unix seconds, 0 = none |
| 112 | `t_last` | i64 | sample clock |
| 120 | `last_activity_pushed` | i64 | last `record_activity` CPI time |
| 128 | `next_request_id` | u64 | mirrors vault `payout_count + 1` |
| 136 | `seq` | u64 | journal sequence |
| 144 | `created_at` | i64 | |
| 152 | `phase_started_at` | i64 | |
| 160 | `journal_hash` | [u8;32] | rolling hash head |
| 192 | `trades_closed` | u32 | |
| 196 | `ruleset_version` | u16 | snapshot of the rule set |
| 198 | `status` / `phase` | u8, u8 | status: 0 Active, 1 Failed, 2 Abandoned, 3 Graduated; phase 1 to 5 |
| 200 | `tight_on`, `latch`, `open_count`, `pending_count`, `ring_head`, `bump`, `next_order_id` (u16) | 8 | `latch` = static floor shifted to 0.96 S |
| 208 | `positions[8]` | 8 x 96 | see below |
| 976 | `orders[10]` | 10 x 48 | pending orders |
| 1456 | `ring[16]` | 16 x 72 | last 16 closed trades |
| 2608 | end | | |

`Position` (96): `market_id u16, flags u8 (side bit), pad`, `qty i64` (signed: long > 0), `avg_entry u64`, `sl u64`, `tp u64`, `timer_start i64`, `opened_at i64`, `mark_mid u64`, `mark_time i64`, `margin u64`, `fee_day i64`, `fees_acc i64`.
`Order` (48): `id u32, market_id u16, kind u8 (0 limit, 1 stop), side u8`, `qty u64`, `trigger u64`, `sl u64`, `tp u64`, `created i64`.
`ClosedTrade` (72): `market u16, side u8, flags u8 (bit0 quick), pad`, `qty u64`, `entry u64`, `exit u64`, `pnl i64`, `fees i64`, `opened i64`, `closed i64`, `seq u64`.

Open positions are capped at **8 per account** (`MAX_OPEN_POSITIONS`). Reason: each open market needs a feed account in the transaction, and a 1,232-byte transaction cannot carry more than a handful of signed
Stork updates. The cap is a constant and a decision for Rovik to revisit (OPEN_QUESTIONS #8).

### 5.2 `WalletProfile` (56 bytes)
`trader Pubkey, next_challenge_id u64, active_count u8, bump u8, pad`. `active_count <= 5` is enforced at `open_account` and `reset_account` (decision 28). It is decremented when an account becomes Failed,
Graduated or Abandoned. `next_challenge_id` hands out unused vault challenge ids (the vault rejects a reused id anyway).

---

## 6. Instruction list

Notation: `[S]` signer, `[W]` writable, `[R]` read-only. "Pipeline" = steps 1 to 4 of section 4.5 (validate evidence, accrue fees, compute equity, static-floor check) run before any trader action.
Compute figures are **estimates** to be measured in the test workspace (module M10); see section 11.

### 6.1 Admin
| Instruction | Signers | Effect |
|---|---|---|
| `init_config` | SL8 admin [S], Rov admin [S], payer | Creates `SectorConfig`, `BountyVault`, `PayoutTally` (0/0, magic `SL8TALLY`), rule set 1. |
| `create_ruleset` | both admins | Creates `RuleSet` version N+1 (immutable) and sets `current_ruleset`. Applies only to NEW accounts. |
| `create_market` / `update_market` | either admin | Create or change a market: status, feed id, sessions, caps. **Free feed remap per decision 24.** Emits `MarketFeedChanged`. |
| `set_bounty`, `withdraw_bounty` | both admins | Bounty size; return unspent SOL to the SL8 wallet only. |
| `set_paused(bool)` | pause: either admin; unpause: both | Blocks `open_account`, `reset_account` and new opens. Closes, cranks and `flag_trader_failed` keep working. |
| `fund_bounty(lamports)` | anyone | SOL into `BountyVault`. |

### 6.2 Trader
| Instruction | Signers | Accounts (R/W) | Effect |
|---|---|---|---|
| `open_account(tier_index, mint)` | trader [S], payer (= trader) [S] | trader; `WalletProfile` [W, init if needed]; `TradingAccount` [W, init]; config [R]; rule set [R]; `sector_authority` [R]; vault: program [R], `product_registry` [W], `trader_state` [W, created by vault], `vault_state` [R], `trader_token_account` [W], mint [R], pool token account [W], SL8 token account [W], token program [R], system program [R]; `payout_tally` not used | Checks cap 5 and not paused; transfers the 0.4% fee (trader -> SL8 token account, round UP); CPI `deposit_fee(amount = tier cost)` signed by `sector_authority`; initialises the account (B = S, phase 1, `f_static` = 0.92 S, ruleset snapshot); journal `Opened`. |
| `reset_account(prev_challenge_id, mint)` | trader [S], payer [S] | as above plus the previous `TradingAccount` [W] and the previous vault `trader_state` [W] | Previous account must be locally `Failed`; CPI `deposit_reset(amount = ceil(tier cost x ladder[phase-1]), reset_phase = phase-1)`; new account starts at the SAME phase with fresh state; 0.4% fee on the reset price (assumption). Needs the vault change (decision 18). |
| `place_order(kind, side, qty, trigger, sl, tp)` + remaining accounts `[market, feed]*` | trader [S] | `TradingAccount` [W]; config [R]; rule set [R]; markets and feeds [R]; vault `product_registry` [R], `trader_state` [W] (for `record_activity`); `sector_authority` [R]; vault program [R]; event accounts | Pipeline, then: market order fills now (netting, 7); limit or stop is stored. Margin check (free equity >= 0). Rejects while `now < lock_until`. CPI `record_activity` if `now - last_activity_pushed >= 86,000 s`; reads return data; `Abandoned` -> status Abandoned, positions voided, orders deleted. |
| `close_position(market, qty or all)` | trader [S] | same as `place_order` | Market close of part or all; pipeline first. Allowed while locked and in reduce-only markets. |
| `modify_position(market, sl, tp)` | trader [S] | as above | Sets SL/TP (validity rule: long SL < bid < TP, short mirrored). `record_activity`. |
| `cancel_order(order_id)` | trader [S] | `TradingAccount` [W] | Deletes a pending order. `record_activity`. |
| `advance_phase` (the "request payout and move to next phase" button) | trader [S], payer (= trader) [S] | `TradingAccount` [W]; config [R]; `payout_tally` [W]; `sector_authority` [R]; vault: program, `product_registry` [W], `trader_state` [W], `vault_state` [W], `payout_claim` [W, created by vault], system program; `WalletProfile` [W] | Requires: Active, flat, `B >= 1.08 S`, phase not failed. Deletes any pending orders (decision 27). Phase 1: `record_activity` only, then fresh phase 2. Phases 2 to 5: CPI `request_payout(amount = 0.08 S, proposed_request_id = next_request_id)`, **read return data**: `Paid` -> bump `payout_tally` (count + 1, total + amount, checked), `next_request_id += 1`; `Abandoned` -> status Abandoned, no tally change. Then fresh phase state. After phase 5: `Graduated`. |
| `release_account` | trader [S] | `TradingAccount` [W], vault `trader_state` [W], registry [R] | Only calls `record_activity`; if the vault says `Abandoned` the local account is marked Abandoned and the wallet slot is freed. Otherwise it is a harmless activity refresh. |
| `close_account` | trader [S] | `TradingAccount` [W, closed] | After 7 days in a terminal state: returns rent to the trader. The event stream remains the record. |

### 6.3 Keepers (permissionless)
| Instruction | Signers | Accounts | Effect |
|---|---|---|---|
| `refresh_marks` + `[market, feed]*` | anyone (pays fee) | `TradingAccount` [W], config, rule set, markets, feeds | Validates evidence, writes marks and `t_last` for the covered positions. No settlement, no bounty. |
| `crank` + `[market, feed]*` | anyone [S, W] | `TradingAccount` [W]; config [R]; rule set [R]; markets and feeds [R]; `WalletProfile` [W]; `BountyVault` [W]; `sector_authority` [R]; vault program [R], `product_registry` [R], `trader_state` [W]; event accounts | Full pipeline of 4.5. Never calls `record_activity`. May CPI `flag_trader_failed`. Pays the bounty only if step 4, 5 or 9 (a floor settlement) happened. |

### 6.4 Signer rule for vault CPIs
Every vault CPI is signed by `sector_authority` through `invoke_signed` with seeds `["setl8_sector_authority", bump]`. Vault-side extra signers: `deposit_fee`/`deposit_reset` need the trader and a payer;
`request_payout` needs a payer; `record_activity` and `flag_trader_failed` need only `sector_authority`. No admin key appears in any money path.

---

## 7. Netting math

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
4. Every close appends one `ClosedTrade` to the ring buffer and emits `Close`. A flip emits two events (`Close`, then `Fill` for the remainder).

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
| # | Case | Rule |
|---|---|---|
| N-1 | Zero quantity, quantity not a multiple of `qty_step`, or below `qty_min` | `InvalidQuantity` |
| N-2 | Fill exactly flattens | Slot cleared, SL/TP cleared, `open_count -= 1` |
| N-3 | Flip when only the closing slice fits margin | Allowed; remainder is margin-checked |
| N-4 | SL above bid for a long (or TP below) | `InvalidStopLevel` |
| N-5 | SL and TP both satisfied by one gap sample | SL takes precedence (conservative) |
| N-6 | A fill that makes `q_tally > 0.028 S` | Phase fails immediately (step 9) |
| N-7 | Add to a position 44 s after open, then close 20 s later | Timer restarted at the add: closing slice age 20 s -> quick |
| N-8 | Same-tx multiple fills | Sequential, each with its own `seq`; one snapshot, one `T` |
| N-9 | Pending order fills in a market where a position exists | Netting applies normally (merge, reduce or flip) |
| N-10 | Fill price rounds to exactly the trigger | Counts as triggered (touching counts) |

---

## 8. Journal and trade history

### 8.1 What is recorded (decision 3)
Every fill and close, plus every floor/mode change, with: market, side, size, leverage, margin, fill price, average entry after, SL, TP, `floor_before`/`floor_after`, mode before/after with a reason code,
realized P&L, fees, `B` after, the sample `publish_time`, the slot, and the account `seq`.

Event kinds: `Opened`, `Fill`, `Close`, `OrderPlaced`, `OrderCancelled`, `OrderRejected`, `OrderTriggered`, `SlTpSet`, `ModeSwitch{TightStart, TightEnd, Breach, Latch, Fail, PhaseFail}`, `FloorRaised`,
`HoldingFee`, `PhaseAdvanced`, `PayoutQueued`, `Failed`, `Abandoned`, `Graduated`, `MarketFeedChanged`.
Reason codes are an enum, not free text.

### 8.2 Storage options compared
| Option | How | Cost | Pros | Cons |
|---|---|---|---|---|
| A. Record account per closed trade | PDA `["trade", account, n]`, about 192 bytes | rent 0.0022272 SOL each (about $0.33 at $150/SOL); 1,000 trades = 2.23 SOL | Readable on-chain by programs and UIs forever | Costs more than a $30 account earns; someone pays rent for every trade; account explosion |
| B. Events + rolling hash | `emit_cpi!` for each event, `journal_hash = sha256(prev_hash ‖ seq ‖ kind ‖ body)` stored in the account | No rent; about 1.5k CU per event | Cheap; **any missing or altered event is detectable** by re-hashing; carries full detail | Needs an indexer or archive node to read events; programs cannot read old events |
| C. Both | A + B | A's cost | Strongest | A's cost |
| D (recommended) | **B + an in-account ring buffer of the last 16 closed trades + a `PhaseSummary` per finished phase carried in the `PhaseAdvanced` event** | 0 extra rent (ring is inside the 2,608 bytes) | UI shows recent trades with no indexer; full history verifiable from events and the hash | Older trades are only in events |

Genesis hash `h0 = sha256("SL8LEV" ‖ account pubkey)`. Hash input uses canonical Borsh bytes of the event body. `emit_cpi!` (self-CPI inner instruction) is used rather than `msg!` logs because logs truncate at 10 KB per transaction.
Retention depends on RPC/archive providers; the hash makes loss detectable, and Setl8 should keep its own copy (indexer, off-chain, not trusted for correctness).

---

## 9. Prices, fees and costs

### 9.1 Spread (decision 14, numbers PENDING from Rovik's friend)
Oracles give one price. The program synthesises bid and ask: `ask = mid x (1 + h)`, `bid = mid x (1 - h)`, `h` = per-market half-spread in the rule set (e6 units). Longs are marked at bid, shorts at ask, so a position
starts slightly negative (see the -$64.80 in 4.12, which is 0.003% half-spread each side on $1.08M). Spread and commission are simulated balance deductions; no tokens move. Until the friend's numbers arrive the
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

Mechanics: each position stores `fee_day` (index of the last 22:00 rollover already charged; a position opened after the day's rollover starts with today's index, so it is charged from the next rollover). Pipeline step 2 charges
`n = today_index - fee_day` nights at once: `fee = ceil(n x rate_e6 x notional_at_stored_mark / 10^6)`. **Approximation:** the notional uses the mark at the time the fee is applied, not at 22:00 exactly. This is documented, bounded by the
price move in the gap, and tested. Each application emits `HoldingFee`. The rates are broker-derived (IG formulas) and doubled; benchmark rates for October 2026 were not verified.

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
| First and every trader action | `place_order`, `close_position`, `modify_position`, `cancel_order`, `advance_phase`, `release_account` | `record_activity` (skipped if called within the last 86,000 s) | none | `last_activity_pushed` |
| Static-floor or quick-limit fail | `crank` (anyone), or any trader action that runs the pipeline | `flag_trader_failed` | none | status Failed, all closed at marks, `active_count - 1` |
| Tight-floor breach | `crank` / pipeline | none | none | all closed, 12 h lock |
| Reset after fail | `reset_account` | `deposit_reset` | trader, payer | new account at the same phase |
| Pass and move on (phase 2 to 5) | `advance_phase` | `request_payout` | trader, payer | tally bumped, fresh phase |
| Pass phase 1 | `advance_phase` | `record_activity` only | trader | fresh phase 2 |
| Graduation (after phase 5 payout) | `advance_phase` | `request_payout` (the vault also auto-graduates at `max_payout_count`) | trader, payer | status Graduated, `active_count - 1` |
| Abandonment (7 days idle) | vault flips it on `record_activity`/`request_payout`/`mark_abandoned`; we learn via return data in `release_account` or the next trader action | `record_activity` | trader | status Abandoned, positions voided, slot freed |

### 10.2 Return-data discipline
After every `record_activity` and `request_payout` CPI the program reads `get_return_data()` and checks `program_id == vault_program`. `Abandoned` is handled as above; a missing or wrong-program return value aborts the transaction.
`ActivityOutcome::Throttled` and `Recorded` are both fine. Nothing tells the trader a payout was queued unless the outcome was `Paid`.

### 10.3 Payout flow against the queued-claim vault
1. Trader presses the button (`advance_phase`); the program checks flat, `B >= 1.08 S`, not failed.
2. CPI `request_payout(trader_wallet, amount = 0.08 S, product_program_id, challenge_id, proposed_request_id = next_request_id)`. The trader signs as `payer` for the `PayoutClaim` rent.
3. On `Paid`: update `payout_tally` (checked add: `requested_count + 1`, `requested_total + amount`), `next_request_id += 1`, then reset the phase state. On any error the whole transaction reverts, including the tally.
4. The vault's permissionless 5-day heartbeat pays queued claims pro rata; this program never learns when. Nothing in this program can move that money.
5. Fixed formula: `amount = 0.08 S`, once per phase 2 to 5, at most 4 per record chain (vault cap). There is no instruction that takes an amount from the caller.

### 10.4 Vault pause (decision 25)
While the vault pauses this product: `open_account`, `reset_account` and `advance_phase` fail at the vault (their CPIs revert), so the trader retries later. Trading, closes, cranks and `flag_trader_failed` continue (the vault allows the flag while paused). The vault freezes
inactivity clocks during a pause. The sector `set_paused` is a separate, additional switch.

### 10.5 `PayoutTally`
PDA `["payout_tally"]` under this program, 25 bytes, layout `b"SL8TALLY" | version 1 | requested_count u64 LE | requested_total u64 LE`, no Anchor discriminator, held as `UncheckedAccount`, parsed and written with `setl8_shared_interfaces::PayoutTally::{parse, write_into}`.
Increases only on `Paid`, checked arithmetic (fail on overflow), never decreased, closed or re-initialised, including across upgrades. Initialised to 0/0 in `init_config`.

---

## 11. Compute, transaction size and stack budget

**ESTIMATES. Measured values replace them in module M10.**
| Instruction | CU estimate | Notes |
|---|---|---|
| `crank` / `refresh_marks` (8 positions) | 90k to 140k | about 4k CU per position of u128 math, 3.5k per adapter read, 4k journal and hash, up to 15k for `flag_trader_failed` CPI. Stork update instructions are separate instructions in the same transaction and are NOT counted here. |
| `place_order` market, 1 market | 60k to 90k | pipeline on open positions + fill + `record_activity` CPI (about 10k) |
| `open_account` | 120k to 160k | `deposit_fee` CPI creates `TraderState` and moves tokens; account init 2,608 bytes |
| `advance_phase` | 80k to 110k | `request_payout` CPI creates `PayoutClaim` |
Request the compute budget explicitly (the default 200k suffices for most; `crank` requests 250k).

**Transaction size.** 1,232 bytes. Each open market needs a market account and a feed account (via an address lookup table), and each Stork update carries a signed blob. Assumed 3 to 4 updates per transaction (UNVERIFIED, M2).
So a snapshot of 8 positions is two to three transactions (`refresh_marks`, then `crank`), inside the 30 s coherence window `W`.

**Stack (4,096 bytes per frame).** `TradingAccount` is zero-copy (`AccountLoader`), never materialised on the stack. All Anchor `Accounts` structs with more than a few large accounts wrap them in `Box<>`. `RuleSet` and `Market` are read with `AccountLoader` too.
**Build check:** `scripts/anchor-build-checked.sh` (same approach as the vault's script of that name) runs `anchor build`/`cargo build-sbf`, fails on any "Stack offset of ... exceeded max offset of 4096" message, and runs `const_assert!`s on every account size in section 5.

---

## 12. Error list
| Code | Name | Raised when |
|---|---|---|
| 6000 | `Unauthorized` | wrong signer for an admin instruction |
| 6001 | `Paused` | sector paused |
| 6002 | `WalletCapReached` | 5 active accounts already |
| 6003 | `InvalidTier` | tier index not in the rule set |
| 6004 | `AccountNotActive` | status is not Active |
| 6005 | `AccountLocked` | opening or adding while `now < lock_until` |
| 6006 | `MarketDisabled` | status 0 |
| 6007 | `MarketClosed` | outside session or feed stale for an OPEN action |
| 6008 | `ReduceOnly` | open attempted in a reduce-only market |
| 6009 | `StaleSample` | `T <= t_last` or age above `max_age_secs` |
| 6010 | `IncoherentSnapshot` | sample times more than `W` apart, or an open position has neither a fresh sample nor a fresh stored mark |
| 6011 | `BadFeedAccount` | wrong owner, wrong PDA, wrong feed id |
| 6012 | `InvalidPrice` | zero or negative, or future-dated beyond 2 s |
| 6013 | `InvalidQuantity` | zero, off-step, below minimum, above `max_notional` |
| 6014 | `InsufficientMargin` | free equity would go negative |
| 6015 | `InvalidStopLevel` | SL/TP on the wrong side of the mark |
| 6016 | `TooManyPositions` | more than 8 open |
| 6017 | `TooManyOrders` | more than 10 pending |
| 6018 | `OrderNotFound` | cancel of an unknown id |
| 6019 | `NotFlat` | `advance_phase` with open positions |
| 6020 | `TargetNotReached` | `B < 1.08 S` |
| 6021 | (reserved) | unused: pending orders are deleted on phase change, not rejected |
| 6022 | `PayoutNotAccepted` | vault returned anything other than `Paid` where it was required |
| 6023 | `ReturnDataInvalid` | return data missing or from the wrong program |
| 6024 | `ResetNotAllowed` | previous account not locally Failed |
| 6025 | `WrongPhase` | operation not valid in this phase |
| 6026 | `MathOverflow` | checked arithmetic failed |
| 6027 | `BountyPoolInsufficient` | internal only; cranks continue unpaid |
| 6028 | `CloseTooEarly` | `close_account` before 7 days terminal |
| 6029 | `RuleSetMissing` | version not found |

---

## 13. Failure and attack analysis

### 13.1 Residual risks accepted by the design (not solved)
| # | Risk | Size | Where discussed |
|---|---|---|---|
| R-1 | Sampling asymmetry and the race (a trader's later favourable sample beats a keeper's earlier adverse one; a trader samples only favourable spikes) | bounded by price moves inside one sampling gap | 4.9 |
| R-2 | Oracle error or compromise pays out wrong prices | bounded by the vault: 32% of size per record chain | 1.3 |
| R-3 | Feed outage on an open market freezes triggers | trader held at last mark; admin can set reduce-only | 4.7 |
| R-4 | A single bad print can fail an account | one price decides; optional two-sample confirmation (off by default) | 4.7 |
| R-5 | Admin free feed remap | **EXCEPTION (decision 24)**: a rigged feed can create payouts up to the vault cap | 14 |

### 13.2 Attacks considered
| # | Attack | Status |
|---|---|---|
| A-1 | Replay an old favourable price | Blocked: feed account holds only the latest value; `T > t_last`; `max_age_secs`. Needs Stork to reject older timestamps (UNVERIFIED, M2). |
| A-2 | Pass a fake feed account | Blocked: owner and PDA derivation checked for every feed. |
| A-3 | Impersonate this program to the vault | Blocked by the vault: `sector_authority` must be a PDA signed by this program id. |
| A-4 | Spam `crank` to farm bounties | Blocked: bounty paid only when a floor settlement (static fail, tight breach, phase fail) happens; a settlement closes or locks the account, so it is not repeatable. SL, TP and pending fills pay no bounty. |
| A-5 | Self-farm bounties by deliberately breaching | Not profitable: each breach costs about 3% of the account's simulated balance and locks it 12 h; a fail ends the account (price $30+). |
| A-6 | Cross-account hedging: wallet A long, wallet B short, one passes | **NOT solved by this design. HIGH severity economics.** The loser costs a reset of 100% to 130% of the tier price (about $859 to $1,117 on $100K); the winner collects $8,000 per phase. Roughly 7x to 9x return per pass before partner-matching friction. Max 5 accounts per wallet does not stop multiple wallets. See OPEN_QUESTIONS #2 for mitigations (consistency rule, minimum days per phase, minimum risk per trade). |
| A-7 | Latency arbitrage: trade against a stale oracle using a faster real market | Partly mitigated: spread, tight `max_age_secs`, and the quick-trade rule (profit under 45 s credited at half). Not eliminated. |
| A-8 | Keep an account alive or kill it from outside | Blocked: `crank` never calls `record_activity`; only the trader's own actions count. |
| A-9 | Split a position to dodge the 45 s rule | Timer restarts on every add; partial closes do not reset; flip makes a new position. A trader may still hold 46 s to avoid halving, which is the intent. |
| A-10 | Overflow or rounding drift | `i128` intermediates, checked arithmetic, rounding rules in 7.4, mutation tests on netting and ratchet. |
| A-11 | Upgrade key compromise | 2-of-2 multisig (decision 30); burn decision after the audit. |
| A-12 | Compute or account-size DoS | Hard caps: 8 positions, 10 orders, 16 ring entries; zero-copy accounts. |
| A-13 | Wallet cap evasion | Not blocked (new wallet). The cap limits convenience only. |
| A-14 | Front-run a keeper's sample with a favourable one | R-1. |

---

## 14. Admin powers, upgrade authority and the "no admin key on money" rule
| Power | Held by | Money impact |
|---|---|---|
| Create or change markets, **re-point feeds, enable or disable** | either admin (EXCEPTION: no delay, decision 24) | Indirect: a rigged feed can create payouts, capped by the vault |
| Publish a new rule set | both admins | Only new accounts; existing accounts keep their snapshot |
| Set bounty size, withdraw unspent bounty to the SL8 wallet | both admins | Bounty pool only; no trader or payout money |
| Pause sector | either (unpause: both) | Stops new purchases/opens; never moves money |
| Upgrade program | 2-of-2 multisig (SL8 admin + Rov admin) | Code could request payouts up to the vault cap; this is the biggest lever; burn decision pending |
No instruction in this program takes an admin-supplied amount into a vault payout or transfers trader or payout tokens. Payout amounts are the fixed formula in 10.3.


---

## 15. Assumptions register (each is also in OPEN_QUESTIONS.md)
| Id | Assumption | Where |
|---|---|---|
| A-RULE-1 | Tight floor L is a frozen ratchet; effective floor = max(F_s, L) | 2.3 |
| A-RULE-2 | Quick tolerance: strictly greater than 0.028 S, checked when the quick slice closes | 2.4 |
| A-RULE-3 | The tight-mode start sample is not checked against the new floor | 4.5 |
| A-ORA-1 | Stork account layout, per-update size and fee, and that Stork rejects older timestamps (all UNVERIFIED) | 3.3, 4.7, 11 |
| A-ORA-2 | Keepers can obtain Stork signed updates without enterprise contracts | 3.3 |
| A-FEE-1 | The 0.4% fee also applies on resets | 9.5 |
| A-FEE-2 | Holding fee uses the mark at application time, not at 22:00 | 9.3 |
| A-BNT-1 | Bounty only for floor settlements (refines decision 21) | 4.8 |
| A-MKT-1 | Launch with USD-quoted markets; cross pairs and non-USD indices wait for conversion feeds | 3.5 |
| A-CAP-1 | At most 8 open positions and 10 pending orders per account | 5.1 |
| A-ADM-1 | Either admin may change markets and feeds (no delay); both admins for rule sets, bounty and upgrade | 14 |
| A-VLT-1 | Vault changes the reset price formula (decision 18) before module M8 is built | 1.4, 9.5 |
