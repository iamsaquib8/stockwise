//! `picks`: a 0-100 score for each scanned stock, made of five pillars
//! (value, quality, growth, trend, risk). Every measure is a percentile rank
//! among the stocks of the same scan, so the score says where a stock stands
//! in that set, not whether a fixed threshold was crossed.
//!
//! The scan works in two stages. The bulk quote ranks every stock that
//! clears the size and liquidity floors; the leaders of that first pass
//! (several times `--top`) then get their fundamentals (return on equity,
//! margins, debt, revenue growth, beta, analyst count) and a year of daily
//! prices, and the whole scan is ranked again. Figures that are too good to
//! be true (P/E below 3, ROE above 80%, a dividend yield above 15%) are
//! flagged "check data" and kept out of the list: they are far more often a
//! one-off gain or a stale row than a bargain. A loss, negative book value
//! or shrinking earnings rank at the bottom of their measure, never as
//! missing data.
//!
//! The method is described in `docs/methodology.md`; bump [`METHODOLOGY`]
//! when it changes so the track record (`picks review`) can tell runs apart.

use crate::api::{Quote, YahooClient};
use crate::market::{self, Market};
use crate::output::CsvRow;
use anyhow::{Result, bail};
use futures_util::StreamExt;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Version of the scoring method, stored with every logged pick.
pub const METHODOLOGY: &str = "2";

/// What the list is for: years of compounding or a few weeks to months.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Horizon {
    /// Years: value, quality and growth lead (the default)
    #[default]
    Long,
    /// Weeks to a few months: momentum, volume and risk lead
    Short,
}

impl Horizon {
    pub fn as_str(self) -> &'static str {
        match self {
            Horizon::Long => "long",
            Horizon::Short => "short",
        }
    }

    /// Pillar weights (value, quality, growth, trend, risk); they add up
    /// to 1. Short term still keeps some quality and value in: momentum in
    /// a weak business is how most short-term losses start.
    fn weights(self) -> [f64; 5] {
        match self {
            Horizon::Long => [0.25, 0.25, 0.15, 0.20, 0.15],
            Horizon::Short => [0.10, 0.15, 0.10, 0.45, 0.20],
        }
    }
}

/// Past these a figure is more likely bad data or a one-off than a better
/// company: the stock is flagged "check data" and left out of the list.
const MAX_PLAUSIBLE_EARNINGS_YIELD: f64 = 1.0 / 3.0; // P/E below 3
const MAX_PLAUSIBLE_ROE: f64 = 0.80;
const MAX_PLAUSIBLE_DIVIDEND_YIELD: f64 = 0.15;

/// Fewest analysts behind a forward EPS estimate for it to count.
const MIN_ANALYSTS: u32 = 3;
/// Most stocks of one sector in a list.
const PER_SECTOR: usize = 3;
/// Fewest daily closes for volatility and drawdown (about three months).
const MIN_CLOSES_RISK: usize = 60;
/// Fewest daily closes for 12-1 month momentum (about ten months).
const MIN_CLOSES_MOMENTUM: usize = 200;
/// Trading sessions in a month, skipped at the end of the momentum window.
const SKIP_MONTH: usize = 21;
/// Sessions in three months, the short-term momentum window.
const QUARTER: usize = 63;
/// Sessions skipped at the end of the short-term window (last week's moves
/// tend to reverse).
const SKIP_WEEK: usize = 5;
/// Recent and earlier windows of the volume ratio, in sessions.
const VOLUME_RECENT: usize = 20;
const VOLUME_BASE: usize = 60;
/// Stocks enriched in the second stage: several times the list, at least this.
const MIN_FINALISTS: usize = 60;
/// Never enrich more than this many (each costs two requests).
const MAX_FINALISTS: usize = 200;
/// Requests in flight while enriching.
const CONCURRENCY: usize = 6;

/// Least share of the score's weight with data for a stock to be listed;
/// below it the score is mostly the pull toward 50, not the stock.
pub const MIN_COVERAGE: f64 = 0.6;

/// A pillar at or above this rank is named among a stock's strongest.
const STRONG: f64 = 80.0;

/// One scored stock.
#[derive(Debug, Clone, Serialize)]
pub struct Pick {
    pub symbol: String,
    pub name: Option<String>,
    pub price: Option<f64>,
    pub change_percent: Option<f64>,
    pub currency: Option<String>,
    pub sector: Option<String>,
    pub market_cap: Option<f64>,
    /// 0-100: the weighted pillars, pulled toward 50 by missing data.
    pub score: f64,
    /// Earnings yield, book value to price and dividend yield.
    pub value: Option<f64>,
    /// Return on equity, profit margin and low debt to equity.
    pub quality: Option<f64>,
    /// Forward EPS against trailing EPS (3+ analysts) and revenue growth.
    pub growth: Option<f64>,
    /// Long term: 12-1 month momentum per unit of volatility and price
    /// against the 200-day average (the 52-week range without a price
    /// history). Short term: 3-month momentum per unit of volatility, price
    /// against the 50-day average, place in the 52-week range and volume
    /// against its usual level.
    pub trend: Option<f64>,
    /// Low volatility, small drawdown and low beta over the last year.
    pub risk: Option<f64>,
    /// Share of the score's weight that had data, 0-1.
    pub coverage: f64,
    /// Why the figures look implausible ("P/E below 3"); a flagged stock is
    /// left out of the list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

impl CsvRow for Pick {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "price",
        "change_percent",
        "currency",
        "sector",
        "market_cap",
        "score",
        "value",
        "quality",
        "growth",
        "trend",
        "risk",
        "coverage",
        "flags",
    ];
}

impl Pick {
    /// The pillars in the top fifth of the scan, for the table.
    pub fn strongest(&self) -> Vec<&'static str> {
        [
            ("Value", self.value),
            ("Quality", self.quality),
            ("Growth", self.growth),
            ("Trend", self.trend),
            ("Risk", self.risk),
        ]
        .into_iter()
        .filter(|(_, v)| v.is_some_and(|v| v >= STRONG))
        .map(|(n, _)| n)
        .collect()
    }
}

/// Size and liquidity a stock needs to be scored, in the market's currency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floors {
    /// Least market capitalisation.
    pub min_mcap: f64,
    /// Least average value traded per session over three months.
    pub min_adv: f64,
}

impl Floors {
    /// The defaults for `market` and cap tier, or the overrides, given in
    /// [`unit`]s (crore in India, millions elsewhere). India: ₹500 crore and
    /// ₹2 crore a day; small caps ₹1 crore a day; micro caps ₹100 crore and
    /// ₹50 lakh a day. US: $1 billion and $5 million a day; small caps $300
    /// million and $2 million; micro caps $50 million and $0.5 million.
    /// Other markets: none. The liquidity floor never goes away by default:
    /// a stock that hardly trades can't be sold when it falls.
    pub fn for_market(
        market: Market,
        cap: Option<market::CapFilter>,
        min_mcap: Option<f64>,
        min_adv: Option<f64>,
    ) -> Self {
        use market::CapFilter as C;
        let (mcap, adv) = match (market, cap) {
            (Market::In, Some(C::Micro)) => (100.0, 0.5),
            (Market::In, Some(C::Small)) => (500.0, 1.0),
            (Market::In, _) => (500.0, 2.0),
            (Market::Us, Some(C::Micro)) => (50.0, 0.5),
            (Market::Us, Some(C::Small)) => (300.0, 2.0),
            (Market::Us, _) => (1_000.0, 5.0),
            _ => (0.0, 0.0),
        };
        let u = unit(market).1;
        Floors {
            min_mcap: min_mcap.unwrap_or(mcap).max(0.0) * u,
            min_adv: min_adv.unwrap_or(adv).max(0.0) * u,
        }
    }

    /// Whether `q` clears both floors. A stock without the figure a floor
    /// needs does not clear it: size unknown is not size enough.
    pub fn admits(&self, q: &Quote) -> bool {
        let mcap_ok = self.min_mcap <= 0.0 || q.market_cap.is_some_and(|m| m >= self.min_mcap);
        let adv = match (q.average_daily_volume_3_month, q.regular_market_price) {
            (Some(v), Some(p)) => Some(v as f64 * p),
            _ => None,
        };
        let adv_ok = self.min_adv <= 0.0 || adv.is_some_and(|a| a >= self.min_adv);
        mcap_ok && adv_ok
    }
}

/// `amount` (in the market's currency) in the unit people use: crore in
/// India, millions or billions elsewhere.
pub fn describe_amount(market: Market, amount: f64) -> String {
    match market {
        Market::In => format!("₹{:.0} crore", amount / 1e7).replace("₹0 crore", "₹0"),
        _ if amount >= 1e9 => format!("{:.1} billion", amount / 1e9),
        _ => format!("{:.1} million", amount / 1e6),
    }
}

/// The unit of `--min-mcap` / `--min-adv` for `market`: its name and size.
pub fn unit(market: Market) -> (&'static str, f64) {
    match market {
        Market::In => ("crore", 1e7),
        _ => ("million", 1e6),
    }
}

/// What a year of daily closes adds to the score.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct History {
    /// Annualised volatility of daily returns.
    pub volatility: Option<f64>,
    /// Largest fall from a peak, as a fraction.
    pub drawdown: Option<f64>,
    /// Return from a year ago to a month ago (12-1 momentum).
    pub momentum: Option<f64>,
    /// Return over three months, ending a week ago.
    pub momentum_3m: Option<f64>,
    /// Average volume of the last 20 sessions against the 60 before.
    pub volume_ratio: Option<f64>,
}

impl History {
    /// Measures of `closes` (oldest first, about a year of sessions).
    pub fn from_closes(closes: &[f64]) -> Self {
        let n = closes.len();
        let mut h = History::default();
        if n >= MIN_CLOSES_RISK {
            h.volatility = crate::technical::annualized_volatility(closes).filter(|v| *v > 0.0);
            h.drawdown = crate::technical::max_drawdown(closes).map(|d| d.0);
        }
        if n >= MIN_CLOSES_MOMENTUM {
            let (start, end) = (closes[0], closes[n - 1 - SKIP_MONTH]);
            if start > 0.0 {
                h.momentum = Some(end / start - 1.0);
            }
        }
        if n > QUARTER + SKIP_WEEK {
            let (start, end) = (
                closes[n - 1 - SKIP_WEEK - QUARTER],
                closes[n - 1 - SKIP_WEEK],
            );
            if start > 0.0 {
                h.momentum_3m = Some(end / start - 1.0);
            }
        }
        h
    }

    /// [`History::from_closes`] plus the volume ratio from daily volumes
    /// (oldest first; zero-volume sessions are left out).
    pub fn from_bars(closes: &[f64], volumes: &[u64]) -> Self {
        let mut h = Self::from_closes(closes);
        let v: Vec<f64> = volumes
            .iter()
            .filter(|v| **v > 0)
            .map(|v| *v as f64)
            .collect();
        if v.len() >= VOLUME_RECENT + VOLUME_BASE {
            let n = v.len();
            let recent = v[n - VOLUME_RECENT..].iter().sum::<f64>() / VOLUME_RECENT as f64;
            let base = v[n - VOLUME_RECENT - VOLUME_BASE..n - VOLUME_RECENT]
                .iter()
                .sum::<f64>()
                / VOLUME_BASE as f64;
            if base > 0.0 {
                h.volume_ratio = Some(recent / base);
            }
        }
        h
    }
}

/// The raw measures of one stock; `None` where the data is missing. Higher
/// is better for every measure (costs such as debt are negated).
#[derive(Default)]
struct Measures {
    earnings_yield: Option<f64>,
    book_yield: Option<f64>,
    dividend_yield: Option<f64>,
    roe: Option<f64>,
    margin: Option<f64>,
    low_leverage: Option<f64>,
    eps_growth: Option<f64>,
    revenue_growth: Option<f64>,
    momentum: Option<f64>,
    momentum_3m: Option<f64>,
    vs_50: Option<f64>,
    volume_ratio: Option<f64>,
    vs_200: Option<f64>,
    range_pos: Option<f64>,
    low_volatility: Option<f64>,
    low_drawdown: Option<f64>,
    low_beta: Option<f64>,
    /// Negative book value: equity is gone.
    negative_book: bool,
    /// Trailing earnings are a loss.
    loss: bool,
}

fn measures(q: &Quote, h: &History) -> Measures {
    let price = q.regular_market_price.filter(|p| *p > 0.0);
    let pos = |v: Option<f64>| v.filter(|x| *x > 0.0);
    let eps = q.eps_trailing_twelve_months;
    let book = q.book_value.filter(|b| *b != 0.0);
    let negative_book = book.is_some_and(|b| b < 0.0);
    let loss = eps.is_some_and(|e| e < 0.0);
    let eps_growth = match (q.eps_forward, eps, q.number_of_analyst_opinions) {
        (Some(f), Some(t), Some(n)) if t > 0.0 && n >= MIN_ANALYSTS => Some((f - t) / t),
        _ => None,
    };
    let momentum = match (h.momentum, h.volatility) {
        (Some(m), Some(v)) => Some(m / v),
        _ => None,
    };
    let momentum_3m = match (h.momentum_3m, h.volatility) {
        (Some(m), Some(v)) => Some(m / v),
        _ => None,
    };
    Measures {
        // A loss is a negative yield: it ranks below every profit.
        earnings_yield: match (eps, price) {
            (Some(e), Some(p)) => Some(e / p),
            _ => pos(q.trailing_pe).map(|pe| 1.0 / pe),
        },
        // Negative book value is a negative yield: it ranks last.
        book_yield: match (book, price) {
            (Some(b), Some(p)) => Some(b / p),
            _ => pos(q.price_to_book).map(|pb| 1.0 / pb),
        },
        dividend_yield: q.dividend_yield_fraction().filter(|d| *d >= 0.0),
        // Return on equity as reported, never EPS over book value: that is
        // earnings yield over book yield, which value already counts.
        roe: q.return_on_equity,
        margin: q.profit_margins,
        // Negative debt to equity means negative equity: ranked last below.
        low_leverage: q.debt_to_equity.filter(|d| *d >= 0.0).map(|d| -d),
        eps_growth,
        revenue_growth: q.revenue_growth,
        momentum,
        momentum_3m,
        vs_50: match (price, pos(q.fifty_day_average)) {
            (Some(p), Some(m)) => Some(p / m - 1.0),
            _ => None,
        },
        volume_ratio: h.volume_ratio,
        vs_200: match (price, pos(q.two_hundred_day_average)) {
            (Some(p), Some(m)) => Some(p / m - 1.0),
            _ => None,
        },
        range_pos: match (price, q.fifty_two_week_low, q.fifty_two_week_high) {
            (Some(p), Some(lo), Some(hi)) if hi > lo => {
                Some(((p - lo) / (hi - lo)).clamp(0.0, 1.0))
            }
            _ => None,
        },
        low_volatility: h.volatility.map(|v| -v),
        low_drawdown: h.drawdown.map(|d| -d),
        low_beta: q.beta.map(|b| -b),
        negative_book: negative_book || q.debt_to_equity.is_some_and(|d| d < 0.0),
        loss,
    }
}

/// Why `q`'s figures look implausible; empty when they look fine.
pub fn data_flags(q: &Quote) -> Vec<String> {
    let mut flags = Vec::new();
    let ey = match (q.eps_trailing_twelve_months, q.regular_market_price) {
        (Some(e), Some(p)) if p > 0.0 => Some(e / p),
        _ => q.trailing_pe.filter(|pe| *pe > 0.0).map(|pe| 1.0 / pe),
    };
    if ey.is_some_and(|y| y > MAX_PLAUSIBLE_EARNINGS_YIELD) {
        flags.push("P/E below 3".to_string());
    }
    if q.return_on_equity.is_some_and(|r| r > MAX_PLAUSIBLE_ROE) {
        flags.push("ROE above 80%".to_string());
    }
    if q.dividend_yield_fraction()
        .is_some_and(|d| d > MAX_PLAUSIBLE_DIVIDEND_YIELD)
    {
        flags.push("dividend yield above 15%".to_string());
    }
    flags
}

/// Percentile rank (0-100) of each value among those present: the share
/// below it plus half of those equal to it (itself included), so the
/// lowest of four ranks 12.5 and a lone value 50.
fn ranks(values: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut sorted: Vec<f64> = values
        .iter()
        .flatten()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len() as f64;
    values
        .iter()
        .map(|v| {
            let v = (*v).filter(|v| v.is_finite())?;
            let below = sorted.partition_point(|x| *x < v) as f64;
            let not_above = sorted.partition_point(|x| *x <= v) as f64;
            Some((below + (not_above - below) / 2.0) / n * 100.0)
        })
        .collect()
}

/// Weighted mean of the parts that have a value, and the share of the
/// parts' weight that had one. `(None, 0.0)` when none has.
fn blend(parts: &[(Option<f64>, f64)]) -> (Option<f64>, f64) {
    let total: f64 = parts.iter().map(|(_, w)| w).sum();
    let weight: f64 = parts
        .iter()
        .filter(|(v, _)| v.is_some())
        .map(|(_, w)| w)
        .sum();
    if weight <= 0.0 || total <= 0.0 {
        return (None, 0.0);
    }
    let mean = parts
        .iter()
        .filter_map(|(v, w)| v.map(|v| v * w))
        .sum::<f64>()
        / weight;
    (Some(mean), weight / total)
}

/// Score every quote against the others, best first (ties by symbol).
/// `history` holds the price-history measures of the stocks that have them
/// (by symbol). Flagged stocks are scored too; [`select`] leaves them out.
pub fn score(quotes: &[Quote], history: &HashMap<String, History>, horizon: Horizon) -> Vec<Pick> {
    let [w_value, w_quality, w_growth, w_trend, w_risk] = horizon.weights();
    let none = History::default();
    let m: Vec<Measures> = quotes
        .iter()
        .map(|q| {
            let h = q
                .symbol
                .as_ref()
                .and_then(|s| history.get(s))
                .unwrap_or(&none);
            measures(q, h)
        })
        .collect();
    let col = |f: fn(&Measures) -> Option<f64>| ranks(&m.iter().map(f).collect::<Vec<_>>());
    let ey = col(|m| m.earnings_yield);
    let by = col(|m| m.book_yield);
    let dy = col(|m| m.dividend_yield);
    let roe = col(|m| m.roe);
    let margin = col(|m| m.margin);
    let leverage = col(|m| m.low_leverage);
    let eps_g = col(|m| m.eps_growth);
    let rev_g = col(|m| m.revenue_growth);
    let mom = col(|m| m.momentum);
    let mom3 = col(|m| m.momentum_3m);
    let vs50 = col(|m| m.vs_50);
    let vratio = col(|m| m.volume_ratio);
    let vs200 = col(|m| m.vs_200);
    let range = col(|m| m.range_pos);
    let vol = col(|m| m.low_volatility);
    let dd = col(|m| m.low_drawdown);
    let beta = col(|m| m.low_beta);
    let bottom = Some(0.0);
    let mut picks: Vec<Pick> = quotes
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let mi = &m[i];
            // Distress ranks at the bottom of its measure: a stock is not
            // average because its equity is gone or it lost money.
            let roe_i = if mi.negative_book { bottom } else { roe[i] };
            let lev_i = if mi.negative_book {
                bottom
            } else {
                leverage[i]
            };
            let margin_i = if mi.loss && margin[i].is_none() {
                bottom
            } else {
                margin[i]
            };
            let eps_g_i = if mi.loss { bottom } else { eps_g[i] };
            let (value, cv) = blend(&[(ey[i], 0.5), (by[i], 0.3), (dy[i], 0.2)]);
            // Negative equity: quality is at the bottom, whatever the margin.
            let (quality, cq) = if mi.negative_book {
                (bottom, 1.0)
            } else {
                blend(&[(roe_i, 0.5), (margin_i, 0.3), (lev_i, 0.2)])
            };
            let (growth, cg) = blend(&[(eps_g_i, 0.6), (rev_g[i], 0.4)]);
            let (trend, ct) = match horizon {
                Horizon::Long if mom[i].is_some() => blend(&[(mom[i], 0.6), (vs200[i], 0.4)]),
                Horizon::Long => blend(&[(vs200[i], 0.6), (range[i], 0.4)]),
                Horizon::Short => blend(&[
                    (mom3[i], 0.35),
                    (vs50[i], 0.20),
                    (range[i], 0.20),
                    (vratio[i], 0.25),
                ]),
            };
            let (risk, cr) = blend(&[(vol[i], 0.4), (dd[i], 0.3), (beta[i], 0.3)]);
            let pillars = [
                (value, w_value),
                (quality, w_quality),
                (growth, w_growth),
                (trend, w_trend),
                (risk, w_risk),
            ];
            let coverage =
                w_value * cv + w_quality * cq + w_growth * cg + w_trend * ct + w_risk * cr;
            // Missing data pulls the score toward the middle: a stock is not
            // ranked high on half the picture.
            let score = blend(&pillars)
                .0
                .map_or(50.0, |raw| 50.0 + (raw - 50.0) * coverage);
            Pick {
                symbol: q.symbol.clone().unwrap_or_default(),
                name: q.short_name.clone().or_else(|| q.long_name.clone()),
                price: q.regular_market_price,
                change_percent: q.regular_market_change_percent,
                currency: q.currency.clone(),
                sector: q.sector.clone(),
                market_cap: q.market_cap,
                score,
                value,
                quality,
                growth,
                trend,
                risk,
                coverage,
                flags: data_flags(q),
            }
        })
        .collect();
    picks.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    picks
}

/// The list to show from `scored` (best first): flagged stocks and stocks
/// with less than [`MIN_COVERAGE`] of their data left out, at most
/// [`PER_SECTOR`] per known sector, `top` long (0 keeps every listed stock
/// in score order, without the sector limit). Returns the list, the flagged
/// stocks and how many had too little data.
pub fn select(scored: Vec<Pick>, top: usize) -> (Vec<Pick>, Vec<Pick>, usize) {
    let (flagged, rest): (Vec<Pick>, Vec<Pick>) =
        scored.into_iter().partition(|p| !p.flags.is_empty());
    let (clean, thin): (Vec<Pick>, Vec<Pick>) =
        rest.into_iter().partition(|p| p.coverage >= MIN_COVERAGE);
    let thin = thin.len();
    if top == 0 {
        return (clean, flagged, thin);
    }
    let mut per_sector: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(top);
    for p in clean {
        if out.len() >= top {
            break;
        }
        if let Some(sector) = p.sector.as_deref().filter(|s| !s.is_empty()) {
            let n = per_sector.entry(sector.to_ascii_lowercase()).or_default();
            if *n >= PER_SECTOR {
                continue;
            }
            *n += 1;
        }
        out.push(p);
    }
    (out, flagged, thin)
}

/// How many first-stage leaders get fundamentals and prices.
fn finalists(top: usize, available: usize) -> usize {
    (top * 4).clamp(MIN_FINALISTS, MAX_FINALISTS).min(available)
}

/// A finished scan.
#[derive(Debug, Clone, Serialize)]
pub struct Run {
    /// The list, best first.
    pub picks: Vec<Pick>,
    /// Stocks in the universe and cap tier before the floors.
    pub universe: usize,
    /// Stocks that cleared the floors and were ranked.
    pub scanned: usize,
    /// Stocks below the size or liquidity floor (or without the figures).
    pub below_floor: usize,
    /// Stocks whose fundamentals and prices were fetched.
    pub enriched: usize,
    /// Stocks left out because their figures look implausible.
    pub flagged: Vec<Pick>,
    /// Stocks left out because less than [`MIN_COVERAGE`] of their data
    /// came through.
    pub thin_data: usize,
    #[serde(skip)]
    pub floors: Option<Floors>,
    pub horizon: Horizon,
    pub methodology: &'static str,
    /// Sources, missing fields, warnings and coverage of the finalists'
    /// data (`provenance::for_scan`).
    pub meta: crate::provenance::Meta,
}

/// Scan `market`'s universe (and cap tier), score it in two stages and
/// return the list. Shared by the CLI and the MCP tool.
pub async fn run(
    client: &YahooClient,
    market: Market,
    universe: Option<market::Universe>,
    cap: Option<market::CapFilter>,
    floors: Floors,
    horizon: Horizon,
    top: usize,
) -> Result<Run> {
    let quotes = market::filter_by_cap(
        crate::commands::universe_quotes(client, market, universe).await?,
        cap,
    );
    let universe_len = quotes.len();
    let (mut kept, below): (Vec<Quote>, Vec<Quote>) =
        quotes.into_iter().partition(|q| floors.admits(q));
    if kept.is_empty() {
        if universe_len == 0 {
            bail!("No stocks to score in this universe and cap tier");
        }
        bail!(
            "None of the {} stocks clears the size and liquidity floors (market cap {} and {} \
             traded a day); lower them with --min-mcap / --min-adv",
            universe_len,
            describe_amount(market, floors.min_mcap),
            describe_amount(market, floors.min_adv)
        );
    }
    // First stage: the bulk quote alone.
    let first = score(&kept, &HashMap::new(), horizon);
    let n = finalists(top, first.len());
    let leaders: HashSet<String> = first
        .iter()
        .filter(|p| p.flags.is_empty())
        .take(n)
        .map(|p| p.symbol.clone())
        .collect();
    let history = enrich_finalists(client, &mut kept, &leaders).await;
    // Second stage: the whole scan again, with the leaders' fuller data.
    let meta = crate::provenance::for_scan(
        kept.iter()
            .filter(|q| q.symbol.as_ref().is_some_and(|s| leaders.contains(s))),
    );
    let (picks, flagged, thin_data) = select(score(&kept, &history, horizon), top);
    Ok(Run {
        picks,
        universe: universe_len,
        scanned: kept.len(),
        below_floor: below.len(),
        enriched: leaders.len(),
        flagged,
        thin_data,
        floors: Some(floors),
        horizon,
        methodology: METHODOLOGY,
        meta,
    })
}

/// Fetch fundamentals (`quoteSummary`) and a year of daily closes for the
/// stocks in `leaders`, filling their quotes in place. Returns the price
/// measures by symbol. A failed request leaves that part missing; the
/// coverage shows it.
pub async fn enrich_finalists(
    client: &YahooClient,
    quotes: &mut [Quote],
    leaders: &HashSet<String>,
) -> HashMap<String, History> {
    if leaders.is_empty() {
        return HashMap::new();
    }
    eprintln!(
        "  \u{24d8} Fetching fundamentals and a year of prices for the {} highest first-pass scores…",
        leaders.len()
    );
    let todo: Vec<Quote> = quotes
        .iter()
        .filter(|q| q.symbol.as_ref().is_some_and(|s| leaders.contains(s)))
        .cloned()
        .collect();
    let done: Vec<(Quote, History)> =
        futures_util::stream::iter(todo.into_iter().map(|mut q| async move {
            // A failed lookup is kept on the quote, so `meta` reports it.
            for w in crate::research::enrich_quote(client, &mut q, false).await {
                q.meta.get_or_insert_with(Default::default).warn(w);
            }
            let sym = q.symbol.clone().unwrap_or_default();
            let history = match client.get_chart(&sym, "1y", "1d").await {
                Ok(chart) => {
                    let closes: Vec<f64> =
                        chart.dated_closes().into_iter().map(|(_, c)| c).collect();
                    let volumes: Vec<u64> = chart
                        .indicators
                        .quote
                        .first()
                        .and_then(|q| q.volume.as_ref())
                        .map(|v| v.iter().flatten().copied().collect())
                        .unwrap_or_default();
                    History::from_bars(&closes, &volumes)
                }
                Err(_) => History::default(),
            };
            (q, history)
        }))
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut by_symbol: HashMap<String, (Quote, History)> = done
        .into_iter()
        .filter_map(|(q, h)| Some((q.symbol.clone()?, (q, h))))
        .collect();
    let mut history = HashMap::new();
    for q in quotes.iter_mut() {
        let Some(sym) = q.symbol.clone() else {
            continue;
        };
        if let Some((full, h)) = by_symbol.remove(&sym) {
            *q = full;
            history.insert(sym, h);
        }
    }
    history
}

/// The benchmark `picks review` compares with, and how it is described.
pub fn benchmark(market: Market) -> Option<(&'static str, &'static str)> {
    match market {
        Market::In => Some(("^NSEI", "NIFTY 50 (price index; dividends not included)")),
        Market::Us => Some(("^SP500TR", "S&P 500 total return")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain, healthy quote; tests change what they need.
    fn quote(sym: &str, sector: &str) -> Quote {
        Quote {
            symbol: Some(sym.into()),
            sector: Some(sector.into()),
            regular_market_price: Some(100.0),
            eps_trailing_twelve_months: Some(5.0),
            eps_forward: Some(5.5),
            number_of_analyst_opinions: Some(10),
            book_value: Some(40.0),
            return_on_equity: Some(0.15),
            profit_margins: Some(0.10),
            debt_to_equity: Some(50.0),
            revenue_growth: Some(0.08),
            beta: Some(1.0),
            two_hundred_day_average: Some(95.0),
            fifty_two_week_low: Some(80.0),
            fifty_two_week_high: Some(115.0),
            dividend_yield: Some(1.0),
            market_cap: Some(1e11),
            average_daily_volume_3_month: Some(1_000_000),
            ..Default::default()
        }
    }

    /// A year of closes rising from `start` by `total` overall, with
    /// day-to-day swings of `swing` (as a fraction).
    fn closes(start: f64, total: f64, swing: f64) -> Vec<f64> {
        (0..250)
            .map(|i| {
                let t = i as f64 / 249.0;
                let wiggle = if i % 2 == 0 { 1.0 + swing } else { 1.0 - swing };
                start * (1.0 + total * t) * wiggle
            })
            .collect()
    }

    #[test]
    fn ranks_are_percentiles_with_ties_and_gaps() {
        let r = ranks(&[Some(1.0), Some(3.0), None, Some(3.0), Some(9.0)]);
        // Mid-rank: a value counts half of itself and of its ties.
        assert_eq!(r[0], Some(12.5));
        assert_eq!(r[1], Some(50.0));
        assert_eq!(r[2], None);
        assert_eq!(r[3], Some(50.0));
        assert_eq!(r[4], Some(87.5));
        assert_eq!(ranks(&[Some(7.0)]), [Some(50.0)]);
        assert_eq!(ranks(&[Some(f64::NAN), Some(1.0)]), [None, Some(50.0)]);
    }

    /// The universe the audit described: a shell company with absurd
    /// figures, a loss-maker, a company with negative book value, a
    /// volatile momentum stock and a steady compounder, among plain peers.
    fn universe() -> (Vec<Quote>, HashMap<String, History>) {
        let mut quotes = Vec::new();
        let mut history = HashMap::new();

        let mut compounder = quote("STEADY.NS", "Consumer");
        compounder.eps_trailing_twelve_months = Some(6.0);
        compounder.eps_forward = Some(7.2);
        compounder.return_on_equity = Some(0.28);
        compounder.profit_margins = Some(0.22);
        compounder.debt_to_equity = Some(5.0);
        compounder.revenue_growth = Some(0.18);
        compounder.beta = Some(0.7);
        compounder.two_hundred_day_average = Some(88.0);
        compounder.dividend_yield = Some(1.5);
        history.insert(
            "STEADY.NS".to_string(),
            History::from_closes(&closes(70.0, 0.45, 0.004)),
        );
        quotes.push(compounder);

        // P/E 2, ROE 90%, tiny: the kind of row the old caps put on top.
        let mut shell = quote("SHELL.BO", "Financial Services");
        shell.eps_trailing_twelve_months = Some(50.0);
        shell.book_value = Some(300.0);
        shell.return_on_equity = Some(0.90);
        shell.market_cap = Some(3e8);
        quotes.push(shell);

        let mut loser = quote("LOSS.NS", "Industrials");
        loser.eps_trailing_twelve_months = Some(-4.0);
        loser.eps_forward = Some(-1.0);
        loser.return_on_equity = Some(-0.12);
        loser.profit_margins = None;
        loser.revenue_growth = Some(-0.05);
        loser.two_hundred_day_average = Some(120.0);
        loser.dividend_yield = None;
        history.insert(
            "LOSS.NS".to_string(),
            History::from_closes(&closes(140.0, -0.30, 0.02)),
        );
        quotes.push(loser);

        let mut broke = quote("BROKE.NS", "Utilities");
        broke.book_value = Some(-25.0);
        broke.debt_to_equity = Some(-300.0);
        broke.return_on_equity = None;
        broke.eps_trailing_twelve_months = Some(0.5);
        broke.two_hundred_day_average = Some(110.0);
        quotes.push(broke);

        let mut rocket = quote("ROCKET.NS", "Technology");
        rocket.eps_trailing_twelve_months = Some(1.0);
        rocket.beta = Some(2.2);
        rocket.two_hundred_day_average = Some(60.0);
        history.insert(
            "ROCKET.NS".to_string(),
            History::from_closes(&closes(40.0, 1.5, 0.06)),
        );
        quotes.push(rocket);

        for (i, sector) in ["Energy", "Healthcare", "Materials", "Consumer"]
            .iter()
            .enumerate()
        {
            let sym = format!("PEER{i}.NS");
            history.insert(sym.clone(), History::from_closes(&closes(90.0, 0.10, 0.01)));
            quotes.push(quote(&sym, sector));
        }
        (quotes, history)
    }

    #[test]
    fn the_steady_compounder_leads_and_distress_ranks_last() {
        let (quotes, history) = universe();
        let (list, flagged, _) = select(score(&quotes, &history, Horizon::Long), 0);
        let order: Vec<&str> = list.iter().map(|p| p.symbol.as_str()).collect();
        assert_eq!(order[0], "STEADY.NS", "{order:?}");
        // The shell company is out of the list and says why.
        assert!(order.iter().all(|s| *s != "SHELL.BO"));
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].symbol, "SHELL.BO");
        assert!(flagged[0].flags.iter().any(|f| f.contains("P/E below 3")));
        assert!(flagged[0].flags.iter().any(|f| f.contains("ROE above 80%")));
        // Loss and negative book value rank last, not in the middle.
        let last_two: HashSet<&str> = order[order.len() - 2..].iter().copied().collect();
        assert_eq!(
            last_two,
            HashSet::from(["LOSS.NS", "BROKE.NS"]),
            "{order:?}"
        );
        for p in &list {
            if p.symbol == "LOSS.NS" || p.symbol == "BROKE.NS" {
                assert!(p.score < 45.0, "{p:?}");
            }
        }
        // The volatile stock leads on trend but not on risk.
        let rocket = list.iter().find(|p| p.symbol == "ROCKET.NS").unwrap();
        assert!(rocket.risk.unwrap() < 30.0, "{rocket:?}");
        assert!(list.iter().all(|p| (0.0..=100.0).contains(&p.score)));
    }

    #[test]
    fn distress_is_scored_not_skipped() {
        let (quotes, history) = universe();
        let scored = score(&quotes, &history, Horizon::Long);
        let broke = scored.iter().find(|p| p.symbol == "BROKE.NS").unwrap();
        // Negative equity fills quality with the bottom rank.
        assert_eq!(broke.quality, Some(0.0));
        let loss = scored.iter().find(|p| p.symbol == "LOSS.NS").unwrap();
        assert!(loss.growth.unwrap() < 20.0, "{loss:?}");
    }

    #[test]
    fn at_most_three_per_sector_in_a_list() {
        let quotes: Vec<Quote> = (0..6)
            .map(|i| {
                let mut q = quote(&format!("BANK{i}.NS"), "Financial Services");
                q.return_on_equity = Some(0.30 - 0.01 * i as f64);
                q
            })
            .chain((0..3).map(|i| {
                let mut q = quote(&format!("OTHER{i}.NS"), &format!("Sector{i}"));
                q.return_on_equity = Some(0.05);
                q
            }))
            .collect();
        let (list, _, _) = select(score(&quotes, &HashMap::new(), Horizon::Long), 5);
        assert_eq!(list.len(), 5);
        let banks = list
            .iter()
            .filter(|p| p.sector.as_deref() == Some("Financial Services"))
            .count();
        assert_eq!(banks, 3);
        // Without a limit every stock stays.
        let (all, _, _) = select(score(&quotes, &HashMap::new(), Horizon::Long), 0);
        assert_eq!(all.len(), 9);
    }

    #[test]
    fn stocks_with_little_data_stay_out_of_the_list() {
        let bare = Quote {
            symbol: Some("BARE.NS".into()),
            regular_market_price: Some(10.0),
            two_hundred_day_average: Some(5.0),
            ..Default::default()
        };
        let (list, _, thin) = select(
            score(
                &[bare, quote("FULL.NS", "X")],
                &HashMap::new(),
                Horizon::Long,
            ),
            10,
        );
        assert_eq!(thin, 1);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].symbol, "FULL.NS");
    }

    #[test]
    fn floors_need_size_and_liquidity() {
        let f = Floors::for_market(Market::In, None, None, None);
        assert_eq!(f.min_mcap, 500.0 * 1e7);
        assert_eq!(f.min_adv, 2.0 * 1e7);
        let mut q = quote("OK.NS", "X");
        q.market_cap = Some(6e9);
        q.average_daily_volume_3_month = Some(300_000); // ₹3 crore a day at ₹100
        assert!(f.admits(&q));
        q.average_daily_volume_3_month = Some(100_000);
        assert!(!f.admits(&q));
        q.average_daily_volume_3_month = None;
        assert!(!f.admits(&q), "unknown liquidity does not clear the floor");
        let none = Floors::for_market(Market::In, None, Some(0.0), Some(0.0));
        assert!(none.admits(&q));
        let us = Floors::for_market(Market::Us, None, Some(2_000.0), None);
        assert_eq!(us.min_mcap, 2e9);
        // Small and micro caps get floors of their own, never none.
        let micro = Floors::for_market(Market::In, Some(market::CapFilter::Micro), None, None);
        assert_eq!(micro.min_mcap, 100.0 * 1e7);
        assert_eq!(micro.min_adv, 0.5 * 1e7);
        let small = Floors::for_market(Market::Us, Some(market::CapFilter::Small), None, None);
        assert_eq!((small.min_mcap, small.min_adv), (3e8, 2e6));
    }

    #[test]
    fn short_horizon_favours_momentum_and_volume() {
        // Two otherwise equal stocks: one cheap and steady, one rising on
        // growing volume.
        let mut cheap = quote("CHEAP.NS", "A");
        cheap.eps_trailing_twelve_months = Some(10.0);
        cheap.return_on_equity = Some(0.20);
        let mut runner = quote("RUNNER.NS", "B");
        runner.eps_trailing_twelve_months = Some(2.5);
        runner.fifty_day_average = Some(85.0);
        runner.fifty_two_week_high = Some(101.0);
        let mut history = HashMap::new();
        let flat: Vec<f64> = (0..250)
            .map(|i| 100.0 + if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        history.insert(
            "CHEAP.NS".to_string(),
            History::from_bars(&flat, &[1_000; 250]),
        );
        let rising = closes(60.0, 0.7, 0.01);
        let volumes: Vec<u64> = (0..250)
            .map(|i| if i >= 230 { 3_000 } else { 1_000 })
            .collect();
        let h = History::from_bars(&rising, &volumes);
        assert!((h.volume_ratio.unwrap() - 3.0).abs() < 1e-9);
        history.insert("RUNNER.NS".to_string(), h);
        let quotes = [cheap, runner];
        let long = score(&quotes, &history, Horizon::Long);
        let short = score(&quotes, &history, Horizon::Short);
        assert_eq!(long[0].symbol, "CHEAP.NS", "{long:?}");
        assert_eq!(short[0].symbol, "RUNNER.NS", "{short:?}");
    }

    #[test]
    fn history_measures_need_enough_closes() {
        let h = History::from_closes(&closes(100.0, 0.2, 0.01));
        assert!(h.volatility.is_some() && h.drawdown.is_some());
        // A year up 20%, measured a month before the end.
        let m = h.momentum.unwrap();
        assert!(m > 0.1 && m < 0.2, "{m}");
        let short = History::from_closes(&[100.0; 30]);
        assert_eq!(short, History::default());
    }

    #[test]
    fn growth_needs_three_analysts() {
        let mut thin = quote("THIN.NS", "X");
        thin.number_of_analyst_opinions = Some(1);
        thin.revenue_growth = None;
        let scored = score(
            &[thin, quote("FULL.NS", "Y")],
            &HashMap::new(),
            Horizon::Long,
        );
        let thin = scored.iter().find(|p| p.symbol == "THIN.NS").unwrap();
        assert_eq!(thin.growth, None);
        assert!(thin.coverage < 1.0);
    }

    #[test]
    fn missing_data_pulls_the_score_to_the_middle() {
        let bare = Quote {
            symbol: Some("BARE.NS".into()),
            regular_market_price: Some(10.0),
            ..Default::default()
        };
        let picks = score(
            &[bare, quote("FULL.NS", "X")],
            &HashMap::new(),
            Horizon::Long,
        );
        let bare = picks.iter().find(|p| p.symbol == "BARE.NS").unwrap();
        assert_eq!(bare.score, 50.0);
        assert_eq!(bare.coverage, 0.0);
        assert!(bare.strongest().is_empty());
    }

    #[test]
    fn finalists_scale_with_the_list() {
        assert_eq!(finalists(15, 1_000), 60);
        assert_eq!(finalists(30, 1_000), 120);
        assert_eq!(finalists(0, 1_000), 60);
        assert_eq!(finalists(100, 1_000), 200);
        assert_eq!(finalists(15, 20), 20);
    }
}
