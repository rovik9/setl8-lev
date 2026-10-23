//! Test support (std and the `engine` crate, which the `Edge` story uses to find the exact order size): a seeded PRNG (xorshift64*) and a generator of random but reproducible
//! account histories - price paths with fat tails, gaps, quiet periods, session closes, weekend gaps and holding-fee
//! nights, and a trader who fires every kind of order. A history is a list of [`Step`]s; the harness feeds them to an
//! [`crate::Evaluator`] (and, in `tests-rs`, posts the batches through the real program). Every path is a pure function of a `u64` seed.
use std::collections::BTreeSet;

use crate::codec::{IntentKind, OrderIntent};

/// xorshift64* (Vigna): small, fast, good enough, and identical everywhere.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // splitmix64 scrambles the seed so nearby seeds give unrelated streams, and zero is never the state
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(if z == 0 { 0x2545_F491_4F6C_DD1D } else { z })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        let span = (i128::from(hi) - i128::from(lo) + 1) as u128;
        let v = u128::from(self.next_u64()) % span;
        (i128::from(lo) + v as i128) as i64
    }

    /// True with probability `ppm` / 1,000,000.
    pub fn chance(&mut self, ppm: u32) -> bool {
        self.next_u64() % 1_000_000 < u64::from(ppm)
    }

    pub fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[(self.next_u64() % xs.len() as u64) as usize]
    }

    /// Roughly normal, standard deviation 1000 (Irwin-Hall of four uniforms).
    pub fn normal_milli(&mut self) -> i64 {
        let s: i64 = (0..4).map(|_| self.range(-1000, 1000)).sum();
        s * 1000 / 1155
    }
}

/// The market classes the generator uses.
#[derive(Clone, Debug)]
pub struct MarketSpec {
    pub id: u16,
    pub class: u8,
    pub symbol: String,
    /// PriceProof source: 2 GMX tickers, 3 GMX signed report (proof carries a 32-byte digest), 4 Hyperliquid.
    pub source: u8,
    pub start_price: u64,
    pub max_age_secs: u16,
    pub spread_e6: u32,
    pub sessions: Vec<(u16, u16)>,
    /// Quantity step and minimum in 1e-6 units (1,000 and 1,000 for the ordinary markets).
    pub qty_step: u64,
    pub qty_min: u64,
    /// The per-position notional cap in micro-USD (1e15 for the ordinary markets).
    pub max_notional_micro: u64,
}

impl MarketSpec {
    /// The ordinary market parameters (step 1,000, minimum 1,000, a cap nobody reaches).
    #[allow(clippy::too_many_arguments)]
    pub fn plain(id: u16, class: u8, symbol: String, source: u8, start_price: u64, max_age_secs: u16, spread_e6: u32, sessions: Vec<(u16, u16)>) -> MarketSpec {
        MarketSpec { id, class, symbol, source, start_price, max_age_secs, spread_e6, sessions, qty_step: 1_000, qty_min: 1_000, max_notional_micro: 1_000_000_000_000_000 }
    }
}

/// One input of a history, in arrival order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Sample { market: u16, sec: i64, mid: u64 },
    Order { nonce: u32, intent: OrderIntent },
    /// Backend time advances to `sec` with no input.
    Tick { sec: i64 },
    /// A posting opportunity: drain the batches (and post them).
    Post { sec: i64, heartbeat: bool },
    /// The trader presses "request payout and move to the next phase" at `sec`.
    Pass { sec: i64, plan: PassPlan },
    /// The evaluator holds (`held`) or releases the account at `sec` (`hold_account` / `release_hold`): every second before `sec`
    /// has been processed and posted first, every second from `sec` on is held (or no longer held).
    Hold { sec: i64, held: bool },
}

/// What happens after the button is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassPlan {
    /// The evaluator confirms; the delay runs out and anyone finalizes (phase 1: the next phase starts at once).
    Confirm,
    /// The evaluator never answers; the trader cancels after an hour.
    Unanswered,
    /// Confirmed, then both admins veto.
    Veto,
    /// The evaluator has queued events the chain has not seen: `reject_pass(JournalBehind)`.
    JournalBehind,
    /// A pending order is left: `reject_pass(Other)`.
    PendingLeft,
}

/// The kind of story a history tells (the campaigns count what happened, not what was planned).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Noise,
    Breach,
    StaticFail,
    QuickFail,
    Pass,
    Night,
    Closed,
    TightClean,
    /// M9b: a breach whose forced closes end at or below the static floor or over the quick limit, and a flip whose
    /// closing slice overshoots the quick limit (decisions D2 and D4).
    Convert,
    /// M9b: the margin of a triggered pending order is short (decision D3); the order stays pending and may fill later.
    Margin,
    /// M9c: market orders that use exactly all the margin (price 1.0, a priced market with a quantity step of one unit, no
    /// commission) and one more unit that is refused; the same at the end of a flip and of an add.
    Edge,
    /// P21 (decision 64): a second market the rule set does not price (half-spread 0). The trader trades the priced market and
    /// tries to open the unpriced one with a market order, a limit order and a stop order: every attempt is refused, nothing is
    /// journalled for it.
    Unpriced,
    /// M9c: a pending order that is individually under the market's notional cap but not together with the position it adds
    /// to: refused at the trigger, filled later when the position is gone.
    Cap,
    /// M9c: a pending reverse order three steps larger than the position: the remainder of the flip is below the market's
    /// minimum, the order is refused at the trigger and filled when the position is gone.
    Remainder,
    /// M9c: the account is held for review while a pending order triggers and a market order arrives; the order is refused,
    /// stays pending and fills when the hold is released.
    Hold,
    /// P26 (decision 69, audit F-16): three or four long positions in as many class-0 markets, all younger than 45 s and in profit;
    /// the trader closes them one after the other. The first close is small, the second alone takes the quick tally over 2.8% of S:
    /// the account fails and the positions still open are closed with it (`Close{Fail}` after the crossing close, then the `Fail`).
    MultiQuick,
    /// P27 (decision 71): three long positions in three class-0 markets with a commission, a slide of 9 to 13% of S through the static
    /// floor: the failing batch holds the three forced closes, their three commissions, the `Fail` and the `Meta` (8 events, 3 proofs),
    /// the heaviest legal `report_fail` the evaluator emits with fees.
    CommissionFail,
    /// P24a (decision 72): the close-all stress stories of the event-count check, never drawn by `generate`: `n` positions in `n` markets
    /// slide through the static floor (`CloseAllStatic`) or the second close crosses the quick limit (`CloseAllQuick`); see `close_all_story`.
    CloseAllStatic,
    CloseAllQuick,
}

/// A complete history.
#[derive(Clone, Debug)]
pub struct PathSpec {
    pub seed: u64,
    pub scenario: Scenario,
    /// Index into the ten launch tiers (DESIGN 9.2).
    pub tier: usize,
    pub size_micro: i64,
    pub phase: u8,
    pub opened_time: i64,
    pub markets: Vec<MarketSpec>,
    pub commission_e6: [u32; 8],
    pub steps: Vec<Step>,
    /// Maximum events per batch the harness uses for this path (a different SPLIT shape per path).
    pub batch_max_events: usize,
    pub end: i64,
}

pub const TIER_SIZES_DOLLARS: [i64; 10] = [2_500, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000];
const USD: i64 = 1_000_000;
const E12: u64 = 1_000_000_000_000;
/// 2026-10-12 (a Monday) 00:00:00 UTC.
pub const MONDAY: i64 = 1_791_763_200;
const DAY: i64 = 86_400;
const FULL_WEEK: [(u16, u16); 1] = [(0, 10_080)];

fn symbol_of(class: u8, id: u16) -> String {
    format!("M{id}C{class}")
}

/// One market's evolving price.
#[derive(Clone, Debug)]
struct MarketGen {
    spec: MarketSpec,
    price: u64,
    next_due: i64,
    /// Session windows closed right now (frozen price, samples keep coming).
    frozen: bool,
}

struct Plot {
    rng: Rng,
    t: i64,
    steps: Vec<Step>,
    mk: Vec<MarketGen>,
    used: BTreeSet<i64>,
    nonce: u32,
    size: i64,
    /// Notional multiple of the main position in thousandths of S, and its side, for profit-space moves.
    k_milli: i64,
    long: bool,
    allow_collide: bool,
    /// The newest second any emitted sample carries (orders and posts are dated with it).
    last: i64,
    /// Margin the script has committed (every order assumed to fill and never to shrink), micro-USD.
    pub committed: i64,
}

impl Plot {
    fn new(rng: Rng, t0: i64, size: i64, markets: &[MarketSpec], allow_collide: bool) -> Plot {
        let mk = markets.iter().map(|m| MarketGen { spec: m.clone(), price: m.start_price, next_due: t0, frozen: false }).collect();
        Plot { rng, t: t0, steps: Vec::new(), mk, used: BTreeSet::new(), nonce: 1, size, k_milli: 1_000, long: true, allow_collide, last: t0, committed: 0 }
    }

    fn emit_sample(&mut self, m: usize, mut sec: i64) {
        if !self.allow_collide {
            while self.used.contains(&sec) {
                sec += 1;
            }
        }
        self.used.insert(sec);
        self.last = self.last.max(sec);
        let (id, mid) = (self.mk[m].spec.id, self.mk[m].price);
        self.steps.push(Step::Sample { market: id, sec, mid });
    }

    /// One sample of the primary market now and of every other market that is due. With steps of `dt` seconds a
    /// secondary market is sampled `m` seconds after the primary (never later than the next step), or at the same second when
    /// collisions are allowed.
    fn tick(&mut self, dt: i64) {
        let t = self.t;
        self.emit_sample(0, t);
        for m in 1..self.mk.len() {
            let off = if self.allow_collide { 0 } else { m as i64 };
            if self.mk[m].next_due <= t && off < dt {
                let sec = t + off;
                self.emit_sample(m, sec);
                let gap = i64::from(self.mk[m].spec.max_age_secs / 4).clamp(10, 300);
                self.mk[m].next_due = t + gap;
            }
        }
    }

    /// Multiplies a market's price by `1 + ppb / 1e9`.
    fn bump(&mut self, m: usize, ppb: i64) {
        let p = i128::from(self.mk[m].price);
        let n = p + p * i128::from(ppb) / 1_000_000_000;
        self.mk[m].price = u64::try_from(n.max(1)).unwrap_or(1);
    }

    /// Random-walk the secondary markets one step.
    fn wander_others(&mut self, base_ppb: i64) {
        for m in 1..self.mk.len() {
            let z = self.rng.normal_milli();
            let mut r = base_ppb * z / 1000;
            if self.rng.chance(20_000) {
                r *= 5;
            }
            if self.rng.chance(2_000) {
                r *= 25;
            }
            if !self.mk[m].frozen {
                self.bump(m, r);
            }
        }
    }

    /// Price change of the primary market, in ppb, that moves the open profit of the main position by `g_ppm` of S.
    fn s_to_ppb(&self, g_ppm: i64) -> i64 {
        let k = self.k_milli.max(1);
        let v = i128::from(g_ppm) * 1_000_000 / i128::from(k);
        let v = i64::try_from(v).unwrap_or(i64::MAX);
        if self.long {
            v
        } else {
            -v
        }
    }

    /// Runs `secs` seconds in steps of `dt`: the main position gains `g_ppm` of S in total (plus noise of `sigma_ppm` of S per step).
    fn run(&mut self, secs: i64, dt: i64, g_ppm: i64, sigma_ppm: i64, others_ppb: i64) {
        let n = (secs / dt).max(1);
        for _ in 0..n {
            self.t += dt;
            let z = self.rng.normal_milli();
            let mut noise = sigma_ppm * z / 1000;
            if self.rng.chance(15_000) {
                noise *= 4;
            }
            let step_g = g_ppm / n + noise;
            let ppb = self.s_to_ppb(step_g);
            self.bump(0, ppb);
            self.wander_others(others_ppb);
            self.tick(dt);
        }
    }

    /// An immediate jump of the main position's profit by `g_ppm` of S (a gap), then one sample.
    fn gap(&mut self, g_ppm: i64) {
        self.t += 1;
        let ppb = self.s_to_ppb(g_ppm);
        self.bump(0, ppb);
        self.tick(1);
    }

    fn qty_for(&self, m: usize, k_milli: i64) -> u64 {
        let notional = i128::from(self.size) * i128::from(k_milli) / 1000;
        let q = notional * i128::from(E12) / i128::from(self.mk[m].price.max(1));
        let q = (q / 1000) * 1000;
        u64::try_from(q.clamp(1_000, i128::from(i64::MAX / 4))).unwrap_or(1_000)
    }

    fn nonce(&mut self) -> u32 {
        let n = self.nonce;
        self.nonce += 1;
        n
    }

    fn order(&mut self, intent: OrderIntent) -> u32 {
        let nonce = self.nonce();
        self.steps.push(Step::Order { nonce, intent });
        nonce
    }

    #[allow(clippy::too_many_arguments)]
    fn intent(&self, kind: IntentKind, m: usize, side: u8, qty: u64, price: u64, sl: u64, tp: u64, target: u32) -> OrderIntent {
        OrderIntent { kind, market: self.mk[m].spec.id, side, qty, price_e12: price, sl_e12: sl, tp_e12: tp, target_nonce: target, client_time: self.last }
    }

    fn market_order(&mut self, m: usize, long: bool, qty: u64, sl: u64, tp: u64) -> u32 {
        let it = self.intent(IntentKind::Market, m, if long { 0 } else { 1 }, qty, 0, sl, tp, 0);
        self.order(it)
    }

    fn close_order(&mut self, m: usize, qty: u64) -> u32 {
        let it = self.intent(IntentKind::Close, m, 0, qty, 0, 0, 0, 0);
        self.order(it)
    }

    #[allow(clippy::too_many_arguments)]
    fn limit_order(&mut self, m: usize, long: bool, qty: u64, level: u64, stop: bool, sl: u64, tp: u64) -> u32 {
        let kind = if stop { IntentKind::Stop } else { IntentKind::Limit };
        let it = self.intent(kind, m, if long { 0 } else { 1 }, qty, level, sl, tp, 0);
        self.order(it)
    }

    fn cancel_order(&mut self, m: usize, target: u32) {
        let it = self.intent(IntentKind::Cancel, m, 0, 0, 0, 0, 0, target);
        self.order(it);
    }

    fn set_sl_tp(&mut self, m: usize, sl: u64, tp: u64) {
        let it = self.intent(IntentKind::SetSlTp, m, 0, 0, 0, sl, tp, 0);
        self.order(it);
    }

    /// Opens the main position (market 0) of `k_milli` thousandths of S.
    fn open_main(&mut self, long: bool, k_milli: i64, sl: u64, tp: u64) {
        self.long = long;
        self.k_milli = k_milli;
        let q = self.qty_for(0, k_milli);
        self.market_order(0, long, q, sl, tp);
    }

    fn post(&mut self, heartbeat: bool) {
        let sec = self.last;
        self.steps.push(Step::Post { sec, heartbeat });
    }

    fn price(&self, m: usize) -> u64 {
        self.mk[m].price
    }

    /// `price x (1 + ppm/1e6)`.
    fn scaled(&self, m: usize, ppm: i64) -> u64 {
        let p = i128::from(self.mk[m].price);
        u64::try_from((p + p * i128::from(ppm) / 1_000_000).max(1)).unwrap_or(1)
    }
}

struct Ctx {
    size: i64,
    lev0: i64,
    phase: u8,
}

fn k_range(ctx: &Ctx) -> (i64, i64) {
    // main position notional between 0.4 S and 45% of the margin capacity (S x leverage), at most 25 S
    let hi = (ctx.lev0 * 450).min(25_000);
    (400.min(hi), hi.max(500))
}

fn main_k(rng: &mut Rng, ctx: &Ctx) -> i64 {
    let (lo, hi) = k_range(ctx);
    rng.range(lo, hi)
}

fn pick_source(rng: &mut Rng) -> u8 {
    match rng.range(0, 99) {
        0..=69 => 2,
        70..=84 => 3,
        _ => 4,
    }
}

fn markets_for(rng: &mut Rng, closed_scenario: bool, two: bool) -> Vec<MarketSpec> {
    let n = if two { rng.range(2, 3) } else { rng.range(1, 3) };
    let mut v = Vec::new();
    // primary: FX major 65%, crypto major 15%, metal 10%, index 10%
    let r = rng.range(0, 99);
    let (class, price, spread, age): (u8, u64, u32, u16) = if r < 65 {
        (0, rng.range(900, 1400) as u64 * E12 / 1000, rng.range(10, 60) as u32, 900)
    } else if r < 80 {
        (6, rng.range(20_000, 90_000) as u64 * E12, rng.range(30, 120) as u32, 900)
    } else if r < 90 {
        (2, rng.range(1_800, 3_200) as u64 * E12, rng.range(20, 80) as u32, 900)
    } else {
        (3, rng.range(3_000, 6_000) as u64 * E12, rng.range(15, 60) as u32, 900)
    };
    v.push(MarketSpec::plain(0, class, symbol_of(class, 0), pick_source(rng), price, age, spread, FULL_WEEK.to_vec()));
    for i in 1..n {
        let id = i as u16;
        let (class, price) = *[(6u8, 30_000u64 * E12), (0, 1_100_000_000_000), (4, 80 * E12), (5, 150 * E12)].get(rng.range(0, 3) as usize).unwrap_or(&(0, E12));
        // a market with Monday to Friday 13:30-20:00 UTC sessions when a closed market is wanted
        let sessions = if closed_scenario && i == 1 {
            (0..5u16).map(|d| (d * 1440 + 13 * 60 + 30, d * 1440 + 20 * 60)).collect()
        } else {
            FULL_WEEK.to_vec()
        };
        v.push(MarketSpec::plain(id, class, symbol_of(class, id), pick_source(rng), price, 900, rng.range(10, 100) as u32, sessions));
    }
    v
}

/// The leverage of a class in the launch rule set.
pub fn leverage_of(class: u8) -> i64 {
    [50, 20, 20, 20, 10, 5, 5, 2].get(usize::from(class)).copied().unwrap_or(1)
}

/// P24a (decision 72): the owner's launch costs: 0.03 % commission per side on every class (300 e6) and the half-spread base of 50 e6.
pub const LAUNCH_COMMISSION_E6: u32 = 300;
pub const LAUNCH_HALF_SPREAD_E6: u32 = 50;

/// Whether the campaigns run with the owner's launch costs on every path (`SETL8_LAUNCH_COSTS` set) instead of the random per-path costs.
/// The `Edge` story (exact arithmetic, no commission by design) keeps its own numbers.
pub fn launch_costs_requested() -> bool {
    std::env::var_os("SETL8_LAUNCH_COSTS").is_some()
}

/// Generates the history of `seed`.
pub fn generate(seed: u64) -> PathSpec {
    generate_inner(seed, false)
}

/// Exploration only (not part of any gate): noise histories only, with commissions and collisions (two markets sampled in the
/// same second) much more often than in `generate`. Run it with `ENGINE_WILD=1` to look for disagreements nobody has written
/// down yet.
pub fn generate_wild(seed: u64) -> PathSpec {
    generate_inner(seed, true)
}

fn generate_inner(seed: u64, wild: bool) -> PathSpec {
    let mut rng = Rng::new(seed);
    // scenario mix (ppm): chosen so that the coverage floors of the gate run are cleared with a wide margin
    let r = if wild { 800 } else { rng.range(0, 999) };
    let scenario = match r {
        0..=99 => Scenario::Breach,
        100..=149 => Scenario::StaticFail,
        150..=219 => Scenario::QuickFail,
        220..=269 => Scenario::Pass,
        270..=389 => Scenario::Night,
        // P27: 4% of the paths (taken from the night stories, whose coverage is far above its floor)
        390..=429 => Scenario::CommissionFail,
        430..=489 => Scenario::Closed,
        490..=599 => Scenario::TightClean,
        940..=999 => Scenario::Convert,
        900..=939 => Scenario::Margin,
        760..=819 => Scenario::Edge,
        820..=869 => Scenario::Cap,
        870..=899 => Scenario::Remainder,
        740..=759 => Scenario::Hold,
        600..=619 => Scenario::MultiQuick,
        700..=739 => Scenario::Unpriced,
        _ => Scenario::Noise,
    };
    let tier = if matches!(scenario, Scenario::Pass) { rng.range(3, 9) as usize } else { rng.range(0, 9) as usize };
    let size = TIER_SIZES_DOLLARS[tier] * USD;
    let phase = match scenario {
        Scenario::Pass => rng.range(1, 5) as u8,
        _ => rng.range(1, 5) as u8,
    };
    let mut markets = markets_for(&mut rng, matches!(scenario, Scenario::Closed), matches!(scenario, Scenario::Convert | Scenario::Unpriced));
    shape_markets(&mut markets, scenario, size);
    let mut commission = [0u32; 8];
    // commission on some paths; more often on the quick-limit stories, where the overshooting slice has no fee (decision D2)
    let comm_ppm = if wild {
        600_000
    } else if scenario == Scenario::CommissionFail {
        1_000_000
    } else if matches!(scenario, Scenario::QuickFail | Scenario::Convert) {
        500_000
    } else {
        250_000
    };
    if rng.chance(comm_ppm) && scenario != Scenario::Edge {
        commission = [rng.range(5, 40) as u32, 30, 30, 30, 40, 50, 60, 80];
    }
    if launch_costs_requested() && scenario != Scenario::Edge {
        commission = [LAUNCH_COMMISSION_E6; 8];
        for m in markets.iter_mut().filter(|m| m.spread_e6 > 0) {
            m.spread_e6 = LAUNCH_HALF_SPREAD_E6;
        }
    }
    let opened_time = match scenario {
        Scenario::Night => MONDAY + 2 * DAY + 20 * 3_600 + rng.range(0, 3_000),
        Scenario::Closed => MONDAY + 2 * DAY + 19 * 3_600 + rng.range(0, 1_500),
        _ => MONDAY + rng.range(0, 3) * DAY + 8 * 3_600 + rng.range(0, 3 * 3_600),
    };
    let collide = rng.chance(if wild { 500_000 } else { 50_000 });
    build_path(seed, scenario, tier, size, phase, markets, commission, opened_time, collide)
}

/// The history of one account once its parameters are fixed: `scenario` plays out on `markets` from `opened_time` on.
#[allow(clippy::too_many_arguments)]
fn build_path(seed: u64, scenario: Scenario, tier: usize, size: i64, phase: u8, markets: Vec<MarketSpec>, commission: [u32; 8], opened_time: i64, collide: bool) -> PathSpec {
    let t0 = opened_time + 600;
    let ctx = Ctx { size, lev0: leverage_of(markets[0].class), phase };
    let mut p = Plot::new(Rng::new(seed), t0, size, &markets, collide);
    p.rng = Rng::new(seed ^ 0xDEAD_BEEF);
    match scenario {
        Scenario::Noise => noise(&mut p, &ctx),
        Scenario::Breach => breach(&mut p, &ctx),
        Scenario::StaticFail => static_fail(&mut p, &ctx),
        Scenario::QuickFail => quick_fail(&mut p, &ctx),
        Scenario::Pass => pass(&mut p, &ctx),
        Scenario::Night => night(&mut p, &ctx),
        Scenario::Closed => closed(&mut p, &ctx),
        Scenario::TightClean => tight_clean(&mut p, &ctx),
        Scenario::Convert => convert(&mut p, &ctx),
        Scenario::Margin => margin(&mut p, &ctx),
        Scenario::Edge => edge(&mut p, &ctx),
        Scenario::Unpriced => unpriced(&mut p, &ctx),
        Scenario::Cap => cap(&mut p, &ctx),
        Scenario::Remainder => remainder(&mut p, &ctx),
        Scenario::Hold => hold(&mut p, &ctx),
        Scenario::MultiQuick => multi_quick(&mut p, &ctx),
        Scenario::CommissionFail => commission_fail(&mut p, &ctx),
        Scenario::CloseAllStatic => close_all(&mut p, false),
        Scenario::CloseAllQuick => close_all(&mut p, true),
    }
    // a final tick one second after the last input closes the last second
    let end = p.last + 2;
    p.steps.push(Step::Tick { sec: end });
    p.steps.push(Step::Post { sec: end, heartbeat: true });
    let batch_max_events = [8usize, 8, 8, 6, 4, 3, 2][(seed % 7) as usize];
    PathSpec { seed, scenario, tier, size_micro: size, phase, opened_time, markets, commission_e6: commission, steps: p.steps, batch_max_events, end }
}

/// P24a: the close-all stories of the event-count check. `n` long positions in `n` class-0 markets of proof `source` (the biggest account tier,
/// so margin never limits), opened together. `quick = false`: they slide together through the static floor and the account fails with all `n`
/// positions closed, each with its commission (`Close, Fee` x n, `Fail`, `Meta`); `quick = true`: the first gains a little, the others a lot,
/// the second close crosses the quick limit and the other `n - 2` positions are closed with it (decision 69, no commission on them).
/// `launch` = the owner's costs (300 e6, 50 e6), otherwise no commission and a half-spread of 30 e6.
pub fn close_all_story(seed: u64, n: usize, quick: bool, launch: bool, source: u8) -> PathSpec {
    let tier = 9;
    let size = TIER_SIZES_DOLLARS[tier] * USD;
    let spread = if launch { LAUNCH_HALF_SPREAD_E6 } else { 30 };
    let markets: Vec<MarketSpec> = (0..n)
        .map(|i| {
            let id = u16::try_from(i).unwrap_or(0);
            MarketSpec::plain(id, 0, symbol_of(0, id), source, 1_100_000_000_000 + u64::from(id) * 37_000_000_000, 900, spread, FULL_WEEK.to_vec())
        })
        .collect();
    let commission = if launch { [LAUNCH_COMMISSION_E6; 8] } else { [0; 8] };
    let scenario = if quick { Scenario::CloseAllQuick } else { Scenario::CloseAllStatic };
    build_path(seed, scenario, tier, size, 2, markets, commission, MONDAY + DAY + 8 * 3_600, true)
}

// ---------------------------------------------------------------------------------------------- M8: the reset story

/// M8: whether the history of `first` goes on after its account failed. The trader pays the reset price (`reset_account`) and
/// continues in a NEW account of the same tier and the SAME phase with fresh engine state; the campaigns run the second
/// segment against the new account. Only the failing stories qualify; a share of them stays failed for good.
pub fn reset_story(first: &PathSpec) -> bool {
    matches!(first.scenario, Scenario::StaticFail | Scenario::QuickFail | Scenario::Convert) && Rng::new(first.seed ^ 0x5E5E_7000_0000_0001).chance(700_000)
}

/// M8: the history of the NEW account of a reset made at `opened_time` (the clock of the `reset_account` transaction): the tier,
/// size, phase, markets and commissions of `first`, another story. Only stories that do not depend on the day of the week
/// (no `Night`, no `Closed`); a pass story only for tiers from $15K up, as in `generate`.
pub fn generate_after_reset(first: &PathSpec, opened_time: i64) -> PathSpec {
    let mut rng = Rng::new(first.seed ^ 0xA5A5_0F0F_1234_5678);
    let r = rng.range(0, 99);
    let scenario = match r {
        0..=34 if first.tier >= 3 => Scenario::Pass,
        0..=54 => Scenario::Noise,
        55..=64 => Scenario::StaticFail,
        65..=74 => Scenario::QuickFail,
        75..=84 => Scenario::Breach,
        85..=94 => Scenario::TightClean,
        _ => Scenario::Margin,
    };
    let seed = first.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x4D38);
    build_path(seed, scenario, first.tier, first.size_micro, first.phase, first.markets.clone(), first.commission_e6, opened_time, false)
}

// ---------------------------------------------------------------------------------------------- scenarios

fn sprinkle_posts(p: &mut Plot) {
    // posting opportunities are part of the path; the harness drains at these points
    let n = p.steps.len();
    if n.is_multiple_of(3) {
        p.post(false);
    }
}

fn noise(p: &mut Plot, ctx: &Ctx) {
    let secs = p.rng.range(200, 900);
    let k = main_k(&mut p.rng, ctx).min(1500);
    p.run(30, 5, 0, 800, 100_000);
    let long = p.rng.chance(500_000);
    p.open_main(long, k, 0, 0);
    let mut pend: Vec<(u32, usize)> = Vec::new();
    let mut elapsed = 0;
    while elapsed < secs {
        let dt = p.rng.pick(&[2i64, 3, 5, 10, 15]);
        let drift = p.rng.range(-4_000, 4_000);
        p.run(dt * 6, dt, drift, 1_500, 200_000);
        elapsed += dt * 6;
        sprinkle_posts(p);
        match p.rng.range(0, 11) {
            0 => {
                // a market order on any market: it can leave a position that a pending order of that market only reduces (D1)
                let kk = p.rng.range(100, 700);
                let long = p.rng.chance(500_000);
                let m = p.rng.range(0, p.mk.len() as i64 - 1) as usize;
                let q = p.qty_for(m, kk);
                p.market_order(m, long, q, 0, 0);
            }
            1 | 2 | 8 | 9 => {
                // a limit or a stop around any market, either side: it can open, add, reduce or flip (D1); its margin is
                // checked when it fills and an order the account cannot margin stays pending (D3)
                {
                    let m = p.rng.range(0, p.mk.len() as i64 - 1) as usize;
                    let long = p.rng.chance(500_000);
                    let stop = p.rng.chance(400_000);
                    let off = p.rng.range(500, 6_000);
                    let below = long != stop;
                    let level = p.scaled(m, if below { -off } else { off });
                    let kk = if p.rng.chance(250_000) { p.rng.range(800, 2_500) } else { p.rng.range(80, 500) };
                    let q = p.qty_for(m, kk);
                    let (sl, tp) = if p.rng.chance(400_000) {
                        let w1 = p.rng.range(1_000, 6_000);
                        let w2 = p.rng.range(1_000, 6_000);
                        if long {
                            (scaled_level(level, -w1), scaled_level(level, w2))
                        } else {
                            (scaled_level(level, w1), scaled_level(level, -w2))
                        }
                    } else {
                        (0, 0)
                    };
                    if p.committed + margin_of(p, m, kk) < ctx.size * 130 / 100 {
                        p.committed += margin_of(p, m, kk);
                        let n = p.limit_order(m, long, q, level, stop, sl, tp);
                        pend.push((n, m));
                    }
                }
            }
            3 => {
                if let Some((n, m)) = pend.pop() {
                    p.cancel_order(m, n);
                }
            }
            4 => {
                let m = p.rng.range(0, p.mk.len() as i64 - 1) as usize;
                p.close_order(m, 0);
            }
            5 => {
                // flip the main position
                let q = p.qty_for(0, k * 3 / 2);
                p.market_order(0, !p.long, q, 0, 0);
                p.long = !p.long;
            }
            6 => {
                let w1 = p.rng.range(3_000, 12_000);
                let w2 = p.rng.range(3_000, 12_000);
                let (sl, tp) = if p.long { (p.scaled(0, -w1), p.scaled(0, w2)) } else { (p.scaled(0, w1), p.scaled(0, -w2)) };
                p.set_sl_tp(0, sl, tp);
            }
            7 => {
                // add to the main position with SL and TP attached
                let w = p.rng.range(4_000, 15_000);
                let (sl, tp) = if p.long { (p.scaled(0, -w), p.scaled(0, w)) } else { (p.scaled(0, w), p.scaled(0, -w)) };
                let q = p.qty_for(0, k / 3);
                let l = p.long;
                p.market_order(0, l, q, sl, tp);
            }
            _ => {}
        }
    }
    p.run(60, 5, 0, 500, 100_000);
    p.close_order(0, 0);
    p.run(30, 5, 0, 300, 50_000);
}

/// M9b stories (decisions D2 and D4): what a breach turns into when its forced closes are booked, and a flip whose closing
/// slice overshoots the quick limit. Percentages are of S, in ppm.
fn convert(p: &mut Plot, ctx: &Ctx) {
    let r = p.rng.range(0, 99);
    if r < 40 && p.mk.len() >= 2 {
        convert_static(p, ctx);
    } else if r < 75 {
        convert_quick_breach(p, ctx);
    } else {
        convert_flip(p, ctx);
    }
}

/// A realized loss of 7.4% to 7.8% of S (the balance sits 0.2% to 0.6% above the static floor), then in under 45 s: position A
/// gains `g` (tight mode starts), position B loses `d`, the equity touches the tight floor (a breach), and the closes
/// (A's profit halved as a quick slice, B's loss in full) end at or below the static floor: `Fail(StaticFloor)`.
fn convert_static(p: &mut Plot, ctx: &Ctx) {
    let k0 = (ctx.lev0 * 150).clamp(400, 2_000);
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 300, 50_000);
    p.open_main(long, k0, 0, 0);
    p.run(10, 5, 0, 100, 50_000);
    let l = p.rng.range(74_000, 78_000);
    p.run(30, 5, -l, 100, 50_000);
    p.close_order(0, 0);
    p.run(15, 5, 0, 100, 50_000);
    p.post(false);
    let delta = 80_000 - l; // B minus the static floor, ppm of S
    let lev1 = leverage_of(p.mk[1].spec.class);
    let k1 = (lev1 * 150).clamp(300, 2_000);
    p.long = true;
    p.k_milli = k0;
    let qa = p.qty_for(0, k0);
    p.market_order(0, true, qa, 0, 0);
    let qb = p.qty_for(1, k1);
    p.market_order(1, true, qb, 0, 0);
    p.run(5, 5, 0, 50, 20_000);
    let g = p.rng.range(12_000, 25_000);
    let d_min = (delta + g / 2 + 500).max(10_200);
    let d_max = delta + g - 500;
    let d = if d_min < d_max { p.rng.range(d_min, d_max) } else { d_max };
    // A gains g: the primary market moves
    p.long = true;
    p.t += 5;
    let ppb = p.s_to_ppb(g);
    p.bump(0, ppb);
    p.tick(5);
    p.run(5, 5, 0, 20, 0);
    // B loses d: the secondary market moves and is sampled now
    p.t += 5;
    let ppb_b = -(i64::try_from(i128::from(d) * 1_000_000 / i128::from(k1)).unwrap_or(i64::MAX));
    p.bump(1, ppb_b);
    p.mk[1].next_due = p.t;
    p.tick(5);
    p.run(15, 5, 0, 10, 0);
    p.post(false);
}

/// Open a large position, gain `a` (11% to 16% of S) within seconds (tight mode), fall back to a profit `pb` of 5.7% of S or more:
/// the equity touches the tight floor, the quick slice is credited half, which is over the 2.8% limit: `Fail(PhaseQuick)`.
fn convert_quick_breach(p: &mut Plot, ctx: &Ctx) {
    let k = main_k(&mut p.rng, ctx).max(1_000.min(k_range(ctx).1));
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 300, 50_000);
    p.open_main(long, k, 0, 0);
    p.run(5, 5, 0, 100, 20_000);
    let a = p.rng.range(110_000, 160_000);
    let pb = p.rng.range(57_500, a - 51_000);
    p.gap(a);
    p.gap(-(a - pb));
    p.run(20, 5, 0, 20, 20_000);
    p.post(false);
}

/// A quick profit of 6% to 11% of S in about 20 s, then a flip: the closing slice alone is credited more than 2.8% of S, so the
/// account fails before the opening remainder.
fn convert_flip(p: &mut Plot, ctx: &Ctx) {
    let k = main_k(&mut p.rng, ctx).max(1_000.min(k_range(ctx).1));
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 300, 50_000);
    p.open_main(long, k, 0, 0);
    p.run(5, 5, 0, 100, 20_000);
    let g = p.rng.range(60_000, 110_000);
    p.run(15, 5, g, 50, 20_000);
    p.run(5, 5, 0, 20, 20_000);
    let q = p.qty_for(0, k * 3 / 2);
    p.market_order(0, !long, q, 0, 0);
    p.long = !long;
    p.run(20, 5, 0, 100, 20_000);
    p.post(false);
}

/// The market parameters a story needs (M9c): exact arithmetic for `Edge`, a notional cap for `Cap`, a quantity minimum for
/// `Remainder`. All three trade a single FX market (class 0, 50x). `Unpriced` keeps two markets and takes the half-spread of the
/// second away (the rule set does not price it, decision 64).
fn shape_markets(markets: &mut Vec<MarketSpec>, scenario: Scenario, size: i64) {
    if matches!(scenario, Scenario::MultiQuick | Scenario::CommissionFail) {
        // three markets, four when the draw gave three (always three for the commission story): all class 0 (50x), open all week, one
        // source and spread each
        let n: u16 = if markets.len() >= 3 && scenario == Scenario::MultiQuick { 4 } else { 3 };
        markets.truncate(1);
        let Some(m0) = markets.first_mut() else { return };
        m0.class = 0;
        m0.symbol = symbol_of(0, 0);
        m0.sessions = FULL_WEEK.to_vec();
        m0.max_age_secs = 900;
        m0.start_price = 1_100_000_000_000;
        let base = m0.clone();
        for i in 1..n {
            let mut m = base.clone();
            m.id = i;
            m.symbol = symbol_of(0, i);
            m.start_price = base.start_price + u64::from(i) * 37_000_000_000;
            m.spread_e6 = base.spread_e6 + u32::from(i) * 7;
            markets.push(m);
        }
        return;
    }
    if scenario == Scenario::Unpriced {
        markets.truncate(2);
        if let Some(m) = markets.get_mut(1) {
            m.spread_e6 = 0;
            m.sessions = FULL_WEEK.to_vec();
        }
        return;
    }
    if !matches!(scenario, Scenario::Edge | Scenario::Cap | Scenario::Remainder) {
        return;
    }
    markets.truncate(1);
    let Some(m) = markets.first_mut() else { return };
    m.class = 0;
    m.sessions = FULL_WEEK.to_vec();
    m.max_age_secs = 900;
    if m.start_price < 800 * E12 / 1000 || m.start_price > 1_500 * E12 / 1000 {
        m.start_price = 1_100_000_000_000;
    }
    match scenario {
        Scenario::Edge => {
            // price 1.0, the half-spread the market was given (a priced market: decision 64), and a step of one unit: the largest
            // order the margin allows is found by `edge_max_order`, which needs a quantity grid fine enough to land on the boundary
            m.start_price = E12;
            m.qty_step = 1;
            m.qty_min = 1;
        }
        Scenario::Cap => m.max_notional_micro = u64::try_from(size.saturating_mul(3)).unwrap_or(u64::MAX),
        _ => m.qty_min = 1_000_000,
    }
}

/// `equity - margin used` that a market order of `qty` units (long or short) in the single edge market leaves, as the evaluator's
/// risk check computes it (no commission): `existing` is the position before the order, `b` the balance, `mid` the market's price
/// and `h` its half-spread. A flip closes the opposite position first at the fill price, as `exec_trade` does.
fn edge_slack(existing: Option<engine::Position>, b: i64, long: bool, qty: u64, h: u32, mid: u64, lev: u16) -> Option<i128> {
    let side = if long { engine::Side::Buy } else { engine::Side::Sell };
    let price = engine::fill_price_e12(mid, h, long).ok()?;
    let (bid, ask) = (engine::bid_from_mid(mid, h).ok()?, engine::ask_from_mid(mid, h).ok()?);
    // a time well after the position was opened: a loss is credited in full whatever the age of the slice
    let t = existing.map_or(0, |p| p.timer_start) + 3_600;
    let day = engine::day_index(t);
    let close_qty = match existing {
        Some(p) if p.qty.is_positive() != long => qty.min(p.qty.unsigned_abs()),
        _ => 0,
    };
    let (mut base, mut b_after) = (existing, b);
    if close_qty > 0 {
        let o = engine::apply_fill(base, side, close_qty, price, t, day).ok()?;
        b_after = b_after.checked_add(o.balance_delta)?;
        base = o.position;
    }
    let np = engine::apply_fill(base, side, qty - close_qty, price, t, day).ok()?.position?;
    let p_open = engine::open_profit_micro(&[(np, bid, ask)]).ok()?;
    let used = engine::margin_micro(np.qty.unsigned_abs(), np.avg_entry_e12, lev).ok()?;
    Some(i128::from(b_after) + i128::from(p_open) - i128::from(used))
}

/// The order quantity (units, at least `lo`) with `equity - margin == 0` exactly and one unit more short of margin. The slack
/// falls by at most one micro-USD per unit (a unit is 0.02 micro-USD of margin and about 2e-6 of loss at 50x), so the largest
/// quantity that still fits lands on zero; the scan below the binary search covers a rounding step of two. `None` when no such
/// quantity exists in the search range.
fn edge_max_order(existing: Option<engine::Position>, b: i64, long: bool, lo: u64, h: u32, mid: u64, lev: u16) -> Option<u64> {
    let fits = |q: u64| edge_slack(existing, b, long, q, h, mid, lev).is_some_and(|x| x >= 0);
    if !fits(lo) {
        return None;
    }
    let mut hi = u64::try_from(i128::from(b.max(1)) * i128::from(lev) * i128::from(E12) / i128::from(mid.max(1)) * 2).ok()?.max(lo + 2);
    let mut good = lo;
    while good + 1 < hi {
        let mid_q = good + (hi - good) / 2;
        if fits(mid_q) {
            good = mid_q;
        } else {
            hi = mid_q;
        }
    }
    // `good` fits and `good + 1` does not; land on a quantity with a slack of exactly zero (and a refusal one unit above)
    (good.saturating_sub(64).max(lo)..=good).rev().find(|&q| edge_slack(existing, b, long, q, h, mid, lev) == Some(0) && !fits(q + 1))
}

/// Market orders that use exactly all the margin: `equity - margin == 0` is accepted (the verifier must accept it), one unit
/// more is refused by the evaluator (an intent on file, no event). Three shapes: a single order, an add to a smaller position,
/// and a flip whose remainder uses it. The market is priced (decision 64), so the fill is at the ask or the bid and the position
/// is marked on the other side: the quantity is found by `edge_max_order`, not by a formula.
fn edge(p: &mut Plot, ctx: &Ctx) {
    let lev = ctx.lev0;
    let lev16 = u16::try_from(lev).unwrap_or(1);
    let long = p.rng.chance(500_000);
    let (h, mid, s) = (p.mk[0].spec.spread_e6, p.mk[0].price, p.size);
    let side = |l: bool| if l { engine::Side::Buy } else { engine::Side::Sell };
    // the position a first order of `q` units opens (a flat account, no commission)
    let opened = |l: bool, q: u64| engine::fill_price_e12(mid, h, l).ok().and_then(|px| engine::apply_fill(None, side(l), q, px, 0, 0).ok()).and_then(|o| o.position);
    p.run(20, 5, 0, 0, 0);
    match p.rng.range(0, 2) {
        0 => {
            if let Some(q) = edge_max_order(None, s, long, 1, h, mid, lev16) {
                p.long = long;
                p.k_milli = lev * 1_000;
                p.market_order(0, long, q, 0, 0);
            }
        }
        1 => {
            let q1 = p.qty_for(0, lev * 600);
            p.open_main(long, lev * 600, 0, 0);
            p.run(10, 5, 0, 0, 0);
            p.post(false);
            // the add: the order that takes the position to the largest size the margin allows
            if let Some(q) = opened(long, q1).and_then(|pos| edge_max_order(Some(pos), s, long, 1, h, mid, lev16)) {
                p.market_order(0, long, q, 0, 0);
            }
        }
        _ => {
            let q_short = p.qty_for(0, lev * 300);
            p.open_main(!long, lev * 300, 0, 0);
            p.run(10, 5, 0, 0, 0);
            p.post(false);
            // closes the short in full and opens the long that uses the whole margin
            if let Some(q) = opened(!long, q_short).and_then(|pos| edge_max_order(Some(pos), s, long, q_short + 1, h, mid, lev16)) {
                p.market_order(0, long, q, 0, 0);
            }
        }
    }
    p.run(10, 5, 0, 0, 0);
    p.post(false);
    // one unit more, same side: refused for margin
    p.market_order(0, long, 1, 0, 0);
    p.run(10, 5, 0, 0, 0);
    p.post(false);
    p.close_order(0, 0);
    p.run(10, 5, 0, 0, 0);
}

/// The unpriced market (decision 64): the trader opens a position in the priced market, then tries to open the second market,
/// whose half-spread the rule set does not have: a market order, a limit order and a stop order, each refused at arrival (no line
/// in the intents file, no event). The position in the priced market goes on and is closed normally.
fn unpriced(p: &mut Plot, ctx: &Ctx) {
    let long = p.rng.chance(500_000);
    let k = main_k(&mut p.rng, ctx).min(1_000);
    p.run(20, 5, 0, 100, 20_000);
    p.open_main(long, k, 0, 0);
    p.run(10, 5, 0, 50, 20_000);
    p.post(false);
    // the second market gets a sample at least every few seconds; the attempts follow the first ones
    p.mk[1].next_due = p.t;
    p.run(10, 5, 0, 50, 20_000);
    let side2 = p.rng.chance(500_000);
    let q2 = p.qty_for(1, 300);
    p.market_order(1, side2, q2, 0, 0);
    p.run(5, 5, 0, 50, 20_000);
    let off = p.rng.range(300, 900);
    let level = p.scaled(1, if side2 { -off } else { off });
    p.limit_order(1, side2, q2, level, false, 0, 0);
    let stop_level = p.scaled(1, if side2 { off } else { -off });
    p.limit_order(1, side2, q2, stop_level, true, 0, 0);
    p.run(20, 5, 0, 50, 20_000);
    p.post(false);
    // a close in the unpriced market: nothing to close; and the priced market still trades: an add, then the close
    p.close_order(1, 0);
    let q3 = p.qty_for(0, 100);
    p.market_order(0, long, q3, 0, 0);
    p.run(20, 5, 0, 50, 20_000);
    p.post(false);
    p.close_order(0, 0);
    p.run(20, 5, 0, 50, 20_000);
}

/// A position of 2 S under a notional cap of 3 S, and a pending order of 2 S: alone it is fine, added to the position it
/// would make 4 S. It triggers when the position loses a little, is refused (`NotionalCap`), and in most stories the trader
/// closes the position so the order, still triggered, fills at the next sample.
fn cap(p: &mut Plot, _ctx: &Ctx) {
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 100, 20_000);
    p.open_main(long, 2_000, 0, 0);
    p.run(10, 5, 0, 50, 20_000);
    let off = p.rng.range(300, 600);
    let level = p.scaled(0, if long { -off } else { off });
    let q = p.qty_for(0, 2_000);
    p.limit_order(0, long, q, level, false, 0, 0);
    let loss = i64::try_from(i128::from(2_000) * i128::from(off) / 1000).unwrap_or(0) + 1_500;
    p.run(20, 5, -loss, 50, 20_000);
    p.post(false);
    p.run(15, 5, 0, 50, 20_000);
    if p.rng.chance(700_000) {
        p.close_order(0, 0);
        p.run(20, 5, 0, 50, 20_000);
        p.post(false);
        p.close_order(0, 0);
    }
    p.run(20, 5, 0, 50, 20_000);
}

/// A position of `k` thousandths of S and a pending reverse limit order for three steps (3,000 units) more than the position:
/// the order is valid when placed (a multiple of the step, above the minimum), but at the trigger the flip's remainder of
/// 3,000 units is below the market's minimum of 1,000,000: refused (`Quantity`), still pending. When the trader closes the
/// position the order, still triggered, opens a position of its own size.
fn remainder(p: &mut Plot, _ctx: &Ctx) {
    let long = p.rng.chance(500_000);
    let k = p.rng.range(400, 1_500);
    p.run(20, 5, 0, 100, 20_000);
    // the position's quantity is the one the order must exceed by three steps: take it before the price moves
    p.long = long;
    p.k_milli = k;
    let q_main = p.qty_for(0, k);
    p.market_order(0, long, q_main, 0, 0);
    p.run(10, 5, 0, 50, 20_000);
    let off = p.rng.range(300, 600);
    // the reverse order closes the position at a level in its profit direction
    let level = p.scaled(0, if long { off } else { -off });
    p.limit_order(0, !long, q_main + 3_000, level, false, 0, 0);
    let gain = i64::try_from(i128::from(k) * i128::from(off) / 1000).unwrap_or(0) + 1_500;
    p.run(20, 5, gain, 50, 20_000);
    p.post(false);
    p.run(15, 5, 0, 50, 20_000);
    if p.rng.chance(700_000) {
        p.close_order(0, 0);
        p.run(20, 5, 0, 50, 20_000);
        p.post(false);
        p.close_order(0, 0);
    }
    p.run(20, 5, 0, 50, 20_000);
}

/// A pending order is triggered while the account is held (`hold_account`): the evaluator refuses it, it stays pending and
/// nothing is journalled; a market order that adds exposure arrives during the hold and is refused too (its intent is on file),
/// a market order that closes works. After `release_hold` the pending order, still triggered, fills at the next sample.
fn hold(p: &mut Plot, _ctx: &Ctx) {
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 100, 20_000);
    p.open_main(long, 1_000, 0, 0);
    p.run(10, 5, 0, 50, 20_000);
    p.post(false);
    let off = p.rng.range(300, 600);
    let level = p.scaled(0, if long { -off } else { off });
    let q = p.qty_for(0, 500);
    p.limit_order(0, long, q, level, false, 0, 0);
    p.run(5, 5, 0, 30, 20_000);
    // the hold starts one second after the last input: everything before it is posted first
    let from = p.last + 1;
    p.steps.push(Step::Hold { sec: from, held: true });
    p.t = from;
    p.last = from;
    // the main position loses `off` of the price: the pending order triggers during the hold
    let loss = i64::try_from(i128::from(1_000) * i128::from(off) / 1000).unwrap_or(0) + 1_500;
    p.run(20, 5, -loss, 50, 20_000);
    // an order that adds exposure arrives during the hold; so does one that closes a part of the position
    let q2 = p.qty_for(0, 200);
    p.market_order(0, long, q2, 0, 0);
    p.run(10, 5, 0, 30, 20_000);
    p.post(false);
    let to = p.last + 1;
    p.steps.push(Step::Hold { sec: to, held: false });
    p.t = to;
    p.last = to;
    p.run(20, 5, 0, 30, 20_000);
    p.post(false);
    if p.rng.chance(600_000) {
        p.close_order(0, 0);
        p.run(20, 5, 0, 30, 20_000);
        p.post(false);
        p.close_order(0, 0);
    }
    p.run(10, 5, 0, 30, 20_000);
}

/// The main position uses 78% to 88% of the margin capacity; a pending order of the same side asking for 25% to 40% more is
/// triggered by a small move and cannot be margined (D3): it stays pending, nothing is journalled for it. In 60% of the
/// stories the trader then closes the main position and the order, still triggered, fills at the next sample.
fn margin(p: &mut Plot, ctx: &Ctx) {
    let lev = ctx.lev0;
    let k = p.rng.range(lev * 780, lev * 880);
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 100, 20_000);
    p.open_main(long, k, 0, 0);
    p.run(10, 5, 0, 50, 20_000);
    let off = p.rng.range(200, 500);
    let level = p.scaled(0, if long { -off } else { off });
    let kk = p.rng.range(lev * 250, lev * 400);
    let q = p.qty_for(0, kk);
    p.limit_order(0, long, q, level, false, 0, 0);
    // the main position loses about `off` of the price: the pending order triggers
    let loss = i64::try_from(i128::from(k) * i128::from(off) / 1000).unwrap_or(0) + 1_500;
    p.run(20, 5, -loss, 50, 20_000);
    p.post(false);
    p.run(15, 5, 0, 50, 20_000);
    if p.rng.chance(600_000) {
        p.close_order(0, 0);
        p.run(20, 5, 0, 50, 20_000);
        p.post(false);
        p.close_order(0, 0);
    }
    p.run(20, 5, 0, 50, 20_000);
}

fn scaled_level(level: u64, ppm: i64) -> u64 {
    let p = i128::from(level);
    u64::try_from((p + p * i128::from(ppm) / 1_000_000).max(1)).unwrap_or(1)
}

fn margin_of(p: &Plot, m: usize, k_milli: i64) -> i64 {
    let lev = leverage_of(p.mk[m].spec.class).max(1);
    let notional = i128::from(p.size) * i128::from(k_milli) / 1000;
    i64::try_from(notional / i128::from(lev)).unwrap_or(i64::MAX)
}

fn breach(p: &mut Plot, ctx: &Ctx) {
    let k = main_k(&mut p.rng, ctx);
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 600, 100_000);
    p.post(false);
    let sl_tp_noise = p.rng.chance(200_000);
    p.open_main(long, k, 0, 0);
    p.run(10, 5, 0, 300, 100_000);
    // climb to a peak
    let g1 = p.rng.range(12_000, 70_000);
    let dt = p.rng.pick(&[2i64, 5, 10]);
    let _r0 = p.rng.range(6, 40);
    p.run(dt * _r0, dt, g1, 600, 150_000);
    p.post(false);
    if p.rng.chance(400_000) {
        // a second leg up
        let g = p.rng.range(5_000, 40_000);
        p.run(dt * 10, dt, g, 400, 100_000);
    }
    if sl_tp_noise {
        p.set_sl_tp(0, 0, 0);
    }
    // then fall back through the tight floor: 5.1% to 7.5% of S below the peak (a gap in 15% of the cases)
    let d = p.rng.range(51_000, 75_000);
    if p.rng.chance(150_000) {
        p.gap(-d);
    } else {
        let _r0 = p.rng.range(8, 30);
        p.run(dt * _r0, dt, -d, 500, 100_000);
    }
    p.post(false);
    // a re-entry attempt during the lock, a pending order that survives it
    p.run(20, 5, 0, 300, 100_000);
    let q = p.qty_for(0, k / 2);
    p.market_order(0, long, q, 0, 0);
    let level = if long { p.scaled(0, -1_000) } else { p.scaled(0, 1_000) };
    let q2 = p.qty_for(0, 300);
    p.limit_order(0, long, q2, level, false, 0, 0);
    if p.rng.chance(500_000) {
        // the lock (12 h) runs out
        p.run(13 * 3_600, 900, 0, 4_000, 300_000);
        p.run(120, 10, 0, 800, 100_000);
        p.post(false);
        p.close_order(0, 0);
        p.run(60, 10, 0, 300, 50_000);
    } else {
        p.run(120, 10, 0, 300, 50_000);
    }
}

fn static_fail(p: &mut Plot, ctx: &Ctx) {
    let k = main_k(&mut p.rng, ctx);
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 600, 100_000);
    p.open_main(long, k, 0, 0);
    p.run(30, 5, 0, 500, 100_000);
    p.post(false);
    if p.rng.chance(500_000) {
        // a gap straight through the static floor
        let d = p.rng.range(82_000, 250_000);
        p.gap(-d);
    } else {
        // a slide through it
        let d = p.rng.range(85_000, 130_000);
        let _r0 = p.rng.range(40, 300);
        p.run(_r0, 5, -d, 500, 100_000);
    }
    p.run(30, 5, 0, 300, 100_000);
}

fn quick_fail(p: &mut Plot, ctx: &Ctx) {
    let k = main_k(&mut p.rng, ctx);
    p.run(20, 5, 0, 600, 100_000);
    let n = p.rng.range(3, 6);
    for _ in 0..n {
        let long = p.rng.chance(500_000);
        p.post(false);
        p.open_main(long, k, 0, 0);
        let gain = p.rng.range(18_000, 40_000);
        let age = p.rng.range(12, 38);
        p.run(age, 3, gain, 200, 100_000);
        p.close_order(0, 0);
        p.run(15, 5, 0, 300, 100_000);
    }
    p.run(30, 5, 0, 300, 100_000);
}

/// F-16 (decision 69): `n` long positions of `k` thousandths of S in as many markets, opened together. Over 30 s each gains its own
/// share of S (ppm): the first a little (its quick credit, half of it, fits under the limit), the others so much that each alone
/// is credited more than the room that is left. The trader closes them in market order: the second close takes the tally over
/// 2.8% of S, so the positions still open (one or two) are closed with it as `Close{Fail}`, and the account fails.
fn multi_quick(p: &mut Plot, _ctx: &Ctx) {
    let n = p.mk.len();
    let k = p.rng.range(1_500, 2_500);
    p.run(20, 5, 0, 200, 20_000);
    p.long = true;
    p.k_milli = k;
    for m in 0..n {
        let q = p.qty_for(m, k);
        p.market_order(m, true, q, 0, 0);
    }
    p.run(5, 5, 0, 50, 0);
    let gains: Vec<i64> = (0..n).map(|m| if m == 0 { p.rng.range(12_000, 16_000) } else { p.rng.range(52_000, 70_000) }).collect();
    let steps = 6;
    for _ in 0..steps {
        p.t += 5;
        for (m, g) in gains.iter().enumerate() {
            let ppb = i64::try_from(i128::from(*g) * 1_000_000 / i128::from(k) / i128::from(steps)).unwrap_or(0);
            p.bump(m, ppb);
            p.mk[m].next_due = p.t;
        }
        p.tick(5);
    }
    p.post(false);
    for m in 0..n {
        p.close_order(m, 0);
    }
    p.run(30, 5, 0, 20, 0);
    p.post(false);
}

/// P27: three long positions in as many class-0 markets slide together through the static floor: the account fails with its three
/// positions closed, each with its commission (`Close, Fee` x 3, `Fail`, `Meta`: 8 events, 3 proofs).
fn commission_fail(p: &mut Plot, _ctx: &Ctx) {
    let n = p.mk.len();
    let k = p.rng.range(1_500, 2_500);
    p.run(20, 5, 0, 200, 20_000);
    p.long = true;
    p.k_milli = k;
    for m in 0..n {
        let q = p.qty_for(m, k);
        p.market_order(m, true, q, 0, 0);
    }
    p.run(5, 5, 0, 50, 0);
    let loss = p.rng.range(90_000, 130_000);
    let steps = 6;
    for _ in 0..steps {
        p.t += 5;
        for m in 0..n {
            let ppb = -i64::try_from(i128::from(loss) * 1_000_000 / i128::from(k) / i128::try_from(n).unwrap_or(1) / i128::from(steps)).unwrap_or(0);
            p.bump(m, ppb);
            p.mk[m].next_due = p.t;
        }
        p.tick(5);
    }
    p.run(30, 5, 0, 20, 0);
    p.post(false);
}

/// P24a: `n = p.mk.len()` long positions of `k` thousandths of S, opened together after a warm-up long enough for every market to have a
/// sample. `quick = false`: all slide together by 9 to 13 % of S in total (the static floor), `quick = true`: the first gains 1.2 to 1.6 % of S,
/// the others 5.2 to 7 % each (the second close crosses the quick limit, the rest are closed with it).
fn close_all(p: &mut Plot, quick: bool) {
    let n = p.mk.len();
    let k = p.rng.range(1_500, 2_500);
    p.run(60, 5, 0, 200, 20_000);
    p.long = true;
    p.k_milli = k;
    for m in 0..n {
        let q = p.qty_for(m, k);
        p.market_order(m, true, q, 0, 0);
    }
    p.run(10, 5, 0, 50, 0);
    let steps = 6;
    let shares: Vec<i64> = (0..n)
        .map(|m| {
            if quick {
                if m == 0 {
                    p.rng.range(12_000, 16_000)
                } else {
                    p.rng.range(52_000, 70_000)
                }
            } else {
                -p.rng.range(90_000, 130_000) / i64::try_from(n).unwrap_or(1)
            }
        })
        .collect();
    for _ in 0..steps {
        p.t += 5;
        for (m, g) in shares.iter().enumerate() {
            let ppb = i64::try_from(i128::from(*g) * 1_000_000 / i128::from(k) / i128::from(steps)).unwrap_or(0);
            p.bump(m, ppb);
            p.mk[m].next_due = p.t;
        }
        p.tick(5);
    }
    if quick {
        p.post(false);
        for m in 0..n {
            p.close_order(m, 0);
        }
    }
    p.run(40, 5, 0, 20, 0);
    p.post(false);
}

fn tight_clean(p: &mut Plot, ctx: &Ctx) {
    let k = main_k(&mut p.rng, ctx);
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 600, 100_000);
    let use_stops = p.rng.chance(500_000);
    // protective levels that the profit leg will hit
    let g = p.rng.range(12_000, 50_000);
    let lvl_ppb = p.s_to_ppb(g * 6 / 10);
    let (sl, tp) = if use_stops {
        let px = p.price(0);
        let tp = scaled_level(px, i64::try_from(i128::from(lvl_ppb) / 1000).unwrap_or(0));
        (0, tp)
    } else {
        (0, 0)
    };
    p.long = long;
    p.k_milli = k;
    p.open_main(long, k, sl, tp);
    p.run(10, 5, 0, 300, 100_000);
    let dt = p.rng.pick(&[3i64, 5, 10]);
    p.run(dt * 20, dt, g, 500, 100_000);
    p.post(false);
    p.close_order(0, 0);
    p.run(60, 5, 0, 300, 100_000);
    // and once more, small, to restart a mode
    p.open_main(!long, k / 2, 0, 0);
    let _r0 = p.rng.range(11_000, 25_000);
    p.run(dt * 10, dt, _r0, 400, 100_000);
    p.close_order(0, 0);
    p.run(30, 5, 0, 300, 100_000);
}

fn pass(p: &mut Plot, ctx: &Ctx) {
    let k = main_k(&mut p.rng, ctx).max(2_000.min(k_range(ctx).1));
    let long = p.rng.chance(500_000);
    p.run(20, 5, 0, 500, 100_000);
    p.open_main(long, k, 0, 0);
    p.run(10, 5, 0, 200, 100_000);
    // +8.4% to +10.5% of S in 5 to 15 minutes, little noise
    let target = p.rng.range(91_000, 112_000);
    let secs = p.rng.range(300, 900);
    p.run(secs, 15, target, 300, 100_000);
    p.post(false);
    p.run(70, 5, 0, 200, 100_000);
    p.close_order(0, 0);
    p.run(20, 5, 0, 100, 50_000);
    p.post(false);
    let r = p.rng.range(0, 99);
    let plan = match r {
        0..=69 => PassPlan::Confirm,
        70..=79 => PassPlan::Unanswered,
        80..=87 => PassPlan::Veto,
        88..=93 => PassPlan::JournalBehind,
        _ => PassPlan::PendingLeft,
    };
    match plan {
        PassPlan::PendingLeft => {
            let level = p.scaled(0, -20_000);
            let q = p.qty_for(0, 300);
            p.limit_order(0, true, q, level, false, 0, 0);
            p.run(10, 5, 0, 100, 50_000);
            p.post(false);
        }
        PassPlan::JournalBehind => {
            // a position opened but not yet posted when the button is pressed
            let q = p.qty_for(0, 300);
            p.market_order(0, true, q, 0, 0);
            p.run(10, 5, 0, 100, 50_000);
        }
        _ => {}
    }
    let sec = p.last + 2;
    p.steps.push(Step::Pass { sec, plan });
    p.t = sec;
    p.last = sec;
    let _ = ctx.phase;
}

fn night(p: &mut Plot, ctx: &Ctx) {
    // a position held through one or more 22:00 UTC rollovers (a weekend when the path starts on a Friday)
    let k = main_k(&mut p.rng, ctx).min(2_000);
    let long = p.rng.chance(500_000);
    p.run(60, 10, 0, 400, 100_000);
    p.open_main(long, k, 0, 0);
    p.run(120, 10, 0, 400, 100_000);
    p.post(false);
    let hours = p.rng.pick(&[3i64, 5, 8, 14, 26, 50]);
    // sparse samples through the night (every 15 minutes), no profit story: a mild drift
    let drift = p.rng.range(-20_000, 25_000);
    p.run(hours * 3_600, 900, drift, 900, 400_000);
    p.post(false);
    if p.rng.chance(700_000) {
        p.close_order(0, 0);
        p.run(120, 10, 0, 300, 50_000);
    }
}

fn closed(p: &mut Plot, ctx: &Ctx) {
    // market 1 trades 13:30-20:00 UTC, Monday to Friday; the path starts a little before the Friday close
    let k = main_k(&mut p.rng, ctx).min(1_500);
    let has_m1 = p.mk.len() > 1;
    p.run(60, 10, 0, 400, 100_000);
    if has_m1 {
        let q = p.qty_for(1, k / 2);
        let long = p.rng.chance(500_000);
        p.market_order(1, long, q, 0, 0);
    }
    p.run(300, 10, 0, 400, 100_000);
    p.post(false);
    // run through the close: the closed market's price freezes but its samples keep coming
    for m in 1..p.mk.len() {
        p.mk[m].frozen = true;
    }
    p.run(2 * 3_600, 300, 0, 600, 200_000);
    // orders into the closed market are refused
    if has_m1 {
        let q = p.qty_for(1, 200);
        p.market_order(1, true, q, 0, 0);
        p.close_order(1, 0);
    }
    p.post(false);
    // over the weekend (two more days) the frozen samples continue every 15 minutes
    let weekend = p.rng.chance(500_000);
    let secs = if weekend { 2 * DAY + 4 * 3_600 } else { 17 * 3_600 };
    p.run(secs, 900, 0, 800, 300_000);
    for m in 1..p.mk.len() {
        p.mk[m].frozen = false;
        // the reopen gap
        let g = p.rng.range(-8_000_000, 8_000_000);
        p.bump(m, g);
    }
    p.run(300, 10, 0, 600, 200_000);
    p.post(false);
    if has_m1 {
        p.close_order(1, 0);
    }
    p.run(60, 10, 0, 300, 100_000);
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    fn markets(n: usize) -> Vec<MarketSpec> {
        let mut m = markets_for(&mut Rng::new(7), false, false);
        while m.len() < n {
            let mut x = m[0].clone();
            x.id = u16::try_from(m.len()).unwrap_or(0);
            m.push(x);
        }
        m.truncate(n);
        m
    }

    /// The multi-quick story trades three markets, four when the draw gave three or more; the commission story always three.
    #[test]
    fn the_stories_with_several_markets_take_three_or_four() {
        for len in 1..=6usize {
            let mut m = markets(len);
            shape_markets(&mut m, Scenario::MultiQuick, 1);
            assert_eq!(m.len(), if len >= 3 { 4 } else { 3 }, "multi-quick from {len} markets");
            let mut m = markets(len);
            shape_markets(&mut m, Scenario::CommissionFail, 1);
            assert_eq!(m.len(), 3, "commission story from {len} markets");
        }
    }
}
