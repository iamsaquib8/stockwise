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
    /// Daily closes of about the last year, oldest first.
    pub closes: Vec<f64>,
    /// The long-term score (0-100) and the share of its inputs present.
    pub score: Option<(f64, f64)>,
    /// `Lower`, `Moderate` or `Higher`, from the long-term score.
    pub risk_tier: Option<String>,
    /// Whose holding-period rules apply (`None`: no long-term rate).
    pub tax_country: Option<Country>,
}

/// Settings that apply to the whole portfolio.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub today: NaiveDate,
    pub base_currency: String,
    /// The investor profile's limits, percent of the portfolio.
    pub max_stock_weight: Option<f64>,
    pub max_smallcap_weight: Option<f64>,
    /// The market's main index, daily closes, oldest first.
    pub benchmark_closes: Vec<f64>,
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

/// Beta of `asset` against `bench`, on their last common sessions.
fn beta(asset: &[f64], bench: &[f64]) -> Option<f64> {
    let n = asset.len().min(bench.len());
    if n < MIN_CLOSES {
        return None;
    }
    let a = returns(&asset[asset.len() - n..]);
    let b = returns(&bench[bench.len() - n..]);
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
    let months = p.tax_country.and_then(Country::long_term_months);
    let long_term_on = |d: NaiveDate, m: u32| crate::sellpreview::long_term_from(d, m);
    let (mut st, mut lt, mut any_term) = (0.0, 0.0, false);
    let mut next: Option<NextLongTerm> = None;
    if let (Some(price), Some(m)) = (p.price, months) {
        for lot in &p.lots {
            let Some(d) = lot.date else { continue };
            let gain = lot.qty * (price - lot.price);
            any_term = true;
            match long_term_on(d, m) {
                Some(on) if on > ctx.today => {
                    st += gain;
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
                _ => lt += gain,
            }
        }
    }
    let oldest = p.lots.iter().filter_map(|l| l.date).min();
    let closes = &p.closes;
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
        beta: beta(closes, &ctx.benchmark_closes),
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
        let pnl_base = p
            .pnl
            .zip(p.value)
            .filter(|(_, v)| *v != 0.0)
            .map_or(0.0, |(g, v)| g * vb / v);
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

/// The portfolio's daily value index over the sessions every holding with
/// history shares, each weighted by today's value: the risk of the
/// portfolio as it stands, not as it was held.
fn risk(inputs: &[PositionInput], positions: &[Position], ctx: &Context, total: f64) -> Risk {
    let with: Vec<(&PositionInput, f64)> = inputs
        .iter()
        .zip(positions)
        .filter(|(i, _)| i.closes.len() >= MIN_CLOSES)
        .filter_map(|(i, p)| p.value_base.filter(|v| *v > 0.0).map(|v| (i, v)))
        .collect();
    let covered: f64 = with.iter().map(|(_, v)| v).sum();
    let sessions = with.iter().map(|(i, _)| i.closes.len()).min().unwrap_or(0);
    let index: Vec<f64> = if with.is_empty() {
        Vec::new()
    } else {
        (0..sessions)
            .map(|k| {
                with.iter()
                    .map(|(i, v)| {
                        let c = &i.closes[i.closes.len() - sessions..];
                        let last = *c.last().expect("non-empty");
                        if last > 0.0 { v * c[k] / last } else { 0.0 }
                    })
                    .sum()
            })
            .collect()
    };
    let scored: Vec<(f64, f64)> = positions
        .iter()
        .filter_map(|p| p.score.zip(p.value_base))
        .collect();
    let score_weight: f64 = scored.iter().map(|(_, w)| w).sum();
    Risk {
        coverage_pct: pct(covered, total),
        sessions,
        volatility_pct: volatility(&returns(&index)),
        max_drawdown_pct: max_drawdown(&index),
        beta: beta(&index, &ctx.benchmark_closes),
        benchmark: ctx.benchmark_name.clone(),
        weighted_score: (score_weight > 0.0)
            .then(|| scored.iter().map(|(s, w)| s * w).sum::<f64>() / score_weight),
    }
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
        .zip(inputs)
        .filter(|(p, _)| p.value_base.is_some())
        .filter_map(|(p, i)| i.fx_to_base.map(|r| p.cost * r))
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
        if let Some(m) = i.tax_country.and_then(Country::long_term_months) {
            for l in &i.lots {
                match l
                    .date
                    .and_then(|d| crate::sellpreview::long_term_from(d, m))
                {
                    None => terms.undated_lots += 1,
                    Some(on) if on > ctx.today => terms.short_term_lots += 1,
                    Some(_) => terms.long_term_lots += 1,
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
        a_in.closes = up.clone();
        let mut b_in = holding("CHOP.NS", 10.0, 100.0, *chop.last().unwrap());
        b_in.closes = chop;
        let mut c = ctx();
        c.benchmark_closes = up.iter().map(|x| x * 2.0).collect();
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
        short.closes = vec![1.0; 10];
        let b = analyze(&[short], &ctx());
        assert_eq!(b.positions[0].volatility_pct, None);
        assert_eq!(b.risk.volatility_pct, None);
    }

    #[test]
    fn observations_use_neutral_words() {
        let mut far = holding("FALL.NS", 10.0, 100.0, 50.0);
        far.closes = (0..100).map(|i| 100.0 - i as f64 * 0.5).collect();
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
