//! `config/launch.json` (the exact `CreateMarketArgs` list in id order and the exact `RuleSetArgs`, every integer a decimal
//! string) and `config/launch.md` (the table for humans). Both are pure functions of the input: two runs give identical bytes.
use crate::derive::*;
use crate::model::*;
use crate::session;
use std::fmt::Write;

/// Minimal ordered JSON value: objects keep their insertion order, arrays of scalars print on one line.
enum J {
    S(String),
    A(Vec<J>),
    O(Vec<(&'static str, J)>),
}

fn n<T: std::fmt::Display>(v: T) -> J {
    J::S(v.to_string())
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn scalar(j: &J) -> bool {
    match j {
        J::S(_) => true,
        J::A(v) => v.iter().all(scalar),
        J::O(_) => false,
    }
}

fn inline(j: &J, out: &mut String) {
    match j {
        J::S(s) => {
            let _ = write!(out, "\"{s}\"");
        }
        J::A(v) => {
            out.push('[');
            for (k, x) in v.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                inline(x, out);
            }
            out.push(']');
        }
        J::O(_) => unreachable!("objects are never inline"),
    }
}

fn pretty(j: &J, depth: usize, out: &mut String) {
    if scalar(j) {
        return inline(j, out);
    }
    let pad = "  ".repeat(depth + 1);
    let (open, close) = if matches!(j, J::O(_)) { ('{', '}') } else { ('[', ']') };
    out.push(open);
    out.push('\n');
    match j {
        J::O(kv) => {
            for (k, (key, v)) in kv.iter().enumerate() {
                let _ = write!(out, "{pad}\"{key}\": ");
                pretty(v, depth + 1, out);
                out.push_str(if k + 1 < kv.len() { ",\n" } else { "\n" });
            }
        }
        J::A(items) => {
            for (k, v) in items.iter().enumerate() {
                out.push_str(&pad);
                pretty(v, depth + 1, out);
                out.push_str(if k + 1 < items.len() { ",\n" } else { "\n" });
            }
        }
        J::S(_) => unreachable!(),
    }
    out.push_str(&"  ".repeat(depth));
    out.push(close);
}

fn strs<T: std::fmt::Display>(v: &[T]) -> J {
    J::A(v.iter().map(n).collect())
}

pub fn json(l: &Launch) -> String {
    let markets = l
        .markets
        .iter()
        .map(|m| {
            let sessions = session::padded(&m.sessions);
            J::O(vec![
                ("market_id", n(m.id)),
                ("class", n(m.class)),
                (
                    "params",
                    J::O(vec![
                        ("status", n(m.status)),
                        ("feed_id", J::S(hex(&m.feed_id))),
                        ("max_age_secs", n(m.max_age_secs)),
                        ("qty_step", n(m.qty_step)),
                        ("qty_min", n(m.qty_min)),
                        ("max_notional_usd", n(m.max_notional_usd)),
                        ("session_count", n(m.sessions.len())),
                        ("sessions", J::A(sessions.iter().map(|(s, e)| J::A(vec![n(s), n(e)])).collect())),
                        ("quote_conv_market", n(m.quote_conv_market)),
                    ]),
                ),
                ("meta", J::O(vec![("symbol", J::S(m.symbol.clone())), ("source", n(m.source))])),
            ])
        })
        .collect();
    let r = &l.ruleset;
    let ruleset = J::O(vec![
        ("version", n(r.version)),
        ("fee_bps", n(r.fee_bps)),
        ("tier_count", n(r.tier_count)),
        ("ladder_len", n(r.ladder_len)),
        ("leverage_x", strs(&r.leverage_x)),
        ("holding_fee_e6", strs(&r.holding_fee_e6)),
        ("commission_e6", strs(&r.commission_e6)),
        ("tier_size", strs(&r.tier_size)),
        ("tier_cost", strs(&r.tier_cost)),
        ("reset_ladder_bps", strs(&r.reset_ladder_bps)),
        ("spread_e6", strs(&r.spread_e6)),
    ]);
    let root = J::O(vec![("format", J::S("setl8-launch".into())), ("version", n(1)), ("markets", J::A(markets)), ("ruleset", ruleset)]);
    let mut out = String::new();
    pretty(&root, 0, &mut out);
    out.push('\n');
    out
}

fn usd(micro: u64) -> String {
    let whole = micro / 1_000_000;
    let s = whole.to_string();
    let mut g = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k).is_multiple_of(3) {
            g.push(',');
        }
        g.push(c);
    }
    let frac = micro % 1_000_000;
    if frac == 0 {
        format!("${g}")
    } else {
        format!("${g}.{:06}", frac).trim_end_matches('0').to_string()
    }
}

fn comma(n: u64) -> String {
    usd(n * 1_000_000)
}

fn secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

const CLASS_NAMES: [&str; CLASSES] = ["FX majors", "FX minors", "Metals", "Indices", "Energy", "Stocks", "Crypto majors", "Crypto alts"];

/// The owner's fixed costs (decision 72) in words, from the numbers of the rule set.
fn owner_costs(i: &Input, r: &RuleSetOut) -> String {
    let (cmin, cmax) = (r.commission_e6.iter().min().copied().unwrap_or(0), r.commission_e6.iter().max().copied().unwrap_or(0));
    let (smin, smax) = (i.market.iter().map(|m| m.half_spread_e6).min().unwrap_or(0), i.market.iter().map(|m| m.half_spread_e6).max().unwrap_or(0));
    let comm = if cmin == cmax { format!("{cmin} e6 of notional ({} %) on every class", pct_e6(u64::from(cmin))) } else { format!("{cmin} to {cmax} e6 of notional by class") };
    let spr = if smin == smax { format!("{smin} e6 ({} bp of price per side, {} bp full) on every market", bp_e6_half(smin), bp_e6(smin)) } else { format!("{smin} to {smax} e6 by market") };
    let mut t = String::new();
    let _ = write!(t, "**Owner costs (decision 72, Rovik, 2026-10-23, revised the same day before any deploy).** Commission per side, charged at open and at close: {comm}; a round trip pays twice that, and \"once\" is read as one flat rate for every class (if Rovik meant {} % per round trip the number would be {} e6 per side). Half-spread: the owner's base is {spr}, read from \"1 pip each side\" as 1 basis point of price per side, so a round trip pays 2 bp; per-class pip sizes are not used (1 pip of EURUSD is about 0.93 bp). The rule is half-spread = max(owner base, oracle-noise floor): the floor is computed per market from the observer data (P24b) and raises the base where oracle noise is larger, so every spread stays flagged PENDING (note: \"owner base {smax} e6 set; oracle-noise floor check pending (P24b)\") and `launch-config verify --production` fails on those flags and on nothing else. News spread: not implemented, decision pending. A rule set is immutable: `create_ruleset` for v{} runs only AFTER P24b has raised the bases where needed.", pct_e6(u64::from(cmax)), cmax / 2, r.version);
    t
}

/// `e6` parts per million of notional as a percentage with `places` decimals (3 e6 = 0.0003 %): 200 e6 -> "0.02".
fn pct_e6(e6: u64) -> String {
    // e6 / 1e6 * 100 = e6 / 10_000 percent
    let hundredths = e6 * 100 / 10_000;
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// A half-spread of `e6` per million is `e6 / 100` basis points for the FULL spread when doubled: 100 e6 half -> 2.00 bp full.
fn bp_e6(half_e6: u16) -> String {
    let hundredths = u64::from(half_e6) * 2; // full spread in 1/100 bp: 2 x 100 e6 = 200 hundredths = 2.00 bp
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

fn bp_e6_half(half_e6: u16) -> String {
    let hundredths = u64::from(half_e6); // 100 e6 = 1.00 bp
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// Per class: leverage, the round-trip commission and the round-trip spread cost as a percentage of the account at full leverage, and how
/// many full-leverage round trips of commission alone use the 8 % static floor; for the FX classes the commission per 100,000 of notional
/// per side next to the reference of $3.50 per standard lot per side (Pepperstone Razor).
pub fn cost_table(i: &Input, r: &RuleSetOut) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Round trip = one side at open and one at close. Percentages are of the account at FULL leverage (leverage x notional share), truncated to two decimals. A full spread is twice the half-spread; the spread cost uses the largest half-spread of the class's markets (the owner base where the class has no market). The static floor is 8 % of the account size.\n");
    let _ = writeln!(s, "| class | name | leverage 1: | commission / side (e6) | round-trip commission (% of account) | round-trip spread (% of account) | full-leverage round trips of commission alone to the 8 % floor | commission per 100,000 notional per side (USD) |");
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|");
    let base = i.market.iter().map(|m| m.half_spread_e6).max().unwrap_or(0);
    for (k, name) in CLASS_NAMES.iter().enumerate() {
        let lev = u64::from(r.leverage_x[k]);
        let c = u64::from(r.commission_e6[k]);
        let half = i.market.iter().filter(|m| usize::from(m.class) == k).map(|m| m.half_spread_e6).max().unwrap_or(base);
        // ppm of the account: leverage x e6 per side, two sides
        let comm_ppm = 2 * lev * c;
        let spread_ppm = 2 * lev * u64::from(half);
        let trips = if comm_ppm == 0 { "-".to_string() } else { let h = 80_000u64 * 100 / comm_ppm; format!("{}.{:02}", h / 100, h % 100) };
        let usd = if k < 2 { format!("{}.{:02}", 100_000 * c / 1_000_000, 100_000 * c % 1_000_000 / 10_000) } else { "-".to_string() };
        let _ = writeln!(s, "| {k} | {name} | {lev} | {c} | {} | {} | {trips} | {usd} |", pct_ppm(comm_ppm), pct_ppm(spread_ppm));
    }
    let _ = writeln!(s, "\nReference for the FX classes: Pepperstone Razor, $3.50 per standard lot (100,000) per side. Here: ${} per 100,000 of notional per side at {} e6.", { let c = u64::from(r.commission_e6[0]); format!("{}.{:02}", 100_000 * c / 1_000_000, 100_000 * c % 1_000_000 / 10_000) }, r.commission_e6[0]);
    let _ = writeln!(s, "\nNews spread: not implemented, decision pending.");
    s
}

/// ppm of the account as a percentage with two decimals: 30,000 ppm = "3.00".
fn pct_ppm(ppm: u64) -> String {
    let h = ppm / 100;
    format!("{}.{:02}", h / 100, h % 100)
}

/// Pending flags: one per market half-spread still a placeholder and one per class commission still a placeholder.
pub fn pending_flags(i: &Input) -> usize {
    i.market.iter().filter(|m| m.spread_pending).count() + i.commission.iter().filter(|c| c.pending).count()
}

pub fn markdown(i: &Input, l: &Launch) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Launch configuration (generated, do not edit)\n");
    let _ = writeln!(s, "Generated by `launch-config` from `config/launch-markets.toml`; `config/launch.json` holds the exact arguments of `create_market` (in id order) and `create_ruleset`. Regenerate with `cargo run --release -- generate` in `tools/launch-config`, check with `launch-config verify`.\n");
    let _ = writeln!(s, "* Markets: {}. Rule set version {}. **PENDING flags: {}** (`launch-config verify --production` fails while any remains).", l.markets.len(), l.ruleset.version, pending_flags(i));
    let _ = writeln!(s, "* Sources (decisions 55 and 56): GMX tickers (source 2) for crypto majors, gold, silver, WTI, Brent, SPY and QQQ; the Hyperliquid xyz oracle (source 4) for JP225, KR200 and the liquid single stocks that pass the go-live rule. One source per market, final for v1.");
    let _ = writeln!(s, "* **SPY and QQQ are the ETFs, not the indices.** They stand in for the US500 and the Nasdaq-100 (class 3); quantities are in ETF shares and prices are the ETF prices.");
    let _ = writeln!(s, "* Every price is in USD, so every `quote_conv_market` is `NO_QUOTE_CONV` (0xFFFF): no market needs another market's price to value it.\n");

    let _ = writeln!(s, "## Markets\n");
    let _ = writeln!(s, "| id | symbol | name | source | class | max_age (s) | sessions (UTC week) | half-spread (e6) | flags |");
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|");
    for (m, o) in i.market.iter().zip(&l.markets) {
        let mut flags: Vec<String> = Vec::new();
        if m.spread_pending {
            flags.push("PENDING spread".into());
        }
        flags.extend(m.verify.iter().map(|v| format!("VERIFY {}", v.split(':').next().unwrap_or(v))));
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} {} | {} | {} | {} | {} |",
            o.id,
            m.symbol,
            m.name,
            m.source,
            m.class,
            CLASS_NAMES[usize::from(m.class)],
            o.max_age_secs,
            session::describe(&o.sessions),
            m.half_spread_e6,
            if flags.is_empty() { "-".to_string() } else { flags.join("; ") },
        );
    }

    let notes: Vec<_> = i.market.iter().filter(|m| !m.spread_note.is_empty()).collect();
    if !notes.is_empty() {
        let _ = writeln!(s, "\nSpread notes:\n");
        for m in notes {
            let _ = writeln!(s, "* {}: {}", m.symbol, m.spread_note);
        }
    }

    let _ = writeln!(s, "\n## How each number was derived\n");
    let _ = writeln!(s, "max_age = longest freeze x {}% rounded up to {} s, never below {} s (decision 57). Quantity step = minimum = the smallest 1-2-5 quantity worth at least ${} at the price used. `max_notional_usd` = {} x class leverage rounded down to 1-2-5, never above {} (stocks {}). **PLACEHOLDER-ADMIN-TUNABLE**: quantities and caps (either admin can change them at any time with `update_market`, decision 24).\n", 100 + i.rules.freeze_margin_pct, i.rules.max_age_round_secs, i.rules.max_age_floor_secs, i.rules.order_notional_usd, comma(i.rules.cap_basis_tier_usd), comma(i.rules.cap_ceiling_usd), comma(i.rules.cap_ceiling_stock_usd));
    let _ = writeln!(s, "| id | symbol | longest freeze (s) | max_age (s) | price used (USD) | qty step = min (1e-6 units) | notional of the minimum | max_notional_usd | oracle median / p95 (bp) |");
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|");
    for (m, o) in i.market.iter().zip(&l.markets) {
        let pm = price_micro(m.price_usd).unwrap_or(0);
        let notional = (u128::from(o.qty_min) * u128::from(pm) / 1_000_000) as u64;
        let stats = match (m.oracle_median_bp, m.oracle_p95_bp) {
            (Some(a), Some(b)) => format!("{a:.2} / {b:.2}"),
            _ => "n/a (GMX)".into(),
        };
        let _ = writeln!(s, "| {} | {} | {} | {} | {} | {} | {} | {} | {} |", o.id, m.symbol, secs(m.freeze_ms), o.max_age_secs, m.price_usd, o.qty_min, usd(notional), usd(o.max_notional_usd), stats);
    }
    let _ = writeln!(s, "\nWhere the price and the freeze come from:\n");
    for m in &i.market {
        let _ = writeln!(s, "* {}: price {}; freeze {}.", m.symbol, m.price_basis, m.freeze_basis);
    }

    let _ = writeln!(s, "\n## What `max_notional_usd` means for the big tiers\n");
    let _ = writeln!(s, "The cap is per position and the same for every tier. With the basis of {} (the biggest tier, $200,000) the biggest tier can hold its class leverage up to the ceilings: for gold (1:20) that is {}, and no market is above {} ({} for stocks). The basis is one line in `config/launch-markets.toml` (`cap_basis_tier_usd`), and the cap itself is changeable by either admin at any time.\n", comma(i.rules.cap_basis_tier_usd), usd(cap_micro(2, 20, &i.rules)), comma(i.rules.cap_ceiling_usd), comma(i.rules.cap_ceiling_stock_usd));
    let _ = writeln!(s, "\n## Sessions\n");
    let _ = writeln!(s, "Minutes of the UTC week (Monday 00:00 = 0), windows `[start, end)`; the windows alone decide open or closed (decision 49), the price-change freshness test comes on top (DESIGN 3.4). Where the probe data does not settle a boundary the NARROWER window is used and the market is marked VERIFY. Holidays, half days and daylight-saving changes are an admin window change (`update_market`).\n");
    let _ = writeln!(s, "| id | symbol | windows (minutes) | basis |");
    let _ = writeln!(s, "|---|---|---|---|");
    for (m, o) in i.market.iter().zip(&l.markets) {
        let w = o.sessions.iter().map(|(a, b)| format!("[{a},{b})")).collect::<Vec<_>>().join(" ");
        let _ = writeln!(s, "| {} | {} | {} | {} |", o.id, m.symbol, w, m.session_note);
    }

    let _ = writeln!(s, "\n## VERIFY items\n");
    let _ = writeln!(s, "| id | symbol | item |");
    let _ = writeln!(s, "|---|---|---|");
    for (m, o) in i.market.iter().zip(&l.markets) {
        for v in &m.verify {
            let _ = writeln!(s, "| {} | {} | {} |", o.id, m.symbol, v);
        }
    }

    let _ = writeln!(s, "\n## Excluded, with the number that failed\n");
    let _ = writeln!(s, "Go-live rule for a Hyperliquid oracle market (decision 56): median |oracle - mid| <= {} bp, p95 <= {} bp, longest freeze <= max_age_secs.\n", i.rules.golive_median_bp, i.rules.golive_p95_bp);
    let _ = writeln!(s, "| candidate | rule | measured | why |");
    let _ = writeln!(s, "|---|---|---|---|");
    for e in &i.excluded {
        let mut got = Vec::new();
        if let Some(v) = e.median_bp {
            got.push(format!("median {v:.2} bp"));
        }
        if let Some(v) = e.p95_bp {
            got.push(format!("p95 {v:.2} bp"));
        }
        if let Some(v) = e.freeze_ms {
            got.push(format!("freeze {} s", secs(v)));
        }
        let _ = writeln!(s, "| {} | {} | {} | {} |", e.symbol, e.rule, if got.is_empty() { "-".into() } else { got.join(", ") }, e.reason);
    }

    let r = &l.ruleset;
    let _ = writeln!(s, "\n## Rule set v{}\n", r.version);
    let _ = writeln!(s, "Fee {} bps. Tiers (size / price, USD): {}. Reset ladder (bps of the tier price by failed phase): {}.\n", r.fee_bps, (0..usize::from(r.tier_count)).map(|k| format!("{} / {}", r.tier_size[k] / 1_000_000, r.tier_cost[k] / 1_000_000)).collect::<Vec<_>>().join(", "), r.reset_ladder_bps[..usize::from(r.ladder_len)].iter().map(u16::to_string).collect::<Vec<_>>().join(", "));
    let _ = writeln!(s, "| class | name | leverage 1: | holding fee / night (e6) | commission / side (e6) | commission flag |");
    let _ = writeln!(s, "|---|---|---|---|---|---|");
    for (k, name) in CLASS_NAMES.iter().enumerate() {
        let pend = i.commission.iter().find(|c| usize::from(c.class) == k).is_some_and(|c| c.pending);
        let _ = writeln!(s, "| {} | {} | {} | {} | {} | {} |", k, name, r.leverage_x[k], r.holding_fee_e6[k], r.commission_e6[k], if pend { "PENDING" } else { "-" });
    }
    if i.commission.iter().any(|c| c.pending) || i.market.iter().any(|m| m.spread_pending && m.spread_note.is_empty()) {
        let _ = writeln!(s, "\nEvery half-spread above and every commission flagged PENDING is a PLACEHOLDER (decision 14: the real numbers come from Rovik's friend). A rule set is immutable: the real numbers are a NEW rule set version, old accounts keep the old one, and `create_ruleset` for v1 is run only AFTER the real numbers exist.\n");
    } else {
        let _ = writeln!(s, "\n{}\n", owner_costs(i, r));
    }
    let _ = writeln!(s, "## Cost table (numbers only)\n");
    let _ = writeln!(s, "{}", cost_table(i, r));

    let _ = writeln!(s, "## Adding a market after launch\n");
    let _ = writeln!(s, "{UNPRICED_GUARD}");
    s
}

/// The answer to P19 point 7, from the investigation recorded in handover/launch-config.md.
pub const UNPRICED_GUARD: &str = "Adding a market after launch requires a NEW rule set version that prices it, and only NEW accounts (opened under that version) can trade it. Decision 64: a market whose half-spread in the ACCOUNT'S OWN rule set is 0 is untradable. The rule set holds `spread_e6` per market id and is immutable; a market created after version N has `spread_e6[id] = 0` in version N, so an account of version N (or older) cannot open or add to a position there: `post_journal` and `report_fail` reject such a `Fill` with `MarketUnpriced` (6109), the evaluator refuses the order (at arrival and at execution, nothing is journalled) and the verifier flags the fill (`FillNotAllowed`, reason \"unpriced market\"). Closes are never refused by this rule; accounts of older versions keep trading every market their rule set prices. The executable path is `an_old_account_cannot_open_a_market_created_after_its_rule_set_program_evaluator_and_verifier_agree` in tests-rs/tests/launch_config.rs. Recommended order for a new market: create it as `status = 0` (disabled) together with the new rule set version that prices it, and enable it when that version is the current one (an account opened before that cannot trade it either way). At launch every market of this file has a half-spread above zero in rule set v1 (`verify` fails otherwise, for every market id).";
