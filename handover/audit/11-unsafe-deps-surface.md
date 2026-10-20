# M12 item 11: unsafe, panics, arithmetic, Anchor surface, features, dependencies

Author: builder self-audit (agent "surface", finding prefix SRF). Base: HEAD b1c37e5 plus the uncommitted work of the other audit agents.
Files read: `programs/setl8-lev-trading/src/**`, `crates/engine/src/**`, `crates/journal-types/src/**`, the git dependency `setl8-shared-interfaces` (checkout 3b0580c), borsh 0.10.4 and anchor-syn 0.32.2 sources from the local cargo cache.
Nothing in `programs/` or `crates/` was edited. No test files written (this item has none by design). One side effect: I ran `touch -c` on `programs/setl8-lev-trading/src/lib.rs` by mistake (mtime only, content unchanged, git shows no diff).

## 0. Result in six lines

1. Production code of the program, `engine` and `journal-types` contains no `unsafe`, no `unwrap()`, no `expect(`, no `panic!`, no `unreachable!`/`todo!`/`unimplemented!`, no non-constant indexing and no unchecked integer arithmetic that a caller can reach. 41 judged hits, 0 caller-triggerable in program, engine or journal-types source.
2. One hit CAN panic (`engine::consts::frac`, raw `i64` multiply, `overflow-checks = true` turns it into a panic), but nothing in production calls it (grep over crates, programs, tests-rs, tools). It is the other agents' ARI-1; I list it, I do not duplicate it.
3. One caller-triggerable ABORT exists in dependency code, not in program source: a lying length prefix on the outer `batch_bytes: Vec<u8>` argument makes borsh 0.10.4 allocate up to 1 MiB on the 32 KiB SBF heap. That is the other agents' JRN-1 (`journal_attacks.rs`). State is not changed (transaction fails), the error is `ProgramFailedToComplete` instead of Anchor 102.
4. `init_if_needed` IS used (2 sites) although this prompt says it must not be. DESIGN 6.2 requires it for the wallet profile. I read the Anchor code generator and the handlers: the re-initialisation guard holds. Recorded as SRF-1 (info).
5. Default build contains only the real admin keys; `localnet` build contains only the test keys. Proven by (a) source (`cfg`), (b) two unit tests in both feature modes, (c) a byte search of the two built `.so` files. No other `cfg(feature)` leaks. The same feature also turns on mock price proofs (`ALLOW_MOCK`), and that is gated identically.
6. `cargo audit`, `cargo deny`, yanked-crate and advisory checks CANNOT run offline and were NOT run. Commands for Rovik are in section 8.

## 1. Lint coverage (which crates deny what)

| crate | `forbid(unsafe_code)` | non-test deny set |
|---|---|---|
| `programs/setl8-lev-trading` | NO (Anchor/Solana macros expand to unsafe entrypoint code; `-W unsafe_code` still gives 0 warnings) | `unwrap_used, expect_used, panic, indexing_slicing, cast_possible_truncation, cast_possible_wrap, cast_sign_loss` (lib.rs:9-18, `cfg_attr(not(test), ...)`) |
| `crates/engine` | yes | the same seven plus `arithmetic_side_effects`, plus `float_arithmetic` (crate-wide, also in tests) |
| `crates/journal-types` | yes | the same seven plus `arithmetic_side_effects` |
| `crates/evaluator` (off-chain) | yes | `float_arithmetic`; `unwrap_used, expect_used, panic` (non-test) |
| `crates/verifier` (off-chain) | no | `unwrap_used, expect_used, panic` (lib.rs:3, not test-gated) |

Gap: the PROGRAM crate lacks `arithmetic_side_effects` and `float_arithmetic` (and `unreachable`, `todo`, `unimplemented`, `string_slice`). Today this costs nothing: I ran `cargo clippy --offline -p setl8-lev-trading -p setl8-lev-engine -p setl8-lev-journal-types -- -D warnings` (clean, 0 warnings) and then again with `-W clippy::arithmetic_side_effects -W clippy::float_arithmetic -W clippy::unreachable -W clippy::todo -W clippy::unimplemented -W clippy::string_slice -W clippy::integer_division -W clippy::unwrap_in_result -W clippy::indexing_slicing -W clippy::unwrap_used -W clippy::expect_used -W clippy::panic`: program crate 0 warnings; engine 1 (`integer_division`, consts.rs:35, which is also the only `allow`ed arithmetic); journal-types 3 (`integer_division` at proof.rs:29, proof.rs:39, session.rs:17; all constant non-zero divisors). The program is clean but nothing STOPS a later raw `a + b` in a handler from passing the gates (SRF-2). The release profile has `overflow-checks = true` (root Cargo.toml), so such an addition would be a panic abort, not a wrap.

Clippy runs used `CARGO_TARGET_DIR=/private/tmp/srf/target` so no repo target dir was touched.

## 2. Production hits, one by one

Method: a Python scan (`/private/tmp/srf/scan.py`, `scan2.py`, `scan3.py`) over every `.rs` file of the three source trees. Everything from the first column-0 `#[cfg(test)]` to end of file is classed TEST (15 files have such a module); the rest is PRODUCTION. Every hit below was then read in context. Comments and string literals were stripped before matching.

### 2.1 Counts per kind (production / test-only)

| kind | production | test only |
|---|---|---|
| `unsafe` | 0 | 0 |
| `.unwrap()` | 0 | 52 |
| `.expect(` | 0 | 0 |
| `panic!` | 0 | 3 |
| `unreachable!`, `todo!`, `unimplemented!` | 0 | 0 |
| `assert*!` / `debug_assert*!` | 6 (5 compile-time `const _: () = assert!`, 1 `debug_assert_eq!`) | 284 |
| indexing / slicing with `[..]` | 0 | 57 |
| panicking std calls the lints do not cover (`split_at`, `copy_from_slice`, `div_euclid`/`rem_euclid`) plus bounded `Vec::with_capacity` | 11 | n/a |
| `.unwrap_or*` | 4 | 0 |
| numeric `as` casts | 2 | 7 |
| arithmetic operators outside checked/saturating helpers (`+ - * / % << >>`) | 18 (14 compile-time constants, 4 run-time) | 32 |
| shared-interfaces (git dependency linked into the program): `expect`, indexing | 2 sites | n/a |

Total production rows judged: 6 + 11 + 4 + 2 + 18 = 41 (plus 2 dependency sites). Judged "cannot fail": 40. Can fail but unreachable from any production path: 1 (consts.rs:35). Caller-triggerable in program, engine or journal-types source: 0.

### 2.2 Production rows

Assertions

| file:line | text | judgment |
|---|---|---|
| state/market.rs:40 | `const _: () = assert!(Market::SIZE == 128);` | compile time, cannot fail at run time |
| state/rule_set.rs:37 | `const _: () = assert!(RuleSet::SIZE == 552);` | same |
| state/sector_config.rs:33 | `const _: () = assert!(SectorConfig::SIZE == 270);` | same |
| state/trading_account.rs:125 | `const _: () = assert!(TradingAccount::LEN == 207);` | same |
| state/wallet_profile.rs:26 | `const _: () = assert!(WalletProfile::SIZE == 56);` | same |
| instructions/post_journal.rs:412 | `debug_assert_eq!(seq, last_seq);` | compiled out in release (SBF build has no debug assertions). Also cannot fail: `apply_batch` already returned `last_seq = first_seq + n - 1` with checked adds (a `SeqOverflow` would have returned earlier), the loop adds 1 per event after the first with `checked_add`, same count |

Panicking std calls not covered by the clippy denies

| file:line | text | judgment |
|---|---|---|
| instructions/create_ruleset.rs:86 | `a.tier_size.split_at(tiers)` | `tiers` is required in `1..=MAX_TIERS` (10) on line 83 and the array has 10 slots: cannot fail |
| instructions/create_ruleset.rs:87 | `a.tier_cost.split_at(tiers)` | same |
| instructions/create_ruleset.rs:92 | `a.reset_ladder_bps.split_at(ladder)` | `ladder` required in `1..=MAX_LADDER` (5), array has 5 slots: cannot fail |
| instructions/market_params.rs:35 | `self.sessions.split_at(used)` | `used <= MAX_SESSIONS` (6) required on line 33, array is `[Session; 6]`: cannot fail |
| journal-types/codec.rs:299 | `self.buf.split_at(n)` | guarded by `n > self.buf.len()` return on line 296: cannot fail |
| journal-types/codec.rs:306 | `a.copy_from_slice(self.take(N)?)` | `take(N)` returns exactly N bytes or an error: cannot fail |
| program/vault_view.rs:81 | `Vec::with_capacity(n)` | `n <= MAX_TIERS` (10) required on line 80, so at most 10 x 16 bytes |
| program/vault_view.rs:93 | `Vec::with_capacity(m)` | `m <= MAX_LADDER` (5) required on line 92 |
| engine/cost.rs:51 | `shifted.div_euclid(i128::from(SECS_PER_DAY))` | constant non-zero positive divisor, `i128`: cannot fail |
| journal-types/session.rs:15 | `.rem_euclid(SECS_PER_WEEK)` | constant non-zero divisor, input saturated into range: cannot fail |
| engine/position.rs:65 | `pnl.div_euclid(2)` | constant divisor 2: cannot fail |

(position.rs:156 `if qty == i64::MIN` is a guard, not a hit. `signum()` at position.rs:84,139 cannot panic.)

`.unwrap_or*` (do any hide an error?)

| file:line | text | judgment |
|---|---|---|
| engine/cost.rs:51 | `i64::try_from(...div_euclid(86400)).unwrap_or(i64::MIN)` | quotient of an `i128` that came from an `i64` is at most about 1e14 in magnitude, `try_from` cannot fail; the fallback is dead |
| journal-types/proof.rs:39 | `i64::try_from(timestamp_ns / 1e9).unwrap_or(i64::MAX)` | `u64::MAX / 1e9` is about 1.8e10, inside `i64`: dead fallback |
| journal-types/session.rs:17 | `u16::try_from(week_secs / 60).unwrap_or(0)` | `week_secs` is `rem_euclid(604800)` so the quotient is below 10080: dead fallback |
| instructions/reset_account.rs:219 | `...position(..).and_then(u8::try_from).unwrap_or(u8::MAX)` | LIVE fallback, hides nothing that matters: if the failed account's size is not in the CURRENT rule set's tier list, `tier_index` becomes 255. It is used only as the `tier_index` field of the `Opened` event (reset_account.rs:240). No state is written from it. The price comes from the vault registry (`reset_amount_micro`) which refuses unknown sizes (`ResetTierInvalid`). Judged benign, event-only sentinel |

`as` casts

| file:line | text | judgment |
|---|---|---|
| journal-types/enums.rs:17 | `self as u8` | `#[repr(u8)]` fieldless enum: lossless |
| instructions/init_config.rs:80 | `PAYOUT_TALLY_MIN_LEN as u64` | constant 25, `usize` to `u64`: lossless on SBF and host. Clippy `cast_possible_truncation` is denied and passes |

Arithmetic outside checked helpers

| file:line | text | judgment |
|---|---|---|
| engine/consts.rs:35 | `size_micro * bps / BPS` in `pub const fn frac`, with `#[allow(clippy::arithmetic_side_effects)]` | CAN panic: overflows `i64` for `size_micro * bps > 9.22e18`, e.g. size above about 9.2e14 micro-USD with bps 10_000. With `overflow-checks = true` that is a panic. NO production caller (`frac` and `consts::static_floor` are used only by their own unit tests and by the evaluator test `rounding_properties.rs`, finding ARI-1). Production uses `frac_exact` (checked i128). Not caller-triggerable through the program. Still a public footgun: any future caller inherits the panic |
| engine/consts.rs:29 | `LOCK_SECS = 12 * 60 * 60` | const evaluation, compile time |
| engine/cost.rs:10 | `ROLLOVER_SECS = 22 * 3600` | same |
| journal-types/session.rs:9, 11 | `7 * 24 * 3600`, `3 * 24 * 3600` | same |
| state/market.rs:36, 37; rule_set.rs:32, 34; sector_config.rs:28, 30; trading_account.rs:120, 122; wallet_profile.rs:22, 23 | `BODY_LEN`/`SIZE`/`LEN` sums | const evaluation, pinned by the `assert!` rows above |
| journal-types/proof.rs:29 | `quantized_value / QUANTIZED_PER_E12` | `i128 / 1_000_000`: constant non-zero divisor, cannot overflow (no `MIN / -1`) |
| journal-types/proof.rs:39 | `timestamp_ns / NS_PER_SEC` | `u64 / 1e9`: cannot fail |
| journal-types/session.rs:17 | `week_secs / 60` | `i128 / 60`: cannot fail |

Dependency code reached by the program (git dependency `setl8-shared-interfaces`, locked to 3b0580c47979387e03f11c280d93a5870c66cd3e)

| file:line | text | judgment |
|---|---|---|
| shared-interfaces/src/lib.rs:159 | `args.serialize(&mut data).expect(..)` in `build_instruction_data` | serialising fixed-size structs into a `Vec<u8>` cannot fail |
| shared-interfaces/src/payout_tally.rs:149-183 | `data[..8]`, `data[VERSION_OFFSET]`, `out[..]`, `data[offset..offset+8]` in `parse`, `write_into`, `read_u64` | each function first rejects `len < 25` (`TallyError::TooShort`) and the program also checks `data.len() >= PAYOUT_TALLY_MIN_LEN` before calling (finalize_pass.rs:140, before `parse`). Cannot fail |

### 2.3 Test-only hits (not part of any deployed build)

`#[cfg(test)]` modules: reset_price.rs:42, vault_view.rs:151, errors.rs:234, pass_common.rs:191, post_journal.rs:450, sector_config.rs:35, market.rs:42, constants/mod.rs:79, wallet_profile.rs:28, constants/admin.rs:28, trading_account.rs:205, rule_set.rs:39, engine units.rs:16, engine consts.rs:43, journal-types lib.rs:68.

| kind | count | where |
|---|---|---|
| `.unwrap()` | 52 | trading_account.rs 25, pass_common.rs 9, reset_price.rs 9, vault_view.rs 5, market.rs 1, rule_set.rs 1, sector_config.rs 1, wallet_profile.rs 1 |
| `panic!` | 3 | pass_common.rs:204, reset_price.rs:57, vault_view.rs:195 (all `other => panic!("expected an error ...")` in test matches) |
| indexing | 57 | byte-layout assertions in the state files, vault_view.rs, reset_price.rs, pass_common.rs |
| `as` | 7 | trading_account.rs 5, vault_view.rs 2 |
| `assert*!` | 284 | all |

Note: the deny set is `cfg_attr(not(test), ...)`, so these never reach the gates. The engine and journal-types test code is in separate `tests/` dirs for the larger suites (not scanned here).

## 3. Anchor-generated and manual surface

### 3.1 Account types, sizes, discriminators

| account | how it is read | size | notes |
|---|---|---|---|
| `SectorConfig`, `TradingAccount`, `RuleSet`, `Market`, `WalletProfile` | `#[account]` (Borsh, 8-byte `sha256("account:<Name>")` discriminator, fixed size, no `Vec`/`String` fields) | 270, 207, 552, 128, 56 | sizes pinned by compile-time asserts (section 2.2). No `zero_copy`, no `realloc` anywhere |
| `PayoutTally` | raw 25 bytes, no discriminator, `UncheckedAccount` + `PayoutTally::parse`/`write_into` | 25 | finalize_pass: `seeds = [PAYOUT_TALLY_SEED]`, `owner = crate::ID`, length check, magic/version check in `parse`. init_config creates it (`create_payout_tally`, also handles a pre-funded address by transfer + allocate + assign) |
| vault `ProductRegistry` / `TraderState` | manual parsers in `vault_view.rs` (bounds-checked `Reader`, discriminator, product id, `n <= 10`, `m <= 5`) | n/a | `check_registry`/`check_trader_state` verify PDA address under the vault program, owner = vault program. Used by reset_account (lines 134, 136, 192) and `vault_status` (activity.rs:42) |
| `Market` as remaining account | `Market::try_deserialize` + owner = this program + `create_program_address(MARKET_SEED, market_id, bump)` equals the passed key (post_journal.rs:103-117); at most 8 accounts | 128 | duplicates in the list are harmless (first match wins) |

### 3.2 `UncheckedAccount` uses

| field | instructions | check |
|---|---|---|
| `admin_a`, `admin_b` | AdminAction (set_evaluator, set_limits, set_paused), create_ruleset, create_market, update_market, ban_wallet, unban_wallet, veto_pass, release_hold, fail_for_violation (set_evaluator, set_limits, set_paused, create_ruleset, create_market, update_market, ban/unban, veto, release_hold, fail_for_violation: 11 instructions) | `require_both_admins` (set_evaluator, set_limits, set_paused when UNpausing, create_ruleset, ban_wallet, unban_wallet, veto_pass, release_hold, fail_for_violation) or `require_either_admin` (create_market, update_market, set_paused when pausing): stored key equality AND `is_signer` (admin_action.rs:29-52). Positional: the `admin_a` slot must hold `config.admin_a`. SL8 and Rov keys differ (unit test). Who-may-call per cell belongs to the who_may_call agent |
| `payout_tally` | init_config, finalize_pass | see 3.1 |
| `sector_authority` | open_account, reset_account, finalize_pass, request_advance, activity (2), report_fail, fail_for_violation | `seeds = [SECTOR_AUTHORITY_SEED], bump` (canonical bump found by Anchor) |
| `vault_program` | same set | `address = config.vault_program @ InvalidVaultProgram` |
| `product_registry` | same set | reset_account: full manual check. All others: NO program-side check, the vault CPI verifies seeds/owner (LIMITED: depends on the vault; tested by the vault-boundary agent) |
| `vault_trader_state` | same set | `vault_status` in activity.rs checks PDA + owner; reset_account checks PDA + owner + header; others rely on the vault |
| `vault_state`, `payout_claim`, `pool_token_account`, `vault_sl8_token_account` | open_account, reset_account, finalize_pass | no program-side check, the vault verifies. The token accounts that are typed (`trader_token_account`, `fee_token_account`, `mint`) carry owner/mint constraints (`InvalidTokenAccount`) |

No `AccountInfo` fields in `#[derive(Accounts)]` structs. `AccountInfo` appears only as helper parameters in `pass_common.rs`/`activity.rs`, always fed from the structs above.

### 3.3 `remaining_accounts`

Only `post_journal` (post_journal.rs:436) and `report_fail` (report_fail.rs:64) read them, through `load_markets` (cap 8, owner, discriminator via `try_deserialize`, PDA re-derivation). No `remaining_accounts` is ever written or forwarded to a CPI.

### 3.4 `realloc`, `close`, `init`, `init_if_needed`

| construct | sites | notes |
|---|---|---|
| `realloc` | none | |
| `close` | `close_account` only (`close = trader`, trader signer, `trading_account.trader == trader`) | terminal status + 7 day retention checked in the handler. PDA seeds contain `challenge_id`, which only increases, so a closed address is never re-targeted by `init` |
| `init` | init_config (config), create_ruleset, create_market, open_account (`trading_account`), reset_account (`trading_account`) | each with `seeds` + `bump`, `space = <TYPE>::SIZE/LEN` |
| `init_if_needed` | open_account.rs:29 (`wallet`), ban_wallet.rs:21 (`wallet`); crate feature `init-if-needed` enabled in Cargo.toml line 23 | USED. See SRF-1 |
| `declare_id!` | lib.rs:33 | `9NAdnJ7ySQqPYuPKu6d6wCbcaHHMnzwFT6xEcRfXX2a7` = base58(sha256("setl8-lev-trading/placeholder-program-id/v1")), recomputed by me: it is the documented PLACEHOLDER, no keypair exists for it. Anchor.toml `[programs.localnet]` carries the same id. `target/deploy/setl8_lev_trading-keypair.json` exists but is git-ignored (`target/`) and, by construction, cannot be the key of the placeholder id; I did not open it. Must be replaced before deploy (checklist) |

### 3.5 `invoke` / `invoke_signed` / CPI sites

| site | target | signer seeds | program id pinned? |
|---|---|---|---|
| open_account.rs:181 `invoke_signed` (`deposit_fee`) | `ctx.accounts.vault_program` | `[SECTOR_AUTHORITY_SEED, bump]`, bump from `find_program_address` (canonical) | yes, `address = config.vault_program` |
| open_account.rs:126 `transfer_checked` (trader -> `fee_token_account`) | SPL Token (`Program<Token>`) | trader signs | `Program<Token>` |
| reset_account.rs:189 `invoke_signed` (`deposit_reset`) | vault | `[SECTOR_AUTHORITY_SEED, ctx.bumps.sector_authority]` | yes |
| reset_account.rs:200 `transfer_checked` | SPL Token | trader signs | yes |
| finalize_pass.rs:114 `invoke_signed` (`request_payout`) | vault | `ctx.bumps.sector_authority` | yes |
| pass_common.rs:57 (`record_activity`), :112 (`flag_trader_failed`) | vault | `authority_bump` passed in = `ctx.bumps.sector_authority` at every call site (request_advance.rs:60, activity.rs:109,159, report_fail.rs:83, fail_for_violation.rs:59) | yes (each caller struct has the `address` constraint) |
| init_config.rs:85-107 `system_program::{create_account, transfer, allocate, assign}` | System | payout tally seeds `[PAYOUT_TALLY_SEED, bump]` | `Program<System>` |

Return data: `clear_return_data()` before each vault call that answers with data, `parse_return` requires the answering program to be the vault and exactly one byte. Every signing CPI uses the one PDA `["setl8_sector_authority"]`; the seeds are constants, no caller-controlled seed is ever signed with.

### 3.6 Argument decoding that allocates

Instruction arguments are fixed-size structs/scalars except `batch_bytes: Vec<u8>` (post_journal, report_fail). Anchor 0.32.2 uses borsh 0.10.4 for it. Read from source (borsh-0.10.4 `de/mod.rs:148-173`): `u8::vec_from_reader` allocates `vec![0u8; min(len, 1 MiB)]` BEFORE reading. With a prefix of 2^16 or 2^32-1 that is 64 KiB / 1 MiB on a 32 KiB heap: allocation failure, abort. Same item as JRN-1 (their tests `journal_attacks.rs`, ignored finding test lines 769-820 observe `ProgramFailedToComplete` + "memory allocation failed"). Not fixable in program source except by taking the argument differently (see the JRN-1 write-up). The inner `JournalBatch::decode` is safe: counts are checked before any allocation (codec.rs:239-262, 395-410).

Heap otherwise: all inputs are bounded by the 1232 byte transaction limit, the event log copy is bounded (8 events, 3 proofs of at most 512 bytes of signed update), so I see no unbounded allocation. I did NOT measure heap headroom at the maximum batch (budget.rs is the place for it).

## 4. Features and the admin keys

| item | finding |
|---|---|
| Cargo features | `default = []`; `localnet = []` (comment: NEVER in default); `no-entrypoint`, `no-idl`, `no-log-ix-name`, `cpi = ["no-entrypoint"]`, `idl-build` |
| uses of `cfg(feature = "localnet")` in program source | exactly 3 places: `constants/admin.rs` (SL8_ADMIN, ROV_ADMIN, one `cfg` pair each) and `instructions/post_journal.rs:16` `ALLOW_MOCK = cfg!(feature = "localnet")` (mock price proofs, source 255). Nothing else in `programs/`, `engine`, `journal-types` references a feature other than `native-sha2` (journal-types, sha2 hashing, off by default for the program: `default-features = false`) |
| who enables `localnet` | only `tests-rs/Cargo.toml:14` (own workspace, own lockfile) and `scripts/build-test-so.sh:16` (`cargo build-sbf --features localnet` into `target/test-deploy/`). No crate in the root workspace depends on the program (grep of `crates/*/Cargo.toml`, `tools/*/Cargo.toml`), so cargo feature unification cannot switch it on in a default build |
| `cargo tree --features localnet` vs default | identical dependency tree (the feature adds no dependency) |
| unit tests, run by me | `cargo test -p setl8-lev-trading --lib -- admin mock_proofs` default features: `default_build_has_the_real_admin_keys` ok, `mock_proofs_follow_the_localnet_feature` ok (ALLOW_MOCK false). With `--features localnet`: `localnet_build_has_the_test_admin_keys` ok, mock test ok (true). 3 passed each time |
| binary check, run by me on the two built artifacts (`/private/tmp/srf/keys.py`) | pubkeys appear in SBF as `lddw` immediates, so the search matches each 8-byte quarter in the split encoding `lo4 + 00000000 + hi4`. `target/deploy/setl8_lev_trading.so` (sha256 7d86b33e1cd5ffeaa7085b5d54fd63b204eba2e5a20f5d9ae70cef13101974af, dated Oct 11 17:01): SL8 real 4/4, ROV real 4/4, SL8 test 0/4, ROV test 0/4. `target/test-deploy/setl8_lev_trading.so` (sha256 754c4b5b9316ef8f70b645dd61713408aa84b460a4f4210fad7f4d1d8c918335, Oct 17 20:39): real 0/4 and 0/4, test 4/4 and 4/4 |

Caveat: the default `.so` is dated before the last source changes (the localnet one is newer). The result proves the gating mechanism, not the final deploy artifact. The check must be repeated on the freshly rebuilt default `.so` (checklist, section 8). The mock-proof gate (`ALLOW_MOCK`) is verified only through the unit test and the source; I did not load the default `.so` into LiteSVM (the real admin keys cannot sign).

## 5. Findings

| id | severity | title | status |
|---|---|---|---|
| SRF-1 | info | `init_if_needed` is used (open_account.wallet, ban_wallet.wallet) against this prompt's "must not be used" | accepted by DESIGN 6.2 (`WalletProfile [W, init if needed]`); guard verified; no change needed, regression tests recommended |
| SRF-2 | low (hardening) | the program crate does not deny `arithmetic_side_effects` / `float_arithmetic` / `unreachable`/`todo`/`unimplemented` although both pure crates do | open; plain implementation change (add the deny lines to lib.rs); currently clean |
| SRF-3 | low (process) | supply chain and reproducibility: git dependency by mutable tag, builds not run with `--locked`, no toolchain pin, advisory tools not installed | open; process items for the deploy checklist |

Cross-references (not new, owned by the other agents): ARI-1 (`engine::consts::frac` raw multiply), JRN-1 (borsh OOM abort on the outer `batch_bytes` prefix). Both counted in section 0.

No caller-triggerable panic, unchecked arithmetic, indexing or `unwrap` was found in the program source, so there is no failing test to hand to the lead for this item. No `#[ignore]` test was written by me, so there was no ignored test to run.

### SRF-1 (info) `init_if_needed`
What it is: `wallet` in `open_account` and in `ban_wallet`, PDA `["wallet", trader]`, plus the cargo feature `init-if-needed` on anchor-lang.
Why it matters: `init_if_needed` is the classic re-initialisation hole when the handler overwrites fields of an existing account.
What I checked: (1) anchor-syn 0.32.2 `constraints.rs:1097-1137`: for an existing account (owner not System) it runs `Account::try_from` (owner must be this program, discriminator must match) and then asserts exact `space`, `owner` and rent exemption; for a new account it creates it with the system program (a pre-funded PDA address is handled by `generate_create_account`). (2) The handlers treat the profile as new only when `wallet.trader == Pubkey::default()` (open_account.rs:107, ban_wallet.rs:39) and never reset counters otherwise (module-1b.md says tested). (3) A profile with `trader == default` can only exist inside the transaction that created it, because both creating handlers set `trader` before returning; a failure reverts the creation. So no persistent all-zero profile exists to be "re-initialised". Result: the guard holds; this is a documented choice, not a hole.
Residual: if a third instruction ever creates a profile without setting `trader`, or `trader` is ever allowed to be the default key, the guard breaks. Suggested tests (the lead may place them): pre-fund the wallet PDA with lamports, then `open_account`; call `ban_wallet` on a wallet that already has `active_count` and `next_challenge_id` set and assert the counters are unchanged afterwards (the module-1b note says this exists; I did not re-verify the test).
Fix type: none required. If the project wants zero `init_if_needed`, that is a DESIGN change (a separate `init_wallet` step).

### SRF-2 (low) missing lint gates in the program crate
Evidence in section 1. With the extra lints enabled the program crate gives 0 warnings today, so every handler already uses checked/saturating helpers. The risk is regression: gates (clippy `-D warnings`) would stay green if someone writes `x + y` in a handler, and `overflow-checks = true` would turn that into a panic abort (a transaction failure, not a wrap, but a panic path for a caller).
Fix: plain implementation change. In `programs/setl8-lev-trading/src/lib.rs` add `clippy::arithmetic_side_effects` and `clippy::float_arithmetic` (plus `clippy::unreachable, clippy::todo, clippy::unimplemented, clippy::string_slice`) to the `cfg_attr(not(test), deny(...))` list. The constants (`BODY_LEN` sums) are const-evaluated and do not trigger the lint. Verify with the clippy command in section 9.

### SRF-3 (low) supply chain and build reproducibility
- `setl8-shared-interfaces` is a git dependency: `git = "https://github.com/rovik9/setl8-turbo", tag = "v0.5.0"`. Cargo.lock pins commit 3b0580c47979387e03f11c280d93a5870c66cd3e. A tag can be moved; only the lockfile protects the build, and `scripts/build-test-so.sh` and `scripts/anchor-build-checked.sh` do not pass `--locked` (grep found no `--locked`/`--frozen` in `scripts/`). The same crate defines the vault CPI shapes and the payout tally layout, so a drifted copy would change what the program sends to the vault.
- No `rust-toolchain` file. Host `rustc 1.94.0`, `cargo-build-sbf 4.0.0` with platform-tools v1.53 (`rustc 1.89.0`). The `.so` hash depends on the platform-tools version; pin and record it.
- `cargo-audit` and `cargo-deny` are not installed on this machine (`which` finds only `cargo-mutants`).
- Tracked lockfiles: `Cargo.lock`, `tests-rs/Cargo.lock`, `tools/{gmx-probe,hl-probe,launch-config,session-observer}/Cargo.lock`. No `deny.toml`/`audit.toml`.
Fix: process, no code change.

## 6. Dependencies of the on-chain program

Commands (all `--offline`, root workspace): `cargo tree -p setl8-lev-trading -e normal`, `... --features localnet`, `... --features cpi,no-idl`, `... --all-features`, `... -d`, `cargo metadata --format-version 1` with a breadth-first search that does not enter proc-macro crates (script `/private/tmp/srf/linked.py`). Raw output kept in `/private/tmp/srf/tree_default.txt` (622 lines), `pkgs.txt`, `linked.txt`.

| view | crates |
|---|---|
| all unique packages in `cargo tree -e normal` (host, includes proc-macros and their dependencies) | 248 |
| candidate run-time crates (all-platform superset, proc-macros excluded) | 161 (4 of them are this repo or its git dependency) |
| with `--all-features` (adds the `idl-build` set: spl-token-2022 pieces, zk-sdk, curve25519 syscalls, regex, aes ... used only by `anchor idl build`, not linked into the `.so`) | 330 |
| default tree vs `--features localnet` vs `--features cpi,no-idl` | identical |

Direct dependencies and locked versions: `anchor-lang 0.32.2` (requirement `0.32.1`, features `init-if-needed`, `event-cpi`), `anchor-spl 0.32.2` (`token` only, `default-features = false`), `setl8-shared-interfaces 0.5.0` (git, commit 3b0580c), `solana-sha256-hasher 2.3.0`, `setl8-lev-engine 0.1.0` (path, no dependencies), `setl8-lev-journal-types 0.1.0` (path, `default-features = false`, so no `sha2`).
Other versions that matter: `solana-program 2.3.0`, `solana-pubkey 2.4.0`, `solana-instruction 2.3.3`, `solana-system-interface 1.0.0`, `spl-token 8.0.0`, `borsh 0.10.4` and `1.8.1`, `bincode 1.3.3`, `curve25519-dalek 4.1.3`, `blake3 1.8.7`, `sha2 0.10.9` and `0.9.9`, `serde 1.0.229`, `bytemuck 1.25.2`, `bs58 0.5.1`.

Unusual things:
1. One git dependency (above), one tag, owned by the project's own GitHub account. No `[patch]`, no `[replace]` anywhere in the workspace manifests. Path dependencies are the two repo crates only.
2. Duplicated major versions in the run-time graph: `base64` 0.12.3/0.21.7/0.22.1, `borsh` 0.10.4/1.8.1, `digest` 0.9.0/0.10.7/0.11.3, `block-buffer` 0.9.0/0.10.4/0.12.1, `crypto-common` 0.1.7/0.2.2, `cpufeatures` 0.2.17/0.3.1, `getrandom` 0.1.16/0.2.17, `rand` 0.7.3/0.8.8, `rand_chacha`, `rand_core`, `sha2` 0.9.9/0.10.9, `solana-loader-v3-interface` 3.0.0/5.0.0, `thiserror` 1.0.69/2.0.21, `wasi` 0.9.0/0.11.1. These are the usual Solana 2.x/Anchor 0.32 mix. `digest 0.11.3` comes from `blake3` (solana-blake3-hasher), the 0.9 line from `libsecp256k1 0.6.0`.
3. Old crates, from my own knowledge and NOT verified online: `libsecp256k1 0.6.0` with `rand 0.7.3`, `getrandom 0.1.16`, `sha2 0.9.9`, `base64 0.12.3` (arrive through `solana-secp256k1-recover`; the program never calls secp256k1 recovery); `bincode 1.3.3` (through anchor-lang); `borsh 0.10.4`. I expect `cargo audit` to print unmaintained-crate notices for some of these. I cannot say which advisories exist today. This is exactly what the online run is for.
4. The off-chain/test side uses `ed25519-dalek 1` (tests-rs `Cargo.toml`). It is not in the on-chain tree (`cargo tree -i ed25519-dalek` finds nothing). The signature-verification review (ed25519, `verify_strict`) is item 8's job; I recall that the 1.x line has a published advisory about a double public key oracle, again to be confirmed by `cargo audit` online.
5. No yanked-crate check is possible offline.

What CANNOT be done offline, and was not done: `cargo audit` (RustSec advisory database), `cargo deny check advisories` (same), yanked-version check (crates.io index), license/ban policy checks beyond what `cargo tree` shows, fetching a newer `setl8-shared-interfaces` to compare with the tag.

## 7. Cannot-test list for this item

- Run-time behaviour of the default (real-key) `.so`: the real admin keys cannot sign in LiteSVM; only the key bytes and the unit tests were checked.
- Anchor-generated code (account validation, discriminator checks, borsh derive output) was read at the generator level for `init_if_needed` only; I did not expand the whole program with `cargo expand`. Panics inside generated code are covered only by the clippy run (generated spans from external proc macros are not linted) and by the other agents' fuzz loop.
- Heap headroom at the largest legal batch (see 3.6).
- Whether the SBF release build really compiles with `overflow-checks = true`: it is set in the root `[profile.release]` and the program is a workspace member, but I did not inspect the build flags of `cargo build-sbf`.
- All items in section 6 that need the network.

## 8. Deploy-checklist additions (Rovik runs these on his Mac, online)

```
cargo install --locked cargo-audit cargo-deny

cd ~/Documents/setl8-lev-trading
cargo audit                                        # root workspace (program, engine, journal-types, evaluator, verifier)
cargo audit --file tests-rs/Cargo.lock
for t in gmx-probe hl-probe launch-config session-observer; do cargo audit --file tools/$t/Cargo.lock; done
cargo deny init                                    # once; then edit deny.toml: sources.allow-git = ["https://github.com/rovik9/setl8-turbo"]
cargo deny check advisories bans licenses sources  # root workspace; advisories includes yanked
cargo deny --manifest-path tests-rs/Cargo.toml check advisories bans sources
cargo tree -p setl8-lev-trading -e normal -d       # eyeball the duplicates in section 6
cargo update --dry-run                             # shows drift against crates.io without changing Cargo.lock
```
and:
1. Build the DEFAULT `.so` fresh with `cargo build-sbf --locked` (add `--locked` to `scripts/anchor-build-checked.sh` and `scripts/build-test-so.sh`), record `cargo-build-sbf --version` (now 4.0.0, platform-tools v1.53) next to the sha256.
2. Run the key check on the fresh default `.so` and on the localnet `.so`: the script `/private/tmp/srf/keys.py` (reproduced in section 9) must print real 4/4 + 4/4 and test 0/4 + 0/4 for the default build. Put the script under `scripts/` (the lead decides; I may not create files outside my fragment).
3. Replace `declare_id!` (lib.rs:33) and `Anchor.toml` with the real program keypair's public key (key generated locally, never printed or committed), rebuild, re-run step 2, and confirm the id inside the `.so`.
4. Do not deploy `target/deploy/setl8_lev_trading-keypair.json` as the program keypair by accident: it is unrelated to the placeholder id.
5. Pin the toolchain: add `rust-toolchain.toml` for the host and record the platform-tools version for SBF.
6. Verify that the git tag `v0.5.0` of `setl8-turbo` still points to 3b0580c47979387e03f11c280d93a5870c66cd3e (`git ls-remote https://github.com/rovik9/setl8-turbo refs/tags/v0.5.0`).
7. After SRF-2 is applied: `cargo clippy --offline -p setl8-lev-trading -- -D warnings` must stay clean.

## 9. Commands that reproduce every number here

```
# lint state (scratch target dir so the repo's target/ is untouched)
cd /Users/rovik/Documents/setl8-lev-trading
export CARGO_TARGET_DIR=/private/tmp/srf/target
cargo clippy --offline -p setl8-lev-trading -p setl8-lev-engine -p setl8-lev-journal-types -- -D warnings
cargo clean --offline -p setl8-lev-trading -p setl8-lev-engine -p setl8-lev-journal-types
cargo clippy --offline -p setl8-lev-trading -p setl8-lev-engine -p setl8-lev-journal-types -- \
  -W clippy::arithmetic_side_effects -W clippy::float_arithmetic -W clippy::unreachable -W clippy::todo \
  -W clippy::unimplemented -W clippy::string_slice -W clippy::integer_division -W clippy::unwrap_in_result \
  -W clippy::indexing_slicing -W clippy::unwrap_used -W clippy::expect_used -W clippy::panic     # 4 integer_division warnings, none in the program

# hit scan (counts in 2.1); files: /private/tmp/srf/scan.py scan2.py scan3.py, output /private/tmp/srf/hits.json
python3 -I /private/tmp/srf/scan.py

# feature gating unit tests
cargo test --offline -p setl8-lev-trading --lib -- admin mock_proofs
cargo test --offline -p setl8-lev-trading --lib --features localnet -- admin mock_proofs

# binary key check
python3 -I /private/tmp/srf/keys.py target/deploy/setl8_lev_trading.so target/test-deploy/setl8_lev_trading.so

# dependencies
cargo tree --offline -p setl8-lev-trading -e normal
cargo tree --offline -p setl8-lev-trading -e normal --features localnet
cargo tree --offline -p setl8-lev-trading -e normal --all-features --prefix none --format '{p}' | sort -u | wc -l
cargo tree --offline -p setl8-lev-trading -e normal -d
cargo metadata --offline --format-version 1 > /private/tmp/srf/meta.json && python3 -I /private/tmp/srf/linked.py

# placeholder id
python3 -c "import hashlib; ..."   # base58(sha256(b'setl8-lev-trading/placeholder-program-id/v1')) == 9NAdnJ7ySQqPYuPKu6d6wCbcaHHMnzwFT6xEcRfXX2a7
```

The key-check script in short form, so it survives `/private/tmp` being cleaned: decode each base58 admin key to 32 bytes, split into four 8-byte chunks `c`, and count the chunks for which `c[:4] + b"\x00"*4 + c[4:]` (or `c` itself) occurs in the `.so` bytes. Keys: SL8 real `SL89fcsKAuWYtkEJah86WLLBxCiHd1DeNczpsDjSHDJ`, Rov real `RovSQZxURnNgNkf7WkzRJTnhSBuN6tdfG2CQxp3rzKZ`, SL8 test `F25s3DdjXdCxYBhh2z8FBusVEMT4b9bGNFVKJi3wFoF4`, Rov test `Bow1CGKGDB9mNxeWdw85E2aCthQ1oZX4oFEe7fYT17ew` (all public keys; the test secret halves are public by design).

## 10. Totals for the report

- Hits judged in production source: 41 (+2 in the git dependency). Cannot fail: 40. Can fail but no production caller: 1 (`engine::consts::frac`, ARI-1). Caller-triggerable in program, engine or journal-types source: 0.
- Caller-triggerable abort in dependency code: 1 (borsh 0.10.4 allocation, JRN-1), state unchanged.
- Test-only hits: 52 `unwrap()`, 3 `panic!`, 57 indexing, 7 `as`, 284 asserts.
- `unsafe` in the three source trees: 0. `init_if_needed` sites: 2. `realloc`: 0. `close`: 1. `UncheckedAccount` field names: 10 (section 3.2). Signing CPI sites: 5 `invoke_signed` (open_account, reset_account, finalize_pass, 2 in pass_common) plus 3 `system_program` calls with signer seeds in init_config (`create_account`, `allocate`, `assign`; the top-up `transfer` is unsigned).
- Findings: SRF-1 info, SRF-2 low, SRF-3 low. No critical, high or medium.
