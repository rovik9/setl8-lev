# P24a state (resume file): the owner's fixed trading costs

Task: handover/prompts/P24a-owner-fixed-costs.md (decision 72). Offline: `export RUSTUP_TOOLCHAIN=stable`. THE PROGRAM IS FROZEN (localnet sha256 43fdd935..., default ebbd0786...): no change to programs/, crates/engine, crates/journal-types, Cargo.lock, Cargo.toml, rust-toolchain.toml, Anchor.toml (`git diff 64629c1 HEAD -- ...` must stay empty).

## DONE
* step 1 (dada536): `config/launch-markets.toml`: commission 300 e6 on all 8 classes (`pending = false`), `half_spread_e6 = 50` on all 21 markets, `spread_pending = true` + `spread_note` "owner base 50 e6 set; oracle-noise floor check pending (P24b)"; `launch.json`, `launch.md` regenerated; the tool got an optional `spread_note`, the owner-cost paragraph, the computed cost table and a production failure that names spread and commission flags. `verify` OK (21 markets, 21 PENDING flags); `verify --production` fails with ONE violation (21 market spreads, 0 class commissions). Rule set stays version 1.
* step 2 (527e92b): the launch-config tests and docs that pinned the placeholders (29 flags, 150/200/... spreads, 20/100 commissions) updated without weakening, plus new tests (owner costs, pip reading, news-spread line, cost table is computed).
* step 3 (38c938a, d89296a): the event-count check. `testkit`: `SETL8_LAUNCH_COSTS` (300 e6 / 50 e6 on every path), `close_all_story`; `tests/launch_costs.rs`; engine-only and differential print the batches at the event maximum. Result: a Fill is 2 events, a Close 2 (a flip 4; the quick-limit crossing slice and forced closes carry no fee); static-floor failure of N = 1 to 8 positions postable with commission; quick-limit failure postable for N = 2 to 4, not from N = 5 on, WITH AND WITHOUT commission (decision 69's three-proof limit, pre-existing): no new unpostable group. 10,000-path campaigns with the launch costs: unpostable 0, largest limit 270,000, largest tx 1,229 bytes, 218 of 79,207 batches at 8 events.
* step 4: the cost table is in launch.md (generated) and in the report.
* step 5 (751b451): decision 72, deploy-checklist section "Owner's fixed costs", launch-config.md section with the event-count tables.

## NEXT
* P24b: the oracle-noise floor per market from the observer data; raises the base where needed and clears the spread flags; THEN `launch-config verify --production` passes and `create_ruleset` v1 can run.
* News spread: not implemented, decision pending (needs a design decision and, with the frozen program, a different mechanism).
* Confirm the reading of "1 pip" (FULL spread of 1 bp, half 50 e6).
