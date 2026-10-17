//! From the input to the exact arguments of `create_market` and `create_ruleset`.
use crate::model::*;
use crate::session;
use sha2::{Digest, Sha256};

pub const NO_QUOTE_CONV: u16 = 0xFFFF;
pub const STATUS_ENABLED: u8 = 1;
pub const MAX_MARKETS: usize = 128;
pub const CLASSES: usize = 8;
pub const TIERS: usize = 10;
pub const LADDER: usize = 5;
const USD: u64 = 1_000_000;

/// `sha256("SL8FEED" || [source] || utf8 symbol)` (DESIGN 3.3, decision 46).
pub fn feed_id(source: u8, symbol: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"SL8FEED");
    h.update([source]);
    h.update(symbol.as_bytes());
    h.finalize().into()
}

/// Decision 57: `freeze x (100 + margin) / 100`, rounded UP to a multiple of `round` seconds, never below `floor`, and it must
/// fit the chain's `u16`. Integer arithmetic only: 29,900 ms with 25 % is 37,375 ms, 38 s, 40 s.
pub fn max_age_secs(freeze_ms: u64, r: &Rules) -> Result<u16, String> {
    if r.max_age_round_secs == 0 {
        return Err("max_age_round_secs must be positive".into());
    }
    let with_margin = (u128::from(freeze_ms) * u128::from(100 + r.freeze_margin_pct)).div_ceil(100);
    let secs = with_margin.div_ceil(1000);
    let round = u128::from(r.max_age_round_secs);
    let rounded = secs.div_ceil(round) * round;
    let v = rounded.max(u128::from(r.max_age_floor_secs));
    u16::try_from(v).map_err(|_| format!("max_age_secs {v} does not fit u16"))
}

/// USD price to micro-USD (nearest).
pub fn price_micro(price_usd: f64) -> Result<u64, String> {
    if !price_usd.is_finite() || price_usd <= 0.0 || price_usd > 1.0e12 {
        return Err(format!("price {price_usd} is not a usable USD price"));
    }
    Ok((price_usd * 1.0e6).round() as u64)
}

/// The smallest number of the form 1, 2 or 5 times a power of ten that is at least `n` (`n >= 1`).
pub fn round_up_125(n: u64) -> u64 {
    let mut p = 1u64;
    loop {
        for m in [1u64, 2, 5] {
            if let Some(x) = m.checked_mul(p).filter(|x| *x >= n) {
                return x;
            }
        }
        match p.checked_mul(10) {
            Some(x) => p = x,
            None => return u64::MAX,
        }
    }
}

/// The largest number of the form 1, 2 or 5 times a power of ten that is at most `n` (`n >= 1`; 0 stays 0).
pub fn round_down_125(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut best = 1u64;
    let mut p = 1u64;
    while p <= n {
        for m in [1u64, 2, 5] {
            if let Some(x) = m.checked_mul(p).filter(|x| *x <= n) {
                best = x;
            }
        }
        match p.checked_mul(10) {
            Some(x) => p = x,
            None => break,
        }
    }
    best
}

/// Quantity step and minimum, in 1e-6 base units: the smallest 1-2-5 quantity worth at least `order_notional_usd` at the price.
/// notional (micro-USD) = quantity x price (micro-USD) / 1e6.
pub fn qty_min(price_micro: u64, order_notional_usd: u64) -> Result<u64, String> {
    let want = u128::from(order_notional_usd) * u128::from(USD) * u128::from(USD);
    let q = want.div_ceil(u128::from(price_micro.max(1)));
    let q = u64::try_from(q).map_err(|_| "quantity does not fit u64".to_string())?;
    Ok(round_up_125(q.max(1)))
}

/// `max_notional_usd` placeholder in micro-USD: basis tier x class leverage, rounded down to 1-2-5, below the ceiling.
pub fn cap_micro(class: u8, leverage: u16, r: &Rules) -> u64 {
    let raw = r.cap_basis_tier_usd.saturating_mul(u64::from(leverage));
    let ceiling = if class == 5 { r.cap_ceiling_stock_usd } else { r.cap_ceiling_usd };
    round_down_125(raw.min(ceiling)) * USD
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketOut {
    pub id: u16,
    pub class: u8,
    pub status: u8,
    pub feed_id: [u8; 32],
    pub max_age_secs: u16,
    pub qty_step: u64,
    pub qty_min: u64,
    pub max_notional_usd: u64,
    pub sessions: Vec<(u16, u16)>,
    pub quote_conv_market: u16,
    /// Not part of `CreateMarketArgs`: which feed the market follows.
    pub symbol: String,
    pub source: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSetOut {
    pub version: u16,
    pub fee_bps: u16,
    pub tier_count: u8,
    pub ladder_len: u8,
    pub leverage_x: [u16; CLASSES],
    pub holding_fee_e6: [u32; CLASSES],
    pub commission_e6: [u32; CLASSES],
    pub tier_size: [u64; TIERS],
    pub tier_cost: [u64; TIERS],
    pub reset_ladder_bps: [u16; LADDER],
    pub spread_e6: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub markets: Vec<MarketOut>,
    pub ruleset: RuleSetOut,
}

fn fixed<T: Copy + Default, const N: usize>(v: &[T], what: &str) -> Result<[T; N], String> {
    if v.len() != N {
        return Err(format!("{what} needs {N} entries, has {}", v.len()));
    }
    let mut a = [T::default(); N];
    a.copy_from_slice(v);
    Ok(a)
}

pub fn ruleset(i: &Input) -> Result<RuleSetOut, String> {
    let rs = &i.ruleset;
    if rs.tier_sizes_usd.len() != rs.tier_costs_usd.len() || rs.tier_sizes_usd.is_empty() || rs.tier_sizes_usd.len() > TIERS {
        return Err("tier_sizes_usd and tier_costs_usd must have the same length, 1 to 10".into());
    }
    if rs.reset_ladder_bps.is_empty() || rs.reset_ladder_bps.len() > LADDER {
        return Err("reset_ladder_bps needs 1 to 5 entries".into());
    }
    let mut tier_size = [0u64; TIERS];
    let mut tier_cost = [0u64; TIERS];
    for (k, (s, c)) in rs.tier_sizes_usd.iter().zip(&rs.tier_costs_usd).enumerate() {
        tier_size[k] = s.checked_mul(USD).ok_or("tier size overflows")?;
        tier_cost[k] = c.checked_mul(USD).ok_or("tier cost overflows")?;
    }
    let mut ladder = [0u16; LADDER];
    ladder[..rs.reset_ladder_bps.len()].copy_from_slice(&rs.reset_ladder_bps);
    // commission: one entry per class, any order in the file
    let mut commission = [0u32; CLASSES];
    let mut seen = [false; CLASSES];
    for c in &i.commission {
        let k = usize::from(c.class);
        if k >= CLASSES || seen[k] {
            return Err(format!("commission entry for class {} is out of range or repeated", c.class));
        }
        seen[k] = true;
        commission[k] = c.e6;
    }
    if seen.iter().any(|s| !s) {
        return Err("commission needs one entry for each of the 8 classes".into());
    }
    if i.market.len() > MAX_MARKETS {
        return Err(format!("{} markets, the rule set indexes {MAX_MARKETS}", i.market.len()));
    }
    let mut spread = vec![0u16; MAX_MARKETS];
    for (id, m) in i.market.iter().enumerate() {
        spread[id] = m.half_spread_e6;
    }
    Ok(RuleSetOut {
        version: rs.version,
        fee_bps: rs.fee_bps,
        tier_count: rs.tier_sizes_usd.len() as u8,
        ladder_len: rs.reset_ladder_bps.len() as u8,
        leverage_x: fixed(&rs.leverage_x, "leverage_x")?,
        holding_fee_e6: fixed(&rs.holding_fee_e6, "holding_fee_e6")?,
        commission_e6: commission,
        tier_size,
        tier_cost,
        reset_ladder_bps: ladder,
        spread_e6: spread,
    })
}

pub fn market(i: &Input, id: usize, rs: &RuleSetOut) -> Result<MarketOut, String> {
    let m = &i.market[id];
    let leverage = *rs.leverage_x.get(usize::from(m.class)).ok_or_else(|| format!("{}: class {} is out of range", m.symbol, m.class))?;
    let pm = price_micro(m.price_usd).map_err(|e| format!("{}: {e}", m.symbol))?;
    let q = qty_min(pm, i.rules.order_notional_usd).map_err(|e| format!("{}: {e}", m.symbol))?;
    Ok(MarketOut {
        id: id as u16,
        class: m.class,
        status: STATUS_ENABLED,
        feed_id: feed_id(m.source, &m.symbol),
        max_age_secs: max_age_secs(m.freeze_ms, &i.rules).map_err(|e| format!("{}: {e}", m.symbol))?,
        qty_step: q,
        qty_min: q,
        max_notional_usd: cap_micro(m.class, leverage, &i.rules),
        sessions: session::windows(&m.session_variants).map_err(|e| format!("{}: {e}", m.symbol))?,
        quote_conv_market: NO_QUOTE_CONV,
        symbol: m.symbol.clone(),
        source: m.source,
    })
}

pub fn generate(i: &Input) -> Result<Launch, String> {
    if i.market.is_empty() {
        return Err("no markets".into());
    }
    let rs = ruleset(i)?;
    let markets = (0..i.market.len()).map(|id| market(i, id, &rs)).collect::<Result<Vec<_>, _>>()?;
    Ok(Launch { markets, ruleset: rs })
}
