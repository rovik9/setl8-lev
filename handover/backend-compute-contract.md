# Backend compute contract (P27, decision 71)

Written for: the engineers who build and send the evaluator's transactions (the backend repo).

The default compute limit of a transaction is 200,000 units. On a real runtime that is **not enough** for `post_journal` and `report_fail` at 7 and 8 events: a `report_fail` of 8 events and 3 proofs needs 224,440 units on Agave 4.0.2, a `post_journal` of 8 events and 3 proofs 188,799. LiteSVM (what `tests-rs` runs) shows 125,290 and 151,284 for the same batches and must not be used for absolute numbers (`handover/cu-runtime.md`, "Why LiteSVM differs"). So every transaction below carries an explicit `SetComputeUnitLimit`, and its value comes from this contract.

## The rule

1. **Every `post_journal` and every `report_fail` transaction carries `SetComputeUnitLimit(L)` with `L` = `recommended_cu_limit(kind, events, proofs)`** (the evaluator puts it in `Batch.cu_limit`; use that field). `events` counts every event of the batch including the `Meta`; `proofs` counts the distinct proofs of the batch.
2. `L` = the worst compute MEASURED on the validator for any batch of that kind with at most that many events and proofs, times 1.20, rounded UP to the next 1,000, never above 1,400,000. Monotone: more events or proofs never ask for less.
3. Every other instruction whose measured worst case is above 70 % of 200,000 (140,000) also carries an explicit limit: `reset_account` (154,289 measured). The five instructions whose cost depends on the trader's key (`open_account`, `reset_account`, `finalize_pass`, `touch_activity`, `release_account`: the bump searches of the program's and the vault's addresses cost about 1,500 per failed attempt) carry **400,000**.
4. Never request more than 1,400,000 (the transaction maximum). The largest limit any batch of the format asks for is 270,000.
5. The transaction never exceeds 1,232 bytes **with** the limit instruction (and with the price instruction when a priority fee is set). The evaluator guarantees it for the batches it emits (`Batch.tx_bytes`, `Batch.tx_format`).

## The function and the table

```rust
// crates/evaluator/src/compute.rs (pure, no network)
pub fn recommended_cu_limit(kind: BatchKind, events: usize, proofs: usize) -> u64;   // ceil(measured x 1.20 to 1,000), at most 1_400_000
pub fn measured_cu(kind: BatchKind, events: usize, proofs: usize) -> u64;             // the table below, CU measured on Agave 4.0.2
pub fn limit_for(measured_max: u64) -> u64;                                           // the formula alone
pub fn tx_size_with_budget(kind, n_markets, batch_len, format: TxFormat, priority_fee: bool) -> usize;
```

A backend in another language implements the formula and uses the table (`handover/cu-runtime.json` is the source; the evaluator's test recomputes the table from it and fails if they differ). Rows: events 0 to 8, columns: proofs 0 to 3. The requested limits (CU):

| events | post_journal, 0 proofs | 1 | 2 | 3 | report_fail, 0 proofs | 1 | 2 | 3 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 52,000 | 52,000 | 52,000 | 52,000 | 113,000 | 113,000 | 113,000 | 113,000 |
| 2 | 75,000 | 78,000 | 78,000 | 78,000 | 113,000 | 113,000 | 113,000 | 113,000 |
| 3 | 94,000 | 103,000 | 106,000 | 106,000 | 138,000 | 138,000 | 138,000 | 138,000 |
| 4 | 114,000 | 126,000 | 129,000 | 132,000 | 163,000 | 163,000 | 166,000 | 166,000 |
| 5 | 134,000 | 150,000 | 153,000 | 156,000 | 189,000 | 189,000 | 192,000 | 195,000 |
| 6 | 154,000 | 174,000 | 177,000 | 180,000 | 214,000 | 214,000 | 217,000 | 220,000 |
| 7 | 174,000 | 198,000 | 201,000 | 204,000 | 239,000 | 239,000 | 242,000 | 245,000 |
| 8 | 194,000 | 221,000 | 224,000 | 227,000 | 264,000 | 264,000 | 267,000 | 270,000 |

The measured units behind it (events 8, proofs 3): `post_journal` 188,799; `report_fail` 224,440 (a failing batch with six forced closes). The 8-event commission case (three closes, three fees, `Fail`, `Meta`, three proofs) measured 213,661; with the table's monotone closure it asks for 270,000.

Instructions outside the batch format (limit = measured worst case, rounded as above, or the fixed value):

| instruction | measured worst (CU) | limit to request |
|---|---:|---|
| `reset_account` | 154,289 | 400,000 (key-dependent) |
| `open_account` | 103,181 | 400,000 (key-dependent) |
| `finalize_pass` | 87,115 | 400,000 (key-dependent) |
| `touch_activity`, `release_account` | 38,590 and 38,596 | 400,000 (key-dependent) |
| all other instructions (admin, `confirm_pass`, `request_advance`, `veto_pass`, `hold_account`, `release_hold`, `fail_for_violation`, `close_account`, ...) | at most 58,447 (`create_ruleset`) | the default 200,000 is enough (a limit is allowed and harmless) |

## Transaction layout and byte counts

Fee payer and only signer: the evaluator key. The instructions, in this order: `SetComputeUnitLimit` (and `SetComputeUnitPrice` if a priority fee is set), then `post_journal` or `report_fail`.

* `SetComputeUnitLimit(L)` adds **40 bytes**: its program key `ComputeBudget111111111111111111111111111111` (32, once in the transaction) + program index (1) + accounts length (1) + data length (1) + data (1 byte tag 2 + 4 bytes `u32`).
* `SetComputeUnitPrice(micro-lamports)` adds **12 bytes** once the program key is there: 1 + 1 + 1 + 9 (tag 3 + `u64`). Limit only: +40. Limit and price: +52.
* Legacy `report_fail` over `n` markets: `1 + 64` (signature count, signature) `+ 3` (header) `+ 1 + 32 x (11 + n + 1)` (keys: evaluator, trading account, config, rule set, wallet, sector authority, vault program, product registry, vault trader state, event authority, program, the markets, the compute-budget program) `+ 32` (blockhash) `+ 1` (instruction count) `+ 8` (limit instruction) `+ [1 + 1 + (11 + n) + 2 + (8 + 4 + batch + 1)]` (the program instruction: program index, account count, account indexes, data length, data). `post_journal` has 6 fixed accounts instead of 11 and no reason byte.
* Example (the group that found the problem): three forced closes (67 bytes each), `Fail` (19), `Meta` (76), three source-3 proofs (93 each) = a 623-byte batch, three markets. Legacy: `65 + 3 + (1 + 32 x 15) + 32 + 1 + 8 + (1 + 1 + 14 + 2 + 636)` = **1,244 bytes** with the limit instruction (1,204 without it; 1,256 with the price instruction): it does not fit 1,232.

### The version-0 transaction with an address lookup table

A group that cannot be split (the crossing close, the forced closes, the `Fail` and the `Meta` of a quick-limit failure are ONE batch: decision 69) and does not fit a legacy transaction is emitted with `Batch.tx_format == V0Lookup`. The backend then sends a **version-0** transaction that loads the shared accounts from an address lookup table (ALT):

* the ALT holds, for the program: the `config` PDA, every `ruleset` PDA, the `event_authority` PDA, the `sector_authority` PDA, the vault program id, the vault `product_registry` PDA and every `market` PDA (read-only, none is a signer). Create it once (`create_lookup_table`, then `extend_lookup_table` with these addresses; wait one slot), and extend it in the same operation as every `create_market` and `create_ruleset`. Never deactivate or close it.
* static keys that stay in the message: the fee payer, the trading account, the wallet and the vault trader state of the trader (they differ per trader), the program id, the compute-budget program (a program id of an instruction must be static). For `post_journal`: the evaluator, the trading account, the program and the compute-budget program. The accounts the table supplies are `config`, `ruleset`, `event_authority` and the markets (3 + n) for `post_journal`, and also `sector_authority`, `vault_program` and `product_registry` (6 + n) for `report_fail`.
* size = legacy size `- (1 + 32 x keys) + (1 + 32 x static keys) + 1` (version byte) `+ 1` (one lookup) `+ 32 + 1 + 1 + loaded` (table key, empty writable list, read-only list with one byte per account loaded). The example: **1,001 bytes** (`1,244 - 481 + 193 + 45`), with the price instruction 1,013. `tests-rs/tests/budget_runtime.rs` checks both formulas against `solana-message`'s own serializer to the byte for every shape, and the version-0 form was executed by the real program in LiteSVM and by the validator of the measurement (102 shapes; four of them need it: `report_fail` of 8 events and 3 proofs with six closes is 1,106 bytes).
* The shapes that need it are rare (a quick-limit failure with three or more positions closed in three source-3 markets, or a static-floor failure with five or six closes): the campaigns print how many batches of a run used it.

## What the evaluator guarantees, what you must do

The evaluator never emits a batch whose transaction exceeds 1,232 bytes (legacy, else lookup-table) or whose measured compute exceeds the limit it asks for; a unit that fits neither way is counted in `Stats::unpostable` (the campaigns assert 0). You must: send `Batch.cu_limit` as the limit, send the format `Batch.tx_format` names, keep the ALT current, and **re-measure**:

* the table is from a local validator (Agave 4.0.2, 273 features). Devnet and mainnet-beta may run another version: run `scripts/devnet-rehearsal.sh selftest measure` against the version you will deploy to (and the devnet rehearsal against devnet, then again against mainnet-beta before launch) and commit the new `handover/cu-runtime.json`. The evaluator's `compute::tests` and `tests-rs/tests/budget_runtime.rs` then fail until `compute_tables.rs` (`cargo test -p setl8-lev-evaluator print_tables -- --ignored --nocapture`) and the runtime ceilings are updated on purpose.
* proofs of the real sources (2, 3, 4) were measured only as mock proofs (source 255) on the localnet build; LiteSVM shows up to 8 % between sources for one shape, inside the 20 % margin.
