//! Readers of the probe CSVs (git-ignored raw data on the Mac) that compute the numbers decisions 56 and 57 are based on.
//! The definitions are those of `tools/hl-probe` (`--oracle-analyze`): the gap between the oracle and the book mid in basis
//! points (median and nearest-rank 95th percentile over every WebSocket message), the longest stretch between two oracle
//! changes (receive time of the WebSocket messages). The committed `config/launch-markets.toml` carries the results, so
//! `verify` never needs the raw data; `launch-config probe` recomputes them and `verify --probe-dir` compares.
use std::collections::BTreeMap;

/// One WebSocket observation of a coin: receive time (ms), `oraclePx`, and the book mid when present.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub ms: u64,
    pub oracle: f64,
    pub mid: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OracleStats {
    pub messages: usize,
    /// Median, 95th percentile (nearest rank) and maximum of |oracle - mid| / mid in bp over the messages that have a mid.
    pub median_bp: f64,
    pub p95_bp: f64,
    pub max_bp: f64,
    /// Longest time between two consecutive oracle changes; when the oracle never changed, the length of the observation.
    pub freeze_ms: u64,
    pub never_changed: bool,
    /// Median of `oraclePx`, the price level used for the quantity placeholders.
    pub price_median: f64,
}

/// `oracle-messages.csv` of `hl-probe --oracle` (`src,recv_ms,coin,oraclePx,markPx,midPx`): the WebSocket rows by coin, in
/// file order (which is receive order). REST rows are skipped, exactly as the probe's analyser does.
pub fn parse_hl_oracle(csv: &str) -> Result<BTreeMap<String, Vec<Point>>, String> {
    let mut out: BTreeMap<String, Vec<Point>> = BTreeMap::new();
    let mut lines = csv.lines();
    match lines.next() {
        Some("src,recv_ms,coin,oraclePx,markPx,midPx") => {}
        other => return Err(format!("oracle-messages.csv: unexpected header {other:?}")),
    }
    for (i, line) in lines.enumerate() {
        let c: Vec<&str> = line.split(',').collect();
        if c.len() < 6 {
            return Err(format!("oracle-messages.csv line {}: {} fields", i + 2, c.len()));
        }
        if c[0] != "ws" {
            continue;
        }
        let ms = c[1].parse::<u64>().map_err(|_| format!("line {}: recv_ms", i + 2))?;
        let oracle = c[3].parse::<f64>().map_err(|_| format!("line {}: oraclePx", i + 2))?;
        out.entry(c[2].to_string()).or_default().push(Point { ms, oracle, mid: c[5].parse::<f64>().ok() });
    }
    Ok(out)
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Nearest-rank 95th percentile of an ascending slice.
fn p95(sorted: &[f64]) -> f64 {
    let rank = ((0.95 * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

pub fn oracle_stats(series: &[Point]) -> Option<OracleStats> {
    let (first, last) = (series.first()?, series.last()?);
    let mut gaps: Vec<f64> = series.iter().filter_map(|p| p.mid.filter(|m| *m != 0.0).map(|m| ((p.oracle - m) / m).abs() * 10_000.0)).collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_by(f64::total_cmp);
    let changes: Vec<u64> = series.windows(2).filter(|w| w[1].oracle != w[0].oracle).map(|w| w[1].ms).collect();
    let freeze_ms = changes.windows(2).map(|w| w[1] - w[0]).max();
    let mut prices: Vec<f64> = series.iter().map(|p| p.oracle).collect();
    prices.sort_by(f64::total_cmp);
    Some(OracleStats {
        messages: series.len(),
        median_bp: median(&gaps),
        p95_bp: p95(&gaps),
        max_bp: gaps[gaps.len() - 1],
        // fewer than two changes: no gap between changes, the whole observation is one freeze
        freeze_ms: freeze_ms.unwrap_or(last.ms - first.ms),
        never_changed: changes.is_empty(),
        price_median: median(&prices),
    })
}

/// `gmx-tickers-1min.csv` (`recv_ms,symbol,usd`): the median USD price per GMX symbol.
pub fn gmx_price_medians(csv: &str) -> Result<BTreeMap<String, f64>, String> {
    let mut by: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut lines = csv.lines();
    match lines.next() {
        Some("recv_ms,symbol,usd") => {}
        other => return Err(format!("gmx-tickers-1min.csv: unexpected header {other:?}")),
    }
    for (i, line) in lines.enumerate() {
        let c: Vec<&str> = line.split(',').collect();
        if c.len() < 3 {
            return Err(format!("gmx-tickers-1min.csv line {}: {} fields", i + 2, c.len()));
        }
        by.entry(c[1].to_string()).or_default().push(c[2].parse::<f64>().map_err(|_| format!("line {}: usd", i + 2))?);
    }
    Ok(by
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(f64::total_cmp);
            (k, median(&v))
        })
        .collect())
}

/// `samples-tracked-all-polls.csv` of `gmx-probe` (`market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw`): the
/// longest gap between two consecutive PRICE changes of a symbol, in milliseconds of the server's `updatedAt`.
pub fn gmx_longest_gap_ms(csv: &str, symbol: &str) -> Result<Option<u64>, String> {
    let mut lines = csv.lines();
    match lines.next() {
        Some("market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw") => {}
        other => return Err(format!("samples csv: unexpected header {other:?}")),
    }
    // The probe's own archive rule (tools/gmx-probe): rows in receive order; a row counts only when its `updatedAt` is newer
    // than anything seen for the symbol, and it is a PRICE change when (min, max) differs from the last counted price.
    let mut max_seen = 0u64;
    let mut last: Option<(&str, &str)> = None;
    let mut times: Vec<u64> = Vec::new();
    for (i, line) in lines.enumerate() {
        let c: Vec<&str> = line.split(',').collect();
        if c.len() < 6 {
            return Err(format!("samples csv line {}: {} fields", i + 2, c.len()));
        }
        if c[0] != symbol {
            continue;
        }
        let updated = c[2].parse::<u64>().map_err(|_| format!("line {}: updated_at_ms", i + 2))?;
        if updated > max_seen {
            max_seen = updated;
            if last != Some((c[3], c[4])) {
                times.push(updated);
                last = Some((c[3], c[4]));
            }
        }
    }
    Ok(times.windows(2).map(|w| w[1] - w[0]).max())
}
