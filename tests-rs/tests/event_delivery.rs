//! Audit finding F-27 (decision 67): every program event is delivered as an `emit_cpi!` self-CPI instruction (not a truncatable log
//! line), so a verifier or an indexer cannot lose it to log truncation. Two guards: the source has no `emit!(` left, and each admin
//! instruction's event is found in the inner instructions of a real transaction with the fields it carried before.
mod common;

use common::pass::cpi_events;
use common::*;
use setl8_lev_trading::events::*;
use solana_signer::Signer;

#[test]
fn no_instruction_uses_a_log_event_any_more() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../programs/setl8-lev-trading/src");
    let mut n = 0;
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                for (i, l) in text.lines().enumerate() {
                    let code = l.split("//").next().unwrap_or("");
                    assert!(!code.contains("emit!("), "{}:{} still uses emit! (a program log), use emit_cpi!", p.display(), i + 1);
                }
                n += 1;
            }
        }
    }
    assert!(n > 30, "the walk found only {n} source files");
}

#[test]
fn the_admin_instructions_deliver_their_events_as_cpi_events() {
    let mut e = Env::initialised();
    let new_eval = solana_pubkey::Pubkey::new_unique();
    let old = e.config().evaluator;
    // set_evaluator
    let ix = set_evaluator_ix(both(&e), new_eval);
    let m = e.send_as_admins(&[ix]).expect("set_evaluator");
    let ev = cpi_events::<EvaluatorChanged>(&m);
    assert_eq!((ev.len(), ev[0].old, ev[0].new), (1, old, new_eval));
    // set_limits
    let ix = set_limits_ix(both(&e), 200_000, 5_000_000_000, 3_600);
    let m = e.send_as_admins(&[ix]).expect("set_limits");
    let ev = cpi_events::<LimitsChanged>(&m);
    assert_eq!((ev.len(), ev[0].new_pass_delay_secs), (1, 200_000));
    // set_paused both ways
    let ix = set_paused_ix(only_sl8(&e), true);
    let m = e.send_as_admins(&[ix]).expect("pause");
    assert_eq!(cpi_events::<PausedChanged>(&m).iter().map(|x| x.paused).collect::<Vec<_>>(), vec![true]);
    let ix = set_paused_ix(both(&e), false);
    let m = e.send_as_admins(&[ix]).expect("unpause");
    assert_eq!(cpi_events::<PausedChanged>(&m).iter().map(|x| x.paused).collect::<Vec<_>>(), vec![false]);
    // the event authority is the program's PDA and a wrong one is refused (the macro's own constraint)
    assert_eq!(event_authority_pda(), solana_pubkey::Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0);
    let mut ix = set_paused_ix(only_sl8(&e), true);
    let last = ix.accounts.len() - 2;
    ix.accounts[last].pubkey = e.payer.pubkey();
    assert!(e.send_as_admins(&[ix]).is_err(), "a wrong event authority must be refused");
    assert_eq!(e.config().paused, 0);
}
