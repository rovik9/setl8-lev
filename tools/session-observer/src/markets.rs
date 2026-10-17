//! The market list comes from `config/launch.json` (never hand-typed): symbol, source, class, sessions, max_age.
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Market {
    pub id: u16,
    pub symbol: String,
    pub source: u8,
    pub class: u8,
    pub max_age_secs: u16,
    /// Weekly windows in minutes of the UTC week, `[start, end)`.
    pub sessions: Vec<(u32, u32)>,
}

fn num(v: &Value) -> Result<u64, String> {
    v.as_str().ok_or_else(|| format!("{v} is not a decimal string"))?.parse::<u64>().map_err(|e| format!("{v}: {e}"))
}

/// Parses `launch.json` (every integer a decimal string).
pub fn parse_launch(text: &str) -> Result<Vec<Market>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("launch.json: {e}"))?;
    let list = v["markets"].as_array().ok_or("launch.json has no markets")?;
    let mut out = Vec::new();
    for m in list {
        let p = &m["params"];
        let used = num(&p["session_count"])? as usize;
        let mut sessions = Vec::new();
        for w in p["sessions"].as_array().ok_or("sessions")?.iter().take(used) {
            sessions.push((num(&w[0])? as u32, num(&w[1])? as u32));
        }
        out.push(Market {
            id: num(&m["market_id"])? as u16,
            symbol: m["meta"]["symbol"].as_str().ok_or("meta.symbol")?.to_string(),
            source: num(&m["meta"]["source"])? as u8,
            class: num(&m["class"])? as u8,
            max_age_secs: num(&p["max_age_secs"])? as u16,
            sessions,
        });
    }
    if out.is_empty() {
        return Err("launch.json has no markets".into());
    }
    Ok(out)
}

/// The Hyperliquid coins the observer subscribes to: every launch market of source 4, plus the twins that give the GMX
/// markets a second source (the same underlying on Hyperliquid).
pub fn hl_coins(markets: &[Market]) -> Vec<String> {
    let mut coins: Vec<String> = markets.iter().filter(|m| m.source == 4).map(|m| m.symbol.clone()).collect();
    for (_, hl) in HL_TWINS {
        if !coins.iter().any(|c| c == hl) {
            coins.push(hl.to_string());
        }
    }
    coins
}

/// GMX symbol and the Hyperliquid coin of the same underlying (the 14 to 20 bp question of oracle-sources.md). SPY and QQQ
/// are ETFs, SP500 and XYZ100 the indices: those two are compared as ratios.
pub const HL_TWINS: [(&str, &str); 9] = [
    ("BTC", "BTC"),
    ("ETH", "ETH"),
    ("SOL", "SOL"),
    ("GOLD", "xyz:GOLD"),
    ("SILVER", "xyz:SILVER"),
    ("WTIOIL", "xyz:CL"),
    ("BRENTOIL", "xyz:BRENTOIL"),
    ("SPY", "xyz:SP500"),
    ("QQQ", "xyz:XYZ100"),
];
/// Launch symbol and the public reference product (Coinbase Exchange spot ticker). Pyth's price endpoints need an API key
/// (HTTP 401 without one, checked 2026-10-17), so Pyth contributes only its published market hours.
pub const REFERENCES: [(&str, &str); 4] = [("BTC", "BTC-USD"), ("ETH", "ETH-USD"), ("SOL", "SOL-USD"), ("GOLD", "PAXG-USD")];

/// Launch symbol and the exact Pyth symbol(s) whose published market hours are recorded.
pub const PYTH_SYMBOLS: [(&str, &str); 24] = [
    ("BTC", "Crypto.BTC/USD"),
    ("ETH", "Crypto.ETH/USD"),
    ("SOL", "Crypto.SOL/USD"),
    ("GOLD", "Metal.XAU/USD"),
    ("SILVER", "Metal.XAG/USD"),
    ("WTIOIL", "Commodities.Index.WTI1M/USD"),
    ("BRENTOIL", "Commodities.BRENTZ6/USD"),
    ("BRENTOIL", "Commodities.Index.BRENT/USD"),
    ("SPY", "Equity.US.SPY/USD"),
    ("QQQ", "Equity.US.QQQ/USD"),
    ("xyz:JP225", "Equity.Index.JP225/JPY"),
    ("xyz:KR200", "Equity.Index.KR200/KRW"),
    ("xyz:AAPL", "Equity.US.AAPL/USD"),
    ("xyz:AMD", "Equity.US.AMD/USD"),
    ("xyz:AMZN", "Equity.US.AMZN/USD"),
    ("xyz:GOOGL", "Equity.US.GOOGL/USD"),
    ("xyz:META", "Equity.US.META/USD"),
    ("xyz:MSFT", "Equity.US.MSFT/USD"),
    ("xyz:NFLX", "Equity.US.NFLX/USD"),
    ("xyz:NVDA", "Equity.US.NVDA/USD"),
    ("xyz:PLTR", "Equity.US.PLTR/USD"),
    ("xyz:TSLA", "Equity.US.TSLA/USD"),
    ("xyz:SP500", "Equity.Index.US500/USD"),
    ("xyz:XYZ100", "Equity.Index.NAS100/USD"),
];
