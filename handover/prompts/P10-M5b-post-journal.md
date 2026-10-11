You are building module M5b (`post_journal`, invariants I-1 to I-12) of setl8-lev-trading in THIS repo (branch main). M5b REQUIRES M5a committed (check `git log` for "M5a" and that `TradingAccount`, `open_account`, tests-rs/tests/common/vault.rs exist) and M2 and M4. If not, STOP and say so.

Read first, in this order: docs/DESIGN.md sections 1.3, 1.6, 2 (all), 3.3 to 3.5 (note: 3.3 text about Stork is SUPERSEDED by decisions 43 to 50 in handover/design-decisions.md; 8.2 and decision 46 are current), 4.1 to 4.9 (4.4 is the invariant table, 4.9 the worked example), 5.1, 6.3 (`post_journal` row), 7.1, 7.4, 8.2, 8.3, 9.1, 10.1, 11, 12 (error list); docs/BUILD_PLAN.md section 0, section 1 and the M5 row; handover/design-decisions.md decisions 22, 31, 32 to 36, 38, 46 to 50; handover/module-2.md, module-3.md, module-4.md, module-5a.md; the code: crates/journal-types (all), crates/engine (all pub fns), programs/setl8-lev-trading/src (state/trading_account.rs, instructions/open_account.rs, state/market.rs, state/rule_set.rs, errors.rs, events.rs), tests-rs/tests/common/*.

RULES
- Work in programs/setl8-lev-trading, tests-rs, scripts, handover. You MAY add small PURE helper functions to crates/engine (new file `claims.rs`: checking helpers) and fix a real bug in crates/journal-types, only if needed, each with tests, and you must rerun `cargo mutants` on the new/changed files and report the numbers (>= 95% of viable killed, survivors explained). Do NOT touch ../setl8-vault or ../setl8-shared-interfaces. NEVER read, print or commit private keys, .env files or keypair JSONs.
- Do not push, tag or change git config. Commit only with explicit paths after scripts/secret-scan-staged.sh. Author env vars GIT_AUTHOR_NAME=Rovik GIT_AUTHOR_EMAIL=therovikdev@gmail.com GIT_COMMITTER_NAME=Rovik GIT_COMMITTER_EMAIL=therovikdev@gmail.com. Commit message ends with: Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
- Program code: no unwrap/expect/panic/indexing, checked arithmetic, no floats, no truncating `as` casts (the lint gate from M5a stays on). The DESIGN is the spec; if wrong or contradictory STOP and report; where silent take the smallest safe (strictest) option and list it under DECISIONS.
- Stack: keep every frame under 4096 bytes (scripts/build-test-so.sh fails on overflow). A batch has at most 8 events and 3 proofs; do not clone the whole batch more than needed.

RESOLVE FIRST (before coding; if the answer needs a design change, STOP and report instead of improvising)
(a) Price check vs spread. DESIGN 4.4 I-4 says the proof value must equal the event price, but DESIGN 7.1/9.1 say fills execute at `ask = ceil(mid x (1 + h))` and `bid = floor(mid x (1 - h))` with `h = rule_set.spread_e6[market_id]` (e6 units of the mid), while the PriceProof carries the MID. `journal_types::check_proof` takes one `event_price_e12` and compares it for equality. Decide and implement the exact rule: for every event that references a proof and has a price, the program derives the expected executed price from the proof mid (`price_e12_from_quantized`), the side and `h`: Fill long = ask, Fill short = bid; Close of a long = bid, Close of a short = ask; Breach and Fail closes follow the Close rule; `TightStart.p_open` and the floor events have no price (they carry a proof only for time and feed checks). State the formula and the rounding you used in module-5b.md and reproduce DESIGN 4.9's fill (ask 1.0800324) from it. If M2's `check_proof` needs a changed signature (for example pass the already-derived expected price), change it with tests.
(b) What counts as exposure-increasing. The program stores no positions. DESIGN 8.2 has `Fill` and `Close` events. Check 4.4, 7.2 and 8.1/8.5: if every `Fill` opens or adds and reductions are `Close` events, then EVERY `Fill` is exposure-increasing (needs `held == 0` and `ev.time >= lock_until`). If the design says otherwise, follow it; if silent, treat every Fill as exposure-increasing and say so under DECISIONS.
(c) `config.paused`: check 6.3 and 10.8. If silent, `post_journal` still works when the sector is paused except it rejects exposure-increasing Fills; list under DECISIONS.

SCOPE
1. `post_journal(batch_bytes: Vec<u8>)` (the argument is the Borsh bytes of `JournalBatch`, decoded with `JournalBatch::decode`; the journal-types crate has no borsh dependency). Accounts: evaluator [S, W], `trading_account` [W], config [R], event_cpi accounts, and the referenced `Market` accounts as `remaining_accounts` (PDA `["market", market_id u16]`, owner = this program, verified). Whole batch is atomic.
2. Apply the invariants of DESIGN 4.4 in order, event by event, using the engine's functions and constants (never re-implement a threshold):
   - I-1 signer == `config.evaluator`; status Active; `pass_pending != 2`; Fill needs `held == 0` and not locked (I-10).
   - I-2 `first_seq == seq + 1` AND `prev_hash == journal_hash` (journal-types' `apply_batch` does NOT compare prev_hash; the program must, M2 hook), hash chain extended per event with the `sol_sha256` hasher, `seq` and `journal_hash` stored at the end. Events are emitted with `emit_cpi!` as `EventLog { account, seq, event_bytes, proof_bytes }` (raw canonical bytes).
   - I-3 time monotone (`ev.time >= last_batch_time`, within the batch too) and `ev.time <= now + 2 s`; `last_batch_time` updated.
   - I-4 markets exist; proof feed id equals `market.feed_id`; `check_proof` with `allow_mock = cfg!(feature = "localnet")` (source 255 rejected otherwise); sources 2 and 4 must have an EMPTY `signed_update`; source 3 `signed_update` must be exactly 32 bytes (decision 47: the sha256 digest of the signed blob, which lives in the archive); market status must allow the event (status 2 reduce-only: Fill rejected, Close allowed; check DESIGN 3.2/3.4 for the rest).
   - I-5 Close: `pnl` recomputed with `engine::pnl_micro`, `quick == (ev.time - timer_start <= 45)`, `credited` per 7.4 (quick profit credited at floor(pnl/2), losses full), `b += credited`, quick profit adds `floor(pnl/2)` to `q_tally`, `trades_closed += 1`. Use `engine::apply_closed_slice` semantics.
   - I-6 Fee amount >= 0 (u64) and `b -= amount` (checked).
   - I-7 latch at `b >= 1.04 S`, never clears.
   - I-8 `q_tally > 0.028 S` after any event: reject the whole batch (the Fail path is M7 `report_fail`).
   - I-9 TightStart / FloorRaised / TightEnd claims checked against the formulas with the engine; I-10 Breach claims and the 12 h lock; Fill after lock rejected.
   - I-11 a `Fail` event in `post_journal` is rejected (it only exists for `report_fail`, M7) with a clear error.
   - I-12 `Meta` must be the last event of every batch (exactly one Meta, at the end); `flat` stored as given.
3. Errors: append new variants at the END of the enum only (codes 6043 and up), one per rejection reason so tests can assert each precisely. Do not reuse a vague error for two different invariants.
4. Events: `EventLog` plus a `BatchPosted { account, first_seq, last_seq, journal_hash }` if useful (decide and list).
5. tests-rs: extend the harness with `post(evaluator, account, batch)` and a batch BUILDER that computes correct chain hashes with the real `journal_types` crate (not by copying program code), so tests can corrupt exactly one thing at a time. A state-digest helper (account bytes + all token balances) to assert "unchanged on every failure".

TESTS (each named; every failure test asserts the exact error code AND an unchanged digest)
- DESIGN 4.9 worked example posted as THREE transactions (fill; tight start + raise; breach + close), asserting the exact account state after each (b, q_tally, tight_on, r, g_peak, l_level, lock_until, flat, seq, journal_hash, trades_closed) with the numbers of 4.9 (g_peak 3,035.107, l_level 98,035.107, breach pnl -2,064.74, B 97,935.26) and the 12 h lock; and the independent chain head recomputed by `journal_types::verify_chain` from the emitted EventLogs read from the transaction's inner instructions.
- I-1: wrong signer, non-active status, `pass_pending == 2` (and == 1 allowed), held Fill rejected while a Close is allowed.
- I-2: wrong `first_seq` (low, high, zero), wrong `prev_hash` (one bit), replay of the same batch, two batches chain like one (post one batch of 4 vs two of 2: identical final hash), event reordering, dropped event.
- I-3: time backwards by 1 s, equal OK, `now + 2 s` OK and `now + 3 s` rejected; within-batch ordering.
- I-4: unknown market, wrong feed id, proof value off by 1 unit of e12 (and the spread derivation from RESOLVE (a)), proof older than `max_age_secs` by 1 s (exact boundary ok), proof from the future by 3 s, source 255 rejected in a non-localnet build (document how you test it: a unit test on the helper if the .so is localnet), sources 2 and 4 with a non-empty `signed_update`, source 3 with 31 and 33 bytes, proof index out of range, reduce-only market.
- I-5 to I-11 boundary suite, each violated by exactly ONE micro-USD or one second and compared with the exact-boundary pass: wrong pnl (+/-1), quick at 45 s vs 46 s, credited wrong, Fee 0 allowed, latch at B == 1.04 S and 1.04 S - 1 micro, q_tally == 0.028 S allowed and +1 micro rejected, TightStart with p_open == 0.01 S rejected and +1 micro accepted, wrong r, wrong l_start (+/-1), wrong l_level (+/-1), FloorRaised with g_peak not above old, TightEnd while not flat, Breach with `b_after` not `<= l_level` (==, +1 micro), Breach when tight is off, Fill at `lock_until - 1` rejected and `lock_until` accepted, Fail event rejected, Meta missing / not last / twice.
- Limits: 0 events, 9 events, 4 proofs, trailing bytes, truncated bytes, an event referencing proof 3 of 3.
- Atomicity: a batch whose last event is invalid changes nothing, including seq and hash.
- Compute units for a 1-event, 4-event and 8-event batch (numbers only, M11 sets budgets); stack check.
- Hand-applied mutations (at least 20, listed in module-5b.md; each must be killed by a named test): `<=` to `<` in the lock check, I-3 `+2` to `+3`, prev_hash compare removed, seq off by one, latch threshold, quick 45 to 46, q_tally `>` to `>=`, p_open strict to non-strict, breach `<=` to `<`, Meta-last check removed, held check removed, fee sign, credited floor to ceil, spread side swapped, and so on.

DEFINITION OF DONE
`scripts/build-test-so.sh` passes; `cd tests-rs && cargo test` passes; `scripts/test-engine.sh` passes; clippy clean (`-D warnings`) for the program, tests-rs and any touched crate; mutation numbers for touched crates; handover/module-5b.md written (instruction accounts in order, the RESOLVE answers with formulas, error table with codes, test counts, CU numbers); commit with explicit paths; do NOT push.

REPORT BACK (paste this block, nothing longer):
- answers to RESOLVE (a), (b), (c) in one paragraph each, with the exact formula for (a)
- test counts total and new, names grouped by invariant
- the 4.9 numbers reproduced (state after each of the three transactions)
- error table (code, name, invariant)
- CU for 1, 4, 8 event batches; stack result
- mutation numbers for touched crates and the 20 hand mutations with the killing test
- every file added or changed, one line each
- DECISIONS where the design was silent
- anything you refused or stopped on
- `git log --oneline -3` and `git status --short`
