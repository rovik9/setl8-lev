//! Harness for the real vault: loads the vault fixture (`tests-rs/fixtures/core_vault.so`, vault core commit 9a5c179) AND our
//! localnet `.so` into one LiteSVM, runs the vault's `init_vault`, registers THIS program as a product, creates the USDC
//! and USDT test mints, funds token accounts and exposes `open`, `account`, `vault_trader_state`.
//!
//! Signature verification is OFF in this environment: the vault's localnet admin keys are public constants whose key
//! files this repo never reads, so the vault admin instructions flag them as signers without signatures (the same trick
//! the vault's own harness uses for PDA signers). Our own test admins and every trader still sign for real where a
//! keypair is available; "missing signature" cases flip the account's `is_signer` flag, which is what the programs check.
//!
//! Numbers used when registering the product (source: `../setl8-vault/tests-rs/tests/common/mod.rs` `Cfg::default()` and
//! DESIGN 10.4 / decision 17): fee split 6500 bps (pool gets floor(cost x 6500 / 10_000), the SL8 wallet the exact
//! remainder), `max_payout_count` 4 (one payout per phase 2 to 5), reset ladder [10000, 11500, 12000, 12750, 13000] bps.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};
use anchor_lang::{AccountDeserialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token::{
    self,
    solana_program::{program_option::COption, program_pack::Pack},
    state::{Account as SplAccount, AccountState, Mint as SplMint},
};
use litesvm::types::{TransactionMetadata, TransactionResult};
use setl8_lev_trading::{
    accounts, constants as c,
    events::Opened,
    instruction,
    instructions::RuleSetArgs,
    state::{TradingAccount, WalletProfile},
};
use setl8_shared_interfaces as si;
use sha2::{Digest, Sha256};
use solana_account::Account as RawAccount;
use solana_keypair::Keypair;
use solana_signer::Signer;

use super::{config_pda, default_args, dup, init_ix_with, launch_ruleset_args, tally_pda, Env, PROGRAM_ID};

/// The vault's `declare_id!` (../setl8-vault/programs/core-vault/src/lib.rs).
pub const VAULT_ID: Pubkey = anchor_lang::prelude::pubkey!("2Z6WNsj4hNhKhmK9Cj3sXV5San9VYhh8gwtyvBfpP6ft");
/// The vault's LOCALNET admin pubkeys (public constants of ../setl8-vault/programs/core-vault/src/constants/admin.rs).
pub const VAULT_SL8_ADMIN: Pubkey = anchor_lang::prelude::pubkey!("9SWJUtUBp1AyqfmAAFffbRfb4Qnr2u6Jqek8vHZMkFKP");
pub const VAULT_ROV_ADMIN: Pubkey = anchor_lang::prelude::pubkey!("D1EuhXLMWzz9Ypy9gkkwjpzVhEXi4RoDc6NQZv9og1m6");
/// sha256 of the vault fixture (vault core commit 9a5c179, localnet build).
pub const VAULT_FIXTURE_SHA256: &str = "42c6267008ca25cfd7b71792f3797307292049d91420dbba8fe07dc3098c89a6";
pub const FEE_SPLIT_BPS: u16 = 6500;
pub const RESET_LADDER_BPS: [u16; 5] = [10_000, 11_500, 12_000, 12_750, 13_000];
/// A trader's starting USDC: 100,000 USDC in base units (6 decimals).
pub const TRADER_START: u64 = 100_000_000_000;

pub fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/core_vault.so")
}

pub fn vault_state_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"vault_state", VAULT_SL8_ADMIN.as_ref(), VAULT_ROV_ADMIN.as_ref()], &VAULT_ID).0
}
pub fn pool_pda(vault_state: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"pool", vault_state.as_ref(), mint.as_ref()], &VAULT_ID).0
}
pub fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"product_registry", PROGRAM_ID.as_ref()], &VAULT_ID).0
}
pub fn vault_trader_state_pda(trader: &Pubkey, challenge_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"trader_state", PROGRAM_ID.as_ref(), trader.as_ref(), &challenge_id.to_le_bytes()], &VAULT_ID).0
}
pub fn sector_authority_pda() -> Pubkey {
    si::derive_sector_authority(&PROGRAM_ID).0
}
pub fn acct_pda(trader: &Pubkey, challenge_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[c::ACCT_SEED, trader.as_ref(), &challenge_id.to_le_bytes()], &PROGRAM_ID).0
}
fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}

fn anchor_disc(name: &str) -> [u8; 8] {
    let h = Sha256::digest(format!("global:{name}").as_bytes());
    let mut d = [0u8; 8];
    d.copy_from_slice(&h[..8]);
    d
}

/// Construction options.
#[derive(Clone)]
pub struct Opts {
    /// Rule set 1 (default: the DESIGN 5.4 launch values). The vault is registered with ITS tiers.
    pub ruleset: RuleSetArgs,
    /// `config.sl8_wallet` = the vault's SL8 admin, so the fee account and the vault's SL8 account can be one account.
    pub fee_wallet_is_vault_sl8: bool,
    /// Do not create rule set 1 in `with` (the caller creates it later, for example after the markets, the deploy order of
    /// handover/launch-config.md). The vault is still registered with the tiers of `ruleset`.
    pub skip_ruleset: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { ruleset: launch_ruleset_args(), fee_wallet_is_vault_sl8: false, skip_ruleset: false }
    }
}

/// What the vault stores per challenge (`TraderState`, decoded by hand from its 115-byte layout).
#[derive(Debug, PartialEq, Eq)]
pub struct VaultTraderState {
    pub trader_wallet: Pubkey,
    pub product_program_id: Pubkey,
    pub challenge_id: u64,
    pub account_size: u64,
    pub payout_count: u64,
    pub status: u8,
    pub last_activity_timestamp: i64,
}

pub struct VaultEnv {
    pub e: Env,
    pub usdc: Pubkey,
    pub usdt: Pubkey,
    pub vault_state: Pubkey,
    pub usdc_pool: Pubkey,
    pub usdt_pool: Pubkey,
    /// The vault's SL8 destination for USDC (owner = the vault's SL8 admin).
    pub vault_sl8_usdc: Pubkey,
    /// `config.sl8_wallet` and its USDC account (receives the 0.4% fee).
    pub fee_wallet: Pubkey,
    pub fee_usdc: Pubkey,
    pub ruleset: RuleSetArgs,
    /// The evaluator key stored in `config.evaluator` (a real keypair so journal tests can sign with it).
    pub evaluator: Keypair,
    pub tracked: Vec<Pubkey>,
    pub traders: HashMap<Pubkey, Pubkey>,
}

/// Every account of `open_account`, with the defaults a correct call uses; tests override single fields.
#[derive(Clone)]
pub struct OpenAccounts {
    pub trader: Pubkey,
    pub payer: Pubkey,
    pub config: Pubkey,
    pub ruleset: Pubkey,
    pub wallet: Pubkey,
    pub trading_account: Pubkey,
    pub sector_authority: Pubkey,
    pub vault_program: Pubkey,
    pub product_registry: Pubkey,
    pub vault_trader_state: Pubkey,
    pub vault_state: Pubkey,
    pub trader_token_account: Pubkey,
    pub mint: Pubkey,
    pub pool_token_account: Pubkey,
    pub vault_sl8_token_account: Pubkey,
    pub fee_token_account: Pubkey,
    pub token_program: Pubkey,
    pub system_program: Pubkey,
    pub trader_signs: bool,
    pub payer_signs: bool,
}

impl OpenAccounts {
    pub fn to_ix(&self, tier_index: u8) -> Instruction {
        let mut metas = accounts::OpenAccount {
            trader: self.trader,
            payer: self.payer,
            config: self.config,
            ruleset: self.ruleset,
            wallet: self.wallet,
            trading_account: self.trading_account,
            sector_authority: self.sector_authority,
            vault_program: self.vault_program,
            product_registry: self.product_registry,
            vault_trader_state: self.vault_trader_state,
            vault_state: self.vault_state,
            trader_token_account: self.trader_token_account,
            mint: self.mint,
            pool_token_account: self.pool_token_account,
            vault_sl8_token_account: self.vault_sl8_token_account,
            fee_token_account: self.fee_token_account,
            token_program: self.token_program,
            system_program: self.system_program,
            event_authority: event_authority(),
            program: PROGRAM_ID,
        }
        .to_account_metas(None);
        // the signer flags of the two signer slots (they may be the same key)
        for (i, signs) in [(0usize, self.trader_signs), (1, self.payer_signs)] {
            metas[i] = AccountMeta { is_signer: signs, ..metas[i].clone() };
        }
        Instruction { program_id: PROGRAM_ID, accounts: metas, data: instruction::OpenAccount { tier_index }.data() }
    }
}

impl VaultEnv {
    pub fn new() -> Self {
        Self::with(Opts::default())
    }

    pub fn with(opts: Opts) -> Self {
        let mut e = Env::new_with(false);
        // the fixture must be the exact vault build this suite was written against
        let bytes = std::fs::read(fixture_path()).unwrap_or_else(|err| panic!("cannot read {} ({err}); see handover/module-5a.md", fixture_path().display()));
        let digest = Sha256::digest(&bytes);
        let got: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, VAULT_FIXTURE_SHA256, "tests-rs/fixtures/core_vault.so is not the expected vault build");
        e.svm.add_program(VAULT_ID, &bytes).expect("load vault");
        for k in [VAULT_SL8_ADMIN, VAULT_ROV_ADMIN] {
            e.svm.airdrop(&k, 10_000_000_000).unwrap();
        }
        let (usdc, usdt) = (Pubkey::new_unique(), Pubkey::new_unique());
        let vault_state = vault_state_pda();
        let fee_wallet = if opts.fee_wallet_is_vault_sl8 { VAULT_SL8_ADMIN } else { Pubkey::new_unique() };
        let mut v = VaultEnv {
            e,
            usdc,
            usdt,
            vault_state,
            usdc_pool: pool_pda(&vault_state, &usdc),
            usdt_pool: pool_pda(&vault_state, &usdt),
            vault_sl8_usdc: Pubkey::default(),
            fee_wallet,
            fee_usdc: Pubkey::default(),
            ruleset: opts.ruleset.clone(),
            evaluator: Keypair::new(),
            tracked: vec![],
            traders: HashMap::new(),
        };
        v.set_mint(&usdc);
        v.set_mint(&usdt);
        v.vault_sl8_usdc = v.new_token_account(&usdc, &VAULT_SL8_ADMIN, 0);
        v.fee_usdc = if opts.fee_wallet_is_vault_sl8 { v.vault_sl8_usdc } else { v.new_token_account(&usdc, &fee_wallet, 0) };
        v.tracked.extend([v.usdc_pool, v.usdt_pool]);

        // vault: init_vault, then register THIS program as a product with the rule set's tiers
        let init = Instruction {
            program_id: VAULT_ID,
            accounts: vec![
                AccountMeta::new(VAULT_SL8_ADMIN, true),
                AccountMeta::new_readonly(VAULT_ROV_ADMIN, true),
                AccountMeta::new(vault_state, false),
                AccountMeta::new_readonly(usdc, false),
                AccountMeta::new_readonly(usdt, false),
                AccountMeta::new(v.usdc_pool, false),
                AccountMeta::new(v.usdt_pool, false),
                AccountMeta::new_readonly(spl_token::ID, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ],
            data: [anchor_disc("init_vault").to_vec(), usdc.to_bytes().to_vec(), usdt.to_bytes().to_vec()].concat(),
        };
        v.send_plain(init).expect("vault init_vault");
        let tiers: Vec<si::ChallengeSize> = (0..usize::from(opts.ruleset.tier_count))
            .map(|i| si::ChallengeSize { size: opts.ruleset.tier_size[i], cost: opts.ruleset.tier_cost[i] })
            .collect();
        let reg = si::register_product(
            VAULT_ID,
            VAULT_SL8_ADMIN,
            VAULT_ROV_ADMIN,
            registry_pda(),
            &[AccountMeta::new_readonly(system_program::ID, false)],
            si::RegisterProductArgs {
                product_program_id: PROGRAM_ID,
                fee_split_bps: FEE_SPLIT_BPS,
                challenge_sizes: tiers,
                max_payout_count: 4,
                reset_price_bps: RESET_LADDER_BPS.to_vec(),
            },
        );
        v.send_plain(reg).expect("vault register_product");

        // our program: init_config pointing at the vault, then rule set 1
        let mut args = default_args();
        args.vault_program = VAULT_ID;
        args.sl8_wallet = fee_wallet;
        args.evaluator = v.evaluator.pubkey();
        v.e.svm.airdrop(&v.evaluator.pubkey(), 1_000_000_000).unwrap();
        let ix = init_ix_with(v.e.payer.pubkey(), v.e.sl8.pubkey(), v.e.rov.pubkey(), &args);
        super::assert_ok(v.e.send_as_admins(&[ix]));
        if !opts.skip_ruleset {
            v.e.create_ruleset_ok(&opts.ruleset);
        }
        v
    }

    /// Sends one instruction without extra signers (the payer signs; unsigned flagged signers pass because sigverify is off).
    pub fn send_plain(&mut self, ix: Instruction) -> TransactionResult {
        self.e.send(&[ix], &[])
    }

    pub fn set_raw(&mut self, addr: &Pubkey, data: Vec<u8>, owner: Pubkey) {
        let lamports = self.e.svm.minimum_balance_for_rent_exemption(data.len());
        self.e.svm.set_account(*addr, RawAccount { lamports, data, owner, executable: false, rent_epoch: 0 }).unwrap();
    }

    pub fn set_mint(&mut self, addr: &Pubkey) {
        let mut data = vec![0u8; SplMint::LEN];
        SplMint::pack(SplMint { mint_authority: COption::None, supply: 0, decimals: 6, is_initialized: true, freeze_authority: COption::None }, &mut data).unwrap();
        self.set_raw(addr, data, spl_token::ID);
    }

    pub fn set_token_account(&mut self, addr: &Pubkey, mint: &Pubkey, owner: &Pubkey, amount: u64) {
        let mut data = vec![0u8; SplAccount::LEN];
        SplAccount::pack(
            SplAccount { mint: *mint, owner: *owner, amount, delegate: COption::None, state: AccountState::Initialized, is_native: COption::None, delegated_amount: 0, close_authority: COption::None },
            &mut data,
        )
        .unwrap();
        self.set_raw(addr, data, spl_token::ID);
    }

    pub fn new_token_account(&mut self, mint: &Pubkey, owner: &Pubkey, amount: u64) -> Pubkey {
        let addr = Pubkey::new_unique();
        self.set_token_account(&addr, mint, owner, amount);
        self.tracked.push(addr);
        addr
    }

    pub fn token_balance(&self, addr: &Pubkey) -> u64 {
        let a = self.e.svm.get_account(addr).expect("token account");
        SplAccount::unpack(&a.data).expect("valid token account").amount
    }

    /// A new trader with 10 SOL and a USDC account holding `TRADER_START`.
    pub fn new_trader(&mut self) -> Keypair {
        self.new_trader_with(TRADER_START)
    }

    /// A trader whose key is `Keypair::new_from_array(seed as 4 little-endian bytes, then 0x5A)`: the program-derived addresses of its accounts (and so
    /// the compute units spent on bump searches) are the same in every run (M11 budget measurements).
    pub fn new_trader_seeded(&mut self, seed: u32) -> Keypair {
        let mut bytes = [0x5Au8; 32];
        bytes[..4].copy_from_slice(&seed.to_le_bytes());
        let k = Keypair::new_from_array(bytes);
        self.e.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        let mint = self.usdc;
        let ta = self.new_token_account(&mint, &k.pubkey(), TRADER_START);
        self.traders.insert(k.pubkey(), ta);
        k
    }

    pub fn new_trader_with(&mut self, usdc: u64) -> Keypair {
        let k = Keypair::new();
        self.e.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        let mint = self.usdc;
        let ta = self.new_token_account(&mint, &k.pubkey(), usdc);
        self.traders.insert(k.pubkey(), ta);
        k
    }

    pub fn trader_token(&self, trader: &Pubkey) -> Pubkey {
        *self.traders.get(trader).expect("trader created with new_trader")
    }

    /// The accounts of a correct `open_account` call for `trader`'s NEXT challenge.
    pub fn open_accounts(&self, trader: &Pubkey) -> OpenAccounts {
        let id = self.wallet_opt(trader).map_or(0, |w| w.next_challenge_id);
        OpenAccounts {
            trader: *trader,
            payer: *trader,
            config: config_pda(),
            ruleset: super::ruleset_pda(1),
            wallet: super::wallet_pda(trader),
            trading_account: acct_pda(trader, id),
            sector_authority: sector_authority_pda(),
            vault_program: VAULT_ID,
            product_registry: registry_pda(),
            vault_trader_state: vault_trader_state_pda(trader, id),
            vault_state: self.vault_state,
            trader_token_account: self.trader_token(trader),
            mint: self.usdc,
            pool_token_account: self.usdc_pool,
            vault_sl8_token_account: self.vault_sl8_usdc,
            fee_token_account: self.fee_usdc,
            token_program: spl_token::ID,
            system_program: system_program::ID,
            trader_signs: true,
            payer_signs: true,
        }
    }

    pub fn send_open(&mut self, oa: &OpenAccounts, tier_index: u8, signer: &Keypair) -> TransactionResult {
        let ix = oa.to_ix(tier_index);
        let s = dup(signer);
        self.e.send(&[ix], &[&s])
    }

    /// `open_account` for `trader`, tier `tier_index`, USDC.
    pub fn open(&mut self, trader: &Keypair, tier_index: u8) -> TransactionResult {
        let oa = self.open_accounts(&trader.pubkey());
        self.send_open(&oa, tier_index, trader)
    }

    pub fn account(&self, addr: &Pubkey) -> TradingAccount {
        let acc = self.e.svm.get_account(addr).expect("trading account exists");
        assert_eq!(acc.owner, PROGRAM_ID);
        TradingAccount::try_deserialize(&mut acc.data.as_slice()).unwrap()
    }

    pub fn wallet_opt(&self, trader: &Pubkey) -> Option<WalletProfile> {
        let acc = self.e.svm.get_account(&super::wallet_pda(trader))?;
        if acc.data.is_empty() {
            return None;
        }
        WalletProfile::try_deserialize(&mut acc.data.as_slice()).ok()
    }

    /// Edits the wallet profile in place (test helper: failing and re-opening is M7).
    pub fn edit_wallet(&mut self, trader: &Pubkey, f: impl FnOnce(&mut WalletProfile)) {
        let addr = super::wallet_pda(trader);
        let mut acc = self.e.svm.get_account(&addr).expect("wallet profile");
        let mut w = WalletProfile::try_deserialize(&mut acc.data.as_slice()).unwrap();
        f(&mut w);
        let mut data = Vec::new();
        anchor_lang::AccountSerialize::try_serialize(&w, &mut data).unwrap();
        acc.data = data;
        self.e.svm.set_account(addr, acc).unwrap();
    }

    pub fn vault_trader_state(&self, trader: &Pubkey, challenge_id: u64) -> Option<VaultTraderState> {
        let acc = self.e.svm.get_account(&vault_trader_state_pda(trader, challenge_id))?;
        if acc.owner != VAULT_ID || acc.data.len() < 115 {
            return None;
        }
        let d = &acc.data;
        let u64_at = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
        Some(VaultTraderState {
            trader_wallet: Pubkey::new_from_array(d[8..40].try_into().unwrap()),
            product_program_id: Pubkey::new_from_array(d[40..72].try_into().unwrap()),
            challenge_id: u64_at(72),
            account_size: u64_at(80),
            payout_count: u64_at(88),
            status: d[96],
            last_activity_timestamp: i64::from_le_bytes(d[97..105].try_into().unwrap()),
        })
    }

    /// The 0.4% fee for tier `i` of this env's rule set, from the DESIGN formula `ceil(cost x bps / 10_000)`.
    pub fn expected_fee(&self, tier: usize) -> u64 {
        let n = u128::from(self.ruleset.tier_cost[tier]) * u128::from(self.ruleset.fee_bps);
        u64::try_from(n.div_ceil(10_000)).unwrap()
    }

    /// The vault's own split (`split_amount` in utils/token_payment.rs): pool gets floor(amount x bps / 10_000), SL8 the rest.
    pub fn vault_split(amount: u64) -> (u64, u64) {
        let pool = u64::try_from(u128::from(amount) * u128::from(FEE_SPLIT_BPS) / 10_000).unwrap();
        (pool, amount - pool)
    }

    /// A digest of every account `open_account` can touch (programs' accounts, vault records, all tracked token
    /// accounts, `extra`) plus the trader's lamports. Equal before and after a rejected call.
    pub fn digest(&self, trader: &Pubkey, extra: &[Pubkey]) -> [u8; 32] {
        let mut addrs: Vec<Pubkey> = vec![config_pda(), tally_pda(), super::wallet_pda(trader), registry_pda(), self.vault_state];
        for v in 1..=3u16 {
            addrs.push(super::ruleset_pda(v));
        }
        for id in 0..8u64 {
            addrs.push(acct_pda(trader, id));
            addrs.push(vault_trader_state_pda(trader, id));
        }
        addrs.extend(self.tracked.iter().copied());
        addrs.extend(extra.iter().copied());
        addrs.push(*trader);
        let mut seen = BTreeMap::new();
        for a in addrs {
            seen.insert(a, ());
        }
        let mut h = Sha256::new();
        for a in seen.keys() {
            h.update(a.as_ref());
            match self.e.svm.get_account(a) {
                Some(acc) if acc.lamports > 0 => {
                    h.update([1u8]);
                    h.update(acc.lamports.to_le_bytes());
                    h.update(acc.owner.as_ref());
                    h.update((acc.data.len() as u64).to_le_bytes());
                    h.update(&acc.data);
                }
                _ => h.update([0u8]),
            }
        }
        h.finalize().into()
    }
}

/// The parts of the vault's `ProductRegistry` this suite checks, decoded by hand from the Anchor/Borsh layout
/// (8-byte discriminator, `product_program_id`, `challenge_sizes`, `fee_split_bps`, `max_payout_count`, `active`,
/// `total_requests_emitted`, `reset_price_bps`, ...; the vault source at 9a5c179 is the reference).
#[derive(Debug, PartialEq, Eq)]
pub struct VaultRegistry {
    pub product_program_id: Pubkey,
    /// `(size, cost)` in registration order.
    pub challenge_sizes: Vec<(u64, u64)>,
    pub fee_split_bps: u16,
    pub max_payout_count: u64,
    pub active: bool,
    pub reset_price_bps: Vec<u16>,
}

impl VaultEnv {
    /// Reads the product registry back from the vault account.
    pub fn registry(&self) -> VaultRegistry {
        let acc = self.e.svm.get_account(&registry_pda()).expect("product registry exists");
        assert_eq!(acc.owner, VAULT_ID);
        let d = &acc.data;
        let u64_at = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
        let mut o = 8;
        let product_program_id = Pubkey::new_from_array(d[o..o + 32].try_into().unwrap());
        o += 32;
        let n = u32_at(o);
        o += 4;
        let mut challenge_sizes = Vec::new();
        for _ in 0..n {
            challenge_sizes.push((u64_at(o), u64_at(o + 8)));
            o += 16;
        }
        let fee_split_bps = u16::from_le_bytes(d[o..o + 2].try_into().unwrap());
        o += 2;
        let max_payout_count = u64_at(o);
        o += 8;
        let active = d[o] != 0;
        o += 1 + 8; // active, total_requests_emitted
        let m = u32_at(o);
        o += 4;
        let reset_price_bps = (0..m).map(|i| u16::from_le_bytes(d[o + 2 * i..o + 2 * i + 2].try_into().unwrap())).collect();
        VaultRegistry { product_program_id, challenge_sizes, fee_split_bps, max_payout_count, active, reset_price_bps }
    }

    /// Marks the vault record `Failed` the way `report_fail` would (`flag_trader_failed`). The `sector_authority` PDA
    /// is flagged as a signer without a signature, which works because this environment runs with signature
    /// verification off (the vault only checks the key and the flag; our program's `invoke_signed` is what provides it
    /// in production).
    pub fn flag_vault_failed(&mut self, trader: &Pubkey, challenge_id: u64) -> TransactionResult {
        let ix = si::flag_trader_failed(
            VAULT_ID,
            sector_authority_pda(),
            registry_pda(),
            &[AccountMeta::new(vault_trader_state_pda(trader, challenge_id), false)],
            si::FlagTraderFailedArgs { trader_wallet: *trader, product_program_id: PROGRAM_ID, challenge_id },
        );
        self.send_plain(ix)
    }

    /// The vault's `deposit_reset` called DIRECTLY (not through our program: M8 does not exist yet), with the
    /// `sector_authority` PDA flagged as a signer as in `flag_vault_failed`. The trader signs for real.
    pub fn vault_deposit_reset(&mut self, trader: &Keypair, prev_id: u64, new_id: u64, amount: u64, reset_phase: u8) -> TransactionResult {
        let tk = trader.pubkey();
        let ix = si::deposit_reset(
            VAULT_ID,
            sector_authority_pda(),
            registry_pda(),
            &[
                AccountMeta::new(vault_trader_state_pda(&tk, prev_id), false),
                AccountMeta::new(vault_trader_state_pda(&tk, new_id), false),
                AccountMeta::new(tk, true),
                AccountMeta::new_readonly(system_program::ID, false),
                AccountMeta::new_readonly(self.vault_state, false),
                AccountMeta::new_readonly(tk, true),
                AccountMeta::new(self.trader_token(&tk), false),
                AccountMeta::new_readonly(self.usdc, false),
                AccountMeta::new(self.usdc_pool, false),
                AccountMeta::new(self.vault_sl8_usdc, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            si::DepositResetArgs { amount, trader_wallet: tk, product_program_id: PROGRAM_ID, prev_challenge_id: prev_id, new_challenge_id: new_id, reset_phase },
        );
        let s = dup(trader);
        self.e.send(&[ix], &[&s])
    }

    /// Overwrites `account_size` (offset 80) and `payout_count` (offset 88) of a vault `TraderState` in place.
    pub fn edit_vault_record(&mut self, trader: &Pubkey, challenge_id: u64, account_size: Option<u64>, payout_count: Option<u64>) {
        let addr = vault_trader_state_pda(trader, challenge_id);
        let mut acc = self.e.svm.get_account(&addr).expect("vault trader state");
        if let Some(v) = account_size {
            acc.data[80..88].copy_from_slice(&v.to_le_bytes());
        }
        if let Some(v) = payout_count {
            acc.data[88..96].copy_from_slice(&v.to_le_bytes());
        }
        self.e.svm.set_account(addr, acc).unwrap();
    }

    /// `update_product_config` replacing the tiers AND the reset ladder (both vault admins, signature verification off).
    pub fn vault_set_config(&mut self, tiers: Vec<(u64, u64)>, ladder: Vec<u16>) -> TransactionResult {
        let cur = self.registry();
        let ix = si::update_product_config(
            VAULT_ID,
            VAULT_SL8_ADMIN,
            VAULT_ROV_ADMIN,
            registry_pda(),
            &[],
            si::UpdateProductConfigArgs {
                product_program_id: PROGRAM_ID,
                challenge_sizes: tiers.into_iter().map(|(size, cost)| si::ChallengeSize { size, cost }).collect(),
                fee_split_bps: cur.fee_split_bps,
                max_payout_count: cur.max_payout_count,
                reset_price_bps: ladder,
            },
        );
        self.send_plain(ix)
    }

    /// `update_product_config` by both vault admins (signature verification is off), keeping everything but the tiers.
    pub fn vault_set_tiers(&mut self, tiers: Vec<(u64, u64)>) -> TransactionResult {
        let cur = self.registry();
        let ix = si::update_product_config(
            VAULT_ID,
            VAULT_SL8_ADMIN,
            VAULT_ROV_ADMIN,
            registry_pda(),
            &[],
            si::UpdateProductConfigArgs {
                product_program_id: PROGRAM_ID,
                challenge_sizes: tiers.into_iter().map(|(size, cost)| si::ChallengeSize { size, cost }).collect(),
                fee_split_bps: cur.fee_split_bps,
                max_payout_count: cur.max_payout_count,
                reset_price_bps: cur.reset_price_bps,
            },
        );
        self.send_plain(ix)
    }
}

/// All `Opened` events emitted through `emit_cpi!` (inner instructions to our program) in a transaction.
pub fn opened_events(meta: &TransactionMetadata) -> Vec<Opened> {
    let mut out = Vec::new();
    for group in &meta.inner_instructions {
        for inner in group {
            let d = &inner.instruction.data;
            if d.len() > 16 && d[..8] == *anchor_lang::event::EVENT_IX_TAG_LE && d[8..16] == *Opened::DISCRIMINATOR {
                out.push(anchor_lang::AnchorDeserialize::deserialize(&mut &d[16..]).expect("event decodes"));
            }
        }
    }
    out
}
