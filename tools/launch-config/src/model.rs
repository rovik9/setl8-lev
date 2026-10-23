//! The human-editable input (`config/launch-markets.toml`). Every field is required unless it has a default, unknown fields
//! are an error, so a typo cannot silently change the launch configuration.
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub meta: Meta,
    pub rules: Rules,
    pub ruleset: RuleSetIn,
    pub commission: Vec<CommissionIn>,
    pub market: Vec<MarketIn>,
    #[serde(default)]
    pub excluded: Vec<ExcludedIn>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub format: String,
    pub version: u32,
}

/// The constants of decisions 56 and 57 and of the placeholder rules (P19 part B rules 3 and 5).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Decision 57: longest freeze plus this many percent ...
    pub freeze_margin_pct: u64,
    /// ... rounded up to a multiple of this many seconds ...
    pub max_age_round_secs: u64,
    /// ... never below this many seconds.
    pub max_age_floor_secs: u64,
    /// Decision 56 go-live rule for the Hyperliquid oracle: median and 95th percentile of |oracle - mid| in bp.
    pub golive_median_bp: f64,
    pub golive_p95_bp: f64,
    /// Quantity placeholder: the smallest quantity worth about this many USD.
    pub order_notional_usd: u64,
    /// `max_notional_usd` placeholder: this account size (USD) times the class leverage ...
    pub cap_basis_tier_usd: u64,
    /// ... rounded down to 1, 2 or 5 times a power of ten, never above this ...
    pub cap_ceiling_usd: u64,
    /// ... or, for stocks (class 5), this.
    pub cap_ceiling_stock_usd: u64,
}

/// Rule set v1 (DESIGN 5.4): the launch values of decisions 13, 15, 17 and 26.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSetIn {
    pub version: u16,
    pub fee_bps: u16,
    pub tier_sizes_usd: Vec<u64>,
    pub tier_costs_usd: Vec<u64>,
    pub reset_ladder_bps: Vec<u16>,
    pub leverage_x: Vec<u16>,
    pub holding_fee_e6: Vec<u32>,
}

/// Commission per side, e6 of notional, for one class. `pending = true` while the number is a placeholder.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommissionIn {
    pub class: u8,
    pub e6: u32,
    pub pending: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketIn {
    /// The symbol the price proof carries (`BTC` on GMX, `xyz:AAPL` on Hyperliquid).
    pub symbol: String,
    /// Proof source byte: 2 GMX tickers, 4 Hyperliquid oracle.
    pub source: u8,
    pub class: u8,
    /// What the market is exposure to; one market per underlying.
    pub underlying: String,
    pub name: String,
    pub price_usd: f64,
    pub price_basis: String,
    /// Longest freeze (ms between two price changes) seen in the probe data, and where it comes from.
    pub freeze_ms: u64,
    pub freeze_basis: String,
    /// Go-live statistics of the Hyperliquid oracle (required for source 4).
    pub oracle_median_bp: Option<f64>,
    pub oracle_p95_bp: Option<f64>,
    /// Half-spread in e6 of the mid (rule set `spread_e6[market id]`) and whether it is still a placeholder.
    pub half_spread_e6: u16,
    pub spread_pending: bool,
    /// Why the half-spread is still flagged (decision 72: the owner's base is set, the oracle-noise floor check is pending); printed in launch.md.
    #[serde(default)]
    pub spread_note: String,
    /// Session variants (see `session.rs`); the generator takes their intersection.
    pub session_variants: Vec<Vec<String>>,
    pub session_note: String,
    /// Open questions about this market; printed in launch.md next to it.
    #[serde(default)]
    pub verify: Vec<String>,
}

/// A candidate that is NOT at launch, with the number that failed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedIn {
    pub symbol: String,
    /// 0 when no source applies.
    pub source: u8,
    /// `median`, `p95` and `freeze` are checked against the measured numbers; `decision-56`, `liquidity` and `not-at-launch`
    /// are policy reasons.
    pub rule: String,
    pub reason: String,
    pub median_bp: Option<f64>,
    pub p95_bp: Option<f64>,
    pub freeze_ms: Option<u64>,
}
