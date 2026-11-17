//! `portfolio checkup`: a scan of the whole portfolio for readings worth a
//! look, grouped in six lenses and ranked by the money they touch.
//!
//! Everything here works on plain inputs (no network, no files), so each
//! rule and threshold is testable. `commands::checkup` gathers the inputs:
//! the `portfolio analyze` figures, look-through fund holdings, scorecards
//! and statements, the harvest and Regular-plan reports, goals and targets.
//!
//! A finding states what was observed, the amount or weight at stake and
//! the command that shows the detail. It never says to buy, sell or keep
//! anything: the wording is a reading of the figures, not a recommendation.

use crate::market::{self, CapTier};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// A single stock above this look-through weight (percent) is noted when the
/// profile sets no limit.
pub const DEFAULT_MAX_STOCK_WEIGHT: f64 = 12.0;
/// Two funds sharing more than this share of their holdings (percent).
pub const FUND_OVERLAP_PCT: f64 = 35.0;
/// Fewer effective bets (1 / HHI of look-through weights) than this is noted.
pub const MIN_EFFECTIVE_BETS: f64 = 5.0;
/// A Piotroski F-Score at or below this (with most signals evaluated).
pub const WEAK_PIOTROSKI: u8 = 3;
/// Piotroski signals needed before a low score counts.
pub const MIN_PIOTROSKI_SIGNALS: u8 = 7;
/// Operating cash flow below this multiple of net profit.
pub const MIN_CFO_TO_PAT: f64 = 0.7;
/// A fall in the promoters' stake of at least this many points in a quarter.
pub const PROMOTER_DROP_PP: f64 = 1.0;
/// Trailing P/E above this, with slowing revenue growth.
pub const STRETCHED_PE: f64 = 60.0;
/// PEG above this, with slowing revenue growth.
pub const STRETCHED_PEG: f64 = 2.5;
/// Portfolio beta above this against its benchmark.
pub const HIGH_BETA: f64 = 1.2;
/// A fall from the peak of the last year deeper than this (percent).
pub const DEEP_DRAWDOWN_PCT: f64 = 25.0;
/// Daily-return correlation above this between two holdings.
pub const HIGH_CORRELATION: f64 = 0.75;
/// Holdings below this weight (percent) are left out of correlation pairs.
pub const MIN_CORRELATION_WEIGHT: f64 = 2.0;
/// Overlapping daily returns needed before a correlation counts.
pub const MIN_CORRELATION_OBS: usize = 60;
/// Lots turning long-term within this many days.
pub const LONG_TERM_SOON_DAYS: i64 = 45;

/// The six groups findings fall in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lens {
    Concentration,
    Forensic,
    Valuation,
    Downside,
    FeeTax,
    Goals,
}

impl Lens {
    pub const ALL: [Lens; 6] = [
        Lens::Concentration,
        Lens::Forensic,
        Lens::Valuation,
        Lens::Downside,
        Lens::FeeTax,
        Lens::Goals,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Lens::Concentration => "Concentration, overlap and cap tiers",
            Lens::Forensic => "Forensic and fundamental flags",
            Lens::Valuation => "Valuation stretch",
            Lens::Downside => "Downside and tail risk",
            Lens::FeeTax => "Fees and tax",
            Lens::Goals => "Goals and targets",
        }
    }
}

/// How much a finding weighs in the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn weight(self) -> f64 {
        match self {
            Severity::Low => 1.0,
            Severity::Medium => 2.0,
            Severity::High => 3.0,
        }
    }
}

/// The rules a checkup runs. Each one counts once in the pass rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    StockWeight,
    SmallcapWeight,
    FundOverlap,
    EffectiveBets,
    WeakPiotroski,
    AltmanDistress,
    CashConversion,
    PromoterPledge,
    PromoterStakeDrop,
    EarningsContraction,
    StretchedValuation,
    HighBeta,
    DeepDrawdown,
    CorrelatedPair,
    ExemptionHeadroom,
    HarvestableLosses,
    RegularPlanCost,
    LongTermSoon,
    TargetDrift,
    GoalBehind,
}

impl Rule {
    pub fn lens(self) -> Lens {
        use Rule::*;
        match self {
            StockWeight | SmallcapWeight | FundOverlap | EffectiveBets => Lens::Concentration,
            WeakPiotroski | AltmanDistress | CashConversion | PromoterPledge
            | PromoterStakeDrop | EarningsContraction => Lens::Forensic,
            StretchedValuation => Lens::Valuation,
            HighBeta | DeepDrawdown | CorrelatedPair => Lens::Downside,
            ExemptionHeadroom | HarvestableLosses | RegularPlanCost | LongTermSoon => Lens::FeeTax,
            TargetDrift | GoalBehind => Lens::Goals,
        }
    }
}

/// One reading worth a look.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub lens: Lens,
    pub rule: Rule,
    pub severity: Severity,
    pub symbol: Option<String>,
    /// What was observed, with its figures.
    pub observed: String,
    /// The money the finding touches, base currency: the position's value,
    /// or the amount itself for portfolio-wide readings.
    pub exposure: f64,
    /// The figure at stake: an amount, or a weight for weight rules.
    pub at_stake: Option<f64>,
    pub weight_pct: Option<f64>,
    /// The command that shows the detail.
    pub inspect: String,
    /// `exposure × severity weight`: the ranking key.
    pub impact: f64,
}

/// Whether a rule had the data to run, and whether it raised anything.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub rule: Rule,
    pub lens: Lens,
    pub clean: bool,
}

/// Fundamentals of one stock, from its scorecard and statements.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fundamentals {
    /// Piotroski points and the number of signals evaluated.
    pub piotroski: Option<(u8, u8)>,
    pub altman_z: Option<f64>,
    pub altman_distress: Option<bool>,
    /// Operating cash flow / net profit, latest year.
    pub cfo_to_pat: Option<f64>,
    pub promoter_pledged: Option<bool>,
    /// Change in the promoters' stake over the last quarter, points.
    pub promoter_change_pp: Option<f64>,
    /// Latest quarter against the same quarter a year ago.
    pub pat_yoy_pct: Option<f64>,
    pub opm_change_pp: Option<f64>,
    pub pe: Option<f64>,
    pub peg: Option<f64>,
    /// Latest quarter's revenue growth (year on year), percent.
    pub revenue_yoy_pct: Option<f64>,
    /// Revenue growth slower than the quarter before.
    pub revenue_decelerating: Option<bool>,
}

/// One holding as the checkup sees it. Values are in the base currency.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HoldingFacts {
    pub symbol: String,
    pub value: f64,
    pub weight_pct: f64,
    pub cap_tier: Option<CapTier>,
    pub sector: Option<String>,
    pub is_fund: bool,
    pub fundamentals: Option<Fundamentals>,
    /// The next lot to turn long-term: days left, its unrealised gain and
    /// the tax difference between the short- and long-term rates.
    pub long_term_soon: Option<(i64, f64, Option<f64>)>,
}

/// Two funds that hold many of the same companies.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FundOverlap {
    pub a: String,
    pub b: String,
    pub overlap_pct: f64,
}

/// Two holdings whose daily returns move together.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CorrelatedPair {
    pub a: String,
    pub b: String,
    pub correlation: f64,
    /// Combined value of the two, base currency.
    pub value: f64,
}

/// The tax year's headroom, India or the US, tax currency.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaxFacts {
    pub currency: String,
    /// The yearly long-term exemption left unused (India's ₹1.25 lakh).
    pub exemption_remaining: f64,
    /// Long-term unrealised gains that could use that headroom.
    pub lt_gain_available: f64,
    /// Long-term rate including cess, as a fraction.
    pub lt_rate: f64,
    pub st_loss: f64,
    pub lt_loss: f64,
    pub loss_tax_saved: f64,
    pub days_to_year_end: i64,
    pub year_end: String,
    /// Base currency per unit of `currency`, for exposure and ranking.
    pub fx_to_base: f64,
}

/// A Regular-plan fund and what it costs a year over its Direct twin.
#[derive(Debug, Clone, PartialEq)]
pub struct BleedFacts {
    pub symbol: String,
    /// Rupees, as AMFI reports them.
    pub value: f64,
    pub annual_cost: f64,
    /// Base currency per rupee.
    pub fx_to_base: f64,
}

/// A saved goal that needs more money than it has.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalFacts {
    pub name: String,
    pub overdue: bool,
    pub funded_pct: f64,
    pub monthly_sip: f64,
    /// Target (in money of the due date) less the linked value.
    pub gap: f64,
    /// The goal's currency (the market's) and base currency per unit of it.
    pub currency: String,
    pub fx_to_base: f64,
}

/// A target weight (symbol or sector) and where it stands now.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftFacts {
    pub key: String,
    pub current_pct: f64,
    pub target_pct: f64,
    pub band_pct: f64,
}

/// Everything a checkup reads.
#[derive(Debug, Clone, Default)]
pub struct CheckupInput {
    pub base_currency: String,
    pub total_value: f64,
    pub holdings: Vec<HoldingFacts>,
    /// Weight of each company through direct shares and funds, percent.
    /// Empty when no fund holdings were known: direct weights are used.
    pub look_through: Vec<(String, f64)>,
    pub fund_overlaps: Vec<FundOverlap>,
    /// Funds whose holdings are known, so overlap could be compared.
    pub funds_compared: usize,
    pub beta: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub benchmark: String,
    /// `None`: no price history to compare.
    pub correlated: Option<Vec<CorrelatedPair>>,
    pub tax: Option<TaxFacts>,
    /// `None`: not looked at (no funds, or offline).
    pub bleed: Option<Vec<BleedFacts>>,
    /// `None`: no saved goals.
    pub goals: Option<Vec<GoalFacts>>,
    /// `None`: no target weights.
    pub drift: Option<Vec<DriftFacts>>,
    pub max_stock_weight: Option<f64>,
    pub max_smallcap_weight: Option<f64>,
}

/// The checkup: findings ranked, the checks behind them, and what was left
/// out.
#[derive(Debug, Clone, Serialize)]
pub struct Checkup {
    pub base_currency: String,
    pub total_value: f64,
    pub herfindahl: f64,
    pub effective_bets: f64,
    pub findings: Vec<Finding>,
    pub checks: Vec<Check>,
    pub clean: usize,
    pub evaluated: usize,
    /// Data that could not be read, so its checks were skipped.
    pub skipped: Vec<String>,
    pub disclaimer: &'static str,
}

/// Said under every checkup.
pub const DISCLAIMER: &str = "Observations from your recorded holdings and public data, not \
     recommendations to buy, sell or hold. Check the figures with the command shown before acting.";

// ── Concentration maths ──

/// Herfindahl-Hirschman index of weights in percent: the sum of squared
/// fractions, from 0 (spread thin) to 1 (a single holding).
pub fn herfindahl(weights_pct: &[f64]) -> f64 {
    let total: f64 = weights_pct.iter().filter(|w| **w > 0.0).sum();
    if total <= 0.0 {
        return 0.0;
    }
    weights_pct
        .iter()
        .filter(|w| **w > 0.0)
        .map(|w| (w / total).powi(2))
        .sum()
}

/// 1 / HHI: the number of equal holdings with the same spread.
pub fn effective_bets(weights_pct: &[f64]) -> f64 {
    let h = herfindahl(weights_pct);
    if h > 0.0 { 1.0 / h } else { 0.0 }
}

/// A fund for the look-through: its value and holdings as `(ticker, fraction)`.
#[derive(Debug, Clone, PartialEq)]
pub struct FundHoldings {
    pub symbol: String,
    pub value: f64,
    pub holdings: Vec<(String, f64)>,
}

/// Each company's weight through direct shares and the funds that hold it,
/// percent of `total`, largest first. Fund holdings not disclosed stay as
/// the fund itself. Tickers match without exchange suffixes.
pub fn look_through(
    direct: &[(String, f64)],
    funds: &[FundHoldings],
    total: f64,
) -> Vec<(String, f64)> {
    if total <= 0.0 {
        return Vec::new();
    }
    let mut by: HashMap<String, f64> = HashMap::new();
    for (sym, value) in direct {
        *by.entry(crate::xray::canonical_ticker(sym)).or_default() += value;
    }
    for f in funds {
        let covered: f64 = f
            .holdings
            .iter()
            .map(|(_, w)| w.max(0.0))
            .sum::<f64>()
            .min(1.0);
        for (sym, w) in &f.holdings {
            *by.entry(crate::xray::canonical_ticker(sym)).or_default() += f.value * w.max(0.0);
        }
        let rest = f.value * (1.0 - covered);
        if rest > 0.0 {
            *by.entry(f.symbol.clone()).or_default() += rest;
        }
    }
    let mut out: Vec<(String, f64)> = by
        .into_iter()
        .map(|(s, v)| (s, v / total * 100.0))
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Share of holdings two funds have in common, percent: the sum over shared
/// companies of the smaller of the two weights.
pub fn fund_overlap(a: &[(String, f64)], b: &[(String, f64)]) -> f64 {
    let map = |h: &[(String, f64)]| {
        let mut m: HashMap<String, f64> = HashMap::new();
        for (s, w) in h {
            *m.entry(crate::xray::canonical_ticker(s)).or_default() += w.max(0.0);
        }
        m
    };
    let (ma, mb) = (map(a), map(b));
    ma.iter()
        .filter_map(|(s, wa)| mb.get(s).map(|wb| wa.min(*wb)))
        .sum::<f64>()
        * 100.0
}

/// Every pair of funds whose overlap is above [`FUND_OVERLAP_PCT`].
pub fn fund_overlaps(funds: &[FundHoldings]) -> Vec<FundOverlap> {
    let mut out = Vec::new();
    for (i, a) in funds.iter().enumerate() {
        for b in &funds[i + 1..] {
            let o = fund_overlap(&a.holdings, &b.holdings);
            if o > FUND_OVERLAP_PCT {
                out.push(FundOverlap {
                    a: a.symbol.clone(),
                    b: b.symbol.clone(),
                    overlap_pct: o,
                });
            }
        }
    }
    out
}

/// Pearson correlation of two equal-length series; `None` when either is flat.
pub fn correlation(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len().min(y.len());
    if n < 2 {
        return None;
    }
    let (mx, my) = (
        x[..n].iter().sum::<f64>() / n as f64,
        y[..n].iter().sum::<f64>() / n as f64,
    );
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (dx, dy) = (x[i] - mx, y[i] - my);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    (sxx > 0.0 && syy > 0.0).then(|| sxy / (sxx.sqrt() * syy.sqrt()))
}

/// A holding's daily closes for the correlation scan.
pub struct Series<'a> {
    pub symbol: &'a str,
    pub value: f64,
    pub weight_pct: f64,
    pub closes: &'a [(chrono::NaiveDate, f64)],
}

/// Pairs of holdings (each at least [`MIN_CORRELATION_WEIGHT`]) whose daily
/// returns on shared sessions correlate above [`HIGH_CORRELATION`].
pub fn correlated_pairs(series: &[Series]) -> Vec<CorrelatedPair> {
    let returns: Vec<(&Series, BTreeMap<chrono::NaiveDate, f64>)> = series
        .iter()
        .filter(|s| s.weight_pct >= MIN_CORRELATION_WEIGHT)
        .map(|s| {
            let r = s
                .closes
                .windows(2)
                .filter(|w| w[0].1 > 0.0)
                .map(|w| (w[1].0, w[1].1 / w[0].1 - 1.0))
                .collect();
            (s, r)
        })
        .collect();
    let mut out = Vec::new();
    for (i, (a, ra)) in returns.iter().enumerate() {
        for (b, rb) in &returns[i + 1..] {
            let (x, y): (Vec<f64>, Vec<f64>) = ra
                .iter()
                .filter_map(|(d, r)| rb.get(d).map(|r2| (*r, *r2)))
                .unzip();
            if x.len() < MIN_CORRELATION_OBS {
                continue;
            }
            if let Some(c) = correlation(&x, &y).filter(|c| *c > HIGH_CORRELATION) {
                out.push(CorrelatedPair {
                    a: a.symbol.to_string(),
                    b: b.symbol.to_string(),
                    correlation: c,
                    value: a.value + b.value,
                });
            }
        }
    }
    out
}

// ── Statement helpers ──

/// Latest quarter against the same quarter a year earlier (quarters oldest
/// first): profit growth (percent) and operating-margin change (points).
pub fn quarter_yoy(quarters: &[crate::research::Period]) -> (Option<f64>, Option<f64>) {
    let n = quarters.len();
    if n < 5 {
        return (None, None);
    }
    let (now, then) = (&quarters[n - 1], &quarters[n - 5]);
    let pat = match (now.net_income, then.net_income) {
        (Some(a), Some(b)) if b > 0.0 => Some((a / b - 1.0) * 100.0),
        _ => None,
    };
    let opm = match (now.op_margin(), then.op_margin()) {
        (Some(a), Some(b)) => Some((a - b) * 100.0),
        _ => None,
    };
    (pat, opm)
}

/// Revenue growth of the latest quarter year on year (percent), and whether
/// it is slower than the quarter before's.
pub fn revenue_trend(quarters: &[crate::research::Period]) -> (Option<f64>, Option<bool>) {
    let n = quarters.len();
    let yoy = |i: usize| match (quarters[i].revenue, quarters[i - 4].revenue) {
        (Some(a), Some(b)) if b > 0.0 => Some((a / b - 1.0) * 100.0),
        _ => None,
    };
    if n < 5 {
        return (None, None);
    }
    let latest = yoy(n - 1);
    let prior = if n >= 6 { yoy(n - 2) } else { None };
    let decel = match (latest, prior) {
        (Some(l), Some(p)) => Some(l < p),
        _ => None,
    };
    (latest, decel)
}

/// Operating cash flow over net profit for the latest year with both, when
/// the profit is positive.
pub fn cfo_to_pat(annual: &[crate::research::Period]) -> Option<f64> {
    // The latest year with both figures; a loss there gives no ratio rather
    // than an older year's.
    let (c, n) = annual
        .iter()
        .rev()
        .find_map(|p| Some((p.ocf?, p.net_income?)))?;
    (n > 0.0).then(|| c / n)
}

// ── Evaluation ──

fn money(v: f64, currency: &str) -> String {
    let c = Some(currency);
    let body = crate::display::group_number(&format!("{:.0}", v), market::is_inr(c));
    format!("{}{}", market::currency_symbol(c), body)
}

struct Builder {
    findings: Vec<Finding>,
    ran: BTreeMap<Rule, bool>,
}

impl Builder {
    fn ran(&mut self, rule: Rule) {
        self.ran.entry(rule).or_insert(true);
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        rule: Rule,
        severity: Severity,
        symbol: Option<&str>,
        observed: String,
        exposure: f64,
        at_stake: Option<f64>,
        weight_pct: Option<f64>,
        inspect: String,
    ) {
        self.ran.insert(rule, false);
        self.findings.push(Finding {
            lens: rule.lens(),
            rule,
            severity,
            symbol: symbol.map(str::to_string),
            observed,
            exposure,
            at_stake,
            weight_pct,
            inspect,
            impact: exposure.max(0.0) * severity.weight(),
        });
    }
}

/// Order findings by impact, largest first (ties: lens, then symbol).
pub fn rank(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.impact
            .total_cmp(&a.impact)
            .then_with(|| a.lens.cmp(&b.lens))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
}

fn is_small(t: Option<CapTier>) -> bool {
    matches!(t, Some(CapTier::Small | CapTier::Micro))
}

fn is_classified(t: Option<CapTier>) -> bool {
    !matches!(t, None | Some(CapTier::Unknown))
}

/// Run every rule over `input`. `skipped` names the data that could not be
/// read; it is carried into the report.
pub fn evaluate(input: &CheckupInput, mut skipped: Vec<String>) -> Checkup {
    let cur = input.base_currency.as_str();
    let total = input.total_value;
    let mut b = Builder {
        findings: Vec::new(),
        ran: BTreeMap::new(),
    };
    let value_of = |sym: &str| {
        input
            .holdings
            .iter()
            .find(|h| h.symbol == sym || crate::xray::canonical_ticker(&h.symbol) == sym)
            .map(|h| h.value)
    };

    // 1. Concentration.
    let weights: Vec<(String, f64)> = if input.look_through.is_empty() {
        input
            .holdings
            .iter()
            .map(|h| (h.symbol.clone(), h.weight_pct))
            .collect()
    } else {
        input.look_through.clone()
    };
    let limit = input.max_stock_weight.unwrap_or(DEFAULT_MAX_STOCK_WEIGHT);
    let fund_symbols: Vec<&str> = input
        .holdings
        .iter()
        .filter(|h| h.is_fund)
        .map(|h| h.symbol.as_str())
        .collect();
    if !weights.is_empty() {
        b.ran(Rule::StockWeight);
    }
    for (sym, w) in &weights {
        // A fund's undisclosed remainder is not one company.
        if fund_symbols.contains(&sym.as_str()) || *w <= limit {
            continue;
        }
        let exposure = total * w / 100.0;
        let how = if input.look_through.is_empty() {
            "of the portfolio"
        } else {
            "of the portfolio, counting shares held through funds"
        };
        b.add(
            Rule::StockWeight,
            if *w > limit * 1.5 {
                Severity::High
            } else {
                Severity::Medium
            },
            Some(sym),
            format!(
                "{sym} is {w:.1}% {how}, above the profile's {limit:.0}% single-stock limit ({} over the limit).",
                money(total * (w - limit) / 100.0, cur)
            ),
            exposure,
            Some(exposure),
            Some(*w),
            "stockwise portfolio xray".into(),
        );
    }
    let herf = herfindahl(&weights.iter().map(|w| w.1).collect::<Vec<_>>());
    let bets = if herf > 0.0 { 1.0 / herf } else { 0.0 };
    if weights.len() > 1 {
        b.ran(Rule::EffectiveBets);
        if bets < MIN_EFFECTIVE_BETS {
            b.add(
                Rule::EffectiveBets,
                Severity::Medium,
                None,
                format!(
                    "The portfolio behaves like {bets:.1} equal-sized holdings (HHI {herf:.3}), \
                     fewer than {MIN_EFFECTIVE_BETS:.0}."
                ),
                total,
                None,
                None,
                "stockwise portfolio analyze".into(),
            );
        }
    }
    let unclassified: Vec<&str> = input
        .holdings
        .iter()
        .filter(|h| !h.is_fund && !is_classified(h.cap_tier))
        .map(|h| h.symbol.as_str())
        .collect();
    if let Some(max_small) = input.max_smallcap_weight
        && input.holdings.iter().any(|h| is_classified(h.cap_tier))
    {
        if !unclassified.is_empty() {
            skipped.push(format!(
                "Cap tier unknown for {}: left out of the small-cap share",
                unclassified.join(", ")
            ));
        }
        b.ran(Rule::SmallcapWeight);
        let small: f64 = input
            .holdings
            .iter()
            .filter(|h| is_small(h.cap_tier))
            .map(|h| h.value)
            .sum();
        let pct = if total > 0.0 {
            small / total * 100.0
        } else {
            0.0
        };
        if pct > max_small {
            b.add(
                Rule::SmallcapWeight,
                Severity::Medium,
                None,
                format!(
                    "Small and micro caps are {pct:.1}% of the portfolio ({}), above the profile's \
                     {max_small:.0}% limit.",
                    money(small, cur)
                ),
                small,
                Some(small),
                Some(pct),
                "stockwise portfolio analyze".into(),
            );
        }
    }
    if input.funds_compared > 1 {
        b.ran(Rule::FundOverlap);
    }
    for o in &input.fund_overlaps {
        let value = value_of(&o.a).unwrap_or(0.0) + value_of(&o.b).unwrap_or(0.0);
        b.add(
            Rule::FundOverlap,
            Severity::Low,
            Some(&o.a),
            format!(
                "{} and {} share {:.0}% of their holdings (above {FUND_OVERLAP_PCT:.0}%).",
                o.a, o.b, o.overlap_pct
            ),
            value,
            Some(value),
            Some(o.overlap_pct),
            "stockwise portfolio xray".into(),
        );
    }

    // 2. Forensic, 3. valuation.
    for h in &input.holdings {
        let Some(f) = &h.fundamentals else { continue };
        let sym = h.symbol.as_str();
        let score_cmd = format!("stockwise score {sym} --explain");
        if let Some((score, evaluated)) = f.piotroski
            && evaluated >= MIN_PIOTROSKI_SIGNALS
        {
            b.ran(Rule::WeakPiotroski);
            if score <= WEAK_PIOTROSKI {
                b.add(
                    Rule::WeakPiotroski,
                    Severity::Medium,
                    Some(sym),
                    format!(
                        "Piotroski F-Score {score} of {evaluated} signals evaluated (at or below {WEAK_PIOTROSKI}); \
                         position {} ({:.1}%).",
                        money(h.value, cur),
                        h.weight_pct
                    ),
                    h.value,
                    Some(h.value),
                    Some(h.weight_pct),
                    score_cmd.clone(),
                );
            }
        }
        if let Some(distress) = f.altman_distress {
            b.ran(Rule::AltmanDistress);
            if distress {
                b.add(
                    Rule::AltmanDistress,
                    Severity::High,
                    Some(sym),
                    format!(
                        "Altman Z-Score {} is in the distress zone; position {} ({:.1}%).",
                        f.altman_z.map_or("–".into(), |z| format!("{z:.2}")),
                        money(h.value, cur),
                        h.weight_pct
                    ),
                    h.value,
                    Some(h.value),
                    Some(h.weight_pct),
                    score_cmd.clone(),
                );
            }
        }
        if let Some(r) = f.cfo_to_pat {
            b.ran(Rule::CashConversion);
            if r < MIN_CFO_TO_PAT {
                b.add(
                    Rule::CashConversion,
                    Severity::Medium,
                    Some(sym),
                    format!(
                        "Operating cash flow was {r:.2}× net profit last year (below {MIN_CFO_TO_PAT}×)."
                    ),
                    h.value,
                    Some(h.value),
                    Some(h.weight_pct),
                    format!("stockwise insights {sym}"),
                );
            }
        }
        if let Some(p) = f.promoter_pledged {
            b.ran(Rule::PromoterPledge);
            if p {
                b.add(
                    Rule::PromoterPledge,
                    Severity::High,
                    Some(sym),
                    "Part of the promoters' holding is pledged or encumbered (latest shareholding filing)."
                        .into(),
                    h.value,
                    Some(h.value),
                    Some(h.weight_pct),
                    format!("stockwise insights {sym}"),
                );
            }
        }
        if let Some(d) = f.promoter_change_pp {
            b.ran(Rule::PromoterStakeDrop);
            if d <= -PROMOTER_DROP_PP {
                b.add(
                    Rule::PromoterStakeDrop,
                    Severity::Medium,
                    Some(sym),
                    format!("The promoters' stake fell {:.2} points last quarter.", -d),
                    h.value,
                    Some(h.value),
                    Some(h.weight_pct),
                    format!("stockwise insights {sym}"),
                );
            }
        }
        if let (Some(pat), Some(opm)) = (f.pat_yoy_pct, f.opm_change_pp) {
            b.ran(Rule::EarningsContraction);
            if pat < 0.0 && opm < 0.0 {
                b.add(
                    Rule::EarningsContraction,
                    Severity::Medium,
                    Some(sym),
                    format!(
                        "Latest quarter: net profit {pat:+.1}% and operating margin {opm:+.1} points \
                         against the same quarter a year ago."
                    ),
                    h.value,
                    Some(h.value),
                    Some(h.weight_pct),
                    format!("stockwise results {sym}"),
                );
            }
        }
        let rich =
            f.pe.is_some_and(|p| p > STRETCHED_PE) || f.peg.is_some_and(|p| p > STRETCHED_PEG);
        if rich && f.revenue_decelerating.is_none() {
            // A high multiple alone is not the rule: without the revenue
            // trend it cannot be judged.
            skipped.push(format!(
                "Revenue trend unknown for {sym}: valuation not judged"
            ));
        } else if f.pe.is_some() || f.peg.is_some() {
            b.ran(Rule::StretchedValuation);
            if rich && f.revenue_decelerating == Some(true) {
                b.add(
                    Rule::StretchedValuation,
                    Severity::Low,
                    Some(sym),
                    format!(
                        "P/E {} and PEG {} with revenue growth slowing to {} year on year; {:.1}% of the portfolio.",
                        f.pe.map_or("–".into(), |p| format!("{p:.0}")),
                        f.peg.map_or("–".into(), |p| format!("{p:.1}")),
                        f.revenue_yoy_pct.map_or("–".into(), |g| format!("{g:+.1}%")),
                        h.weight_pct
                    ),
                    h.value,
                    Some(h.value),
                    Some(h.weight_pct),
                    format!("stockwise fundamental {sym}"),
                );
            }
        }
    }

    // 4. Downside.
    if let Some(beta) = input.beta {
        b.ran(Rule::HighBeta);
        if beta > HIGH_BETA {
            b.add(
                Rule::HighBeta,
                Severity::Medium,
                None,
                format!(
                    "Portfolio beta is {beta:.2} against {} (above {HIGH_BETA}): a 10% index move has \
                     come with about {:.0}% here.",
                    input.benchmark,
                    beta * 10.0
                ),
                total,
                None,
                None,
                "stockwise portfolio benchmark".into(),
            );
        }
    }
    if let Some(dd) = input.max_drawdown_pct {
        b.ran(Rule::DeepDrawdown);
        if dd.abs() > DEEP_DRAWDOWN_PCT {
            let lost = total * dd.abs() / 100.0;
            b.add(
                Rule::DeepDrawdown,
                Severity::Medium,
                None,
                format!(
                    "The portfolio as weighted today fell {:.1}% from its peak in the last year \
                     (about {} at today's value).",
                    dd.abs(),
                    money(lost, cur)
                ),
                lost,
                Some(lost),
                None,
                "stockwise portfolio analyze".into(),
            );
        }
    }
    if let Some(pairs) = &input.correlated {
        b.ran(Rule::CorrelatedPair);
        for p in pairs {
            b.add(
                Rule::CorrelatedPair,
                Severity::Low,
                Some(&p.a),
                format!(
                    "{} and {} moved together (daily-return correlation {:.2}, above {HIGH_CORRELATION}); \
                     together {}.",
                    p.a,
                    p.b,
                    p.correlation,
                    money(p.value, cur)
                ),
                p.value,
                Some(p.value),
                None,
                "stockwise pcorr".into(),
            );
        }
    }

    // 5. Fees and tax.
    if let Some(t) = &input.tax {
        let tc = t.currency.as_str();
        b.ran(Rule::ExemptionHeadroom);
        let usable = t.exemption_remaining.min(t.lt_gain_available).max(0.0);
        if usable > 0.0 {
            let saved = usable * t.lt_rate;
            b.add(
                Rule::ExemptionHeadroom,
                Severity::Medium,
                None,
                format!(
                    "{} of this year's long-term exemption is unused and {} of long-term gains are \
                     unrealised; gains booked within it carry no tax (about {} at the long-term rate). \
                     {} days to {}.",
                    money(t.exemption_remaining, tc),
                    money(t.lt_gain_available, tc),
                    money(saved, tc),
                    t.days_to_year_end,
                    t.year_end
                ),
                saved * t.fx_to_base,
                Some(saved * t.fx_to_base),
                None,
                "stockwise harvest".into(),
            );
        }
        b.ran(Rule::HarvestableLosses);
        let losses = t.st_loss.abs() + t.lt_loss.abs();
        if losses > 0.0 && t.loss_tax_saved > 0.0 {
            b.add(
                Rule::HarvestableLosses,
                Severity::Medium,
                None,
                format!(
                    "Unrealised losses of {} (short-term {}, long-term {}) could offset {} of tax on \
                     this year's gains. {} days to {}.",
                    money(losses, tc),
                    money(t.st_loss.abs(), tc),
                    money(t.lt_loss.abs(), tc),
                    money(t.loss_tax_saved, tc),
                    t.days_to_year_end,
                    t.year_end
                ),
                t.loss_tax_saved * t.fx_to_base,
                Some(t.loss_tax_saved * t.fx_to_base),
                None,
                "stockwise harvest".into(),
            );
        }
    }
    if let Some(bleed) = &input.bleed {
        b.ran(Rule::RegularPlanCost);
        for f in bleed.iter().filter(|f| f.annual_cost > 0.0) {
            b.add(
                Rule::RegularPlanCost,
                Severity::Medium,
                Some(&f.symbol),
                format!(
                    "Regular plan: about {} a year more in expenses than its Direct plan on {}.",
                    money(f.annual_cost, "INR"),
                    money(f.value, "INR")
                ),
                f.value * f.fx_to_base,
                Some(f.annual_cost * f.fx_to_base),
                None,
                "stockwise mf bleed".into(),
            );
        }
    }
    if input.holdings.iter().any(|h| h.long_term_soon.is_some()) || input.tax.is_some() {
        b.ran(Rule::LongTermSoon);
    }
    for h in &input.holdings {
        let Some((days, gain, diff)) = h.long_term_soon else {
            continue;
        };
        if days > LONG_TERM_SOON_DAYS || gain <= 0.0 {
            continue;
        }
        b.add(
            Rule::LongTermSoon,
            Severity::Low,
            Some(&h.symbol),
            format!(
                "A lot turns long-term in {days} days; its unrealised gain is taxed{} less after that.",
                diff.map_or(String::new(), |d| format!(" about {}", money(d, cur)))
            ),
            h.value,
            diff,
            Some(h.weight_pct),
            format!("stockwise portfolio lots {}", h.symbol),
        );
    }

    // 6. Goals and targets.
    if let Some(drift) = &input.drift {
        b.ran(Rule::TargetDrift);
        for d in drift {
            let gap = d.current_pct - d.target_pct;
            if gap.abs() <= d.band_pct {
                continue;
            }
            let amount = total * gap.abs() / 100.0;
            b.add(
                Rule::TargetDrift,
                Severity::Low,
                Some(&d.key),
                format!(
                    "{} is {:.1}% against a {:.1}% target ({:+.1} points, outside the ±{:.1} band; {}).",
                    d.key,
                    d.current_pct,
                    d.target_pct,
                    gap,
                    d.band_pct,
                    money(amount, cur)
                ),
                amount,
                Some(amount),
                Some(d.current_pct),
                "stockwise portfolio rebalance".into(),
            );
        }
    }
    if let Some(goals) = &input.goals {
        b.ran(Rule::GoalBehind);
        for g in goals.iter().filter(|g| g.gap > 0.0) {
            b.add(
                Rule::GoalBehind,
                if g.overdue {
                    Severity::High
                } else {
                    Severity::Low
                },
                Some(&g.name),
                if g.overdue {
                    format!(
                        "Goal `{}` is past its date and {:.0}% funded ({} short).",
                        g.name,
                        g.funded_pct,
                        money(g.gap, &g.currency)
                    )
                } else {
                    format!(
                        "Goal `{}` is {:.0}% funded; reaching it needs about {} a month ({} short today).",
                        g.name,
                        g.funded_pct,
                        money(g.monthly_sip, &g.currency),
                        money(g.gap, &g.currency)
                    )
                },
                g.gap * g.fx_to_base,
                Some(g.gap * g.fx_to_base),
                None,
                format!("stockwise goal show {}", g.name),
            );
        }
    }

    let mut findings = b.findings;
    rank(&mut findings);
    let checks: Vec<Check> = b
        .ran
        .into_iter()
        .map(|(rule, clean)| Check {
            rule,
            lens: rule.lens(),
            clean,
        })
        .collect();
    let clean = checks.iter().filter(|c| c.clean).count();
    Checkup {
        base_currency: input.base_currency.clone(),
        total_value: total,
        herfindahl: herf,
        effective_bets: bets,
        evaluated: checks.len(),
        clean,
        findings,
        checks,
        skipped,
        disclaimer: DISCLAIMER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn h(sym: &str, value: f64, total: f64) -> HoldingFacts {
        HoldingFacts {
            symbol: sym.into(),
            value,
            weight_pct: value / total * 100.0,
            cap_tier: Some(CapTier::Large),
            ..Default::default()
        }
    }

    fn input(holdings: Vec<HoldingFacts>) -> CheckupInput {
        let total = holdings.iter().map(|h| h.value).sum();
        CheckupInput {
            base_currency: "INR".into(),
            total_value: total,
            holdings,
            benchmark: "NIFTY 50".into(),
            max_stock_weight: Some(12.0),
            max_smallcap_weight: Some(20.0),
            ..Default::default()
        }
    }

    fn rules(c: &Checkup) -> Vec<Rule> {
        c.findings.iter().map(|f| f.rule).collect()
    }

    /// Ten equal holdings: nothing above any limit.
    fn spread() -> CheckupInput {
        input(
            (0..10)
                .map(|i| h(&format!("S{i}.NS"), 100.0, 1000.0))
                .collect(),
        )
    }

    #[test]
    fn hhi_and_effective_bets() {
        assert!((herfindahl(&[50.0, 50.0]) - 0.5).abs() < 1e-12);
        assert!((effective_bets(&[25.0; 4]) - 4.0).abs() < 1e-9);
        assert!((herfindahl(&[100.0]) - 1.0).abs() < 1e-12);
        assert_eq!(effective_bets(&[]), 0.0);
        // Weights need not sum to 100.
        assert!((effective_bets(&[10.0, 10.0]) - 2.0).abs() < 1e-9);
        // 60/20/20: 0.36 + 0.04 + 0.04 = 0.44.
        assert!((herfindahl(&[60.0, 20.0, 20.0]) - 0.44).abs() < 1e-12);
    }

    #[test]
    fn clean_portfolio_has_no_findings() {
        let c = evaluate(&spread(), vec![]);
        assert!(c.findings.is_empty(), "{:?}", c.findings);
        assert_eq!(c.clean, c.evaluated);
        assert!((c.effective_bets - 10.0).abs() < 1e-9);
    }

    #[test]
    fn stock_weight_above_profile_limit() {
        let mut i = input(vec![h("BIG.NS", 300.0, 1000.0), h("A.NS", 700.0, 1000.0)]);
        i.max_stock_weight = Some(80.0);
        assert!(!rules(&evaluate(&i, vec![])).contains(&Rule::StockWeight));
        i.max_stock_weight = Some(25.0);
        let c = evaluate(&i, vec![]);
        let f: Vec<&Finding> = c
            .findings
            .iter()
            .filter(|f| f.rule == Rule::StockWeight)
            .collect();
        assert_eq!(f.len(), 2);
        // 70% is above 1.5 × 25%: high.
        assert_eq!(f[0].symbol.as_deref(), Some("A.NS"));
        assert_eq!(f[0].severity, Severity::High);
        assert!(f[0].observed.contains("70.0%"), "{}", f[0].observed);
        assert_eq!(f[0].inspect, "stockwise portfolio xray");
    }

    #[test]
    fn look_through_counts_fund_holdings() {
        let direct = vec![("TCS.NS".to_string(), 100.0)];
        let fund = FundHoldings {
            symbol: "MF:1".into(),
            value: 100.0,
            holdings: vec![("TCS".into(), 0.5), ("INFY".into(), 0.3)],
        };
        let w = look_through(&direct, &[fund], 200.0);
        let get = |s: &str| w.iter().find(|x| x.0 == s).map(|x| x.1).unwrap();
        assert_eq!(w[0].0, "TCS");
        assert!((get("TCS") - 75.0).abs() < 1e-9);
        assert!((get("INFY") - 15.0).abs() < 1e-9);
        // The undisclosed 20% stays the fund's own.
        assert!((get("MF:1") - 10.0).abs() < 1e-9);
    }

    #[test]
    fn look_through_weight_raises_stock_finding() {
        let mut i = spread();
        i.look_through = vec![("S1".into(), 14.0), ("S2".into(), 9.0)];
        let c = evaluate(&i, vec![]);
        let f = c
            .findings
            .iter()
            .find(|f| f.rule == Rule::StockWeight)
            .unwrap();
        assert!(f.observed.contains("through funds"));
        assert!((f.exposure - 140.0).abs() < 1e-9);
    }

    #[test]
    fn fund_overlap_threshold() {
        let a = vec![("X".to_string(), 0.4), ("Y".into(), 0.3), ("Z".into(), 0.3)];
        let b = vec![
            ("X.NS".to_string(), 0.2),
            ("Y".into(), 0.2),
            ("W".into(), 0.6),
        ];
        assert!((fund_overlap(&a, &b) - 40.0).abs() < 1e-9);
        let funds = vec![
            FundHoldings {
                symbol: "MF:1".into(),
                value: 1.0,
                holdings: a.clone(),
            },
            FundHoldings {
                symbol: "MF:2".into(),
                value: 1.0,
                holdings: b,
            },
            FundHoldings {
                symbol: "MF:3".into(),
                value: 1.0,
                holdings: vec![("Q".into(), 1.0)],
            },
        ];
        let o = fund_overlaps(&funds);
        assert_eq!(o.len(), 1);
        assert_eq!((o[0].a.as_str(), o[0].b.as_str()), ("MF:1", "MF:2"));
    }

    #[test]
    fn few_effective_bets() {
        let i = input(vec![h("A.NS", 500.0, 1000.0), h("B.NS", 500.0, 1000.0)]);
        let mut i = i;
        i.max_stock_weight = Some(60.0);
        let c = evaluate(&i, vec![]);
        assert!(rules(&c).contains(&Rule::EffectiveBets));
        assert!((c.effective_bets - 2.0).abs() < 1e-9);
    }

    #[test]
    fn smallcap_share_above_limit() {
        let mut i = spread();
        for x in i.holdings.iter_mut().take(3) {
            x.cap_tier = Some(CapTier::Small);
        }
        i.holdings[3].cap_tier = Some(CapTier::Micro);
        let c = evaluate(&i, vec![]);
        let f = c
            .findings
            .iter()
            .find(|f| f.rule == Rule::SmallcapWeight)
            .unwrap();
        assert_eq!(f.weight_pct, Some(40.0));
        i.max_smallcap_weight = Some(40.0);
        assert!(!rules(&evaluate(&i, vec![])).contains(&Rule::SmallcapWeight));
    }

    fn with_fundamentals(f: Fundamentals) -> CheckupInput {
        let mut i = spread();
        i.holdings[0].fundamentals = Some(f);
        i
    }

    #[test]
    fn forensic_thresholds() {
        let weak = Fundamentals {
            piotroski: Some((3, 9)),
            altman_z: Some(1.2),
            altman_distress: Some(true),
            cfo_to_pat: Some(0.5),
            promoter_pledged: Some(true),
            promoter_change_pp: Some(-1.5),
            pat_yoy_pct: Some(-12.0),
            opm_change_pp: Some(-2.0),
            ..Default::default()
        };
        let c = evaluate(&with_fundamentals(weak), vec![]);
        for r in [
            Rule::WeakPiotroski,
            Rule::AltmanDistress,
            Rule::CashConversion,
            Rule::PromoterPledge,
            Rule::PromoterStakeDrop,
            Rule::EarningsContraction,
        ] {
            assert!(rules(&c).contains(&r), "{r:?}");
        }
        let f = c
            .findings
            .iter()
            .find(|f| f.rule == Rule::WeakPiotroski)
            .unwrap();
        assert_eq!(f.inspect, "stockwise score S0.NS --explain");

        let fine = Fundamentals {
            piotroski: Some((4, 9)),
            altman_distress: Some(false),
            cfo_to_pat: Some(0.7),
            promoter_pledged: Some(false),
            promoter_change_pp: Some(-0.9),
            pat_yoy_pct: Some(-5.0),
            opm_change_pp: Some(1.0),
            ..Default::default()
        };
        let c = evaluate(&with_fundamentals(fine), vec![]);
        assert!(c.findings.is_empty(), "{:?}", c.findings);
        // A low score from few signals does not count.
        let partial = Fundamentals {
            piotroski: Some((2, 5)),
            ..Default::default()
        };
        let c = evaluate(&with_fundamentals(partial), vec![]);
        assert!(c.findings.is_empty());
        assert!(!c.checks.iter().any(|c| c.rule == Rule::WeakPiotroski));
    }

    #[test]
    fn valuation_needs_slowing_revenue() {
        let rich = |decel| Fundamentals {
            pe: Some(75.0),
            peg: Some(1.0),
            revenue_yoy_pct: Some(8.0),
            revenue_decelerating: decel,
            ..Default::default()
        };
        assert!(
            rules(&evaluate(&with_fundamentals(rich(Some(true))), vec![]))
                .contains(&Rule::StretchedValuation)
        );
        assert!(
            evaluate(&with_fundamentals(rich(Some(false))), vec![])
                .findings
                .is_empty()
        );
        let unknown = evaluate(&with_fundamentals(rich(None)), vec![]);
        assert!(unknown.findings.is_empty());
        assert!(
            !unknown
                .checks
                .iter()
                .any(|c| c.rule == Rule::StretchedValuation)
        );
        assert!(unknown.skipped[0].contains("Revenue trend unknown"));
        let peg = Fundamentals {
            pe: Some(30.0),
            peg: Some(2.6),
            revenue_decelerating: Some(true),
            ..Default::default()
        };
        assert!(
            rules(&evaluate(&with_fundamentals(peg), vec![])).contains(&Rule::StretchedValuation)
        );
    }

    #[test]
    fn downside_thresholds() {
        let mut i = spread();
        i.beta = Some(1.2);
        i.max_drawdown_pct = Some(-25.0);
        assert!(evaluate(&i, vec![]).findings.is_empty());
        i.beta = Some(1.3);
        i.max_drawdown_pct = Some(-30.0);
        let c = evaluate(&i, vec![]);
        assert!(rules(&c).contains(&Rule::HighBeta));
        let dd = c
            .findings
            .iter()
            .find(|f| f.rule == Rule::DeepDrawdown)
            .unwrap();
        assert!((dd.exposure - 300.0).abs() < 1e-9);
    }

    fn walk(seed: u64, n: usize) -> Vec<f64> {
        // A deterministic pseudo-random walk.
        let mut x = seed;
        let mut p = 100.0;
        (0..n)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = ((x >> 33) as f64 / (1u64 << 31) as f64) - 0.5;
                p *= 1.0 + r * 0.04;
                p
            })
            .collect()
    }

    #[test]
    fn correlation_pairs_from_closes() {
        let d0 = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let dated = |v: Vec<f64>| -> Vec<(NaiveDate, f64)> {
            v.into_iter()
                .enumerate()
                .map(|(i, p)| (d0 + chrono::Days::new(i as u64), p))
                .collect()
        };
        let a = dated(walk(1, 120));
        let b = dated(a.iter().map(|(_, p)| p * 2.0).collect());
        let c = dated(walk(99, 120));
        let short = dated(walk(1, 30));
        let s = |sym, closes: &'static [(NaiveDate, f64)], w| Series {
            symbol: sym,
            value: 100.0,
            weight_pct: w,
            closes,
        };
        let (a, b, c, short) = (a.leak(), b.leak(), c.leak(), short.leak());
        let pairs = correlated_pairs(&[s("A", a, 10.0), s("B", b, 10.0), s("C", c, 10.0)]);
        assert_eq!(pairs.len(), 1);
        assert!((pairs[0].correlation - 1.0).abs() < 1e-9);
        // Too small a weight, or too short a history: left out.
        assert!(correlated_pairs(&[s("A", a, 10.0), s("B", b, 1.0)]).is_empty());
        assert!(correlated_pairs(&[s("A", a, 10.0), s("S", short, 10.0)]).is_empty());
        assert_eq!(correlation(&[1.0, 1.0], &[1.0, 2.0]), None);
    }

    #[test]
    fn tax_headroom_and_losses() {
        let mut i = spread();
        i.tax = Some(TaxFacts {
            currency: "INR".into(),
            exemption_remaining: 125_000.0,
            lt_gain_available: 40_000.0,
            lt_rate: 0.13,
            st_loss: -10_000.0,
            lt_loss: 0.0,
            loss_tax_saved: 2_000.0,
            days_to_year_end: 134,
            year_end: "31 Mar 2027".into(),
            fx_to_base: 1.0,
        });
        let c = evaluate(&i, vec![]);
        let head = c
            .findings
            .iter()
            .find(|f| f.rule == Rule::ExemptionHeadroom)
            .unwrap();
        // Limited by the gains there are: 40,000 × 13%.
        assert!((head.exposure - 5_200.0).abs() < 1e-6);
        assert!(head.observed.contains("₹1,25,000"), "{}", head.observed);
        assert!(rules(&c).contains(&Rule::HarvestableLosses));

        let mut used = i.clone();
        used.tax.as_mut().unwrap().exemption_remaining = 0.0;
        used.tax.as_mut().unwrap().loss_tax_saved = 0.0;
        let c = evaluate(&used, vec![]);
        assert!(c.findings.is_empty());
        assert!(
            c.checks
                .iter()
                .any(|c| c.rule == Rule::ExemptionHeadroom && c.clean)
        );
    }

    #[test]
    fn long_term_countdown_window() {
        let mut i = spread();
        i.holdings[0].long_term_soon = Some((45, 500.0, Some(25.0)));
        i.holdings[1].long_term_soon = Some((46, 500.0, Some(25.0)));
        i.holdings[2].long_term_soon = Some((10, -50.0, None));
        let c = evaluate(&i, vec![]);
        let f: Vec<&Finding> = c
            .findings
            .iter()
            .filter(|f| f.rule == Rule::LongTermSoon)
            .collect();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].inspect, "stockwise portfolio lots S0.NS");
    }

    #[test]
    fn bleed_drift_and_goals() {
        let mut i = spread();
        i.bleed = Some(vec![
            BleedFacts {
                symbol: "MF:100".into(),
                value: 200_000.0,
                annual_cost: 1_400.0,
                fx_to_base: 1.0,
            },
            BleedFacts {
                symbol: "MF:200".into(),
                value: 1.0,
                annual_cost: 0.0,
                fx_to_base: 1.0,
            },
        ]);
        i.drift = Some(vec![
            DriftFacts {
                key: "IT".into(),
                current_pct: 30.0,
                target_pct: 20.0,
                band_pct: 5.0,
            },
            DriftFacts {
                key: "S1.NS".into(),
                current_pct: 10.0,
                target_pct: 12.0,
                band_pct: 5.0,
            },
        ]);
        i.goals = Some(vec![
            GoalFacts {
                name: "home".into(),
                overdue: false,
                funded_pct: 40.0,
                monthly_sip: 10_000.0,
                gap: 600_000.0,
                currency: "INR".into(),
                fx_to_base: 1.0,
            },
            GoalFacts {
                name: "done".into(),
                overdue: false,
                funded_pct: 100.0,
                monthly_sip: 0.0,
                gap: 0.0,
                currency: "INR".into(),
                fx_to_base: 1.0,
            },
        ]);
        let c = evaluate(&i, vec![]);
        let count = |r| c.findings.iter().filter(|f| f.rule == r).count();
        assert_eq!(count(Rule::RegularPlanCost), 1);
        assert_eq!(count(Rule::TargetDrift), 1);
        assert_eq!(count(Rule::GoalBehind), 1);
        // The goal's gap is the largest exposure.
        assert_eq!(c.findings[0].rule, Rule::GoalBehind);
    }

    #[test]
    fn ranking_is_exposure_times_severity() {
        let mk = |sev, exposure, sym: &str| Finding {
            lens: Lens::Forensic,
            rule: Rule::WeakPiotroski,
            severity: sev,
            symbol: Some(sym.into()),
            observed: String::new(),
            exposure,
            at_stake: None,
            weight_pct: None,
            inspect: String::new(),
            impact: exposure * Severity::weight(sev),
        };
        let mut f = vec![
            mk(Severity::Low, 250.0, "A"),
            mk(Severity::High, 100.0, "B"),
            mk(Severity::Medium, 140.0, "C"),
        ];
        rank(&mut f);
        let order: Vec<&str> = f.iter().map(|f| f.symbol.as_deref().unwrap()).collect();
        assert_eq!(order, ["B", "C", "A"]);
    }

    #[test]
    fn pass_count_and_skips() {
        let mut i = spread();
        i.beta = Some(1.5);
        i.max_drawdown_pct = Some(-5.0);
        let c = evaluate(&i, vec!["scorecards: offline".into()]);
        assert_eq!(c.evaluated, c.checks.len());
        assert_eq!(c.clean, c.evaluated - 1);
        assert_eq!(c.skipped, ["scorecards: offline"]);
    }

    #[test]
    fn wording_is_neutral() {
        let mut i = with_fundamentals(Fundamentals {
            piotroski: Some((1, 9)),
            altman_distress: Some(true),
            pe: Some(90.0),
            revenue_decelerating: Some(true),
            ..Default::default()
        });
        i.beta = Some(2.0);
        let c = evaluate(&i, vec![]);
        for f in &c.findings {
            let t = f.observed.to_lowercase();
            for word in ["buy", "sell", "should", "recommend", "exit", "switch"] {
                assert!(!t.contains(word), "`{word}` in {}", f.observed);
            }
        }
    }

    fn period(end: &str, revenue: f64, net: f64, op: f64) -> crate::research::Period {
        crate::research::Period {
            end: NaiveDate::parse_from_str(end, "%Y-%m-%d").unwrap(),
            revenue: Some(revenue),
            net_income: Some(net),
            operating_income: Some(op),
            ..Default::default()
        }
    }

    #[test]
    fn statement_helpers() {
        let q = vec![
            period("2025-03-31", 90.0, 9.0, 18.0),
            period("2025-06-30", 100.0, 10.0, 20.0),
            period("2025-09-30", 100.0, 10.0, 20.0),
            period("2025-12-31", 100.0, 10.0, 20.0),
            period("2026-03-31", 110.0, 10.0, 20.0),
            period("2026-06-30", 105.0, 8.0, 15.75),
        ];
        let (pat, opm) = quarter_yoy(&q);
        assert!((pat.unwrap() + 20.0).abs() < 1e-9);
        assert!((opm.unwrap() + 5.0).abs() < 1e-9);
        let (yoy, decel) = revenue_trend(&q);
        assert!((yoy.unwrap() - 5.0).abs() < 1e-9);
        // 5% now against 22.2% the quarter before.
        assert_eq!(decel, Some(true));
        assert_eq!(quarter_yoy(&q[..4]), (None, None));
        let mut annual = vec![period("2025-03-31", 100.0, 10.0, 20.0)];
        annual[0].ocf = Some(6.0);
        assert!((cfo_to_pat(&annual).unwrap() - 0.6).abs() < 1e-12);
        annual[0].net_income = Some(-1.0);
        assert_eq!(cfo_to_pat(&annual), None);
        // A loss in the latest year gives no ratio, not an older year's.
        let mut older = period("2024-03-31", 100.0, 10.0, 20.0);
        older.ocf = Some(5.0);
        annual.insert(0, older);
        assert_eq!(cfo_to_pat(&annual), None);
    }

    #[test]
    fn unknown_cap_tiers_do_not_count_as_checked() {
        let mut i = spread();
        for x in &mut i.holdings {
            x.cap_tier = Some(CapTier::Unknown);
        }
        let c = evaluate(&i, vec![]);
        assert!(!c.checks.iter().any(|c| c.rule == Rule::SmallcapWeight));
        i.holdings[0].cap_tier = Some(CapTier::Large);
        let c = evaluate(&i, vec![]);
        assert!(c.checks.iter().any(|c| c.rule == Rule::SmallcapWeight));
        assert!(
            c.skipped[0].starts_with("Cap tier unknown for S1.NS"),
            "{:?}",
            c.skipped
        );
    }

    #[test]
    fn overlap_counts_only_when_funds_were_compared() {
        let mut i = spread();
        i.holdings[0].is_fund = true;
        i.holdings[1].is_fund = true;
        assert!(
            !evaluate(&i, vec![])
                .checks
                .iter()
                .any(|c| c.rule == Rule::FundOverlap)
        );
        i.funds_compared = 2;
        assert!(
            evaluate(&i, vec![])
                .checks
                .iter()
                .any(|c| c.rule == Rule::FundOverlap && c.clean)
        );
    }

    #[test]
    fn exposure_is_in_the_base_currency() {
        let mut i = spread();
        i.base_currency = "USD".into();
        i.bleed = Some(vec![BleedFacts {
            symbol: "MF:100".into(),
            value: 200_000.0,
            annual_cost: 1_400.0,
            fx_to_base: 0.012,
        }]);
        let c = evaluate(&i, vec![]);
        let f = &c.findings[0];
        assert!((f.exposure - 2_400.0).abs() < 1e-9);
        assert!((f.at_stake.unwrap() - 16.8).abs() < 1e-9);
        // The text keeps the rupee amounts AMFI reports.
        assert!(f.observed.contains("₹1,400"), "{}", f.observed);
    }
}
