# Golden vectors for non-Rust backends

A production backend for setl8-lev-trading may be written in another language. It must behave exactly like the reference
evaluator (`crates/evaluator`) and the `engine` and `journal-types` crates under it. These files are the contract.

**The rule: reproduce every file bit for bit.** For each file below, feed the `in` of every case to your implementation and
compare with `out`: the integers, the booleans, the errors and (for the scenarios) every byte of every batch, of the sample
archive and of the intents file. One differing byte, one differing micro-USD, or a different error name is a bug in the new backend.

Nothing here is approximate: money is micro-USD (6 decimals), prices are 1e-12 USD, quantities are 1e-6 base units, and all
intermediates are exact integers (the Rust code uses `i128`/`u128` with checked operations; where an operation would
overflow, the case's `out` is `{"err":"Overflow"}`).

## Format

* JSON, UTF-8, keys sorted alphabetically. **Every integer is a decimal string** (`"1080032400000"`, `"-2064740000"`);
  booleans are JSON booleans; byte strings (hashes, batches, archives, signatures) are lower-case hex.
* Each function file has `function`, `description`, `version` ("1") and `cases`: a list of `{"name", "in", "out"}` (and `kind`
  where one file holds several functions). `out` is `{"ok": ...}` or `{"err": "<EngineError name>"}` for fallible functions
  (`Overflow`, `InvalidQuantity`, `InvalidStopLevel`, `InvalidInput`), or the value itself for total functions.
* Side is `"buy"` or `"sell"`; sample actions are `{"kind": "none" | "fail" | "breach" | "tight_start" | "floor_raised", ...}`.

## Files (2,645 cases)

| file | cases | what |
|---|---|---|
| `apply_fill.json` | 581 | netting of one fill: open, merge (VWAP, long rounds up, short down), partial reduce, flat, flip, quick slices at the 45 s boundary, errors |
| `pnl_and_credit.json` | 190 | `pnl_micro` (floor toward minus infinity) and `slice_credit` (quick half-credit, quick tally) |
| `margin.json` | 196 | `margin_micro` (one ceiling over the whole expression) |
| `fees.json` | 179 | `fee_ceil_micro`, `holding_fee_micro`, `swap_fee_micro`, `day_index` (rollover 22:00 UTC), `nights_due`, `validate_quantity` |
| `prices.json` | 260 | `ask_from_mid`, `bid_from_mid`, `price_from_oracle`, `fill_price_e12`, `close_price_e12` |
| `triggers.json` | 220 | `check_protective` (SL beats TP) and `check_pending` (limit/stop, closed market, lock) |
| `evaluate_sample.json` | 600 | every decision of one sample: none, static-floor fail, breach, tight start, floor raised, with the state after |
| `claims.json` | 54 | the program's checks of an evaluator's claims (`check_close`, `check_tight_start`, `check_floor_raised`, `check_breach`) |
| `sha256.json` | 35 | SHA-256 |
| `hash_chain.json` | 179 | `genesis_hash`, `feed_id`, `next_hash` (one chain step per event with and without a proof), `apply_batch` |
| `batch_encoding.json` | 140 | canonical Borsh bytes of `JournalBatch` (all nine event types) and `PriceProof`, with the proof digest |
| `scenarios/*.json` | 11 | complete small histories, below |

## Scenarios

Each scenario file holds the `rules` (leverage, holding and commission rates, markets with sessions and half-spreads), the
`account`, the ordered `steps` and the expected outputs:

* `steps`: `sample` (market, source timestamp ns, 18-decimal value, receive time ns, blob digest), `order` (trader nonce, the
  48 order bytes, the ed25519 signature of `SL8LEV|account|nonce|order` by `account.trader`), `tick` (backend second), and
  `drain` (the batching policy `max_events`, `heartbeat`: the point where batches are cut). Replay them in order.
* `batches`: for every batch the evaluator seals, in order: `kind`, the exact `JournalBatch` bytes (`batch_bytes`), the
  market accounts the transaction names, `meta_time`, the committed archive records and intents bytes, the transaction size
  (`tx_bytes`, with the `SetComputeUnitLimit` instruction; `tx_format` says `legacy` or `v0_lookup`, a version-0 transaction with an address
  lookup table, chosen only for a batch that cannot be split and does not fit a legacy transaction), the compute units MEASURED on a real
  runtime for the batch's shape (`cu`) and the limit the transaction must request (`cu_limit`, P27 / decision 71), and the account as it must read afterwards (`account_after`: all fields the journal decides).
* `archive`: the full sample archive file (16-byte segment headers and 68-byte records) and `intents`: the intents file text.

The scenarios include DESIGN 4.9, DESIGN 2.6 R1, a quick round trip with commission, a weekend of holding fees, a quick flip,
a static-floor fail, a quick-limit fail, a limit fill followed by its stop-loss, a two-market night, a reached pass target, and a
breach lock with a pending order that survives it.

## Regenerating and checking

* `cargo test -p setl8-lev-evaluator --test golden` regenerates every file from the code and compares it byte for byte with
  the committed one (and checks that no file is a bare JSON number anywhere). `GOLDEN_WRITE=1` rewrites the files.
* `python3 check.py` re-implements `apply_fill`, `pnl_micro`, `slice_credit`, `margin_micro`, the fees, the prices, the triggers
  and `evaluate_sample` in plain Python integers, and reads the cases back: it must report 0 mismatches. Use it as the model for a port.
* A change to a rule, a rounding direction or a byte layout changes these files; that is the point. Review the diff, bump
  `ENGINE_VERSION`, and update every backend.
