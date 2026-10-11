mod common;

use common::*;
use setl8_lev_trading::{
    constants as c,
    errors::Setl8Error,
    events::{MarketCreated, MarketFeedChanged, MarketUpdated},
    instructions::{CreateMarketArgs, MarketParams, UpdateMarketArgs},
    state::Session,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

fn create(e: &mut Env, args: &CreateMarketArgs) -> litesvm::types::TransactionResult {
    let admins = Admins::both(e);
    let ix = create_market_ix(e.payer.pubkey(), admins, args);
    e.send_admins(ix, admins)
}

fn update(e: &mut Env, admins: Admins, args: &UpdateMarketArgs) -> litesvm::types::TransactionResult {
    let ix = update_market_ix(admins, args);
    e.send_admins(ix, admins)
}

// -------------------------------------------------------------------- create_market

#[test]
fn create_market_ids_are_sequential_and_state_matches() {
    let mut e = Env::initialised();
    let a0 = create_market_args(0, 3, 0xA0);
    let r = create(&mut e, &a0);
    let logs = r.as_ref().expect("market 0 created").logs.clone();
    let created = events_of::<MarketCreated>(&logs);
    assert_eq!(created.len(), 1);
    assert_eq!((created[0].market_id, created[0].class, created[0].feed_id), (0, 3, [0xA0; 32]));
    assert_eq!(e.config().market_count, 1);

    assert_ok(create(&mut e, &create_market_args(1, 6, 0xA1)));
    assert_eq!(e.config().market_count, 2);

    let acc = e.svm.get_account(&market_pda(0)).unwrap();
    assert_eq!(acc.data.len(), 128);
    assert_eq!(acc.owner, PROGRAM_ID);
    let m = e.market(0);
    assert_eq!((m.market_id, m.class, m.status), (0, 3, 1));
    assert_eq!(m.feed_id, [0xA0; 32]);
    assert_eq!(m.max_age_secs, 60);
    assert_eq!((m.qty_step, m.qty_min, m.max_notional_usd), (1_000, 10_000, 500_000_000_000));
    assert_eq!(m.session_count, 2);
    assert_eq!(m.sessions[0], Session { start_min: 0, end_min: 7_200 });
    assert_eq!(m.sessions[1], Session { start_min: 7_500, end_min: 10_080 });
    assert_eq!(m.sessions[2..], [Session::default(); 4]);
    assert_eq!(m.quote_conv_market, c::NO_QUOTE_CONV);
    assert_eq!(m.reserved, [0u8; 30]);
    assert_eq!(e.market(1).class, 6);
}

#[test]
fn create_market_rejects_skipped_ids() {
    let mut e = Env::initialised();
    assert_err(&create(&mut e, &create_market_args(5, 0, 1)), Setl8Error::MarketUnknown);
    assert_err(&create(&mut e, &create_market_args(1, 0, 1)), Setl8Error::MarketUnknown);
    assert_eq!(e.config().market_count, 0);
    assert_ok(create(&mut e, &create_market_args(0, 0, 1)));
    // going back to an existing id is also refused (it is not the next id)
    assert!(create(&mut e, &create_market_args(0, 0, 2)).is_err());
    assert_err(&create(&mut e, &create_market_args(2, 0, 1)), Setl8Error::MarketUnknown);
}

#[test]
fn create_market_rejects_id_128_even_when_it_is_the_next_id() {
    let mut e = Env::initialised();
    assert_err(&create(&mut e, &create_market_args(128, 0, 1)), Setl8Error::MarketUnknown);
    for id in 0..128u16 {
        assert_ok(create(&mut e, &create_market_args(id, (id % 8) as u8, 1 + (id % 200) as u8)));
    }
    assert_eq!(e.config().market_count, 128);
    // market_count == 128 == args.market_id, so only the upper bound can refuse this one
    assert_err(&create(&mut e, &create_market_args(128, 0, 1)), Setl8Error::MarketUnknown);
    assert_eq!(e.config().market_count, 128);
}

fn expect_market_rejected(label: &str, id_prefill: u16, mutate: impl FnOnce(&mut CreateMarketArgs)) {
    let mut e = Env::initialised();
    for id in 0..id_prefill {
        assert_ok(create(&mut e, &create_market_args(id, 0, 1 + id as u8)));
    }
    let mut args = create_market_args(id_prefill, 2, 0x55);
    mutate(&mut args);
    println!("case: {label}");
    assert_err(&create(&mut e, &args), Setl8Error::MarketUnknown);
    assert_eq!(e.config().market_count, id_prefill, "{label}: market_count moved");
    assert!(e.svm.get_account(&market_pda(id_prefill)).is_none(), "{label}: account was created");
}

fn expect_market_ok(label: &str, id_prefill: u16, mutate: impl FnOnce(&mut CreateMarketArgs)) {
    let mut e = Env::initialised();
    for id in 0..id_prefill {
        assert_ok(create(&mut e, &create_market_args(id, 0, 1 + id as u8)));
    }
    let mut args = create_market_args(id_prefill, 2, 0x55);
    mutate(&mut args);
    println!("case: {label}");
    assert_ok(create(&mut e, &args));
    assert_eq!(e.config().market_count, id_prefill + 1);
}

#[test]
fn create_market_rejects_every_invalid_field() {
    expect_market_rejected("class 8", 0, |a| a.class = 8);
    expect_market_rejected("class 255", 0, |a| a.class = 255);
    expect_market_rejected("status 3", 0, |a| a.params.status = 3);
    expect_market_rejected("feed all zero", 0, |a| a.params.feed_id = [0; 32]);
    expect_market_rejected("max_age 0", 0, |a| a.params.max_age_secs = 0);
    expect_market_rejected("qty_step 0", 0, |a| a.params.qty_step = 0);
    expect_market_rejected("qty_min below qty_step", 0, |a| {
        a.params.qty_step = 1_000;
        a.params.qty_min = 999;
    });
    expect_market_rejected("max_notional 0", 0, |a| a.params.max_notional_usd = 0);
    expect_market_rejected("session_count 7", 0, |a| a.params.session_count = 7);
    expect_market_rejected("session start == end", 0, |a| a.params.sessions[0] = Session { start_min: 100, end_min: 100 });
    expect_market_rejected("session start > end", 0, |a| a.params.sessions[0] = Session { start_min: 200, end_min: 100 });
    expect_market_rejected("session end 10081", 0, |a| a.params.sessions[1] = Session { start_min: 7_500, end_min: 10_081 });
    expect_market_rejected("unused slot non-zero", 0, |a| a.params.sessions[2] = Session { start_min: 1, end_min: 2 });
    expect_market_rejected("unused slot half zero", 0, |a| a.params.sessions[5].end_min = 1);
    expect_market_rejected("quote_conv = itself", 1, |a| a.params.quote_conv_market = 1);
    expect_market_rejected("quote_conv = a later market", 1, |a| a.params.quote_conv_market = 2);
    expect_market_rejected("quote_conv = 0 with no markets", 0, |a| a.params.quote_conv_market = 0);
    expect_market_rejected("quote_conv = 127", 2, |a| a.params.quote_conv_market = 127);
}

#[test]
fn create_market_accepts_boundary_values() {
    expect_market_ok("session end exactly 10080", 0, |a| a.params.sessions[1].end_min = 10_080);
    expect_market_ok("six sessions", 0, |a| {
        a.params.session_count = 6;
        for i in 0..6u16 {
            a.params.sessions[i as usize] = Session { start_min: i * 1_000, end_min: i * 1_000 + 500 };
        }
    });
    expect_market_ok("no sessions", 0, |a| {
        a.params.session_count = 0;
        a.params.sessions = [Session::default(); 6];
    });
    expect_market_ok("qty_min == qty_step", 0, |a| a.params.qty_min = a.params.qty_step);
    expect_market_ok("status 0", 0, |a| a.params.status = 0);
    expect_market_ok("status 2", 0, |a| a.params.status = 2);
    expect_market_ok("class 7", 0, |a| a.class = 7);
    expect_market_ok("quote_conv = earlier market", 1, |a| a.params.quote_conv_market = 0);
    expect_market_ok("quote_conv = 0xFFFF", 1, |a| a.params.quote_conv_market = 0xFFFF);
}

#[test]
fn either_admin_alone_can_create_a_market() {
    let mut e = Env::initialised();
    let a = Admins::only_sl8(&e);
    let ix = create_market_ix(e.payer.pubkey(), a, &create_market_args(0, 0, 1));
    assert_ok(e.send_admins(ix, a));
    let a = Admins::only_rov(&e);
    let ix = create_market_ix(e.payer.pubkey(), a, &create_market_args(1, 0, 2));
    assert_ok(e.send_admins(ix, a));
    assert_eq!(e.config().market_count, 2);
}

#[test]
fn create_market_fails_with_no_admin_signer() {
    let mut e = Env::initialised();
    let a = Admins::neither(&e);
    let ix = create_market_ix(e.payer.pubkey(), a, &create_market_args(0, 0, 1));
    assert_err(&e.send_admins(ix, a), Setl8Error::Unauthorized);
    assert_eq!(e.config().market_count, 0);
}

#[test]
fn create_market_fails_for_non_admins() {
    let mut e = Env::initialised();
    let (x, y) = (Keypair::new(), Keypair::new());
    let ix = create_market_ix(e.payer.pubkey(), Admins::strangers(&x, &y), &create_market_args(0, 0, 1));
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
    // one real admin NOT signing, a stranger signing in the other slot: still no admin signature
    let sl8 = e.sl8.pubkey();
    let admins = Admins { a: sl8, b: x.pubkey(), a_signs: false, b_signs: true };
    let ix = create_market_ix(e.payer.pubkey(), admins, &create_market_args(0, 0, 1));
    assert_err(&e.send(&[ix], &[&x]), Setl8Error::Unauthorized);
    assert_eq!(e.config().market_count, 0);
}

// -------------------------------------------------------------------- update_market

fn changed_params() -> MarketParams {
    let mut p = market_params(0xB0);
    p.status = 2;
    p.max_age_secs = 120;
    p.qty_step = 500;
    p.qty_min = 5_000;
    p.max_notional_usd = 7_000_000_000;
    p.session_count = 1;
    p.sessions = [Session::default(); 6];
    p.sessions[0] = Session { start_min: 60, end_min: 600 };
    p
}

fn env_with_markets(n: u16) -> Env {
    let mut e = Env::initialised();
    for id in 0..n {
        e.create_market_ok(&create_market_args(id, (id % 8) as u8, 0xA0 + id as u8));
    }
    e
}

#[test]
fn update_market_changes_persist_and_keep_id_class_bump() {
    let mut e = env_with_markets(2);
    let before = e.svm.get_account(&market_pda(1)).unwrap();
    let mut p = changed_params();
    p.quote_conv_market = 0;
    let admins = Admins::both(&e);
    assert_ok(update(&mut e, admins, &UpdateMarketArgs { market_id: 1, params: p.clone() }));

    let m = e.market(1);
    assert_eq!((m.status, m.feed_id, m.max_age_secs), (2, [0xB0; 32], 120));
    assert_eq!((m.qty_step, m.qty_min, m.max_notional_usd), (500, 5_000, 7_000_000_000));
    assert_eq!(m.session_count, 1);
    assert_eq!(m.sessions[0], Session { start_min: 60, end_min: 600 });
    assert_eq!(m.sessions[1..], [Session::default(); 5]);
    assert_eq!(m.quote_conv_market, 0);

    // id (8..10), class (10), bump (97) and reserved (98..128) are byte-identical
    let after = e.svm.get_account(&market_pda(1)).unwrap();
    assert_eq!(after.data.len(), 128);
    assert_eq!(&after.data[..11], &before.data[..11]);
    assert_eq!(&after.data[97..], &before.data[97..]);
    assert_eq!(e.market(1).class, 1);
    // the other market is untouched
    assert_eq!(e.market(0).feed_id, [0xA0; 32]);
}

#[test]
fn update_market_with_the_same_values_leaves_the_account_identical() {
    let mut e = env_with_markets(1);
    let before = e.svm.get_account(&market_pda(0)).unwrap();
    let admins = Admins::both(&e);
    assert_ok(update(&mut e, admins, &UpdateMarketArgs { market_id: 0, params: market_params(0xA0) }));
    assert_eq!(e.svm.get_account(&market_pda(0)).unwrap().data, before.data);
}

#[test]
fn a_feed_change_emits_market_feed_changed_with_old_and_new_feed() {
    let mut e = env_with_markets(1);
    e.svm.warp_to_slot(777);
    let admins = Admins::both(&e);
    let r = update(&mut e, admins, &UpdateMarketArgs { market_id: 0, params: market_params(0xC7) });
    let logs = r.expect("update succeeds").logs;
    let ev = events_of::<MarketFeedChanged>(&logs);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].market_id, 0);
    assert_eq!(ev[0].old_feed, [0xA0; 32]);
    assert_eq!(ev[0].new_feed, [0xC7; 32]);
    assert_eq!(ev[0].slot, 777);
    assert_eq!(events_of::<MarketUpdated>(&logs).len(), 1);
    assert_eq!(e.market(0).feed_id, [0xC7; 32]);
}

#[test]
fn a_non_feed_change_emits_market_updated_but_not_feed_changed() {
    let mut e = env_with_markets(1);
    let mut p = market_params(0xA0); // same feed as created
    p.max_age_secs = 90;
    let admins = Admins::both(&e);
    let logs = update(&mut e, admins, &UpdateMarketArgs { market_id: 0, params: p }).expect("update succeeds").logs;
    assert!(events_of::<MarketFeedChanged>(&logs).is_empty());
    assert_eq!(events_of::<MarketUpdated>(&logs).len(), 1);
    assert_eq!(events_of::<MarketUpdated>(&logs)[0].market_id, 0);
    assert_eq!(e.market(0).max_age_secs, 90);
}

#[test]
fn either_admin_can_update_a_market() {
    let mut e = env_with_markets(1);
    let a = Admins::only_sl8(&e);
    assert_ok(update(&mut e, a, &UpdateMarketArgs { market_id: 0, params: market_params(0xD1) }));
    assert_eq!(e.market(0).feed_id, [0xD1; 32]);
    let a = Admins::only_rov(&e);
    assert_ok(update(&mut e, a, &UpdateMarketArgs { market_id: 0, params: market_params(0xD2) }));
    assert_eq!(e.market(0).feed_id, [0xD2; 32]);
}

#[test]
fn non_admins_cannot_update_a_market() {
    let mut e = env_with_markets(1);
    let before = e.svm.get_account(&market_pda(0)).unwrap();
    let args = UpdateMarketArgs { market_id: 0, params: market_params(0xEE) };
    let (x, y) = (Keypair::new(), Keypair::new());
    let ix = update_market_ix(Admins::strangers(&x, &y), &args);
    assert_err(&e.send(&[ix], &[&x, &y]), Setl8Error::Unauthorized);
    let a = Admins::neither(&e);
    assert_err(&update(&mut e, a, &args), Setl8Error::Unauthorized);
    assert_eq!(e.svm.get_account(&market_pda(0)).unwrap(), before);
}

#[test]
fn update_market_applies_the_same_validations() {
    let mut e = env_with_markets(3);
    let before = e.svm.get_account(&market_pda(1)).unwrap();
    let admins = Admins::both(&e);
    type Case = (&'static str, Box<dyn Fn(&mut MarketParams)>);
    let cases: Vec<Case> = vec![
        ("status 3", Box::new(|p| p.status = 3)),
        ("feed zero", Box::new(|p| p.feed_id = [0; 32])),
        ("max_age 0", Box::new(|p| p.max_age_secs = 0)),
        ("qty_step 0", Box::new(|p| p.qty_step = 0)),
        ("qty_min < qty_step", Box::new(|p| p.qty_min = p.qty_step - 1)),
        ("max_notional 0", Box::new(|p| p.max_notional_usd = 0)),
        ("session_count 7", Box::new(|p| p.session_count = 7)),
        ("session end 10081", Box::new(|p| p.sessions[0].end_min = 10_081)),
        ("session start == end", Box::new(|p| p.sessions[0] = Session { start_min: 5, end_min: 5 })),
        ("unused slot non-zero", Box::new(|p| p.sessions[4] = Session { start_min: 1, end_min: 2 })),
        ("quote_conv = itself", Box::new(|p| p.quote_conv_market = 1)),
        ("quote_conv = later existing market", Box::new(|p| p.quote_conv_market = 2)),
        ("quote_conv = nonexistent", Box::new(|p| p.quote_conv_market = 3)),
    ];
    for (label, f) in cases {
        let mut p = market_params(0xA1);
        f(&mut p);
        println!("case: {label}");
        assert_err(&update(&mut e, admins, &UpdateMarketArgs { market_id: 1, params: p }), Setl8Error::MarketUnknown);
    }
    assert_eq!(e.svm.get_account(&market_pda(1)).unwrap(), before);
    // an earlier market as conversion market is fine
    let mut p = market_params(0xA1);
    p.quote_conv_market = 0;
    assert_ok(update(&mut e, admins, &UpdateMarketArgs { market_id: 1, params: p }));
}

#[test]
fn update_market_fails_for_a_market_that_does_not_exist() {
    let mut e = env_with_markets(1);
    let a = Admins::both(&e);
    let r = update(&mut e, a, &UpdateMarketArgs { market_id: 1, params: market_params(1) });
    assert!(r.is_err());
    assert!(e.svm.get_account(&market_pda(1)).is_none());
}
