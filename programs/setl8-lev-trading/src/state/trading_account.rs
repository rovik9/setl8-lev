use anchor_lang::prelude::*;
use engine::{AccountState, EngineError};

use crate::constants::{PASS_NONE, STATUS_ACTIVE};
use crate::errors::Setl8Error;

/// DESIGN 5.1. PDA `["acct", trader, challenge_id u64 LE]`; 207 bytes including the discriminator, plain Borsh,
/// fields in this exact order. Flags are `u8` and must be 0 or 1 when read (`flag`).
#[account]
pub struct TradingAccount {
    pub trader: Pubkey,
    pub challenge_id: u64,
    /// S in micro-USD.
    pub size_s: u64,
    pub created_at: i64,
    /// Closed balance B, maintained by the program from the journal.
    pub b: i64,
    /// Credited quick profit this phase.
    pub q_tally: i64,
    /// Unix seconds, 0 = none (set by `Breach`).
    pub lock_until: i64,
    pub last_batch_time: i64,
    pub last_activity_pushed: i64,
    /// Mirrors the vault's `payout_count + 1`.
    pub next_request_id: u64,
    pub seq: u64,
    pub journal_hash: [u8; 32],
    pub trades_closed: u32,
    pub ruleset_version: u16,
    /// 0 Active, 1 Failed, 2 Abandoned, 3 Graduated.
    pub status: u8,
    /// 1 to 5.
    pub phase: u8,
    pub latch: u8,
    pub flat: u8,
    /// 0 none, 1 requested, 2 confirmed.
    pub pass_pending: u8,
    pub vetoes: u8,
    pub bump: u8,
    pub tight_on: u8,
    pub held: u8,
    pub r: i64,
    pub g_peak: i64,
    pub l_level: i64,
    pub pass_requested_at: i64,
    pub pass_proposed_at: i64,
}

/// The parts of a brand-new account that differ between `open_account` (phase 1, first payout) and `reset_account` (the
/// failed phase, the payouts the vault carried over). Everything else of a fresh account is the same for both.
pub struct FreshAccount {
    pub trader: Pubkey,
    pub challenge_id: u64,
    pub size_s: u64,
    pub now: i64,
    pub genesis_hash: [u8; 32],
    pub ruleset_version: u16,
    pub phase: u8,
    /// The vault's `payout_count + 1` of this challenge (1 for a purchase).
    pub next_request_id: u64,
    pub bump: u8,
}

impl TradingAccount {
    /// Writes a fresh account (`init`ed, all zero): B = S, nothing open, tight off, floors as at the start of a phase,
    /// `seq` 0 with the genesis hash, status Active, nothing pending, not held.
    pub fn init_fresh(&mut self, f: &FreshAccount) -> Result<()> {
        let size_i = i64::try_from(f.size_s).map_err(|_| error!(Setl8Error::MathOverflow))?;
        self.trader = f.trader;
        self.challenge_id = f.challenge_id;
        self.size_s = f.size_s;
        self.created_at = f.now;
        self.b = size_i;
        self.q_tally = 0;
        self.lock_until = 0;
        self.last_batch_time = f.now;
        self.last_activity_pushed = f.now;
        self.next_request_id = f.next_request_id;
        self.seq = 0;
        self.journal_hash = f.genesis_hash;
        self.trades_closed = 0;
        self.ruleset_version = f.ruleset_version;
        self.status = STATUS_ACTIVE;
        self.phase = f.phase;
        self.latch = 0;
        self.flat = 1;
        self.pass_pending = PASS_NONE;
        self.vetoes = 0;
        self.bump = f.bump;
        self.tight_on = 0;
        self.held = 0;
        self.r = 0;
        self.g_peak = 0;
        self.l_level = 0;
        self.pass_requested_at = 0;
        self.pass_proposed_at = 0;
        Ok(())
    }

    /// `reset_at`: when a Failed account was reset by `reset_account` (0 = not reset). Stored in `pass_requested_at`, which
    /// `end_account` zeroes and which nothing reads once the account is terminal, exactly like `terminal_at` in
    /// `pass_proposed_at`. A second reset of the same record is refused locally before any CPI.
    pub fn reset_at(&self) -> i64 {
        self.pass_requested_at
    }

    /// `terminal_at`: when the account became Failed, Abandoned or Graduated (M7, RESOLVE (b)). Stored in `pass_proposed_at`,
    /// which is meaningless once nothing can be pending; every transition into a terminal state sets it (`end_account`).
    pub fn terminal_at(&self) -> i64 {
        self.pass_proposed_at
    }

    /// 32+8x10 +32 +4+2 +9 +5x8 bytes after the discriminator (DESIGN 5.1).
    pub const BODY_LEN: usize = 32 + 8 * 10 + 32 + 4 + 2 + 9 + 8 * 5;
    /// Total account size including the discriminator: 207.
    pub const LEN: usize = 8 + Self::BODY_LEN;
}

const _: () = assert!(TradingAccount::LEN == 207);

/// A stored flag byte: 0 or 1, anything else is `InvalidFlag`.
pub fn flag(v: u8) -> Result<bool> {
    match v {
        0 => Ok(false),
        1 => Ok(true),
        _ => err!(Setl8Error::InvalidFlag),
    }
}

/// `F_s`: 0.92 S, or 0.96 S once latched (`latch` is a stored flag byte).
pub fn static_floor_micro(size_s: u64, latch: u8) -> Result<i64> {
    let size = i64::try_from(size_s).map_err(|_| error!(Setl8Error::MathOverflow))?;
    let st = AccountState { latch: flag(latch)?, ..AccountState::new_phase(size) };
    engine::static_floor_micro(&st).map_err(engine_err)
}

fn engine_err(e: EngineError) -> anchor_lang::error::Error {
    match e {
        EngineError::Overflow => error!(Setl8Error::MathOverflow),
        _ => error!(Setl8Error::EngineStateMismatch),
    }
}

impl TradingAccount {
    /// The engine's view of this account. `l_start` is not stored (DESIGN 5.1); while tight mode is on it is
    /// `l_level - max(0, g_peak - 0.01 S)` by the engine's own formula. Flags other than 0/1 are rejected.
    pub fn to_engine_state(&self) -> Result<AccountState> {
        let size = i64::try_from(self.size_s).map_err(|_| error!(Setl8Error::MathOverflow))?;
        let tight_on = flag(self.tight_on)?;
        let l_start = if tight_on {
            let slack = engine::frac_exact(size, engine::consts::RATCHET_SLACK_BPS).map_err(engine_err)?;
            let extra = self.g_peak.checked_sub(slack).ok_or(Setl8Error::MathOverflow)?.max(0);
            self.l_level.checked_sub(extra).ok_or(Setl8Error::MathOverflow)?
        } else {
            0
        };
        Ok(AccountState {
            size_micro: size,
            b: self.b,
            latch: flag(self.latch)?,
            tight_on,
            r: self.r,
            l_start,
            g_peak: self.g_peak,
            l_level: self.l_level,
            q_tally: self.q_tally,
            lock_until: self.lock_until,
            flat: flag(self.flat)?,
        })
    }

    /// Writes an engine state back. The size must be unchanged, and while tight mode is on the engine's `l_start`
    /// must equal the one the stored fields imply (otherwise the 207-byte layout could not reproduce the state).
    pub fn apply_engine_state(&mut self, st: &AccountState) -> Result<()> {
        let size = i64::try_from(self.size_s).map_err(|_| error!(Setl8Error::MathOverflow))?;
        require!(st.size_micro == size, Setl8Error::EngineStateMismatch);
        let (b, latch, tight_on, r, g_peak, l_level, q_tally, lock_until, flat) =
            (st.b, u8::from(st.latch), u8::from(st.tight_on), st.r, st.g_peak, st.l_level, st.q_tally, st.lock_until, u8::from(st.flat));
        let before = (self.b, self.latch, self.tight_on, self.r, self.g_peak, self.l_level, self.q_tally, self.lock_until, self.flat);
        self.b = b;
        self.latch = latch;
        self.tight_on = tight_on;
        self.r = r;
        self.g_peak = g_peak;
        self.l_level = l_level;
        self.q_tally = q_tally;
        self.lock_until = lock_until;
        self.flat = flat;
        let implied = self.to_engine_state();
        if implied.map_or(true, |s| s.l_start != st.l_start) {
            // restore the previous values: a rejected state must not leave a half-written account
            (self.b, self.latch, self.tight_on, self.r, self.g_peak, self.l_level, self.q_tally, self.lock_until, self.flat) = before;
            return err!(Setl8Error::EngineStateMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S100K: u64 = 100_000_000_000;

    fn full() -> TradingAccount {
        TradingAccount {
            trader: Pubkey::new_from_array([0xA1; 32]),
            challenge_id: 0x0102030405060708,
            size_s: S100K,
            created_at: 1_700_000_001,
            b: S100K as i64,
            q_tally: 11,
            lock_until: 12,
            last_batch_time: 1_700_000_002,
            last_activity_pushed: 1_700_000_003,
            next_request_id: 1,
            seq: 0x1112131415161718,
            journal_hash: [0xB2; 32],
            trades_closed: 0x21222324,
            ruleset_version: 0x3132,
            status: 0,
            phase: 1,
            latch: 0,
            flat: 1,
            pass_pending: 0,
            vetoes: 0,
            bump: 0xFE,
            tight_on: 0,
            held: 0,
            r: 41,
            g_peak: 42,
            l_level: 43,
            pass_requested_at: 44,
            pass_proposed_at: 45,
        }
    }

    /// Every field's byte offset and width against the DESIGN 5.1 table (offsets include the 8-byte discriminator).
    #[test]
    fn layout_offsets_match_design_5_1() {
        let a = full();
        let mut d = Vec::new();
        a.try_serialize(&mut d).unwrap();
        assert_eq!(d.len(), 207);
        assert_eq!(TradingAccount::LEN, 207);
        let table: [(&str, usize, usize); 30] = [
            ("trader", 8, 32), ("challenge_id", 40, 8), ("size_s", 48, 8), ("created_at", 56, 8), ("b", 64, 8), ("q_tally", 72, 8),
            ("lock_until", 80, 8), ("last_batch_time", 88, 8), ("last_activity_pushed", 96, 8), ("next_request_id", 104, 8), ("seq", 112, 8),
            ("journal_hash", 120, 32), ("trades_closed", 152, 4), ("ruleset_version", 156, 2), ("status", 158, 1), ("phase", 159, 1),
            ("latch", 160, 1), ("flat", 161, 1), ("pass_pending", 162, 1), ("vetoes", 163, 1), ("bump", 164, 1), ("tight_on", 165, 1),
            ("held", 166, 1), ("r", 167, 8), ("g_peak", 175, 8), ("l_level", 183, 8), ("pass_requested_at", 191, 8), ("pass_proposed_at", 199, 8),
            ("discriminator", 0, 8), ("end", 207, 0),
        ];
        // contiguous: each field starts where the previous one ended
        let mut at = 8;
        for (name, off, w) in table.iter().take(28) {
            assert_eq!(*off, at, "{name} offset");
            at += w;
        }
        assert_eq!(at, 207);
        let le = |off: usize, w: usize| -> Vec<u8> { d[off..off + w].to_vec() };
        assert_eq!(le(8, 32), vec![0xA1; 32]);
        assert_eq!(le(40, 8), 0x0102030405060708u64.to_le_bytes());
        assert_eq!(le(48, 8), S100K.to_le_bytes());
        assert_eq!(le(56, 8), 1_700_000_001i64.to_le_bytes());
        assert_eq!(le(64, 8), (S100K as i64).to_le_bytes());
        assert_eq!(le(72, 8), 11i64.to_le_bytes());
        assert_eq!(le(80, 8), 12i64.to_le_bytes());
        assert_eq!(le(88, 8), 1_700_000_002i64.to_le_bytes());
        assert_eq!(le(96, 8), 1_700_000_003i64.to_le_bytes());
        assert_eq!(le(104, 8), 1u64.to_le_bytes());
        assert_eq!(le(112, 8), 0x1112131415161718u64.to_le_bytes());
        assert_eq!(le(120, 32), vec![0xB2; 32]);
        assert_eq!(le(152, 4), 0x21222324u32.to_le_bytes());
        assert_eq!(le(156, 2), 0x3132u16.to_le_bytes());
        assert_eq!((d[158], d[159], d[160], d[161], d[162], d[163], d[164], d[165], d[166]), (0, 1, 0, 1, 0, 0, 0xFE, 0, 0));
        assert_eq!(le(167, 8), 41i64.to_le_bytes());
        assert_eq!(le(175, 8), 42i64.to_le_bytes());
        assert_eq!(le(183, 8), 43i64.to_le_bytes());
        assert_eq!(le(191, 8), 44i64.to_le_bytes());
        assert_eq!(le(199, 8), 45i64.to_le_bytes());
    }

    fn zeroed() -> TradingAccount {
        let mut d = vec![0u8; TradingAccount::LEN];
        d[..8].copy_from_slice(<TradingAccount as anchor_lang::Discriminator>::DISCRIMINATOR);
        TradingAccount::try_deserialize(&mut d.as_slice()).unwrap()
    }

    /// `init_fresh` writes every field: a dirty account (all fields set) comes out as exactly the fresh account of the arguments.
    #[test]
    fn init_fresh_writes_every_field_of_a_fresh_account() {
        let f = FreshAccount {
            trader: Pubkey::new_from_array([7; 32]),
            challenge_id: 9,
            size_s: S100K,
            now: 1_700_000_123,
            genesis_hash: [0xEE; 32],
            ruleset_version: 3,
            phase: 4,
            next_request_id: 3,
            bump: 251,
        };
        let mut a = full();
        a.status = 1;
        a.held = 1;
        a.latch = 1;
        a.tight_on = 1;
        a.pass_pending = 2;
        a.vetoes = 6;
        a.flat = 0;
        a.init_fresh(&f).unwrap();
        let want = TradingAccount {
            trader: f.trader,
            challenge_id: 9,
            size_s: S100K,
            created_at: f.now,
            b: S100K as i64,
            q_tally: 0,
            lock_until: 0,
            last_batch_time: f.now,
            last_activity_pushed: f.now,
            next_request_id: 3,
            seq: 0,
            journal_hash: [0xEE; 32],
            trades_closed: 0,
            ruleset_version: 3,
            status: 0,
            phase: 4,
            latch: 0,
            flat: 1,
            pass_pending: 0,
            vetoes: 0,
            bump: 251,
            tight_on: 0,
            held: 0,
            r: 0,
            g_peak: 0,
            l_level: 0,
            pass_requested_at: 0,
            pass_proposed_at: 0,
        };
        let (mut got, mut exp) = (Vec::new(), Vec::new());
        a.try_serialize(&mut got).unwrap();
        want.try_serialize(&mut exp).unwrap();
        assert_eq!(got, exp);
        // a size that does not fit i64 is refused
        let mut z = zeroed();
        assert!(z.init_fresh(&FreshAccount { size_s: u64::MAX, ..f }).is_err());
    }

    #[test]
    fn reset_at_is_the_pass_requested_at_field() {
        let mut a = full();
        assert_eq!(a.reset_at(), 44);
        a.pass_requested_at = 0;
        assert_eq!(a.reset_at(), 0);
    }

    #[test]
    fn round_trip_and_rent() {
        let a = full();
        let mut d = Vec::new();
        a.try_serialize(&mut d).unwrap();
        let back = TradingAccount::try_deserialize(&mut d.as_slice()).unwrap();
        let mut d2 = Vec::new();
        back.try_serialize(&mut d2).unwrap();
        assert_eq!(d, d2);
        assert_eq!(back.trader, a.trader);
        assert_eq!(back.seq, a.seq);
        // rent = (128 + 207) x 6960 lamports (DESIGN 5)
        assert_eq!((128 + TradingAccount::LEN as u64) * 6960, 2_331_600);
    }

    #[test]
    fn engine_state_round_trip_when_tight_mode_is_off() {
        let a = full();
        let st = a.to_engine_state().unwrap();
        assert_eq!((st.size_micro, st.b, st.latch, st.tight_on, st.l_start, st.flat), (S100K as i64, S100K as i64, false, false, 0, true));
        let mut b = a.clone_for_test();
        b.apply_engine_state(&st).unwrap();
        assert_eq!(b.b, a.b);
        assert_eq!((b.latch, b.tight_on, b.flat), (0, 0, 1));
    }

    #[test]
    fn engine_state_round_trip_in_tight_mode_derives_l_start() {
        // R 100k, l_start 96k, g_peak 3.0k: L = 96k + (3.0k - 1.0k) = 98k  =>  l_start = 98k - 2k = 96k
        let mut a = full();
        a.tight_on = 1;
        a.r = 100_000_000_000;
        a.g_peak = 3_000_000_000;
        a.l_level = 98_000_000_000;
        let st = a.to_engine_state().unwrap();
        assert_eq!(st.l_start, 96_000_000_000);
        // g_peak under the 0.01 S slack: l_start == l_level
        a.g_peak = 500_000_000;
        a.l_level = 96_200_000_000;
        assert_eq!(a.to_engine_state().unwrap().l_start, 96_200_000_000);
        // writing back an engine state whose l_start the fields cannot reproduce is refused and changes nothing
        let mut st2 = a.to_engine_state().unwrap();
        st2.l_start += 1;
        let before = (a.b, a.l_level);
        assert!(a.apply_engine_state(&st2).is_err());
        assert_eq!((a.b, a.l_level), before);
        // size mismatch is refused
        let mut st3 = a.to_engine_state().unwrap();
        st3.size_micro += 1_000_000;
        assert!(a.apply_engine_state(&st3).is_err());
    }

    #[test]
    fn flag_bytes_other_than_0_or_1_are_rejected() {
        assert!(!flag(0).unwrap());
        assert!(flag(1).unwrap());
        for v in 2..=255u8 {
            assert!(flag(v).is_err(), "{v}");
            for which in 0..3 {
                let mut a = full();
                match which {
                    0 => a.latch = v,
                    1 => a.tight_on = v,
                    _ => a.flat = v,
                }
                assert!(a.to_engine_state().is_err(), "field {which} value {v}");
            }
            assert!(static_floor_micro(S100K, v).is_err());
        }
    }

    #[test]
    fn static_floor_for_latch_zero_and_one() {
        assert_eq!(static_floor_micro(S100K, 0).unwrap(), 92_000_000_000);
        assert_eq!(static_floor_micro(S100K, 1).unwrap(), 96_000_000_000);
        assert_eq!(static_floor_micro(2_500_000_000, 0).unwrap(), 2_300_000_000);
        assert_eq!(static_floor_micro(2_500_000_000, 1).unwrap(), 2_400_000_000);
        assert!(static_floor_micro(u64::MAX, 0).is_err());
        assert!(static_floor_micro(0, 0).is_err());
    }

    #[test]
    fn apply_engine_state_writes_every_engine_field() {
        let mut a = full();
        let mut st = a.to_engine_state().unwrap();
        st.b = 104_000_000_000;
        st.latch = true;
        st.q_tally = 500_000;
        st.lock_until = 77;
        st.flat = false;
        a.apply_engine_state(&st).unwrap();
        assert_eq!((a.b, a.latch, a.q_tally, a.lock_until, a.flat), (104_000_000_000, 1, 500_000, 77, 0));
        assert_eq!(a.to_engine_state().unwrap(), st);
    }

    impl TradingAccount {
        fn clone_for_test(&self) -> TradingAccount {
            let mut d = Vec::new();
            self.try_serialize(&mut d).unwrap();
            TradingAccount::try_deserialize(&mut d.as_slice()).unwrap()
        }
    }
}
