# Module M5a: TradingAccount, open_account, real-vault harness (DONE, verified 2026-10-11)

Toolchain as before (rustc 1.94.0, anchor-cli 0.32.1, solana-cli 4.0.2, cargo-build-sbf 4.0.0, litesvm =0.7.0). `post_journal` is NOT in this module (M5b).

## TradingAccount (DESIGN 5.1): 207 bytes, tested field by field
PDA `["acct", trader, challenge_id u64 LE]`; offsets include the 8-byte discriminator (`state/trading_account.rs`, test `layout_offsets_match_design_5_1`).

| field | offset | width | | field | offset | width |
|---|---|---|---|---|---|---|
| discriminator | 0 | 8 | | status | 158 | 1 |
| trader | 8 | 32 | | phase | 159 | 1 |
| challenge_id | 40 | 8 | | latch | 160 | 1 |
| size_s | 48 | 8 | | flat | 161 | 1 |
| created_at | 56 | 8 | | pass_pending | 162 | 1 |
| b | 64 | 8 | | vetoes | 163 | 1 |
| q_tally | 72 | 8 | | bump | 164 | 1 |
| lock_until | 80 | 8 | | tight_on | 165 | 1 |
| last_batch_time | 88 | 8 | | held | 166 | 1 |
| last_activity_pushed | 96 | 8 | | r | 167 | 8 |
| next_request_id | 104 | 8 | | g_peak | 175 | 8 |
| seq | 112 | 8 | | l_level | 183 | 8 |
| journal_hash | 120 | 32 | | pass_requested_at | 191 | 8 |
| trades_closed | 152 | 4 | | pass_proposed_at | 199 | 8 |
| ruleset_version | 156 | 2 | | end | 207 | |

Rent = (128 + 207) x 6960 = 2,331,600 lamports (asserted against the live account). `LEN == 207` is also a `const` assert.
Helpers: `flag(u8)` (0/1 else `InvalidFlag`), `static_floor_micro(size_s, latch)`, `to_engine_state()` and `apply_engine_state()` (both `Result`, see decisions). `l_start` is not stored in the 207 bytes; while tight mode is on it is derived as `l_level - max(0, g_peak - 0.01 S)` (the engine's own formula), and `apply_engine_state` refuses a state whose `l_start` the stored fields cannot reproduce.

## open_account(tier_index: u8): accounts in order (20)
1 `trader` [S, W] (buyer) · 2 `payer` [S, W] (must equal trader) · 3 `config` · 4 `ruleset` (of `config.current_ruleset`) · 5 `wallet` [W] (`["wallet", trader]`, `init_if_needed`) · 6 `trading_account` [W] (`init`, seeds use `wallet.next_challenge_id`) · 7 `sector_authority` (PDA `["setl8_sector_authority"]`) · 8 `vault_program` (= `config.vault_program`) · 9 `product_registry` [W] (vault) · 10 `vault_trader_state` [W] (created by the vault) · 11 `vault_state` · 12 `trader_token_account` [W] (owner = trader, mint = `mint`) · 13 `mint` · 14 `pool_token_account` [W] (vault) · 15 `vault_sl8_token_account` [W] (vault's SL8 destination) · 16 `fee_token_account` [W] (owner = `config.sl8_wallet`, mint = `mint`) · 17 `token_program` · 18 `system_program` · 19 `event_authority` · 20 `program` (19 and 20 from `#[event_cpi]`).

Order of checks: paused (`Paused`); payer == trader (`Unauthorized`); rule set is current, version and PDA (`RuleSetMissing`); `tier_index < tier_count` (`InvalidTier`); wallet profile created if absent; banned (`WalletBanned`); `active_count < 5` (`WalletCapReached`); `challenge_id = next_challenge_id`, then `next_challenge_id + 1` and `active_count + 1`; the 0.4% fee `ceil(tier_cost x fee_bps / 10_000)` (engine `swap_fee_micro`) by `transfer_checked` trader -> fee account (skipped when 0); the vault CPI; initialisation of the account; `Opened` through `emit_cpi!`.

### Vault CPI (`deposit_fee`, shared-interfaces v0.4.0 builder, `invoke_signed` with seeds `["setl8_sector_authority", bump]`)
Accounts exactly as the vault declares them: 0 `sector_authority` [S] · 1 `product_registry` [W] · 2 `trader_state` [W] · 3 `payer` [S, W] (our payer = trader) · 4 `system_program` · 5 `vault_state` · 6 `trader` [S] · 7 `trader_token_account` [W] · 8 `mint` · 9 `pool_token_account` [W] · 10 `vault_sl8_token_account` [W] · 11 `token_program`.
Args: `amount = tier_cost` (never anything else), `product_program_id = this program`, `challenge_id`, `trader_wallet = trader`, `account_size = size_s`. The vault checks `(account_size, amount)` against its registered tiers, takes the price from the trader (pool: `floor(amount x fee_split_bps / 10_000)`, SL8: exact remainder) and rejects mints that are not its own two. Any failure anywhere reverts the fee transfer, the profile and the account.

## Harness (`tests-rs/tests/common/vault.rs`, written fresh; the vault is never touched)
- UPDATE 2026-10-13: the fixture is now vault core commit 9a5c179, sha256 `42c6267008ca25cfd7b71792f3797307292049d91420dbba8fe07dc3098c89a6` (decision 51); the hash in the next bullet is the original one.
- `VaultEnv::new()` loads `tests-rs/fixtures/core_vault.so` (sha256 `ed4c8c7cb93bd209cbd03ed46e58f755ca99e3aff681b4dc75b806cb6d762ba7`, vault HEAD 591ced6; the harness asserts the hash and fails loudly otherwise) AND our localnet `.so` in one LiteSVM, creates USDC and USDT 6-decimal mints, runs the vault's real `init_vault`, registers THIS program as a product (sector authority derived from OUR program id), runs our `init_config` (vault = the vault id, `sl8_wallet` = a separate fee wallet) and `create_ruleset` (launch values), creates the vault's SL8 token account and the fee token account.
- Vault admin signatures: the vault's localnet admin keys are public constants but their key files are not read here. This environment therefore runs with signature verification OFF (`Env::new_with(false)`); the vault admin instructions flag those accounts as signers. Traders and our admins still sign for real where a keypair exists. The 72 older tests keep `Env::new()` (verification ON).
- Vault registration numbers: `fee_split_bps = 6500` (copied from the vault's own `Cfg::default()` in tests-rs/tests/common/mod.rs), `max_payout_count = 4` (DESIGN 10.4), `reset_price_bps = [10000, 11500, 12000, 12750, 13000]` (decision 17), tiers = the rule set's ten (size, cost) pairs in micro-USD.
- Helpers: `open(trader, tier)`, `open_accounts(trader)` (every account, overridable) + `send_open`, `account(addr)`, `wallet_opt`, `edit_wallet`, `vault_trader_state(trader, id)` (decoded by hand from the vault's 115-byte layout), `digest(trader, extra)` (hash of every account `open_account` can touch plus all token accounts and the trader's lamports), `opened_events(meta)` (decodes the `emit_cpi!` inner instruction), `expected_fee`, `vault_split` (the vault's formula).

## Tests
- Program unit tests: 16 (7 new: layout offsets, round trip + rent, engine-state round trips with tight mode off and on, flags 2..=255 rejected, `static_floor_micro` latch 0/1, `apply_engine_state` writes every field). Workspace total 226 (engine 133, journal-types 77, program 16).
- tests-rs: 95 (72 old + 23 new in `tests/open_account.rs`): happy path for every tier index 0..=9 (every field, balances, vault split, vault record, wallet, event); also with USDT; fee and vault SL8 as one account; the ten-tier fee table of DESIGN 9.2; ceil-not-floor-not-round with `fee_bps = 1` and a zero-fee rule set; challenge ids and a replayed address; five accounts and the sixth refused; cap boundary 4 -> 5, 5/6/255 refused; rejections (paused; tier index 10, 11, 255 and a 3-tier rule set; banned and unbanned again; `ban_wallet` on a wallet without a profile; stale rule set and a non-rule-set account; fake mint; token account of another owner or mint; fee account of a wrong owner or mint; vault SL8 account and pool replaced; missing trader signature; payer != trader; wrong sector authority; wrong vault program; wrong vault registry/state/trader_state; balances below cost + fee); atomicity with a vault `pause_product`; compute units.
- Mutation spot-check of `open_account` (16 hand mutants: cap removed / off by one, ban, pause, payer and rule-set checks removed, tier bound, vault amount != cost, no fee, fee to the wrong account, id and count not incremented, `b`, genesis hash, `flat`, `next_request_id`): all killed.

## Compute units and stack
(M11 measured every instruction again with fixed keys and the worst bump-search keys: `open_account` up to 135,595 CU, see handover/module-11.md; the numbers below are the M5a measurements.)
`open_account` consumed 101,341 to 109,994 CU over six runs (the range comes from the PDA bump search for random keys; about 44.9k are inside the vault's `deposit_fee`, 6.1k per token transfer, 6.5k for the event CPI). The first open of a wallet (profile created) and later opens differ by about 0 to 1.5k. Default request is 200k, so it fits; M11 sets budgets.
`scripts/build-test-so.sh` and `scripts/anchor-build-checked.sh` (default and localnet): no stack-frame overflow. Large accounts are `Box`ed.

## Decisions (design silent)
1. Error codes appended at the end of the enum (codes above are unchanged): 6039 `InvalidFlag`, 6040 `InvalidTokenAccount`, 6041 `InvalidVaultProgram`, 6042 `EngineStateMismatch`. A wrong `sector_authority` address fails with Anchor's `ConstraintSeeds` (2006); a missing signature with `AccountNotSigner` (3010); `payer != trader` with `Unauthorized`; a stale or wrong rule set with `RuleSetMissing`.
2. The mint is an account, not an instruction argument (`open_account(tier_index)`); the vault accepts only its own two mints.
3. There are TWO SL8 token accounts: `fee_token_account` (owner `config.sl8_wallet`, receives the 0.4%) and `vault_sl8_token_account` (the vault's own destination, owned by the vault's `sl8_wallet`). They may be the same account (tested).
4. The rule set is validated before the tier index is read (its `tier_count` cannot be trusted otherwise); both checks come before any state change, so the order only decides which error a doubly-wrong call reports.
5. `to_engine_state` and `apply_engine_state` return `Result` (the prompt's signature had none) because flags other than 0/1 must be rejected. `apply_engine_state` also refuses a size change and an `l_start` the 207-byte layout cannot reproduce, and leaves the account unchanged when it refuses.
6. A zero fee (`fee_bps = 0`) skips the transfer instead of sending a zero-amount transfer.
7. `payer` is a separate signer slot that must equal `trader` (DESIGN 6.2); because Solana merges the signer flags of equal keys, a missing trader signature means both flags missing.
8. The program depends on `journal-types` with `default-features = false`; `genesis_hash` runs on the `sol_sha256` syscall via `solana-sha256-hasher` (anchor does not re-export `hash`). `solana-sha256-hasher` is already in the lock file (a dependency of `solana-program`).
9. `[lints.rust] unexpected_cfgs` is declared for Anchor's internal cfgs and the program has lint denies for unwrap/expect/panic/indexing/truncating casts in non-test code; this found three `w[0] < w[1]` index spots in M1b's `create_ruleset`, now `is_sorted_by`. `result_large_err` is allowed for tests-rs (litesvm's error type is large).

## Surprises
- Fresh random keys change the CU by up to 8.7k through the PDA bump search; any CU budget test must fix the keys.
- With sigverify OFF a "missing signature" test must clear the `is_signer` flag rather than omit a signature; with equal keys in two slots both flags must be cleared.
