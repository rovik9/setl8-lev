//! Triage of claim arith-04 of the M12 independent review (program behaviour).
//! The test asserts the CORRECT behaviour and is ignored until the finding is fixed.
mod common;

use common::*;
use setl8_lev_trading::{errors::Setl8Error, instructions::RuleSetArgs};
use solana_signer::Signer;

type Mutation = fn(&mut RuleSetArgs);

fn create(mutate: impl FnOnce(&mut RuleSetArgs)) -> (Env, RuleSetArgs, litesvm::types::TransactionResult) {
    let mut e = Env::initialised();
    let mut args = launch_ruleset_args();
    mutate(&mut args);
    let admins = Admins::both(&e);
    let ix = create_ruleset_ix(e.payer.pubkey(), admins, &args);
    let r = e.send_admins(ix, admins);
    (e, args, r)
}

/// FINDING arith-04: a tier size the engine cannot use (`frac_exact` needs size x bps divisible by 10_000, so a multiple of 250
/// micro-USD, and `to_engine_state` needs it to fit an i64) must be refused by `create_ruleset`, because a rule set is immutable
/// and `open_account` would take the price for an account whose every later instruction fails.
#[test]
#[ignore = "FINDING arith-04: create_ruleset accepts tier sizes the engine cannot use (not a multiple of 250 micro-USD, or above i64::MAX)"]
fn arith_04_create_ruleset_refuses_unusable_tier_sizes() {
    let cases: [(&str, Mutation); 3] = [
        ("tier_size[0] = 2_500_000_001", |a| a.tier_size[0] = 2_500_000_001),
        ("tier_size[0] = 2_500_000_100 (multiple of 100, not of 250)", |a| a.tier_size[0] = 2_500_000_100),
        ("tier_size[9] = i64::MAX + 1", |a| a.tier_size[9] = (i64::MAX as u64) + 1),
    ];
    for (label, mutate) in cases {
        let (e, args, r) = create(mutate);
        println!("case: {label}");
        assert_err(&r, Setl8Error::InvalidTier);
        assert!(e.svm.get_account(&ruleset_pda(args.version)).is_none(), "{label}: the rule set was created");
    }
}
