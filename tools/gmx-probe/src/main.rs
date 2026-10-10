//! GMX Oracle API probe (prompt P5). Read-only GETs, `User-Agent: setl8-probe/0.1`, at most 10 requests per second in
//! total, stop and back off 60 s on any 429/403. Raw data goes to `data/` (git-ignored); the markdown tables go to
//! `data/report-tables.md`. `PROBE_QUICK=1` shortens every phase for a smoke test.
use gmx_probe::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PRIMARY: &str = "arbitrum-api.gmxinfra.io";
const FALLBACKS: [&str; 2] = ["arbitrum-api-fallback.gmxinfra.io", "arbitrum-api-fallback.gmxinfra2.io"];
const SYMS13: [&str; 13] =
    ["BTC", "ETH", "SOL", "XRP", "DOGE", "LINK", "GOLD", "SILVER", "WTIOIL", "BRENTOIL", "NATGAS", "SPY", "QQQ"];
const TRACKED: [&str; 6] = ["BTC", "ETH", "SOL", "GOLD", "SPY", "WTIOIL"];
const CLOSED7: [&str; 7] = ["GOLD", "SILVER", "WTIOIL", "BRENTOIL", "NATGAS", "SPY", "QQQ"];
const MIN_INTERVAL: Duration = Duration::from_millis(100); // 10 requests per second in total
const UA: &str = "setl8-probe/0.1";

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn utc_string(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let weekday = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"][days.rem_euclid(7) as usize];
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC ({weekday})", rem / 3600, rem % 3600 / 60, rem % 60)
}

// ------------------------------------------------------------------ http with a global rate limiter

struct Resp {
    status: u16,
    body: String,
    sent_ms: u64,
    recv_ms: u64,
    latency_ms: u64,
}

struct Http {
    agent: ureq::Agent,
    last: Mutex<Instant>,
    halt: AtomicBool,
    incidents: Mutex<Vec<String>>,
    status_counts: Mutex<BTreeMap<u16, u64>>,
    errors: Mutex<Vec<String>>,
    requests: Mutex<u64>,
}

impl Http {
    fn new() -> Self {
        Http {
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).user_agent(UA).build(),
            last: Mutex::new(Instant::now() - MIN_INTERVAL),
            halt: AtomicBool::new(false),
            incidents: Mutex::new(Vec::new()),
            status_counts: Mutex::new(BTreeMap::new()),
            errors: Mutex::new(Vec::new()),
            requests: Mutex::new(0),
        }
    }

    fn halted(&self) -> bool {
        self.halt.load(Ordering::SeqCst)
    }

    fn get(&self, host: &str, path: &str) -> Result<Resp, String> {
        {
            // one global gate: requests start at least 100 ms apart, whatever thread sends them
            let mut last = self.last.lock().map_err(|_| "limiter poisoned".to_string())?;
            let since = last.elapsed();
            if since < MIN_INTERVAL {
                thread::sleep(MIN_INTERVAL - since);
            }
            *last = Instant::now();
        }
        if let Ok(mut n) = self.requests.lock() {
            *n += 1;
        }
        let url = format!("https://{host}{path}");
        let sent_ms = now_ms();
        let t = Instant::now();
        let (status, body) = match self.agent.get(&url).call() {
            Ok(r) => (r.status(), r.into_string().map_err(|e| format!("read body: {e}"))),
            Err(ureq::Error::Status(code, r)) => (code, r.into_string().map_err(|e| format!("read body: {e}"))),
            Err(e) => {
                let msg = format!("{host}{path}: {e}");
                if let Ok(mut v) = self.errors.lock() {
                    v.push(msg.clone());
                }
                return Err(msg);
            }
        };
        let latency_ms = t.elapsed().as_millis() as u64;
        let recv_ms = now_ms();
        if let Ok(mut m) = self.status_counts.lock() {
            *m.entry(status).or_insert(0) += 1;
        }
        if status == 429 || status == 403 {
            self.halt.store(true, Ordering::SeqCst);
            if let Ok(mut v) = self.incidents.lock() {
                v.push(format!("{} HTTP {status} on {host}{path}", utc_string(recv_ms)));
            }
        }
        let body = body?;
        Ok(Resp { status, body, sent_ms, recv_ms, latency_ms })
    }

    /// After a 429/403: wait 60 s with no traffic. A second incident aborts the run.
    fn backoff_if_halted(&self) -> Result<(), String> {
        if self.halted() {
            let n = self.incidents.lock().map(|v| v.len()).unwrap_or(0);
            eprintln!("!! 429/403 seen ({n} incident(s)): stopping the load, backing off 60 s");
            if n >= 2 {
                return Err("second 429/403: run aborted".into());
            }
            thread::sleep(Duration::from_secs(60));
            self.halt.store(false, Ordering::SeqCst);
        }
        Ok(())
    }
}

struct RawPoll {
    tick: usize,
    #[allow(dead_code)]
    sent_ms: u64,
    recv_ms: u64,
    latency_ms: u64,
    status: u16,
    body: String,
    err: Option<String>,
}

struct PollRun {
    polls: Vec<RawPoll>,
    skipped_ticks: usize,
    ticks: usize,
}

/// Sends one GET per `interval_ms` tick (in its own thread, at most `cap` in flight) for `duration_s` seconds.
fn run_poller(http: &Arc<Http>, host: &str, path: &str, interval_ms: u64, duration_s: u64, cap: usize) -> PollRun {
    let (tx, rx) = mpsc::channel::<RawPoll>();
    let inflight = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    let (mut tick, mut skipped) = (0usize, 0usize);
    while start.elapsed() < Duration::from_secs(duration_s) && !http.halted() {
        let due = start + Duration::from_millis(interval_ms * tick as u64);
        let now = Instant::now();
        if due > now {
            thread::sleep(due - now);
        }
        if inflight.load(Ordering::SeqCst) >= cap {
            skipped += 1;
        } else {
            inflight.fetch_add(1, Ordering::SeqCst);
            let (http, tx, inflight, host, path, t) = (http.clone(), tx.clone(), inflight.clone(), host.to_string(), path.to_string(), tick);
            thread::spawn(move || {
                let poll = match http.get(&host, &path) {
                    Ok(r) => RawPoll { tick: t, sent_ms: r.sent_ms, recv_ms: r.recv_ms, latency_ms: r.latency_ms, status: r.status, body: r.body, err: None },
                    Err(e) => RawPoll { tick: t, sent_ms: now_ms(), recv_ms: now_ms(), latency_ms: 0, status: 0, body: String::new(), err: Some(e) },
                };
                let _ = tx.send(poll);
                inflight.fetch_sub(1, Ordering::SeqCst);
            });
        }
        tick += 1;
    }
    drop(tx);
    let wait = Instant::now();
    while inflight.load(Ordering::SeqCst) > 0 && wait.elapsed() < Duration::from_secs(40) {
        thread::sleep(Duration::from_millis(50));
    }
    let mut polls: Vec<RawPoll> = rx.try_iter().collect();
    polls.sort_by_key(|p| p.tick);
    PollRun { polls, skipped_ticks: skipped, ticks: tick }
}

// ------------------------------------------------------------------ markdown helpers

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut s = format!("| {} |\n|{}|\n", headers.join(" | "), headers.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
    for r in rows {
        s.push_str(&format!("| {} |\n", r.join(" | ")));
    }
    s.push('\n');
    s
}

fn f(v: f64, d: usize) -> String {
    if v.is_finite() {
        format!("{v:.d$}")
    } else {
        "n/a".to_string()
    }
}

fn dir() -> PathBuf {
    let p = PathBuf::from("data");
    let _ = fs::create_dir_all(p.join("raw"));
    p
}

fn save(name: &str, body: &str) {
    let _ = fs::write(dir().join("raw").join(name), body);
}

fn json(body: &str) -> Option<Value> {
    serde_json::from_str(body).ok()
}

fn keys_of(arr: &[Value]) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();
    for v in arr {
        if let Some(o) = v.as_object() {
            set.extend(o.keys().cloned());
        }
    }
    let mut k: Vec<String> = set.into_iter().collect();
    k.sort();
    k
}

fn pct_i64(values: &mut [i64], p: f64) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let rank = ((p / 100.0) * values.len() as f64).ceil() as usize;
    values.get(rank.clamp(1, values.len()) - 1).copied()
}

// ------------------------------------------------------------------ data model

#[derive(Clone, Debug)]
struct TickRow {
    sym: String,
    min: u128,
    max: u128,
    updated_at: u64,
    ts: u64,
}

fn parse_tickers(body: &str) -> Option<Vec<TickRow>> {
    let v = json(body)?;
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        out.push(TickRow {
            sym: e.get("tokenSymbol")?.as_str()?.to_string(),
            min: e.get("minPrice")?.as_str()?.parse().ok()?,
            max: e.get("maxPrice")?.as_str()?.parse().ok()?,
            updated_at: e.get("updatedAt")?.as_u64()?,
            ts: e.get("timestamp").and_then(|t| t.as_u64()).unwrap_or(0),
        });
    }
    Some(out)
}

fn token_decimals(tokens: &Value) -> (HashMap<String, i32>, HashMap<String, String>) {
    let mut dec = HashMap::new();
    let mut addr_to_sym = HashMap::new();
    if let Some(arr) = tokens.get("tokens").and_then(|t| t.as_array()) {
        for t in arr {
            if let (Some(s), Some(d)) = (t.get("symbol").and_then(|x| x.as_str()), t.get("decimals").and_then(|x| x.as_i64())) {
                dec.insert(s.to_string(), d as i32);
                if let Some(a) = t.get("address").and_then(|x| x.as_str()) {
                    addr_to_sym.insert(a.to_lowercase(), s.to_string());
                }
            }
        }
    }
    (dec, addr_to_sym)
}

fn median_usd(row: &TickRow, dec: i32, od: Option<i32>) -> f64 {
    let lo = usd_price(&row.min.to_string(), dec, od).unwrap_or(f64::NAN);
    let hi = usd_price(&row.max.to_string(), dec, od).unwrap_or(f64::NAN);
    (lo + hi) / 2.0
}

fn quick() -> bool {
    std::env::var("PROBE_QUICK").is_ok()
}

fn main() {
    let t_run = Instant::now();
    let started_ms = now_ms();
    let http = Arc::new(Http::new());
    let mut report = String::new();
    report.push_str(&format!("# GMX probe raw tables\n\nRun started: {}\nMode: {}\n\n", utc_string(started_ms), if quick() { "QUICK (smoke test)" } else { "full" }));
    let mut abort: Option<String> = None;

    macro_rules! phase {
        ($name:expr, $body:expr) => {
            if abort.is_none() {
                eprintln!("== {} ({}s elapsed)", $name, t_run.elapsed().as_secs());
                let r: Result<String, String> = (|| $body)();
                match r {
                    Ok(s) => report.push_str(&s),
                    Err(e) => {
                        report.push_str(&format!("\n**PHASE {} FAILED: {e}**\n\n", $name));
                        if e.contains("aborted") {
                            abort = Some(e);
                        }
                    }
                }
                if let Err(e) = http.backoff_if_halted() {
                    abort = Some(e);
                }
                let _ = fs::write(dir().join("report-tables.md"), &report);
            }
        };
    }

    // shared across phases
    let tokens_dec: Arc<Mutex<(HashMap<String, i32>, HashMap<String, String>)>> = Arc::new(Mutex::new((HashMap::new(), HashMap::new())));
    let state_snapshots: Arc<Mutex<Vec<(u64, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let info_body: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let ticker_series: Arc<Mutex<Vec<(u64, Vec<TickRow>)>>> = Arc::new(Mutex::new(Vec::new()));

    // ---------------------------------------------------------------- 1. hosts
    phase!("1 hosts", {
        let n = if quick() { 3 } else { 20 };
        let mut rows = Vec::new();
        let mut snap: Vec<(String, Option<Value>, Option<Vec<TickRow>>)> = Vec::new();
        for host in std::iter::once(PRIMARY).chain(FALLBACKS.iter().copied()) {
            let (mut lat, mut ok, mut fail) = (Vec::new(), 0, 0);
            for _ in 0..n {
                match http.get(host, "/ping") {
                    Ok(r) if r.status == 200 && r.body.contains("ok") => {
                        ok += 1;
                        lat.push(r.latency_ms);
                    }
                    Ok(r) => {
                        fail += 1;
                        lat.push(r.latency_ms);
                    }
                    Err(_) => fail += 1,
                }
                thread::sleep(Duration::from_millis(200));
                if http.halted() {
                    break;
                }
            }
            let (p50, p95, p99) = latency_percentiles(&lat).unwrap_or((0, 0, 0));
            rows.push(vec![host.to_string(), format!("{ok}/{n}"), fail.to_string(), p50.to_string(), p95.to_string(), p99.to_string(), lat.iter().max().copied().unwrap_or(0).to_string()]);
            let tok = http.get(host, "/tokens").ok().and_then(|r| json(&r.body));
            let tk = http.get(host, "/prices/tickers").ok().and_then(|r| parse_tickers(&r.body));
            snap.push((host.to_string(), tok, tk));
        }
        let mut s = String::from("## 1. Hosts\n\n/ping x N per host, 200 ms apart (latency in ms, full request time).\n\n");
        s.push_str(&table(&["host", "ok", "failed", "p50", "p95", "p99", "max"], &rows));
        // identical data?
        let mut rows = Vec::new();
        if let Some((_, ptok, ptk)) = snap.first().cloned() {
            for (host, tok, tk) in snap.iter().skip(1) {
                let tokens_same = tok.is_some() && *tok == ptok;
                let (mut same_syms, mut max_rel, mut max_dt) = (false, 0.0f64, 0i64);
                if let (Some(a), Some(b)) = (&ptk, tk) {
                    let sa: HashSet<&str> = a.iter().map(|r| r.sym.as_str()).collect();
                    let sb: HashSet<&str> = b.iter().map(|r| r.sym.as_str()).collect();
                    same_syms = sa == sb;
                    let bm: HashMap<&str, &TickRow> = b.iter().map(|r| (r.sym.as_str(), r)).collect();
                    for r in a {
                        if let Some(o) = bm.get(r.sym.as_str()) {
                            let (x, y) = (((r.min + r.max) / 2) as f64, ((o.min + o.max) / 2) as f64);
                            max_rel = max_rel.max(rel_err(x, y));
                            max_dt = max_dt.max((r.updated_at as i64 - o.updated_at as i64).abs());
                        }
                    }
                }
                rows.push(vec![host.clone(), tokens_same.to_string(), same_syms.to_string(), f(max_rel * 10_000.0, 2), max_dt.to_string()]);
            }
        }
        s.push_str("Fallbacks vs primary, fetched back to back (not at the same instant):\n\n");
        s.push_str(&table(&["fallback", "/tokens identical", "same ticker symbols", "max median diff (bp)", "max updatedAt diff (ms)"], &rows));
        Ok(s)
    });

    // ---------------------------------------------------------------- 2. static shape
    phase!("2 static shape", {
        let mut s = String::from("## 2. Static shape (primary host)\n\n");
        let mut fetched: HashMap<&str, Value> = HashMap::new();
        for (name, path) in [("tokens", "/tokens"), ("tickers", "/prices/tickers"), ("signed", "/signed_prices/latest"), ("state", "/markets/state"), ("info", "/markets/info")] {
            let r = http.get(PRIMARY, path)?;
            save(&format!("{name}.json"), &r.body);
            if name == "state" {
                if let Ok(mut v) = state_snapshots.lock() {
                    v.push((r.recv_ms, r.body.clone()));
                }
            }
            if name == "info" {
                if let Ok(mut v) = info_body.lock() {
                    *v = r.body.clone();
                }
            }
            if let Some(v) = json(&r.body) {
                fetched.insert(name, v);
            }
            s.push_str(&format!("- `{path}`: HTTP {}, {} bytes, {} ms\n", r.status, r.body.len(), r.latency_ms));
        }
        s.push('\n');
        if let Some(t) = fetched.get("tokens") {
            if let Ok(mut g) = tokens_dec.lock() {
                *g = token_decimals(t);
            }
        }
        let list = |v: &Value, key: Option<&str>| -> Vec<Value> {
            match key {
                Some(k) => v.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default(),
                None => v.as_array().cloned().unwrap_or_default(),
            }
        };
        let tokens = fetched.get("tokens").map(|v| list(v, Some("tokens"))).unwrap_or_default();
        let tickers = fetched.get("tickers").map(|v| list(v, None)).unwrap_or_default();
        let signed = fetched.get("signed").map(|v| list(v, Some("signedPrices"))).unwrap_or_default();
        let state = fetched.get("state").map(|v| list(v, Some("markets"))).unwrap_or_default();
        let info = fetched.get("info").map(|v| list(v, Some("markets"))).unwrap_or_default();
        let docs_tickers = ["tokenSymbol", "tokenAddress", "minPrice", "maxPrice", "medianPrice", "oracleDecimals", "updatedAt"];
        let docs_signed = ["tokenSymbol", "tokenAddress", "minPrice", "maxPrice", "medianPrice", "oracleDecimals", "signer", "signature", "signatureWithoutBlockHash", "salt", "minBlockHash", "maxBlockHash", "tokenOracleType", "minBlockNumber", "maxBlockNumber", "minBlockTimestamp", "maxBlockTimestamp", "createdAt", "blob", "oracleType", "feedVersion"];
        let docs_state = ["marketToken", "status", "closingPrice", "latestMarketStatusTransitionTimestamp"];
        let mut rows = Vec::new();
        for (name, arr, docs) in [
            ("/prices/tickers", &tickers, docs_tickers.to_vec()),
            ("/signed_prices/latest", &signed, docs_signed.to_vec()),
            ("/markets/state", &state, docs_state.to_vec()),
        ] {
            let live = keys_of(arr);
            let missing: Vec<&str> = docs.iter().filter(|d| !live.iter().any(|l| l == *d)).copied().collect();
            let extra: Vec<&String> = live.iter().filter(|l| !docs.contains(&l.as_str())).collect();
            rows.push(vec![name.to_string(), arr.len().to_string(), format!("{} fields: {}", live.len(), live.join(", ")), missing.join(", "), extra.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(", ")]);
        }
        rows.push(vec!["/tokens".into(), tokens.len().to_string(), keys_of(&tokens).join(", "), "(undocumented)".into(), "".into()]);
        rows.push(vec!["/markets/info".into(), info.len().to_string(), keys_of(&info).join(", "), "(undocumented)".into(), "".into()]);
        s.push_str("Fields live vs the docs (documented-but-absent and present-but-undocumented):\n\n");
        s.push_str(&table(&["endpoint", "entries", "live fields", "in docs, NOT live", "live, NOT in docs"], &rows));
        // null-ness of suspicious fields
        let nulls = |arr: &[Value], k: &str| arr.iter().filter(|e| e.get(k).map_or(true, |v| v.is_null())).count();
        s.push_str(&format!(
            "Null counts in signed prices ({} entries): oracleDecimals null {}, minPrice null {}, signer null {}, signature null {}, blob null {}, minPriceFull null {}.\n\n",
            signed.len(), nulls(&signed, "oracleDecimals"), nulls(&signed, "minPrice"), nulls(&signed, "signer"), nulls(&signed, "signature"), nulls(&signed, "blob"), nulls(&signed, "minPriceFull")
        ));
        let sym_in = |arr: &[Value], key: &str, sym: &str| arr.iter().any(|e| e.get(key).and_then(|x| x.as_str()) == Some(sym));
        let mut rows = Vec::new();
        for sym in SYMS13 {
            let c = http.get(PRIMARY, &format!("/prices/candles?tokenSymbol={sym}&period=1m&limit=2"))?;
            let cv = json(&c.body);
            let n = cv.as_ref().and_then(|v| v.get("candles")).and_then(|v| v.as_array()).map_or(0, |a| a.len());
            rows.push(vec![
                sym.to_string(),
                sym_in(&tickers, "tokenSymbol", sym).to_string(),
                sym_in(&signed, "tokenSymbol", sym).to_string(),
                format!("HTTP {} / {} candles", c.status, n),
                sym_in(&tokens, "symbol", sym).to_string(),
            ]);
        }
        s.push_str("Coverage of the 13 symbols:\n\n");
        s.push_str(&table(&["symbol", "in tickers", "in signed_prices", "candles (1m, limit 2)", "in /tokens"], &rows));
        Ok(s)
    });

    // ---------------------------------------------------------------- 3. price scaling
    phase!("3 price scaling", {
        let (dec, _) = tokens_dec.lock().map(|g| g.clone()).map_err(|_| "lock".to_string())?;
        let mut candles: Vec<(String, f64)> = Vec::new();
        for sym in SYMS13 {
            let c = http.get(PRIMARY, &format!("/prices/candles?tokenSymbol={sym}&period=1m&limit=2"))?;
            if let Some(close) = json(&c.body).and_then(|v| v.get("candles")?.as_array()?.first()?.as_array()?.get(4)?.as_f64()) {
                candles.push((sym.to_string(), close));
            }
        }
        let tk = http.get(PRIMARY, "/prices/tickers")?;
        let rows_t = parse_tickers(&tk.body).ok_or("tickers parse")?;
        let by: HashMap<&str, &TickRow> = rows_t.iter().map(|r| (r.sym.as_str(), r)).collect();
        let (mut ok_a, mut ok_b, mut table_rows) = (0, 0, Vec::new());
        for (sym, close) in &candles {
            if let (Some(row), Some(d)) = (by.get(sym.as_str()), dec.get(sym)) {
                let a = median_usd(row, *d, None);
                let b = median_usd(row, *d, Some(8));
                let (ea, eb) = (rel_err(a, *close), rel_err(b, *close));
                if ea < 0.005 {
                    ok_a += 1;
                }
                if eb < 0.005 {
                    ok_b += 1;
                }
                table_rows.push(vec![sym.clone(), d.to_string(), format!("{}/{}", row.min, row.max), f(a, 6), f(b, 6), f(*close, 6), format!("{}%", f(ea * 100.0, 4)), format!("{}%", f(eb * 100.0, 4))]);
            }
        }
        let mut s = String::from("## 3. Price scaling\n\n");
        s.push_str("Formula A = v / 10^(30 - tokenDecimals); formula B = v / 10^(30 - tokenDecimals - oracleDecimals). The tickers carry NO oracleDecimals (and signed prices have it null), so B is evaluated with a hypothetical oracleDecimals = 8. Median = (minPrice + maxPrice) / 2 (the live tickers have no medianPrice). Candle = close of the newest 1m candle (cached up to 15 s).\n\n");
        s.push_str(&table(&["symbol", "tokenDecimals", "min/max raw", "A: USD", "B (od=8): USD", "1m candle close", "A rel err", "B rel err"], &table_rows));
        s.push_str(&format!("Within 0.5%: formula A {ok_a}/{} tokens, formula B {ok_b}/{}.\n\n", candles.len(), candles.len()));
        let mut rows = Vec::new();
        for sym in ["BTC", "ETH", "SOL", "GOLD"] {
            if let (Some(row), Some(d)) = (by.get(sym), dec.get(sym)) {
                let (lo, hi, med) = (usd_price(&row.min.to_string(), *d, None).unwrap_or(f64::NAN), usd_price(&row.max.to_string(), *d, None).unwrap_or(f64::NAN), median_usd(row, *d, None));
                rows.push(vec![sym.to_string(), f(lo, 6), f(hi, 6), f(med, 6), f(spread_bps(lo, hi, med), 3)]);
            }
        }
        s.push_str("Spread (max - min) / median at the sample time (formula A):\n\n");
        s.push_str(&table(&["symbol", "min USD", "max USD", "median USD", "spread (bp)"], &rows));
        Ok(s)
    });

    // ---------------------------------------------------------------- 4. cadence (+ 8. storage)
    let mut series_done = false;
    phase!("4 cadence", {
        let secs = if quick() { 20 } else { 600 };
        let run = run_poller(&http, PRIMARY, "/prices/tickers", 250, secs, 8);
        let mut ok = Vec::new();
        let (mut errs, mut non200) = (0usize, 0usize);
        for p in &run.polls {
            if p.err.is_some() {
                errs += 1;
            } else if p.status != 200 {
                non200 += 1;
            } else if let Some(rows) = parse_tickers(&p.body) {
                ok.push((p, rows));
            } else {
                errs += 1;
            }
        }
        let lat: Vec<u64> = ok.iter().map(|(p, _)| p.latency_ms).collect();
        let (p50, p95, p99) = latency_percentiles(&lat).unwrap_or((0, 0, 0));
        let mut s = String::from("## 4. Update cadence (/prices/tickers every 250 ms)\n\n");
        s.push_str(&format!(
            "Duration {secs} s, {} ticks, {} skipped (more than 8 in flight), {} ok responses, {} transport errors/parse failures, {} non-200. Latency ms: p50 {p50}, p95 {p95}, p99 {p99}, max {}.\n\n",
            run.ticks, run.skipped_ticks, ok.len(), errs, non200, lat.iter().max().copied().unwrap_or(0)
        ));
        // clock relation
        let mut off: Vec<i64> = ok.iter().filter_map(|(p, r)| r.first().map(|x| p.recv_ms as i64 - x.updated_at as i64)).collect();
        let mut off_s: Vec<i64> = ok.iter().filter_map(|(p, r)| r.first().map(|x| p.recv_ms as i64 / 1000 - x.ts as i64)).collect();
        s.push_str(&format!(
            "`updatedAt` units: 13-digit values; client receive time minus `updatedAt` (ms): p50 {:?}, p95 {:?}, min {:?}, max {:?} -> MILLISECONDS. `timestamp` (separate field) is seconds: receive time (s) minus `timestamp`: p50 {:?}, max {:?}.\n\n",
            pct_i64(&mut off, 50.0), pct_i64(&mut off, 95.0), off.iter().min(), off.iter().max(), pct_i64(&mut off_s, 50.0), off_s.iter().max()
        ));
        // per symbol series in tick order
        let all_syms: Vec<String> = ok.first().map(|(_, r)| r.iter().map(|x| x.sym.clone()).collect()).unwrap_or_default();
        let minutes = (ok.last().map(|(p, _)| p.recv_ms).unwrap_or(0).saturating_sub(ok.first().map(|(p, _)| p.recv_ms).unwrap_or(0))) as f64 / 60_000.0;
        let mut rows = Vec::new();
        let mut closed_rows = Vec::new();
        let mut bin: Vec<u8> = Vec::new();
        let mut bin_price: Vec<u8> = Vec::new();
        let mut stored_price_syms = 0usize;
        let mut csv = String::from("market,recv_unix_ms,updated_at_ms,bid_raw,ask_raw,median_raw\n");
        let mut stored_syms = 0usize;
        let idx_of: HashMap<&str, u16> = all_syms.iter().enumerate().map(|(i, s)| (s.as_str(), i as u16)).collect();
        for sym in &all_syms {
            let mut last: Option<(u64, u128, u128)> = None;
            let (mut changes, mut price_changes) = (Vec::new(), Vec::new());
            let (mut max_seen, mut out_of_order, mut dup_updated, mut updated_values) = (0u64, 0usize, 0usize, HashSet::new());
            let mut stored_any = false;
            let mut ooo_recv = 0usize;
            let mut max_recv = 0u64;
            let mut by_recv: Vec<(u64, u64)> = ok.iter().filter_map(|(p, rt)| rt.iter().find(|x| &x.sym == sym).map(|r| (p.recv_ms, r.updated_at))).collect();
            by_recv.sort_by_key(|x| x.0);
            for (_, u) in &by_recv {
                if *u < max_recv {
                    ooo_recv += 1;
                }
                max_recv = max_recv.max(*u);
            }
            let mut price_stored = false;
            let mut last_price: Option<(u128, u128)> = None;
            // Archive rule: a sample counts only if its updatedAt is newer than anything seen for the symbol (monotone,
            // append-only); older or equal updatedAt values (out-of-order answers, repeats) are not stored.
            for (p, rows_t) in &ok {
                if let Some(r) = rows_t.iter().find(|x| &x.sym == sym) {
                    let prev_max = max_seen;
                    if r.updated_at < prev_max {
                        out_of_order += 1;
                    }
                    if TRACKED.contains(&sym.as_str()) {
                        csv.push_str(&csv_line(sym, p.recv_ms, &Record::new(idx_of[sym.as_str()], r.updated_at, r.min, r.max)));
                        csv.push('\n');
                    }
                    if r.updated_at == prev_max {
                        if let Some((_, mn, mx)) = last {
                            if (mn, mx) != (r.min, r.max) {
                                dup_updated += 1;
                            }
                        }
                    }
                    if r.updated_at > prev_max {
                        max_seen = r.updated_at;
                        let changed_price = last_price != Some((r.min, r.max));
                        if changed_price {
                            price_changes.push(r.updated_at);
                            bin_price.extend_from_slice(&Record::new(idx_of[sym.as_str()], r.updated_at, r.min, r.max).to_bytes());
                            last_price = Some((r.min, r.max));
                            price_stored = true;
                        }
                        changes.push(r.updated_at);
                        updated_values.insert(r.updated_at);
                        bin.extend_from_slice(&Record::new(idx_of[sym.as_str()], r.updated_at, r.min, r.max).to_bytes());
                        stored_any = true;
                        last = Some((r.updated_at, r.min, r.max));
                    }
                }
            }
            if stored_any {
                stored_syms += 1;
            }
            if price_stored {
                stored_price_syms += 1;
            }
            let gs = gap_stats(&price_changes);
            let per_min = |n: usize| if minutes > 0.0 { n as f64 / minutes } else { 0.0 };
            let row = vec![
                sym.clone(),
                f(per_min(price_changes.len().saturating_sub(1)), 1),
                f(per_min(updated_values.len().saturating_sub(1)), 1),
                gs.map_or("n/a".into(), |g| g.median.to_string()),
                gs.map_or("n/a".into(), |g| g.p95.to_string()),
                gs.map_or("n/a".into(), |g| g.max.to_string()),
                format!("{out_of_order} / {ooo_recv}"),
                dup_updated.to_string(),
            ];
            if TRACKED.contains(&sym.as_str()) {
                rows.push(row.clone());
            }
            if CLOSED7.contains(&sym.as_str()) {
                closed_rows.push(row);
            }
        }
        // order the tracked rows as requested
        rows.sort_by_key(|r| TRACKED.iter().position(|t| *t == r[0]).unwrap_or(99));
        s.push_str("Per-symbol cadence (observation window ");
        s.push_str(&format!("{} min; gaps are between consecutive PRICE changes, in ms of server `updatedAt`):\n\n", f(minutes, 2)));
        s.push_str(&table(&["symbol", "price changes / min", "distinct updatedAt / min", "gap median ms", "gap p95 ms", "gap max ms", "out-of-order updatedAt (by send order / by receive order)", "same updatedAt, different price"], &rows));
        // global updatedAt cycle
        let mut global: Vec<u64> = ok.iter().filter_map(|(_, r)| r.first().map(|x| x.updated_at)).collect();
        global.sort_unstable();
        global.dedup();
        let gg = gap_stats(&global);
        s.push_str(&format!(
            "Server-wide `updatedAt` (first ticker row) took {} distinct values; gap median {:?} ms, p95 {:?} ms, max {:?} ms.\n\n",
            global.len(), gg.map(|g| g.median), gg.map(|g| g.p95), gg.map(|g| g.max)
        ));
        let mut distinct_per: Vec<i64> = ok.iter().map(|(_, r)| r.iter().map(|x| x.updated_at).collect::<HashSet<_>>().len() as i64).collect();
        let (dmin, dmax) = (distinct_per.iter().min().copied(), distinct_per.iter().max().copied());
        let dmed = pct_i64(&mut distinct_per, 50.0);
        s.push_str(&format!("Distinct updatedAt values inside ONE response: min {dmin:?}, median {dmed:?}, max {dmax:?} (one row per ticker).\n\n"));
        let all_same = ok.iter().filter(|(_, r)| r.iter().map(|x| x.updated_at).collect::<HashSet<_>>().len() == 1).count();
        s.push_str(&format!("Responses where ALL tickers carry the same updatedAt: {} of {}.\n\n", all_same, ok.len()));
        // closed-market behaviour (data only, status table in phase 5)
        s.push_str("### Closed-market symbols: do their prices still change? (same window)\n\n");
        s.push_str(&table(&["symbol", "price changes / min", "distinct updatedAt / min", "gap median ms", "gap p95 ms", "gap max ms", "out-of-order (send / receive order)", "dup updatedAt"], &closed_rows));
        // storage
        let bin_path = dir().join("samples-changes.bin");
        let csv_path = dir().join("samples-tracked-all-polls.csv");
        let binp_path = dir().join("samples-price-changes.bin");
        fs::write(&binp_path, &bin_price).map_err(|e| e.to_string())?;
        fs::write(&bin_path, &bin).map_err(|e| e.to_string())?;
        fs::write(&csv_path, &csv).map_err(|e| e.to_string())?;
        let mut sums = String::new();
        for p in [&bin_path, &binp_path, &csv_path] {
            let bytes = fs::read(p).map_err(|e| e.to_string())?;
            sums.push_str(&format!("{}  {}\n", hex(&Sha256::digest(&bytes)), p.file_name().and_then(|x| x.to_str()).unwrap_or("?")));
        }
        let _ = fs::write(dir().join("SHA256SUMS"), &sums);
        let window_min = (secs as f64 / 60.0).max(0.001);
        let scale = 10.0 / window_min;
        let bin_per10 = bin.len() as f64 * scale;
        let csv_per10 = csv.len() as f64 * scale;
        let binp_per10 = bin_price.len() as f64 * scale;
        let binp_per_market_month = binp_per10 / stored_price_syms.max(1) as f64 * 4320.0;
        let months_blocks = 4320.0; // 144 x 30 // 10-minute blocks in 30 days
        let bin_per_market_month = bin_per10 / stored_syms.max(1) as f64 * months_blocks;
        let csv_per_market_month = csv_per10 / TRACKED.len() as f64 * months_blocks;
        s.push_str("\n### Sample files (DESIGN 4.5 shape: market, timestamp, bid = minPrice, ask = maxPrice, median)\n\n");
        s.push_str(&table(
            &["file", "bytes", "rows", "bytes per 10 min", "per market per month (30 d)", "x 100 markets per month"],
            &[
                vec![
                    "samples-changes.bin (64-byte records, ALL tickers, only when (updatedAt, min, max) changes)".into(),
                    bin.len().to_string(),
                    (bin.len() / RECORD_BYTES).to_string(),
                    f(bin_per10, 0),
                    format!("{} MB", f(bin_per_market_month / 1e6, 2)),
                    format!("{} GB", f(bin_per_market_month * 100.0 / 1e9, 2)),
                ],
                vec![
                    "samples-price-changes.bin (64-byte records, ALL tickers, only when (minPrice, maxPrice) changes)".into(),
                    bin_price.len().to_string(),
                    (bin_price.len() / RECORD_BYTES).to_string(),
                    f(binp_per10, 0),
                    format!("{} MB", f(binp_per_market_month / 1e6, 2)),
                    format!("{} GB", f(binp_per_market_month * 100.0 / 1e9, 2)),
                ],
                vec![
                    "samples-tracked-all-polls.csv (6 tracked symbols, EVERY poll)".into(),
                    csv.len().to_string(),
                    csv.lines().count().saturating_sub(1).to_string(),
                    f(csv_per10, 0),
                    format!("{} MB", f(csv_per_market_month / 1e6, 2)),
                    format!("{} GB", f(csv_per_market_month * 100.0 / 1e9, 2)),
                ],
            ],
        ));
        s.push_str(&format!("sha256 of both files is in data/SHA256SUMS:\n\n```\n{sums}```\n\n"));
        if let Ok(mut g) = ticker_series.lock() {
            *g = ok.iter().map(|(p, r)| (p.recv_ms, r.clone())).collect();
        }
        series_done = true;
        Ok(s)
    });
    let _ = series_done;

    // ---------------------------------------------------------------- 5. closed markets
    phase!("5 closed markets", {
        let r = http.get(PRIMARY, "/markets/state")?;
        save("state-end.json", &r.body);
        if let Ok(mut v) = state_snapshots.lock() {
            v.push((r.recv_ms, r.body.clone()));
        }
        let (dec, addr_to_sym) = tokens_dec.lock().map(|g| g.clone()).map_err(|_| "lock".to_string())?;
        let info = info_body.lock().map(|g| g.clone()).map_err(|_| "lock".to_string())?;
        let info_v = json(&info).unwrap_or(Value::Null);
        let mut token_to_sym: HashMap<String, String> = HashMap::new(); // marketToken -> index symbol
        let mut mname: HashMap<String, String> = HashMap::new();
        for m in info_v.get("markets").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
            if let (Some(mt), Some(it)) = (m.get("marketToken").and_then(|x| x.as_str()), m.get("indexToken").and_then(|x| x.as_str())) {
                if let Some(sym) = addr_to_sym.get(&it.to_lowercase()) {
                    token_to_sym.insert(mt.to_lowercase(), sym.clone());
                }
                mname.insert(mt.to_lowercase(), m.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string());
            }
        }
        let snaps = state_snapshots.lock().map(|g| g.clone()).map_err(|_| "lock".to_string())?;
        let last_tick: Option<TickRow> = None;
        let _ = last_tick;
        let series = ticker_series.lock().map(|g| g.clone()).map_err(|_| "lock".to_string())?;
        let mut s = format!("## 5. Closed markets\n\nRun wall-clock: start {} / this phase {}.\n\n", utc_string(started_ms), utc_string(now_ms()));
        let mut status_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut rows = Vec::new();
        for (when, body) in &snaps {
            let v = json(body).unwrap_or(Value::Null);
            let arr = v.get("markets").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            status_counts.clear();
            for m in &arr {
                *status_counts.entry(format!("{} / closed={}", m.get("status").and_then(|x| x.as_str()).unwrap_or("?"), m.get("isMarketClosed").map_or("?".to_string(), |x| x.to_string()))).or_insert(0) += 1;
            }
            for sym in CLOSED7 {
                for m in &arr {
                    let mt = m.get("marketToken").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                    if token_to_sym.get(&mt).map(|x| x.as_str()) == Some(sym) {
                        let cp = m.get("closingPrice");
                        let cp_usd = cp.and_then(|c| c.as_str()).and_then(|c| usd_price(c, *dec.get(sym).unwrap_or(&18), None));
                        let latest = series.last().and_then(|(_, r)| r.iter().find(|x| x.sym == sym)).map(|r| median_usd(r, *dec.get(sym).unwrap_or(&18), None));
                        rows.push(vec![
                            utc_string(*when)[11..19].to_string(),
                            sym.to_string(),
                            mname.get(&mt).cloned().unwrap_or_default(),
                            m.get("status").map_or("?".into(), |x| x.to_string()),
                            m.get("isMarketClosed").map_or("?".into(), |x| x.to_string()),
                            cp.map_or("?".into(), |x| x.to_string()),
                            cp_usd.map_or("n/a".into(), |x| f(x, 4)),
                            latest.map_or("n/a".into(), |x| f(x, 4)),
                            m.get("latestMarketStatusTransitionTimestamp").map_or("?".into(), |x| x.to_string()),
                        ]);
                    }
                }
            }
            s.push_str(&format!("/markets/state at {}: {} markets, status counts {:?}\n\n", utc_string(*when), arr.len(), status_counts));
        }
        s.push_str(&table(&["UTC time", "symbol", "market name", "status", "isMarketClosed", "closingPrice (raw)", "closingPrice USD (formula A)", "ticker median USD (last poll)", "latestMarketStatusTransitionTimestamp"], &rows));
        Ok(s)
    });

    // ---------------------------------------------------------------- 6. signed prices
    phase!("6 signed prices", {
        let secs = if quick() { 10 } else { 180 };
        let (dec, _) = tokens_dec.lock().map(|g| g.clone()).map_err(|_| "lock".to_string())?;
        let run = run_poller(&http, PRIMARY, "/signed_prices/latest", 1000, secs, 4);
        struct Entry {
            sym: String,
            created_ms: Option<u64>,
            min_bt: Option<i64>,
            max_bt: Option<i64>,
            blob: Option<String>,
            min_full: Option<String>,
            max_full: Option<String>,
        }
        let mut polls: Vec<(u64, Vec<Entry>)> = Vec::new();
        let (mut bad, mut non200) = (0usize, 0usize);
        for p in &run.polls {
            if p.err.is_some() {
                bad += 1;
                continue;
            }
            if p.status != 200 {
                non200 += 1;
                continue;
            }
            let Some(v) = json(&p.body) else {
                bad += 1;
                continue;
            };
            let arr = v.get("signedPrices").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let ents = arr
                .iter()
                .map(|e| Entry {
                    sym: e.get("tokenSymbol").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
                    created_ms: e.get("createdAt").and_then(|x| x.as_str()).and_then(parse_iso_ms),
                    min_bt: e.get("minBlockTimestamp").and_then(|x| x.as_i64()),
                    max_bt: e.get("maxBlockTimestamp").and_then(|x| x.as_i64()),
                    blob: e.get("blob").and_then(|x| x.as_str()).map(|x| x.to_string()),
                    min_full: e.get("minPriceFull").and_then(|x| x.as_str()).map(|x| x.to_string()),
                    max_full: e.get("maxPriceFull").and_then(|x| x.as_str()).map(|x| x.to_string()),
                })
                .collect();
            polls.push((p.recv_ms, ents));
        }
        let lat: Vec<u64> = run.polls.iter().filter(|p| p.status == 200).map(|p| p.latency_ms).collect();
        let (p50, p95, p99) = latency_percentiles(&lat).unwrap_or((0, 0, 0));
        let mut s = String::from("## 6. Signed prices (/signed_prices/latest, once per second)\n\n");
        let counts: Vec<usize> = polls.iter().map(|(_, e)| e.len()).collect();
        let union: HashSet<&str> = polls.iter().flat_map(|(_, e)| e.iter().map(|x| x.sym.as_str())).collect();
        s.push_str(&format!(
            "Duration {secs} s, {} ticks, {} skipped, {} ok, {bad} errors, {non200} non-200; latency ms p50 {p50}, p95 {p95}, p99 {p99}. Tokens per response: min {:?}, max {:?}; union over the run: {}.\n\n",
            run.ticks, run.skipped_ticks, polls.len(), counts.iter().min(), counts.iter().max(), union.len()
        ));
        // ages
        let mut rows = Vec::new();
        let mut groups: Vec<(String, Vec<&str>)> = vec![("ALL tokens".to_string(), vec![])];
        for sym in ["BTC", "ETH", "SOL", "GOLD", "SPY"] {
            groups.push((sym.to_string(), vec![sym]));
        }
        for (name, syms) in &groups {
            let (mut created, mut maxbt, mut minbt, mut span) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for (recv, ents) in &polls {
                for e in ents.iter().filter(|e| syms.is_empty() || syms.contains(&e.sym.as_str())) {
                    if let Some(c) = e.created_ms {
                        created.push(*recv as i64 - c as i64);
                    }
                    if let Some(b) = e.max_bt {
                        maxbt.push(*recv as i64 - b * 1000);
                    }
                    if let Some(b) = e.min_bt {
                        minbt.push(*recv as i64 - b * 1000);
                    }
                    if let (Some(a), Some(b)) = (e.min_bt, e.max_bt) {
                        span.push(b - a);
                    }
                }
            }
            let g = |v: &mut Vec<i64>| format!("{:?} / {:?} / {:?}", pct_i64(v, 50.0), pct_i64(v, 95.0), v.iter().max());
            rows.push(vec![name.clone(), g(&mut created), g(&mut maxbt), g(&mut minbt), format!("{:?} / {:?}", span.iter().min(), span.iter().max())]);
        }
        s.push_str("Age versus the client wall clock at receipt, ms (p50 / p95 / max):\n\n");
        s.push_str(&table(&["tokens", "now - createdAt", "now - maxBlockTimestamp", "now - minBlockTimestamp", "max - min block ts (s), min / max"], &rows));
        // blobs
        let first = polls.first().map(|(_, e)| e).ok_or("no signed polls")?;
        let mut len_dist: BTreeMap<usize, usize> = BTreeMap::new();
        let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
        for e in first {
            if let Some(b) = e.blob.as_deref().and_then(unhex) {
                *len_dist.entry(b.len()).or_insert(0) += 1;
                blobs.push((e.sym.clone(), b));
            }
        }
        s.push_str(&format!("Blob length in bytes over the {} tokens of the first response: {:?}\n\n", blobs.len(), len_dist));
        let prefix = blobs.iter().skip(1).fold(blobs.first().map_or(0, |b| b.1.len()), |acc, (_, b)| {
            let a = &blobs[0].1;
            acc.min(a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count())
        });
        s.push_str(&format!("Longest byte prefix common to all those blobs: {prefix} bytes.\n\n"));
        // change rate of blobs per token over the run
        let mut distinct: HashMap<&str, HashSet<&str>> = HashMap::new();
        for (_, ents) in &polls {
            for e in ents {
                if let Some(b) = &e.blob {
                    distinct.entry(e.sym.as_str()).or_default().insert(b.as_str());
                }
            }
        }
        let mut dv: Vec<usize> = distinct.values().map(|x| x.len()).collect();
        dv.sort_unstable();
        s.push_str(&format!("Distinct blobs per token over {} responses: min {:?}, median {:?}, max {:?}.\n\n", polls.len(), dv.first(), dv.get(dv.len() / 2), dv.last()));
        // first 16 bytes + price search
        let mut rows = Vec::new();
        let mut hit_tokens = 0usize;
        let mut offsets_seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut all_hex = String::new();
        for e in first {
            let Some(b) = e.blob.as_deref().and_then(unhex) else { continue };
            let d = *dec.get(&e.sym).unwrap_or(&18);
            let usd = match (e.min_full.as_deref().and_then(|x| usd_price(x, d, None)), e.max_full.as_deref().and_then(|x| usd_price(x, d, None))) {
                (Some(a), Some(c)) => (a + c) / 2.0,
                _ => f64::NAN,
            };
            let raw_med = match (e.min_full.as_deref().and_then(|x| x.parse::<u128>().ok()), e.max_full.as_deref().and_then(|x| x.parse::<u128>().ok())) {
                (Some(a), Some(c)) => ((a / 2) + (c / 2)) as f64,
                _ => 0.0,
            };
            let targets = vec![("usd x1e18".to_string(), usd * 1e18), ("usd x1e8".to_string(), usd * 1e8), ("usd x1e12".to_string(), usd * 1e12), ("usd x1e6".to_string(), usd * 1e6), ("raw 30-dec".to_string(), raw_med)];
            let hits = if usd.is_finite() { find_be_values(&b, &targets, 0.005) } else { vec![] };
            all_hex.push_str(&format!("{} {} {}\n", e.sym, b.len(), hex(&b)));
            if !hits.is_empty() {
                hit_tokens += 1;
            }
            for h in &hits {
                *offsets_seen.entry(format!("{} width {} at byte {}", h.scale, h.width, h.offset)).or_insert(0) += 1;
            }
            if SYMS13.contains(&e.sym.as_str()) {
                let best = hits.iter().map(|h| format!("{} w{}@{} (err {}%)", h.scale, h.width, h.offset, f(h.rel_err * 100.0, 3))).take(4).collect::<Vec<_>>().join("; ");
                rows.push(vec![e.sym.clone(), b.len().to_string(), hex(&b[..16.min(b.len())]), f(usd, 4), if best.is_empty() { "none within 0.5%".into() } else { best }]);
            }
        }
        let _ = fs::write(dir().join("signed-blobs-first-poll.txt"), all_hex);
        s.push_str("First 16 bytes and a search for the price inside the blob (big-endian integers of width 8/16/24/32 at every byte offset, within 0.5% of the price in several scalings):\n\n");
        s.push_str(&table(&["symbol", "blob bytes", "first 16 bytes (hex)", "price USD from min/maxPriceFull", "price-like integers in the blob"], &rows));
        s.push_str(&format!("Tokens (of {}) with at least one price-like integer in the blob: {hit_tokens}. Most frequent hit positions: {:?}\n\n", first.len(), {
            let mut v: Vec<_> = offsets_seen.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            v.into_iter().take(6).map(|(k, n)| format!("{k} x{n}")).collect::<Vec<_>>()
        }));
        s.push_str("Observation only: nothing here verifies a signature or decodes the report; `signer`, `signature` and the block fields are null/absent as listed in section 2.\n\n");
        Ok(s)
    });

    // ---------------------------------------------------------------- 7. candles
    phase!("7 candles", {
        let mut rows = Vec::new();
        for sym in ["BTC", "ETH", "SOL", "GOLD", "SPY"] {
            let r = http.get(PRIMARY, &format!("/prices/candles?tokenSymbol={sym}&period=1m&limit=10000"))?;
            save(&format!("candles-{sym}-1m.json"), &r.body);
            let v = json(&r.body).ok_or("candles parse")?;
            let arr = v.get("candles").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let ts: Vec<i64> = arr.iter().filter_map(|c| c.as_array()?.first()?.as_i64()).collect();
            let descending = ts.windows(2).all(|w| w[0] > w[1]);
            let ascending = ts.windows(2).all(|w| w[0] < w[1]);
            let mut asc = ts.clone();
            asc.sort_unstable();
            let (missing, gaps) = minute_gaps(&asc);
            let span_h = (asc.last().copied().unwrap_or(0) - asc.first().copied().unwrap_or(0)) as f64 / 3600.0;
            let mut biggest = gaps.clone();
            biggest.sort_by(|a, b| b.2.cmp(&a.2));
            rows.push(vec![
                sym.to_string(),
                format!("HTTP {}", r.status),
                arr.len().to_string(),
                format!("{} .. {}", asc.first().map_or("-".into(), |t| utc_string(*t as u64 * 1000)[..16].to_string()), asc.last().map_or("-".into(), |t| utc_string(*t as u64 * 1000)[..16].to_string())),
                f(span_h, 1),
                if descending { "newest first".into() } else if ascending { "oldest first".into() } else { "mixed".into() },
                missing.to_string(),
                gaps.len().to_string(),
                biggest.iter().take(3).map(|(a, b, m)| format!("{}..{} ({m} missing)", &utc_string(*a as u64 * 1000)[5..16], &utc_string(*b as u64 * 1000)[5..16])).collect::<Vec<_>>().join("; "),
                format!("{} ms", r.latency_ms),
            ]);
        }
        let mut s = String::from("## 7. Candle backfill (1m, limit 10000)\n\n");
        s.push_str(&table(&["symbol", "status", "candles", "span (UTC)", "span hours", "ordering", "missing minutes", "gaps", "largest gaps", "latency"], &rows));
        Ok(s)
    });

    // ---------------------------------------------------------------- summary
    let statuses = http.status_counts.lock().map(|m| m.clone()).unwrap_or_default();
    let errs = http.errors.lock().map(|m| m.clone()).unwrap_or_default();
    let incidents = http.incidents.lock().map(|m| m.clone()).unwrap_or_default();
    let total = http.requests.lock().map(|n| *n).unwrap_or(0);
    let mut s = String::from("## Run summary\n\n");
    s.push_str(&format!(
        "Run ended {} after {} s; {total} requests in total ({} per second on average, hard cap 10/s by the global limiter).\nHTTP status counts: {:?}\n429/403 incidents: {:?}\nTransport errors/timeouts: {} {:?}\nAborted: {:?}\n",
        utc_string(now_ms()), t_run.elapsed().as_secs(), f(total as f64 / t_run.elapsed().as_secs_f64().max(1.0), 2), statuses, incidents, errs.len(), errs.iter().take(5).collect::<Vec<_>>(), abort
    ));
    report.push_str(&s);
    let _ = fs::write(dir().join("report-tables.md"), &report);
    println!("{report}");
    let _ = std::io::stdout().flush();
}
