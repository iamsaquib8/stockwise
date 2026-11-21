//! Portfolio rebalancing engine: target weights, drift bands, and trade plans
//! that balance their cash, choose the lots they sell for tax, and count
//! what the trades cost.
//!
//! 1. **Targets** ([`resolve_targets`]): a target key is a symbol (held or
//!    not), a sector, or an asset class (`equity`, `debt`, `gold`,
//!    `international`, `cash`). A symbol target is carved out of its sector
//!    and asset-class targets, and a sector target out of its asset class,
//!    so nothing is counted twice. Keys that match nothing are reported.
//! 2. **Plans** ([`plan`]): a full rebalance (sells pay for buys, plus any new
//!    cash), buy-only investing of new cash, or a sell-only `raise` of a cash
//!    amount at the lowest tax. Orders are whole shares (fund units to three
//!    decimals); the leftover cash goes to the holdings furthest below target,
//!    and buys are trimmed by the same priority when cash runs short.
//!    Projected weights come from the rounded orders.
//! 3. **Tax and costs**: sells take losing lots first, then the lots with the
//!    lowest tax, and leave lots that turn long-term within
//!    [`LONG_TERM_SOON_DAYS`] for last (where the ledger allows choosing
//!    lots; Indian demat accounts sell first-in, first-out). Each sell line
//!    carries its short- and long-term gain and the tax it adds; each trade
//!    its statutory charges, brokerage and FX spread.
//! 4. **Drift** ([`drift_report`]): current against target per key, with
//!    the 5/25 band (5 points, or 25% of the target, whichever is smaller).
//!
//! The figures are arithmetic on the user's own targets, not advice.

use crate::tax::{self, Buckets, Country};
use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Small epsilon for floating-point comparisons.
const EPS: f64 = 1e-9;

/// The 5/25 rule's absolute band, in percentage points.
pub const ABSOLUTE_BAND_PCT: f64 = 5.0;
/// The 5/25 rule's relative band, in percent of the target.
pub const RELATIVE_BAND_PCT: f64 = 25.0;
/// A short-term lot at a gain this close to turning long-term is sold last.
pub const LONG_TERM_SOON_DAYS: i64 = crate::sellpreview::COUNTDOWN_DAYS;
/// For ordering sells only: the rate assumed on gains whose tax is not
/// estimated (taxed under other rules, such as an Indian resident's foreign
/// shares at the slab rate).
const OTHER_RULES_ORDER_RATE: f64 = 0.30;
/// Indian mutual-fund purchases pay 0.005% stamp duty.
const FUND_STAMP_PCT: f64 = 0.005;

/// Singular or plural noun for a whole-share order.
fn shares_word(shares: f64) -> &'static str {
    if (shares - 1.0).abs() < 1e-9 {
        "share"
    } else {
        "shares"
    }
}

fn is_zero(v: &f64) -> bool {
    v.abs() < EPS
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Asset class of a holding, for `equity` / `debt` / `gold` /
/// `international` / `cash` targets.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    #[default]
    Equity,
    Debt,
    Gold,
    International,
    Cash,
}

impl AssetClass {
    pub const ALL: [AssetClass; 5] = [
        AssetClass::Equity,
        AssetClass::Debt,
        AssetClass::Gold,
        AssetClass::International,
        AssetClass::Cash,
    ];

    /// The asset class a target key names, if any.
    pub fn parse(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "equity" | "equities" | "stocks" => Some(Self::Equity),
            "debt" | "bonds" | "fixed income" => Some(Self::Debt),
            "gold" | "commodity" | "commodities" | "precious metals" => Some(Self::Gold),
            "international" | "intl" | "foreign" | "global" | "overseas" => {
                Some(Self::International)
            }
            "cash" | "liquid" => Some(Self::Cash),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Equity => "equity",
            Self::Debt => "debt",
            Self::Gold => "gold",
            Self::International => "international",
            Self::Cash => "cash",
        }
    }
}

/// The asset class of a holding. `category` is a fund's scheme category (or
/// a known ETF's); `foreign` is true when the holding trades in another
/// currency than the portfolio's base. Uses the same look-through rules as
/// `portfolio xray`.
pub fn classify(symbol: &str, is_fund: bool, category: Option<&str>, foreign: bool) -> AssetClass {
    use crate::offmarket::{EtfKind, non_equity_etf};
    use crate::xray::{AssetClassType, classify_asset_class};
    if let Some(kind) = non_equity_etf(symbol) {
        return match kind {
            EtfKind::Gold | EtfKind::Silver => AssetClass::Gold,
            EtfKind::International => AssetClass::International,
            EtfKind::Liquid => AssetClass::Cash,
            EtfKind::Debt => AssetClass::Debt,
        };
    }
    let is_sgb = crate::offmarket::is_sgb(symbol);
    match classify_asset_class(symbol, is_fund, is_sgb, category) {
        AssetClassType::Commodity => AssetClass::Gold,
        AssetClassType::Debt => AssetClass::Debt,
        AssetClassType::Cash => AssetClass::Cash,
        AssetClassType::Equity => {
            let cat = category.unwrap_or("").to_ascii_lowercase();
            let overseas = ["international", "overseas", "global", "nasdaq", "s&p 500"]
                .iter()
                .any(|w| cat.contains(w));
            if foreign || overseas {
                AssetClass::International
            } else {
                AssetClass::Equity
            }
        }
    }
}

/// Which statutory charges a trade in the holding pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fees {
    /// None modelled (US and UK shares: SEC/FINRA fees are cents).
    #[default]
    None,
    /// An NSE/BSE delivery trade: STT, stamp duty, exchange, SEBI, GST, DP.
    IndiaDelivery,
    /// An Indian mutual fund: stamp duty on purchases.
    IndiaFund,
}

/// One open lot of a holding, as the planner sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LotInput {
    /// The lot's number in `portfolio lots` (1-based, oldest first).
    pub lot_no: usize,
    pub qty: f64,
    pub date: String,
    pub date_estimated: bool,
    /// Cost per unit for tax, in the base currency (India: the 31 Jan 2018
    /// price for grandfathered lots), purchase charges included.
    pub unit_cost: f64,
    /// Long-term if sold today.
    pub long_term: bool,
    /// The first long-term day, when not long-term yet.
    pub long_term_on: Option<NaiveDate>,
}

/// One holding's market data passed into the rebalancer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HoldingInput {
    pub symbol: String,
    pub shares: f64,
    /// Market value in the base currency.
    pub value: f64,
    /// Price per unit in the base currency.
    pub price: f64,
    pub sector: Option<String>,
    /// Fractional units (mutual funds).
    pub is_fund: bool,
    /// An exchange-traded fund: exempt from the single-stock limit.
    pub is_etf: bool,
    pub asset_class: AssetClass,
    /// Open lots, oldest first. Empty for a target not held yet.
    pub lots: Vec<LotInput>,
    /// Sales must take the oldest lots first (Indian demat accounts and
    /// mutual funds). Otherwise lots may be chosen (`portfolio sell --lots`).
    pub fifo_only: bool,
    /// Gains are taxed under the residence's listed-share rules, so the
    /// plan estimates their tax.
    pub taxed: bool,
    pub fees: Fees,
    /// Traded in another currency than the base: pays an FX spread.
    pub foreign: bool,
    /// Units of the holding's own currency per unit of base currency (0: 1).
    pub local_per_base: f64,
}

impl HoldingInput {
    fn held(&self) -> bool {
        self.shares > EPS
    }

    /// Whether the profile's single-stock limit applies: a share, not a
    /// fund, an ETF or a gold bond.
    fn single_stock(&self) -> bool {
        !self.is_fund
            && !self.is_etf
            && matches!(
                self.asset_class,
                AssetClass::Equity | AssetClass::International
            )
    }

    /// The smallest order: one share, or a thousandth of a fund unit.
    fn step(&self) -> f64 {
        if self.is_fund { 0.001 } else { 1.0 }
    }

    /// Whole orderable units in `qty` (rounded down).
    fn units_down(&self, qty: f64) -> f64 {
        if qty <= EPS {
            return 0.0;
        }
        let s = self.step();
        ((qty / s) + 1e-6).floor() * s
    }

    /// Orderable units covering `qty` (rounded up).
    fn units_up(&self, qty: f64) -> f64 {
        if qty <= EPS {
            return 0.0;
        }
        let s = self.step();
        ((qty / s) - 1e-6).ceil() * s
    }
}

/// Action classification for rebalancing orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebalanceActionType {
    Buy,
    Sell,
    #[default]
    Ok,
}

/// What a trade costs, in the base currency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TradeCosts {
    pub stt: f64,
    pub stamp: f64,
    /// Exchange transaction charge, SEBI fee and the GST on them.
    pub exchange: f64,
    pub brokerage: f64,
    pub dp: f64,
    pub fx_spread: f64,
    pub total: f64,
}

/// Shares a sell takes out of one lot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LotSlice {
    pub lot_no: usize,
    pub qty: f64,
    pub bought: String,
    pub unit_cost: f64,
    pub gain: f64,
    pub long_term: bool,
    /// The lot turns long-term on this day, within [`LONG_TERM_SOON_DAYS`]:
    /// waiting would tax the gain at the long-term rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_term_on: Option<String>,
}

/// One row in the rebalance plan.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RebalanceItem {
    pub symbol: String,
    pub current_value: f64,
    pub current_pct: f64,
    pub target_pct: f64,
    /// Value after the order, from the rounded share count.
    pub projected_value: f64,
    /// Weight after every order, of the portfolio plus the cash left in it.
    pub projected_pct: f64,
    pub price: f64,
    /// Value of the order: positive buys, negative sells.
    pub diff_value: f64,
    /// Shares or units in the order (always positive).
    pub diff_shares: f64,
    pub action_type: RebalanceActionType,
    pub action_display: String,
    pub is_fund: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sector: Option<String>,
    /// False for a target not held yet.
    #[serde(default)]
    pub held: bool,
    #[serde(default)]
    pub is_etf: bool,
    #[serde(default)]
    pub asset_class: AssetClass,
    /// Sells: the lots sold, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lots: Vec<LotSlice>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub st_gain: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lt_gain: f64,
    /// Tax the sell adds to this tax year's estimate (cess included);
    /// `None` when its gains are taxed under rules the plan does not model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tax: Option<f64>,
    #[serde(default)]
    pub costs: TradeCosts,
    /// The command that records the sale with these lots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Which kind of plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    /// Sells and buys back to target, plus any new cash.
    #[default]
    Rebalance,
    /// New cash only, never a sell.
    BuyOnly,
    /// Sells only, to raise a cash amount.
    Raise,
}

/// The tax the plan's sells add this tax year.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanTax {
    pub st_gain: f64,
    pub lt_gain: f64,
    /// Tax added, cess included.
    pub tax: f64,
    /// The same sells taken first-in, first-out, where that differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fifo_tax: Option<f64>,
    /// `fifo_tax - tax`: what choosing the lots saves.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub saved_vs_fifo: f64,
    /// Gains on holdings taxed under other rules (not in `tax`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub untaxed_gain: f64,
}

/// Complete rebalance recommendation plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RebalancePlan {
    pub total_value: f64,
    /// Holdings after the orders plus the cash left in the portfolio.
    pub projected_value: f64,
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inflow: Option<f64>,
    pub drift_tolerance: f64,
    pub items: Vec<RebalanceItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub mode: PlanMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raise: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cash_buffer: f64,
    #[serde(default)]
    pub sell_proceeds: f64,
    #[serde(default)]
    pub buy_cost: f64,
    /// All trade costs.
    #[serde(default)]
    pub costs: f64,
    /// Cash the trades free (positive) or need (negative), after costs.
    #[serde(default)]
    pub net_cash: f64,
    /// Cash left uninvested after the plan (new cash included).
    #[serde(default)]
    pub cash_left: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tax: Option<PlanTax>,
    /// Holdings left out (no price or exchange rate).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<String>,
    /// The single-stock limit the plan holds to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_stock_weight: Option<f64>,
}

impl RebalancePlan {
    pub fn empty(currency: impl Into<String>) -> Self {
        Self {
            total_value: 0.0,
            projected_value: 0.0,
            currency: currency.into(),
            inflow: None,
            drift_tolerance: 0.0,
            items: Vec::new(),
            warnings: Vec::new(),
            mode: PlanMode::Rebalance,
            raise: None,
            cash_buffer: 0.0,
            sell_proceeds: 0.0,
            buy_cost: 0.0,
            costs: 0.0,
            net_cash: 0.0,
            cash_left: 0.0,
            tax: None,
            excluded: Vec::new(),
            max_stock_weight: None,
        }
    }
}

/// The investor's tax position, for the tax a plan's sells add.
#[derive(Debug, Clone, Copy)]
pub struct TaxSetup {
    pub country: Country,
    /// Gains and losses realized this tax year (and losses brought forward),
    /// in the base currency.
    pub realized: Buckets,
}

/// How to build the plan.
#[derive(Debug, Clone)]
pub struct PlanOptions {
    /// New cash to invest.
    pub inflow: Option<f64>,
    /// With `inflow`: sell overweight holdings too (a full rebalance on the
    /// larger total) instead of investing the new cash only.
    pub with_sells: bool,
    /// Sell only, to raise this much cash (after costs).
    pub raise: Option<f64>,
    /// Cash to keep uninvested.
    pub cash_buffer: f64,
    pub drift_tolerance: f64,
    /// Hard limit on a single stock's weight, in percent.
    pub max_stock_weight: Option<f64>,
    /// Orders worth less than this are left out.
    pub min_trade: f64,
    /// Brokerage per share order, in the base currency.
    pub brokerage: f64,
    /// FX spread on trades in another currency, in percent.
    pub fx_spread_pct: f64,
    pub today: NaiveDate,
    pub tax: Option<TaxSetup>,
}

impl PlanOptions {
    pub fn new(today: NaiveDate) -> Self {
        Self {
            inflow: None,
            with_sells: false,
            raise: None,
            cash_buffer: 0.0,
            drift_tolerance: 0.0,
            max_stock_weight: None,
            min_trade: 0.0,
            brokerage: 0.0,
            fx_spread_pct: 0.0,
            today,
            tax: None,
        }
    }

    fn mode(&self) -> PlanMode {
        if self.raise.is_some() {
            PlanMode::Raise
        } else if self.inflow.is_some() && !self.with_sells {
            PlanMode::BuyOnly
        } else {
            PlanMode::Rebalance
        }
    }
}

/// Parse a target weight specification string: `KEY:PERCENT` or `KEY=PERCENT`
/// (e.g., `TCS.NS:25`, `Technology=30%`, `INFY: 20.5`).
pub fn parse_target(s: &str) -> Result<(String, f64)> {
    let s = s.trim();
    let (key, val) = s
        .split_once(':')
        .or_else(|| s.split_once('='))
        .context("Target must be in KEY:PERCENTAGE or KEY=PERCENTAGE format (e.g. TCS:25)")?;
    let key = key.trim().to_string();
    if key.is_empty() {
        bail!("Target key cannot be empty in '{}'", s);
    }
    let val_str = val.trim().trim_end_matches('%');
    let pct: f64 = val_str
        .parse()
        .with_context(|| format!("Invalid percentage '{}' in target '{}'", val.trim(), s))?;
    if !(0.0..=100.0).contains(&pct) {
        bail!(
            "Target percentage must be between 0 and 100, got {} in '{}'",
            pct,
            s
        );
    }
    Ok((key, pct))
}

/// Parse multiple comma-separated or space-separated target arguments.
pub fn parse_targets(inputs: &[String]) -> Result<Vec<(String, f64)>> {
    let mut out = Vec::new();
    for input in inputs {
        for part in input.split(',') {
            let part = part.trim();
            if !part.is_empty() {
                out.push(parse_target(part)?);
            }
        }
    }
    Ok(out)
}

/// Check if a target key matches a holding symbol (case-insensitive, bare or suffixed).
pub fn matches_symbol(target_key: &str, symbol: &str) -> bool {
    if target_key.eq_ignore_ascii_case(symbol) {
        return true;
    }
    // Match bare key e.g. "TCS" matches "TCS.NS" or "TCS.BO"
    if symbol
        .split_once('.')
        .is_some_and(|(bare, _)| target_key.eq_ignore_ascii_case(bare))
    {
        return true;
    }
    // Match if key has suffix and symbol is bare: "TCS.NS" matches "TCS"
    if target_key
        .split_once('.')
        .is_some_and(|(bare, _)| bare.eq_ignore_ascii_case(symbol))
    {
        return true;
    }
    false
}

/// What a target key was matched as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    Symbol,
    Sector,
    AssetClass,
}

/// One target key after matching: which holdings it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyGroup {
    pub key: String,
    pub kind: KeyKind,
    /// The configured percentage.
    pub configured_pct: f64,
    /// Indices into the holdings.
    pub members: Vec<usize>,
}

/// Target weights per holding, and what each key matched.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTargets {
    /// Percent per holding, summing to 100 (or 0 with no holdings).
    pub weights: Vec<f64>,
    pub groups: Vec<KeyGroup>,
    /// Keys that match no holding, sector or asset class.
    pub unmatched: Vec<String>,
    pub warnings: Vec<String>,
}

/// Spread `budget` over `members` in proportion to their value (equally
/// when they have none).
fn spread(holdings: &[HoldingInput], members: &[usize], budget: f64, out: &mut [Option<f64>]) {
    if members.is_empty() {
        return;
    }
    let total: f64 = members.iter().map(|&i| holdings[i].value.max(0.0)).sum();
    for &i in members {
        out[i] = Some(if total > EPS {
            budget * holdings[i].value.max(0.0) / total
        } else {
            budget / members.len() as f64
        });
    }
}

/// Resolve target weights for every holding.
///
/// 1. Symbol targets (held or not) fix that holding's weight.
/// 2. A sector target is shared by the sector's other holdings in
///    proportion to their value, after its symbol targets are carved out.
/// 3. An asset-class target is shared the same way, after the symbol and
///    sector targets inside the class are carved out.
/// 4. What is left of 100% goes equally to holdings no target covers; if
///    the targets do not add up to 100% they are scaled to it.
pub fn resolve_targets(
    holdings: &[HoldingInput],
    configured: &BTreeMap<String, f64>,
) -> ResolvedTargets {
    let mut out = ResolvedTargets {
        weights: Vec::new(),
        groups: Vec::new(),
        unmatched: Vec::new(),
        warnings: Vec::new(),
    };
    if holdings.is_empty() {
        out.unmatched = configured.keys().cloned().collect();
        return out;
    }

    // Classify every key.
    for (key, &pct) in configured {
        let symbol_members: Vec<usize> = holdings
            .iter()
            .enumerate()
            .filter(|(_, h)| matches_symbol(key, &h.symbol))
            .map(|(i, _)| i)
            .take(1)
            .collect();
        let (kind, members) = if !symbol_members.is_empty() {
            (KeyKind::Symbol, symbol_members)
        } else if let Some(class) = AssetClass::parse(key) {
            let m: Vec<usize> = holdings
                .iter()
                .enumerate()
                .filter(|(_, h)| h.asset_class == class)
                .map(|(i, _)| i)
                .collect();
            (KeyKind::AssetClass, m)
        } else {
            let m: Vec<usize> = holdings
                .iter()
                .enumerate()
                .filter(|(_, h)| {
                    h.sector
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(key.trim()))
                })
                .map(|(i, _)| i)
                .collect();
            (KeyKind::Sector, m)
        };
        if members.is_empty() {
            out.unmatched.push(key.clone());
            continue;
        }
        out.groups.push(KeyGroup {
            key: key.clone(),
            kind,
            configured_pct: pct,
            members,
        });
    }
    for key in &out.unmatched {
        out.warnings.push(if AssetClass::parse(key).is_some() {
            format!("Target '{key}' matches no holding in that asset class — ignored")
        } else {
            format!(
                "Target '{key}' matches no holding, sector or asset class — ignored \
                 (check the spelling, or `portfolio target remove {key}`)"
            )
        });
    }

    let n = holdings.len();
    let mut targets: Vec<Option<f64>> = vec![None; n];
    // 1. Symbols.
    for g in out.groups.iter().filter(|g| g.kind == KeyKind::Symbol) {
        for &i in &g.members {
            targets[i] = Some(g.configured_pct);
        }
    }
    // 2. Sectors, then 3. asset classes: carve out what is already set.
    for kind in [KeyKind::Sector, KeyKind::AssetClass] {
        for g in out.groups.iter().filter(|g| g.kind == kind) {
            let carved: f64 = g.members.iter().filter_map(|&i| targets[i]).sum();
            let free: Vec<usize> = g
                .members
                .iter()
                .copied()
                .filter(|&i| targets[i].is_none())
                .collect();
            let rest = g.configured_pct - carved;
            if rest < -0.01 {
                out.warnings.push(format!(
                    "Targets inside '{}' add up to {:.1}%, more than its {:.1}% target; \
                     they take precedence",
                    g.key, carved, g.configured_pct
                ));
            }
            if free.is_empty() {
                if rest > 0.01 {
                    out.warnings.push(format!(
                        "'{}' target {:.1}% is fully covered by narrower targets ({:.1}%); \
                         the other {:.1}% goes to holdings without a target",
                        g.key, g.configured_pct, carved, rest
                    ));
                }
                continue;
            }
            spread(holdings, &free, rest.max(0.0), &mut targets);
        }
    }

    // 4. The remainder.
    let unassigned: Vec<usize> = (0..n).filter(|&i| targets[i].is_none()).collect();
    let assigned_sum: f64 = targets.iter().flatten().sum();
    if unassigned.is_empty() {
        if (assigned_sum - 100.0).abs() > 0.01 && assigned_sum > EPS {
            out.warnings.push(format!(
                "Configured targets sum to {:.1}%; normalized to 100.0%",
                assigned_sum
            ));
            let factor = 100.0 / assigned_sum;
            for val in targets.iter_mut().flatten() {
                *val *= factor;
            }
        }
    } else if assigned_sum < 100.0 - EPS {
        let remaining = 100.0 - assigned_sum;
        let per_unassigned = remaining / unassigned.len() as f64;
        if assigned_sum > EPS {
            out.warnings.push(format!(
                "Configured targets sum to {:.1}%; remaining {:.1}% allocated equally among {} untargeted holding(s)",
                assigned_sum,
                remaining,
                unassigned.len()
            ));
        }
        for idx in unassigned {
            targets[idx] = Some(per_unassigned);
        }
    } else {
        if (assigned_sum - 100.0).abs() > 0.01 {
            out.warnings.push(format!(
                "Configured targets sum to {:.1}% (exceeds 100%); normalized among targeted holdings, {} untargeted set to 0.0%",
                assigned_sum,
                unassigned.len()
            ));
            let factor = 100.0 / assigned_sum;
            for val in targets.iter_mut().flatten() {
                *val *= factor;
            }
        } else {
            out.warnings.push(format!(
                "Configured targets sum to 100.0%; {} untargeted holding(s) set to 0.0% target",
                unassigned.len()
            ));
        }
        for idx in unassigned {
            targets[idx] = Some(0.0);
        }
    }

    let equal = 100.0 / n as f64;
    out.weights = targets.into_iter().map(|t| t.unwrap_or(equal)).collect();
    out
}

/// Resolve and normalize target weights for all holdings (see
/// [`resolve_targets`]): the weights and the warnings.
pub fn resolve_target_weights(
    holdings: &[HoldingInput],
    configured: &BTreeMap<String, f64>,
) -> (Vec<f64>, Vec<String>) {
    let r = resolve_targets(holdings, configured);
    (r.weights, r.warnings)
}

/// Hold single stocks to `cap` percent: the excess goes to the other
/// holdings in proportion to their targets. When nothing else can take it
/// (a portfolio of single stocks only, too few for the limit), the targets
/// stay as they are and a warning says so.
fn apply_cap(weights: &mut [f64], holdings: &[HoldingInput], cap: f64, warnings: &mut Vec<String>) {
    let mut w = weights.to_vec();
    let mut fixed = vec![false; w.len()];
    let mut capped: Vec<String> = Vec::new();
    for _ in 0..w.len() + 1 {
        let mut excess = 0.0;
        for (i, h) in holdings.iter().enumerate() {
            if h.single_stock() && w[i] > cap + 1e-6 {
                if !fixed[i] && weights[i] > cap + 1e-6 {
                    capped.push(format!("{} target {:.1}%", h.symbol, weights[i]));
                }
                excess += w[i] - cap;
                w[i] = cap;
                fixed[i] = true;
            }
        }
        if excess <= 1e-9 {
            break;
        }
        let takers: Vec<usize> = (0..w.len())
            .filter(|&i| !fixed[i] && (!holdings[i].single_stock() || w[i] < cap - 1e-9))
            .collect();
        if takers.is_empty() {
            warnings.push(format!(
                "The single-stock limit (max_stock_weight {cap:.1}%) can't hold: nothing else \
                 can take the other {excess:.1}%, so targets are left as they are"
            ));
            return;
        }
        let base: f64 = takers.iter().map(|&i| w[i]).sum();
        for &i in &takers {
            w[i] += if base > EPS {
                excess * w[i] / base
            } else {
                excess / takers.len() as f64
            };
        }
    }
    for c in capped {
        warnings.push(format!(
            "{c} is above the profile's single-stock limit (max_stock_weight {cap:.1}%): capped, \
             the excess goes to the other holdings"
        ));
    }
    weights.copy_from_slice(&w);
}

/// What trading `amount` (base currency) of `h` costs.
pub fn trade_costs(h: &HoldingInput, sell: bool, amount: f64, o: &PlanOptions) -> TradeCosts {
    let mut c = TradeCosts::default();
    if amount <= EPS {
        return c;
    }
    let to_local = if h.local_per_base > EPS {
        h.local_per_base
    } else {
        1.0
    };
    match h.fees {
        Fees::IndiaDelivery => {
            let e = crate::trade_charges::estimate_delivery(sell, amount * to_local, o.today);
            c.stt = e.stt / to_local;
            c.stamp = e.stamp / to_local;
            c.exchange = (e.exchange + e.sebi + e.gst) / to_local;
            c.dp = e.dp / to_local;
        }
        Fees::IndiaFund => {
            if !sell {
                c.stamp = amount * FUND_STAMP_PCT / 100.0;
            }
        }
        Fees::None => {}
    }
    if !h.is_fund {
        c.brokerage = o.brokerage;
    }
    if h.foreign {
        c.fx_spread = amount * o.fx_spread_pct / 100.0;
    }
    c.total = c.stt + c.stamp + c.exchange + c.brokerage + c.dp + c.fx_spread;
    c
}

/// A holding's orders while the plan is built.
#[derive(Debug, Clone, Default)]
struct Order {
    target_value: f64,
    buy: f64,
    sell: f64,
    /// Lots sold so far (lot index, qty), for raise mode.
    taken: Vec<(usize, f64)>,
}

impl Order {
    fn projected(&self, h: &HoldingInput) -> f64 {
        h.value + (self.buy - self.sell) * h.price
    }
}

fn buy_cost(h: &HoldingInput, qty: f64, o: &PlanOptions) -> f64 {
    let amount = qty * h.price;
    amount + trade_costs(h, false, amount, o).total
}

fn sell_net(h: &HoldingInput, qty: f64, o: &PlanOptions) -> f64 {
    let amount = qty * h.price;
    amount - trade_costs(h, true, amount, o).total
}

/// Whether `h` can hold `value` out of `total` under the single-stock limit.
fn under_cap(h: &HoldingInput, value: f64, total: f64, o: &PlanOptions) -> bool {
    match o.max_stock_weight {
        Some(cap) if h.single_stock() && total > EPS => value / total * 100.0 <= cap + 1e-6,
        _ => true,
    }
}

/// Cash still needed to pay for every buy.
fn shortfall(holdings: &[HoldingInput], orders: &[Order], cash: f64, o: &PlanOptions) -> f64 {
    let buys: f64 = holdings
        .iter()
        .zip(orders)
        .map(|(h, ord)| buy_cost(h, ord.buy, o))
        .sum();
    buys - cash
}

/// Remove buys, lowest priority first, until `cash` pays for them: the buy
/// whose last unit brings it least closer to its target goes first.
fn trim_buys(holdings: &[HoldingInput], orders: &mut [Order], cash: f64, o: &PlanOptions) {
    for _ in 0..10_000 {
        let need = shortfall(holdings, orders, cash, o);
        if need <= 1e-6 {
            return;
        }
        let pick = (0..holdings.len())
            .filter(|&i| orders[i].buy > EPS)
            .min_by(|&a, &b| {
                let score = |i: usize| {
                    let gap = orders[i].target_value - orders[i].projected(&holdings[i]);
                    2.0 * gap + holdings[i].price
                };
                score(a).total_cmp(&score(b))
            });
        let Some(i) = pick else { return };
        let h = &holdings[i];
        let units = h.units_up(need / h.price).max(h.step()).min(orders[i].buy);
        orders[i].buy = h.units_down(orders[i].buy - units);
    }
}

/// Spend the cash left on the holdings furthest below target, a share
/// count at a time, while a unit brings one closer to its target.
fn fill_buys(
    holdings: &[HoldingInput],
    orders: &mut [Order],
    cash: f64,
    total: f64,
    o: &PlanOptions,
) {
    for _ in 0..(holdings.len() * 4 + 16) {
        let left = -shortfall(holdings, orders, cash, o);
        if left <= EPS {
            return;
        }
        let mut best: Option<(usize, f64)> = None;
        for (i, h) in holdings.iter().enumerate() {
            let ord = &orders[i];
            if ord.sell > EPS || h.price <= EPS {
                continue;
            }
            let gap = ord.target_value - ord.projected(h);
            let half = if h.is_fund { 0.0 } else { h.price / 2.0 };
            if gap <= half + EPS {
                continue;
            }
            if ord.buy <= EPS && gap < o.min_trade {
                continue;
            }
            if best.is_none_or(|(_, g)| gap - half > g) {
                best = Some((i, gap - half));
            }
        }
        let Some((i, _)) = best else { return };
        let h = &holdings[i];
        let gap = orders[i].target_value - orders[i].projected(h);
        let mut add = if h.is_fund {
            h.units_down(gap / h.price)
        } else {
            (gap / h.price + 0.5).floor().max(1.0)
        };
        if !h.is_fund {
            add = add.min((left / h.price).floor().max(1.0));
        }
        // As many as the cash and the single-stock limit allow.
        let before = buy_cost(h, orders[i].buy, o);
        while add > EPS {
            let extra = buy_cost(h, orders[i].buy + add, o) - before;
            let value = orders[i].projected(h) + add * h.price;
            if extra <= left + 1e-6 && under_cap(h, value, total, o) {
                break;
            }
            add = if h.is_fund {
                h.units_down(add * 0.9_f64.min((left / extra.max(EPS)).max(0.0)))
            } else {
                add - 1.0
            };
        }
        if add <= EPS {
            // Nothing affordable here: drop it from this round.
            orders[i].target_value = orders[i].projected(h);
            continue;
        }
        orders[i].buy += add;
    }
}

/// Continuous new-cash split that minimizes the squared deviation from
/// target: `min Σ (v_i + c_i - T_i)²` with `c_i ≥ 0`, `Σ c_i = inflow`,
/// where `T_i` is the target value in the larger portfolio. The solution
/// is `c_i = max(0, T_i - v_i + θ)` for the level `θ` that spends the cash
/// (water-filling): the most underweight holdings fill first, and those
/// filling share each further rupee equally.
pub fn compute_inflow_allocation(
    holdings: &[HoldingInput],
    target_pcts: &[f64],
    inflow: f64,
) -> Result<Vec<f64>> {
    if inflow <= 0.0 {
        bail!("Inflow amount must be positive, got {:.2}", inflow);
    }
    if holdings.is_empty() {
        return Ok(Vec::new());
    }
    let total: f64 = holdings.iter().map(|h| h.value).sum::<f64>() + inflow;
    let gaps: Vec<f64> = holdings
        .iter()
        .zip(target_pcts)
        .map(|(h, &t)| total * t / 100.0 - h.value)
        .collect();
    let spent = |theta: f64| -> f64 { gaps.iter().map(|g| (g + theta).max(0.0)).sum() };
    let max_gap = gaps.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (mut lo, mut hi) = (-max_gap, inflow - max_gap.min(0.0) + 1.0);
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if spent(mid) < inflow {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let mut out: Vec<f64> = gaps.iter().map(|g| (g + hi).max(0.0)).collect();
    // Absorb the last rounding error in the largest allocation.
    let diff = inflow - out.iter().sum::<f64>();
    if let Some((i, _)) = out.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) {
        out[i] = (out[i] + diff).max(0.0);
    }
    Ok(out)
}

/// Tax that adding `sale` to `running` adds.
fn tax_added(setup: &TaxSetup, running: Buckets, sale: Buckets) -> f64 {
    tax::estimate(setup.country, running.plus(sale)).total()
        - tax::estimate(setup.country, running).total()
}

fn lot_soon(l: &LotInput, today: NaiveDate) -> Option<NaiveDate> {
    let on = l.long_term_on?;
    let days = (on - today).num_days();
    (!l.long_term && !l.date_estimated && (0..=LONG_TERM_SOON_DAYS).contains(&days)).then_some(on)
}

/// What selling `qty` from lot `li` of `h` costs in tax, per unit of
/// proceeds (negative for a loss), given what is realized so far. Lots at
/// a gain that turn long-term soon rank after everything else.
fn lot_rate(
    h: &HoldingInput,
    li: usize,
    qty: f64,
    running: Buckets,
    o: &PlanOptions,
) -> (bool, f64) {
    let l = &h.lots[li];
    let gain = qty * (h.price - l.unit_cost);
    let proceeds = (qty * h.price).max(EPS);
    let soon = gain > 0.0 && lot_soon(l, o.today).is_some();
    let rate = match (&o.tax, h.taxed) {
        (Some(setup), true) => {
            let mut b = Buckets::default();
            b.add(gain, l.long_term);
            let t = tax_added(setup, running, b);
            // A loss saves at least what it offsets; rank losses by size.
            if gain < 0.0 {
                t.min(0.0) / proceeds + gain / proceeds * 1e-3
            } else {
                t / proceeds
            }
        }
        _ => gain / proceeds * OTHER_RULES_ORDER_RATE,
    };
    (soon, rate)
}

/// The lots a sell of `qty` takes: oldest first where the ledger requires
/// it, else losses first, then the lowest tax, lots that turn long-term
/// soon last.
fn pick_lots(h: &HoldingInput, qty: f64, running: Buckets, o: &PlanOptions) -> Vec<(usize, f64)> {
    let mut left: Vec<f64> = h.lots.iter().map(|l| l.qty).collect();
    let mut out = Vec::new();
    let mut need = qty;
    let mut run = running;
    while need > 1e-9 {
        let pick = if h.fifo_only {
            left.iter().position(|q| *q > 1e-9)
        } else {
            (0..h.lots.len())
                .filter(|&i| left[i] > 1e-9)
                .min_by(|&a, &b| {
                    let ra = lot_rate(h, a, left[a].min(need), run, o);
                    let rb = lot_rate(h, b, left[b].min(need), run, o);
                    ra.0.cmp(&rb.0).then(ra.1.total_cmp(&rb.1))
                })
        };
        let Some(i) = pick else { break };
        let take = left[i].min(need);
        left[i] -= take;
        need -= take;
        let l = &h.lots[i];
        run.add(take * (h.price - l.unit_cost), l.long_term);
        out.push((i, take));
    }
    out
}

fn fifo_lots(h: &HoldingInput, qty: f64) -> Vec<(usize, f64)> {
    let mut need = qty;
    let mut out = Vec::new();
    for (i, l) in h.lots.iter().enumerate() {
        if need <= 1e-9 {
            break;
        }
        let take = l.qty.min(need);
        need -= take;
        out.push((i, take));
    }
    out
}

fn slices(h: &HoldingInput, taken: &[(usize, f64)], o: &PlanOptions) -> Vec<LotSlice> {
    // One slice per lot, in the order taken.
    let mut merged: Vec<(usize, f64)> = Vec::new();
    for &(i, q) in taken {
        match merged.iter_mut().find(|(j, _)| *j == i) {
            Some(m) => m.1 += q,
            None => merged.push((i, q)),
        }
    }
    merged
        .into_iter()
        .map(|(i, q)| {
            let l = &h.lots[i];
            let gain = q * (h.price - l.unit_cost);
            LotSlice {
                lot_no: l.lot_no,
                qty: q,
                bought: l.date.clone(),
                unit_cost: l.unit_cost,
                gain,
                long_term: l.long_term,
                long_term_on: if gain > 0.0 {
                    lot_soon(l, o.today).map(|d| d.to_string())
                } else {
                    None
                },
            }
        })
        .collect()
}

fn slice_buckets(slices: &[LotSlice]) -> Buckets {
    let mut b = Buckets::default();
    for s in slices {
        b.add(s.gain, s.long_term);
    }
    b
}

fn fmt_units(h: &HoldingInput, q: f64) -> String {
    if h.is_fund {
        format!("{q:.3} units")
    } else {
        format!("{q:.0} {}", shares_word(q))
    }
}

/// Build the full rebalance recommendation plan for a portfolio (the
/// options' defaults: no costs, no tax, no single-stock limit).
pub fn compute_rebalance_plan(
    holdings: &[HoldingInput],
    configured_targets: &BTreeMap<String, f64>,
    inflow: Option<f64>,
    drift_tolerance: f64,
    currency: &str,
) -> Result<RebalancePlan> {
    let mut o = PlanOptions::new(chrono::Local::now().date_naive());
    o.inflow = inflow;
    o.drift_tolerance = drift_tolerance;
    plan(holdings, configured_targets, &o, currency)
}

/// Build a rebalance plan: see the module docs for the modes.
pub fn plan(
    all: &[HoldingInput],
    configured: &BTreeMap<String, f64>,
    o: &PlanOptions,
    currency: &str,
) -> Result<RebalancePlan> {
    if let Some(cash) = o.inflow
        && !(cash > 0.0 && cash.is_finite())
    {
        bail!("Inflow amount must be positive, got {:.2}", cash);
    }
    if let Some(r) = o.raise {
        if !(r > 0.0 && r.is_finite()) {
            bail!("The amount to raise must be positive, got {:.2}", r);
        }
        if o.inflow.is_some() {
            bail!("--raise sells to raise cash; it can't be combined with --inflow");
        }
    }
    if o.cash_buffer < 0.0 || !o.cash_buffer.is_finite() {
        bail!("The cash buffer can't be negative");
    }
    let mode = o.mode();
    let mut plan = RebalancePlan::empty(currency);
    plan.mode = mode;
    plan.inflow = o.inflow;
    plan.raise = o.raise;
    plan.cash_buffer = o.cash_buffer;
    plan.drift_tolerance = o.drift_tolerance;
    plan.max_stock_weight = o.max_stock_weight;

    // Holdings without a usable price can't be weighed: leave them out.
    let holdings: Vec<HoldingInput> = all
        .iter()
        .filter(|h| {
            let ok = h.price > EPS && h.price.is_finite() && h.value.is_finite();
            if !ok && h.held() {
                plan.excluded.push(h.symbol.clone());
            }
            ok
        })
        .cloned()
        .collect();
    if !plan.excluded.is_empty() {
        plan.warnings.push(format!(
            "No price for {} — left out; weights are of the other holdings",
            plan.excluded.join(", ")
        ));
    }
    if holdings.is_empty() {
        return Ok(plan);
    }

    let resolved = resolve_targets(&holdings, configured);
    plan.warnings.extend(resolved.warnings.iter().cloned());
    let mut weights = resolved.weights;
    if let Some(cap) = o.max_stock_weight {
        apply_cap(&mut weights, &holdings, cap, &mut plan.warnings);
    }

    let total: f64 = holdings.iter().map(|h| h.value).sum();
    plan.total_value = total;
    let inflow = o.inflow.unwrap_or(0.0);
    let mut orders: Vec<Order> = vec![Order::default(); holdings.len()];
    let mut realized = o.tax.map(|t| t.realized).unwrap_or_default();

    // `kept`: the total the projected weights are measured against.
    let kept;
    match mode {
        PlanMode::Rebalance => {
            let invest = total + inflow - o.cash_buffer;
            if invest <= EPS {
                bail!("Nothing to invest after the cash buffer");
            }
            kept = total + inflow;
            for (i, h) in holdings.iter().enumerate() {
                orders[i].target_value = invest * weights[i] / 100.0;
                let cur_pct = h.value / kept * 100.0;
                if (cur_pct - weights[i]).abs() <= o.drift_tolerance + 1e-9 {
                    continue;
                }
                let want = orders[i].target_value - h.value;
                if want < 0.0 {
                    let mut q = h.units_down((-want / h.price).min(h.shares));
                    // Rounding down must not leave a stock above the limit, nor sell more than held.
                    if !under_cap(h, h.value - q * h.price, kept, o) {
                        q = h
                            .units_up((-want / h.price).min(h.shares))
                            .min(h.units_down(h.shares));
                    }
                    if q * h.price >= o.min_trade.max(EPS) {
                        orders[i].sell = q;
                    }
                } else {
                    let q = h.units_down(want / h.price);
                    if q * h.price >= o.min_trade.max(EPS) {
                        orders[i].buy = q;
                    }
                }
            }
            let sells = |orders: &[Order]| -> f64 {
                holdings
                    .iter()
                    .zip(orders)
                    .map(|(h, ord)| sell_net(h, ord.sell, o))
                    .sum()
            };
            let mut cash = inflow - o.cash_buffer + sells(&orders);
            // Short of cash: trim holdings over target inside the band first.
            loop {
                let need = shortfall(&holdings, &orders, cash, o);
                if need <= 1e-6 {
                    break;
                }
                let pick = (0..holdings.len())
                    .filter(|&i| {
                        let h = &holdings[i];
                        orders[i].buy <= EPS
                            && orders[i].projected(h) - orders[i].target_value >= h.price - 1e-9
                            && h.shares - orders[i].sell >= h.step()
                    })
                    .max_by(|&a, &b| {
                        let over =
                            |i: usize| orders[i].projected(&holdings[i]) - orders[i].target_value;
                        over(a).total_cmp(&over(b))
                    });
                let Some(j) = pick else { break };
                let h = &holdings[j];
                let over = orders[j].projected(h) - orders[j].target_value;
                let q = h
                    .units_up(need / h.price)
                    .min(h.units_down(over / h.price))
                    .min(h.shares - orders[j].sell)
                    .max(h.step());
                orders[j].sell += q;
                cash = inflow - o.cash_buffer + sells(&orders);
            }
            trim_buys(&holdings, &mut orders, cash, o);
            fill_buys(&holdings, &mut orders, cash, kept, o);
            // A sell that fell under the minimum after all is dropped.
            for (h, ord) in holdings.iter().zip(orders.iter_mut()) {
                if ord.buy > EPS && ord.buy * h.price < o.min_trade {
                    ord.buy = 0.0;
                }
            }
        }
        PlanMode::BuyOnly => {
            let invest = inflow - o.cash_buffer;
            if invest <= EPS {
                bail!("The cash buffer leaves none of the new cash to invest");
            }
            kept = total + inflow;
            let alloc = compute_inflow_allocation(&holdings, &weights, invest)?;
            for (i, h) in holdings.iter().enumerate() {
                orders[i].target_value = (total + invest) * weights[i] / 100.0;
                let q = h.units_down(alloc[i] / h.price);
                if q * h.price >= o.min_trade.max(EPS)
                    && under_cap(h, h.value + q * h.price, kept, o)
                {
                    orders[i].buy = q;
                }
            }
            trim_buys(&holdings, &mut orders, invest, o);
            fill_buys(&holdings, &mut orders, invest, kept, o);
        }
        PlanMode::Raise => {
            let goal = o.raise.unwrap_or(0.0) + o.cash_buffer;
            if goal > total + EPS {
                bail!(
                    "Can't raise {:.2}: the priced holdings are worth {:.2}",
                    goal,
                    total
                );
            }
            let after = total - goal;
            for (i, ord) in orders.iter_mut().enumerate() {
                ord.target_value = after * weights[i] / 100.0;
            }
            let mut left: Vec<Vec<f64>> = holdings
                .iter()
                .map(|h| h.lots.iter().map(|l| l.qty).collect())
                .collect();
            let mut running = realized;
            let raised = |orders: &[Order]| -> f64 {
                holdings
                    .iter()
                    .zip(orders)
                    .map(|(h, ord)| sell_net(h, ord.sell, o))
                    .sum()
            };
            // Lowest tax first; among equal tax, the most overweight.
            for _ in 0..100_000 {
                let need = goal - raised(&orders);
                if need <= 1e-6 {
                    break;
                }
                let mut best: Option<(usize, usize, f64, (bool, f64, f64))> = None;
                for (i, h) in holdings.iter().enumerate() {
                    let max_q = h.units_down(h.shares - orders[i].sell);
                    if max_q < h.step() - 1e-9 {
                        continue;
                    }
                    let cands: Vec<usize> = if h.fifo_only {
                        left[i].iter().position(|q| *q > 1e-9).into_iter().collect()
                    } else {
                        (0..h.lots.len()).filter(|&li| left[i][li] > 1e-9).collect()
                    };
                    for li in cands {
                        let q = left[i][li]
                            .min(max_q)
                            .min(h.units_up(need / h.price).max(h.step()));
                        let q = if h.is_fund { q } else { q.floor() };
                        if q <= 1e-9 {
                            continue;
                        }
                        let (soon, rate) = lot_rate(h, li, q, running, o);
                        let over =
                            -(orders[i].projected(h) - orders[i].target_value) / h.value.max(EPS);
                        // Rates within a hundredth of a percent tie.
                        let key = (soon, (rate * 1e4).round(), over);
                        let better = best.as_ref().is_none_or(|b| {
                            key.0
                                .cmp(&b.3.0)
                                .then(key.1.total_cmp(&b.3.1))
                                .then(key.2.total_cmp(&b.3.2))
                                .is_lt()
                        });
                        if better {
                            best = Some((i, li, q, key));
                        }
                    }
                }
                let Some((i, li, q, _)) = best else { break };
                let h = &holdings[i];
                left[i][li] -= q;
                orders[i].sell += q;
                orders[i].taken.push((li, q));
                let l = &h.lots[li];
                running.add(q * (h.price - l.unit_cost), l.long_term);
            }
            let got = raised(&orders);
            if got + 1e-6 < goal {
                plan.warnings.push(format!(
                    "Selling everything priced raises only {:.2} of the {:.2} asked",
                    got, goal
                ));
            }
            kept = total - o.raise.unwrap_or(0.0);
        }
    }

    // Lines, lots, tax and costs.
    let tax_setup = o.tax;
    let mut fifo_running = realized;
    let mut any_choice = false;
    let mut untaxed_gain = 0.0;
    let (mut st_sum, mut lt_sum) = (0.0, 0.0);
    for (i, h) in holdings.iter().enumerate() {
        let ord = &orders[i];
        let mut item = RebalanceItem {
            symbol: h.symbol.clone(),
            current_value: h.value,
            current_pct: if total > EPS {
                h.value / total * 100.0
            } else {
                0.0
            },
            target_pct: weights[i],
            projected_value: ord.projected(h),
            price: h.price,
            is_fund: h.is_fund,
            sector: h.sector.clone(),
            held: h.held(),
            is_etf: h.is_etf,
            asset_class: h.asset_class,
            ..Default::default()
        };
        if ord.sell > EPS {
            let taken = if ord.taken.is_empty() {
                pick_lots(h, ord.sell, realized, o)
            } else {
                ord.taken.clone()
            };
            item.lots = slices(h, &taken, o);
            let b = slice_buckets(&item.lots);
            item.st_gain = b.st_gain - b.st_loss;
            item.lt_gain = b.lt_gain - b.lt_loss;
            if h.taxed
                && let Some(setup) = &tax_setup
            {
                item.tax = Some(round2(tax_added(setup, realized, b)));
                realized = realized.plus(b);
                st_sum += item.st_gain;
                lt_sum += item.lt_gain;
                let fifo = slices(h, &fifo_lots(h, ord.sell), o);
                fifo_running = fifo_running.plus(slice_buckets(&fifo));
                if !h.fifo_only {
                    any_choice = true;
                }
            } else {
                untaxed_gain += item.st_gain + item.lt_gain;
            }
            let amount = ord.sell * h.price;
            item.costs = trade_costs(h, true, amount, o);
            item.diff_value = -amount;
            item.diff_shares = ord.sell;
            item.action_type = RebalanceActionType::Sell;
            item.action_display = format!("SELL {}", fmt_units(h, ord.sell));
            let lot_nos: Vec<String> = item.lots.iter().map(|s| s.lot_no.to_string()).collect();
            let in_fifo_order = item
                .lots
                .iter()
                .zip(&h.lots)
                .all(|(s, l)| s.lot_no == l.lot_no);
            item.record = Some(if h.fifo_only || in_fifo_order {
                format!("stockwise portfolio sell {} {}", h.symbol, ord.sell)
            } else {
                format!(
                    "stockwise portfolio sell {} {} --lots {}",
                    h.symbol,
                    ord.sell,
                    lot_nos.join(",")
                )
            });
            for s in item.lots.iter().filter(|s| s.long_term_on.is_some()) {
                item.notes.push(format!(
                    "lot {} turns long-term on {}: waiting taxes its gain at the long-term rate",
                    s.lot_no,
                    s.long_term_on.as_deref().unwrap_or_default()
                ));
            }
            plan.sell_proceeds += amount;
        } else if ord.buy > EPS {
            let amount = ord.buy * h.price;
            item.costs = trade_costs(h, false, amount, o);
            item.diff_value = amount;
            item.diff_shares = ord.buy;
            item.action_type = RebalanceActionType::Buy;
            item.action_display = format!("BUY {}", fmt_units(h, ord.buy));
            plan.buy_cost += amount;
        } else {
            let gap = ord.target_value - h.value;
            item.action_display = if mode != PlanMode::BuyOnly
                && gap < 0.0
                && -gap < h.price
                && (h.value / kept * 100.0 - weights[i]).abs() > o.drift_tolerance
            {
                format!(
                    "OK (< 1 sh over: {}{:.0})",
                    crate::market::currency_symbol(Some(currency)),
                    -gap
                )
            } else {
                "OK".to_string()
            };
        }
        plan.costs += item.costs.total;
        plan.items.push(item);
    }

    plan.net_cash = plan.sell_proceeds - plan.buy_cost - plan.costs;
    plan.cash_left = match mode {
        PlanMode::Raise => plan.net_cash - o.raise.unwrap_or(0.0),
        _ => inflow + plan.net_cash,
    };
    let invested: f64 = plan.items.iter().map(|it| it.projected_value).sum();
    plan.projected_value = invested + plan.cash_left.max(0.0);
    let denom = plan.projected_value.max(EPS);
    let _ = kept;
    for it in &mut plan.items {
        it.projected_pct = it.projected_value / denom * 100.0;
    }
    if let Some(cap) = o.max_stock_weight {
        for (h, it) in holdings.iter().zip(&plan.items) {
            if h.single_stock() && it.projected_pct > cap + 0.05 {
                plan.warnings.push(format!(
                    "{} stays at {:.1}% after the plan, above the single-stock limit of {:.1}%{}",
                    it.symbol,
                    it.projected_pct,
                    cap,
                    if mode == PlanMode::BuyOnly {
                        " (buy-only never sells)"
                    } else {
                        ""
                    }
                ));
            }
        }
    }
    if let Some(setup) = tax_setup
        && plan.sell_proceeds > EPS
    {
        let before = o.tax.map(|t| t.realized).unwrap_or_default();
        let tax_now = tax::estimate(setup.country, realized).total()
            - tax::estimate(setup.country, before).total();
        let fifo_tax = tax::estimate(setup.country, fifo_running).total()
            - tax::estimate(setup.country, before).total();
        let differs = any_choice && (fifo_tax - tax_now).abs() > 0.005;
        plan.tax = Some(PlanTax {
            st_gain: round2(st_sum),
            lt_gain: round2(lt_sum),
            tax: round2(tax_now),
            fifo_tax: differs.then_some(round2(fifo_tax)),
            saved_vs_fifo: if differs {
                round2(fifo_tax - tax_now)
            } else {
                0.0
            },
            untaxed_gain: round2(untaxed_gain),
        });
    } else if untaxed_gain.abs() > EPS {
        plan.tax = Some(PlanTax {
            untaxed_gain: round2(untaxed_gain),
            ..Default::default()
        });
    }
    Ok(plan)
}

/// One target key in the drift report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftRow {
    pub key: String,
    /// `symbol`, `sector`, `asset_class`, or `untargeted` for the holdings
    /// no target covers.
    pub kind: String,
    pub current_pct: f64,
    /// The normalized target (what `rebalance` aims for).
    pub target_pct: f64,
    /// `current_pct - target_pct`, in percentage points.
    pub drift_pct: f64,
    /// Drift in percent of the target; `None` for a zero target.
    pub relative_drift_pct: Option<f64>,
    /// The band: the absolute band or 25% of the target, whichever is smaller.
    pub band_pct: f64,
    /// `ok`, `over` or `under`.
    pub status: String,
}

/// `portfolio target drift`: current against target per key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    pub currency: String,
    pub total_value: f64,
    pub absolute_band_pct: f64,
    pub relative_band_pct: f64,
    pub rows: Vec<DriftRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<String>,
}

impl DriftReport {
    /// Rows outside their band.
    pub fn breaches(&self) -> impl Iterator<Item = &DriftRow> {
        self.rows.iter().filter(|r| r.status != "ok")
    }

    /// A one-line notice for the scheduled drift check, or `None` when every
    /// key is inside its band.
    pub fn notice(&self) -> Option<String> {
        let parts: Vec<String> = self
            .breaches()
            .map(|r| {
                format!(
                    "{} {:.1}% vs {:.1}% target ({:+.1} pts, band ±{:.1})",
                    r.key, r.current_pct, r.target_pct, r.drift_pct, r.band_pct
                )
            })
            .collect();
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "Portfolio drift: {} — see `stockwise portfolio rebalance`",
            parts.join("; ")
        ))
    }
}

/// The band for a target: `absolute` points or [`RELATIVE_BAND_PCT`] of
/// the target, whichever is smaller (the 5/25 rule).
pub fn band_for(target_pct: f64, absolute: f64) -> f64 {
    if target_pct > EPS {
        absolute.min(target_pct * RELATIVE_BAND_PCT / 100.0)
    } else {
        absolute
    }
}

/// Current against target for every target key, with the same normalized
/// targets `rebalance` uses.
pub fn drift_report(
    all: &[HoldingInput],
    configured: &BTreeMap<String, f64>,
    absolute_band: f64,
    currency: &str,
) -> DriftReport {
    let holdings: Vec<HoldingInput> = all
        .iter()
        .filter(|h| h.value.is_finite() && (h.value > EPS || !h.held()))
        .cloned()
        .collect();
    let excluded: Vec<String> = all
        .iter()
        .filter(|h| !(h.value.is_finite() && (h.value > EPS || !h.held())))
        .map(|h| h.symbol.clone())
        .collect();
    let resolved = resolve_targets(&holdings, configured);
    let total: f64 = holdings.iter().map(|h| h.value).sum();
    let pct = |v: f64| if total > EPS { v / total * 100.0 } else { 0.0 };
    let mut rows = Vec::new();
    let mut covered = vec![false; holdings.len()];
    let row = |key: String, kind: &str, members: &[usize]| {
        let current = pct(members.iter().map(|&i| holdings[i].value).sum());
        let target: f64 = members.iter().map(|&i| resolved.weights[i]).sum();
        let drift = current - target;
        let band = band_for(target, absolute_band);
        DriftRow {
            key,
            kind: kind.to_string(),
            current_pct: round2(current),
            target_pct: round2(target),
            drift_pct: round2(drift),
            relative_drift_pct: (target > EPS).then(|| round2(drift / target * 100.0)),
            band_pct: round2(band),
            status: if drift.abs() <= band + 1e-9 {
                "ok"
            } else if drift > 0.0 {
                "over"
            } else {
                "under"
            }
            .to_string(),
        }
    };
    for g in &resolved.groups {
        let kind = match g.kind {
            KeyKind::Symbol => "symbol",
            KeyKind::Sector => "sector",
            KeyKind::AssetClass => "asset_class",
        };
        for &i in &g.members {
            covered[i] = true;
        }
        rows.push(row(g.key.clone(), kind, &g.members));
    }
    let rest: Vec<usize> = (0..holdings.len()).filter(|&i| !covered[i]).collect();
    if !rest.is_empty() && !resolved.groups.is_empty() {
        rows.push(row("Other holdings".to_string(), "untargeted", &rest));
    }
    let mut warnings = resolved.warnings;
    if !excluded.is_empty() {
        warnings.push(format!(
            "No price for {} — left out; weights are of the other holdings",
            excluded.join(", ")
        ));
    }
    DriftReport {
        currency: currency.to_string(),
        total_value: round2(total),
        absolute_band_pct: absolute_band,
        relative_band_pct: RELATIVE_BAND_PCT,
        rows,
        warnings,
        excluded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_holding(symbol: &str, shares: f64, price: f64, sector: Option<&str>) -> HoldingInput {
        HoldingInput {
            symbol: symbol.to_string(),
            shares,
            value: shares * price,
            price,
            sector: sector.map(String::from),
            is_fund: symbol.starts_with("MF:"),
            ..Default::default()
        }
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn lot(no: usize, qty: f64, cost: f64, date: &str, today: NaiveDate) -> LotInput {
        let bought = day(date);
        let long = crate::tax::is_long_term(bought, today, 12);
        LotInput {
            lot_no: no,
            qty,
            date: date.into(),
            date_estimated: false,
            unit_cost: cost,
            long_term: long,
            long_term_on: (!long).then(|| crate::sellpreview::long_term_from(bought, 12).unwrap()),
        }
    }

    fn targets(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn parse_target_valid_formats() {
        assert_eq!(parse_target("TCS:25").unwrap(), ("TCS".to_string(), 25.0));
        assert_eq!(
            parse_target("RELIANCE.NS: 35.5%").unwrap(),
            ("RELIANCE.NS".to_string(), 35.5)
        );
        assert_eq!(
            parse_target("Technology=40").unwrap(),
            ("Technology".to_string(), 40.0)
        );
        assert_eq!(
            parse_target("Financial Services: 20%").unwrap(),
            ("Financial Services".to_string(), 20.0)
        );
    }

    #[test]
    fn parse_target_invalid_formats() {
        assert!(parse_target("TCS").is_err());
        assert!(parse_target("TCS:105").is_err());
        assert!(parse_target("TCS:-5").is_err());
        assert!(parse_target(":25").is_err());
        assert!(parse_target("TCS:abc").is_err());
    }

    #[test]
    fn parse_targets_multiple_inputs() {
        let inputs = vec!["TCS:20, INFY:30".to_string(), "RELIANCE:50".to_string()];
        let res = parse_targets(&inputs).unwrap();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0], ("TCS".to_string(), 20.0));
        assert_eq!(res[1], ("INFY".to_string(), 30.0));
        assert_eq!(res[2], ("RELIANCE".to_string(), 50.0));
    }

    #[test]
    fn target_normalization_when_sum_differs_from_100() {
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 3000.0, Some("Technology")),
            sample_holding("INFY.NS", 20.0, 1500.0, Some("Technology")),
        ];
        let targets = targets(&[("TCS.NS", 60.0), ("INFY.NS", 60.0)]);
        let (resolved, warnings) = resolve_target_weights(&holdings, &targets);
        assert_eq!(resolved.len(), 2);
        assert!((resolved[0] - 50.0).abs() < 1e-6);
        assert!((resolved[1] - 50.0).abs() < 1e-6);
        assert!(warnings[0].contains("normalized to 100.0%"));
    }

    #[test]
    fn partial_targets_fill_untargeted_holdings_equally() {
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 3000.0, Some("Technology")),
            sample_holding("INFY.NS", 20.0, 1500.0, Some("Technology")),
            sample_holding("HDFCBANK.NS", 30.0, 1500.0, Some("Financial Services")),
        ];
        let targets = targets(&[("TCS.NS", 50.0)]);
        let (resolved, warnings) = resolve_target_weights(&holdings, &targets);
        assert!((resolved[0] - 50.0).abs() < 1e-6);
        assert!((resolved[1] - 25.0).abs() < 1e-6);
        assert!((resolved[2] - 25.0).abs() < 1e-6);
        assert!(warnings[0].contains("allocated equally"));
    }

    #[test]
    fn sector_target_distributed_pro_rata() {
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 1000.0, Some("Technology")),
            sample_holding("INFY.NS", 30.0, 1000.0, Some("Technology")),
            sample_holding("RELIANCE.NS", 20.0, 2000.0, Some("Energy")),
        ];
        let targets = targets(&[("Technology", 40.0), ("Energy", 60.0)]);
        let (resolved, _) = resolve_target_weights(&holdings, &targets);
        assert!((resolved[0] - 10.0).abs() < 1e-6);
        assert!((resolved[1] - 30.0).abs() < 1e-6);
        assert!((resolved[2] - 60.0).abs() < 1e-6);
    }

    #[test]
    fn a_symbol_target_is_carved_out_of_its_sector() {
        // Technology 40% with TCS 15% inside it: INFY gets the other 25%,
        // not 40% on top of TCS's 15%.
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 1000.0, Some("Technology")),
            sample_holding("INFY.NS", 30.0, 1000.0, Some("Technology")),
            sample_holding("RELIANCE.NS", 20.0, 2000.0, Some("Energy")),
        ];
        let t = targets(&[("Technology", 40.0), ("TCS", 15.0), ("Energy", 60.0)]);
        let r = resolve_targets(&holdings, &t);
        assert!((r.weights[0] - 15.0).abs() < 1e-6);
        assert!((r.weights[1] - 25.0).abs() < 1e-6);
        assert!((r.weights[2] - 60.0).abs() < 1e-6);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn asset_class_targets_use_the_holdings_class() {
        let mut gold = sample_holding("GOLDBEES.NS", 100.0, 50.0, None);
        gold.asset_class = AssetClass::Gold;
        gold.is_etf = true;
        let mut us = sample_holding("AAPL", 10.0, 1000.0, Some("Technology"));
        us.asset_class = AssetClass::International;
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 1000.0, Some("Technology")),
            sample_holding("HDFCBANK.NS", 10.0, 1000.0, Some("Financial Services")),
            gold,
            us,
        ];
        // equity 70 with TCS 30 inside: HDFC gets 40.
        let t = targets(&[
            ("equity", 70.0),
            ("TCS", 30.0),
            ("gold", 10.0),
            ("international", 20.0),
        ]);
        let r = resolve_targets(&holdings, &t);
        assert!((r.weights[0] - 30.0).abs() < 1e-6);
        assert!((r.weights[1] - 40.0).abs() < 1e-6);
        assert!((r.weights[2] - 10.0).abs() < 1e-6);
        assert!((r.weights[3] - 20.0).abs() < 1e-6);
    }

    #[test]
    fn an_unmatched_target_warns() {
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 1000.0, Some("Technology")),
            sample_holding("INFY.NS", 10.0, 1000.0, Some("Technology")),
        ];
        let t = targets(&[("Tecnology", 50.0), ("debt", 10.0)]);
        let r = resolve_targets(&holdings, &t);
        assert_eq!(
            r.unmatched,
            vec!["Tecnology".to_string(), "debt".to_string()]
        );
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("'Tecnology' matches no holding"))
        );
        assert!(r.warnings.iter().any(|w| w.contains("'debt'")));
        // Ignored: equal weights.
        assert!((r.weights[0] - 50.0).abs() < 1e-6);
        let plan = compute_rebalance_plan(&holdings, &t, None, 0.0, "INR").unwrap();
        assert!(plan.warnings.iter().any(|w| w.contains("Tecnology")));
    }

    #[test]
    fn an_unheld_target_buys() {
        // NEW.NS is not held: a 20% target gives a buy line paid for by sells.
        let mut new = sample_holding("NEW.NS", 0.0, 500.0, None);
        new.value = 0.0;
        let holdings = vec![
            sample_holding("TCS.NS", 50.0, 1000.0, None),
            sample_holding("INFY.NS", 50.0, 1000.0, None),
            new,
        ];
        let t = targets(&[("TCS", 40.0), ("INFY", 40.0), ("NEW", 20.0)]);
        let plan = compute_rebalance_plan(&holdings, &t, None, 0.0, "INR").unwrap();
        let it = &plan.items[2];
        assert!(!it.held);
        assert_eq!(it.action_type, RebalanceActionType::Buy);
        assert_eq!(it.diff_shares, 40.0);
        assert_eq!(plan.items[0].diff_shares, 10.0);
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Sell);
        assert!(plan.cash_left.abs() < 1e-6);
    }

    #[test]
    fn cash_balances_to_within_one_share() {
        let holdings = vec![
            sample_holding("A", 37.0, 913.0, None),
            sample_holding("B", 11.0, 2771.0, None),
            sample_holding("C", 260.0, 97.5, None),
            sample_holding("D", 3.0, 4410.0, None),
        ];
        let t = targets(&[("A", 25.0), ("B", 25.0), ("C", 25.0), ("D", 25.0)]);
        let mut o = PlanOptions::new(day("2026-11-21"));
        o.inflow = Some(10_000.0);
        o.with_sells = true;
        let plan = super::plan(&holdings, &t, &o, "INR").unwrap();
        let max_buy_price = plan
            .items
            .iter()
            .filter(|i| i.target_pct > 0.0)
            .map(|i| i.price)
            .fold(0.0, f64::max);
        assert!(plan.cash_left >= -1e-6, "overspent: {}", plan.cash_left);
        assert!(
            plan.cash_left < max_buy_price,
            "left {} uninvested",
            plan.cash_left
        );
        // Sells + new cash pay for the buys.
        assert!((plan.sell_proceeds + 10_000.0 - plan.buy_cost - plan.cash_left).abs() < 1e-6);
        // Same without new cash.
        let mut o = PlanOptions::new(day("2026-11-21"));
        o.drift_tolerance = 0.0;
        let p2 = super::plan(&holdings, &t, &o, "INR").unwrap();
        assert!(p2.cash_left >= -1e-6 && p2.cash_left < max_buy_price);
    }

    #[test]
    fn projected_weights_come_from_the_rounded_orders() {
        let holdings = vec![
            sample_holding("A", 10.0, 3333.0, None),
            sample_holding("B", 100.0, 77.0, None),
        ];
        let t = targets(&[("A", 50.0), ("B", 50.0)]);
        let plan = compute_rebalance_plan(&holdings, &t, None, 0.0, "INR").unwrap();
        let total = plan.projected_value;
        for it in &plan.items {
            let shares = holdings
                .iter()
                .find(|h| h.symbol == it.symbol)
                .unwrap()
                .shares;
            let signed = match it.action_type {
                RebalanceActionType::Buy => it.diff_shares,
                RebalanceActionType::Sell => -it.diff_shares,
                RebalanceActionType::Ok => 0.0,
            };
            let value = (shares + signed) * it.price;
            assert!((it.projected_value - value).abs() < 1e-6);
            assert!((it.projected_pct - value / total * 100.0).abs() < 1e-9);
        }
        // Not simply the target.
        assert!((plan.items[0].projected_pct - 50.0).abs() > 0.01);
        let sum: f64 = plan.items.iter().map(|i| i.projected_pct).sum::<f64>()
            + plan.cash_left / total * 100.0;
        assert!((sum - 100.0).abs() < 1e-6);
    }

    #[test]
    fn an_etf_is_exempt_from_the_single_stock_limit() {
        let mut etf = sample_holding("NIFTYBEES.NS", 100.0, 250.0, None);
        etf.is_etf = true;
        let holdings = vec![
            etf,
            sample_holding("TCS.NS", 25.0, 1000.0, None),
            sample_holding("INFY.NS", 25.0, 1000.0, None),
            sample_holding("HDFC.NS", 25.0, 1000.0, None),
        ];
        let t = targets(&[
            ("NIFTYBEES", 70.0),
            ("TCS", 10.0),
            ("INFY", 10.0),
            ("HDFC", 10.0),
        ]);
        let mut o = PlanOptions::new(day("2026-11-21"));
        o.max_stock_weight = Some(25.0);
        let plan = plan(&holdings, &t, &o, "INR").unwrap();
        assert!((plan.items[0].target_pct - 70.0).abs() < 1e-6);
        assert!(!plan.warnings.iter().any(|w| w.contains("NIFTYBEES")));
        // A stock over the limit is capped.
        let t2 = targets(&[("TCS", 70.0)]);
        let p2 = super::plan(&holdings, &t2, &o, "INR").unwrap();
        assert!((p2.items[1].target_pct - 25.0).abs() < 1e-6);
        assert!(p2.items[1].projected_pct <= 25.0 + 1e-6);
        assert!(
            p2.warnings
                .iter()
                .any(|w| w.contains("TCS.NS target 70.0%"))
        );
    }

    #[test]
    fn raise_sells_the_lowest_tax_lots() {
        // US resident (lots can be chosen). Today 2026-11-21.
        let today = day("2026-11-21");
        // A: lot 1 long-term gain, lot 2 short-term loss. B: short-term gain.
        let a = HoldingInput {
            symbol: "AAA".into(),
            shares: 20.0,
            value: 20.0 * 100.0,
            price: 100.0,
            lots: vec![
                lot(1, 10.0, 40.0, "2020-01-02", today),
                lot(2, 10.0, 130.0, "2026-06-01", today),
            ],
            taxed: true,
            ..Default::default()
        };
        let b = HoldingInput {
            symbol: "BBB".into(),
            shares: 20.0,
            value: 2000.0,
            price: 100.0,
            lots: vec![lot(1, 20.0, 50.0, "2026-03-01", today)],
            taxed: true,
            ..Default::default()
        };
        let holdings = vec![a, b];
        let mut o = PlanOptions::new(today);
        o.raise = Some(1000.0);
        o.tax = Some(TaxSetup {
            country: Country::Us,
            realized: Buckets::default(),
        });
        let plan = plan(&holdings, &BTreeMap::new(), &o, "USD").unwrap();
        assert_eq!(plan.mode, PlanMode::Raise);
        // The losing lot (AAA lot 2) goes first: 10 × 100 = 1,000.
        let a = &plan.items[0];
        assert_eq!(a.action_type, RebalanceActionType::Sell);
        assert_eq!(a.diff_shares, 10.0);
        assert_eq!(a.lots.len(), 1);
        assert_eq!(a.lots[0].lot_no, 2);
        assert!(a.st_gain < 0.0);
        assert_eq!(
            a.record.as_deref(),
            Some("stockwise portfolio sell AAA 10 --lots 2")
        );
        assert_eq!(plan.items[1].action_type, RebalanceActionType::Ok);
        let tax = plan.tax.unwrap();
        assert!(tax.tax <= 0.0);
        // FIFO would have sold lot 1 (a $600 long-term gain).
        assert!(tax.saved_vs_fifo > 0.0);
        assert!(plan.net_cash >= 1000.0 - 1e-6);
    }

    #[test]
    fn raise_with_fifo_lots_prefers_the_cheaper_holding() {
        // India: FIFO within a holding, so the choice is between holdings.
        let today = day("2026-11-21");
        let mk = |sym: &str, cost: f64, date: &str| HoldingInput {
            symbol: sym.into(),
            shares: 10.0,
            value: 1000.0,
            price: 100.0,
            lots: vec![lot(1, 10.0, cost, date, today)],
            taxed: true,
            fifo_only: true,
            ..Default::default()
        };
        // High short-term gain vs long-term gain inside the exemption.
        let holdings = vec![
            mk("STG.NS", 20.0, "2026-05-01"),
            mk("LTG.NS", 20.0, "2021-05-01"),
        ];
        let mut o = PlanOptions::new(today);
        o.raise = Some(500.0);
        o.tax = Some(TaxSetup {
            country: Country::India,
            realized: Buckets::default(),
        });
        let plan = plan(&holdings, &BTreeMap::new(), &o, "INR").unwrap();
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Ok);
        assert_eq!(plan.items[1].action_type, RebalanceActionType::Sell);
        assert_eq!(plan.items[1].diff_shares, 5.0);
        assert_eq!(plan.tax.unwrap().tax, 0.0);
    }

    #[test]
    fn a_lot_turning_long_term_soon_is_sold_last() {
        let today = day("2026-11-21");
        // Lot 1 turns long-term on 2026-12-02 (11 days); lot 2 is a larger
        // short-term gain with months to go, but waiting on lot 1 saves more.
        let h = HoldingInput {
            symbol: "AAA".into(),
            shares: 20.0,
            value: 2000.0,
            price: 100.0,
            lots: vec![
                lot(1, 10.0, 50.0, "2025-12-01", today),
                lot(2, 10.0, 30.0, "2026-06-01", today),
            ],
            taxed: true,
            ..Default::default()
        };
        let mut o = PlanOptions::new(today);
        o.raise = Some(500.0);
        o.tax = Some(TaxSetup {
            country: Country::Us,
            realized: Buckets::default(),
        });
        let plan = plan(&[h], &BTreeMap::new(), &o, "USD").unwrap();
        assert_eq!(plan.items[0].lots[0].lot_no, 2);
    }

    #[test]
    fn unpriced_holdings_are_left_out_with_a_warning() {
        let mut gone = sample_holding("GONE.NS", 10.0, 0.0, None);
        gone.value = 0.0;
        let holdings = vec![
            sample_holding("A", 10.0, 100.0, None),
            sample_holding("B", 30.0, 100.0, None),
            gone,
        ];
        let plan = compute_rebalance_plan(&holdings, &BTreeMap::new(), None, 0.0, "INR").unwrap();
        assert_eq!(plan.excluded, vec!["GONE.NS".to_string()]);
        assert_eq!(plan.items.len(), 2);
        assert!(plan.warnings.iter().any(|w| w.contains("GONE.NS")));
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Buy);
    }

    #[test]
    fn india_costs_are_counted() {
        let mut a = sample_holding("TCS.NS", 100.0, 1000.0, None);
        a.fees = Fees::IndiaDelivery;
        let mut b = sample_holding("INFY.NS", 50.0, 1000.0, None);
        b.fees = Fees::IndiaDelivery;
        let mut o = PlanOptions::new(day("2026-11-21"));
        o.brokerage = 20.0;
        let plan = plan(&[a, b], &BTreeMap::new(), &o, "INR").unwrap();
        let sell = &plan.items[0];
        assert_eq!(sell.action_type, RebalanceActionType::Sell);
        // STT 0.1% of 25,000 = 25, plus DP and brokerage.
        assert!((sell.costs.stt - 25.0).abs() < 0.01);
        assert!(sell.costs.dp > 0.0);
        assert_eq!(sell.costs.brokerage, 20.0);
        assert!(plan.items[1].costs.stamp > 0.0);
        assert!(plan.costs > 0.0);
        assert!(plan.cash_left >= -1e-6);
    }

    #[test]
    fn min_trade_drops_small_orders() {
        let holdings = vec![
            sample_holding("A", 52.0, 100.0, None),
            sample_holding("B", 48.0, 100.0, None),
        ];
        let mut o = PlanOptions::new(day("2026-11-21"));
        o.min_trade = 500.0;
        let plan = plan(&holdings, &BTreeMap::new(), &o, "INR").unwrap();
        assert!(
            plan.items
                .iter()
                .all(|i| i.action_type == RebalanceActionType::Ok)
        );
    }

    #[test]
    fn drift_report_uses_the_5_25_band() {
        // Technology 47% vs 40 (band 5): over. Gold 9% vs 8 (band 2): ok.
        let mut gold = sample_holding("GOLDBEES.NS", 9.0, 10.0, None);
        gold.asset_class = AssetClass::Gold;
        let holdings = vec![
            sample_holding("TCS.NS", 47.0, 10.0, Some("Technology")),
            sample_holding("HDFC.NS", 44.0, 10.0, Some("Financials")),
            gold,
        ];
        let t = targets(&[("Technology", 40.0), ("gold", 8.0)]);
        let r = drift_report(&holdings, &t, ABSOLUTE_BAND_PCT, "INR");
        let tech = r.rows.iter().find(|r| r.key == "Technology").unwrap();
        assert_eq!(tech.current_pct, 47.0);
        assert_eq!(tech.status, "over");
        let gold = r.rows.iter().find(|r| r.key == "gold").unwrap();
        assert_eq!(gold.band_pct, 2.0);
        assert_eq!(gold.status, "ok");
        let notice = r.notice().unwrap();
        assert!(notice.contains("Technology"));
        assert!(!notice.contains("gold"));
        let other = r.rows.iter().find(|r| r.kind == "untargeted").unwrap();
        assert_eq!(other.target_pct, 52.0);
    }

    #[test]
    fn drift_tolerance_filters_small_movements() {
        let holdings = vec![
            sample_holding("STOCK_A", 100.0, 51.0, None),
            sample_holding("STOCK_B", 100.0, 49.0, None),
        ];
        let targets = targets(&[("STOCK_A", 50.0), ("STOCK_B", 50.0)]);
        let plan = compute_rebalance_plan(&holdings, &targets, None, 2.0, "INR").unwrap();
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Ok);
        assert_eq!(plan.items[1].action_type, RebalanceActionType::Ok);
        let plan_strict = compute_rebalance_plan(&holdings, &targets, None, 0.5, "INR").unwrap();
        assert_eq!(plan_strict.items[0].action_type, RebalanceActionType::Sell);
        assert_eq!(plan_strict.items[1].action_type, RebalanceActionType::Buy);
    }

    #[test]
    fn inflow_only_never_issues_sell_recommendations() {
        let holdings = vec![
            sample_holding("HEAVILY_OVERWEIGHT", 80.0, 1000.0, None),
            sample_holding("UNDERWEIGHT", 20.0, 1000.0, None),
        ];
        let targets = targets(&[("HEAVILY_OVERWEIGHT", 50.0), ("UNDERWEIGHT", 50.0)]);
        let plan = compute_rebalance_plan(&holdings, &targets, Some(30_000.0), 2.0, "INR").unwrap();
        assert_eq!(plan.projected_value, 130_000.0);
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Ok);
        assert_eq!(plan.items[0].diff_shares, 0.0);
        assert_eq!(plan.items[1].action_type, RebalanceActionType::Buy);
        assert_eq!(plan.items[1].diff_shares, 30.0);
        assert!((plan.items[1].diff_value - 30_000.0).abs() < 1e-4);
    }

    #[test]
    fn inflow_water_filling_multiple_underweight() {
        // T = 35,000 each; c = max(0, T - v + θ): A 25k + θ, B 15k + θ, C 0.
        // 40k + 2θ = 15k gives θ = -12.5k: A 12.5k, B 2.5k.
        let holdings = vec![
            sample_holding("A", 10.0, 1000.0, None),
            sample_holding("B", 20.0, 1000.0, None),
            sample_holding("C", 60.0, 1000.0, None),
        ];
        let allocs = compute_inflow_allocation(
            &holdings,
            &[33.33333333, 33.33333333, 33.33333333],
            15_000.0,
        )
        .unwrap();
        assert!((allocs[0] - 12_500.0).abs() < 1.0);
        assert!((allocs[1] - 2_500.0).abs() < 1.0);
        assert_eq!(allocs[2], 0.0);
        assert!((allocs.iter().sum::<f64>() - 15_000.0).abs() < 1e-6);
    }

    #[test]
    fn inflow_split_minimizes_squared_deviation_with_unequal_targets() {
        // Targets 60/40 of 110k = 66k / 44k; v = 50k / 50k; inflow 10k.
        // A is 16k short, B 6k over: all 10k to A (not pro-rata to targets).
        let holdings = vec![
            sample_holding("A", 50.0, 1000.0, None),
            sample_holding("B", 50.0, 1000.0, None),
        ];
        let a = compute_inflow_allocation(&holdings, &[60.0, 40.0], 10_000.0).unwrap();
        assert!((a[0] - 10_000.0).abs() < 1e-6);
        assert!(a[1].abs() < 1e-6);
        // Two short holdings share each further rupee equally: gaps 30k and
        // 10k, inflow 30k: A 25k, B 5k (θ = -5k), not 22.5k / 7.5k.
        let holdings = vec![
            sample_holding("A", 30.0, 1000.0, None),
            sample_holding("B", 50.0, 1000.0, None),
            sample_holding("C", 70.0, 1000.0, None),
        ];
        // Total 180k: T = 60k each -> gaps A 30k, B 10k, C -10k.
        let a = compute_inflow_allocation(&holdings, &[100.0 / 3.0; 3], 30_000.0).unwrap();
        assert!((a[0] - 25_000.0).abs() < 1.0, "{a:?}");
        assert!((a[1] - 5_000.0).abs() < 1.0, "{a:?}");
        assert!(a[2].abs() < 1e-6);
    }

    #[test]
    fn mutual_fund_fractional_units() {
        let holdings = vec![sample_holding("MF:122639", 100.0, 52.345, None)];
        let targets = BTreeMap::new();
        let plan = compute_rebalance_plan(&holdings, &targets, Some(5000.0), 0.0, "INR").unwrap();
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Buy);
        assert!(plan.items[0].is_fund);
        assert!(plan.items[0].action_display.contains("units"));
        assert!((plan.items[0].diff_shares - (5000.0 / 52.345)).abs() < 1e-3);
    }

    #[test]
    fn overweight_under_one_share_reports_ok_not_a_zero_share_sell() {
        let holdings = vec![
            sample_holding("OVERWEIGHT", 2.0, 100.0, None),
            sample_holding("BASE", 10.0, 100.0, None),
        ];
        let targets = targets(&[("OVERWEIGHT", 10.0), ("BASE", 90.0)]);
        let plan = compute_rebalance_plan(&holdings, &targets, None, 2.0, "INR").unwrap();
        let item = &plan.items[0];
        assert_eq!(item.action_type, RebalanceActionType::Ok);
        assert_eq!(item.diff_shares, 0.0);
        assert!(!item.action_display.contains("SELL"));
    }

    #[test]
    fn single_share_orders_use_the_singular_noun() {
        let holdings = vec![
            sample_holding("ONE", 4.0, 100.0, None),
            sample_holding("BASE", 16.0, 100.0, None),
        ];
        let sell_targets = targets(&[("ONE", 15.0), ("BASE", 85.0)]);
        let sell = compute_rebalance_plan(&holdings, &sell_targets, None, 2.0, "INR").unwrap();
        assert_eq!(sell.items[0].action_type, RebalanceActionType::Sell);
        assert_eq!(sell.items[0].diff_shares, 1.0);
        assert_eq!(sell.items[0].action_display, "SELL 1 share");

        let buy_targets = targets(&[("ONE", 25.0), ("BASE", 75.0)]);
        let buy = compute_rebalance_plan(&holdings, &buy_targets, None, 2.0, "INR").unwrap();
        assert_eq!(buy.items[0].action_type, RebalanceActionType::Buy);
        assert_eq!(buy.items[0].diff_shares, 1.0);
        assert_eq!(buy.items[0].action_display, "BUY 1 share");
    }

    #[test]
    fn single_stock_cap_fallback_never_sells_more_shares_than_held() {
        let mut a = sample_holding("A", 0.6, 1000.0, None);
        a.is_fund = false;
        let b = sample_holding("B", 14.0, 100.0, None);
        let mut o = PlanOptions::new(NaiveDate::from_ymd_opt(2026, 11, 21).unwrap());
        o.max_stock_weight = Some(20.0);
        o.drift_tolerance = 0.0;
        o.min_trade = 0.0;
        let p = plan(&[a, b], &targets(&[("A", 20.0), ("B", 80.0)]), &o, "INR").unwrap();
        assert!(
            p.items[0].diff_shares.abs() <= 0.6,
            "sold more than held: {:?}",
            p.items[0]
        );
        assert!(p.items[0].projected_value >= 0.0);

        let mut a2 = sample_holding("A", 1.5, 1000.0, None);
        a2.is_fund = false;
        let b2 = sample_holding("B", 5.0, 100.0, None);
        let p2 = plan(&[a2, b2], &targets(&[("A", 20.0), ("B", 80.0)]), &o, "INR").unwrap();
        assert!(
            p2.items[0].diff_shares.abs() <= 1.5,
            "sold more than held: {:?}",
            p2.items[0]
        );
    }
}
