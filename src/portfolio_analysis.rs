//! `portfolio analyze`: the figures behind the portfolio dashboard, from
//! plain inputs (no network, no files), so every number is testable.
//!
//! Per holding: weight, P&L, today's move, the gain split by tax term,
//! the next lot to turn long-term, where the price sits against its
//! 200-day average and 52-week range, volatility, drawdown and beta, and
//! the long-term score. Across the portfolio: the same figures grouped by
//! market, currency, cap tier, sector and tax term; concentration; risk of
//! the weighted portfolio; and observations, each with the numbers that
//! raised it.
//!
//! Everything here describes the holdings. Nothing is a call to buy, sell
//! or keep anything: observations state a reading and its figures.

use crate::market::CapTier;
use crate::tax::Country;
use chrono::NaiveDate;
use serde::Serialize;

/// Trading sessions in a year, for annualising daily figures.
const YEAR: f64 = 252.0;
/// A sector above this share of the portfolio is noted.
pub const SECTOR_LIMIT_PCT: f64 = 30.0;
/// Lots turning long-term within this many days are noted.
pub const LONG_TERM_SOON_DAYS: i64 = 45;
/// A holding this far below its cost (percent) is noted.
pub const LOSS_NOTE_PCT: f64 = 10.0;
/// A price this far below its 200-day average (percent) is noted.
pub const BELOW_200_DAY_PCT: f64 = 5.0;
/// A price this far below its high of the last year (percent) is noted.
pub const DRAWDOWN_NOTE_PCT: f64 = 25.0;
/// Within this share of a 52-week high or low (percent) counts as near it.
pub const NEAR_RANGE_PCT: f64 = 3.0;
/// A long-term score below this (with enough data) is noted.
pub const WEAK_SCORE: f64 = 45.0;
/// Daily closes needed before volatility, drawdown or beta are shown.
pub const MIN_CLOSES: usize = 60;

/// How long a holding's lots must be held to be long-term, for the
/// investor's country of residence (see `portfolio_analyze::term_rule`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermRule {
    /// The months [`Country::long_term_months`] gives for `tax_country`.
    #[default]
    CountryDefault,
    /// This many months (an Indian resident's foreign shares: 24).
    Months(u32),
    /// Never long-term: every gain is taxed at the slab rate (crypto).
    NoLongTerm,
    /// An Indian debt fund: units bought from 1 April 2023 are slab-taxed
    /// with no long-term term; older units are long-term after 24 months.
    DebtFund,
}

/// The day from which Indian debt-fund units have no long-term term
/// (Finance Act 2023, Sec. 50AA).
pub fn debt_fund_cutoff() -> NaiveDate {
    NaiveDate::from_ymd_opt(2023, 4, 1).expect("valid date")
}

impl TermRule {
    /// Months a lot bought on `bought` must be held to turn long-term, given
    /// the country's default; `None` when it never turns long-term.
    pub fn months_for(self, default: Option<u32>, bought: NaiveDate) -> Option<u32> {
        match self {
            TermRule::CountryDefault => default,
            TermRule::Months(m) => Some(m),
            TermRule::NoLongTerm => None,
            TermRule::DebtFund if bought >= debt_fund_cutoff() => None,
            TermRule::DebtFund => Some(24),
        }
    }

    /// Whether the holding has a tax term at all (gains are split into
    /// short and long term).
    pub fn has_terms(self, default: Option<u32>) -> bool {
        match self {
            TermRule::CountryDefault => default.is_some(),
            _ => true,
        }
    }
}

/// One purchase lot still held.
#[derive(Debug, Clone, PartialEq)]
pub struct LotInput {
    pub qty: f64,
    /// Price paid per share, in the holding's currency.
    pub price: f64,
    pub date: Option<NaiveDate>,
    /// The date was not known and is a guess.
    pub estimated: bool,
}

/// Everything known about one holding.
#[derive(Debug, Clone, Default)]
pub struct PositionInput {
    pub symbol: String,
    pub name: Option<String>,
    /// The market it trades on ("India", "United States", "United Kingdom").
    pub market: String,
    /// Its own currency (`INR`, `USD`, `GBp`).
    pub currency: Option<String>,
    pub cap_tier: Option<CapTier>,
    pub sector: Option<String>,
    pub shares: f64,
    /// Cost and value in the holding's own currency.
    pub cost: f64,
    pub value: Option<f64>,
    /// Base-currency value of one unit of the holding's currency (for
    /// totals); `None` without an exchange rate.
    pub fx_to_base: Option<f64>,
    pub price: Option<f64>,
    pub day_change_pct: Option<f64>,
    pub fifty_day_average: Option<f64>,
    pub two_hundred_day_average: Option<f64>,
    pub fifty_two_week_low: Option<f64>,
    pub fifty_two_week_high: Option<f64>,
    pub xirr_pct: Option<f64>,
    pub lots: Vec<LotInput>,
    /// Daily closes of about the last year by session date, oldest first.
    pub closes: Vec<(NaiveDate, f64)>,
    /// The long-term score (0-100) and the share of its inputs present.
    pub score: Option<(f64, f64)>,
    /// `Lower`, `Moderate` or `Higher`, from the long-term score.
    pub risk_tier: Option<String>,
    /// Whose holding-period rules apply (`None`: no long-term rate).
    pub tax_country: Option<Country>,
    /// The holding period for the investor's residence (see [`TermRule`]).
    pub term_rule: TermRule,
    /// Cost in the base currency at each purchase's own exchange rate (or
    /// what was paid in the base). `None`: `cost × fx_to_base` (today's rate).
    pub cost_base: Option<f64>,
    /// `closes` converted to the base currency at each day's rate; empty
    /// when the holding is in the base currency or no rates were found.
    pub closes_base: Vec<(NaiveDate, f64)>,
}

impl PositionInput {
    /// Closes in the base currency: `closes_base`, else `closes` when the
    /// holding is in the base currency (`fx_to_base` of 1).
    pub fn base_closes(&self) -> &[(NaiveDate, f64)] {
        if !self.closes_base.is_empty() {
            &self.closes_base
        } else if self.fx_to_base == Some(1.0) {
            &self.closes
        } else {
            &[]
        }
    }

    /// Cost in the base currency: at the purchases' rates when known, else
    /// at today's.
    pub fn base_cost(&self) -> Option<f64> {
        self.cost_base
            .or_else(|| self.fx_to_base.map(|r| self.cost * r))
    }
}

/// Settings that apply to the whole portfolio.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub today: NaiveDate,
    pub base_currency: String,
    /// The investor profile's limits, percent of the portfolio.
    pub max_stock_weight: Option<f64>,
    pub max_smallcap_weight: Option<f64>,
    /// The market's main index, daily closes by session date, oldest first.
    pub benchmark_closes: Vec<(NaiveDate, f64)>,
    pub benchmark_name: String,
}

/// One holding's figures.
#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub symbol: String,
    pub name: Option<String>,
    pub market: String,
    pub currency: Option<String>,
    pub cap_tier: &'static str,
    pub sector: Option<String>,
    pub shares: f64,
    pub cost: f64,
    pub value: Option<f64>,
    pub value_base: Option<f64>,
    /// Cost in the base currency at the purchases' own exchange rates
    /// (today's when those are unknown), alongside `value_base`.
    pub cost_base: Option<f64>,
    /// Part of the base-currency P&L from exchange-rate moves since the
    /// purchases: cost at today's rate less cost at the purchases' rates.
    pub fx_effect_base: Option<f64>,
    pub weight_pct: Option<f64>,
    pub pnl: Option<f64>,
    pub pnl_pct: Option<f64>,
    pub day_change_pct: Option<f64>,
    /// Today's move of the holding, in the base currency.
    pub day_change_base: Option<f64>,
    pub xirr_pct: Option<f64>,
    /// Days since the oldest lot still held was bought.
    pub days_held: Option<i64>,
    /// Unrealised gain of lots held less than / at least the long-term
    /// period, in the holding's currency.
    pub short_term_gain: Option<f64>,
    pub long_term_gain: Option<f64>,
    /// Dated lots held less than / at least the long-term period.
    pub short_term_lots: usize,
    pub long_term_lots: usize,
    /// The next lot to become long-term.
    pub next_long_term: Option<NextLongTerm>,
    pub vs_200_day_pct: Option<f64>,
    /// 0 at the 52-week low, 100 at the high.
    pub range_position_pct: Option<f64>,
    pub volatility_pct: Option<f64>,
    /// Fall from the highest close of the last year, percent.
    pub from_high_pct: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub beta: Option<f64>,
    pub score: Option<f64>,
    pub score_coverage: Option<f64>,
    pub risk_tier: Option<String>,
    pub estimated_dates: bool,
}

/// A lot about to turn long-term.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextLongTerm {
    pub date: NaiveDate,
    pub days: i64,
    pub qty: f64,
    /// That lot's unrealised gain, holding currency.
    pub gain: Option<f64>,
    /// Tax on that gain at the short-term rate minus at the long-term
    /// rate, holding currency (positive gains only).
    pub rate_difference: Option<f64>,
}

/// A slice of the portfolio (a market, currency, tier, sector or term).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    pub name: String,
    pub holdings: usize,
    /// Value in the base currency and share of the total.
    pub value_base: f64,
    pub weight_pct: f64,
    pub pnl_base: f64,
    /// The value in the group's own currency, when every holding in it
    /// uses the same one.
    pub local_value: Option<f64>,
    pub local_currency: Option<String>,
}

/// How spread out the portfolio is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Concentration {
    pub largest_pct: f64,
    pub largest: Option<String>,
    pub top5_pct: f64,
    /// Sum of squared weights (0-1): 1 is a single holding.
    pub herfindahl: f64,
    /// 1 / Herfindahl: the number of equal holdings with the same spread.
    pub effective_holdings: f64,
}

/// Risk of the portfolio as weighted today, from its holdings' closes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Risk {
    /// Share of the value with enough price history behind the figures.
    pub coverage_pct: f64,
    pub sessions: usize,
    pub volatility_pct: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub beta: Option<f64>,
    pub benchmark: String,
    /// Weighted mean of the holdings' long-term scores.
    pub weighted_score: Option<f64>,
}

/// Unrealised gains by tax term, base currency.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Terms {
    pub short_term_gain: f64,
    pub long_term_gain: f64,
    pub short_term_lots: usize,
    pub long_term_lots: usize,
    /// Lots whose purchase date is unknown.
    pub undated_lots: usize,
}

/// How notable an observation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Worth a look: a profile limit, a large loss, a large fall.
    Notice,
    /// For information.
    Info,
}

/// One reading, with the figures that raised it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Observation {
    pub kind: &'static str,
    pub level: Level,
    pub symbol: Option<String>,
    pub text: String,
}

/// The whole dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub base_currency: String,
    pub value: f64,
    pub cost: f64,
    pub pnl: f64,
    pub pnl_pct: f64,
    pub day_change: f64,
    pub day_change_pct: Option<f64>,
    pub positions: Vec<Position>,
    pub by_market: Vec<Group>,
    pub by_currency: Vec<Group>,
    pub by_cap_tier: Vec<Group>,
    pub by_sector: Vec<Group>,
    pub terms: Terms,
    pub concentration: Concentration,
    pub risk: Risk,
    pub observations: Vec<Observation>,
    /// Holdings with no price or no exchange rate, left out of totals.
    pub left_out: Vec<String>,
}

fn pct(part: f64, whole: f64) -> f64 {
    if whole != 0.0 {
        part / whole * 100.0
    } else {
        0.0
    }
}

fn returns(closes: &[f64]) -> Vec<f64> {
    closes
        .windows(2)
        .filter(|w| w[0] > 0.0)
        .map(|w| w[1] / w[0] - 1.0)
        .collect()
}

fn volatility(rets: &[f64]) -> Option<f64> {
    if rets.len() + 1 < MIN_CLOSES {
        return None;
    }
    let n = rets.len() as f64;
    let mean = rets.iter().sum::<f64>() / n;
    let var = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Some(var.sqrt() * YEAR.sqrt() * 100.0)
}

/// Largest fall from a running peak, percent (positive).
fn max_drawdown(closes: &[f64]) -> Option<f64> {
    if closes.len() < MIN_CLOSES {
        return None;
    }
    let mut peak = f64::MIN;
    let mut worst = 0.0_f64;
    for &c in closes {
        peak = peak.max(c);
        if peak > 0.0 {
            worst = worst.max(1.0 - c / peak);
        }
    }
    Some(worst * 100.0)
}

/// Closes of the sessions both series have, in date order.
pub(crate) fn join(a: &[(NaiveDate, f64)], b: &[(NaiveDate, f64)]) -> (Vec<f64>, Vec<f64>) {
    let theirs: std::collections::HashMap<NaiveDate, f64> = b.iter().copied().collect();
    a.iter()
        .filter_map(|(d, x)| theirs.get(d).map(|y| (*x, *y)))
        .unzip()
}

/// Beta of `asset` against `bench`, on the sessions they share.
fn beta(asset: &[(NaiveDate, f64)], bench: &[(NaiveDate, f64)]) -> Option<f64> {
    let (asset, bench) = join(asset, bench);
    if asset.len() < MIN_CLOSES {
        return None;
    }
    let a = returns(&asset);
    let b = returns(&bench);
    let m = a.len().min(b.len());
    if m < 2 {
        return None;
    }
    let (a, b) = (&a[..m], &b[..m]);
    let ma = a.iter().sum::<f64>() / m as f64;
    let mb = b.iter().sum::<f64>() / m as f64;
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let var: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    (var > 0.0).then(|| cov / var)
}

/// The figures of one holding.
fn position(p: &PositionInput, ctx: &Context, total_base: f64) -> Position {
    let value_base = p.value.zip(p.fx_to_base).map(|(v, r)| v * r);
    let pnl = p.value.map(|v| v - p.cost);
    let default_months = p.tax_country.and_then(Country::long_term_months);
    let long_term_on = |d: NaiveDate, m: u32| crate::sellpreview::long_term_from(d, m);
    let (mut st, mut lt, mut any_term) = (0.0, 0.0, false);
    let (mut st_lots, mut lt_lots) = (0, 0);
    let mut next: Option<NextLongTerm> = None;
    if let Some(price) = p.price.filter(|_| p.term_rule.has_terms(default_months)) {
        for lot in &p.lots {
            let Some(d) = lot.date else { continue };
            let gain = lot.qty * (price - lot.price);
            any_term = true;
            let Some(m) = p.term_rule.months_for(default_months, d) else {
                // Never long-term: slab-taxed.
                st += gain;
                st_lots += 1;
                continue;
            };
            match long_term_on(d, m) {
                Some(on) if on > ctx.today => {
                    st += gain;
                    st_lots += 1;
                    let days = (on - ctx.today).num_days();
                    if next.as_ref().is_none_or(|n| on < n.date) {
                        let diff = p
                            .tax_country
                            .map(|c| gain.max(0.0) * (c.st_rate() - c.lt_rate()) / 100.0);
                        next = Some(NextLongTerm {
                            date: on,
                            days,
                            qty: lot.qty,
                            gain: Some(gain),
                            rate_difference: diff,
                        });
                    }
                }
                _ => {
                    lt += gain;
                    lt_lots += 1;
                }
            }
        }
    }
    let oldest = p.lots.iter().filter_map(|l| l.date).min();
    let closes: Vec<f64> = p.closes.iter().map(|(_, c)| *c).collect();
    let closes = &closes;
    let enough = closes.len() >= MIN_CLOSES;
    let year_high = enough.then(|| closes.iter().copied().fold(f64::MIN, f64::max));
    Position {
        symbol: p.symbol.clone(),
        name: p.name.clone(),
        market: p.market.clone(),
        currency: p.currency.clone(),
        cap_tier: p.cap_tier.unwrap_or(CapTier::Unknown).as_str(),
        sector: p.sector.clone(),
        shares: p.shares,
        cost: p.cost,
        value: p.value,
        value_base,
        cost_base: value_base.and(p.base_cost()),
        fx_effect_base: value_base
            .and(p.fx_to_base)
            .zip(p.cost_base)
            .map(|(r, hist)| p.cost * r - hist),
        weight_pct: value_base.map(|v| pct(v, total_base)),
        pnl,
        pnl_pct: pnl.map(|g| pct(g, p.cost)),
        day_change_pct: p.day_change_pct,
        day_change_base: value_base
            .zip(p.day_change_pct)
            .map(|(v, c)| v - v / (1.0 + c / 100.0)),
        xirr_pct: p.xirr_pct,
        days_held: oldest.map(|d| (ctx.today - d).num_days()),
        short_term_gain: any_term.then_some(st),
        long_term_gain: any_term.then_some(lt),
        short_term_lots: st_lots,
        long_term_lots: lt_lots,
        next_long_term: next,
        vs_200_day_pct: p
            .price
            .zip(p.two_hundred_day_average.filter(|a| *a > 0.0))
            .map(|(px, a)| (px / a - 1.0) * 100.0),
        range_position_pct: match (p.price, p.fifty_two_week_low, p.fifty_two_week_high) {
            (Some(px), Some(lo), Some(hi)) if hi > lo => {
                Some(((px - lo) / (hi - lo) * 100.0).clamp(0.0, 100.0))
            }
            _ => None,
        },
        volatility_pct: volatility(&returns(closes)),
        from_high_pct: year_high
            .zip(closes.last())
            .filter(|(h, _)| *h > 0.0)
            .map(|(h, c)| (1.0 - c / h) * 100.0),
        max_drawdown_pct: max_drawdown(closes),
        // Against a benchmark in the base currency, so in the base too.
        beta: beta(
            if p.base_closes().is_empty() {
                &p.closes
            } else {
                p.base_closes()
            },
            &ctx.benchmark_closes,
        ),
        score: p.score.map(|s| s.0),
        score_coverage: p.score.map(|s| s.1),
        risk_tier: p.risk_tier.clone(),
        estimated_dates: p.lots.iter().any(|l| l.estimated),
    }
}

/// Group positions by `key`, largest first.
fn group(positions: &[Position], total: f64, key: impl Fn(&Position) -> String) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for p in positions {
        let Some(vb) = p.value_base else { continue };
        let name = key(p);
        // Value less cost, both at today's rate: a holding worth nothing
        // still counts its whole cost as a loss.
        let pnl_base = p.cost_base.map_or(0.0, |c| vb - c);
        let g = match out.iter_mut().find(|g| g.name == name) {
            Some(g) => g,
            None => {
                out.push(Group {
                    name,
                    holdings: 0,
                    value_base: 0.0,
                    weight_pct: 0.0,
                    pnl_base: 0.0,
                    local_value: Some(0.0),
                    local_currency: p.currency.clone(),
                });
                out.last_mut().expect("just pushed")
            }
        };
        g.holdings += 1;
        g.value_base += vb;
        g.pnl_base += pnl_base;
        if g.local_currency == p.currency {
            g.local_value = g.local_value.zip(p.value).map(|(a, b)| a + b);
        } else {
            g.local_value = None;
            g.local_currency = None;
        }
    }
    for g in &mut out {
        g.weight_pct = pct(g.value_base, total);
        if g.local_currency.is_none() {
            g.local_value = None;
        }
    }
    out.sort_by(|a, b| {
        b.value_base
            .total_cmp(&a.value_base)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn concentration(positions: &[Position]) -> Concentration {
    let mut w: Vec<(f64, &str)> = positions
        .iter()
        .filter_map(|p| p.weight_pct.map(|w| (w, p.symbol.as_str())))
        .collect();
    w.sort_by(|a, b| b.0.total_cmp(&a.0));
    let hhi: f64 = w.iter().map(|(x, _)| (x / 100.0).powi(2)).sum();
    Concentration {
        largest_pct: w.first().map_or(0.0, |x| x.0),
        largest: w.first().map(|x| x.1.to_string()),
        top5_pct: w.iter().take(5).map(|x| x.0).sum(),
        herfindahl: hhi,
        effective_holdings: if hhi > 0.0 { 1.0 / hhi } else { 0.0 },
    }
}

/// The portfolio's daily value index, each holding weighted by today's
/// base-currency value: the risk of the portfolio as it stands, not as it
/// was held. Closes are in the base currency (so currency moves count) and
/// sit on the union of every holding's sessions: each day's return is the
/// weighted return of the holdings priced on that day and the day before,
/// so a holding with a short history or another market's holidays only
/// leaves out its own missing days instead of cutting every series to the
/// shortest common window.
fn risk(inputs: &[PositionInput], positions: &[Position], ctx: &Context, total: f64) -> Risk {
    let with: Vec<(&[(NaiveDate, f64)], f64)> = inputs
        .iter()
        .zip(positions)
        .filter_map(|(i, p)| {
            let closes = i.base_closes();
            (closes.len() >= MIN_CLOSES)
                .then_some(closes)
                .zip(p.value_base.filter(|v| *v > 0.0))
        })
        .collect();
    let covered: f64 = with.iter().map(|(_, v)| v).sum();
    let index = weighted_index(&with);
    let values: Vec<f64> = index.iter().map(|(_, v)| *v).collect();
    let scored: Vec<(f64, f64)> = positions
        .iter()
        .filter_map(|p| p.score.zip(p.value_base))
        .collect();
    let score_weight: f64 = scored.iter().map(|(_, w)| w).sum();
    Risk {
        coverage_pct: pct(covered, total),
        sessions: index.len(),
        volatility_pct: volatility(&returns(&values)),
        max_drawdown_pct: max_drawdown(&values),
        beta: beta(&index, &ctx.benchmark_closes),
        benchmark: ctx.benchmark_name.clone(),
        weighted_score: (score_weight > 0.0)
            .then(|| scored.iter().map(|(s, w)| s * w).sum::<f64>() / score_weight),
    }
}

/// A growth-of-100 index of `series` (dated closes, each with a fixed
/// weight) on the union of their dates. Each day's return is the
/// weight-averaged return of the series with a close on that day and an
/// earlier one (forward-filled across their own gaps).
pub fn weighted_index(series: &[(&[(NaiveDate, f64)], f64)]) -> Vec<(NaiveDate, f64)> {
    let mut calendar: Vec<NaiveDate> = series
        .iter()
        .flat_map(|(s, _)| s.iter().map(|(d, _)| *d))
        .collect();
    calendar.sort();
    calendar.dedup();
    let aligned: Vec<Vec<Option<f64>>> = series
        .iter()
        .map(|(s, _)| {
            let mut sorted = s.to_vec();
            sorted.sort_by_key(|p| p.0);
            crate::benchmark::align(&calendar, &sorted)
        })
        .collect();
    let mut index: Vec<(NaiveDate, f64)> = Vec::new();
    for t in 1..calendar.len() {
        let (mut num, mut den) = (0.0, 0.0);
        for ((_, w), a) in series.iter().zip(&aligned) {
            if let (Some(p0), Some(p1)) = (a[t - 1], a[t]) {
                num += w * (p1 / p0 - 1.0);
                den += w;
            }
        }
        if den <= 0.0 {
            continue;
        }
        if index.is_empty() {
            index.push((calendar[t - 1], 100.0));
        }
        let last = index.last().expect("just pushed").1;
        index.push((calendar[t], last * (1.0 + num / den)));
    }
    index
}

/// A benchmark blended from several indices (closes in the base currency),
/// each with a fixed weight, e.g. NIFTY 50 and the S&P 500 in the shares of
/// a portfolio's Indian and US holdings. Daily returns are blended on the
/// union of their sessions.
pub fn blend_benchmarks(parts: &[(Vec<(NaiveDate, f64)>, f64)]) -> Vec<(NaiveDate, f64)> {
    let refs: Vec<(&[(NaiveDate, f64)], f64)> = parts
        .iter()
        .filter(|(s, w)| !s.is_empty() && *w > 0.0)
        .map(|(s, w)| (s.as_slice(), *w))
        .collect();
    weighted_index(&refs)
}

fn money(v: f64, cur: Option<&str>) -> String {
    crate::display::format_price(v, cur)
}

/// The observations, notices first, then by symbol.
fn observations(a: &Analysis, ctx: &Context) -> Vec<Observation> {
    let mut out = Vec::new();
    let mut add = |kind: &'static str, level: Level, symbol: Option<&str>, text: String| {
        out.push(Observation {
            kind,
            level,
            symbol: symbol.map(str::to_string),
            text,
        })
    };
    for p in &a.positions {
        let sym = Some(p.symbol.as_str());
        let cur = p.currency.as_deref();
        if let (Some(w), Some(max)) = (p.weight_pct, ctx.max_stock_weight)
            && w > max
        {
            add(
                "above_profile_stock_limit",
                Level::Notice,
                sym,
                format!(
                    "{:.1}% of the portfolio; the profile's limit for one stock is {:.0}%.",
                    w, max
                ),
            );
        }
        if let (Some(g), Some(pc)) = (p.pnl, p.pnl_pct)
            && pc <= -LOSS_NOTE_PCT
        {
            add(
                "below_cost",
                Level::Notice,
                sym,
                format!(
                    "{:.1}% below cost, an unrealised loss of {}; a realised loss can be set off against gains of the same year.",
                    -pc,
                    money(-g, cur)
                ),
            );
        }
        if let Some(n) = &p.next_long_term
            && n.days <= LONG_TERM_SOON_DAYS
            && n.gain.is_some_and(|g| g > 0.0)
        {
            let mut text = format!(
                "{} share(s) turn long-term on {} (in {} days); their gain of {} is short-term until then",
                fmt_qty(n.qty),
                n.date.format("%d %b %Y"),
                n.days,
                money(n.gain.unwrap_or(0.0), cur)
            );
            if let Some(d) = n.rate_difference.filter(|d| *d > 0.0) {
                text.push_str(&format!(
                    "; at the standard rates the tax on it differs by about {}",
                    money(d, cur)
                ));
            }
            text.push('.');
            add("turns_long_term_soon", Level::Info, sym, text);
        }
        if let Some(f) = p.from_high_pct
            && f >= DRAWDOWN_NOTE_PCT
        {
            add(
                "far_below_year_high",
                Level::Notice,
                sym,
                format!("{:.1}% below its highest close of the last year.", f),
            );
        }
        if let Some(d) = p.vs_200_day_pct
            && d <= -BELOW_200_DAY_PCT
        {
            add(
                "below_200_day_average",
                Level::Info,
                sym,
                format!("{:.1}% below its 200-day average.", -d),
            );
        }
        if let Some(r) = p.range_position_pct {
            if r >= 100.0 - NEAR_RANGE_PCT {
                add(
                    "near_52_week_high",
                    Level::Info,
                    sym,
                    "At or near its 52-week high.".to_string(),
                );
            } else if r <= NEAR_RANGE_PCT {
                add(
                    "near_52_week_low",
                    Level::Info,
                    sym,
                    "At or near its 52-week low.".to_string(),
                );
            }
        }
        if let (Some(s), Some(c)) = (p.score, p.score_coverage)
            && s < WEAK_SCORE
            && c >= 0.5
        {
            add(
                "weak_long_term_score",
                Level::Info,
                sym,
                format!(
                    "Long-term score {:.0} of 100 ({:.0}% of its inputs present).",
                    s,
                    c * 100.0
                ),
            );
        }
        if p.estimated_dates {
            add(
                "estimated_purchase_date",
                Level::Info,
                sym,
                "A purchase date is estimated, so the tax term and holding period may be off; `portfolio lots` sets it.".to_string(),
            );
        }
    }
    let small: f64 = a
        .by_cap_tier
        .iter()
        .filter(|g| g.name == "small" || g.name == "micro")
        .map(|g| g.weight_pct)
        .sum();
    if let Some(max) = ctx.max_smallcap_weight
        && small > max
    {
        add(
            "above_profile_smallcap_limit",
            Level::Notice,
            None,
            format!(
                "Small and micro caps are {:.1}% of the portfolio; the profile's limit is {:.0}%.",
                small, max
            ),
        );
    }
    for g in &a.by_sector {
        if g.name != UNKNOWN_SECTOR && g.weight_pct > SECTOR_LIMIT_PCT {
            add(
                "sector_concentration",
                Level::Notice,
                None,
                format!(
                    "{} is {:.1}% of the portfolio ({} holdings), above {:.0}%.",
                    g.name, g.weight_pct, g.holdings, SECTOR_LIMIT_PCT
                ),
            );
        }
    }
    if a.by_currency.len() > 1
        && let Some(g) = a.by_currency.iter().find(|g| g.name != a.base_currency)
    {
        let foreign: f64 = a
            .by_currency
            .iter()
            .filter(|g| g.name != a.base_currency)
            .map(|g| g.weight_pct)
            .sum();
        add(
            "currency_exposure",
            Level::Info,
            None,
            format!(
                "{:.1}% of the value is in currencies other than {} (largest: {}); exchange-rate moves change it in {}.",
                foreign, a.base_currency, g.name, a.base_currency
            ),
        );
    }
    if !a.left_out.is_empty() {
        add(
            "left_out_of_totals",
            Level::Info,
            None,
            format!(
                "Left out of the totals (no price or exchange rate): {}.",
                a.left_out.join(", ")
            ),
        );
    }
    out.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.symbol.cmp(&b.symbol)));
    out
}

fn fmt_qty(q: f64) -> String {
    if q.fract() == 0.0 {
        format!("{:.0}", q)
    } else {
        format!("{:.3}", q)
    }
}

/// Sector name for holdings without one.
pub const UNKNOWN_SECTOR: &str = "Unclassified";

/// Analyse the holdings.
pub fn analyze(inputs: &[PositionInput], ctx: &Context) -> Analysis {
    let total: f64 = inputs
        .iter()
        .filter_map(|p| p.value.zip(p.fx_to_base).map(|(v, r)| v * r))
        .sum();
    let positions: Vec<Position> = inputs.iter().map(|p| position(p, ctx, total)).collect();
    let cost: f64 = positions
        .iter()
        .filter(|p| p.value_base.is_some())
        .filter_map(|p| p.cost_base)
        .sum();
    let day_change: f64 = positions.iter().filter_map(|p| p.day_change_base).sum();
    let left_out: Vec<String> = positions
        .iter()
        .filter(|p| p.value_base.is_none())
        .map(|p| p.symbol.clone())
        .collect();
    let to_base = |p: &Position, i: &PositionInput, x: Option<f64>| {
        x.zip(i.fx_to_base)
            .map(|(v, r)| v * r)
            .filter(|_| p.value_base.is_some())
    };
    let mut terms = Terms::default();
    for (p, i) in positions.iter().zip(inputs) {
        terms.short_term_gain += to_base(p, i, p.short_term_gain).unwrap_or(0.0);
        terms.long_term_gain += to_base(p, i, p.long_term_gain).unwrap_or(0.0);
        let default_months = i.tax_country.and_then(Country::long_term_months);
        if i.term_rule.has_terms(default_months) {
            for l in &i.lots {
                let Some(d) = l.date else {
                    terms.undated_lots += 1;
                    continue;
                };
                match i
                    .term_rule
                    .months_for(default_months, d)
                    .map(|m| crate::sellpreview::long_term_from(d, m))
                {
                    None => terms.short_term_lots += 1,
                    Some(None) => terms.undated_lots += 1,
                    Some(Some(on)) if on > ctx.today => terms.short_term_lots += 1,
                    Some(Some(_)) => terms.long_term_lots += 1,
                }
            }
        }
    }
    let mut a = Analysis {
        base_currency: ctx.base_currency.clone(),
        value: total,
        cost,
        pnl: total - cost,
        pnl_pct: pct(total - cost, cost),
        day_change,
        day_change_pct: (total - day_change > 0.0).then(|| pct(day_change, total - day_change)),
        by_market: group(&positions, total, |p| p.market.clone()),
        by_currency: group(&positions, total, |p| {
            p.currency.clone().unwrap_or_else(|| "?".to_string())
        }),
        by_cap_tier: group(&positions, total, |p| p.cap_tier.to_string()),
        by_sector: group(&positions, total, |p| {
            p.sector
                .clone()
                .unwrap_or_else(|| UNKNOWN_SECTOR.to_string())
        }),
        terms,
        concentration: concentration(&positions),
        risk: risk(inputs, &positions, ctx, total),
        observations: Vec::new(),
        left_out,
        positions,
    };
    a.observations = observations(&a, ctx);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Closes on consecutive days from 1 Jan 2025.
    fn dated(v: Vec<f64>) -> Vec<(NaiveDate, f64)> {
        v.into_iter()
            .enumerate()
            .map(|(i, c)| (day("2025-01-01") + chrono::Days::new(i as u64), c))
            .collect()
    }

    fn ctx() -> Context {
        Context {
            today: day("2026-11-15"),
            base_currency: "INR".into(),
            max_stock_weight: Some(20.0),
            max_smallcap_weight: Some(25.0),
            benchmark_closes: Vec::new(),
            benchmark_name: "NIFTY 50".into(),
        }
    }

    fn holding(sym: &str, shares: f64, cost: f64, price: f64) -> PositionInput {
        PositionInput {
            symbol: sym.into(),
            market: "India".into(),
            currency: Some("INR".into()),
            cap_tier: Some(CapTier::Large),
            sector: Some("Technology".into()),
            shares,
            cost: shares * cost,
            value: Some(shares * price),
            fx_to_base: Some(1.0),
            price: Some(price),
            lots: vec![LotInput {
                qty: shares,
                price: cost,
                date: Some(day("2024-01-10")),
                estimated: false,
            }],
            tax_country: Some(Country::India),
            ..Default::default()
        }
    }

    #[test]
    fn totals_weights_and_groups_in_the_base_currency() {
        let mut us = holding("AAPL", 10.0, 100.0, 150.0);
        us.market = "United States".into();
        us.currency = Some("USD".into());
        us.fx_to_base = Some(80.0);
        us.sector = Some("Technology".into());
        let mut bank = holding("HDFCBANK.NS", 100.0, 1500.0, 1600.0);
        bank.sector = Some("Financial Services".into());
        bank.day_change_pct = Some(1.0);
        let mut unpriced = holding("OLD.NS", 1.0, 10.0, 10.0);
        unpriced.value = None;
        unpriced.price = None;
        let a = analyze(&[us, bank, unpriced], &ctx());
        // 10 × 150 × 80 = 120,000 and 100 × 1,600 = 160,000.
        assert!((a.value - 280_000.0).abs() < 1e-6);
        assert!((a.cost - (80_000.0 + 150_000.0)).abs() < 1e-6);
        assert_eq!(a.left_out, ["OLD.NS"]);
        let w = |s: &str| {
            a.positions
                .iter()
                .find(|p| p.symbol == s)
                .unwrap()
                .weight_pct
                .unwrap()
        };
        assert!((w("HDFCBANK.NS") - 160.0 / 2.8).abs() < 1e-9);
        // Markets keep their own currency's total.
        let us_group = a
            .by_market
            .iter()
            .find(|g| g.name == "United States")
            .unwrap();
        assert_eq!(us_group.local_value, Some(1500.0));
        assert_eq!(us_group.local_currency.as_deref(), Some("USD"));
        assert!((us_group.pnl_base - 40_000.0).abs() < 1e-6);
        assert_eq!(a.by_currency.len(), 2);
        // Today's move: 160,000 is 1% above yesterday's 158,415.84.
        assert!((a.day_change - (160_000.0 - 160_000.0 / 1.01)).abs() < 1e-6);
        assert!(a.observations.iter().any(|o| o.kind == "currency_exposure"));
        assert!(
            a.observations
                .iter()
                .any(|o| o.kind == "left_out_of_totals")
        );
    }

    #[test]
    fn tax_terms_split_lots_and_find_the_next_to_turn_long_term() {
        let mut h = holding("TCS.NS", 15.0, 100.0, 200.0);
        h.lots = vec![
            LotInput {
                qty: 10.0,
                price: 100.0,
                date: Some(day("2024-01-10")),
                estimated: false,
            },
            LotInput {
                qty: 5.0,
                price: 100.0,
                date: Some(day("2025-12-01")),
                estimated: false,
            },
        ];
        let a = analyze(&[h], &ctx());
        let p = &a.positions[0];
        assert_eq!(p.long_term_gain, Some(1000.0));
        assert_eq!(p.short_term_gain, Some(500.0));
        let n = p.next_long_term.as_ref().unwrap();
        assert_eq!(n.qty, 5.0);
        // Long-term from the day after 12 months: 2 Dec 2026.
        assert_eq!(n.date, day("2026-12-02"));
        assert_eq!(n.days, 17);
        // (20% - 12.5%) of 500.
        assert!((n.rate_difference.unwrap() - 37.5).abs() < 1e-9);
        assert_eq!((a.terms.short_term_lots, a.terms.long_term_lots), (1, 1));
        assert!(
            a.observations
                .iter()
                .any(|o| o.kind == "turns_long_term_soon")
        );
        assert_eq!(
            p.days_held,
            Some((day("2026-11-15") - day("2024-01-10")).num_days())
        );
    }

    #[test]
    fn concentration_and_profile_limits() {
        let a = analyze(
            &[
                holding("A.NS", 1.0, 50.0, 60.0),
                holding("B.NS", 1.0, 30.0, 20.0),
                holding("C.NS", 1.0, 20.0, 20.0),
            ],
            &ctx(),
        );
        let c = &a.concentration;
        assert_eq!(c.largest.as_deref(), Some("A.NS"));
        assert!((c.largest_pct - 60.0).abs() < 1e-9);
        // 0.6² + 0.2² + 0.2² = 0.44.
        assert!((c.herfindahl - 0.44).abs() < 1e-9);
        assert!((c.effective_holdings - 1.0 / 0.44).abs() < 1e-9);
        let kinds: Vec<&str> = a.observations.iter().map(|o| o.kind).collect();
        assert!(kinds.contains(&"above_profile_stock_limit"));
        assert!(kinds.contains(&"below_cost"), "{kinds:?}");
        assert!(kinds.contains(&"sector_concentration"));
        // Notices come before information.
        assert!(a.observations.windows(2).all(|w| w[0].level <= w[1].level));
    }

    #[test]
    fn risk_of_the_weighted_portfolio_and_each_holding() {
        // A steady climb and a choppy series; the benchmark moves with the first.
        let up: Vec<f64> = (0..120).map(|i| 100.0 * 1.002_f64.powi(i)).collect();
        let chop: Vec<f64> = (0..120)
            .map(|i| 100.0 + if i % 2 == 0 { 3.0 } else { -3.0 })
            .collect();
        let mut a_in = holding("UP.NS", 10.0, 100.0, *up.last().unwrap());
        a_in.closes = dated(up.clone());
        let mut b_in = holding("CHOP.NS", 10.0, 100.0, *chop.last().unwrap());
        b_in.closes = dated(chop);
        let mut c = ctx();
        c.benchmark_closes = dated(up.iter().map(|x| x * 2.0).collect());
        let a = analyze(&[a_in, b_in], &c);
        let up_pos = &a.positions[0];
        assert!(up_pos.volatility_pct.unwrap() < 1.0);
        // Same daily returns as the benchmark: a beta of 1.
        assert!((up_pos.beta.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(up_pos.max_drawdown_pct, Some(0.0));
        assert!(a.positions[1].volatility_pct.unwrap() > 50.0);
        assert!((a.risk.coverage_pct - 100.0).abs() < 1e-9);
        assert_eq!(a.risk.sessions, 120);
        assert!(a.risk.volatility_pct.unwrap() > 0.0);
        // Too little history: no figures.
        let mut short = holding("NEW.NS", 1.0, 1.0, 1.0);
        short.closes = dated(vec![1.0; 10]);
        let b = analyze(&[short], &ctx());
        assert_eq!(b.positions[0].volatility_pct, None);
        assert_eq!(b.risk.volatility_pct, None);
    }

    #[test]
    fn histories_are_joined_by_date_not_by_position() {
        // B misses every 10th session (a holiday elsewhere). Joined by date,
        // A against B is a perfect match: beta 1. By position it would not be.
        let a: Vec<f64> = (0..150)
            .map(|i| 100.0 + (i as f64 * 0.7).sin() * 5.0 + i as f64 * 0.1)
            .collect();
        let a_dated = dated(a);
        let b_dated: Vec<(NaiveDate, f64)> = a_dated
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 10 != 5)
            .map(|(_, (d, c))| (*d, c * 3.0))
            .collect();
        let mut h = holding("A.NS", 1.0, 100.0, 100.0);
        h.closes = a_dated.clone();
        let mut c = ctx();
        c.benchmark_closes = b_dated.clone();
        let x = analyze(&[h.clone()], &c);
        assert!(
            (x.positions[0].beta.unwrap() - 1.0).abs() < 1e-9,
            "{:?}",
            x.positions[0].beta
        );
        // Two holdings: every session either has counts; a day B misses
        // leaves only B out of that day's return.
        let mut other = holding("B.NS", 1.0, 100.0, 100.0);
        other.closes = b_dated;
        let y = analyze(&[h, other], &ctx());
        assert_eq!(y.risk.sessions, 150);
    }

    #[test]
    fn a_short_history_does_not_cut_the_long_one() {
        // A has a year of closes, NEW only the last 70 sessions. The
        // portfolio index still spans the year.
        let long = dated((0..250).map(|i| 100.0 + i as f64 * 0.2).collect());
        let short: Vec<(NaiveDate, f64)> = long[180..].iter().map(|(d, c)| (*d, c * 2.0)).collect();
        let mut a = holding("A.NS", 1.0, 100.0, 150.0);
        a.closes = long;
        let mut b = holding("NEW.NS", 1.0, 100.0, 300.0);
        b.closes = short;
        let x = analyze(&[a, b], &ctx());
        assert_eq!(x.risk.sessions, 250);
        assert!((x.risk.coverage_pct - 100.0).abs() < 1e-9);
    }

    #[test]
    fn risk_uses_closes_in_the_base_currency() {
        let mut us = holding("AAPL", 1.0, 100.0, 100.0);
        us.currency = Some("USD".into());
        us.fx_to_base = Some(84.0);
        us.closes = dated(vec![100.0; 80]);
        // Without base closes a foreign holding is left out of risk.
        let a = analyze(std::slice::from_ref(&us), &ctx());
        assert_eq!(a.risk.coverage_pct, 0.0);
        // A flat price with a moving rupee is not flat in rupees.
        us.closes_base = dated((0..80).map(|i| 100.0 * (80.0 + i as f64 * 0.05)).collect());
        let b = analyze(&[us], &ctx());
        assert!((b.risk.coverage_pct - 100.0).abs() < 1e-9);
        assert!(b.risk.max_drawdown_pct == Some(0.0));
        assert_eq!(b.risk.sessions, 80);
    }

    #[test]
    fn blended_benchmark_weights_daily_returns() {
        let a = dated(vec![100.0, 110.0, 110.0]);
        let b = dated(vec![100.0, 100.0, 90.0]);
        let blend = blend_benchmarks(&[(a, 0.5), (b, 0.5)]);
        let v: Vec<f64> = blend.iter().map(|(_, v)| *v).collect();
        assert_eq!(v.len(), 3);
        assert!((v[1] - 105.0).abs() < 1e-9);
        assert!((v[2] - 105.0 * 0.95).abs() < 1e-9);
        // Empty parts are ignored.
        assert_eq!(blend_benchmarks(&[(Vec::new(), 1.0)]), Vec::new());
    }

    #[test]
    fn an_indian_residents_us_lot_held_18_months_is_short_term() {
        // Bought 18 months before `today` (2026-11-15).
        let mut us = holding("AAPL", 10.0, 100.0, 150.0);
        us.currency = Some("USD".into());
        us.fx_to_base = Some(84.0);
        us.tax_country = Some(Country::India);
        us.term_rule = TermRule::Months(24);
        us.lots[0].date = Some(day("2025-05-15"));
        let a = analyze(std::slice::from_ref(&us), &ctx());
        let p = &a.positions[0];
        assert_eq!((p.short_term_lots, p.long_term_lots), (1, 0));
        assert_eq!(p.long_term_gain, Some(0.0));
        assert_eq!(p.next_long_term.as_ref().unwrap().date, day("2027-05-16"));
        assert_eq!((a.terms.short_term_lots, a.terms.long_term_lots), (1, 0));
        // Under the 12-month rule it would have been long-term.
        us.term_rule = TermRule::CountryDefault;
        let b = analyze(&[us], &ctx());
        assert_eq!(b.positions[0].long_term_lots, 1);
    }

    #[test]
    fn debt_fund_units_from_april_2023_are_never_long_term() {
        let mut f = holding("MF:119551", 100.0, 10.0, 12.0);
        f.term_rule = TermRule::DebtFund;
        f.lots = vec![
            LotInput {
                qty: 50.0,
                price: 10.0,
                date: Some(day("2021-01-10")),
                estimated: false,
            },
            LotInput {
                qty: 50.0,
                price: 10.0,
                date: Some(day("2023-06-01")),
                estimated: false,
            },
        ];
        let a = analyze(&[f], &ctx());
        let p = &a.positions[0];
        // The old lot is long-term after 24 months; the new one never.
        assert_eq!((p.short_term_lots, p.long_term_lots), (1, 1));
        assert!(p.next_long_term.is_none());
        assert_eq!(p.short_term_gain, Some(100.0));
    }

    #[test]
    fn cost_at_the_purchase_rate_shows_the_rupee_gain() {
        // Bought at USDINR 75, same dollar price today at 84: +12% in rupees.
        let mut us = holding("AAPL", 10.0, 100.0, 100.0);
        us.market = "United States".into();
        us.currency = Some("USD".into());
        us.fx_to_base = Some(84.0);
        us.cost_base = Some(10.0 * 100.0 * 75.0);
        let a = analyze(&[us], &ctx());
        assert!((a.pnl_pct - 12.0).abs() < 1e-9);
        let p = &a.positions[0];
        assert_eq!(p.pnl, Some(0.0));
        assert!((p.fx_effect_base.unwrap() - 9000.0).abs() < 1e-9);
        assert!((a.by_currency[0].pnl_base - 9000.0).abs() < 1e-9);
    }

    #[test]
    fn a_holding_worth_nothing_keeps_its_loss_in_its_group() {
        let mut gone = holding("GONE.NS", 10.0, 10.0, 0.0);
        gone.sector = Some("Energy".into());
        let a = analyze(&[gone, holding("B.NS", 1.0, 100.0, 120.0)], &ctx());
        let energy = a.by_sector.iter().find(|g| g.name == "Energy").unwrap();
        assert_eq!(energy.pnl_base, -100.0);
        let sum: f64 = a.by_sector.iter().map(|g| g.pnl_base).sum();
        assert!((sum - a.pnl).abs() < 1e-9, "groups add up to the total");
    }

    #[test]
    fn lots_on_each_side_are_counted_per_holding() {
        // Two short-term lots: no long-term lot, so not "mixed".
        let mut h = holding("NEW.NS", 10.0, 100.0, 110.0);
        h.lots = vec![
            LotInput {
                qty: 5.0,
                price: 100.0,
                date: Some(day("2026-06-01")),
                estimated: false,
            },
            LotInput {
                qty: 5.0,
                price: 100.0,
                date: Some(day("2026-09-01")),
                estimated: false,
            },
        ];
        let a = analyze(&[h], &ctx());
        let p = &a.positions[0];
        assert_eq!((p.short_term_lots, p.long_term_lots), (2, 0));
        assert_eq!(p.next_long_term.as_ref().unwrap().qty, 5.0);
    }

    #[test]
    fn observations_use_neutral_words() {
        let mut far = holding("FALL.NS", 10.0, 100.0, 50.0);
        far.closes = dated((0..100).map(|i| 100.0 - i as f64 * 0.5).collect());
        far.two_hundred_day_average = Some(80.0);
        far.fifty_two_week_low = Some(49.0);
        far.fifty_two_week_high = Some(110.0);
        far.score = Some((30.0, 0.8));
        far.cap_tier = Some(CapTier::Small);
        far.lots[0].estimated = true;
        let a = analyze(&[far, holding("B.NS", 1.0, 1.0, 1.0)], &ctx());
        let kinds: Vec<&str> = a.observations.iter().map(|o| o.kind).collect();
        for k in [
            "below_cost",
            "far_below_year_high",
            "below_200_day_average",
            "near_52_week_low",
            "weak_long_term_score",
            "estimated_purchase_date",
            "above_profile_smallcap_limit",
        ] {
            assert!(kinds.contains(&k), "{k} missing from {kinds:?}");
        }
        for o in &a.observations {
            crate::verdict::testing::assert_neutral(&o.text);
        }
    }
}
