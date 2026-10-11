//! The rules file (format version 1): the rule set the account snapshotted plus the markets, as JSON.
//!
//! Producible from the program's `RuleSet` and `Market` accounts: `fee_bps`, `leverage_x`, `holding_fee_e6`,
//! `commission_e6`, `tier_size` (first `tier_count` entries) and `spread_e6` come from `RuleSet` (`spread_e6` as an object
//! keyed by market id, only the markets that exist); every market object mirrors a `Market` account
//! (`feed_id` hex, `sessions` = the first `session_count` windows as `[start_min, end_min]`).
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
pub struct MarketJson {
    pub id: u16,
    pub class: u8,
    pub feed_id: String,
    pub max_age_secs: u16,
    /// 0 disabled, 1 enabled, 2 reduce-only.
    pub status: u8,
    pub qty_step: u64,
    pub qty_min: u64,
    pub max_notional_usd: u64,
    pub sessions: Vec<[u16; 2]>,
}

#[derive(Clone, Debug, Deserialize)]
struct RulesJson {
    version: u32,
    ruleset_version: u16,
    fee_bps: u16,
    leverage_x: Vec<u16>,
    holding_fee_e6: Vec<u32>,
    commission_e6: Vec<u32>,
    tier_sizes: Vec<u64>,
    spread_e6: BTreeMap<String, u16>,
    markets: Vec<MarketJson>,
}

#[derive(Clone, Debug)]
pub struct Market {
    pub id: u16,
    pub class: u8,
    pub feed_id: [u8; 32],
    pub max_age_secs: u16,
    pub status: u8,
    pub qty_step: u64,
    pub qty_min: u64,
    pub max_notional_usd: u64,
    pub sessions: Vec<(u16, u16)>,
}

#[derive(Clone, Debug)]
pub struct Rules {
    pub ruleset_version: u16,
    pub fee_bps: u16,
    pub leverage_x: [u16; 8],
    pub holding_fee_e6: [u32; 8],
    pub commission_e6: [u32; 8],
    pub tier_sizes: Vec<u64>,
    pub spread_e6: BTreeMap<u16, u16>,
    pub markets: BTreeMap<u16, Market>,
}

fn arr8<T: Copy + Default>(v: &[T], name: &str) -> Result<[T; 8], String> {
    if v.len() != 8 {
        return Err(format!("{name} must have 8 entries, has {}", v.len()));
    }
    let mut out = [T::default(); 8];
    for (o, x) in out.iter_mut().zip(v.iter()) {
        *o = *x;
    }
    Ok(out)
}

impl Rules {
    pub fn parse(bytes: &[u8]) -> Result<Rules, String> {
        let j: RulesJson = serde_json::from_slice(bytes).map_err(|e| format!("rules file: {e}"))?;
        if j.version != 1 {
            return Err(format!("rules file version {} is not supported (1)", j.version));
        }
        let mut spread = BTreeMap::new();
        for (k, v) in &j.spread_e6 {
            let id: u16 = k.parse().map_err(|_| format!("spread_e6 key {k:?} is not a market id"))?;
            spread.insert(id, *v);
        }
        let mut markets = BTreeMap::new();
        for m in &j.markets {
            let feed_id = crate::hexutil::decode_array::<32>(&m.feed_id).map_err(|e| format!("market {} feed_id: {e}", m.id))?;
            if m.class > 7 {
                return Err(format!("market {} class {} is above 7", m.id, m.class));
            }
            let sessions = m.sessions.iter().map(|s| (s[0], s[1])).collect();
            let market = Market {
                id: m.id,
                class: m.class,
                feed_id,
                max_age_secs: m.max_age_secs,
                status: m.status,
                qty_step: m.qty_step,
                qty_min: m.qty_min,
                max_notional_usd: m.max_notional_usd,
                sessions,
            };
            if markets.insert(m.id, market).is_some() {
                return Err(format!("market {} appears twice", m.id));
            }
        }
        Ok(Rules {
            ruleset_version: j.ruleset_version,
            fee_bps: j.fee_bps,
            leverage_x: arr8(&j.leverage_x, "leverage_x")?,
            holding_fee_e6: arr8(&j.holding_fee_e6, "holding_fee_e6")?,
            commission_e6: arr8(&j.commission_e6, "commission_e6")?,
            tier_sizes: j.tier_sizes,
            spread_e6: spread,
            markets,
        })
    }

    /// Half-spread of a market in e6 of the mid (0 when the rule set lists none).
    pub fn spread(&self, market: u16) -> u32 {
        u32::from(self.spread_e6.get(&market).copied().unwrap_or(0))
    }

    pub fn market_by_feed(&self, feed: &[u8; 32]) -> Option<&Market> {
        self.markets.values().find(|m| &m.feed_id == feed)
    }

    pub fn class_of(&self, market: u16) -> Option<usize> {
        self.markets.get(&market).map(|m| usize::from(m.class))
    }
}

#[cfg(test)]
pub(crate) fn sample_json() -> String {
    tests::sample_json()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample_json() -> String {
        let feed = crate::hexutil::encode(&[7u8; 32]);
        format!(
            r#"{{"version":1,"ruleset_version":1,"fee_bps":40,"leverage_x":[50,20,20,20,10,5,5,2],"holding_fee_e6":[100,200,400,200,400,400,1000,1600],
"commission_e6":[0,0,0,0,0,0,0,0],"tier_sizes":[100000000000],"spread_e6":{{"0":30}},
"markets":[{{"id":0,"class":0,"feed_id":"{feed}","max_age_secs":60,"status":1,"qty_step":1000,"qty_min":1000000,"max_notional_usd":5,"sessions":[[0,10080]]}}]}}"#
        )
    }

    #[test]
    fn parses_and_looks_up() {
        let r = Rules::parse(sample_json().as_bytes()).unwrap();
        assert_eq!(r.spread(0), 30);
        assert_eq!(r.spread(9), 0);
        assert_eq!(r.class_of(0), Some(0));
        assert_eq!(r.market_by_feed(&[7; 32]).map(|m| m.id), Some(0));
        assert!(r.market_by_feed(&[8; 32]).is_none());
        assert_eq!(r.markets.get(&0).unwrap().sessions, vec![(0, 10080)]);
    }

    #[test]
    fn market_class_boundary_is_seven() {
        let ok = sample_json();
        assert!(Rules::parse(ok.replace("\"class\":0", "\"class\":7").as_bytes()).is_ok());
        assert!(Rules::parse(ok.replace("\"class\":0", "\"class\":8").as_bytes()).is_err());
    }

    #[test]
    fn rejects_bad_files() {
        assert!(Rules::parse(b"nope").is_err());
        let ok = sample_json();
        assert!(Rules::parse(ok.replace("\"version\":1,", "\"version\":2,").as_bytes()).is_err());
        assert!(Rules::parse(ok.replace("[50,20,20,20,10,5,5,2]", "[50]").as_bytes()).is_err());
        assert!(Rules::parse(ok.replace("\"class\":0", "\"class\":8").as_bytes()).is_err());
        assert!(Rules::parse(ok.replace("\"0\":30", "\"x\":30").as_bytes()).is_err());
        assert!(Rules::parse(ok.replace(&crate::hexutil::encode(&[7u8; 32]), "abcd").as_bytes()).is_err());
    }
}
