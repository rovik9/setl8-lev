use anchor_lang::prelude::*;
use engine::{AccountState, ClaimError, CloseClaim, Rules};
use journal_types::{
    apply_batch, check_proof_without_price, check_signed_update_shape, in_session, price_e12_from_quantized, ChainError,
    DecodeError, JournalBatch, JournalEvent, PriceProof, ProofError, Side, NO_PROOF,
};
use solana_sha256_hasher::hashv;

use crate::constants::*;
use crate::errors::Setl8Error;
use crate::events::EventLog;
use crate::state::{Market, RuleSet, SectorConfig, TradingAccount};

/// Mock proofs (source 255) are accepted only by the localnet build (decision 46).
const ALLOW_MOCK: bool = cfg!(feature = "localnet");
/// A batch has at most 8 events, so at most 8 markets can be referenced.
const MAX_MARKET_ACCOUNTS: usize = 8;

/// DESIGN 6.3 `post_journal`. The referenced `Market` accounts are the remaining accounts.
#[event_cpi]
#[derive(Accounts)]
pub struct PostJournal<'info> {
    /// The evaluator key (also the fee payer).
    #[account(mut)]
    pub evaluator: Signer<'info>,
    #[account(
        mut,
        seeds = [ACCT_SEED, trading_account.trader.as_ref(), &trading_account.challenge_id.to_le_bytes()],
        bump = trading_account.bump
    )]
    pub trading_account: Box<Account<'info, TradingAccount>>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, SectorConfig>>,
    /// The rule set the account was bought under (its `spread_e6` gives the half-spread of each market).
    #[account(
        seeds = [RULESET_SEED, &trading_account.ruleset_version.to_le_bytes()],
        bump = ruleset.bump,
        constraint = ruleset.version == trading_account.ruleset_version @ Setl8Error::RuleSetMissing
    )]
    pub ruleset: Box<Account<'info, RuleSet>>,
}

fn err<T>(e: Setl8Error) -> Result<T> {
    Err(error!(e))
}

fn proof_err(e: ProofError) -> Setl8Error {
    match e {
        ProofError::WrongFeed => Setl8Error::ProofWrongFeed,
        ProofError::PriceMismatch => Setl8Error::EventPriceMismatch,
        ProofError::TooOld => Setl8Error::ProofTooOld,
        ProofError::InFuture => Setl8Error::ProofInFuture,
        ProofError::UnknownSource => Setl8Error::ProofUnknownSource,
        ProofError::MockNotAllowed => Setl8Error::ProofMockNotAllowed,
        ProofError::SignedUpdateTooLong | ProofError::BadSignedUpdate => Setl8Error::ProofBadSignedUpdate,
        ProofError::ValueNotPositive | ProofError::ValueTooLarge => Setl8Error::ProofValueInvalid,
    }
}

fn claim_err(e: ClaimError) -> Setl8Error {
    match e {
        ClaimError::Engine(_) => Setl8Error::MathOverflow,
        ClaimError::ZeroQuantity => Setl8Error::QuantityInvalid,
        ClaimError::ZeroPrice => Setl8Error::PriceInvalid,
        ClaimError::TimerAfterTime => Setl8Error::TimerInvalid,
        ClaimError::PnlMismatch => Setl8Error::PnlMismatch,
        ClaimError::QuickMismatch => Setl8Error::QuickMismatch,
        ClaimError::CreditMismatch => Setl8Error::CreditMismatch,
        ClaimError::TightAlreadyOn => Setl8Error::TightAlreadyOn,
        ClaimError::TightNotOn => Setl8Error::TightNotOn,
        ClaimError::TightStartPOpen => Setl8Error::TightStartPOpen,
        ClaimError::TightStartGPeak => Setl8Error::TightStartGPeak,
        ClaimError::TightStartR => Setl8Error::TightStartR,
        ClaimError::TightStartLStart => Setl8Error::TightStartLStart,
        ClaimError::TightStartLLevel => Setl8Error::TightStartLLevel,
        ClaimError::FloorNotRaised => Setl8Error::FloorNotRaised,
        ClaimError::FloorLLevel => Setl8Error::FloorLLevel,
        ClaimError::BreachLLevel => Setl8Error::BreachLLevel,
        ClaimError::BreachBalance => Setl8Error::BreachBalance,
        ClaimError::BreachNotBelowFloor => Setl8Error::BreachNotBelowFloor,
        ClaimError::BreachBelowStaticFloor => Setl8Error::BreachBelowStaticFloor,
    }
}

fn decode_err(bytes: &[u8], e: DecodeError) -> Setl8Error {
    match e {
        DecodeError::BadEventCount => {
            // the event count sits after first_seq (8) and prev_hash (32)
            let zero = bytes.get(40..44).is_some_and(|c| c == [0u8; 4]);
            if zero {
                Setl8Error::EmptyBatch
            } else {
                Setl8Error::BatchTooLarge
            }
        }
        DecodeError::BadProofCount => Setl8Error::BatchTooLarge,
        DecodeError::SignedUpdateTooLong => Setl8Error::ProofBadSignedUpdate,
        DecodeError::BadProofIndex => Setl8Error::ProofIndexOutOfRange,
        _ => Setl8Error::BatchMalformed,
    }
}

fn load_markets(program_id: &Pubkey, accounts: &[AccountInfo]) -> Result<Vec<Market>> {
    require!(accounts.len() <= MAX_MARKET_ACCOUNTS, Setl8Error::BatchTooLarge);
    let mut out = Vec::new();
    for info in accounts {
        require_keys_eq!(*info.owner, *program_id, Setl8Error::MarketUnknown);
        let data = info.try_borrow_data()?;
        let mut bytes: &[u8] = &data;
        let m = Market::try_deserialize(&mut bytes).map_err(|_| error!(Setl8Error::MarketUnknown))?;
        let expected = Pubkey::create_program_address(&[MARKET_SEED, &m.market_id.to_le_bytes(), &[m.bump]], program_id)
            .map_err(|_| error!(Setl8Error::MarketUnknown))?;
        require_keys_eq!(expected, *info.key, Setl8Error::MarketUnknown);
        out.push(m);
    }
    Ok(out)
}

/// Read-only inputs shared by every event of the batch.
struct Env<'a> {
    batch: &'a JournalBatch,
    markets: &'a [Market],
    ruleset: &'a RuleSet,
    rules: Rules,
}

/// The evolving state while a batch is applied.
struct Run {
    st: AccountState,
    last_time: i64,
    trades_closed: u32,
    /// `flat` as the batch's Meta will set it.
    final_flat: bool,
}

impl<'a> Env<'a> {
    fn market(&self, id: u16) -> Result<&'a Market> {
        match self.markets.iter().find(|m| m.market_id == id) {
            Some(m) => Ok(m),
            None => err(Setl8Error::MarketUnknown),
        }
    }

    /// The proof an event references, which decisive events must have.
    fn required_proof(&self, ev: &JournalEvent) -> Result<&'a PriceProof> {
        match ev.proof_ref() {
            Some(p) if p != NO_PROOF => match self.batch.proofs.get(usize::from(p)) {
                Some(proof) => Ok(proof),
                None => err(Setl8Error::ProofIndexOutOfRange),
            },
            _ => err(Setl8Error::ProofMissing),
        }
    }

    /// I-4 for a proof tied to a market: shape, feed, value, age. Returns the mid price (e12).
    fn check_market_proof(&self, proof: &PriceProof, m: &Market, time: i64) -> Result<u64> {
        check_signed_update_shape(proof).map_err(|e| error!(proof_err(e)))?;
        check_proof_without_price(proof, &m.feed_id, u32::from(m.max_age_secs), time, ALLOW_MOCK).map_err(|e| error!(proof_err(e)))?;
        price_e12_from_quantized(proof.quantized_value).map_err(|e| error!(proof_err(e)))
    }

    /// I-4 for events without a market field (tight start, floor raise, breach): the proof must carry the feed of one of
    /// the passed markets; only feed, value and age are checked.
    fn check_floating_proof(&self, proof: &PriceProof, time: i64) -> Result<()> {
        let Some(m) = self.markets.iter().find(|m| m.feed_id == proof.feed_id) else {
            return err(Setl8Error::ProofWrongFeed);
        };
        self.check_market_proof(proof, m, time).map(|_| ())
    }

    fn half_spread(&self, market_id: u16) -> Result<u32> {
        match self.ruleset.spread_e6.get(usize::from(market_id)) {
            Some(h) => Ok(u32::from(*h)),
            None => err(Setl8Error::MarketUnknown),
        }
    }

    /// Market status allows opening or adding (status 1, inside a session window).
    fn check_open_allowed(&self, m: &Market, time: i64) -> Result<()> {
        match m.status {
            1 => {}
            MARKET_STATUS_REDUCE_ONLY => return err(Setl8Error::MarketReduceOnly),
            _ => return err(Setl8Error::MarketDisabled),
        }
        let mut windows = [(0u16, 0u16); MAX_SESSIONS];
        for (w, s) in windows.iter_mut().zip(m.sessions.iter()) {
            *w = (s.start_min, s.end_min);
        }
        require!(in_session(&windows, m.session_count, time), Setl8Error::MarketClosed);
        Ok(())
    }
}

fn apply_slice(run: &mut Run, rules: &Rules, credit: i64, tally_delta: i64) -> Result<()> {
    let d = engine::apply_closed_slice(&run.st, rules, credit, tally_delta).map_err(|_| error!(Setl8Error::MathOverflow))?;
    // I-8: Q above the limit rejects the whole batch (the fail path is report_fail)
    require!(d.fail.is_none(), Setl8Error::QuickLimitExceeded);
    run.st = d.state;
    Ok(())
}

/// One event. Time (I-3) is checked by the caller.
fn apply_event(env: &Env, run: &mut Run, acct: &TradingAccount, ev: &JournalEvent) -> Result<()> {
    match ev {
        JournalEvent::Fill { market, side, qty, price_e12, time, .. } => {
            // every Fill increases exposure (DESIGN 8.1): not held, not locked, market open for opening
            require!(acct.held == 0, Setl8Error::AccountHeld);
            require!(engine::can_increase_exposure(&run.st, *time), Setl8Error::AccountLocked);
            let m = env.market(*market)?;
            env.check_open_allowed(m, *time)?;
            let proof = env.required_proof(ev)?;
            engine::validate_quantity(*qty, m.qty_step, m.qty_min).map_err(|_| error!(Setl8Error::QuantityInvalid))?;
            let mid = env.check_market_proof(proof, m, *time)?;
            let expected = engine::fill_price_e12(mid, env.half_spread(*market)?, *side == Side::Long).map_err(|_| error!(Setl8Error::MathOverflow))?;
            require!(*price_e12 != 0, Setl8Error::PriceInvalid);
            require!(*price_e12 == expected, Setl8Error::EventPriceMismatch);
        }
        JournalEvent::Close { market, side, qty, entry_e12, exit_e12, time, timer_start, pnl, quick, credited, .. } => {
            let m = env.market(*market)?;
            let proof = env.required_proof(ev)?;
            let mid = env.check_market_proof(proof, m, *time)?;
            let long = *side == Side::Long;
            let expected = engine::close_price_e12(mid, env.half_spread(*market)?, long).map_err(|_| error!(Setl8Error::MathOverflow))?;
            require!(*exit_e12 != 0, Setl8Error::PriceInvalid);
            require!(*exit_e12 == expected, Setl8Error::EventPriceMismatch);
            let credit = engine::check_close(&CloseClaim {
                long,
                qty: *qty,
                entry_e12: *entry_e12,
                exit_e12: *exit_e12,
                time: *time,
                timer_start: *timer_start,
                pnl: *pnl,
                quick: *quick,
                credited: *credited,
            })
            .map_err(|e| error!(claim_err(e)))?;
            apply_slice(run, &env.rules, credit.credit, credit.tally_delta)?;
            run.trades_closed = run.trades_closed.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
        }
        JournalEvent::Fee { market, amount, .. } => {
            env.market(*market)?;
            let amount = i64::try_from(*amount).map_err(|_| error!(Setl8Error::FeeInvalid))?;
            let debit = amount.checked_neg().ok_or(Setl8Error::MathOverflow)?;
            apply_slice(run, &env.rules, debit, 0)?;
        }
        JournalEvent::TightStart { time, p_open, r, l_start, g_peak, l_level, .. } => {
            env.check_floating_proof(env.required_proof(ev)?, *time)?;
            run.st = engine::check_tight_start(&run.st, *p_open, *r, *l_start, *g_peak, *l_level).map_err(|e| error!(claim_err(e)))?;
        }
        JournalEvent::FloorRaised { time, g_peak, l_level, .. } => {
            env.check_floating_proof(env.required_proof(ev)?, *time)?;
            run.st = engine::check_floor_raised(&run.st, *g_peak, *l_level).map_err(|e| error!(claim_err(e)))?;
        }
        JournalEvent::TightEnd { .. } => {
            require!(run.final_flat, Setl8Error::TightEndNotFlat);
            run.st = engine::check_tight_end(&run.st).map_err(|e| error!(claim_err(e)))?;
        }
        JournalEvent::Breach { time, l_level, b_after, .. } => {
            env.check_floating_proof(env.required_proof(ev)?, *time)?;
            require!(run.final_flat, Setl8Error::BreachNotFlat);
            run.st = engine::check_breach(&run.st, *time, *l_level, *b_after).map_err(|e| error!(claim_err(e)))?;
        }
        JournalEvent::Fail { .. } => return err(Setl8Error::FailEventNotAllowed),
        JournalEvent::Meta { flat, engine_version, .. } => {
            require!(*engine_version == engine::ENGINE_VERSION, Setl8Error::EngineVersionMismatch);
            run.st.flat = *flat;
        }
    }
    Ok(())
}

/// I-12: exactly one Meta, and it is the last event. Returns its `flat`.
fn meta_flat(batch: &JournalBatch) -> Result<bool> {
    let metas = batch.events.iter().filter(|e| matches!(e, JournalEvent::Meta { .. })).count();
    require!(metas != 0, Setl8Error::MetaMissing);
    require!(metas == 1, Setl8Error::MetaTwice);
    match batch.events.last() {
        Some(JournalEvent::Meta { flat, .. }) => Ok(*flat),
        _ => err(Setl8Error::MetaNotLast),
    }
}

pub fn handle_post_journal<'info>(ctx: Context<'_, '_, 'info, 'info, PostJournal<'info>>, batch_bytes: Vec<u8>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    // I-1
    require_keys_eq!(ctx.accounts.evaluator.key(), ctx.accounts.config.evaluator, Setl8Error::Unauthorized);
    let acct = &ctx.accounts.trading_account;
    require!(acct.status == STATUS_ACTIVE, Setl8Error::AccountNotActive);
    require!(acct.pass_pending != PASS_CONFIRMED, Setl8Error::PassPending);

    let batch = JournalBatch::decode(&batch_bytes).map_err(|e| error!(decode_err(&batch_bytes, e)))?;

    // I-2: sequence and previous hash (apply_batch does not compare prev_hash)
    let expected_seq = acct.seq.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
    require!(batch.first_seq == expected_seq, Setl8Error::BadSequence);
    require!(batch.prev_hash == acct.journal_hash, Setl8Error::BadPrevHash);

    let final_flat = meta_flat(&batch)?;
    let markets = load_markets(ctx.program_id, ctx.remaining_accounts)?;
    let st = acct.to_engine_state()?;
    let env = Env { batch: &batch, markets: &markets, ruleset: &ctx.accounts.ruleset, rules: Rules::default() };
    let mut run = Run { st, last_time: acct.last_batch_time, trades_closed: acct.trades_closed, final_flat };

    let (new_hash, last_seq) = apply_batch(&|parts: &[&[u8]]| hashv(parts).to_bytes(), &acct.journal_hash, batch.first_seq, &batch).map_err(|e| match e {
        ChainError::ZeroFirstSeq => error!(Setl8Error::BadSequence),
        ChainError::SeqOverflow => error!(Setl8Error::MathOverflow),
        ChainError::ProofMismatch => error!(Setl8Error::ProofMissing),
        ChainError::Encode(_) => error!(Setl8Error::BatchMalformed),
    })?;

    let account_key = acct.key();
    let latest_allowed = now.checked_add(EVENT_FUTURE_SKEW_SECS).ok_or(Setl8Error::MathOverflow)?;
    let mut seq = batch.first_seq;
    for (i, ev) in batch.events.iter().enumerate() {
        if i > 0 {
            seq = seq.checked_add(1).ok_or(Setl8Error::MathOverflow)?;
        }
        // I-3
        let t = ev.time();
        require!(t >= run.last_time, Setl8Error::TimeBackwards);
        require!(t <= latest_allowed, Setl8Error::TimeInFuture);
        run.last_time = t;

        apply_event(&env, &mut run, acct, ev)?;

        let proof_bytes = match ev.proof_index().and_then(|idx| batch.proofs.get(idx)) {
            Some(p) => p.to_bytes().map_err(|_| error!(Setl8Error::BatchMalformed))?,
            None => Vec::new(),
        };
        emit_cpi!(EventLog { account: account_key, seq, event_bytes: ev.to_bytes(), proof_bytes });
    }
    // a batch that ends flat cannot leave tight mode on
    require!(!(run.st.tight_on && run.st.flat), Setl8Error::TightNotEnded);
    debug_assert_eq!(seq, last_seq);

    let acct = &mut ctx.accounts.trading_account;
    acct.apply_engine_state(&run.st)?;
    acct.trades_closed = run.trades_closed;
    acct.last_batch_time = run.last_time;
    acct.seq = last_seq;
    acct.journal_hash = new_hash;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use journal_types::{check_proof_without_price, feed_id};

    #[test]
    fn mock_proofs_follow_the_localnet_feature() {
        assert_eq!(ALLOW_MOCK, cfg!(feature = "localnet"));
    }

    /// A source-255 proof is refused when mocks are not allowed, and the program reports it with its own code.
    #[test]
    fn a_mock_proof_is_refused_in_a_production_build() {
        let feed = feed_id(&|parts: &[&[u8]]| hashv(parts).to_bytes(), 255, "MOCK");
        let proof = PriceProof { source: 255, feed_id: feed, timestamp_ns: 1_000_000_000_000, quantized_value: 1_000_000_000_000, signed_update: vec![] };
        let e = check_proof_without_price(&proof, &feed, 60, 1_000, false).unwrap_err();
        assert_eq!(u32::from(proof_err(e)), u32::from(Setl8Error::ProofMockNotAllowed));
        assert!(check_proof_without_price(&proof, &feed, 60, 1_000, true).is_ok());
    }

    #[test]
    fn every_proof_and_claim_error_has_its_own_program_error() {
        use std::collections::HashSet;
        let proofs = [
            ProofError::WrongFeed,
            ProofError::PriceMismatch,
            ProofError::TooOld,
            ProofError::InFuture,
            ProofError::UnknownSource,
            ProofError::MockNotAllowed,
            ProofError::SignedUpdateTooLong,
            ProofError::ValueNotPositive,
        ];
        let set: HashSet<u32> = proofs.iter().map(|e| u32::from(proof_err(*e))).collect();
        // SignedUpdateTooLong shares ProofBadSignedUpdate with BadSignedUpdate; ValueNotPositive/ValueTooLarge share ProofValueInvalid
        assert_eq!(set.len(), proofs.len());
    }
}
