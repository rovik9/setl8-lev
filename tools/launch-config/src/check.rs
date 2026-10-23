//! Every rule `launch-config verify` enforces. Each function returns the violations it found as plain sentences, so a test
//! can assert the exact sentence of a failing fixture.
use crate::derive::*;
use crate::emit;
use crate::model::*;
use crate::probe;
use std::collections::BTreeSet;

/// GMX tickers (source 2): symbol and class (decision 55).
pub const GMX_ALLOWED: [(&str, u8); 9] = [("BTC", 6), ("ETH", 6), ("SOL", 6), ("GOLD", 2), ("SILVER", 2), ("WTIOIL", 4), ("BRENTOIL", 4), ("SPY", 3), ("QQQ", 3)];
/// Hyperliquid xyz oracle (source 4): the two indices and the twelve liquid single stocks the probe followed (decisions 55, 56).
pub const HL_ALLOWED: [(&str, u8); 14] = [
    ("xyz:JP225", 3),
    ("xyz:KR200", 3),
    ("xyz:AAPL", 5),
    ("xyz:AMD", 5),
    ("xyz:AMZN", 5),
    ("xyz:COIN", 5),
    ("xyz:GOOGL", 5),
    ("xyz:META", 5),
    ("xyz:MSFT", 5),
    ("xyz:MSTR", 5),
    ("xyz:NFLX", 5),
    ("xyz:NVDA", 5),
    ("xyz:PLTR", 5),
    ("xyz:TSLA", 5),
];
/// Never at launch (decision 56), whatever else says so. Checked by name before the allow-lists.
pub const FORBIDDEN: [&str; 14] =
    ["xyz:EUR", "xyz:GBP", "xyz:JPY", "xyz:KRW", "NATGAS", "xyz:NATGAS", "xyz:HO", "xyz:OURA", "xyz:CAMBRICON", "xyz:TWST", "xyz:UMC", "xyz:SP500", "xyz:XYZ100", "xyz:GLW"];

/// The launch values of the rule set (DESIGN 5.4; decisions 13, 15, 17, 26).
pub const LAUNCH_FEE_BPS: u16 = 40;
pub const LAUNCH_TIER_SIZES_USD: [u64; 10] = [2_500, 5_000, 10_000, 15_000, 20_000, 25_000, 50_000, 75_000, 100_000, 200_000];
pub const LAUNCH_TIER_COSTS_USD: [u64; 10] = [30, 55, 99, 145, 189, 235, 489, 689, 859, 1_599];
pub const LAUNCH_LADDER_BPS: [u16; 5] = [10_000, 11_500, 12_000, 12_750, 13_000];
pub const LAUNCH_LEVERAGE: [u16; 8] = [50, 20, 20, 20, 10, 5, 5, 2];
pub const LAUNCH_HOLDING_E6: [u32; 8] = [100, 200, 400, 200, 400, 400, 1000, 1600];

fn allowed(source: u8, symbol: &str) -> Option<u8> {
    let list: &[(&str, u8)] = match source {
        2 => &GMX_ALLOWED,
        4 => &HL_ALLOWED,
        _ => return None,
    };
    list.iter().find(|(s, _)| *s == symbol).map(|(_, c)| *c)
}

/// Rule 1: markets and sources follow decisions 55 and 56; never two markets for one underlying.
pub fn sources(i: &Input) -> Vec<String> {
    let mut v = Vec::new();
    let (mut symbols, mut underlyings, mut feeds) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for m in &i.market {
        if FORBIDDEN.contains(&m.symbol.as_str()) {
            v.push(format!("{}: not at launch (decision 56)", m.symbol));
            continue;
        }
        match allowed(m.source, &m.symbol) {
            None => v.push(format!("{}: not an allowed symbol for source {} (decisions 55 and 56)", m.symbol, m.source)),
            Some(c) if c != m.class => v.push(format!("{}: class {} but decision 55 says class {c}", m.symbol, m.class)),
            Some(_) => {}
        }
        if !symbols.insert(m.symbol.clone()) {
            v.push(format!("{}: two markets with this symbol", m.symbol));
        }
        if !underlyings.insert(m.underlying.clone()) {
            v.push(format!("{}: a second market on the underlying {:?}", m.symbol, m.underlying));
        }
        if !feeds.insert(feed_id(m.source, &m.symbol)) {
            v.push(format!("{}: duplicate feed id", m.symbol));
        }
    }
    for e in &i.excluded {
        if i.market.iter().any(|m| m.symbol == e.symbol) {
            v.push(format!("{}: listed as a market and as excluded", e.symbol));
        }
    }
    v
}

/// Rule 1 (go-live) for the Hyperliquid oracle markets, and the numbers of every excluded candidate that claims to fail it.
pub fn golive(i: &Input) -> Vec<String> {
    let mut v = Vec::new();
    let r = &i.rules;
    for m in i.market.iter().filter(|m| m.source == 4) {
        let max_age = max_age_secs(m.freeze_ms, r).unwrap_or(0);
        match (m.oracle_median_bp, m.oracle_p95_bp) {
            (Some(med), Some(p95)) => {
                if med > r.golive_median_bp {
                    v.push(format!("{}: median |oracle - mid| {med:.2} bp is above {} bp (decision 56)", m.symbol, r.golive_median_bp));
                }
                if p95 > r.golive_p95_bp {
                    v.push(format!("{}: p95 |oracle - mid| {p95:.2} bp is above {} bp (decision 56)", m.symbol, r.golive_p95_bp));
                }
            }
            _ => v.push(format!("{}: a Hyperliquid market needs oracle_median_bp and oracle_p95_bp (decision 56)", m.symbol)),
        }
        if m.freeze_ms > u64::from(max_age) * 1000 {
            v.push(format!("{}: longest freeze {} ms is longer than max_age_secs {max_age}", m.symbol, m.freeze_ms));
        }
    }
    for e in &i.excluded {
        let fails = match e.rule.as_str() {
            "median" => e.median_bp.is_some_and(|x| x > r.golive_median_bp),
            "p95" => e.p95_bp.is_some_and(|x| x > r.golive_p95_bp),
            "decision-56" | "liquidity" | "not-at-launch" => true,
            other => {
                v.push(format!("{}: unknown exclusion rule {other:?}", e.symbol));
                continue;
            }
        };
        if !fails {
            v.push(format!("{}: excluded for rule {:?} but its number passes that rule", e.symbol, e.rule));
        }
    }
    v
}

/// Rule 3: the sessions.
pub fn sessions(i: &Input, l: &Launch) -> Vec<String> {
    let mut v = Vec::new();
    for (m, o) in i.market.iter().zip(&l.markets) {
        let always = o.sessions == [(0, crate::session::MINUTES_PER_WEEK)];
        if m.class == 6 && !always {
            v.push(format!("{}: crypto trades 24/7 and has no closed window", m.symbol));
        }
        if m.class != 6 && always {
            v.push(format!("{}: a non-crypto market must have its real trading windows, not 24/7", m.symbol));
        }
        if !always && m.verify.is_empty() {
            v.push(format!("{}: a boundary the data does not settle must carry a VERIFY flag", m.symbol));
        }
        if o.sessions.is_empty() || o.sessions.len() > crate::session::MAX_SESSIONS {
            v.push(format!("{}: {} session windows", m.symbol, o.sessions.len()));
        }
    }
    v
}

/// Rules 4 and 5 and the shape of the rule set: placeholders follow their formulas, the launch values are the launch values,
/// and no market is unpriced.
pub fn numbers(i: &Input, l: &Launch) -> Vec<String> {
    let mut v = Vec::new();
    let r = &i.rules;
    for (m, o) in i.market.iter().zip(&l.markets) {
        if o.qty_step != o.qty_min || o.qty_min == 0 {
            v.push(format!("{}: quantity step and minimum must be equal and positive", m.symbol));
        }
        let ceiling = if m.class == 5 { r.cap_ceiling_stock_usd } else { r.cap_ceiling_usd };
        if o.max_notional_usd == 0 || o.max_notional_usd > ceiling * 1_000_000 {
            v.push(format!("{}: max_notional_usd {} is zero or above the ceiling {ceiling} USD", m.symbol, o.max_notional_usd));
        }
        match l.ruleset.spread_e6.get(usize::from(o.id)) {
            Some(0) | None => v.push(format!("{}: half-spread 0 in the rule set: an unpriced market (it would trade at the mid)", m.symbol)),
            Some(_) => {}
        }
    }
    let rs = &i.ruleset;
    if rs.fee_bps != LAUNCH_FEE_BPS
        || rs.tier_sizes_usd != LAUNCH_TIER_SIZES_USD
        || rs.tier_costs_usd != LAUNCH_TIER_COSTS_USD
        || rs.reset_ladder_bps != LAUNCH_LADDER_BPS
        || rs.leverage_x != LAUNCH_LEVERAGE
        || rs.holding_fee_e6 != LAUNCH_HOLDING_E6
    {
        v.push("the rule set differs from the launch values of DESIGN 5.4 (fee 40, tiers, ladder, leverage, holding fee)".into());
    }
    if l.ruleset.spread_e6.iter().skip(i.market.len()).any(|s| *s != 0) {
        v.push("a half-spread is set for a market id that does not exist".into());
    }
    v
}

/// Everything except the comparison with the committed files.
pub fn all(i: &Input, l: &Launch, production: bool) -> Vec<String> {
    let mut v = sources(i);
    v.extend(golive(i));
    v.extend(sessions(i, l));
    v.extend(numbers(i, l));
    if production {
        let n = emit::pending_flags(i);
        if n > 0 {
            let spreads = i.market.iter().filter(|m| m.spread_pending).count();
            v.push(format!("{n} PENDING flags remain ({spreads} market spreads, {} class commissions): the rule set numbers are placeholders or not final (decisions 14 and 72); not for production", n - spreads));
        }
    }
    v
}

/// The committed `launch.json` and `launch.md` must be exactly what the input generates.
pub fn files(i: &Input, l: &Launch, json: &str, md: &str) -> Vec<String> {
    let mut v = Vec::new();
    if emit::json(l) != json {
        v.push("config/launch.json differs from what config/launch-markets.toml generates (run `launch-config generate`)".into());
    }
    if emit::markdown(i, l) != md {
        v.push("config/launch.md differs from what config/launch-markets.toml generates (run `launch-config generate`)".into());
    }
    v
}

/// Compare the numbers committed in the input with the raw probe data. `hl` is `oracle-messages.csv`, `gmx_1min` the
/// `gmx-tickers-1min.csv` of the same run, `gmx_samples` the Saturday `samples-tracked-all-polls.csv`.
pub fn against_probe_data(i: &Input, hl: Option<&str>, gmx_1min: Option<&str>, gmx_samples: Option<&str>) -> Result<Vec<String>, String> {
    let mut v = Vec::new();
    let hl = hl.map(probe::parse_hl_oracle).transpose()?;
    let gmx = gmx_1min.map(probe::gmx_price_medians).transpose()?;
    let same6 = |a: f64, b: f64| format!("{a:.6}") == format!("{b:.6}");
    for m in &i.market {
        if m.source == 4 {
            let Some(series) = hl.as_ref().and_then(|h| h.get(&m.symbol)) else { continue };
            let s = probe::oracle_stats(series).ok_or_else(|| format!("{}: no usable messages", m.symbol))?;
            if m.freeze_ms != s.freeze_ms {
                v.push(format!("{}: freeze_ms {} in the input, {} in the data", m.symbol, m.freeze_ms, s.freeze_ms));
            }
            for (name, have, got) in [("oracle_median_bp", m.oracle_median_bp, s.median_bp), ("oracle_p95_bp", m.oracle_p95_bp, s.p95_bp)] {
                if have.map(|x| format!("{x:.2}")) != Some(format!("{got:.2}")) {
                    v.push(format!("{}: {name} {have:?} in the input, {got:.2} in the data", m.symbol));
                }
            }
            if !same6(m.price_usd, s.price_median) {
                v.push(format!("{}: price_usd {} in the input, {} in the data", m.symbol, m.price_usd, s.price_median));
            }
        } else if let Some(p) = gmx.as_ref().and_then(|g| g.get(&m.symbol)) {
            if !same6(m.price_usd, *p) {
                v.push(format!("{}: price_usd {} in the input, {p} in the data", m.symbol, m.price_usd));
            }
        }
        // the Saturday samples are in-session only for the 24/7 crypto markets; a closed metal's Saturday gap is not a freeze
        if let (2, 6, Some(csv)) = (m.source, m.class, gmx_samples) {
            if let Some(gap) = probe::gmx_longest_gap_ms(csv, &m.symbol)? {
                if m.freeze_ms < gap {
                    v.push(format!("{}: freeze_ms {} is below the {gap} ms gap in the samples", m.symbol, m.freeze_ms));
                }
            }
        }
    }
    for e in &i.excluded {
        let Some(series) = hl.as_ref().and_then(|h| h.get(&e.symbol)) else { continue };
        let Some(s) = probe::oracle_stats(series) else { continue };
        for (name, have, got) in [("median_bp", e.median_bp, s.median_bp), ("p95_bp", e.p95_bp, s.p95_bp)] {
            if let Some(x) = have {
                if format!("{x:.2}") != format!("{got:.2}") {
                    v.push(format!("{} (excluded): {name} {x:.2} in the input, {got:.2} in the data", e.symbol));
                }
            }
        }
    }
    Ok(v)
}
