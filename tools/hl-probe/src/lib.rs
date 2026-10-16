//! Pure helpers of the Hyperliquid probe (floats are fine here: this is a measurement tool, not the engine).

/// Prices are stored as integers scaled by 1e12.
pub const PRICE_SCALE: u32 = 12;

/// Parses a decimal string like `1234.5678` into an integer scaled by `10^decimals` (extra digits are truncated).
pub fn parse_scaled(s: &str, decimals: u32) -> Option<u128> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('-') || s.contains(['e', 'E']) {
        return None;
    }
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) || !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut frac: String = frac_part.chars().take(decimals as usize).collect();
    while frac.len() < decimals as usize {
        frac.push('0');
    }
    let int: u128 = if int_part.is_empty() { 0 } else { int_part.parse().ok()? };
    int.checked_mul(10u128.checked_pow(decimals)?)?.checked_add(frac.parse::<u128>().ok()?)
}

/// `(a - b) / b` in basis points (signed).
pub fn gap_bps(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return f64::INFINITY;
    }
    (a - b) / b * 10_000.0
}

/// Top-of-book spread in basis points of the mid: `(ask - bid) / mid x 10_000`.
pub fn spread_bps(bid: f64, ask: f64) -> f64 {
    let mid = (bid + ask) / 2.0;
    if mid <= 0.0 {
        return f64::INFINITY;
    }
    (ask - bid) / mid * 10_000.0
}

pub fn median(values: &[f64]) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Nearest-rank percentile of an ascending slice (`p` in 0..=100).
pub fn percentile(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.clamp(1, sorted.len()) - 1).copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapStats {
    pub gaps: usize,
    pub median: u64,
    pub p95: u64,
    pub max: u64,
}

/// Statistics of the gaps between consecutive entries of `times_ms` (sorted first). `None` below 2 entries.
pub fn gap_stats(times_ms: &[u64]) -> Option<GapStats> {
    if times_ms.len() < 2 {
        return None;
    }
    let mut t = times_ms.to_vec();
    t.sort_unstable();
    let mut gaps: Vec<u64> = t.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_unstable();
    Some(GapStats { gaps: gaps.len(), median: percentile(&gaps, 50.0)?, p95: percentile(&gaps, 95.0)?, max: *gaps.last()? })
}

/// Coin classes (a GUESS from the ticker; the API carries no class field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Stock,
    Etf,
    Index,
    Fx,
    Commodity,
    Crypto,
    Other,
}

impl Class {
    pub fn name(&self) -> &'static str {
        match self {
            Class::Stock => "stock",
            Class::Etf => "ETF",
            Class::Index => "index",
            Class::Fx => "FX",
            Class::Commodity => "commodity",
            Class::Crypto => "crypto",
            Class::Other => "other",
        }
    }
}

pub const ALL_CLASSES: [Class; 7] = [Class::Stock, Class::Etf, Class::Index, Class::Fx, Class::Commodity, Class::Crypto, Class::Other];

const INDICES: &[&str] = &["XYZ100", "SP500", "JP225", "KR200", "NIFTY", "IBOV", "VIX", "DXY", "US500", "USA500", "USA100", "USTECH", "SMALL2000", "JPN225", "BVIV", "TOTAL2", "OTHERS", "BTCD", "MAG7", "SEMIS", "ROBOT", "INFOTECH", "NUCLEAR", "DEFENSE", "ENERGY", "BIOTECH", "SEMI", "USENERGY", "USBOND", "10Y", "30Y", "2Y"];
const FX: &[&str] = &["EUR", "GBP", "JPY", "KRW", "CHF", "AUD", "CAD", "CNH"];
const COMMODITIES: &[&str] = &["GOLD", "SILVER", "COPPER", "PLATINUM", "PALLADIUM", "ALUMINIUM", "CL", "OIL", "BRENTOIL", "NATGAS", "GAS", "HO", "TTF", "CORN", "WHEAT", "SOY", "URANIUM", "USOIL", "GOLDJM", "SILVERJM"];
const ETFS: &[&str] = &["EWY", "EWJ", "EWZ", "EWT", "XLE", "SMH", "TLT", "XBI", "SOXL", "MAGS", "KORU", "URNM", "GDX", "IGV", "GLDMINE", "DRV", "SNXX"];
const OTHERS: &[&str] = &["H100", "VOL", "PURRDAT", "SPCX", "UNITREE", "CXMT", "YMTC", "SHEIN", "ZHIPU", "MINIMAX", "GIGADEV", "SHAZ", "BIRD", "KSTR", "STRC", "SPACEX", "OPENAI", "ANTHROPIC", "ANTH", "OAI", "USDE", "ANSEM", "TREAD", "BASED", "LIGHTER"];

/// Splits `dex:COIN` into `(dex, COIN)`; the main dex has no prefix.
pub fn split_coin(name: &str) -> (&str, &str) {
    match name.split_once(':') {
        Some((d, c)) => (d, c),
        None => ("", name),
    }
}

/// Classification guess. The main dex (no prefix) is crypto unless the base is a known non-crypto name; builder dexes
/// default to `Stock` for plain tickers and to `Crypto` on the crypto-only dex `hyna`.
pub fn classify(name: &str) -> Class {
    let (dex, base) = split_coin(name);
    if INDICES.contains(&base) {
        Class::Index
    } else if FX.contains(&base) {
        Class::Fx
    } else if COMMODITIES.contains(&base) {
        Class::Commodity
    } else if ETFS.contains(&base) {
        Class::Etf
    } else if OTHERS.contains(&base) {
        Class::Other
    } else if dex.is_empty() || dex == "hyna" {
        Class::Crypto
    } else if base.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        Class::Stock
    } else {
        Class::Other
    }
}

// ------------------------------------------------------------------ sample files (DESIGN 4.5 shape)

/// One archived sample: market index, time (ms), bid, ask and mid as integers scaled by 1e12 (0 = not given).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub market_idx: u16,
    pub time_ms: u64,
    pub bid: u128,
    pub ask: u128,
    pub mid: u128,
}

pub const RECORD_BYTES: usize = 64;

impl Record {
    /// Mid from bid and ask (rounded down).
    pub fn from_bbo(market_idx: u16, time_ms: u64, bid: u128, ask: u128) -> Self {
        Record { market_idx, time_ms, bid, ask, mid: bid / 2 + ask / 2 + (bid % 2 + ask % 2) / 2 }
    }

    /// A mid-only sample (allMids): bid and ask are 0.
    pub fn mid_only(market_idx: u16, time_ms: u64, mid: u128) -> Self {
        Record { market_idx, time_ms, bid: 0, ask: 0, mid }
    }

    /// 64 bytes little endian: idx u16, 6 zero bytes, time u64, bid u128, ask u128, mid u128.
    pub fn to_bytes(&self) -> [u8; RECORD_BYTES] {
        let mut b = [0u8; RECORD_BYTES];
        b[0..2].copy_from_slice(&self.market_idx.to_le_bytes());
        b[8..16].copy_from_slice(&self.time_ms.to_le_bytes());
        b[16..32].copy_from_slice(&self.bid.to_le_bytes());
        b[32..48].copy_from_slice(&self.ask.to_le_bytes());
        b[48..64].copy_from_slice(&self.mid.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; RECORD_BYTES]) -> Self {
        let u128_at = |o: usize| {
            let mut a = [0u8; 16];
            a.copy_from_slice(&b[o..o + 16]);
            u128::from_le_bytes(a)
        };
        let mut idx = [0u8; 2];
        idx.copy_from_slice(&b[0..2]);
        let mut t = [0u8; 8];
        t.copy_from_slice(&b[8..16]);
        Record { market_idx: u16::from_le_bytes(idx), time_ms: u64::from_le_bytes(t), bid: u128_at(16), ask: u128_at(32), mid: u128_at(48) }
    }
}

pub fn read_records(bytes: &[u8]) -> Option<Vec<Record>> {
    if bytes.len() % RECORD_BYTES != 0 {
        return None;
    }
    Some(
        bytes
            .chunks_exact(RECORD_BYTES)
            .map(|c| {
                let mut a = [0u8; RECORD_BYTES];
                a.copy_from_slice(c);
                Record::from_bytes(&a)
            })
            .collect(),
    )
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Missing minutes in an ascending list of candle open times (ms, 60,000 apart when complete): total and the gaps.
pub fn minute_gaps_ms(ts_ascending: &[u64]) -> (u64, Vec<(u64, u64, u64)>) {
    let mut total = 0;
    let mut gaps = Vec::new();
    for w in ts_ascending.windows(2) {
        let d = w[1] - w[0];
        if d > 60_000 {
            let missing = d / 60_000 - 1;
            total += missing;
            gaps.push((w[0], w[1], missing));
        }
    }
    (total, gaps)
}

/// Direction of an FX coin from its level: what the price is quoted in.
pub fn fx_direction(coin_base: &str, level: f64) -> &'static str {
    match coin_base {
        "EUR" | "GBP" | "AUD" => {
            if (0.3..5.0).contains(&level) { "USD per 1 foreign unit (base = foreign currency)" } else if level > 0.0 && level < 0.3 { "unclear" } else { "unclear" }
        }
        "JPY" | "KRW" | "CNH" | "CHF" | "CAD" => {
            if level > 0.0 && level < 0.1 { "USD per 1 foreign unit" } else if level > 20.0 { "foreign units per 1 USD" } else { "unclear" }
        }
        _ => "unclear",
    }
}

// ------------------------------------------------------------------ oracle measurements (P11)

/// One observation of a coin's context: receive time (ms), oracle price, mark price and the book mid when present.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OraclePoint {
    pub ms: u64,
    pub oracle: f64,
    pub mark: f64,
    pub mid: Option<f64>,
}

/// `|a - b| / b` in basis points (absolute value of `gap_bps`).
pub fn abs_gap_bps(a: f64, b: f64) -> f64 {
    gap_bps(a, b).abs()
}

/// Receive times at which the oracle price differs from the previous observation (the first observation is not a change).
pub fn oracle_change_times(series: &[OraclePoint]) -> Vec<u64> {
    series.windows(2).filter(|w| w[1].oracle != w[0].oracle).map(|w| w[1].ms).collect()
}

/// Sizes of the oracle jumps in basis points (absolute), one per change.
pub fn oracle_jumps_bps(series: &[OraclePoint]) -> Vec<f64> {
    series.windows(2).filter(|w| w[1].oracle != w[0].oracle).map(|w| abs_gap_bps(w[1].oracle, w[0].oracle)).collect()
}

/// The longest stretch without an oracle change (from the change that started it, or the first observation, to the next
/// change or the last observation) together with the number of times the book mid changed inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frozen {
    pub start_ms: u64,
    pub duration_ms: u64,
    pub mid_changes: u32,
}

/// Longest oracle-frozen stretch in which the mid changed at least `min_mid_changes` times. `None` if there is none.
pub fn longest_frozen_while_moving(series: &[OraclePoint], min_mid_changes: u32) -> Option<Frozen> {
    let first = series.first()?;
    let mut best: Option<Frozen> = None;
    let consider = |start: u64, end: u64, mids: u32, best: &mut Option<Frozen>| {
        if mids >= min_mid_changes && best.map_or(true, |b| end - start > b.duration_ms) {
            *best = Some(Frozen { start_ms: start, duration_ms: end - start, mid_changes: mids });
        }
    };
    let (mut start, mut mids) = (first.ms, 0u32);
    for w in series.windows(2) {
        if w[1].oracle != w[0].oracle {
            consider(start, w[1].ms, mids, &mut best);
            start = w[1].ms;
            mids = 0;
        } else if let (Some(a), Some(b)) = (w[0].mid, w[1].mid) {
            if a != b {
                mids += 1;
            }
        }
    }
    if let Some(last) = series.last() {
        consider(start, last.ms, mids, &mut best);
    }
    best
}

/// Episodes (start ms, duration ms) during which the book mid is more than `threshold_bp` away from the oracle price: the
/// time the oracle "lags" the book. An episode ends at the first observation back inside the threshold (its duration runs to
/// that observation); an episode still open at the last observation is closed there. Observations without a mid are skipped.
pub fn lag_episodes(series: &[OraclePoint], threshold_bp: f64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut open: Option<u64> = None;
    for p in series {
        // an observation without a mid says nothing about the book: it neither opens nor closes an episode
        let Some(m) = p.mid else { continue };
        let off = abs_gap_bps(m, p.oracle) > threshold_bp;
        match (open, off) {
            (None, true) => open = Some(p.ms),
            (Some(s), false) => {
                out.push((s, p.ms - s));
                open = None;
            }
            _ => {}
        }
    }
    if let (Some(s), Some(last)) = (open, series.last()) {
        out.push((s, last.ms - s));
    }
    out
}

/// The value of the latest observation at or before `t_ms` (`None` if there is none yet).
pub fn value_at(series: &[OraclePoint], t_ms: u64) -> Option<&OraclePoint> {
    let idx = series.partition_point(|p| p.ms <= t_ms);
    idx.checked_sub(1).and_then(|i| series.get(i))
}

/// Reporting class used in the oracle report: crypto (no dex prefix), stock, ETF, index, FX, metal, energy, other.
pub fn report_class(name: &str) -> &'static str {
    let (_, base) = split_coin(name);
    match classify(name) {
        Class::Commodity => {
            if ["CL", "BRENTOIL", "NATGAS", "HO", "TTF"].contains(&base) {
                "energy"
            } else {
                "metal"
            }
        }
        Class::Stock => "stock",
        Class::Etf => "ETF",
        Class::Index => "index",
        Class::Fx => "FX",
        Class::Crypto => "crypto",
        Class::Other => "other",
    }
}

/// Civil date `YYYY-MM-DD` (UTC) of a Unix time in milliseconds.
pub fn utc_date(ms: u64) -> String {
    let days = (ms / 1000 / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Statistics of a list of values: (median, 95th percentile nearest-rank, max). `None` if empty.
pub fn med_p95_max(values: &[f64]) -> Option<(f64, f64, f64)> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((0.95 * v.len() as f64).ceil() as usize).clamp(1, v.len());
    Some((median(&v)?, v[rank - 1], *v.last()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_decimal_parsing() {
        assert_eq!(parse_scaled("1", 12), Some(1_000_000_000_000));
        assert_eq!(parse_scaled("1.5", 12), Some(1_500_000_000_000));
        assert_eq!(parse_scaled("0.000000000001", 12), Some(1));
        assert_eq!(parse_scaled("0.0000000000019", 12), Some(1), "extra digits truncate");
        assert_eq!(parse_scaled("67250.123456789012", 12), Some(67_250_123_456_789_012));
        assert_eq!(parse_scaled(".5", 12), Some(500_000_000_000));
        assert_eq!(parse_scaled("5.", 12), Some(5_000_000_000_000));
        assert_eq!(parse_scaled("", 12), None);
        assert_eq!(parse_scaled("-1", 12), None);
        assert_eq!(parse_scaled("1e5", 12), None);
        assert_eq!(parse_scaled("1.2.3", 12), None);
        assert_eq!(parse_scaled("abc", 12), None);
    }

    #[test]
    fn bps_helpers() {
        assert!((gap_bps(101.0, 100.0) - 100.0).abs() < 1e-9);
        assert!((gap_bps(99.0, 100.0) + 100.0).abs() < 1e-9);
        assert!(gap_bps(1.0, 0.0).is_infinite());
        assert!((spread_bps(99.0, 101.0) - 200.0).abs() < 1e-9);
        assert!(spread_bps(0.0, 0.0).is_infinite());
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[f64::NAN, 5.0]), Some(5.0));
    }

    #[test]
    fn gap_statistics() {
        let g = gap_stats(&[0, 100, 300, 600, 1000]).unwrap();
        assert_eq!(g, GapStats { gaps: 4, median: 200, p95: 400, max: 400 });
        assert_eq!(gap_stats(&[1000, 0, 600, 100, 300]), Some(g));
        assert_eq!(gap_stats(&[5]), None);
        assert_eq!(percentile(&(1..=100).collect::<Vec<u64>>(), 95.0), Some(95));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn classification_guesses() {
        assert_eq!(classify("xyz:AAPL"), Class::Stock);
        assert_eq!(classify("xyz:NVDA"), Class::Stock);
        assert_eq!(classify("xyz:SP500"), Class::Index);
        assert_eq!(classify("xyz:XYZ100"), Class::Index);
        assert_eq!(classify("xyz:JP225"), Class::Index);
        assert_eq!(classify("xyz:EUR"), Class::Fx);
        assert_eq!(classify("xyz:KRW"), Class::Fx);
        assert_eq!(classify("xyz:GOLD"), Class::Commodity);
        assert_eq!(classify("xyz:CL"), Class::Commodity);
        assert_eq!(classify("xyz:BRENTOIL"), Class::Commodity);
        assert_eq!(classify("xyz:EWY"), Class::Etf);
        assert_eq!(classify("xyz:H100"), Class::Other);
        assert_eq!(classify("BTC"), Class::Crypto);
        assert_eq!(classify("kPEPE"), Class::Crypto);
        assert_eq!(classify("hyna:SOL"), Class::Crypto);
        assert_eq!(classify("km:AAPL"), Class::Stock);
        assert_eq!(split_coin("xyz:AAPL"), ("xyz", "AAPL"));
        assert_eq!(split_coin("BTC"), ("", "BTC"));
        assert_eq!(Class::Fx.name(), "FX");
    }

    #[test]
    fn sample_file_round_trip() {
        let r = Record::from_bbo(7, 1_791_657_944_312, 229_950_000_000_000, 230_050_000_000_000);
        assert_eq!(r.mid, 230_000_000_000_000);
        assert_eq!(Record::from_bytes(&r.to_bytes()), r);
        let m = Record::mid_only(3, 5, 123);
        assert_eq!((m.bid, m.ask, m.mid), (0, 0, 123));
        let rs: Vec<Record> = (0..10).map(|i| Record::from_bbo(i, 1_000 + u64::from(i), u128::from(i) * 5 + 1, u128::from(i) * 5 + 3)).collect();
        let mut file = Vec::new();
        for r in &rs {
            file.extend_from_slice(&r.to_bytes());
        }
        assert_eq!(read_records(&file).unwrap(), rs);
        assert_eq!(read_records(&file[..file.len() - 1]), None);
        assert_eq!(read_records(&[]).unwrap(), vec![]);
        assert_eq!(Record::from_bbo(0, 1, u128::MAX, u128::MAX).mid, u128::MAX);
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn candle_gaps_in_ms() {
        let (t, g) = minute_gaps_ms(&[0, 60_000, 180_000, 240_000, 540_000]);
        assert_eq!(t, 1 + 4);
        assert_eq!(g, vec![(60_000, 180_000, 1), (240_000, 540_000, 4)]);
        assert_eq!(minute_gaps_ms(&[0, 60_000]), (0, vec![]));
    }

    #[test]
    fn fx_direction_from_level() {
        assert_eq!(fx_direction("EUR", 1.1), "USD per 1 foreign unit (base = foreign currency)");
        assert_eq!(fx_direction("JPY", 0.0065), "USD per 1 foreign unit");
        assert_eq!(fx_direction("JPY", 150.0), "foreign units per 1 USD");
        assert_eq!(fx_direction("KRW", 0.0007), "USD per 1 foreign unit");
        assert_eq!(fx_direction("KRW", 1400.0), "foreign units per 1 USD");
        assert_eq!(fx_direction("XXX", 1.0), "unclear");
    }

    fn pt(ms: u64, oracle: f64, mid: Option<f64>) -> OraclePoint {
        OraclePoint { ms, oracle, mark: oracle, mid }
    }

    #[test]
    fn oracle_changes_and_jumps() {
        let s = [pt(0, 100.0, None), pt(1_000, 100.0, None), pt(2_000, 101.0, None), pt(3_000, 101.0, None), pt(9_000, 100.0, None)];
        assert_eq!(oracle_change_times(&s), vec![2_000, 9_000]);
        let j = oracle_jumps_bps(&s);
        assert_eq!(j.len(), 2);
        assert!((j[0] - 100.0).abs() < 1e-9);
        assert!((j[1] - 99.0099009901).abs() < 1e-6, "{}", j[1]);
        assert!(oracle_change_times(&[]).is_empty());
        assert!(oracle_change_times(&s[..1]).is_empty());
        assert_eq!(gap_stats(&oracle_change_times(&s)).map(|g| g.max), Some(7_000));
    }

    #[test]
    fn frozen_oracle_while_the_mid_moves() {
        // oracle changes at 5 s and 65 s; between them the mid changes 3 times; after 65 s it moves once until 70 s
        let s = [
            pt(0, 10.0, Some(10.0)),
            pt(5_000, 10.5, Some(10.0)),
            pt(20_000, 10.5, Some(10.1)),
            pt(30_000, 10.5, Some(10.2)),
            pt(31_000, 10.5, Some(10.2)),
            pt(60_000, 10.5, Some(10.3)),
            pt(65_000, 10.6, Some(10.3)),
            pt(70_000, 10.6, Some(10.4)),
        ];
        let f = longest_frozen_while_moving(&s, 1).unwrap();
        assert_eq!(f, Frozen { start_ms: 5_000, duration_ms: 60_000, mid_changes: 3 });
        // demanding four mid changes leaves nothing
        assert_eq!(longest_frozen_while_moving(&s, 4), None);
        // a frozen oracle with a still mid is not "while moving"
        let still = [pt(0, 1.0, Some(5.0)), pt(100_000, 1.0, Some(5.0))];
        assert_eq!(longest_frozen_while_moving(&still, 1), None);
        assert_eq!(longest_frozen_while_moving(&still, 0), Some(Frozen { start_ms: 0, duration_ms: 100_000, mid_changes: 0 }));
        assert_eq!(longest_frozen_while_moving(&[], 0), None);
    }

    #[test]
    fn lag_episodes_of_the_oracle() {
        // 10 bp of 100.0 is 0.1: the mid at 100.2 is 20 bp away
        let s = [
            pt(0, 100.0, Some(100.0)),
            pt(1_000, 100.0, Some(100.2)),
            pt(4_000, 100.0, Some(100.3)),
            pt(6_000, 100.25, Some(100.3)),
            pt(8_000, 100.25, Some(100.0)),
            pt(9_000, 100.25, None),
            pt(10_000, 100.25, Some(100.0)),
        ];
        // episode 1: from 1 s until the oracle moved at 6 s (back inside 10 bp), 5 s; episode 2: from 8 s (25 bp) until 10 s
        assert_eq!(lag_episodes(&s, 10.0), vec![(1_000, 5_000), (8_000, 2_000)]);
        // an episode open at the end is closed at the last observation
        let open = [pt(0, 1.0, Some(1.0)), pt(1_000, 1.0, Some(1.1)), pt(4_000, 1.0, Some(1.1))];
        assert_eq!(lag_episodes(&open, 10.0), vec![(1_000, 3_000)]);
        assert!(lag_episodes(&open, 2_000.0).is_empty());
    }

    #[test]
    fn value_lookup_and_stats() {
        let s = [pt(10, 1.0, None), pt(20, 2.0, None), pt(30, 3.0, None)];
        assert_eq!(value_at(&s, 9), None);
        assert_eq!(value_at(&s, 10).map(|p| p.oracle), Some(1.0));
        assert_eq!(value_at(&s, 25).map(|p| p.oracle), Some(2.0));
        assert_eq!(value_at(&s, 99).map(|p| p.oracle), Some(3.0));
        assert_eq!(med_p95_max(&[1.0, 2.0, 3.0, 4.0, 100.0]), Some((3.0, 100.0, 100.0)));
        assert_eq!(med_p95_max(&[]), None);
        assert!((abs_gap_bps(99.0, 100.0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn report_classes_and_dates() {
        assert_eq!(report_class("xyz:GOLD"), "metal");
        assert_eq!(report_class("xyz:PALLADIUM"), "metal");
        assert_eq!(report_class("xyz:CL"), "energy");
        assert_eq!(report_class("xyz:NATGAS"), "energy");
        assert_eq!(report_class("xyz:SP500"), "index");
        assert_eq!(report_class("xyz:EUR"), "FX");
        assert_eq!(report_class("xyz:AAPL"), "stock");
        assert_eq!(report_class("BTC"), "crypto");
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_791_813_600_000), "2026-10-12");
        assert_eq!(utc_date(1_791_813_600_000 - 1), "2026-10-12");
        assert_eq!(utc_date(1_791_763_200_000 - 1), "2026-10-11");
    }
}
