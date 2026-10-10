//! Pure helpers of the GMX probe: price conversion, gap statistics, the sample file format, blob search, candle gaps.
//! (A measurement tool: floats are fine here, unlike in the engine.)

/// `10^e` as f64.
pub fn pow10(e: i32) -> f64 {
    10f64.powi(e)
}

/// USD price from a GMX 30-decimal string. `oracle_decimals = None` gives formula A: `v / 10^(30 - token_decimals)`;
/// `Some(d)` gives formula B: `v / 10^(30 - token_decimals - d)`. `None` if the string is not a non-negative integer.
pub fn usd_price(raw: &str, token_decimals: i32, oracle_decimals: Option<i32>) -> Option<f64> {
    let v: u128 = raw.trim().parse().ok()?;
    let exp = 30 - token_decimals - oracle_decimals.unwrap_or(0);
    Some(v as f64 / pow10(exp))
}

/// `|a - b| / |b|`.
pub fn rel_err(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return f64::INFINITY;
    }
    ((a - b) / b).abs()
}

/// Spread in basis points: `(max - min) / median x 10_000`.
pub fn spread_bps(min: f64, max: f64, median: f64) -> f64 {
    if median == 0.0 {
        return f64::INFINITY;
    }
    (max - min) / median * 10_000.0
}

/// Nearest-rank percentile of an ascending slice (`p` in 0..=100).
pub fn percentile(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = rank.clamp(1, sorted.len()) - 1;
    sorted.get(idx).copied()
}

/// Gap statistics (milliseconds) between consecutive change times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapStats {
    pub gaps: usize,
    pub median: u64,
    pub p95: u64,
    pub max: u64,
}

/// Statistics of the gaps between consecutive entries of `times_ms` (any order is sorted first). `None` if fewer than 2 times.
pub fn gap_stats(times_ms: &[u64]) -> Option<GapStats> {
    if times_ms.len() < 2 {
        return None;
    }
    let mut t = times_ms.to_vec();
    t.sort_unstable();
    let mut gaps: Vec<u64> = t.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_unstable();
    Some(GapStats {
        gaps: gaps.len(),
        median: percentile(&gaps, 50.0)?,
        p95: percentile(&gaps, 95.0)?,
        max: *gaps.last()?,
    })
}

/// Latency percentiles (p50, p95, p99) of unsorted values.
pub fn latency_percentiles(values: &[u64]) -> Option<(u64, u64, u64)> {
    let mut v = values.to_vec();
    v.sort_unstable();
    Some((percentile(&v, 50.0)?, percentile(&v, 95.0)?, percentile(&v, 99.0)?))
}

// ------------------------------------------------------------------ sample file formats (DESIGN 4.5 shape)

/// One archived sample: market index, server `updatedAt` (ms), bid (= minPrice), ask (= maxPrice) and median
/// (= (bid + ask) / 2) as raw 30-decimal integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub market_idx: u16,
    pub updated_at_ms: u64,
    pub bid: u128,
    pub ask: u128,
    pub median: u128,
}

/// Size of one binary record.
pub const RECORD_BYTES: usize = 64;

impl Record {
    pub fn new(market_idx: u16, updated_at_ms: u64, bid: u128, ask: u128) -> Self {
        Record { market_idx, updated_at_ms, bid, ask, median: bid / 2 + ask / 2 + (bid % 2 + ask % 2) / 2 }
    }

    /// 64 bytes, little endian: idx u16, 6 zero bytes, updated_at u64, bid u128, ask u128, median u128.
    pub fn to_bytes(&self) -> [u8; RECORD_BYTES] {
        let mut b = [0u8; RECORD_BYTES];
        b[0..2].copy_from_slice(&self.market_idx.to_le_bytes());
        b[8..16].copy_from_slice(&self.updated_at_ms.to_le_bytes());
        b[16..32].copy_from_slice(&self.bid.to_le_bytes());
        b[32..48].copy_from_slice(&self.ask.to_le_bytes());
        b[48..64].copy_from_slice(&self.median.to_le_bytes());
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
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&b[8..16]);
        Record {
            market_idx: u16::from_le_bytes(idx),
            updated_at_ms: u64::from_le_bytes(ts),
            bid: u128_at(16),
            ask: u128_at(32),
            median: u128_at(48),
        }
    }
}

/// Reads a whole binary sample file; `None` if its length is not a multiple of the record size.
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

/// CSV line: `market,recv_unix_ms,updated_at_ms,bid,ask,median`.
pub fn csv_line(market: &str, recv_ms: u64, r: &Record) -> String {
    format!("{market},{recv_ms},{},{},{},{}", r.updated_at_ms, r.bid, r.ask, r.median)
}

/// Parses a line produced by `csv_line` (market name, receive time, record without index).
pub fn parse_csv_line(line: &str) -> Option<(String, u64, Record)> {
    let mut it = line.trim_end().split(',');
    let market = it.next()?.to_string();
    let recv: u64 = it.next()?.parse().ok()?;
    let updated: u64 = it.next()?.parse().ok()?;
    let bid: u128 = it.next()?.parse().ok()?;
    let ask: u128 = it.next()?.parse().ok()?;
    let median: u128 = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((market, recv, Record { market_idx: 0, updated_at_ms: updated, bid, ask, median }))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes `0x...` or plain hex; `None` on odd length or non-hex.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()).collect()
}

// ------------------------------------------------------------------ blob search

/// A place where a big-endian integer inside a blob is close to a target price scale.
#[derive(Clone, Debug, PartialEq)]
pub struct BlobHit {
    pub offset: usize,
    pub width: usize,
    pub scale: String,
    pub value: u128,
    pub rel_err: f64,
}

/// Looks for big-endian unsigned integers of width 8, 16, 24 or 32 bytes (at every byte offset) whose value is within
/// `tol` (relative) of one of the `(scale name, target value)` pairs. Widths above 16 need leading zero bytes.
pub fn find_be_values(blob: &[u8], targets: &[(String, f64)], tol: f64) -> Vec<BlobHit> {
    let mut hits = Vec::new();
    for &width in &[8usize, 16, 24, 32] {
        if blob.len() < width {
            continue;
        }
        for offset in 0..=(blob.len() - width) {
            let w = &blob[offset..offset + width];
            let lead = width.saturating_sub(16);
            if w[..lead].iter().any(|&b| b != 0) {
                continue;
            }
            let mut a = [0u8; 16];
            let tail = &w[lead..];
            a[16 - tail.len()..].copy_from_slice(tail);
            let value = u128::from_be_bytes(a);
            if value == 0 {
                continue;
            }
            for (name, target) in targets {
                if *target > 0.0 {
                    let e = rel_err(value as f64, *target);
                    if e <= tol {
                        hits.push(BlobHit { offset, width, scale: name.clone(), value, rel_err: e });
                    }
                }
            }
        }
    }
    hits
}

// ------------------------------------------------------------------ candles

/// Missing minutes in an ascending list of candle start times (seconds, 60 s apart when complete).
/// Returns the total number of missing candles and the gaps as `(last_seen, next_seen, missing)`.
pub fn minute_gaps(ts_ascending: &[i64]) -> (u64, Vec<(i64, i64, u64)>) {
    let mut total = 0u64;
    let mut gaps = Vec::new();
    for w in ts_ascending.windows(2) {
        let d = w[1] - w[0];
        if d > 60 {
            let missing = (d / 60 - 1) as u64;
            total += missing;
            gaps.push((w[0], w[1], missing));
        }
    }
    (total, gaps)
}

/// Unix milliseconds of an ISO-8601 UTC time like `2026-10-10T18:45:57.464Z`.
pub fn parse_iso_ms(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':');
    let (hh, mm, ss): (i64, i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let mut frac_ms: String = frac.chars().take(3).collect();
    while frac_ms.len() < 3 {
        frac_ms.push('0');
    }
    let ms: i64 = frac_ms.parse().ok()?;
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    u64::try_from(secs * 1000 + ms).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden vectors: values seen live on 2026-10-10 (see handover/gmx-probe-report.md).
    #[test]
    fn price_conversion_golden_vectors() {
        // APE: decimals 18, raw 152970540433 -> 0.152970540433 USD
        let ape = usd_price("152970540433", 18, None).unwrap();
        assert!((ape - 0.152970540433).abs() < 1e-15);
        // APT: decimals 8, raw 8747368733790331750000 -> 0.8747...
        let apt = usd_price("8747368733790331750000", 8, None).unwrap();
        assert!((apt - 0.874736873379033).abs() < 1e-12);
        // BTC signed full price: decimals 8, 829419936994195400000000000 -> 82941.99...
        let btc = usd_price("829419936994195400000000000", 8, None).unwrap();
        assert!((btc - 82941.99369941954).abs() < 1e-6);
        // the 2026-10-10 probe run (tokenDecimals in /tokens): each matched the 1m candle close within 0.5%
        for (raw, dec, want) in [
            ("829464189426712150000000000", 8, 82946.41894267215),
            ("2509584327229797", 18, 2509.584327229797),
            ("110189121391252093750000", 9, 110.18912139125209),
            ("1402634462495451725000000", 6, 1.4026344624954517),
            ("4196832500000000", 18, 4196.8325),
            ("4196837500000000", 18, 4196.8375),
            ("779772500000000", 18, 779.7725),
            ("3418250000000", 18, 3.41825),
        ] {
            let got = usd_price(raw, dec, None).unwrap();
            assert!(rel_err(got, want) < 1e-12, "{raw} dec {dec}: {got} vs {want}");
        }
        // formula B subtracts the oracle decimals as well
        assert!((usd_price("152970540433", 18, Some(8)).unwrap() - 15297054.0433).abs() < 1e-3);
        assert_eq!(usd_price("abc", 18, None), None);
        assert_eq!(usd_price("-5", 18, None), None);
    }

    #[test]
    fn relative_error_and_spread() {
        assert!((rel_err(101.0, 100.0) - 0.01).abs() < 1e-12);
        assert!((rel_err(99.0, 100.0) - 0.01).abs() < 1e-12);
        assert!(rel_err(1.0, 0.0).is_infinite());
        assert!((spread_bps(99.0, 101.0, 100.0) - 200.0).abs() < 1e-9);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 50.0), Some(50));
        assert_eq!(percentile(&v, 95.0), Some(95));
        assert_eq!(percentile(&v, 99.0), Some(99));
        assert_eq!(percentile(&v, 100.0), Some(100));
        assert_eq!(percentile(&v, 0.0), Some(1));
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[7], 99.0), Some(7));
        assert_eq!(latency_percentiles(&[5, 1, 3, 2, 4]), Some((3, 5, 5)));
    }

    #[test]
    fn gap_statistics() {
        // changes at 0, 100, 300, 600, 1000 ms: gaps 100, 200, 300, 400
        let g = gap_stats(&[0, 100, 300, 600, 1000]).unwrap();
        assert_eq!(g, GapStats { gaps: 4, median: 200, p95: 400, max: 400 });
        // unsorted input is sorted first
        assert_eq!(gap_stats(&[1000, 0, 600, 100, 300]), Some(g));
        assert_eq!(gap_stats(&[5]), None);
        assert_eq!(gap_stats(&[]), None);
        assert_eq!(gap_stats(&[10, 10]), Some(GapStats { gaps: 1, median: 0, p95: 0, max: 0 }));
    }

    #[test]
    fn record_round_trip_binary() {
        let r = Record::new(113, 1_791_657_944_312, 829_419_936_994_195_400_000_000_000, 829_436_566_649_517_950_000_000_000);
        assert_eq!(r.median, (r.bid + r.ask) / 2);
        let b = r.to_bytes();
        assert_eq!(b.len(), 64);
        assert_eq!(Record::from_bytes(&b), r);
        // many records through a file image
        let rs: Vec<Record> = (0..10).map(|i| Record::new(i, 1_000 + u64::from(i), u128::from(i) * 7 + 1, u128::from(i) * 7 + 3)).collect();
        let mut file = Vec::new();
        for r in &rs {
            file.extend_from_slice(&r.to_bytes());
        }
        assert_eq!(read_records(&file).unwrap(), rs);
        assert_eq!(read_records(&file[..file.len() - 1]), None);
        assert_eq!(read_records(&[]).unwrap(), vec![]);
    }

    #[test]
    fn record_median_does_not_overflow_for_huge_values() {
        let r = Record::new(0, 1, u128::MAX, u128::MAX);
        assert_eq!(r.median, u128::MAX);
        assert_eq!(Record::new(0, 1, 3, 4).median, 3);
        assert_eq!(Record::new(0, 1, 3, 5).median, 4);
    }

    #[test]
    fn csv_round_trip() {
        let r = Record::new(0, 1_791_657_944_312, 152_970_540_433, 153_027_321_299);
        let line = csv_line("APE", 1_791_657_944_900, &r);
        assert_eq!(line, "APE,1791657944900,1791657944312,152970540433,153027321299,152998930866");
        let (m, recv, back) = parse_csv_line(&line).unwrap();
        assert_eq!((m.as_str(), recv), ("APE", 1_791_657_944_900));
        assert_eq!((back.updated_at_ms, back.bid, back.ask, back.median), (r.updated_at_ms, r.bid, r.ask, r.median));
        assert_eq!(parse_csv_line("APE,1,2,3"), None);
        assert_eq!(parse_csv_line("APE,1,2,3,4,5,6"), None);
        assert_eq!(parse_csv_line("APE,x,2,3,4,5"), None);
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex(&[0, 1, 0xab, 0xff]), "0001abff");
        assert_eq!(unhex("0x0001abff"), Some(vec![0, 1, 0xab, 0xff]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn blob_search_finds_big_endian_prices() {
        // 32-byte word holding 82_941 x 1e18 (BE), preceded by junk
        let price = 82_941u128 * 1_000_000_000_000_000_000;
        let mut blob = vec![0xAA; 5];
        let mut word = [0u8; 32];
        word[16..].copy_from_slice(&price.to_be_bytes());
        blob.extend_from_slice(&word);
        blob.extend_from_slice(&[0xBB; 7]);
        let hits = find_be_values(&blob, &[("1e18".to_string(), 82_941.5e18)], 0.001);
        assert!(hits.iter().any(|h| h.width == 32 && h.offset == 5 && h.value == price));
        assert!(hits.iter().any(|h| h.width == 16 && h.offset == 5 + 16));
        // a value 2% away is not a hit at 0.1% tolerance
        assert!(find_be_values(&blob, &[("x".to_string(), price as f64 * 1.02)], 0.001).is_empty());
        // short blobs and zero targets are fine
        assert!(find_be_values(&[1, 2, 3], &[("x".to_string(), 1.0)], 0.1).is_empty());
        assert!(find_be_values(&blob, &[("z".to_string(), 0.0)], 0.1).is_empty());
    }

    #[test]
    fn candle_gap_detection() {
        let ts = [0i64, 60, 120, 240, 300, 600];
        let (total, gaps) = minute_gaps(&ts);
        assert_eq!(total, 1 + 4);
        assert_eq!(gaps, vec![(120, 240, 1), (300, 600, 4)]);
        assert_eq!(minute_gaps(&[0, 60, 120]), (0, vec![]));
        assert_eq!(minute_gaps(&[5]), (0, vec![]));
    }

    #[test]
    fn iso_time_parsing() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.500Z"), Some(1500));
        assert_eq!(parse_iso_ms("2000-03-01T00:00:00.000Z"), Some(951_868_800_000));
        // from the live sample: createdAt 2026-10-10T18:45:57.464Z next to minBlockTimestamp 1791657956
        assert_eq!(parse_iso_ms("2026-10-10T18:45:57.464Z"), Some(1_791_657_957_464));
        assert_eq!(parse_iso_ms("2026-10-10 18:45:57"), None);
        assert_eq!(parse_iso_ms("nonsense"), None);
    }
}
