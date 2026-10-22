# Devnet rehearsal (P23)

The real program (the frozen candidate, re-keyed in a scratch copy), the real core vault (vault HEAD `4ca958c`, re-keyed in a scratch copy) and the real launch configuration (`config/launch.json`: 21 markets, rule set v1, the ten tiers), driven end to end through a real cluster by `tools/devnet-rehearsal` with the reference evaluator, then replayed by the public verifier from the CHAIN'S OWN events.

## 0. Status: read this first

**The run on devnet itself has NOT happened.** The public faucet refused every request (`airdrop request failed ... rate limit`, for 2 SOL, 1 SOL and through the driver's `fund` command); all fourteen throwaway keys hold 0 SOL. The two deployments alone need about 11.6 SOL on devnet. Everything else is built, committed and tested, so the run is a handful of commands once the keys are funded (section 3). Rovik funds the PUBLIC keys of section 2 in the browser (https://faucet.solana.com, or any wallet that holds devnet SOL); nobody has to give the tool a key or a login.

What was done instead, and what the tables below are:

* the whole driver (S0 to S5, both days) was run against a **local `solana-test-validator`** (Agave 4.0.2, `--limit-ledger-size` large): a real bank, real fees, the real runtime and compute metering, the real SPL token, associated-token and system programs, a private cluster. The scenario results of section 4 and the compute numbers of section 5 are from that run. They are **not devnet results**; the compute numbers in particular depend on the runtime version, and devnet runs whatever Agave version it runs today (re-measure on devnet, section 5).
* the day-2 part (the pass delay) ran on a **selftest-only scratch build whose minimum pass delay is 60 s** (the program's minimum is 1 day; this build was never and never will be deployed to devnet). The devnet run uses the real minimum, 86,400 s, and takes two days.
* the 12 h breach lock of account F1 cannot be waited out in a selftest; it is the one day-2 item that has not run anywhere.

## 1. What the rehearsal does and does not cover

| scenario | what | cluster state it needs |
|---|---|---|
| S0 | both programs deployed (hashes and sizes recorded), the two test mints, the token accounts, `init_vault`, `register_product` with the REAL launch values: the ten tiers of `config/launch.json` (unique sizes), `reset_price_bps [10000, 11500, 12000, 12750, 13000]`, `max_payout_count 4`, fee split 6,500 bps | deployed programs |
| S1 | `init_config` (throwaway evaluator, `pass_delay_secs` 86,400, window 86,400, cap $100,000), `create_market` x21 in id order, `create_ruleset` v1 (placeholder spreads and commissions: a mechanics rehearsal) | S0 |
| S2 | eight accounts of seven traders (below) | S1 |
| S3 | seven attacks, landed on the cluster (no preflight) | S2 accounts |
| S4 | compute units and transaction size of every instruction of the run against LiteSVM and the `budget.rs` ceilings, plus the heaviest legal `post_journal` and `report_fail` | S2, S3 |
| S5 | the chain's own events of every honest account fetched by RPC, written in the verifier's formats, replayed with a snapshot taken from the chain; tampered copies must be flagged | S2 |

S2 in detail (tier 0, $2,500, $30; BTC, ETH and SOL with the markets' own feed ids; the prices are a mock oracle's):

| account | trader | what happens |
|---|---|---|
| A1 | trader1 | phase 1 pass story (+10% of S in 60 s, held longer than 45 s), `request_advance`, `confirm_pass` (phase 2 starts at once); a phase 2 pass story, `request_advance`, `confirm_pass`: `pass_pending 2`. While it waits: a journal batch is refused (`PassPending`), `touch_activity`, `finalize_pass` is refused (`PassDelayNotElapsed`). **Day 2:** `touch_activity` (the vault clock), `finalize_pass` (the vault queues a payout claim of 0.08 S, the tally grows, phase 3), a static-floor fail in phase 3, `reset_account` at ladder step 3 (A3) |
| B1, B2, B3 | trader2 | B1: static-floor fail in phase 1 (`report_fail`), `reset_account` at ladder step 1 (100%) into B2; B2: phase 1 pass, then a fail in phase 2, `reset_account` at ladder step 2 (115%) into B3 (opened IN phase 2) |
| C1 | trader3 | a long, `hold_account`, the held account falls to its static floor, `report_fail` (Failed + held), `reset_account` refused `ReviewPending`, `fail_for_violation` (both admins), `reset_account` refused `WalletBanned`, `unban_wallet`, `reset_account` refused `ViolationNoReset` |
| D1 | trader4 | two passes, the second one confirmed and then vetoed by both admins |
| E1 | trader5 | the forced close-all after a quick-limit overshoot (decision 69): three longs, the trader closes two, the second takes the quick tally over 0.028 S, the third is closed with it as `Close{Fail}`, `Fail{PhaseQuick}`, all in one `report_fail` |
| F1 | trader6 | a breach (tight mode, the price falls back through the tight floor): `Breach`, 12 h lock, the phase is not failed; traded with MOCK proofs (source 255); **after 12 h** it trades again |
| G1, G2, H1 to H4 | trader7, trader5 | junk accounts for the attacks (G) and for the heaviest batches (H); they carry made-up `Meta` hashes and are not verified |

S3 (every one must fail with the exact error of the LiteSVM suite): a batch built for G1 posted to G2 (`BadPrevHash` 6043); the same batch posted twice (`BadSequence` 6007); a post signed by the trader instead of the evaluator (`Unauthorized` 6000); a `Fill` in market 21, created after rule set v1 so its half-spread is 0 (`MarketUnpriced` 6109); `request_advance` with another key in the `sector_authority` slot (Anchor `ConstraintSeeds` 2006); `set_limits` with a 7-day pass delay (`InvalidLimits` 6034); `update_product_config` at the vault with a repeated tier size (`DuplicateChallengeSize` 6044, the vault now refuses it).

**Not covered** (also section 9): the real-key default build (its admin keys are not on this machine), the 12 h lock expiry and the 4th and 5th phases, mainnet RPC behaviour, the evaluator's hot-key signer, a second price recorder, real oracle sources, the vault's heartbeat payout (5-day cycle), bonds, the admin multisig ceremony.

## 2. Keys (all throwaway, outside the repo)

`scripts/devnet-rehearsal.sh --cluster devnet keys` writes the key files into `~/.setl8-devnet-lev` (mode 700, files mode 600) and prints PUBLIC keys only. **The vault's and this program's compiled-in admin keys are throwaway keys too**: the scratch builds replace the two `localnet` admin pubkeys of each program (and the program id) with the public keys below, so the committed private halves of the public test admin keys are never read. Anyone who held these keys could sign admin actions on this deployment: acceptable ONLY on devnet.

| key | public address | SOL it needs | why |
|---|---|---:|---|
| program-vault (program id) | `qmeDsSeK7eFSU8aNoqLFJxZstoMA3EgMrcsuMFjsF2Z` | - | the vault's id |
| program-lev (program id) | `HmkvQfgCMnihGzdZE1FTKVacZN76JS9bfEu2QBJBAdvY` | - | this program's id |
| upgrade-authority | `GxDJcsyKDk7iS3eAhDGigsTWmuBvzY5z8WkNhVPP2a9p` | 11.5908 | rent of both programs, doubled while a deploy buffer exists (535,712 and 862,704 bytes) |
| keeper | `98j1TXdxt77vsDVcLmABWyMP2bWXVoYqRg85nGVGvcth` | 0.75 | fee and rent payer of the admin transactions, the mints, the token accounts, the claims; the fee wallet |
| evaluator | `9kSc8VXbUgRKwxcVaqBPnraZzE5eLT2cGP8bcqy7Kpb7` | 0.25 | fee payer of every journal batch |
| vault-admin-sl8 | `HiB78C1x7TMsHzkAzvxwaVGvkFCPFdxy4WzcV2NmE9i2` | 0.10 | pays the rent of the vault state and the registry |
| vault-admin-rov | `2hdsCzasnWDo96STaZYNWaFUoHWt5qLQGrMTwGqU5HtH` | 0.01 | signs |
| lev-admin-sl8 | `AbJ7iwVVrZHjkVgHmuKgUVdYW3qVRjLZNKnKECYR6Shf` | 0.01 | signs |
| lev-admin-rov | `BFbW5FYZTkUcJdxcxCDW4XM9rz4kgb4VatYEKR1bfitJ` | 0.01 | signs |
| trader1 to trader7 | `7urBRSkX...`, `748HMery...`, `hrCZYN74...`, `9ic6XQam...`, `9GJb6mKG...`, `DtjS6Jq5...`, `3MZeT3Dx...` (full list: `status` or `needs`) | 0.05 each | rent of their accounts and fees |
| **total** | | **13.07 SOL** | `needs` prints the exact numbers from the cluster's own rent figures |

(The local validator's rent is higher than devnet's: its `needs` totals 17.32 SOL, devnet's 13.07.) The faucet gives at most 5 SOL per request and was rate limited on the first request, from this machine, today. If it is rate limited for you too: use https://faucet.solana.com in the browser for the upgrade authority (it needs most), then `scripts/devnet-rehearsal.sh --cluster devnet fund` tops up the small keys.

## 3. Runbook (exact commands, in order)

All commands from the repository root, offline, `RUSTUP_TOOLCHAIN=stable` (the script exports it). The script accepts only `--cluster devnet`, only an RPC URL that contains `devnet` and not `mainnet` (`DEVNET_RPC_URL` overrides `https://api.devnet.solana.com`; the URL is never printed), and only a node whose genesis hash is devnet's (`EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG`). Every `solana` command passes an explicit `--url` and `--keypair`; your own solana configuration and wallet are never used.

```
scripts/devnet-rehearsal.sh --cluster devnet keys      # once; prints public keys
scripts/devnet-rehearsal.sh --cluster devnet build     # ~50 s: vault and program in scratch trees, the driver, the verifier
scripts/devnet-rehearsal.sh --cluster devnet needs     # which key needs how much SOL; exit 3 while any is short
scripts/devnet-rehearsal.sh --cluster devnet fund      # faucet with backoff; or fund the public keys in the browser, then run needs again
scripts/devnet-rehearsal.sh --cluster devnet deploy    # solana program deploy of both programs (skips a deployed one)
scripts/devnet-rehearsal.sh --cluster devnet run       # day 1: S0..S5, about 20 minutes (real time: histories unfold at the cluster's pace)
scripts/devnet-rehearsal.sh --cluster devnet status    # the state (public data only)
# one day later (state.json says the unix second; `run` prints it):
scripts/devnet-rehearsal.sh --cluster devnet run       # day 2: touch_activity, finalize_pass, phase 3, the third reset, S4 and S5 again
# twelve hours after the breach of F1 the same command trades F1 again (scenario S2-lock)
```

* `build` prints: `scratch build without swaps = the freeze note's localnet hash (43fdd935...)` (the proof that the scratch tree and the toolchain give the frozen bytes), the vault `.so` (sha256 `ef6bbe719c466006795c43eb1cf708a9e1fec42c92b4e29e7c66a9bf61a0f15c`, 535,712 bytes), this program's re-keyed `.so` (sha256 `03b0333240a531af718f297d803d512efaf2cb5cd1edf21712e0734e179343ee`, 862,704 bytes: the same size as the frozen build, only the three 32-byte constants differ) and the driver's hash.
* `run` prints one `[PASS]` or `[FAIL]` line per check with the transaction signatures of the refusals, and ends with the results table (`results.md` and `measure.md` in `~/.setl8-devnet-lev/work-devnet/`). `run` is idempotent: it can be interrupted and started again; what is on chain is skipped (a batch whose sequence the chain already holds, a transaction recorded in `state.json`).
* `status` prints the vault and program hashes, every account (trader, challenge id, address, creation second), the results table, the findings and the day-2 items with their unix seconds.
* **Faucet notes.** The public faucet is rate limited per IP and per day; the first request of the day from this machine failed. `fund` asks for at most 5 SOL per request, retries with a growing pause and says what is still missing. If `needs` still exits 3, STOP and fund the printed PUBLIC keys in the browser.
* **The selftest** (no SOL): `scripts/devnet-rehearsal.sh selftest all` starts a local validator, funds, deploys the selftest build and runs everything (about 12 minutes); `selftest start`, `selftest run [--only S4,S5]`, `selftest stop` do the steps one by one. It needs `--cluster devnet build` first.

## 4. Results (selftest: local validator, Agave 4.0.2, vault `4ca958c`)

| scenario | selftest result | devnet |
|---|---|---|
| S0 deploy, vault, product registration | PASS (7 checks, 15 transactions) | PENDING (faucet) |
| S1 config, 21 markets, rule set v1 | PASS (7 checks, 23 transactions) | PENDING |
| S2 trader lifecycle, day 1 | PASS (66 checks, 72 transactions) | PENDING |
| S2 day 2 (finalize, phase 3, third reset) | PASS (12 checks, 4 transactions; delay 60 s on the selftest build) | PENDING |
| S2 lock (F1 after 12 h) | PENDING (cannot run in a selftest) | PENDING |
| S3 attacks | PASS (25 checks, 11 refused transactions) | PENDING |
| S4 measure | **FAIL**: a legal `report_fail` of 8 events and 3 proofs exhausts 200,000 CU (finding D-01); the other checks pass | PENDING |
| S5 verifier on the chain's events | PASS: 8 honest accounts VALID with a chain snapshot (library and the `setl8-verify account` command, exit 0), 4 tamper cases flagged | PENDING |

Signatures of the selftest (local validator; they mean nothing on devnet) are in `state.json`; examples: the refusals `ReviewPending` `4vtWr2fe...`, `WalletBanned` `3LKAbUM8...`, `ViolationNoReset` `FpiuRU9R...`, `PassDelayNotElapsed` `3KsEd5ka...`, `BadPrevHash` `UvxgQAzm...`, `BadSequence` `4QvH452q...`, `Unauthorized` `4tmZk6VE...`, `MarketUnpriced` `4UhLFjWg...`, `ConstraintSeeds` `3ZyCue5f...`, `InvalidLimits` `ocwzMQCJ...`, `DuplicateChallengeSize` `4XCU3d7R...`.

What S2 checked on the real runtime, to the base unit: the purchase (the trader pays price + 0.4% fee, the pool gets floor(65%), the vault's SL8 account the rest, the fee wallet the fee), the reset prices (`AccountReset.price` = the vault's own `reset_price` of the tier cost and the ladder step: $30.00, $34.50, $36.00 for steps 1 to 3, plus the 0.4% fee), the stored `TradingAccount` against the evaluator's state after EVERY batch (b, q_tally, lock, last batch time, seq, journal hash, trades, latch, flat, tight mode, r, g_peak, l_level), the vault record statuses (Active, Failed), the payout claim (0.08 S = $200.00, request id 1), the payout tally (+1, +$200), `PassFinalized` and `PassVetoed` events, and decision 69 on the real runtime (the failing batch holds the crossing close and ONE `Close{Fail}`).

S5 in detail: for each honest account the driver lists the transactions that touch its address (`getSignaturesForAddress`), fetches each (`getTransaction`, version 0), decodes the `emit_cpi!` self-invocations (event tag + discriminator + Borsh) into the verifier's `opened`, `event`, `pass_requested`, `pass_confirmed`, `pass_vetoed`, `phase_advanced`, `pass_finalized`, `account_failed`, `violation_fail`, `account_held` lines, builds the rules file from the chain's `RuleSet` and 21 `Market` accounts, writes the snapshot from the stored `TradingAccount`, adds the evaluator's sample archive and intents file, and runs `verify_account` and the `setl8-verify` command. The number of `event` lines equals `seq` for every account (nothing fails to decode). Tampered copies: a sample dropped (`ArchiveMissing, SamplesHashMismatch, ...`), a price changed (`... WrongLLevel`), an intent submitted twice (`IntentsHashMismatch, IntentDecode, ...`), the last journal event left out (`SequenceGap, StateMismatch`): all MISMATCH. The files are kept in `work-*/verify/<account>/chain/` for anyone to replay with the command line.

## 5. Compute units and transaction size (S4): LiteSVM against the cluster

LiteSVM numbers are those of today's `budget.rs` run (the table of module-11.md predates F-27 for the admin instructions; its `create_market` row says 16,815, the budget test says 31,705). Cluster numbers: the largest transaction of each instruction in the selftest (not the worst cases of the budget test: they are different shapes, so "below" is expected for the trader instructions).

| instruction | n | cluster CU | LiteSVM worst CU | ceiling CU | cluster bytes | ceiling bytes | verdict |
|---|---:|---:|---:|---:|---:|---:|---|
| confirm_pass | 5 | 35,652 | 23,971 | 27,600 | 278 | 440 | OVER CEILING |
| create_market | 22 | 45,102 | 31,705 | 36,500 | 594 | 690 | OVER CEILING |
| create_ruleset | 1 | 55,297 | 46,405 | 53,400 | 1,018 | 1,180 | OVER CEILING |
| hold_account | 1 | 36,578 | 24,633 | 28,400 | 343 | 510 | OVER CEILING |
| unban_wallet | 1 | 32,537 | 20,591 | 23,700 | 503 | 580 | OVER CEILING |
| veto_pass | 1 | 34,589 | 22,643 | 26,100 | 471 | 550 | OVER CEILING |
| post_journal | 46 | 186,144 | 125,290 | 144,100 | 1,141 | 1,232 | OVER CEILING (the heaviest legal shape: 8 events, 3 proofs) |
| report_fail | 8 | 196,812 | 151,284 | 174,000 | 1,148 | 1,232 | OVER CEILING (7 events; **8 events fail**) |
| request_advance | 5 | 51,565 | 45,937 | 52,900 | 443 | 620 | +12%, within the ceiling |
| fail_for_violation | 1 | 39,902 | 44,045 | 50,700 | 668 | 770 | -9% |
| finalize_pass | 1 | 85,616 | 84,594 | 97,300 | 575 | 780 | +1% |
| init_config | 1 | 14,442 | 16,050 | 18,500 | 583 | 680 | -10% |
| open_account | 12 | 92,486 | 135,595 | 156,000 | 742 | 970 | -32% (not the worst key) |
| reset_account | 3 | 128,390 | 158,203 | 182,000 | 815 | 1,050 | -19% (not the worst key) |
| touch_activity | 2 | 24,992 | 52,145 | 60,000 | 443 | 620 | -52% (throttled in the run, the vault call in the worst case) |
| init_vault, register_product, create_ata, create_mint, mint_to | | 27,176 / 11,125 / 58,471 / 364 / 120 | - | - | | | not in the budget test |

Not exercised by the run (no scenario needs them): `ban_wallet`, `cancel_pass`, `close_account`, `reject_pass`, `release_account`, `release_hold`, `set_evaluator`, `set_limits` (only refused), `set_paused`, `update_market`.

**D-01 (high): the event-heavy batches are much closer to the 200,000-unit limit than the model says.** Every event the program emits through `emit_cpi!` is a self-invocation of the program; on this runtime it costs 12,539 CU INSIDE the inner call (the same number every time, for every instruction) plus the invoke itself, about 17,300 CU per additional event in total, against 10,000 to 12,000 in LiteSVM. Measured on the cluster, a `report_fail` with 3 proofs: 5 events 162,284 CU, 6 events 179,548, 7 events 196,812 (3,188 below the limit), **8 events fail with `ComputationalBudgetExceeded`** (signature in `state.json`, label `report_fail ev=8 pr=3 heavy H1`; LiteSVM: 151,284). A `post_journal` of 8 events and 3 proofs uses 186,144 CU (LiteSVM 125,290; 7% below the limit). The evaluator's `cu_estimate` (30,000 + 12,500 per event + 8,000 per proof + 40,000 for a fail) says 194,000 for the 8-event `report_fail` and lets the batch go. A batch the model accepts and the chain refuses cannot be posted unless the transaction asks for more compute: the account is stuck. Where it bites: a failing batch with commission fees (classes 2 to 5 have a commission in the launch rule set): three positions closed, three fees, the `Fail`, the `Meta` = 8 events; a forced close-all (decision 69) with three closes is 5 events and fits (162,284). **Fixes outside the program** (this prompt does not touch it): the evaluator adds `SetComputeUnitLimit(400_000)` (40 bytes: the heaviest transaction becomes 1,221 of 1,232) when its model says more than about 120,000, or limits batches to 6 events; and `cu_estimate` and the `budget.rs` ceilings are re-fitted on the cluster's runtime. A fix in the program (fewer or cheaper events) is a program change and is not planned.

**D-02 (medium): the ceilings of `budget.rs`.** Eight instructions exceed their LiteSVM-calibrated ceilings on the cluster: the six cheap ones above by 5,000 to 14,000 CU (each emits one event), and the two batch instructions in their heaviest shapes. The ceilings are LiteSVM's, not the runtime's: re-run `budget.rs` against the runtime that will host the program (the test validator is the same software as devnet's nodes).

**Program-id lottery (information).** The cost of the bump searches depends on the program id: this program's `__event_authority` has bump 248 here against 252 under the repo's placeholder id (4 more attempts, about 6,000 CU), `sector_authority` 255 against 253. The `budget.rs` "worst key" covers the trader's key, not the program's id: measure with the real id.

## 6. Verifier on the chain's events (S5): the decode gap

The chain's events DO decode into the verifier's format (every `emit_cpi!` event of the program is an inner instruction of its transaction; none of the events of S2 was lost; the log built from the chain alone gives VALID for all eight honest accounts, including a Failed + held account (C1), a violation, a veto, the forced close-all (E1), resets (B2, B3, A3) and a phase 3 account (A1)). Nothing in the verifier's formats needed to change.

**D-03 (low): the verifier does not know the mock proof source 255.** The `localnet` build accepts source-255 proofs; the verifier's archive and proof checks do not, so an account traded with mock proofs is never VALID (`ProofShape`). The first run of the driver used mock proofs everywhere (as the prompt said) and S5 failed for that reason alone. The rehearsal therefore trades the accounts it verifies with each market's real source format (source 2, unsigned GMX tickers: the program checks only their shape, on the `localnet` build exactly as on a production build) and keeps source 255 for F1 and the junk accounts. F1's verdict is recorded: `ProofShape`.

**RPC history (information).** S5 needs an RPC that keeps the whole history of the account (`getSignaturesForAddress`, `getTransaction`). The first local run lost the early transactions because the validator keeps only the last 10,000 shreds by default; the script now starts it with a large `--limit-ledger-size`. The public devnet RPC keeps history; a pruned RPC (or a provider that limits `getSignaturesForAddress`) gives a wrong "missing opened line".

## 7. Discrepancies between the LiteSVM suite and the cluster

1. Compute units (D-01, D-02).
2. Time: LiteSVM sets the clock; on a cluster an event cannot be dated before the account exists or more than 2 s after the block time (`TimeInFuture`), so a history takes the time it covers. The driver posts a batch only when the measured block time of the newest confirmed slot has reached its `Meta`; a cached offset of the wall clock let two batches through early in a test.
3. Account edits: the LiteSVM suite edits accounts to reach states (`edit_account`); on a cluster every state is reached through instructions. Reaching phase 3 needs a real 1-day wait.
4. After a violation `reset_account` answers `WalletBanned` first (the ban check precedes the marker check); `ViolationNoReset` only after `unban_wallet`. The LiteSVM suite tests the second after un-banning; the checklist and the runbook now say so.
5. The verifier (D-03).
6. Nothing else differed: every refusal of S3 and S2 carried the exact error code of the suite, the stored account equalled the evaluator's state after every batch, and the vault split, the reset prices and the payout claim were exact to the base unit.

## 8. Findings

| id | severity | what | evidence |
|---|---|---|---|
| D-01 | high | a legal `report_fail` of 8 events and 3 proofs needs more than 200,000 CU on the runtime (7 events: 196,812); `cu_estimate` and `budget.rs` are LiteSVM's | `state.json` label `report_fail ev=8 pr=3 heavy H1` (selftest signature `5saGid1r...`), `post_journal ev=8 pr=3 heavy G2` 186,144 CU |
| D-02 | medium | eight instructions above their `budget.rs` ceilings on the runtime | section 5 |
| D-03 | low | the verifier flags source-255 proofs (`ProofShape`) | `work-*/verify/F1/chain/` |
| D-04 | info | the decision-69 group (crossing close, forced closes, `Fail`, `Meta`) with three closes is 5 events and costs 162,284 CU: it fits; with fees it can reach 8 events | section 5 |

None is a program bug that stops a scenario: D-01 is a cost the program is not designed around; the evaluator or the program must make room for it before mainnet.

## 9. What the rehearsal does not cover

* **The real-key default build.** Its admin keys are not on this machine; the deployed program here is the `localnet` build with throwaway program id and admin keys (the scratch build WITHOUT the swaps equals the freeze note's hash 43fdd935..., the swapped build has the same size). The default build's behaviour is the same code with other constants, but its deployment, its upgrade authority handling and the 2-of-2 ceremony are not rehearsed.
* **Mainnet RPC behaviour:** rate limits, commitment levels, priority fees, transaction landing under load, dropped transactions (the driver re-sends after 20 s and retries `TimeInFuture`, nothing more).
* **The evaluator's hot-key signer:** here the evaluator is a key file.
* **A second price recorder** and **real oracle sources:** the prices are the mock oracle's; the proofs are unsigned source-2 proofs with the markets' real feed ids.
* **The vault's heartbeat payout** (5-day cycle): the claim is queued and checked, never paid out.
* **Day 5 and later:** phases 4 and 5, graduation, abandonment (7 idle days), the 7-day account retention (`close_account`).
* **The 12 h lock expiry:** scenario S2-lock runs 12 h after the breach on the real run.
* **Real spreads and commissions:** placeholders.

## 10. Files

`tools/devnet-rehearsal` (the driver; own `Cargo.lock`, not a workspace member), `scripts/devnet-rehearsal.sh`, this file. The state of a run: `~/.setl8-devnet-lev/work-devnet/state.json` (public data only), `results.md`, `measure.md`, `verify/<account>/chain/{events.jsonl,rules.json,archive.bin,intents.jsonl,snapshot.json}`; the evaluator's files per account: `<account>.archive.bin`, `<account>.intents.jsonl`. Gates of the tool: `cargo clippy --offline --locked --all-targets -- -D warnings` and `cargo test --offline --locked` in `tools/devnet-rehearsal` (39 tests: URL and genesis guards, the RPC record parser, the events decoder against the verifier's parser, the launch file, the stories (a pass, a static fail, a hold and fail, a breach, the forced close-all) against the real evaluator, the vault decoders, the measure tables against `budget.rs`, the verifier on a locally produced account and its tampered copies, the state file, and a grep-as-a-test that no output statement names a key file, a seed, a secret or the RPC URL).
