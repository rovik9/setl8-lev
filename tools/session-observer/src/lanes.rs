//! The network side: one thread per lane (GMX tickers, Hyperliquid WebSocket, Hyperliquid REST, public reference, Pyth market
//! hours), a heartbeat thread, and the run loop. PUBLIC READ-ONLY endpoints only (never `/exchange`, no keys, no accounts, no
//! signing), `User-Agent: setl8-probe/0.1`, every lane well under half of the documented limits. Excluded from mutation testing
//! (it needs the network); the decisions it takes (change filter, rotation, back-off, `--until`) are tested in their own modules.
use crate::guard::{should_stop, Guard, Verdict};
use crate::markets::{self, Market};
use crate::store::{self, ChangeFilter, LaneWriter};
use crate::timeutil::{fmt_utc, now_ms};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

const UA: &str = "setl8-probe/0.1";
const GMX_HOSTS: [&str; 3] = ["arbitrum-api.gmxinfra.io", "arbitrum-api-fallback.gmxinfra.io", "arbitrum-api-fallback.gmxinfra2.io"];
const HL_WS: &str = "wss://api.hyperliquid.xyz/ws";
const HL_INFO: &str = "https://api.hyperliquid.xyz/info";
const COINBASE: &str = "https://api.exchange.coinbase.com";
const HERMES: &str = "https://hermes.pyth.network";

#[derive(Default, Clone, Debug)]
pub struct LaneStats {
    pub requests: u64,
    pub errors: u64,
    pub rows: u64,
    pub reconnects: u64,
    pub http429: u64,
    pub http403: u64,
    pub last_change: BTreeMap<String, u64>,
}

pub struct Shared {
    pub root: PathBuf,
    pub stop: AtomicBool,
    pub until: Option<u64>,
    stats: Mutex<BTreeMap<&'static str, LaneStats>>,
    log: Mutex<std::fs::File>,
}

impl Shared {
    pub fn new(root: PathBuf, until: Option<u64>) -> std::io::Result<Arc<Shared>> {
        std::fs::create_dir_all(&root)?;
        let log = OpenOptions::new().create(true).append(true).open(root.join("observer.log"))?;
        Ok(Arc::new(Shared { root, stop: AtomicBool::new(false), until, stats: Mutex::new(BTreeMap::new()), log: Mutex::new(log) }))
    }

    pub fn log(&self, msg: &str) {
        let line = format!("{} {msg}", fmt_utc(now_ms()));
        println!("{line}");
        if let Ok(mut f) = self.log.lock() {
            let _ = writeln!(f, "{line}");
        }
    }

    fn stopping(&self) -> bool {
        should_stop(now_ms(), self.until, self.stop.load(Ordering::SeqCst))
    }

    fn with<R>(&self, lane: &'static str, f: impl FnOnce(&mut LaneStats) -> R) -> R {
        let mut g = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        f(g.entry(lane).or_default())
    }

    fn row(&self, lane: &'static str, key: &str, ms: u64) {
        self.with(lane, |s| {
            s.rows += 1;
            s.last_change.insert(key.to_string(), ms);
        });
    }

    /// Sleeps `ms` in short slices so a stop is noticed within 200 ms.
    fn sleep(&self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end && !self.stopping() {
            thread::sleep(Duration::from_millis(200.min(ms)));
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).user_agent(UA).build()
}

/// GET or POST: `(status, body)`; transport errors are `Err`.
fn call(agent: &ureq::Agent, url: &str, body: Option<&Value>) -> Result<(u16, String), String> {
    let r = match body {
        Some(b) => agent.post(url).set("Content-Type", "application/json").send_string(&b.to_string()),
        None => agent.get(url).call(),
    };
    match r {
        Ok(resp) => Ok((resp.status(), resp.into_string().map_err(|e| format!("read body: {e}"))?)),
        Err(ureq::Error::Status(code, resp)) => Ok((code, resp.into_string().unwrap_or_default())),
        Err(e) => Err(format!("{url}: {e}")),
    }
}

/// One request through the lane's guard and statistics. `None` when the guard forbids sending or the request failed.
fn guarded(sh: &Shared, lane: &'static str, guard: &mut Guard, agent: &ureq::Agent, url: &str, body: Option<&Value>) -> Option<String> {
    if !guard.may_send(now_ms()) {
        return None;
    }
    sh.with(lane, |s| s.requests += 1);
    match call(agent, url, body) {
        Err(e) => {
            sh.with(lane, |s| s.errors += 1);
            sh.log(&format!("{lane}: {e}"));
            None
        }
        Ok((status, text)) => {
            let v = guard.on_status(Some(status), now_ms());
            if status == 429 || status == 403 {
                sh.with(lane, |s| {
                    s.errors += 1;
                    if status == 429 {
                        s.http429 += 1
                    } else {
                        s.http403 += 1
                    }
                });
                sh.log(&format!("{lane}: HTTP {status} on {url} -> {}", if v == Verdict::LongPause { "pause 10 min" } else { "back off 60 s" }));
                return None;
            }
            if !(200..300).contains(&status) {
                sh.with(lane, |s| s.errors += 1);
                sh.log(&format!("{lane}: HTTP {status} on {url}"));
                return None;
            }
            Some(text)
        }
    }
}

/// Runs `body` again after a panic; returns when it returns.
fn supervised(sh: &Arc<Shared>, name: &'static str, body: impl Fn(&Arc<Shared>) + Send + Sync + 'static) {
    loop {
        if catch_unwind(AssertUnwindSafe(|| body(sh))).is_ok() || sh.stopping() {
            return;
        }
        sh.log(&format!("{name}: panicked, restarting the lane in 5 s"));
        sh.sleep(5_000);
        if sh.stopping() {
            return;
        }
    }
}

// ------------------------------------------------------------------ GMX

fn token_decimals(text: &str) -> std::collections::HashMap<String, i32> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let mut out = std::collections::HashMap::new();
    if let Some(arr) = v.get("tokens").and_then(|t| t.as_array()) {
        for t in arr {
            if let (Some(s), Some(d)) = (t.get("symbol").and_then(|x| x.as_str()), t.get("decimals").and_then(|x| x.as_i64())) {
                out.insert(s.to_string(), d as i32);
            }
        }
    }
    out
}

fn gmx_get(sh: &Shared, guard: &mut Guard, ag: &ureq::Agent, path: &str) -> Option<String> {
    for host in GMX_HOSTS {
        if let Some(t) = guarded(sh, "gmx", guard, ag, &format!("https://{host}{path}"), None) {
            return Some(t);
        }
        if !guard.may_send(now_ms()) {
            return None;
        }
    }
    None
}

fn gmx_lane(sh: &Arc<Shared>, symbols: &[String]) {
    let ag = agent();
    let mut guard = Guard::new();
    let mut w = LaneWriter::new(&sh.root, "gmx", "recv_ms,symbol,updated_at_ms,min,max,mid");
    let mut sw = LaneWriter::new(&sh.root, "gmx-state", "recv_ms,markets,status_counts,closed_flags,transition_timestamps_set");
    let mut filter = ChangeFilter::new();
    let mut decimals = std::collections::HashMap::new();
    let (mut next_tokens, mut next_state) = (0u64, 0u64);
    while !sh.stopping() {
        let tick = Instant::now();
        let now = now_ms();
        if now >= next_tokens {
            if let Some(t) = gmx_get(sh, &mut guard, &ag, "/tokens") {
                let d = token_decimals(&t);
                if !d.is_empty() {
                    decimals = d;
                    next_tokens = now + 3_600_000;
                }
            }
        }
        if let Some(text) = gmx_get(sh, &mut guard, &ag, "/prices/tickers") {
            let recv = now_ms();
            if let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(&text) {
                for r in &rows {
                    let sym = r.get("tokenSymbol").and_then(|x| x.as_str()).unwrap_or("");
                    if !symbols.iter().any(|s| s == sym) {
                        continue;
                    }
                    let (Some(min), Some(max), Some(dec)) = (r.get("minPrice").and_then(|x| x.as_str()), r.get("maxPrice").and_then(|x| x.as_str()), decimals.get(sym)) else { continue };
                    let upd = r.get("updatedAt").and_then(|x| x.as_u64()).unwrap_or(0);
                    if !filter.changed(sym, &format!("{upd}|{min}|{max}")) {
                        continue;
                    }
                    let (Some(a), Some(b)) = (gmx_probe::usd_price(min, *dec, None), gmx_probe::usd_price(max, *dec, None)) else { continue };
                    if w.write(recv, &format!("{recv},{sym},{upd},{a},{b},{}", (a + b) / 2.0)).is_ok() {
                        sh.row("gmx", sym, recv);
                    }
                }
            }
        }
        if now_ms() >= next_state {
            next_state = now_ms() + 300_000;
            if let Some(text) = gmx_get(sh, &mut guard, &ag, "/markets/state") {
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                let list = v.as_array().cloned().unwrap_or_default();
                let mut counts: BTreeMap<String, u32> = BTreeMap::new();
                let (mut closed, mut trans) = (0, 0);
                for m in &list {
                    *counts.entry(m.get("status").map(|s| s.to_string().trim_matches('"').to_string()).unwrap_or_default()).or_default() += 1;
                    closed += u32::from(m.get("isMarketClosed").and_then(|x| x.as_bool()).unwrap_or(false));
                    trans += u32::from(!m.get("latestMarketStatusTransitionTimestamp").map(|x| x.is_null()).unwrap_or(true));
                }
                let cs = counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";");
                let recv = now_ms();
                let _ = sw.write(recv, &format!("{recv},{},{cs},{closed},{trans}", list.len()));
            }
        }
        let _ = w.tick(now_ms());
        let _ = sw.tick(now_ms());
        let spent = tick.elapsed();
        sh.sleep(5_000u64.saturating_sub(spent.as_millis() as u64));
    }
    let _ = w.close();
    let _ = sw.close();
}

// ------------------------------------------------------------------ Hyperliquid

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

type Sock = WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

fn set_timeout(sock: &mut Sock, d: Duration) {
    match sock.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(d));
        }
        tungstenite::stream::MaybeTlsStream::Rustls(s) => {
            let _ = s.get_mut().set_read_timeout(Some(d));
        }
        _ => {}
    }
}

fn hl_ws_lane(sh: &Arc<Shared>, lane: &'static str, coins: Vec<String>, file: &str) {
    let mut guard = Guard::new();
    let mut w = LaneWriter::new(&sh.root, file, "recv_ms,transport,coin,oraclePx,markPx,midPx");
    let mut filter = ChangeFilter::new();
    let mut fails = 0u32;
    while !sh.stopping() {
        if !guard.may_send(now_ms()) {
            sh.sleep(guard.wait_ms(now_ms()).min(5_000));
            continue;
        }
        let req = match HL_WS.into_client_request() {
            Ok(mut r) => {
                r.headers_mut().insert("User-Agent", UA.parse().unwrap_or_else(|_| "setl8-probe".parse().expect("static header")));
                r
            }
            Err(e) => {
                sh.log(&format!("{lane}: request: {e}"));
                sh.sleep(60_000);
                continue;
            }
        };
        sh.with(lane, |s| s.requests += 1);
        let mut sock = match tungstenite::connect(req) {
            Ok((s, _)) => s,
            Err(e) => {
                let status = match &e {
                    tungstenite::Error::Http(r) => Some(r.status().as_u16()),
                    _ => None,
                };
                let v = guard.on_status(status, now_ms());
                sh.with(lane, |s| {
                    s.errors += 1;
                    s.reconnects += 1;
                    if status == Some(429) {
                        s.http429 += 1
                    }
                    if status == Some(403) {
                        s.http403 += 1
                    }
                });
                fails += 1;
                sh.log(&format!("{lane}: connect error {e} ({v:?})"));
                sh.sleep((5_000u64 << fails.min(4)).min(60_000));
                continue;
            }
        };
        set_timeout(&mut sock, Duration::from_secs(5));
        for c in &coins {
            let _ = sock.send(Message::Text(json!({"method":"subscribe","subscription":{"type":"activeAssetCtx","coin":c}}).to_string()));
        }
        sh.log(&format!("{lane}: connected, {} coins", coins.len()));
        let (mut last_ping, mut last_msg) = (Instant::now(), Instant::now());
        let mut got_any = false;
        while !sh.stopping() {
            if last_ping.elapsed() >= Duration::from_secs(30) {
                let _ = sock.send(Message::Text(r#"{"method":"ping"}"#.to_string()));
                last_ping = Instant::now();
            }
            match sock.read() {
                Ok(Message::Text(t)) => {
                    last_msg = Instant::now();
                    got_any = true;
                    fails = 0;
                    let recv = now_ms();
                    let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                    if v.get("channel").and_then(|c| c.as_str()) != Some("activeAssetCtx") {
                        continue;
                    }
                    let data = &v["data"];
                    let coin = str_of(data, "coin");
                    let ctx = &data["ctx"];
                    let (o, m, d) = (str_of(ctx, "oraclePx"), str_of(ctx, "markPx"), str_of(ctx, "midPx"));
                    if coin.is_empty() || !filter.changed(&coin, &format!("{o}|{m}|{d}")) {
                        continue;
                    }
                    if w.write(recv, &format!("{recv},ws,{coin},{o},{m},{d}")).is_ok() {
                        sh.row(lane, &coin, recv);
                    }
                }
                Ok(Message::Close(c)) => {
                    sh.log(&format!("{lane}: server close {c:?}"));
                    break;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    sh.log(&format!("{lane}: read error {e}"));
                    break;
                }
            }
            if last_msg.elapsed() > Duration::from_secs(90) {
                sh.log(&format!("{lane}: silent for 90 s, reconnecting"));
                break;
            }
            let _ = w.tick(now_ms());
        }
        let _ = sock.close(None);
        sh.with(lane, |s| s.reconnects += 1);
        if !got_any {
            fails += 1;
        }
        if !sh.stopping() {
            sh.sleep((2_000u64 << fails.min(5)).min(60_000));
        }
    }
    let _ = w.close();
}

fn hl_rest_lane(sh: &Arc<Shared>, tracked: Vec<String>) {
    let ag = agent();
    let mut guard = Guard::new();
    let mut w = LaneWriter::new(&sh.root, "hl-rest", "recv_ms,transport,coin,oraclePx,markPx,midPx");
    let mut filter = ChangeFilter::new();
    let body = json!({"type":"metaAndAssetCtxs","dex":"xyz"});
    while !sh.stopping() {
        let t0 = Instant::now();
        if let Some(text) = guarded(sh, "hl-rest", &mut guard, &ag, HL_INFO, Some(&body)) {
            let recv = now_ms();
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let names = v[0]["universe"].as_array().cloned().unwrap_or_default();
                let ctxs = v[1].as_array().cloned().unwrap_or_default();
                for (n, c) in names.iter().zip(ctxs.iter()) {
                    let coin = str_of(n, "name");
                    if !tracked.contains(&coin) {
                        continue;
                    }
                    let (o, m, d) = (str_of(c, "oraclePx"), str_of(c, "markPx"), str_of(c, "midPx"));
                    if filter.changed(&coin, &format!("{o}|{m}|{d}")) && w.write(recv, &format!("{recv},rest,{coin},{o},{m},{d}")).is_ok() {
                        sh.row("hl-rest", &coin, recv);
                    }
                }
            }
        }
        let _ = w.tick(now_ms());
        sh.sleep(30_000u64.saturating_sub(t0.elapsed().as_millis() as u64));
    }
    let _ = w.close();
}

// ------------------------------------------------------------------ public reference (Coinbase Exchange spot tickers)

fn ref_lane(sh: &Arc<Shared>) {
    let ag = agent();
    let mut guard = Guard::new();
    let mut w = LaneWriter::new(&sh.root, "ref", "recv_ms,product,symbol,server_ms,price,bid,ask");
    let mut filter = ChangeFilter::new();
    while !sh.stopping() {
        let t0 = Instant::now();
        for (sym, product) in markets::REFERENCES {
            if let Some(text) = guarded(sh, "ref", &mut guard, &ag, &format!("{COINBASE}/products/{product}/ticker"), None) {
                let recv = now_ms();
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                let t = str_of(&v, "time");
                let server = gmx_probe::parse_iso_ms(&t).unwrap_or(0);
                let (p, b, a) = (str_of(&v, "price"), str_of(&v, "bid"), str_of(&v, "ask"));
                if filter.changed(product, &format!("{server}|{p}|{b}|{a}")) && w.write(recv, &format!("{recv},{product},{sym},{server},{p},{b},{a}")).is_ok() {
                    sh.row("ref", sym, recv);
                }
            }
            sh.sleep(300);
        }
        let _ = w.tick(now_ms());
        sh.sleep(5_000u64.saturating_sub(t0.elapsed().as_millis() as u64));
    }
    let _ = w.close();
}

// ------------------------------------------------------------------ Pyth: feed resolution and published market hours (prices need a key)

fn pyth_lane(sh: &Arc<Shared>) {
    let ag = agent();
    let mut guard = Guard::new();
    let mut feeds = LaneWriter::new(&sh.root, "pyth-feeds", "recv_ms,launch_symbol,pyth_symbol,id,asset_type,schedule");
    let mut hours = LaneWriter::new(&sh.root, "pyth-hours", "recv_ms,launch_symbol,pyth_symbol,is_open,next_open,next_close");
    let mut filter = ChangeFilter::new();
    let mut ids: Vec<(String, String, String)> = Vec::new();
    let (mut resolved, mut next_probe) = (false, 0u64);
    let mut next_hours = 0u64;
    while !sh.stopping() {
        let now = now_ms();
        if now >= next_hours {
            next_hours = now + 600_000;
            let mut found = 0;
            for (launch, pyth) in markets::PYTH_SYMBOLS {
                let url = format!("{HERMES}/v2/price_feeds?query={}", pyth.replace('/', "%2F"));
                let Some(text) = guarded(sh, "pyth", &mut guard, &ag, &url, None) else {
                    sh.sleep(500);
                    continue;
                };
                let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&text) else { continue };
                let hit = list.iter().find(|f| f["attributes"]["symbol"].as_str() == Some(pyth));
                let recv = now_ms();
                match hit {
                    None => {
                        if !resolved {
                            sh.log(&format!("pyth: NOT FOUND {pyth} (for {launch})"));
                        }
                    }
                    Some(f) => {
                        found += 1;
                        let id = str_of(f, "id");
                        if !resolved {
                            let _ = feeds.write(recv, &format!("{recv},{launch},{pyth},{id},{},\"{}\"", str_of(&f["attributes"], "asset_type"), str_of(&f["attributes"], "schedule")));
                            ids.push((launch.to_string(), pyth.to_string(), id));
                        }
                        let mh = &f["market_hours"];
                        let (open, no, nc) = (mh["is_open"].to_string(), mh["next_open"].to_string(), mh["next_close"].to_string());
                        if filter.changed(pyth, &format!("{open}|{no}|{nc}")) && hours.write(recv, &format!("{recv},{launch},{pyth},{open},{no},{nc}")).is_ok() {
                            sh.row("pyth", launch, recv);
                        }
                    }
                }
                sh.sleep(500);
            }
            if !resolved {
                resolved = true;
                sh.log(&format!("pyth: resolved {found} of {} feeds (prices are not recorded: see the next line)", markets::PYTH_SYMBOLS.len()));
            }
        }
        // the price endpoints answered 401 (an API key is required) on 2026-10-17: check once an hour whether that changed
        if now >= next_probe {
            next_probe = now + 3_600_000;
            if let Some((_, _, id)) = ids.first() {
                let url = format!("{HERMES}/v2/updates/price/latest?ids[]={id}&parsed=true&encoding=hex");
                sh.with("pyth", |s| s.requests += 1);
                match call(&ag, &url, None) {
                    Ok((200, _)) => sh.log("pyth: the price endpoint answered 200 without a key now (this build still records market hours only)"),
                    Ok((s, _)) => sh.log(&format!("pyth: price endpoint HTTP {s} (a key is required); only market hours are recorded")),
                    Err(e) => sh.log(&format!("pyth: {e}")),
                }
            }
        }
        let _ = (feeds.tick(now_ms()), hours.tick(now_ms()));
        sh.sleep(5_000);
    }
    let _ = (feeds.close(), hours.close());
}

// ------------------------------------------------------------------ heartbeat

const LANES: [&str; 5] = ["gmx", "hl-ws-a", "hl-ws-b", "hl-rest", "ref"];

fn heartbeat(sh: &Arc<Shared>) {
    let mut w = LaneWriter::new(&sh.root, "heartbeat", "utc_ms,lane,requests,errors,rows,reconnects,http429,http403,last_change_ms_by_market");
    let mut next = now_ms();
    while !sh.stopping() {
        let now = now_ms();
        if now >= next {
            next = now + 60_000;
            let snapshot: Vec<(&'static str, LaneStats)> = {
                let g = sh.stats.lock().unwrap_or_else(|e| e.into_inner());
                g.iter().map(|(k, v)| (*k, v.clone())).collect()
            };
            let mut summary = Vec::new();
            for (lane, s) in &snapshot {
                let lc = s.last_change.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";");
                let _ = w.write(now, &format!("{now},{lane},{},{},{},{},{},{},{lc}", s.requests, s.errors, s.rows, s.reconnects, s.http429, s.http403));
                summary.push(format!("{lane}: req {} err {} rows {} reconn {}", s.requests, s.errors, s.rows, s.reconnects));
            }
            sh.log(&format!("status | {}", summary.join(" | ")));
        }
        let _ = w.tick(now_ms());
        sh.sleep(1_000);
    }
    let _ = w.close();
}

/// Runs every lane until a signal or `--until`; returns after a clean flush.
pub fn run(markets: Vec<Market>, root: PathBuf, until: Option<u64>) -> Result<(), String> {
    let sh = Shared::new(root.clone(), until).map_err(|e| format!("data dir: {e}"))?;
    sh.log(&store::restart_line(store::last_heartbeat_ms(&root), now_ms()));
    let gmx_syms: Vec<String> = markets.iter().filter(|m| m.source == 2).map(|m| m.symbol.clone()).collect();
    let coins = markets::hl_coins(&markets);
    sh.log(&format!("start: {} markets ({} GMX, {} Hyperliquid), {} Hyperliquid coins, until {:?}", markets.len(), gmx_syms.len(), markets.iter().filter(|m| m.source == 4).count(), coins.len(), until.map(fmt_utc)));
    {
        let sh2 = sh.clone();
        ctrlc::set_handler(move || {
            sh2.stop.store(true, Ordering::SeqCst);
        })
        .map_err(|e| format!("signal handler: {e}"))?;
    }
    let (a, b): (Vec<String>, Vec<String>) = {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for (i, c) in coins.iter().enumerate() {
            if i % 2 == 0 { a.push(c.clone()) } else { b.push(c.clone()) }
        }
        (a, b)
    };
    let tracked: Vec<String> = coins.iter().filter(|c| c.starts_with("xyz:")).cloned().collect();
    let mut handles = Vec::new();
    {
        let (s, syms) = (sh.clone(), gmx_syms);
        handles.push(thread::spawn(move || supervised(&s, "gmx", move |sh| gmx_lane(sh, &syms))));
    }
    for (lane, coins, file) in [("hl-ws-a", a, "hl-a"), ("hl-ws-b", b, "hl-b")] {
        let s = sh.clone();
        handles.push(thread::spawn(move || supervised(&s, lane, move |sh| hl_ws_lane(sh, lane, coins.clone(), file))));
    }
    {
        let (s, t) = (sh.clone(), tracked);
        handles.push(thread::spawn(move || supervised(&s, "hl-rest", move |sh| hl_rest_lane(sh, t.clone()))));
    }
    {
        let s = sh.clone();
        handles.push(thread::spawn(move || supervised(&s, "ref", ref_lane)));
    }
    {
        let s = sh.clone();
        handles.push(thread::spawn(move || supervised(&s, "pyth", pyth_lane)));
    }
    {
        let s = sh.clone();
        handles.push(thread::spawn(move || heartbeat(&s)));
    }
    while !sh.stopping() {
        thread::sleep(Duration::from_millis(500));
    }
    sh.log("stopping: flushing lanes");
    for h in handles {
        let _ = h.join();
    }
    sh.log("stopped cleanly");
    let _ = LANES;
    Ok(())
}
