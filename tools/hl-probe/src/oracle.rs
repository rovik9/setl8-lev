//! `--oracle` mode (prompt P11 part 1): where does `oraclePx` come from, how often does it change, how far is it from the
//! book, and how does it compare with GMX. Read-only: `activeAssetCtx` WebSocket subscriptions, `metaAndAssetCtxs` for dex
//! `xyz` every 10 s, one GMX `/prices/tickers` fetch per minute. Raw data goes to `data/weekday-<UTC date>/` (git-ignored).
use super::*;
use std::fs::create_dir_all;

const MAIN3: [&str; 3] = ["BTC", "ETH", "SOL"];
const GMX_SYMS: [&str; 10] = ["GOLD", "SILVER", "WTIOIL", "BRENTOIL", "NATGAS", "SPY", "QQQ", "BTC", "ETH", "SOL"];
const THIN_FIXED: [&str; 6] = ["xyz:EUR", "xyz:GBP", "xyz:JPY", "xyz:JP225", "xyz:KR200", "xyz:PALLADIUM"];
const GMX_HOST: &str = "https://arbitrum-api.gmxinfra.io";

struct Msg {
    rest: bool,
    recv_ms: u64,
    coin: String,
    oracle: f64,
    mark: f64,
    mid: Option<f64>,
}

fn point(m: &Msg) -> OraclePoint {
    OraclePoint { ms: m.recv_ms, oracle: m.oracle, mark: m.mark, mid: m.mid }
}

fn pooled_gaps(series: &[&Vec<OraclePoint>]) -> Option<GapStats> {
    let mut gaps: Vec<u64> = Vec::new();
    for s in series {
        let t = oracle_change_times(s);
        gaps.extend(t.windows(2).map(|w| w[1] - w[0]));
    }
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    Some(GapStats { gaps: gaps.len(), median: percentile(&gaps, 50.0)?, p95: percentile(&gaps, 95.0)?, max: *gaps.last()? })
}

fn secs1(ms: u64) -> String {
    f(ms as f64 / 1000.0, 1)
}

fn stats3(v: &[f64], d: usize) -> [String; 3] {
    match med_p95_max(v) {
        Some((a, b, c)) => [f(a, d), f(b, d), f(c, d)],
        None => ["n/a".into(), "n/a".into(), "n/a".into()],
    }
}

fn minutes_of(s: &[OraclePoint]) -> f64 {
    match (s.first(), s.last()) {
        (Some(a), Some(b)) if b.ms > a.ms => (b.ms - a.ms) as f64 / 60_000.0,
        _ => 0.0,
    }
}

/// Sections 1b to 6 of the report from the collected series. Class tables use the `core` coins only (the 28 working coins of
/// the weekend list and BTC/ETH/SOL); the five thinnest stocks appear in the per-coin table and in section 5.
fn sections(
    s: &mut String,
    ws_series: &BTreeMap<String, Vec<OraclePoint>>,
    rest_series: &BTreeMap<String, Vec<OraclePoint>>,
    gmx_rows: &[(u64, HashMap<String, f64>)],
    xyz_coins: &[(String, bool, f64)],
    smallest5: &[(String, f64)],
    core: &HashSet<String>,
) {
    // WS vs REST
    {
        let mut by_class: BTreeMap<&str, (u64, u64, u64, Vec<f64>)> = BTreeMap::new();
        for (coin, rs) in rest_series {
            let Some(ws) = ws_series.get(coin) else { continue };
            for p in rs {
                // the REST row was received at p.ms; compare with the latest WS oracle at or before it and with the next one
                let e = by_class.entry(report_class(coin)).or_insert((0, 0, 0, vec![]));
                e.0 += 1;
                if let Some(w) = value_at(ws, p.ms) {
                    if w.oracle == p.oracle {
                        e.1 += 1;
                    } else if value_at(ws, p.ms + 2_000).map_or(false, |w2| w2.oracle == p.oracle) {
                        e.2 += 1; // the WS value arrived within 2 s after the REST row
                    }
                    e.3.push(abs_gap_bps(p.oracle, w.oracle));
                }
            }
        }
        let rows: Vec<Vec<String>> = by_class
            .iter()
            .map(|(c, (n, same, later, d))| {
                let st = stats3(d, 2);
                vec![c.to_string(), n.to_string(), format!("{} ({}%)", same, f(*same as f64 * 100.0 / (*n).max(1) as f64, 1)), format!("{later}"), st[0].clone(), st[1].clone(), st[2].clone()]
            })
            .collect();
        s.push_str("REST `oraclePx` against the latest WebSocket `oraclePx` at the poll's receive time (xyz coins only):\n\n");
        s.push_str(&table(&["class", "REST rows", "same as the latest WS value", "WS had it only within the next 2 s", "|REST - WS| bp median", "p95", "max"], &rows));
    }

    // ---- 2. per coin cadence
    s.push_str("## 2. Oracle cadence (WebSocket `activeAssetCtx`; a change = `oraclePx` differs from the previous message of the coin)\n\n");
    let classes = ["stock", "ETF", "index", "FX", "metal", "energy", "crypto", "other"];
    let mut coin_rows = Vec::new();
    let mut class_rate: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut class_frozen: BTreeMap<&str, (u64, String, u32)> = BTreeMap::new();
    let mut never: Vec<String> = Vec::new();
    for (coin, sr) in ws_series {
        let cl = report_class(coin);
        let changes = oracle_change_times(sr);
        let mins = minutes_of(sr);
        let rate = if mins > 0.0 { changes.len() as f64 / mins } else { 0.0 };
        if core.contains(coin) {
            class_rate.entry(cl).or_default().push(rate);
        }
        if changes.is_empty() {
            never.push(coin.clone());
        }
        let gs = gap_stats(&changes);
        let fr = longest_frozen_while_moving(sr, 1);
        if let (Some(fz), true) = (fr, core.contains(coin)) {
            let e = class_frozen.entry(cl).or_insert((0, String::new(), 0));
            if fz.duration_ms > e.0 {
                *e = (fz.duration_ms, coin.clone(), fz.mid_changes);
            }
        }
        let mid_moves = sr.windows(2).filter(|w| matches!((w[0].mid, w[1].mid), (Some(a), Some(b)) if a != b)).count();
        coin_rows.push(vec![
            if core.contains(coin) { coin.clone() } else { format!("{coin} (thin-stock check only)") },
            cl.into(),
            sr.len().to_string(),
            changes.len().to_string(),
            f(rate, 2),
            gs.map_or("n/a".into(), |g| secs1(g.median)),
            gs.map_or("n/a".into(), |g| secs1(g.p95)),
            gs.map_or("n/a".into(), |g| secs1(g.max)),
            mid_moves.to_string(),
            fr.map_or("none".into(), |x| format!("{} s ({} mid changes)", secs1(x.duration_ms), x.mid_changes)),
        ]);
    }
    s.push_str("### Per class\n\n");
    let mut rows = Vec::new();
    for cl in classes {
        let series: Vec<&Vec<OraclePoint>> = ws_series.iter().filter(|(c, _)| report_class(c) == cl && core.contains(*c)).map(|(_, v)| v).collect();
        if series.is_empty() {
            continue;
        }
        let rates = class_rate.get(cl).cloned().unwrap_or_default();
        let (rmed, _, rmax) = med_p95_max(&rates).unwrap_or((0.0, 0.0, 0.0));
        let rmin = rates.iter().copied().fold(f64::INFINITY, f64::min);
        let gs = pooled_gaps(&series);
        let fz = class_frozen.get(cl);
        rows.push(vec![
            cl.into(),
            series.len().to_string(),
            f(rmed, 2),
            f(rmin, 2),
            f(rmax, 2),
            gs.map_or("n/a".into(), |g| secs1(g.median)),
            gs.map_or("n/a".into(), |g| secs1(g.p95)),
            gs.map_or("n/a".into(), |g| secs1(g.max)),
            fz.map_or("none".into(), |(d, c, m)| format!("{} s on {} ({} mid changes)", secs1(*d), c, m)),
        ]);
    }
    s.push_str(&table(&["class", "coins", "oracle changes / min (median coin)", "min coin", "max coin", "gap median s (pooled)", "gap p95 s", "gap max s", "longest freeze while the book mid moved"], &rows));
    s.push_str(&format!("Coins whose oracle never changed in the window: {}.\n\n", if never.is_empty() { "none".into() } else { never.join(", ") }));
    s.push_str("### Per coin\n\n");
    s.push_str(&table(&["coin", "class", "messages", "oracle changes", "changes / min", "gap median s", "gap p95 s", "gap max s", "mid changes", "longest oracle freeze while the mid moved"], &coin_rows));

    // ---- 3. jumps
    s.push_str("## 3. Size of oracle jumps (|new - old| / old, bp, per change)\n\n");
    let mut rows = Vec::new();
    for cl in classes {
        let mut j = Vec::new();
        for (c, sr) in ws_series {
            if report_class(c) == cl && core.contains(c) {
                j.extend(oracle_jumps_bps(sr));
            }
        }
        if j.is_empty() {
            continue;
        }
        let st = stats3(&j, 2);
        rows.push(vec![cl.into(), j.len().to_string(), st[0].clone(), st[1].clone(), st[2].clone()]);
    }
    s.push_str(&table(&["class", "jumps", "median bp", "p95 bp", "max bp"], &rows));

    // ---- 4. oracle vs book
    s.push_str("## 4. Oracle against the book mid and against `markPx` (|gap| in bp over every message)\n\n");
    let mut rows = Vec::new();
    let mut lag_rows = Vec::new();
    let mut coin_gap_rows: Vec<(String, Vec<String>)> = Vec::new();
    for cl in classes {
        let (mut vm, mut vk) = (Vec::new(), Vec::new());
        let mut eps: Vec<u64> = Vec::new();
        let (mut n_eps, mut off_ms, mut span_ms) = (0usize, 0u64, 0u64);
        for (c, sr) in ws_series {
            if report_class(c) != cl || !core.contains(c) {
                continue;
            }
            for p in sr {
                if let Some(m) = p.mid {
                    vm.push(abs_gap_bps(p.oracle, m));
                }
                vk.push(abs_gap_bps(p.oracle, p.mark));
            }
            let e = lag_episodes(sr, 10.0);
            n_eps += e.len();
            off_ms += e.iter().map(|x| x.1).sum::<u64>();
            eps.extend(e.iter().map(|x| x.1));
            span_ms += sr.last().map_or(0, |l| l.ms) - sr.first().map_or(0, |f0| f0.ms);
        }
        if vk.is_empty() {
            continue;
        }
        let (a, b) = (stats3(&vm, 2), stats3(&vk, 2));
        rows.push(vec![cl.into(), vm.len().to_string(), a[0].clone(), a[1].clone(), a[2].clone(), b[0].clone(), b[1].clone(), b[2].clone()]);
        eps.sort_unstable();
        lag_rows.push(vec![
            cl.into(),
            n_eps.to_string(),
            f(off_ms as f64 / 1000.0, 0),
            f(off_ms as f64 * 100.0 / span_ms.max(1) as f64, 2),
            percentile(&eps, 50.0).map_or("n/a".into(), secs1),
            percentile(&eps, 95.0).map_or("n/a".into(), secs1),
            eps.last().map_or("n/a".into(), |x| secs1(*x)),
        ]);
    }
    s.push_str(&table(&["class", "messages with a mid", "|oracle-mid| median", "p95", "max", "|oracle-mark| median", "p95", "max"], &rows));
    s.push_str("### Oracle lag behind the book: episodes with the mid more than 10 bp away from the oracle\n\nAn episode starts at the first message with |mid - oracle| > 10 bp and ends at the first message back inside 10 bp (an open one ends at the last message). Duration = how long the oracle was more than 10 bp off the book.\n\n");
    s.push_str(&table(&["class", "episodes", "total seconds off", "% of observed time", "episode median s", "p95 s", "max s"], &lag_rows));

    // ---- 5. thin markets
    s.push_str("## 5. Thin markets\n\n");
    let mut thin: Vec<String> = THIN_FIXED.iter().map(|x| x.to_string()).collect();
    for (n, _) in smallest5 {
        if !thin.contains(n) {
            thin.push(n.clone());
        }
    }
    let mut rows = Vec::new();
    for c in &thin {
        let Some(sr) = ws_series.get(c) else {
            rows.push(vec![c.clone(), "no data".into()]);
            continue;
        };
        let changes = oracle_change_times(sr);
        let mins = minutes_of(sr);
        let gs = gap_stats(&changes);
        let vm: Vec<f64> = sr.iter().filter_map(|p| p.mid.map(|m| abs_gap_bps(p.oracle, m))).collect();
        let vk: Vec<f64> = sr.iter().map(|p| abs_gap_bps(p.oracle, p.mark)).collect();
        let (a, b) = (stats3(&vm, 2), stats3(&vk, 2));
        let vol = xyz_coins.iter().find(|x| &x.0 == c).map_or(0.0, |x| x.2);
        rows.push(vec![
            c.clone(),
            f(vol, 0),
            sr.len().to_string(),
            changes.len().to_string(),
            f(if mins > 0.0 { changes.len() as f64 / mins } else { 0.0 }, 2),
            gs.map_or("n/a".into(), |g| secs1(g.median)),
            gs.map_or("n/a".into(), |g| secs1(g.max)),
            a[0].clone(),
            a[1].clone(),
            a[2].clone(),
            b[0].clone(),
            b[2].clone(),
            longest_frozen_while_moving(sr, 1).map_or("none".into(), |x| format!("{} s ({})", secs1(x.duration_ms), x.mid_changes)),
        ]);
        coin_gap_rows.push((c.clone(), vec![]));
    }
    s.push_str(&table(
        &["coin", "24 h notional USD", "messages", "oracle changes", "changes / min", "gap median s", "gap max s", "|oracle-mid| median bp", "p95", "max", "|oracle-mark| median bp", "max", "longest freeze while mid moved (mid changes)"],
        &rows,
    ));

    // ---- 6. vs GMX
    s.push_str("## 6. Hyperliquid oracle against GMX tickers (one GMX fetch per minute; HL value = latest WS `oraclePx` at the fetch's send time)\n\n");
    let pairs: [(&str, &str, bool); 10] = [
        ("GOLD", "xyz:GOLD", false),
        ("SILVER", "xyz:SILVER", false),
        ("WTIOIL", "xyz:CL", false),
        ("BRENTOIL", "xyz:BRENTOIL", false),
        ("NATGAS", "xyz:NATGAS", false),
        ("SPY", "xyz:SP500", true),
        ("QQQ", "xyz:XYZ100", true),
        ("BTC", "BTC", false),
        ("ETH", "ETH", false),
        ("SOL", "SOL", false),
    ];
    let mut rows = Vec::new();
    for (g, h, ratio) in pairs {
        let Some(hs) = ws_series.get(h) else { continue };
        let mut vals: Vec<(f64, f64)> = Vec::new(); // (gmx, hl oracle)
        for (t, m) in gmx_rows {
            if let (Some(gv), Some(hp)) = (m.get(g), value_at(hs, *t)) {
                vals.push((*gv, hp.oracle));
            }
        }
        if vals.is_empty() {
            rows.push(vec![format!("{g} vs {h}"), "0".into(), "no data".into()]);
            continue;
        }
        if ratio {
            let rs: Vec<f64> = vals.iter().map(|(a, b)| a / b).collect();
            let med = median(&rs).unwrap_or(f64::NAN);
            let dev: Vec<f64> = rs.iter().map(|r| abs_gap_bps(*r, med)).collect();
            let signed: Vec<f64> = rs.iter().map(|r| gap_bps(*r, med)).collect();
            let st = stats3(&dev, 2);
            rows.push(vec![format!("{g} / {h} (ratio)"), vals.len().to_string(), format!("median ratio {}", f(med, 6)), f(median(&signed).unwrap_or(f64::NAN), 2), st[0].clone(), st[1].clone(), st[2].clone()]);
        } else {
            let signed: Vec<f64> = vals.iter().map(|(a, b)| gap_bps(*a, *b)).collect();
            let abs: Vec<f64> = signed.iter().map(|x| x.abs()).collect();
            let st = stats3(&abs, 2);
            rows.push(vec![format!("{g} vs {h}"), vals.len().to_string(), format!("GMX last {} / HL oracle last {}", f(vals.last().map_or(0.0, |x| x.0), 4), f(vals.last().map_or(0.0, |x| x.1), 4)), f(median(&signed).unwrap_or(f64::NAN), 2), st[0].clone(), st[1].clone(), st[2].clone()]);
        }
    }
    s.push_str(&table(&["pair", "minutes compared", "level", "signed median bp (GMX - HL; for ratios: ratio vs its own median)", "|gap| median bp", "p95", "max"], &rows));
}


/// `--oracle-analyze <dir>`: recomputes sections 1b to 6 from `oracle-messages.csv`, `gmx-tickers-1min.csv` and
/// `raw/metaAndAssetCtxs-xyz-start.json` of an earlier `--oracle` run (the thin-stock list is `raw/setup.txt`, one coin per line).
pub fn analyze(dir: &str) {
    let d = PathBuf::from(dir);
    let mut ws: BTreeMap<String, Vec<OraclePoint>> = BTreeMap::new();
    let mut rs: BTreeMap<String, Vec<OraclePoint>> = BTreeMap::new();
    for line in fs::read_to_string(d.join("oracle-messages.csv")).unwrap_or_default().lines().skip(1) {
        let c: Vec<&str> = line.split(',').collect();
        if c.len() < 6 {
            continue;
        }
        let (Ok(ms), Ok(o), Ok(m)) = (c[1].parse::<u64>(), c[3].parse::<f64>(), c[4].parse::<f64>()) else { continue };
        let p = OraclePoint { ms, oracle: o, mark: m, mid: c[5].parse::<f64>().ok() };
        (if c[0] == "rest" { &mut rs } else { &mut ws }).entry(c[2].to_string()).or_default().push(p);
    }
    let mut gmx: BTreeMap<u64, HashMap<String, f64>> = BTreeMap::new();
    for line in fs::read_to_string(d.join("gmx-tickers-1min.csv")).unwrap_or_default().lines().skip(1) {
        let c: Vec<&str> = line.split(',').collect();
        if let (Some(t), Some(sym), Some(v)) = (c.first().and_then(|x| x.parse::<u64>().ok()), c.get(1), c.get(2).and_then(|x| x.parse::<f64>().ok())) {
            gmx.entry(t).or_default().insert(sym.to_string(), v);
        }
    }
    let gmx_rows: Vec<(u64, HashMap<String, f64>)> = gmx.into_iter().collect();
    let snap: Value = serde_json::from_str(&fs::read_to_string(d.join("raw").join("metaAndAssetCtxs-xyz-start.json")).unwrap_or_default()).unwrap_or(Value::Null);
    let uni = snap.get(0).and_then(|m| m.get("universe")).and_then(|u| u.as_array()).cloned().unwrap_or_default();
    let cxs = snap.get(1).and_then(|c| c.as_array()).cloned().unwrap_or_default();
    let xyz_coins: Vec<(String, bool, f64)> =
        uni.iter().zip(cxs.iter()).map(|(u, c)| (u.get("name").and_then(|x| x.as_str()).unwrap_or("?").to_string(), false, c.get("dayNtlVlm").and_then(num).unwrap_or(0.0))).collect();
    let thin: Vec<(String, f64)> = fs::read_to_string(d.join("raw").join("setup.txt"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| (l.to_string(), xyz_coins.iter().find(|x| x.0 == l).map_or(0.0, |x| x.2)))
        .collect();
    let core: HashSet<String> = ws.keys().filter(|k| !thin.iter().any(|t| &t.0 == *k) || WANT.iter().any(|(_, b)| format!("xyz:{b}") == **k)).cloned().collect();
    let mut s = String::new();
    sections(&mut s, &ws, &rs, &gmx_rows, &xyz_coins, &thin, &core);
    println!("{s}");
}

pub fn run() {
    let started = now_ms();
    let out = PathBuf::from("data").join(format!("weekday-{}", utc_date(started)));
    let _ = create_dir_all(out.join("raw"));
    let secs: u64 = if quick() { 60 } else { 900 };
    let rest = Arc::new(Rest::new());
    let mut report = format!("# Hyperliquid oracle probe: raw tables\n\nRun started: {}\nPlanned duration: {} s{}\n\n", utc_string(started), secs, if quick() { " (QUICK smoke test)" } else { "" });
    let save_raw = |name: &str, body: &str| {
        let _ = fs::write(out.join("raw").join(name), body);
    };

    // ---------------------------------------------------------------- discovery
    eprintln!("== oracle: discovery");
    let (dexs, _) = rest.info(&json!({"type":"perpDexs"}), 20).expect("perpDexs");
    save_raw("perpDexs.json", &dexs.to_string());
    let (snap, _) = rest.info(&json!({"type":"metaAndAssetCtxs","dex":"xyz"}), 20).expect("metaAndAssetCtxs xyz");
    save_raw("metaAndAssetCtxs-xyz-start.json", &snap.to_string());
    let xyz_dex = dexs.as_array().and_then(|a| a.iter().find(|d| d.get("name").and_then(|n| n.as_str()) == Some("xyz"))).cloned().unwrap_or(Value::Null);
    let uni = snap.get(0).and_then(|m| m.get("universe")).and_then(|u| u.as_array()).cloned().unwrap_or_default();
    let cxs = snap.get(1).and_then(|c| c.as_array()).cloned().unwrap_or_default();
    // (name, delisted, 24h notional)
    // `isDelisted` is only in `allPerpMetas`, not in the per-dex `metaAndAssetCtxs` universe
    let (metas, _) = rest.info(&json!({"type":"allPerpMetas"}), 20).expect("allPerpMetas");
    save_raw("allPerpMetas.json", &metas.to_string());
    let dex_idx = dexs.as_array().and_then(|a| a.iter().position(|d| d.get("name").and_then(|n| n.as_str()) == Some("xyz"))).unwrap_or(1);
    let delisted: HashSet<String> = metas
        .get(dex_idx)
        .and_then(|m| m.get("universe"))
        .and_then(|u| u.as_array())
        .map(|a| a.iter().filter(|c| c.get("isDelisted").and_then(|x| x.as_bool()).unwrap_or(false)).filter_map(|c| c.get("name").and_then(|x| x.as_str()).map(String::from)).collect())
        .unwrap_or_default();
    let mut xyz_coins: Vec<(String, bool, f64)> = Vec::new();
    for (u, c) in uni.iter().zip(cxs.iter()) {
        let name = u.get("name").and_then(|x| x.as_str()).unwrap_or("?").to_string();
        let isd = delisted.contains(&name);
        xyz_coins.push((name, isd, c.get("dayNtlVlm").and_then(num).unwrap_or(0.0)));
    }
    let mut wanted: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for (_, base) in WANT {
        let name = format!("xyz:{base}");
        match xyz_coins.iter().find(|c| c.0 == name) {
            Some(c) if !c.1 => wanted.push(name),
            _ => dropped.push(name),
        }
    }
    let mut small: Vec<&(String, bool, f64)> = xyz_coins.iter().filter(|c| !c.1 && c.2 > 0.0 && report_class(&c.0) == "stock").collect();
    small.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
    let smallest5: Vec<(String, f64)> = small.iter().take(5).map(|c| (c.0.clone(), c.2)).collect();
    save_raw("setup.txt", &smallest5.iter().map(|(n, _)| format!("{n}\n")).collect::<String>());
    let mut watch: Vec<String> = wanted.clone();
    watch.extend(MAIN3.iter().map(|s| s.to_string()));
    for (n, _) in &smallest5 {
        if !watch.contains(n) {
            watch.push(n.clone());
        }
    }
    report.push_str(&format!(
        "## Setup\n\n- Working coins of the weekend list (not delisted today): {} ({} dropped as delisted/missing: {}).\n- Main dex control: BTC, ETH, SOL.\n- Five smallest active `xyz` stocks by 24 h notional (> 0): {}.\n- Subscribed `activeAssetCtx` coins: {} on 3 WebSocket connections (round robin), plus REST `metaAndAssetCtxs` dex `xyz` every 10 s.\n\n",
        wanted.len(), dropped.len(), dropped.join(", "),
        smallest5.iter().map(|(n, v)| format!("{n} (${})", f(*v, 0))).collect::<Vec<_>>().join(", "),
        watch.len()
    ));
    report.push_str("## Who sets the oracle (REST `perpDexs`, dex `xyz`)\n\n```json\n");
    report.push_str(&serde_json::to_string_pretty(&xyz_dex).unwrap_or_default());
    report.push_str("\n```\n\n");
    let _ = fs::write(out.join("oracle-report-tables.md"), &report);

    // ---------------------------------------------------------------- collection
    let msgs: Arc<Mutex<Vec<Msg>>> = Arc::new(Mutex::new(Vec::new()));
    let keysets: Arc<Mutex<BTreeMap<String, u64>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let stop_at = Instant::now() + Duration::from_secs(secs);
    eprintln!("== oracle: collecting for {secs} s from {}", utc_string(now_ms()));
    let mut handles = Vec::new();
    for k in 0..3usize {
        let coins: Vec<String> = watch.iter().enumerate().filter(|(i, _)| i % 3 == k).map(|(_, c)| c.clone()).collect();
        let subs: Vec<Value> = coins.iter().map(|c| json!({"type":"activeAssetCtx","coin":c})).collect();
        let (msgs, keysets) = (msgs.clone(), keysets.clone());
        handles.push(thread::spawn(move || {
            ws_loop(subs, Duration::from_secs(secs), |text, recv| {
                let Ok(v) = serde_json::from_str::<Value>(text) else { return };
                if v.get("channel").and_then(|c| c.as_str()) != Some("activeAssetCtx") {
                    return;
                }
                let Some(d) = v.get("data") else { return };
                let (Some(coin), Some(ctx)) = (d.get("coin").and_then(|c| c.as_str()), d.get("ctx")) else { return };
                if let (Ok(mut ks), Some(dm), Some(cm)) = (keysets.lock(), d.as_object(), ctx.as_object()) {
                    let top = v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default();
                    let key = format!("WS message keys [{top}]; data keys [{}]; ctx keys [{}]", dm.keys().cloned().collect::<Vec<_>>().join(","), cm.keys().cloned().collect::<Vec<_>>().join(","));
                    *ks.entry(key).or_insert(0) += 1;
                }
                let (Some(oracle), Some(mark)) = (ctx.get("oraclePx").and_then(num), ctx.get("markPx").and_then(num)) else { return };
                if let Ok(mut g) = msgs.lock() {
                    g.push(Msg { rest: false, recv_ms: recv, coin: coin.to_string(), oracle, mark, mid: ctx.get("midPx").and_then(num) });
                }
            })
        }));
    }
    // REST poller (dex xyz every 10 s)
    let rest_h = {
        let (rest, msgs, keysets, watch) = (rest.clone(), msgs.clone(), keysets.clone(), watch.clone());
        thread::spawn(move || {
            let (mut polls, mut errs) = (0u64, 0u64);
            let mut next = Instant::now();
            while Instant::now() < stop_at {
                let body = json!({"type":"metaAndAssetCtxs","dex":"xyz"});
                match rest.info(&body, 20) {
                    Ok((v, _)) => {
                        polls += 1;
                        let recv = now_ms();
                        let uni = v.get(0).and_then(|m| m.get("universe")).and_then(|u| u.as_array()).cloned().unwrap_or_default();
                        let cx = v.get(1).and_then(|c| c.as_array()).cloned().unwrap_or_default();
                        if let (Ok(mut ks), Some(first)) = (keysets.lock(), cx.first().and_then(|c| c.as_object())) {
                            *ks.entry(format!("REST metaAndAssetCtxs[1][i] keys [{}]; top-level array length {}", first.keys().cloned().collect::<Vec<_>>().join(","), v.as_array().map_or(0, |a| a.len()))).or_insert(0) += 1;
                        }
                        if let Ok(mut g) = msgs.lock() {
                            for (u, c) in uni.iter().zip(cx.iter()) {
                                let name = u.get("name").and_then(|x| x.as_str()).unwrap_or("");
                                if watch.iter().any(|w| w == name) {
                                    if let (Some(o), Some(m)) = (c.get("oraclePx").and_then(num), c.get("markPx").and_then(num)) {
                                        g.push(Msg { rest: true, recv_ms: recv, coin: name.to_string(), oracle: o, mark: m, mid: c.get("midPx").and_then(num) });
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        errs += 1;
                        eprintln!("!! REST poll failed: {e}");
                        if errs >= 3 {
                            break;
                        }
                    }
                }
                next += Duration::from_secs(10);
                let now = Instant::now();
                if next > now {
                    thread::sleep(next - now);
                }
            }
            (polls, errs)
        })
    };
    // GMX poller (one /prices/tickers per minute; /tokens once for the decimals)
    let gmx_h = thread::spawn(move || {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).user_agent(UA).build();
        let mut incidents: Vec<String> = Vec::new();
        let mut dec: HashMap<String, i32> = HashMap::new();
        if let Ok(r) = agent.get(&format!("{GMX_HOST}/tokens")).call() {
            if let Ok(Ok(Value::Object(o))) = r.into_string().map(|b| serde_json::from_str::<Value>(&b)) {
                for t in o.get("tokens").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
                    if let (Some(s), Some(d)) = (t.get("symbol").and_then(|x| x.as_str()), t.get("decimals").and_then(|x| x.as_i64())) {
                        dec.insert(s.to_string(), d as i32);
                    }
                }
            }
        }
        let mut rows: Vec<(u64, HashMap<String, f64>)> = Vec::new();
        // the first fetch waits for the WebSocket series to start, so that every GMX row has a Hyperliquid value to compare with
        let mut next = Instant::now() + Duration::from_secs(if quick() { 20 } else { 30 });
        thread::sleep(next.saturating_duration_since(Instant::now()));
        while Instant::now() < stop_at {
            let recv0 = now_ms();
            match agent.get(&format!("{GMX_HOST}/prices/tickers")).call() {
                Ok(r) => {
                    let body = r.into_string().unwrap_or_default();
                    let mut m = HashMap::new();
                    if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&body) {
                        for e in a {
                            let sym = e.get("tokenSymbol").and_then(|x| x.as_str()).unwrap_or("");
                            if !GMX_SYMS.contains(&sym) {
                                continue;
                            }
                            if let (Some(lo), Some(hi), Some(d)) = (e.get("minPrice").and_then(|x| x.as_str()), e.get("maxPrice").and_then(|x| x.as_str()), dec.get(sym)) {
                                if let (Ok(lo), Ok(hi)) = (lo.parse::<f64>(), hi.parse::<f64>()) {
                                    m.insert(sym.to_string(), (lo + hi) / 2.0 / 10f64.powi(30 - d));
                                }
                            }
                        }
                    }
                    rows.push((recv0, m));
                }
                Err(ureq::Error::Status(c, _)) if c == 429 || c == 403 => {
                    incidents.push(format!("{} GMX HTTP {c}", utc_string(now_ms())));
                    eprintln!("!! GMX HTTP {c}: backing off 60 s");
                    thread::sleep(Duration::from_secs(60));
                    if incidents.len() >= 2 {
                        break;
                    }
                }
                Err(e) => incidents.push(format!("{} GMX error: {e}", utc_string(now_ms()))),
            }
            next += Duration::from_secs(60);
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            }
        }
        (rows, incidents, dec)
    });
    let ws_stats: Vec<WsStats> = handles
        .into_iter()
        .map(|h| h.join().unwrap_or(WsStats { connects: 0, reconnects: 0, incidents: vec!["thread panicked".into()], messages: 0, first_messages: vec![], pongs: 0 }))
        .collect();
    let (rest_polls, rest_errs) = rest_h.join().unwrap_or((0, 99));
    let (gmx_rows, gmx_incidents, _gmx_dec) = gmx_h.join().unwrap_or((vec![], vec!["thread panicked".into()], HashMap::new()));
    let ended = now_ms();
    eprintln!("== oracle: collection done at {}", utc_string(ended));

    // ---------------------------------------------------------------- data
    let mut all: Vec<Msg> = msgs.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default();
    all.sort_by_key(|m| m.recv_ms);
    let mut csv = String::from("src,recv_ms,coin,oraclePx,markPx,midPx\n");
    for m in &all {
        csv.push_str(&format!("{},{},{},{},{},{}\n", if m.rest { "rest" } else { "ws" }, m.recv_ms, m.coin, m.oracle, m.mark, m.mid.map_or(String::new(), |x| x.to_string())));
    }
    let _ = fs::write(out.join("oracle-messages.csv"), &csv);
    let mut gcsv = String::from("recv_ms,symbol,usd\n");
    for (t, m) in &gmx_rows {
        for (s, v) in m {
            gcsv.push_str(&format!("{t},{s},{v}\n"));
        }
    }
    let _ = fs::write(out.join("gmx-tickers-1min.csv"), &gcsv);
    let mut ws_series: BTreeMap<String, Vec<OraclePoint>> = BTreeMap::new();
    let mut rest_series: BTreeMap<String, Vec<OraclePoint>> = BTreeMap::new();
    for m in &all {
        (if m.rest { &mut rest_series } else { &mut ws_series }).entry(m.coin.clone()).or_default().push(point(m));
    }
    let _ = fs::write(out.join("raw").join("ws-first-messages.txt"), ws_stats.iter().map(|w| w.first_messages.join("\n")).collect::<Vec<_>>().join("\n\n"));

    let mut s = String::new();
    // ---- 1. source
    s.push_str("## 1. Source and message format\n\n");
    s.push_str(&format!("Collection: {} to {} ({} s planned).\n\n", utc_string(started), utc_string(ended), secs));
    let ks = keysets.lock().map(|g| g.clone()).unwrap_or_default();
    s.push_str("Field sets seen (count of messages/polls):\n\n");
    for (k, n) in &ks {
        s.push_str(&format!("- `{k}`: {n}\n"));
    }
    s.push_str("\n");
    let has_time = ks.keys().any(|k| k.contains("time") || k.contains("Time") || k.contains("timestamp"));
    s.push_str(&format!(
        "**Server timestamp for `oraclePx` in these messages: {}.** (Looked for any key containing `time`/`timestamp` in the WebSocket envelope, `data` and `ctx` objects and in the REST context rows.)\n\n",
        if has_time { "A TIME-LIKE KEY WAS SEEN, see the field sets above" } else { "NO" }
    ));
    let rows: Vec<Vec<String>> = ws_stats
        .iter()
        .enumerate()
        .map(|(i, w)| vec![format!("ws {}", i + 1), w.connects.to_string(), w.reconnects.to_string(), w.messages.to_string(), w.pongs.to_string(), w.incidents.join("; ")])
        .collect();
    s.push_str(&table(&["connection", "connects", "reconnects", "messages", "pongs", "incidents"], &rows));
    s.push_str(&format!(
        "REST: {rest_polls} polls of `metaAndAssetCtxs` (dex xyz), {rest_errs} errors; REST weight total {}, max in a trailing minute {}; HTTP statuses {:?}. GMX: {} fetches, incidents {:?}.\n\n",
        rest.weight_total.lock().map(|g| *g).unwrap_or(0),
        rest.max_window_weight.lock().map(|g| *g).unwrap_or(0),
        rest.statuses.lock().map(|g| g.clone()).unwrap_or_default(),
        gmx_rows.len(),
        gmx_incidents
    ));
    let core: HashSet<String> = wanted.iter().cloned().chain(MAIN3.iter().map(|x| x.to_string())).collect();
    sections(&mut s, &ws_series, &rest_series, &gmx_rows, &xyz_coins, &smallest5, &core);
    report.push_str(&s);
    let _ = fs::write(out.join("oracle-report-tables.md"), &report);
    println!("{}", &report[report.find("## 1. Source").unwrap_or(0)..]);
}
