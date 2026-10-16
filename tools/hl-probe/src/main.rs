//! Hyperliquid (HIP-3) probe (prompt P6). Read-only: POST /info and read-only WebSocket subscriptions, never /exchange.
//! `User-Agent: setl8-probe/0.1`; REST weight kept under 280 per minute (half the documented 1200 is 600; the prompt asks
//! for at most 300), at most 2 WebSocket connections. Any 429/403 or WebSocket error: stop, back off 60 s, report.
//! `PROBE_QUICK=1` shortens the run for a smoke test. Raw data goes to `data/` (git-ignored).
use hl_probe::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

mod oracle;

const INFO: &str = "https://api.hyperliquid.xyz/info";
const WS_URL: &str = "wss://api.hyperliquid.xyz/ws";
const UA: &str = "setl8-probe/0.1";
const WEIGHT_BUDGET: u32 = 280;

/// The coins we need (class label, base name). The prompt calls it "40 coins"; the list has 34 entries.
const WANT: [(&str, &str); 34] = [
    ("stock", "AAPL"), ("stock", "MSFT"), ("stock", "NVDA"), ("stock", "TSLA"), ("stock", "AMZN"), ("stock", "GOOGL"), ("stock", "META"), ("stock", "NFLX"),
    ("stock", "AMD"), ("stock", "COIN"), ("stock", "MSTR"), ("stock", "PLTR"),
    ("index", "SP500"), ("index", "XYZ100"), ("index", "JP225"), ("index", "KR200"), ("index", "NIFTY"), ("index", "IBOV"), ("index", "VIX"), ("index", "DXY"),
    ("FX", "EUR"), ("FX", "GBP"), ("FX", "JPY"), ("FX", "KRW"),
    ("metal", "GOLD"), ("metal", "SILVER"), ("metal", "COPPER"), ("metal", "PLATINUM"), ("metal", "PALLADIUM"), ("metal", "ALUMINIUM"),
    ("energy", "CL"), ("energy", "BRENTOIL"), ("energy", "NATGAS"), ("energy", "HO"),
];
/// The 20 coins of the depth test (13 xyz coins, 3 main-dex crypto coins, 4 more of my choice).
const L2_COINS: [&str; 20] = [
    "xyz:AAPL", "xyz:NVDA", "xyz:TSLA", "xyz:SP500", "xyz:XYZ100", "xyz:JP225", "xyz:EUR", "xyz:JPY", "xyz:GOLD", "xyz:SILVER", "xyz:COPPER", "xyz:CL",
    "xyz:BRENTOIL", "BTC", "ETH", "SOL", "xyz:MSFT", "xyz:GBP", "xyz:NATGAS", "xyz:PLATINUM",
];
const BBO_COINS: [&str; 6] = ["xyz:AAPL", "xyz:NVDA", "xyz:SP500", "xyz:EUR", "xyz:GOLD", "xyz:CL"];
const TRACKED: [&str; 6] = ["xyz:AAPL", "xyz:NVDA", "xyz:SP500", "xyz:EUR", "xyz:GOLD", "xyz:CL"];

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
    let wd = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"][days.rem_euclid(7) as usize];
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC ({wd})", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn quick() -> bool {
    std::env::var("PROBE_QUICK").is_ok()
}

fn dir() -> PathBuf {
    let p = PathBuf::from("data");
    let _ = fs::create_dir_all(p.join("raw"));
    p
}

fn save(name: &str, body: &str) {
    let _ = fs::write(dir().join("raw").join(name), body);
}

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

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

// ------------------------------------------------------------------ REST with a weight budget

struct Rest {
    agent: ureq::Agent,
    window: Mutex<VecDeque<(Instant, u32)>>,
    incidents: Mutex<Vec<String>>,
    statuses: Mutex<BTreeMap<u16, u64>>,
    errors: Mutex<Vec<String>>,
    weight_total: Mutex<u64>,
    requests: Mutex<u64>,
    max_window_weight: Mutex<u32>,
}

impl Rest {
    fn new() -> Self {
        Rest {
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).user_agent(UA).build(),
            window: Mutex::new(VecDeque::new()),
            incidents: Mutex::new(vec![]),
            statuses: Mutex::new(BTreeMap::new()),
            errors: Mutex::new(vec![]),
            weight_total: Mutex::new(0),
            requests: Mutex::new(0),
            max_window_weight: Mutex::new(0),
        }
    }

    /// Waits until `w` more weight fits in the trailing 60 s, then books it.
    fn reserve(&self, w: u32) {
        loop {
            let wait = {
                let mut win = match self.window.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                while win.front().map_or(false, |(t, _)| t.elapsed() >= Duration::from_secs(60)) {
                    win.pop_front();
                }
                let sum: u32 = win.iter().map(|(_, x)| *x).sum();
                if sum + w <= WEIGHT_BUDGET || win.is_empty() {
                    win.push_back((Instant::now(), w));
                    if let Ok(mut m) = self.max_window_weight.lock() {
                        *m = (*m).max(sum + w);
                    }
                    if let Ok(mut t) = self.weight_total.lock() {
                        *t += u64::from(w);
                    }
                    return;
                }
                win.front().map(|(t, _)| Duration::from_secs(60).saturating_sub(t.elapsed()) + Duration::from_millis(50))
            };
            thread::sleep(wait.unwrap_or(Duration::from_millis(100)));
        }
    }

    /// Books extra weight after the fact (candle responses cost 1 per 60 items returned).
    fn add_weight(&self, w: u32) {
        if let Ok(mut win) = self.window.lock() {
            win.push_back((Instant::now(), w));
        }
        if let Ok(mut t) = self.weight_total.lock() {
            *t += u64::from(w);
        }
    }

    /// POST /info. On 429/403: record, back off 60 s, retry once; a second hit aborts.
    fn info(&self, body: &Value, weight: u32) -> Result<(Value, u64), String> {
        for attempt in 0..2 {
            self.reserve(weight);
            if let Ok(mut n) = self.requests.lock() {
                *n += 1;
            }
            let t = Instant::now();
            let res = self.agent.post(INFO).set("Content-Type", "application/json").send_string(&body.to_string());
            let latency = t.elapsed().as_millis() as u64;
            let (status, text) = match res {
                Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
                Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap_or_default()),
                Err(e) => {
                    let m = format!("POST /info {body}: {e}");
                    if let Ok(mut v) = self.errors.lock() {
                        v.push(m.clone());
                    }
                    return Err(m);
                }
            };
            if let Ok(mut m) = self.statuses.lock() {
                *m.entry(status).or_insert(0) += 1;
            }
            if status == 429 || status == 403 {
                if let Ok(mut v) = self.incidents.lock() {
                    v.push(format!("{} HTTP {status} on /info {}", utc_string(now_ms()), body.to_string().chars().take(80).collect::<String>()));
                }
                eprintln!("!! HTTP {status}: backing off 60 s");
                thread::sleep(Duration::from_secs(60));
                if attempt == 1 {
                    return Err("aborted: second 429/403".into());
                }
                continue;
            }
            if status != 200 {
                return Err(format!("HTTP {status}: {}", text.chars().take(200).collect::<String>()));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("json: {e}"))?;
            return Ok((v, latency));
        }
        Err("unreachable".into())
    }
}

// ------------------------------------------------------------------ WebSocket threads

struct WsStats {
    connects: u32,
    reconnects: u32,
    incidents: Vec<String>,
    messages: u64,
    first_messages: Vec<String>,
    pongs: u64,
}

type Sock = WebSocket<MaybeTlsStream<TcpStream>>;

fn set_timeout(sock: &mut Sock, d: Duration) {
    match sock.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(d));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.get_mut().set_read_timeout(Some(d));
        }
        _ => {}
    }
}

fn ws_loop(subs: Vec<Value>, duration: Duration, mut on_text: impl FnMut(&str, u64)) -> WsStats {
    let mut st = WsStats { connects: 0, reconnects: 0, incidents: vec![], messages: 0, first_messages: vec![], pongs: 0 };
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let req = match WS_URL.into_client_request() {
            Ok(mut r) => {
                r.headers_mut().insert("User-Agent", UA.parse().unwrap_or_else(|_| "setl8-probe".parse().unwrap()));
                r
            }
            Err(e) => {
                st.incidents.push(format!("request: {e}"));
                break;
            }
        };
        let mut sock = match tungstenite::connect(req) {
            Ok((s, _)) => s,
            Err(e) => {
                st.incidents.push(format!("{} connect error: {e}", utc_string(now_ms())));
                if st.incidents.len() >= 3 {
                    break;
                }
                thread::sleep(Duration::from_secs(60));
                st.reconnects += 1;
                continue;
            }
        };
        st.connects += 1;
        set_timeout(&mut sock, Duration::from_secs(5));
        for s in &subs {
            let _ = sock.send(Message::Text(json!({"method":"subscribe","subscription":s}).to_string()));
        }
        let mut last_ping = Instant::now();
        let mut failed = false;
        while Instant::now() < deadline {
            if last_ping.elapsed() >= Duration::from_secs(30) {
                let _ = sock.send(Message::Text(r#"{"method":"ping"}"#.to_string()));
                last_ping = Instant::now();
            }
            match sock.read() {
                Ok(Message::Text(t)) => {
                    st.messages += 1;
                    if st.first_messages.len() < 6 {
                        st.first_messages.push(t.chars().take(600).collect());
                    }
                    if t.contains("\"pong\"") {
                        st.pongs += 1;
                    }
                    on_text(&t, now_ms());
                }
                Ok(Message::Close(c)) => {
                    st.incidents.push(format!("{} server close: {c:?}", utc_string(now_ms())));
                    failed = true;
                    break;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    st.incidents.push(format!("{} read error: {e}", utc_string(now_ms())));
                    failed = true;
                    break;
                }
            }
        }
        let _ = sock.close(None);
        if !failed {
            break;
        }
        if st.incidents.len() >= 3 {
            break;
        }
        eprintln!("!! websocket problem, backing off 60 s");
        thread::sleep(Duration::from_secs(60));
        st.reconnects += 1;
    }
    st
}

// ------------------------------------------------------------------ data model

#[derive(Clone, Debug)]
struct Coin {
    name: String,
    dex: String,
    class: Class,
    max_lev: u64,
    sz_dec: u64,
    delisted: bool,
    flags: String,
}

struct Ctx {
    mark: Option<f64>,
    mid: Option<f64>,
    oracle: Option<f64>,
    prev_day: Option<f64>,
    day_vol: Option<f64>,
    oi: Option<f64>,
    impact: String,
}

struct Level {
    px: f64,
    sz: f64,
}

fn levels(v: &Value) -> Option<(Vec<Level>, Vec<Level>)> {
    let l = v.get("levels")?.as_array()?;
    let side = |x: &Value| -> Vec<Level> {
        x.as_array().map(|a| a.iter().filter_map(|e| Some(Level { px: num(e.get("px")?)?, sz: num(e.get("sz")?)? })).collect()).unwrap_or_default()
    };
    Some((side(l.first()?), side(l.get(1)?)))
}

struct AllMids {
    recv_ms: u64,
    mids: Vec<(String, u128)>,
}

struct Bbo {
    recv_ms: u64,
    coin: String,
    time: u64,
    bid: Option<(u128, f64)>,
    ask: Option<(u128, f64)>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--oracle-analyze") {
        oracle::analyze(args.get(i + 1).map_or("data", |x| x.as_str()));
        return;
    }
    if args.iter().any(|a| a == "--oracle") {
        oracle::run();
        return;
    }
    let t_run = Instant::now();
    let started_ms = now_ms();
    let rest = Arc::new(Rest::new());
    let mut report = format!("# Hyperliquid probe raw tables\n\nRun started: {}\nMode: {}\n\n", utc_string(started_ms), if quick() { "QUICK (smoke test)" } else { "full" });
    let flush = |r: &str| {
        let _ = fs::write(dir().join("report-tables.md"), r);
    };

    // ================================================================ 1. discovery
    eprintln!("== 1 discovery");
    let (dexs, _) = rest.info(&json!({"type":"perpDexs"}), 20).expect("perpDexs");
    let (metas, _) = rest.info(&json!({"type":"allPerpMetas"}), 20).expect("allPerpMetas");
    save("perpDexs.json", &dexs.to_string());
    save("allPerpMetas.json", &metas.to_string());
    let dex_list = dexs.as_array().cloned().unwrap_or_default();
    let meta_list = metas.as_array().cloned().unwrap_or_default();
    let mut coins: Vec<Coin> = Vec::new();
    let mut dex_rows = Vec::new();
    for (i, m) in meta_list.iter().enumerate() {
        let uni = m.get("universe").and_then(|u| u.as_array()).cloned().unwrap_or_default();
        let d = dex_list.get(i).cloned().unwrap_or(Value::Null);
        let dname = d.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let mut delisted = 0;
        for c in &uni {
            let name = c.get("name").and_then(|x| x.as_str()).unwrap_or("?").to_string();
            let isd = c.get("isDelisted").and_then(|x| x.as_bool()).unwrap_or(false);
            if isd {
                delisted += 1;
            }
            let mut flags = Vec::new();
            for k in ["onlyIsolated", "marginMode", "growthMode"] {
                if let Some(v) = c.get(k) {
                    flags.push(format!("{k}={}", v.to_string().trim_matches('"')));
                }
            }
            coins.push(Coin {
                class: classify(&name),
                dex: split_coin(&name).0.to_string(),
                name,
                max_lev: c.get("maxLeverage").and_then(|x| x.as_u64()).unwrap_or(0),
                sz_dec: c.get("szDecimals").and_then(|x| x.as_u64()).unwrap_or(0),
                delisted: isd,
                flags: flags.join(","),
            });
        }
        dex_rows.push(vec![
            if dname.is_empty() { "(main)".to_string() } else { dname.clone() },
            d.get("fullName").and_then(|x| x.as_str()).unwrap_or("Hyperliquid main perps").to_string(),
            d.get("deployer").and_then(|x| x.as_str()).unwrap_or("-").to_string(),
            d.get("oracleUpdater").and_then(|x| x.as_str()).unwrap_or("-").to_string(),
            uni.len().to_string(),
            delisted.to_string(),
            (uni.len() - delisted).to_string(),
            m.get("collateralToken").map_or("-".into(), |x| x.to_string()),
            uni.first().and_then(|c| c.get("name")).and_then(|x| x.as_str()).unwrap_or("-").to_string(),
        ]);
    }
    let mut s = String::from("## 1. Discovery (perpDexs + allPerpMetas)\n\n");
    s.push_str(&table(&["dex", "full name", "deployer", "oracleUpdater", "markets", "delisted", "active", "collateralToken", "first coin"], &dex_rows));
    let by_dex: BTreeMap<String, Vec<&Coin>> = coins.iter().fold(BTreeMap::new(), |mut m, c| {
        m.entry(c.dex.clone()).or_default().push(c);
        m
    });
    s.push_str("Coin-name format: `<dex>:<TICKER>` for builder dexes (for example `xyz:AAPL`, `xyz:XYZ100`), a bare ticker on the main dex (`BTC`, `kPEPE`). A bare `AAPL` returns `null` from l2Book.\n\n");
    let mut rows = Vec::new();
    for (d, cs) in &by_dex {
        let mut row = vec![if d.is_empty() { "(main)".into() } else { d.clone() }];
        for cl in ALL_CLASSES {
            let n = cs.iter().filter(|c| !c.delisted && c.class == cl).count();
            row.push(n.to_string());
        }
        row.push(cs.iter().filter(|c| !c.delisted).count().to_string());
        rows.push(row);
    }
    s.push_str("Active (not delisted) coins per class (class = a guess from the ticker, see decisions):\n\n");
    s.push_str(&table(&["dex", "stock", "ETF", "index", "FX", "commodity", "crypto", "other", "total"], &rows));
    s.push_str("### Every coin on `xyz` (and flags)\n\n");
    let mut rows = Vec::new();
    for c in by_dex.get("xyz").cloned().unwrap_or_default() {
        rows.push(vec![c.name.clone(), c.class.name().into(), c.max_lev.to_string(), c.sz_dec.to_string(), if c.delisted { "DELISTED".into() } else { "active".into() }, c.flags.clone()]);
    }
    s.push_str(&table(&["coin", "class (guess)", "max leverage", "szDecimals", "isDelisted", "other flags"], &rows));
    report.push_str(&s);
    flush(&report);

    // ================================================================ 3. snapshots (metaAndAssetCtxs per dex with 10+ markets)
    eprintln!("== 3 snapshots");
    let big_dexs: Vec<String> = by_dex.iter().filter(|(_, v)| v.len() >= 10).map(|(k, _)| k.clone()).collect();
    let mut ctxs: HashMap<String, Ctx> = HashMap::new();
    let mut csv = String::from("dex,coin,class,delisted,markPx,midPx,oraclePx,prevDayPx,dayNtlVlm,openInterest,impactPxs\n");
    for d in &big_dexs {
        let body = if d.is_empty() { json!({"type":"metaAndAssetCtxs"}) } else { json!({"type":"metaAndAssetCtxs","dex":d}) };
        match rest.info(&body, 20) {
            Ok((v, _)) => {
                save(&format!("metaAndAssetCtxs-{}.json", if d.is_empty() { "main" } else { d }), &v.to_string());
                let uni = v.get(0).and_then(|m| m.get("universe")).and_then(|u| u.as_array()).cloned().unwrap_or_default();
                let cx = v.get(1).and_then(|c| c.as_array()).cloned().unwrap_or_default();
                for (u, c) in uni.iter().zip(cx.iter()) {
                    let name = u.get("name").and_then(|x| x.as_str()).unwrap_or("?").to_string();
                    let ctx = Ctx {
                        mark: c.get("markPx").and_then(num),
                        mid: c.get("midPx").and_then(num),
                        oracle: c.get("oraclePx").and_then(num),
                        prev_day: c.get("prevDayPx").and_then(num),
                        day_vol: c.get("dayNtlVlm").and_then(num),
                        oi: c.get("openInterest").and_then(num),
                        impact: c.get("impactPxs").map_or("".into(), |x| x.to_string().replace(',', ";")),
                    };
                    let cl = classify(&name);
                    let isd = u.get("isDelisted").and_then(|x| x.as_bool()).unwrap_or(false);
                    csv.push_str(&format!(
                        "{},{},{},{},{},{},{},{},{},{},{}\n",
                        d, name, cl.name(), isd,
                        c.get("markPx").map_or("".into(), |x| x.to_string().trim_matches('"').to_string()),
                        c.get("midPx").map_or("".into(), |x| x.to_string().trim_matches('"').to_string()),
                        c.get("oraclePx").map_or("".into(), |x| x.to_string().trim_matches('"').to_string()),
                        c.get("prevDayPx").map_or("".into(), |x| x.to_string().trim_matches('"').to_string()),
                        c.get("dayNtlVlm").map_or("".into(), |x| x.to_string().trim_matches('"').to_string()),
                        c.get("openInterest").map_or("".into(), |x| x.to_string().trim_matches('"').to_string()),
                        ctx.impact
                    ));
                    ctxs.insert(name, ctx);
                }
            }
            Err(e) => report.push_str(&format!("metaAndAssetCtxs for `{d}` failed: {e}\n\n")),
        }
    }
    let _ = fs::write(dir().join("snapshot-all.csv"), &csv);

    // ================================================================ 2. coverage + 4. depth
    eprintln!("== 2/4 coverage and depth");
    let mut l2_results: HashMap<String, (Vec<Level>, Vec<Level>, u64)> = HashMap::new(); // (bids, asks, receive lag ms)
    let mut l2_raw = String::new();
    let mut want_rows = Vec::new();
    let mut chosen: Vec<(String, String, String)> = Vec::new(); // (label, base, coin string)
    for (label, base) in WANT {
        let cands: Vec<&Coin> = coins.iter().filter(|c| split_coin(&c.name).1 == base).collect();
        let pick = cands.iter().find(|c| c.dex == "xyz").or_else(|| cands.first()).map(|c| c.name.clone());
        chosen.push((label.to_string(), base.to_string(), pick.unwrap_or_default()));
        let _ = cands;
    }
    let mut l2_todo: Vec<String> = chosen.iter().filter(|c| !c.2.is_empty()).map(|c| c.2.clone()).collect();
    for c in L2_COINS {
        if !l2_todo.contains(&c.to_string()) {
            l2_todo.push(c.to_string());
        }
    }
    for coin in &l2_todo {
        if let Ok((v, _)) = rest.info(&json!({"type":"l2Book","coin":coin}), 2) {
            let recv = now_ms();
            l2_raw.push_str(&format!("{}\n", v));
            if let Some((b, a)) = levels(&v) {
                let t = v.get("time").and_then(|x| x.as_u64()).unwrap_or(0);
                l2_results.insert(coin.clone(), (b, a, if t > 0 { (recv as i64 - t as i64).max(i64::MIN) as u64 } else { 0 }));
            }
        }
    }
    let _ = fs::write(dir().join("raw").join("l2books.jsonl"), l2_raw);
    for (label, base, pick) in &chosen {
        let cands: Vec<String> = coins.iter().filter(|c| split_coin(&c.name).1 == *base).map(|c| format!("{}{}", c.name, if c.delisted { " (delisted)" } else { "" })).collect();
        let (works, note) = match l2_results.get(pick) {
            Some((b, a, _)) if !b.is_empty() && !a.is_empty() => ("yes".to_string(), format!("{} bid / {} ask levels", b.len(), a.len())),
            Some(_) => ("NO (empty book)".to_string(), "levels empty".to_string()),
            None => ("NO".to_string(), if pick.is_empty() { "coin not in any meta".into() } else { "no response".into() }),
        };
        let delisted = coins.iter().find(|c| &c.name == pick).map_or(false, |c| c.delisted);
        want_rows.push(vec![label.clone(), base.clone(), if pick.is_empty() { "-".into() } else { format!("`{pick}`") }, if delisted { "DELISTED".into() } else { "active".into() }, works, note, cands.join(", ")]);
    }
    let mut s = String::from("\n## 2. Coverage of the coins we need\n\n(The prompt says 40 coins; its list has 34 entries.) `works` = l2Book with that exact string returned non-empty bid and ask sides.\n\n");
    s.push_str(&table(&["class", "base", "coin string used", "status", "l2Book works", "book", "all coins with this ticker (any dex)"], &want_rows));
    // snapshot of the wanted coins
    let mut rows = Vec::new();
    for (_, base, pick) in &chosen {
        if let Some(c) = ctxs.get(pick) {
            let (mo, io) = (c.mark.zip(c.oracle).map(|(m, o)| gap_bps(m, o)), c.mid.zip(c.oracle).map(|(m, o)| gap_bps(m, o)));
            rows.push(vec![
                pick.clone(), base.clone(),
                c.mark.map_or("-".into(), |x| f(x, 4)), c.mid.map_or("-".into(), |x| f(x, 4)), c.oracle.map_or("-".into(), |x| f(x, 4)), c.prev_day.map_or("-".into(), |x| f(x, 4)),
                c.day_vol.map_or("-".into(), |x| f(x, 0)), c.oi.zip(c.mark).map_or("-".into(), |(o, m)| f(o * m, 0)),
                mo.map_or("-".into(), |x| f(x, 2)), io.map_or("-".into(), |x| f(x, 2)), c.impact.clone(),
            ]);
        }
    }
    s.push_str("### 3a. Snapshot of the wanted coins (metaAndAssetCtxs; OI in USD = openInterest x markPx; gaps in bp, signed)\n\n");
    s.push_str(&table(&["coin", "base", "markPx", "midPx", "oraclePx", "prevDayPx", "dayNtlVlm USD", "OI USD", "mark vs oracle bp", "mid vs oracle bp", "impactPxs"], &rows));
    // per class on xyz and per dex
    let class_stats = |dex: &str| -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for cl in ALL_CLASSES {
            let items: Vec<(&Coin, &Ctx)> = coins.iter().filter(|c| c.dex == dex && !c.delisted && c.class == cl).filter_map(|c| ctxs.get(&c.name).map(|x| (c, x))).collect();
            if items.is_empty() {
                continue;
            }
            let mo: Vec<f64> = items.iter().filter_map(|(_, c)| c.mark.zip(c.oracle).map(|(m, o)| gap_bps(m, o).abs())).collect();
            let io: Vec<f64> = items.iter().filter_map(|(_, c)| c.mid.zip(c.oracle).map(|(m, o)| gap_bps(m, o).abs())).collect();
            let vol: f64 = items.iter().filter_map(|(_, c)| c.day_vol).sum();
            let oi: f64 = items.iter().filter_map(|(_, c)| c.oi.zip(c.mark).map(|(o, m)| o * m)).sum();
            let thin = items.iter().filter(|(_, c)| c.day_vol.unwrap_or(0.0) < 100_000.0).count();
            let med_vol = median(&items.iter().filter_map(|(_, c)| c.day_vol).collect::<Vec<_>>());
            rows.push(vec![
                cl.name().into(), items.len().to_string(),
                median(&mo).map_or("-".into(), |x| f(x, 2)), f(mo.iter().cloned().fold(f64::NAN, f64::max), 2),
                median(&io).map_or("-".into(), |x| f(x, 2)), f(io.iter().cloned().fold(f64::NAN, f64::max), 2),
                f(vol, 0), med_vol.map_or("-".into(), |x| f(x, 0)), f(oi, 0), format!("{thin}"),
            ]);
        }
        rows
    };
    for d in &big_dexs {
        let rows = class_stats(d);
        if rows.is_empty() {
            continue;
        }
        s.push_str(&format!("### 3b. Gaps, volume and open interest per class: dex `{}` (|gap| in bp: median / max; USD)\n\n", if d.is_empty() { "(main)" } else { d }));
        s.push_str(&table(&["class", "coins", "|mark-oracle| median", "max", "|mid-oracle| median", "max", "24h notional sum", "24h notional median per coin", "open interest sum (USD)", "coins under $100k/24h"], &rows));
    }
    // depth
    let mut rows = Vec::new();
    let mut proofs_ok = 0;
    for coin in L2_COINS {
        if let Some((bids, asks, t)) = l2_results.get(coin) {
            let (bb, ba) = (bids.first(), asks.first());
            let sorted_b = bids.windows(2).all(|w| w[0].px > w[1].px);
            let sorted_a = asks.windows(2).all(|w| w[0].px < w[1].px);
            let cross_ok = matches!((bb, ba), (Some(b), Some(a)) if b.px < a.px);
            if cross_ok && sorted_a && sorted_b {
                proofs_ok += 1;
            }
            rows.push(vec![
                coin.to_string(),
                bb.map_or("-".into(), |b| f(b.px, 4)), ba.map_or("-".into(), |a| f(a.px, 4)),
                match (bb, ba) { (Some(b), Some(a)) => f(spread_bps(b.px, a.px), 2), _ => "-".into() },
                bb.map_or("-".into(), |b| f(b.px * b.sz, 0)), ba.map_or("-".into(), |a| f(a.px * a.sz, 0)),
                format!("{}/{}", bids.len(), asks.len()),
                format!("{cross_ok} / {sorted_b} / {sorted_a}"),
                if *t > 0 { format!("{} ms", *t as i64) } else { "-".into() },
            ]);
        }
    }
    s.push_str("## 4. Spread and depth (l2Book; `levels[0]` vs `levels[1]`)\n\n");
    s.push_str("PROOF of orientation: `levels[0]` is the BIDS and `levels[1]` the ASKS when, for every book, best level[0] price < best level[1] price, level[0] prices strictly descend and level[1] prices strictly ascend (columns: best bid < best ask / bids descending / asks ascending).\n\n");
    s.push_str(&table(&["coin", "best bid (levels[0][0])", "best ask (levels[1][0])", "spread bp", "size at best bid USD", "size at best ask USD", "levels bid/ask", "proof: bid<ask / bids desc / asks asc", "book `time` vs receive time (ms)"], &rows));
    s.push_str(&format!("Books that pass the whole proof: {proofs_ok} of {}.\n\n", rows.len()));
    report.push_str(&s);
    flush(&report);

    // ================================================================ 5. WebSocket (15 minutes) + REST work meanwhile
    eprintln!("== 5 websocket");
    let ws_secs: u64 = if quick() { 25 } else { 900 };
    let all_mids: Arc<Mutex<Vec<AllMids>>> = Arc::new(Mutex::new(Vec::new()));
    let bbos: Arc<Mutex<Vec<Bbo>>> = Arc::new(Mutex::new(Vec::new()));
    let am = all_mids.clone();
    let h1 = thread::spawn(move || {
        ws_loop(vec![json!({"type":"allMids","dex":"xyz"})], Duration::from_secs(ws_secs), move |text, recv| {
            let Ok(v) = serde_json::from_str::<Value>(text) else { return };
            if v.get("channel").and_then(|c| c.as_str()) != Some("allMids") {
                return;
            }
            let mids = v.get("data").and_then(|d| d.get("mids").or(Some(d))).and_then(|m| m.as_object());
            if let Some(m) = mids {
                let list: Vec<(String, u128)> = m.iter().filter_map(|(k, x)| Some((k.clone(), parse_scaled(x.as_str()?, PRICE_SCALE)?))).collect();
                if let Ok(mut g) = am.lock() {
                    g.push(AllMids { recv_ms: recv, mids: list });
                }
            }
        })
    });
    let bb = bbos.clone();
    let h2 = thread::spawn(move || {
        let subs: Vec<Value> = BBO_COINS.iter().map(|c| json!({"type":"bbo","coin":c})).collect();
        ws_loop(subs, Duration::from_secs(ws_secs), move |text, recv| {
            let Ok(v) = serde_json::from_str::<Value>(text) else { return };
            if v.get("channel").and_then(|c| c.as_str()) != Some("bbo") {
                return;
            }
            let Some(d) = v.get("data") else { return };
            let side = |x: Option<&Value>| -> Option<(u128, f64)> {
                let x = x?;
                Some((parse_scaled(x.get("px")?.as_str()?, PRICE_SCALE)?, num(x.get("sz")?)?))
            };
            let arr = d.get("bbo").and_then(|b| b.as_array());
            if let (Some(coin), Some(time)) = (d.get("coin").and_then(|c| c.as_str()), d.get("time").and_then(|t| t.as_u64())) {
                if let Ok(mut g) = bb.lock() {
                    g.push(Bbo { recv_ms: recv, coin: coin.to_string(), time, bid: side(arr.and_then(|a| a.first())), ask: side(arr.and_then(|a| a.get(1))) });
                }
            }
        })
    });

    // ---- 7. candles (REST, meanwhile)
    eprintln!("== 7 candles");
    let mut rows = Vec::new();
    let candle_coins: Vec<&str> = if quick() { vec!["xyz:AAPL", "BTC"] } else { vec!["xyz:AAPL", "xyz:SP500", "xyz:EUR", "xyz:GOLD", "BTC"] };
    for coin in &candle_coins {
        for (label, span_ms) in [("last 24 h", 86_400_000u64), ("max window (asked 30 days)", 30 * 86_400_000u64)] {
            let end = now_ms();
            let start = end - span_ms;
            match rest.info(&json!({"type":"candleSnapshot","req":{"coin":coin,"interval":"1m","startTime":start,"endTime":end}}), 20) {
                Ok((v, lat)) => {
                    let arr = v.as_array().cloned().unwrap_or_default();
                    rest.add_weight(u32::try_from(arr.len() / 60).unwrap_or(0));
                    save(&format!("candles-{}-{}.json", coin.replace(':', "_"), if span_ms == 86_400_000 { "24h" } else { "max" }), &v.to_string());
                    let ts: Vec<u64> = arr.iter().filter_map(|c| c.get("t").and_then(|x| x.as_u64())).collect();
                    let asc = ts.windows(2).all(|w| w[0] < w[1]);
                    let desc = ts.windows(2).all(|w| w[0] > w[1]);
                    let mut sorted = ts.clone();
                    sorted.sort_unstable();
                    let (missing, gaps) = minute_gaps_ms(&sorted);
                    let largest = { let mut g = gaps.clone(); g.sort_by(|a, b| b.2.cmp(&a.2)); g.into_iter().take(2).map(|(a, b, m)| format!("{}..{} ({m})", &utc_string(a)[5..16], &utc_string(b)[5..16])).collect::<Vec<_>>().join("; ") };
                    rows.push(vec![
                        coin.to_string(), label.to_string(), arr.len().to_string(),
                        sorted.first().map_or("-".into(), |t| utc_string(*t)[..16].to_string()), sorted.last().map_or("-".into(), |t| utc_string(*t)[..16].to_string()),
                        if asc { "oldest first".into() } else if desc { "newest first".into() } else { "mixed".into() },
                        missing.to_string(), gaps.len().to_string(), largest, format!("{lat} ms"),
                    ]);
                }
                Err(e) => rows.push(vec![coin.to_string(), label.to_string(), format!("ERROR {e}"), "".into(), "".into(), "".into(), "".into(), "".into(), "".into(), "".into()]),
            }
        }
    }
    let mut candle_s = String::from("## 7. Candles (candleSnapshot, 1m)\n\n");
    candle_s.push_str(&table(&["coin", "request", "candles returned", "earliest (UTC)", "latest (UTC)", "ordering", "missing minutes", "gaps", "largest gaps (missing)", "latency"], &rows));

    // ---- GMX one fetch (for the closed-market comparison)
    eprintln!("== 6 gmx comparison fetch");
    let gmx_fetch_ms;
    let mut gmx: HashMap<String, f64> = HashMap::new();
    {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).user_agent(UA).build();
        gmx_fetch_ms = now_ms();
        if let Ok(r) = agent.get("https://arbitrum-api.gmxinfra.io/prices/tickers").call() {
            if let Ok(body) = r.into_string() {
                if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&body) {
                    for e in a {
                        if let (Some(sym), Some(lo), Some(hi)) = (e.get("tokenSymbol").and_then(|x| x.as_str()), e.get("minPrice").and_then(|x| x.as_str()), e.get("maxPrice").and_then(|x| x.as_str())) {
                            if let (Ok(lo), Ok(hi)) = (lo.parse::<u128>(), hi.parse::<u128>()) {
                                // tokenDecimals 18 for the tradfi/commodity tokens (handover/gmx-probe-report.md); other tokens are not compared
                                gmx.insert(sym.to_string(), ((lo as f64) + (hi as f64)) / 2.0 / 1e12);
                            }
                        }
                    }
                }
            }
        }
    }

    let ws1 = h1.join().unwrap_or(WsStats { connects: 0, reconnects: 0, incidents: vec!["thread panicked".into()], messages: 0, first_messages: vec![], pongs: 0 });
    let ws2 = h2.join().unwrap_or(WsStats { connects: 0, reconnects: 0, incidents: vec!["thread panicked".into()], messages: 0, first_messages: vec![], pongs: 0 });
    let am = all_mids.lock().map(|g| g.iter().map(|m| AllMids { recv_ms: m.recv_ms, mids: m.mids.clone() }).collect::<Vec<_>>()).unwrap_or_default();
    let bm = bbos.lock().map(|g| g.iter().map(|b| Bbo { recv_ms: b.recv_ms, coin: b.coin.clone(), time: b.time, bid: b.bid, ask: b.ask }).collect::<Vec<_>>()).unwrap_or_default();
    let _ = fs::write(dir().join("raw").join("ws-first-messages.txt"), format!("allMids:\n{}\n\nbbo:\n{}\n", ws1.first_messages.join("\n"), ws2.first_messages.join("\n")));

    // ---- analysis of the WebSocket data
    let window_min = match (am.first(), am.last()) {
        (Some(a), Some(b)) => (b.recv_ms - a.recv_ms) as f64 / 60_000.0,
        _ => 0.0,
    };
    let mut s = String::from("## 5. WebSocket cadence\n\n");
    s.push_str(&format!(
        "Connection 1 `allMids` (dex xyz): {} messages in {} min ({} per minute), connects {}, reconnects {}, pongs {}. Connection 2 `bbo` x6: {} messages, connects {}, reconnects {}, pongs {}.\nIncidents (errors/closes): conn 1 {:?}, conn 2 {:?}\n\n",
        am.len(), f(window_min, 2), f(am.len() as f64 / window_min.max(0.001), 1), ws1.connects, ws1.reconnects, ws1.pongs, bm.len(), ws2.connects, ws2.reconnects, ws2.pongs, ws1.incidents, ws2.incidents
    ));
    let sizes: Vec<u64> = am.iter().map(|m| m.mids.len() as u64).collect();
    let mut sizes_sorted = sizes.clone();
    sizes_sorted.sort_unstable();
    s.push_str(&format!("Coins per allMids message: min {:?}, median {:?}, max {:?}.\n\n", sizes_sorted.first(), percentile(&sizes_sorted, 50.0), sizes_sorted.last()));
    // per coin series from allMids
    let idx_xyz: Vec<&Coin> = coins.iter().filter(|c| c.dex == "xyz").collect();
    let sym_idx: HashMap<&str, u16> = idx_xyz.iter().enumerate().map(|(i, c)| (c.name.as_str(), i as u16)).collect();
    struct CoinSeries {
        changes: Vec<u64>,
        first: u128,
        last: u128,
        min: u128,
        max: u128,
        abs_move_bps: f64,
        msgs: usize,
    }
    let mut series: HashMap<String, CoinSeries> = HashMap::new();
    let mut bin_mid: Vec<u8> = Vec::new();
    let mut stored_coins: HashSet<String> = HashSet::new();
    for m in &am {
        for (coin, mid) in &m.mids {
            let e = series.entry(coin.clone()).or_insert(CoinSeries { changes: vec![], first: *mid, last: 0, min: *mid, max: *mid, abs_move_bps: 0.0, msgs: 0 });
            e.msgs += 1;
            if e.last != *mid {
                if e.last != 0 {
                    e.abs_move_bps += ((*mid as f64) - (e.last as f64)).abs() / (e.last as f64) * 10_000.0;
                }
                e.changes.push(m.recv_ms);
                e.last = *mid;
                e.min = e.min.min(*mid);
                e.max = e.max.max(*mid);
                if let Some(i) = sym_idx.get(coin.as_str()) {
                    bin_mid.extend_from_slice(&Record::mid_only(*i, m.recv_ms, *mid).to_bytes());
                    stored_coins.insert(coin.clone());
                }
            }
        }
    }
    let mut rows = Vec::new();
    for coin in TRACKED {
        let bbo_c: Vec<&Bbo> = bm.iter().filter(|b| b.coin == coin).collect();
        let mut bt: Vec<u64> = Vec::new();
        let (mut last_px, mut ooo, mut max_time) = (None::<(u128, u128)>, 0usize, 0u64);
        let mut lags: Vec<u64> = Vec::new();
        for b in &bbo_c {
            if b.time < max_time {
                ooo += 1;
            }
            max_time = max_time.max(b.time);
            lags.push(b.recv_ms.saturating_sub(b.time));
            let px = (b.bid.map_or(0, |x| x.0), b.ask.map_or(0, |x| x.0));
            if last_px != Some(px) {
                bt.push(b.time);
                last_px = Some(px);
            }
        }
        lags.sort_unstable();
        let gs = series.get(coin).and_then(|c| gap_stats(&c.changes));
        let bg = gap_stats(&bt);
        let per_min = |n: usize| n as f64 / window_min.max(0.001);
        rows.push(vec![
            coin.to_string(),
            series.get(coin).map_or("-".into(), |c| f(per_min(c.changes.len().saturating_sub(1)), 2)),
            gs.map_or("n/a".into(), |g| format!("{} / {} / {}", g.median, g.p95, g.max)),
            f(per_min(bbo_c.len()), 1),
            f(per_min(bt.len().saturating_sub(1)), 2),
            bg.map_or("n/a".into(), |g| format!("{} / {} / {}", g.median, g.p95, g.max)),
            ooo.to_string(),
            format!("{:?} / {:?} / {:?}", percentile(&lags, 50.0), percentile(&lags, 95.0), lags.last()),
        ]);
    }
    s.push_str("Per tracked coin (gaps in ms between price CHANGES: median / p95 / max; allMids gaps use client receive time because allMids carries no server time; bbo gaps use the message `time`):\n\n");
    s.push_str(&table(&["coin", "allMids mid changes/min", "allMids gap ms", "bbo msgs/min", "bbo price changes/min", "bbo gap ms", "bbo out-of-order `time`", "bbo recv - time ms (p50 / p95 / max)"], &rows));
    s.push_str("The wall-clock comparison exists only for bbo (`time` is a server millisecond timestamp); a negative or large lag would show a skewed client clock. allMids messages have no time field.\n\n");
    // staleness: any coin where bbo lag > 5 s
    let stale = bm.iter().filter(|b| b.recv_ms.saturating_sub(b.time) > 5_000).count();
    s.push_str(&format!("bbo messages older than 5 s on arrival: {stale} of {}.\n\n", bm.len()));
    // bbo orientation proof
    let bbo_ok = bm.iter().filter(|b| matches!((b.bid, b.ask), (Some(x), Some(y)) if x.0 < y.0)).count();
    let bbo_missing = bm.iter().filter(|b| b.bid.is_none() || b.ask.is_none()).count();
    s.push_str(&format!("bbo orientation: `bbo[0]` price below `bbo[1]` price in {bbo_ok} of {} messages ({bbo_missing} had a missing side) -> bbo[0] is the bid.\n\n", bm.len()));

    // ---- 6. closed-market behaviour per class (xyz)
    let mut rows = Vec::new();
    for cl in ALL_CLASSES {
        let cs: Vec<(&&Coin, &CoinSeries)> = idx_xyz.iter().filter(|c| !c.delisted && c.class == cl).filter_map(|c| series.get(&c.name).map(|s| (c, s))).collect();
        if cs.is_empty() {
            continue;
        }
        let per_hour = |n: usize| n.saturating_sub(1) as f64 / window_min.max(0.001) * 60.0;
        let changes: Vec<f64> = cs.iter().map(|(_, s)| per_hour(s.changes.len())).collect();
        let range: Vec<f64> = cs.iter().map(|(_, s)| gap_bps(s.max as f64, s.min as f64).abs()).collect();
        let abs_move: Vec<f64> = cs.iter().map(|(_, s)| s.abs_move_bps * 60.0 / window_min.max(0.001)).collect();
        let net: Vec<f64> = cs.iter().map(|(_, s)| gap_bps(s.last as f64, s.first as f64).abs()).collect();
        let frozen = cs.iter().filter(|(_, s)| s.changes.len() <= 1).count();
        rows.push(vec![
            cl.name().into(), cs.len().to_string(), frozen.to_string(),
            f(median(&changes).unwrap_or(f64::NAN), 1), f(changes.iter().cloned().fold(f64::NAN, f64::max), 1),
            f(median(&net).unwrap_or(f64::NAN), 2), f(net.iter().cloned().fold(f64::NAN, f64::max), 2),
            f(median(&range).unwrap_or(f64::NAN), 2), f(median(&abs_move).unwrap_or(f64::NAN), 1),
        ]);
    }
    s.push_str(&format!("## 6. Closed-market behaviour\n\nRun wall-clock: {} to {}. US stocks, indices, FX, gold and oil are closed on their traditional venues at this time (weekend); the prices below are the HIP-3 venue's OWN mids, not a tradfi quote.\n\nPer class on `xyz` over the {} min allMids window (movement per hour = observed activity x 60 / window, linear extrapolation):\n\n", utc_string(started_ms), utc_string(now_ms()), f(window_min, 1)));
    s.push_str(&table(&["class", "active coins seen", "frozen (<=1 change)", "mid changes/hour median", "max", "net move bp (|last-first|) median", "max", "range bp median", "sum of |moves| bp/hour median"], &rows));
    let mut rows = Vec::new();
    for coin in TRACKED {
        if let Some(c) = series.get(coin) {
            rows.push(vec![coin.to_string(), c.changes.len().saturating_sub(1).to_string(), f(gap_bps(c.last as f64, c.first as f64), 2), f(gap_bps(c.max as f64, c.min as f64), 2), f(c.abs_move_bps * 60.0 / window_min.max(0.001), 1)]);
        }
    }
    s.push_str("Tracked coins:\n\n");
    s.push_str(&table(&["coin", "mid changes in window", "net move bp", "range bp", "sum |moves| bp/hour"], &rows));
    // GMX comparison
    let hl_at = |coin: &str, t: u64| -> Option<f64> {
        // mid nearest in time to the GMX fetch
        am.iter().min_by_key(|m| (m.recv_ms as i64 - t as i64).abs()).and_then(|m| m.mids.iter().find(|(c, _)| c == coin)).map(|(_, v)| *v as f64 / 1e12)
    };
    let mut rows = Vec::new();
    for (g, h) in [("GOLD", "xyz:GOLD"), ("SILVER", "xyz:SILVER"), ("WTIOIL", "xyz:CL"), ("BRENTOIL", "xyz:BRENTOIL"), ("NATGAS", "xyz:NATGAS"), ("SPY", "xyz:SP500"), ("QQQ", "xyz:XYZ100")] {
        let (gv, hv) = (gmx.get(g).copied(), hl_at(h, gmx_fetch_ms));
        rows.push(vec![
            g.to_string(), h.to_string(), gv.map_or("-".into(), |x| f(x, 4)), hv.map_or("-".into(), |x| f(x, 4)),
            match (gv, hv) { (Some(a), Some(b)) if matches!(g, "SPY" | "QQQ") => format!("ratio HL/GMX {}", f(b / a, 3)), (Some(a), Some(b)) => format!("{} bp", f(gap_bps(b, a), 1)), _ => "-".into() },
        ]);
    }
    s.push_str(&format!("### GMX comparison (one live fetch of GMX /prices/tickers at {}; GMX median USD = (minPrice + maxPrice) / 2 / 10^12 for 18-decimal tokens; HL value = nearest allMids mid)\n\n", utc_string(gmx_fetch_ms)));
    s.push_str(&table(&["GMX symbol", "HL coin", "GMX USD", "HL mid USD", "difference (HL - GMX)"], &rows));
    s.push_str("SPY and QQQ are ETFs while `xyz:SP500` and `xyz:XYZ100` are index levels, so only the ratio is shown (it need not be a constant).\n\n");
    // FX direction
    let mut rows = Vec::new();
    for base in ["EUR", "GBP", "JPY", "KRW"] {
        let coin = format!("xyz:{base}");
        let lvl = hl_at(&coin, now_ms()).or_else(|| ctxs.get(&coin).and_then(|c| c.mid.or(c.mark)));
        rows.push(vec![coin.clone(), lvl.map_or("-".into(), |x| format!("{x}")), lvl.map_or("-".into(), |x| fx_direction(base, x).to_string()), if coins.iter().any(|c| c.name == coin && c.delisted) { "delisted".into() } else { "active".into() }]);
    }
    s.push_str("## 7b. FX quote direction\n\n");
    s.push_str(&table(&["coin", "level seen", "direction", "status"], &rows));
    s.push_str(&candle_s);

    // ---- 8. storage
    let mut bin_bbo: Vec<u8> = Vec::new();
    let bbo_idx: HashMap<&str, u16> = BBO_COINS.iter().enumerate().map(|(i, c)| (*c, i as u16)).collect();
    for b in &bm {
        if let (Some(bid), Some(ask)) = (b.bid, b.ask) {
            bin_bbo.extend_from_slice(&Record::from_bbo(bbo_idx.get(b.coin.as_str()).copied().unwrap_or(0), b.time, bid.0, ask.0).to_bytes());
        }
    }
    let p1 = dir().join("ws-allmids-changes.bin");
    let p2 = dir().join("ws-bbo.bin");
    let _ = fs::write(&p1, &bin_mid);
    let _ = fs::write(&p2, &bin_bbo);
    let mut sums = String::new();
    for p in [&p1, &p2] {
        let bytes = fs::read(p).unwrap_or_default();
        sums.push_str(&format!("{}  {}\n", hex(&Sha256::digest(&bytes)), p.file_name().and_then(|x| x.to_str()).unwrap_or("?")));
    }
    let _ = fs::write(dir().join("SHA256SUMS"), &sums);
    let scale = 10.0 / (ws_secs as f64 / 60.0);
    let per_market = |bytes: usize, markets: usize| bytes as f64 * scale / markets.max(1) as f64 * 4320.0;
    let (m1, m2) = (per_market(bin_mid.len(), stored_coins.len()), per_market(bin_bbo.len(), BBO_COINS.len()));
    s.push_str("## 8. Storage (DESIGN 4.5 shape: market, timestamp, bid, ask, mid; 64-byte records, prices x1e12)\n\n");
    s.push_str(&table(
        &["file", "records", "bytes", "bytes per 10 min", "markets in file", "per market per month", "x 100 markets per month"],
        &[
            vec!["ws-allmids-changes.bin (mid only; one record per mid change of any xyz coin)".into(), (bin_mid.len() / RECORD_BYTES).to_string(), bin_mid.len().to_string(), f(bin_mid.len() as f64 * scale, 0), stored_coins.len().to_string(), format!("{} MB", f(m1 / 1e6, 2)), format!("{} GB", f(m1 * 100.0 / 1e9, 2))],
            vec!["ws-bbo.bin (bid, ask, mid; every bbo message of 6 coins)".into(), (bin_bbo.len() / RECORD_BYTES).to_string(), bin_bbo.len().to_string(), f(bin_bbo.len() as f64 * scale, 0), BBO_COINS.len().to_string(), format!("{} MB", f(m2 / 1e6, 2)), format!("{} GB", f(m2 * 100.0 / 1e9, 2))],
        ],
    ));
    s.push_str(&format!("sha256 (also in data/SHA256SUMS):\n\n```\n{sums}```\n\n"));
    report.push_str(&s);

    // ---- summary
    let statuses = rest.statuses.lock().map(|m| m.clone()).unwrap_or_default();
    let incidents = rest.incidents.lock().map(|m| m.clone()).unwrap_or_default();
    let errors = rest.errors.lock().map(|m| m.clone()).unwrap_or_default();
    report.push_str(&format!(
        "## Run summary\n\nRun ended {} after {} s. REST: {} requests, total weight {} (highest trailing-60 s sum {} of the {} budget), status counts {:?}, 429/403 incidents {:?}, transport errors {:?}. WebSocket: connections 2 (limit 3), incidents {:?} / {:?}.\n",
        utc_string(now_ms()), t_run.elapsed().as_secs(), rest.requests.lock().map(|n| *n).unwrap_or(0), rest.weight_total.lock().map(|n| *n).unwrap_or(0),
        rest.max_window_weight.lock().map(|n| *n).unwrap_or(0), WEIGHT_BUDGET, statuses, incidents, errors, ws1.incidents, ws2.incidents
    ));
    flush(&report);
    println!("{report}");
}
