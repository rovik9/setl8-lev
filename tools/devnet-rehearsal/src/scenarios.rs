//! S0 and S1: the two programs, the vault, the mints and the token accounts, the registration of this program as a product with the
//! REAL launch values, and the configuration of this program (`init_config`, 21 markets, rule set v1).
use anchor_lang::solana_program::pubkey::Pubkey;
use setl8_lev_trading::instructions::InitConfigArgs;
use solana_signer::Signer;

use crate::ix::{self, Pay};
use crate::rehearsal::{sha256_hex, usd, Rehearsal};
use crate::state::ProgramRec;
use crate::vaultview;

/// The vault split of the product (6,500 bps to the pool; the harness and the vault's own tests use it).
pub const FEE_SPLIT_BPS: u16 = 6_500;
pub const MAX_PAYOUT_COUNT: u64 = 4;
/// `init_config` values of the rehearsal: the minimum pass delay (1 day) and the real window and cap (deploy-checklist.md).
pub const PASS_DELAY_SECS: u32 = 86_400;
pub const WINDOW_SECS: u32 = 86_400;
pub const MAX_PAYOUT_PER_WINDOW: u64 = 100_000_000_000;
/// Test tokens: what each trader starts with and what the vault pool is seeded with (6 decimals).
pub const TRADER_FUNDS: u64 = 100_000_000_000;
pub const POOL_SEED: u64 = 50_000_000_000;

impl Rehearsal {
    pub fn usdc(&self) -> Pubkey {
        self.keys.pubkey("mint-usdc")
    }
    pub fn usdt(&self) -> Pubkey {
        self.keys.pubkey("mint-usdt")
    }
    pub fn vault_state(&self) -> Pubkey {
        self.ids.vault_state(&self.keys.pubkey("vault-admin-sl8"), &self.keys.pubkey("vault-admin-rov"))
    }
    /// The fee wallet of this program's config (`sl8_wallet`): the keeper key.
    pub fn fee_wallet(&self) -> Pubkey {
        self.keys.pubkey("keeper")
    }

    /// The token side of `open_account` / `reset_account` for `trader_name` (USDC).
    pub fn pay_for(&self, trader_name: &str) -> Pay {
        let vs = self.vault_state();
        let usdc = self.usdc();
        Pay {
            vault_state: vs,
            trader_token: ix::ata(&self.keys.pubkey(trader_name), &usdc),
            mint: usdc,
            pool: self.ids.pool(&vs, &usdc),
            vault_sl8_token: ix::ata(&self.keys.pubkey("vault-admin-sl8"), &usdc),
            fee_token: ix::ata(&self.fee_wallet(), &usdc),
        }
    }

    /// (sha256 of the ELF, its length) of a deployed upgradeable program, read from its program-data account.
    pub fn program_hash(&self, program: &Pubkey) -> Result<(String, u64), String> {
        let acc = self.c.account(program).map_err(|e| e.to_string())?.ok_or_else(|| format!("program {program} is not deployed"))?;
        if !acc.executable || acc.data.len() < 36 {
            return Err(format!("{program} is not an upgradeable program"));
        }
        let pd: Pubkey = Pubkey::new_from_array(acc.data[4..36].try_into().map_err(|_| "program-data address".to_string())?);
        let d = self.c.account(&pd).map_err(|e| e.to_string())?.ok_or("no program-data account")?;
        if d.data.len() < 45 {
            return Err("program-data account too short".into());
        }
        let elf = &d.data[45..];
        Ok((sha256_hex(elf), elf.len() as u64))
    }

    // ------------------------------------------------------------------------------------------------ S0

    pub fn s0(&mut self) -> Result<(), String> {
        self.begin("S0");
        let r = self.s0_inner();
        if let Err(e) = r {
            self.fail_with(&e);
        }
        self.end("vault and this program deployed; the vault initialised; this program registered as a product with the launch tiers, ladder and fee split");
        Ok(())
    }

    fn s0_inner(&mut self) -> Result<(), String> {
        // the two programs are on the cluster; their hashes are recorded and compared with the builds
        for (name, id, so) in [("vault", self.ids.vault, self.vault_so.clone()), ("lev", self.ids.lev, self.lev_so.clone())] {
            let (sha, size) = self.program_hash(&id)?;
            let rec = ProgramRec { id: id.to_string(), sha256: sha.clone(), size };
            match name {
                "vault" => self.st.vault = rec,
                _ => self.st.lev = rec,
            }
            let want = so.as_ref().and_then(|p| std::fs::read(p).ok()).map(|b| sha256_hex(&b));
            match want {
                Some(w) => {
                    self.check(w == sha, &format!("{name} program {id}: deployed sha256 {sha} equals the build ({size} bytes)"));
                }
                None => {
                    self.say(&format!("  {name} program {id}: sha256 {sha}, {size} bytes (no build file given to compare)"));
                }
            }
        }
        self.save()?;

        // the mints (6 decimals, mint authority = keeper, freeze authority = a throwaway key) and the token accounts
        let rent_mint = self.c.min_rent(82).map_err(|e| e.to_string())?;
        for (n, label) in [("mint-usdc", "create_mint usdc"), ("mint-usdt", "create_mint usdt")] {
            let mint = self.keys.pubkey(n);
            if !self.exists(&mint) {
                let ixs = ix::create_mint(&self.keys.pubkey("keeper"), &mint, rent_mint, &self.keys.pubkey("keeper"), &self.keys.pubkey("freeze-authority"));
                self.send(label, &ixs, "keeper", &[n], false).map_err(|e| e.text)?;
            }
            self.st.addrs.insert(n.to_string(), mint.to_string());
        }
        let (usdc, usdt) = (self.usdc(), self.usdt());
        let owners: Vec<String> = ["vault-admin-sl8", "keeper", "trader1", "trader2", "trader3", "trader4", "trader5", "trader6", "trader7"].iter().map(|s| s.to_string()).collect();
        let mut wanted: Vec<(Pubkey, Pubkey)> = owners.iter().map(|o| (self.keys.pubkey(o), usdc)).collect();
        wanted.push((self.keys.pubkey("vault-admin-sl8"), usdt));
        let missing: Vec<(Pubkey, Pubkey)> = wanted.into_iter().filter(|(o, m)| !self.exists(&ix::ata(o, m))).collect();
        for chunk in missing.chunks(4) {
            let ixs: Vec<_> = chunk.iter().map(|(o, m)| ix::create_ata_idempotent(&self.keys.pubkey("keeper"), o, m)).collect();
            self.send("create_ata", &ixs, "keeper", &[], false).map_err(|e| e.text)?;
        }
        // test balances for the traders
        for n in ["trader1", "trader2", "trader3", "trader4", "trader5", "trader6", "trader7"] {
            let ta = ix::ata(&self.keys.pubkey(n), &usdc);
            if self.token_balance(&ta) < TRADER_FUNDS / 2 {
                self.send("mint_to trader", &[ix::mint_to(&usdc, &ta, &self.keys.pubkey("keeper"), TRADER_FUNDS)], "keeper", &[], false).map_err(|e| e.text)?;
            }
        }

        // the vault
        let vs = self.vault_state();
        if !self.exists(&vs) {
            let init = ix::init_vault(&self.ids, &self.keys.pubkey("vault-admin-sl8"), &self.keys.pubkey("vault-admin-rov"), &usdc, &usdt);
            self.send("init_vault", &[init], "keeper", &["vault-admin-sl8", "vault-admin-rov"], false).map_err(|e| e.text)?;
        }
        self.check(self.exists(&vs), "init_vault: the vault state exists");
        self.st.addrs.insert("vault_state".into(), vs.to_string());
        let pool = self.ids.pool(&vs, &usdc);
        self.st.addrs.insert("usdc_pool".into(), pool.to_string());

        // this program as a product, with the REAL launch values
        let tiers = self.launch.tiers();
        let ladder = self.launch.ladder();
        if !self.exists(&self.ids.registry()) {
            let reg = ix::register_product(&self.ids, &self.keys.pubkey("vault-admin-sl8"), &self.keys.pubkey("vault-admin-rov"), &tiers, FEE_SPLIT_BPS, MAX_PAYOUT_COUNT, &ladder);
            self.send("register_product", &[reg], "keeper", &["vault-admin-sl8", "vault-admin-rov"], false).map_err(|e| e.text)?;
        }
        let acc = self.c.account(&self.ids.registry()).map_err(|e| e.to_string())?.ok_or("no product registry")?;
        let reg = vaultview::parse_registry(&acc.data).ok_or("the product registry does not decode")?;
        self.check(reg.product_program_id == self.ids.lev, "register_product: the product is this program");
        self.check(reg.tiers == tiers && reg.tiers.len() == 10, &format!("register_product: the {} tiers equal config/launch.json (10 unique sizes)", reg.tiers.len()));
        self.check(reg.reset_price_bps == vec![10_000, 11_500, 12_000, 12_750, 13_000], "register_product: reset_price_bps [10000, 11500, 12000, 12750, 13000]");
        self.check(reg.max_payout_count == 4 && reg.fee_split_bps == FEE_SPLIT_BPS && reg.active, "register_product: max_payout_count 4, fee split 6500 bps, active");

        // liquidity for the payouts: our own test mint goes straight into the pool
        if self.token_balance(&pool) < POOL_SEED / 2 {
            self.send("mint_to pool", &[ix::mint_to(&usdc, &pool, &self.keys.pubkey("keeper"), POOL_SEED)], "keeper", &[], false).map_err(|e| e.text)?;
        }
        self.say(&format!("  pool holds {} test USDC", usd(self.token_balance(&pool))));
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------ S1

    pub fn s1(&mut self) -> Result<(), String> {
        self.begin("S1");
        let r = self.s1_inner();
        if let Err(e) = r {
            self.fail_with(&e);
        }
        self.end("init_config, 21 markets from config/launch.json in id order, rule set v1 (placeholder spreads and commissions: a mechanics rehearsal)");
        Ok(())
    }

    fn s1_inner(&mut self) -> Result<(), String> {
        let (a, b) = (self.keys.pubkey("lev-admin-sl8"), self.keys.pubkey("lev-admin-rov"));
        let keeper = self.keys.pubkey("keeper");
        if !self.exists(&self.ids.config()) {
            let args = InitConfigArgs {
                evaluator: self.keys.pubkey("evaluator"),
                vault_program: self.ids.vault,
                sl8_wallet: self.fee_wallet(),
                pass_delay_secs: self.pass_delay,
                max_payout_per_window: MAX_PAYOUT_PER_WINDOW,
                window_secs: WINDOW_SECS,
            };
            let ix = ix::init_config(&self.ids, &keeper, &a, &b, &args);
            self.send("init_config", &[ix], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false).map_err(|e| e.text)?;
        }
        let cfg = self.config()?;
        self.check(cfg.admin_a == a && cfg.admin_b == b, "init_config: the two admins are the compiled-in (throwaway) keys");
        self.check(cfg.evaluator == self.keys.pubkey("evaluator") && cfg.vault_program == self.ids.vault && cfg.sl8_wallet == self.fee_wallet(), "init_config: evaluator, vault program and fee wallet as given");
        self.check(cfg.pass_delay_secs == self.pass_delay && cfg.window_secs == WINDOW_SECS && cfg.max_payout_per_window == MAX_PAYOUT_PER_WINDOW, &format!("init_config: delay {} s, window 86,400 s, cap $100,000", self.pass_delay));

        let n = self.config()?.market_count;
        let todo: Vec<_> = self.launch.markets.iter().filter(|m| m.args.market_id >= n).map(|m| m.args.clone()).collect();
        for args in todo {
            let ix = ix::create_market(&self.ids, &keeper, &a, &b, &args);
            self.send(&format!("create_market {}", args.market_id), &[ix], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false).map_err(|e| e.text)?;
        }
        let cfg = self.config()?;
        // (the S3 attacks create a 22nd market later; a resumed run sees it)
        self.check(usize::from(cfg.market_count) >= self.launch.markets.len(), "create_market x21 in id order exist");
        if cfg.current_ruleset == 0 {
            let ix = ix::create_ruleset(&self.ids, &keeper, &a, &b, &self.launch.ruleset);
            self.send("create_ruleset", &[ix], "keeper", &["lev-admin-sl8", "lev-admin-rov"], false).map_err(|e| e.text)?;
        }
        let cfg = self.config()?;
        self.check(cfg.current_ruleset == 1, "create_ruleset v1 is current");
        // the chain is the file
        let mut same = true;
        for m in &self.launch.markets {
            let on = self.market(m.args.market_id)?;
            let p = &m.args.params;
            same &= (on.class, on.status, on.feed_id, on.max_age_secs, on.qty_step, on.qty_min, on.max_notional_usd, on.session_count, on.quote_conv_market)
                == (m.args.class, p.status, p.feed_id, p.max_age_secs, p.qty_step, p.qty_min, p.max_notional_usd, p.session_count, p.quote_conv_market);
            for k in 0..6 {
                same &= (on.sessions[k].start_min, on.sessions[k].end_min) == (p.sessions[k].start_min, p.sessions[k].end_min);
            }
        }
        self.check(same, "the 21 Market accounts equal config/launch.json field by field");
        let rs = self.ruleset(1)?;
        let l = &self.launch.ruleset;
        self.check(
            (rs.version, rs.fee_bps, rs.tier_count, rs.ladder_len, rs.leverage_x, rs.holding_fee_e6, rs.commission_e6, rs.tier_size, rs.tier_cost, rs.reset_ladder_bps)
                == (l.version, l.fee_bps, l.tier_count, l.ladder_len, l.leverage_x, l.holding_fee_e6, l.commission_e6, l.tier_size, l.tier_cost, l.reset_ladder_bps)
                && rs.spread_e6 == l.spread_e6,
            "the RuleSet account equals config/launch.json (spreads and commissions are placeholders)",
        );
        let _ = (self.keys.get("keeper").pubkey(),);
        Ok(())
    }
}
