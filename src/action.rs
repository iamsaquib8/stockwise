//! Rule-based actions: BUY / WATCH / AVOID for a stock you don't hold,
//! ADD / HOLD / TRIM / EXIT for one you do, or NO CALL when the data is too
//! thin to judge.
//!
//! The engine is deterministic: the same inputs always give the same action.
//! Every action carries up to three reasons, a confidence level, the price
//! whose daily close would invalidate it, a review date, an optional
//! position-size hint, an optional tax note and the disclaimer. The rules are
//! written out in `docs/methodology.md` ("Actions"); bump [`METHODOLOGY`]
//! whenever one of them changes.
//!
//! Nothing else in StockWise decides an action. The AI layer and the MCP
//! server explain the action computed here; they never invent or change one.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Serialize;

/// Version of the action rules. Bump it whenever a rule or threshold changes.
pub const METHODOLOGY: &str = "2";

/// Below this data coverage (percent) the engine makes no call.
pub const MIN_COVERAGE: f64 = 60.0;

/// Printed with every action.
pub const DISCLAIMER: &str = "Rule-based educational signal from StockWise's published methodology, \
     not investment advice. StockWise is not a SEBI-registered investment adviser or research \
     analyst. Markets carry risk and you can lose money; check the reasons and decide for yourself.";

// Thresholds. Keep these in step with docs/methodology.md.
const BUY_COMPOSITE: f64 = 65.0;
const AVOID_COMPOSITE: f64 = 45.0;
const EXIT_COMPOSITE: f64 = 40.0;
const MAX_DE: f64 = 1.5;
const MAX_PLEDGE_PCT: f64 = 25.0;
const SMALL_MAX_PLEDGE_PCT: f64 = 10.0;
const PE_STRETCH_BUY: f64 = 1.2;
const PE_STRETCH_TRIM: f64 = 1.5;
const TRIM_GAIN_PCT: f64 = 50.0;
const EARNINGS_BLACKOUT_DAYS: i64 = 5;
const LONG_ATR_MULT: f64 = 2.5;
const SHORT_ATR_MULT: f64 = 1.5;
const SHORT_VOLUME_RATIO: f64 = 1.2;
const LONG_REVIEW_DAYS: i64 = 90;
const SHORT_REVIEW_SESSIONS: u32 = 10;
const TAX_WAIT_DAYS: i64 = 30;
const SMALL_CAP_MAX_CIRCUITS: usize = 2;
/// Market cap below which the small-cap gates apply: ₹5,000 crore in India,
/// $2 billion (US-dollar equivalent) elsewhere.
pub const SMALL_CAP_INR: f64 = 5.0e10;
pub const SMALL_CAP_USD: f64 = 2.0e9;

/// What the engine says to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kind {
    // Holdings, in review order.
    Exit,
    Trim,
    Add,
    Hold,
    // Stocks not held.
    Buy,
    Watch,
    Avoid,
    /// Not enough data, or not a listed stock.
    NoCall,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Buy => "BUY",
            Kind::Watch => "WATCH",
            Kind::Avoid => "AVOID",
            Kind::Add => "ADD",
            Kind::Hold => "HOLD",
            Kind::Trim => "TRIM",
            Kind::Exit => "EXIT",
            Kind::NoCall => "NO CALL",
        }
    }

    /// Whether this kind is for a stock you hold.
    pub fn is_holding(self) -> bool {
        matches!(self, Kind::Exit | Kind::Trim | Kind::Add | Kind::Hold)
    }

    /// Order for `portfolio review`: EXIT, TRIM, ADD, HOLD, then NO CALL.
    pub fn review_rank(self) -> u8 {
        match self {
            Kind::Exit => 0,
            Kind::Trim => 1,
            Kind::Add => 2,
            Kind::Hold => 3,
            Kind::Buy => 4,
            Kind::Watch => 5,
            Kind::Avoid => 6,
            Kind::NoCall => 7,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(
            match s.trim().to_ascii_uppercase().replace('_', " ").as_str() {
                "BUY" => Kind::Buy,
                "WATCH" => Kind::Watch,
                "AVOID" => Kind::Avoid,
                "ADD" => Kind::Add,
                "HOLD" => Kind::Hold,
                "TRIM" => Kind::Trim,
                "EXIT" => Kind::Exit,
                "NO CALL" | "NOCALL" => Kind::NoCall,
                _ => return None,
            },
        )
    }

    /// Positive (+1), negative (-1) or neutral (0), for pillar agreement.
    fn direction(self) -> i8 {
        match self {
            Kind::Buy | Kind::Add => 1,
            Kind::Avoid | Kind::Exit | Kind::Trim => -1,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

/// How long the action is meant for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Horizon {
    #[default]
    Long,
    Short,
}

impl From<crate::picks::Horizon> for Horizon {
    fn from(h: crate::picks::Horizon) -> Self {
        match h {
            crate::picks::Horizon::Long => Horizon::Long,
            crate::picks::Horizon::Short => Horizon::Short,
        }
    }
}

impl Horizon {
    pub fn as_str(self) -> &'static str {
        match self {
            Horizon::Long => "long",
            Horizon::Short => "short",
        }
    }
}

/// How big a position the rules allow.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SizeHint {
    /// Largest position, percent of the portfolio: the profile's
    /// `risk_per_trade` divided by the distance to the invalidation price,
    /// capped by `max_stock_weight` (half of it for a small cap).
    pub max_weight_pct: f64,
    /// For BUY/ADD: shares to buy to reach `max_weight_pct`; for TRIM: shares
    /// to sell to get back under the cap. `None` when the portfolio value is
    /// unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shares: Option<f64>,
    /// Which limit set the size: `risk_per_trade`, `max_stock_weight` or
    /// `small_cap_half_cap`.
    pub limited_by: &'static str,
}

/// The engine's answer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Action {
    pub kind: Kind,
    /// `kind` as printed (`BUY`, `NO CALL`).
    pub label: &'static str,
    pub confidence: Confidence,
    /// Up to three reasons, most important first.
    pub reasons: Vec<String>,
    /// A daily close below this price invalidates the action. Set for BUY,
    /// ADD, HOLD and TRIM.
    pub invalidation_price: Option<f64>,
    /// When to look again (`YYYY-MM-DD`). `None` only for NO CALL.
    pub review_by: Option<String>,
    pub size_hint: Option<SizeHint>,
    pub tax_note: Option<String>,
    pub horizon: Horizon,
    pub held: bool,
    /// Data coverage the action was decided on, 0 to 100.
    pub coverage: f64,
    /// Key fields no source could supply (filled for NO CALL).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    pub currency: String,
    pub methodology: &'static str,
    pub disclaimer: &'static str,
}

impl Action {
    /// A NO CALL for a value that has not been decided yet (replaced by the
    /// engine's answer before anything prints it).
    pub fn undecided(currency: &str) -> Self {
        let i = Inputs {
            currency: currency.to_string(),
            ..Default::default()
        };
        no_call(&i, "Not evaluated".to_string())
    }

    /// `ACTION: BUY · medium confidence · invalid below ₹2,410.50 · review by 21 Feb 2027`,
    /// or `ACTION: NO CALL — missing: ROE, D/E`.
    pub fn headline(&self) -> String {
        if self.kind == Kind::NoCall {
            let what = if self.missing.is_empty() {
                self.reasons.first().cloned().unwrap_or_default()
            } else {
                format!("missing: {}", self.missing.join(", "))
            };
            return format!("ACTION: NO CALL — {what}");
        }
        let mut parts = vec![
            format!("ACTION: {}", self.label),
            format!("{} confidence", self.confidence.label()),
        ];
        if let Some(p) = self.invalidation_price {
            parts.push(format!(
                "invalid below {}{}",
                crate::market::currency_symbol(Some(&self.currency)),
                money(p)
            ));
        }
        if let Some(d) = self.review_by.as_deref().and_then(parse_date) {
            parts.push(format!("review by {}", d.format("%d %b %Y")));
        }
        parts.join(" · ")
    }

    /// The headline, the reasons, the size hint and tax note, and the
    /// disclaimer, one per line with `indent`.
    pub fn render(&self, indent: &str) -> String {
        let mut o = format!("{indent}{}\n", self.headline());
        for r in &self.reasons {
            o.push_str(&format!("{indent}  • {r}\n"));
        }
        if let Some(s) = &self.size_hint {
            o.push_str(&format!("{indent}  • {}\n", size_text(self, s)));
        }
        if let Some(t) = &self.tax_note {
            o.push_str(&format!("{indent}  • Tax: {t}\n"));
        }
        o.push_str(&format!(
            "{indent}  Methodology {} ({} horizon). {}\n",
            self.methodology,
            self.horizon.as_str(),
            self.disclaimer
        ));
        o
    }

    /// Short cell for tables: `BUY (med)`, `NO CALL`.
    pub fn cell(&self) -> String {
        match self.kind {
            Kind::NoCall => "NO CALL".to_string(),
            k => format!(
                "{} ({})",
                k.label(),
                match self.confidence {
                    Confidence::High => "high",
                    Confidence::Medium => "med",
                    Confidence::Low => "low",
                }
            ),
        }
    }
}

fn size_text(a: &Action, s: &SizeHint) -> String {
    // "1 share", "2 shares": by the rounded number that is printed.
    let shares = |n: f64| {
        let n = format!("{n:.0}");
        let word = if n == "1" { "share" } else { "shares" };
        format!("{n} {word}")
    };
    match (a.kind, s.shares) {
        (Kind::Trim, Some(n)) if s.limited_by == "direct_holding_only" => format!(
            "Size: sell all {} directly held; fund look-through keeps combined weight at or above the {:.1}% cap ({})",
            shares(n),
            s.max_weight_pct,
            s.limited_by
        ),
        (Kind::Trim, Some(n)) => format!(
            "Size: sell about {} to get back to {:.1}% of the portfolio ({})",
            shares(n),
            s.max_weight_pct,
            s.limited_by
        ),
        (Kind::Trim, None) => format!(
            "Size: bring the position back to {:.1}% of the portfolio ({})",
            s.max_weight_pct, s.limited_by
        ),
        (_, Some(n)) => format!(
            "Size: at most {:.1}% of the portfolio, about {} ({})",
            s.max_weight_pct,
            shares(n),
            s.limited_by
        ),
        (_, None) => format!(
            "Size: at most {:.1}% of the portfolio ({})",
            s.max_weight_pct, s.limited_by
        ),
    }
}

fn money(v: f64) -> String {
    if v >= 1000.0 {
        let whole = format!("{:.2}", v);
        let (int, frac) = whole.split_once('.').unwrap_or((&whole, "00"));
        let mut out = String::new();
        for (i, c) in int.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        format!("{}.{frac}", out.chars().rev().collect::<String>())
    } else {
        format!("{v:.2}")
    }
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Price-derived measures the rules read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Technicals {
    /// Last daily close.
    pub close: Option<f64>,
    pub sma50: Option<f64>,
    pub sma200: Option<f64>,
    /// 14-session average true range.
    pub atr14: Option<f64>,
    /// Lowest low of the 20 sessions before the last one.
    pub low20: Option<f64>,
    /// Highest close of the last 20 sessions (the reference for a holding's
    /// trailing invalidation).
    pub high_close20: Option<f64>,
    /// Average volume of the last 20 sessions over the last 60.
    pub volume_ratio: Option<f64>,
    /// Average value traded per session over the last 20, in the quote
    /// currency.
    pub avg_traded_value: Option<f64>,
    /// Sessions in the last 20 that closed limit-like: a move of 5% or more
    /// that closed at the day's high or low (only counted for NSE/BSE).
    pub circuit_hits: Option<usize>,
}

impl Technicals {
    /// Measures from daily bars, oldest first. Slices of different lengths
    /// are aligned on their last bar.
    pub fn from_bars(close: &[f64], high: &[f64], low: &[f64], volume: &[f64]) -> Self {
        let n = close.len();
        let ohlc = n.min(high.len()).min(low.len());
        let (c, h, l) = (
            &close[n - ohlc..],
            &high[high.len() - ohlc..],
            &low[low.len() - ohlc..],
        );
        let avg = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
        let vols: Vec<f64> = volume.iter().copied().filter(|v| v.is_finite()).collect();
        let last_n = |v: &[f64], k: usize| v.len().checked_sub(k).map(|s| v[s..].to_vec());
        let vol20 = last_n(&vols, 20).and_then(|v| avg(&v));
        let vol60 = last_n(&vols, 60).and_then(|v| avg(&v));
        let traded = if n >= 20 && volume.len() >= 20 {
            let cs = &close[n - 20..];
            let vs = &volume[volume.len() - 20..];
            avg(&cs.iter().zip(vs).map(|(c, v)| c * v).collect::<Vec<_>>())
        } else {
            None
        };
        let prior_lows = (ohlc > 20).then(|| l[ohlc - 21..ohlc - 1].iter().copied());
        Technicals {
            close: close.last().copied(),
            sma50: crate::technical::sma(close, 50),
            sma200: crate::technical::sma(close, 200),
            atr14: crate::technical::atr(h, l, c, 14),
            low20: prior_lows.map(|it| it.fold(f64::INFINITY, f64::min)),
            high_close20: (n >= 20)
                .then(|| close[n - 20..].iter().copied().fold(f64::MIN, f64::max)),
            volume_ratio: vol20
                .zip(vol60)
                .filter(|(_, b)| *b > 0.0)
                .map(|(a, b)| a / b),
            avg_traded_value: traded,
            circuit_hits: None,
        }
        .with_circuits(c, h, l)
    }

    fn with_circuits(mut self, c: &[f64], h: &[f64], l: &[f64]) -> Self {
        let n = c.len();
        if n >= 21 {
            let hits = (n - 20..n)
                .filter(|&i| {
                    let prev = c[i - 1];
                    if prev <= 0.0 {
                        return false;
                    }
                    let mv = (c[i] / prev - 1.0).abs();
                    let at_edge =
                        (h[i] - c[i]).abs() <= c[i] * 0.001 || (c[i] - l[i]).abs() <= c[i] * 0.001;
                    mv >= 0.05 - 1e-9 && at_edge
                })
                .count();
            self.circuit_hits = Some(hits);
        }
        self
    }
}

/// The holding the action is about, when you own the stock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoldingContext {
    pub shares: f64,
    /// Average cost per share, quote currency.
    pub avg_cost: f64,
    /// Share of the portfolio, percent. `None` when the portfolio could not be
    /// valued.
    pub weight_pct: Option<f64>,
    /// Combined weight including what funds and ETFs in the portfolio hold
    /// of the same company (`xray` look-through), percent.
    pub lookthrough_weight_pct: Option<f64>,
    /// Target weight from `portfolio target`, percent.
    pub target_weight_pct: Option<f64>,
    /// Limits from `notes.json` that the stock now breaks.
    pub thesis_breaches: Vec<String>,
    /// The next lot to turn long-term: days, date and the tax that waiting
    /// saves (quote currency).
    pub next_long_term: Option<(i64, NaiveDate, Option<f64>)>,
}

/// Everything the rules read. Fields the data did not have stay `None`; a
/// rule whose input is missing is not hit, and coverage decides whether a
/// call is made at all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub symbol: String,
    pub currency: String,
    pub today: NaiveDate,
    pub horizon: Horizon,
    /// Whether the instrument is a listed stock (funds, ETFs without data,
    /// indices, FX and crypto get NO CALL).
    pub is_stock: bool,
    /// Last price (the last close when markets are shut).
    pub price: Option<f64>,
    /// Data coverage, 0 to 100.
    pub coverage: f64,
    pub missing: Vec<String>,
    /// Composite score, 0 to 100.
    pub composite: Option<f64>,
    /// Safety / balance-sheet pillar score, 0 to 100.
    pub safety: Option<f64>,
    /// Valuation pillar score, 0 to 100.
    pub valuation: Option<f64>,
    /// Every pillar score that had data.
    pub pillars: Vec<f64>,
    pub altman_distress: bool,
    pub book_value: Option<f64>,
    pub eps: Option<f64>,
    pub profit_margin: Option<f64>,
    /// Debt to equity as a ratio (1.5 = 150%).
    pub debt_to_equity: Option<f64>,
    pub is_financial: bool,
    /// Promoter shares pledged, percent of promoter holding.
    pub pledge_pct: Option<f64>,
    /// Whether any promoter shares are pledged (when the percentage is not
    /// known).
    pub pledged: Option<bool>,
    pub pe: Option<f64>,
    /// The stock's own median trailing P/E over the history available (up to
    /// five years).
    pub pe_own_median: Option<f64>,
    /// The sector's P/E (NSE sector index) or the median of its peers.
    pub pe_sector: Option<f64>,
    /// The median P/E of a broad set of stocks (the `picks` scan), used only
    /// when there is neither an own median nor a sector figure.
    pub pe_market: Option<f64>,
    pub tech: Technicals,
    pub next_earnings: Option<NaiveDate>,
    pub market_cap: Option<f64>,
    /// Least average traded value for the cap tier (see `picks` floors), in
    /// the quote currency.
    pub adv_floor: Option<f64>,
    /// Auditor resignation or a qualified / adverse opinion found in filings.
    pub auditor_flag: Option<String>,
    /// ASM / GSM surveillance stage or suspension, from the exchange.
    pub surveillance: Option<String>,
    pub holding: Option<HoldingContext>,
    /// Profile limits, percent.
    pub max_stock_weight: f64,
    pub risk_per_trade: f64,
    /// Portfolio value in the quote currency, for share counts.
    pub portfolio_value: Option<f64>,
}

impl Inputs {
    /// Defaults that every caller shares: today, the long horizon, and the
    /// active profile's limits.
    pub fn new(symbol: &str, currency: &str, today: NaiveDate) -> Self {
        let p = crate::config::EffectiveProfile::default();
        Inputs {
            symbol: symbol.to_string(),
            currency: currency.to_string(),
            today,
            is_stock: true,
            max_stock_weight: p.max_stock_weight,
            risk_per_trade: p.risk_per_trade,
            ..Default::default()
        }
    }

    /// Take the position limits from `p`.
    pub fn with_profile(mut self, p: &crate::config::EffectiveProfile) -> Self {
        self.max_stock_weight = p.max_stock_weight;
        self.risk_per_trade = p.risk_per_trade;
        self
    }

    /// Whether the small-cap gates apply: market cap below ₹5,000 crore or
    /// $2 billion.
    pub fn is_small_cap(&self) -> bool {
        let Some(m) = self.market_cap.filter(|m| *m > 0.0) else {
            return false;
        };
        if self.currency.eq_ignore_ascii_case("INR") {
            return m < SMALL_CAP_INR;
        }
        // Yahoo reports London prices in pence (`GBp`/`GBX`) while `market_cap`
        // is already in pounds (`GBP`).
        let cap_ccy = crate::market::minor_unit(&self.currency)
            .map_or(self.currency.as_str(), |(major, _)| major);
        crate::market::approx_usd(m, Some(cap_ccy)).is_some_and(|usd| usd < SMALL_CAP_USD)
    }

    fn pe_reference(&self) -> Option<(f64, &'static str)> {
        match (self.pe_own_median, self.pe_sector) {
            (Some(o), Some(s)) if o > 0.0 && s > 0.0 => {
                // The more generous of the two, as the rule says "or".
                if o >= s {
                    Some((o, "its own median P/E"))
                } else {
                    Some((s, "the sector P/E"))
                }
            }
            (Some(o), _) if o > 0.0 => Some((o, "its own median P/E")),
            (_, Some(s)) if s > 0.0 => Some((s, "the sector P/E")),
            _ => self
                .pe_market
                .filter(|m| *m > 0.0)
                .map(|m| (m, "the market median P/E")),
        }
    }

    fn loss_making(&self) -> bool {
        self.eps.is_some_and(|e| e < 0.0) || self.profit_margin.is_some_and(|m| m < 0.0)
    }

    fn csym(&self) -> &'static str {
        crate::market::currency_symbol(Some(&self.currency))
    }
}

/// Hard gates: any one makes a stock you don't hold AVOID and one you hold
/// EXIT. Returns the reasons.
pub fn hard_gates(i: &Inputs) -> Vec<String> {
    let mut g = Vec::new();
    if i.altman_distress {
        g.push("Altman Z-score is in the distress zone".to_string());
    }
    if i.book_value.is_some_and(|b| b < 0.0) {
        g.push("Negative shareholders' equity (book value below zero)".to_string());
    }
    if !i.is_financial
        && i.loss_making()
        && let Some(de) = i.debt_to_equity.filter(|d| *d > MAX_DE)
    {
        g.push(format!(
            "Loss-making with debt to equity {de:.1}x (above {MAX_DE:.1}x)"
        ));
    }
    if let Some(p) = i.pledge_pct.filter(|p| *p > MAX_PLEDGE_PCT) {
        g.push(format!(
            "Promoter pledge {p:.0}% (above {MAX_PLEDGE_PCT:.0}%)"
        ));
    }
    if let Some(c) = i.composite.filter(|c| *c < AVOID_COMPOSITE)
        && i.holding.is_none()
    {
        g.push(format!(
            "Composite score {c:.0} (below {AVOID_COMPOSITE:.0})"
        ));
    }
    g
}

/// Small-cap gates (market cap below ₹5,000 crore / $2 billion).
pub fn small_cap_gates(i: &Inputs) -> Vec<String> {
    let mut g = Vec::new();
    if !i.is_small_cap() {
        return g;
    }
    if let Some(p) = i.pledge_pct.filter(|p| *p > SMALL_MAX_PLEDGE_PCT) {
        g.push(format!(
            "Small cap with promoter pledge {p:.0}% (above {SMALL_MAX_PLEDGE_PCT:.0}%)"
        ));
    }
    if let (Some(adv), Some(floor)) = (i.tech.avg_traded_value, i.adv_floor)
        && adv < floor
    {
        g.push(format!(
            "Small cap trading {}{} a day on average, below the {}{} floor",
            i.csym(),
            short_amount(adv, &i.currency),
            i.csym(),
            short_amount(floor, &i.currency)
        ));
    }
    if let Some(n) = i.tech.circuit_hits.filter(|n| *n > SMALL_CAP_MAX_CIRCUITS) {
        g.push(format!(
            "Small cap hit a price band {n} times in 20 sessions"
        ));
    }
    if let Some(a) = &i.auditor_flag {
        g.push(format!("Small cap with an auditor red flag: {a}"));
    }
    if let Some(s) = &i.surveillance {
        g.push(format!("Small cap on exchange surveillance: {s}"));
    }
    g
}

fn short_amount(v: f64, currency: &str) -> String {
    if currency.eq_ignore_ascii_case("INR") {
        if v >= 1e7 {
            format!("{:.1} crore", v / 1e7)
        } else {
            format!("{:.1} lakh", v / 1e5)
        }
    } else if v >= 1e9 {
        format!("{:.1} billion", v / 1e9)
    } else {
        format!("{:.1} million", v / 1e6)
    }
}

/// The price whose daily close invalidates the action.
///
/// - Long horizon: max(200-day average, reference − 2.5 × ATR).
/// - Short horizon: max(reference − 1.5 × ATR, lowest low of the prior 20
///   sessions).
///
/// The reference is the last close for a stock you don't hold, and the
/// highest close of the last 20 sessions for a holding (so the level trails
/// the price up and does not move with each day's close).
pub fn invalidation(i: &Inputs) -> Option<f64> {
    let t = &i.tech;
    let close = t.close.or(i.price)?;
    let reference = if i.holding.is_some() {
        t.high_close20.unwrap_or(close).max(close)
    } else {
        close
    };
    let atr = t.atr14;
    let level = match i.horizon {
        Horizon::Long => {
            let atr_stop = atr.map(|a| reference - LONG_ATR_MULT * a);
            match (t.sma200, atr_stop) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            }
        }
        Horizon::Short => {
            let atr_stop = atr.map(|a| reference - SHORT_ATR_MULT * a);
            match (atr_stop, t.low20) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            }
        }
    };
    level
        .filter(|l| l.is_finite() && *l > 0.0)
        .map(round_price)
        .filter(|l| *l > 0.0)
}

fn round_price(v: f64) -> f64 {
    if v >= 1.0 {
        (v * 100.0).round() / 100.0
    } else if v >= 0.01 {
        (v * 10_000.0).round() / 10_000.0
    } else {
        (v * 1_000_000.0).round() / 1_000_000.0
    }
}

/// The next review date: the next earnings date if it comes within 90 days,
/// else 90 days out; 10 trading sessions out on the short horizon.
pub fn review_by(i: &Inputs) -> NaiveDate {
    match i.horizon {
        Horizon::Short => add_sessions(i.today, SHORT_REVIEW_SESSIONS),
        Horizon::Long => {
            let limit = i.today + Duration::days(LONG_REVIEW_DAYS);
            match i.next_earnings {
                Some(e) if e > i.today && e <= limit => e,
                _ => limit,
            }
        }
    }
}

fn add_sessions(from: NaiveDate, n: u32) -> NaiveDate {
    let mut d = from;
    let mut left = n;
    while left > 0 {
        d += Duration::days(1);
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
            left -= 1;
        }
    }
    d
}

fn earnings_soon(i: &Inputs) -> Option<i64> {
    let e = i.next_earnings?;
    let days = (e - i.today).num_days();
    (0..=EARNINGS_BLACKOUT_DAYS).contains(&days).then_some(days)
}

/// Weight cap for this stock, percent: the profile's `max_stock_weight`,
/// halved for a small cap.
fn weight_cap(i: &Inputs) -> (f64, &'static str) {
    if i.is_small_cap() {
        (i.max_stock_weight / 2.0, "small_cap_half_cap")
    } else {
        (i.max_stock_weight, "max_stock_weight")
    }
}

fn size_for_entry(i: &Inputs, inv: Option<f64>) -> Option<SizeHint> {
    let price = i.tech.close.or(i.price)?;
    let inv = inv.filter(|v| *v < price)?;
    let risk_weight = i.risk_per_trade * price / (price - inv);
    let (cap, cap_name) = weight_cap(i);
    // What funds and ETFs in the portfolio already hold of the company counts
    // against the cap, so only the rest is left for the direct holding. With
    // no look-through weight (or no direct weight) the cap applies as is.
    let indirect = i
        .holding
        .as_ref()
        .and_then(|h| Some((h.lookthrough_weight_pct? - h.weight_pct?).max(0.0)))
        .unwrap_or(0.0);
    let cap = (cap - indirect).max(0.0);
    let (mut w, mut by) = if risk_weight <= cap {
        (risk_weight, "risk_per_trade")
    } else {
        (cap, cap_name)
    };
    if let Some(t) = i
        .holding
        .as_ref()
        .and_then(|h| h.target_weight_pct)
        .filter(|t| *t > 0.0)
        && t < w
    {
        w = t;
        by = "target_weight";
    }
    let held = i.holding.as_ref().map_or(0.0, |h| h.shares);
    let shares = i
        .portfolio_value
        .filter(|v| *v > 0.0 && price > 0.0)
        .map(|v| ((w / 100.0 * v / price).floor() - held).max(0.0));
    Some(SizeHint {
        max_weight_pct: round1(w),
        shares,
        limited_by: by,
    })
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn confidence(i: &Inputs, kind: Kind, decisive_gate: bool) -> Confidence {
    let strong = i.pillars.iter().filter(|s| **s >= 65.0).count();
    let weak = i.pillars.iter().filter(|s| **s < 45.0).count();
    let mixed = strong >= 2 && weak >= 2;
    let agree = decisive_gate
        || match kind.direction() {
            1 => weak == 0,
            -1 => strong == 0 || weak > strong,
            _ => strong == 0 || weak == 0,
        };
    if i.coverage < 70.0 || mixed {
        Confidence::Low
    } else if i.coverage >= 85.0 && agree {
        Confidence::High
    } else {
        Confidence::Medium
    }
}

fn no_call(i: &Inputs, why: String) -> Action {
    Action {
        kind: Kind::NoCall,
        label: Kind::NoCall.label(),
        confidence: Confidence::Low,
        reasons: vec![why],
        invalidation_price: None,
        review_by: None,
        size_hint: None,
        tax_note: None,
        horizon: i.horizon,
        held: i.holding.is_some(),
        coverage: round1(i.coverage),
        missing: i.missing.clone(),
        currency: i.currency.clone(),
        methodology: METHODOLOGY,
        disclaimer: DISCLAIMER,
    }
}

fn finish(i: &Inputs, kind: Kind, mut reasons: Vec<String>, decisive_gate: bool) -> Action {
    reasons.retain(|r| !r.is_empty());
    reasons.dedup();
    reasons.truncate(3);
    if reasons.is_empty() {
        reasons.push("No rule was triggered either way".to_string());
    }
    let inv = invalidation(i);
    let shows_level = matches!(kind, Kind::Buy | Kind::Add | Kind::Hold | Kind::Trim);
    if shows_level && inv.is_none() {
        let (why, field) = match i.horizon {
            Horizon::Long => (
                "No 200-day average or 14-day ATR to set an invalidation price".to_string(),
                "200-day average",
            ),
            Horizon::Short => (
                "No 14-day ATR or 20-day low to set an invalidation price".to_string(),
                "price history",
            ),
        };
        if i.holding.is_some() {
            let mut a = no_call(i, why);
            if !a.missing.iter().any(|m| m == field) {
                a.missing.push(field.to_string());
            }
            return a;
        }
        let mut r = vec![why];
        r.extend(reasons);
        return finish(i, Kind::Watch, r, false);
    }
    let size_hint = match kind {
        Kind::Buy | Kind::Add => size_for_entry(i, inv),
        Kind::Trim => trim_size(i),
        _ => None,
    };
    let tax_note = if matches!(kind, Kind::Trim | Kind::Exit) && !decisive_gate {
        tax_note(i)
    } else {
        None
    };
    Action {
        kind,
        label: kind.label(),
        confidence: confidence(i, kind, decisive_gate),
        reasons,
        invalidation_price: if shows_level { inv } else { None },
        review_by: Some(review_by(i).to_string()),
        size_hint,
        tax_note,
        horizon: i.horizon,
        held: i.holding.is_some(),
        coverage: round1(i.coverage),
        missing: Vec::new(),
        currency: i.currency.clone(),
        methodology: METHODOLOGY,
        disclaimer: DISCLAIMER,
    }
}

fn trim_size(i: &Inputs) -> Option<SizeHint> {
    let h = i.holding.as_ref()?;
    let (cap, by) = weight_cap(i);
    let direct = h.weight_pct?;
    let total = h.lookthrough_weight_pct.map_or(direct, |lw| lw.max(direct));
    if total <= cap || direct <= 0.0 {
        return Some(SizeHint {
            max_weight_pct: round1(cap),
            shares: None,
            limited_by: by,
        });
    }
    // Direct reduction needed to bring combined exposure back to `cap`,
    // capped at the entire direct holding when indirect fund exposure
    // alone stays at or above `cap`.
    let reduce_pct = (total - cap).min(direct);
    let (shares, limited_by) = if total - direct >= cap {
        (h.shares.ceil(), "direct_holding_only")
    } else {
        ((h.shares * (reduce_pct / direct)).ceil().min(h.shares), by)
    };
    Some(SizeHint {
        max_weight_pct: round1(cap),
        shares: Some(shares),
        limited_by,
    })
}

fn tax_note(i: &Inputs) -> Option<String> {
    let (days, date, saving) = i.holding.as_ref()?.next_long_term?;
    if !(0..=TAX_WAIT_DAYS).contains(&days) {
        return None;
    }
    let saving = saving.filter(|s| *s > 0.0)?;
    Some(format!(
        "a lot turns long-term on {} — waiting {days} day{} saves about {}{} in tax",
        date.format("%d %b %Y"),
        if days == 1 { "" } else { "s" },
        i.csym(),
        money(saving)
    ))
}

/// An exchange-traded fund Yahoo labels EQUITY (NSE's NIFTYBEES, for one):
/// "ETF" or "BeES" in its name, or a known index fund.
fn fund_by_name(symbol: &str, q: &crate::api::Quote) -> bool {
    let name = q.long_name.as_deref().or(q.short_name.as_deref());
    let etf_word = name.is_some_and(|n| {
        n.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w.eq_ignore_ascii_case("etf") || w.eq_ignore_ascii_case("bees"))
    });
    etf_word || crate::index_weights::index_of_fund(symbol, name).is_some()
}

/// The action for `i`. Deterministic: no clock, no network, no randomness.
pub fn decide(i: &Inputs) -> Action {
    if !i.is_stock {
        let mut a = no_call(
            i,
            "Not a listed company stock: ETFs, funds, indices, FX and crypto have no \
             company data to judge"
                .to_string(),
        );
        // Company fields are not "missing" for an ETF: they never apply.
        a.missing.clear();
        return a;
    }
    if i.price.or(i.tech.close).is_none() {
        let mut a = no_call(i, "No price".to_string());
        if !a.missing.iter().any(|m| m == "price") {
            a.missing.insert(0, "price".to_string());
        }
        return a;
    }
    if i.coverage < MIN_COVERAGE {
        return no_call(
            i,
            format!(
                "Data coverage {:.0}% is below the {MIN_COVERAGE:.0}% needed for a call",
                i.coverage
            ),
        );
    }
    if i.composite.is_none() {
        let mut a = no_call(i, "No composite score".to_string());
        if a.missing.is_empty() {
            a.missing.push("composite score".to_string());
        }
        return a;
    }
    match &i.holding {
        None => decide_new(i),
        Some(h) => decide_held(i, h),
    }
}

fn decide_new(i: &Inputs) -> Action {
    let mut gates = hard_gates(i);
    gates.extend(small_cap_gates(i));
    if !gates.is_empty() {
        return finish(i, Kind::Avoid, gates, true);
    }
    let composite = i.composite.unwrap_or(0.0);
    let price = i.tech.close.or(i.price).unwrap_or(0.0);
    let mut met = Vec::new();
    let mut unmet = Vec::new();

    if composite >= BUY_COMPOSITE {
        met.push(format!("Composite score {composite:.0} (65 or more)"));
    } else {
        unmet.push(format!("Composite score {composite:.0} is below 65"));
    }
    match i.safety {
        Some(s) if s < 45.0 => unmet.push(format!("Safety pillar is Weak ({s:.0})")),
        _ => {}
    }
    match i.tech.sma200 {
        Some(a) if price > a => met.push(format!(
            "Price above its 200-day average ({}{})",
            i.csym(),
            money(a)
        )),
        Some(a) => unmet.push(format!(
            "Price below its 200-day average ({}{})",
            i.csym(),
            money(a)
        )),
        None => unmet.push("Too little price history for a 200-day average".to_string()),
    }
    match (i.pe.filter(|p| *p > 0.0), i.pe_reference()) {
        (Some(pe), Some((r, what))) if pe <= PE_STRETCH_BUY * r => {
            met.push(format!("P/E {pe:.1} within 1.2x {what} ({r:.1})"))
        }
        (Some(pe), Some((r, what))) => {
            unmet.push(format!("P/E {pe:.1} is more than 1.2x {what} ({r:.1})"))
        }
        (None, _) => unmet.push("No positive P/E to check valuation against".to_string()),
        (Some(_), None) => {
            unmet.push("No P/E history or sector P/E to check valuation against".to_string())
        }
    }
    if i.horizon == Horizon::Short {
        match i.tech.sma50 {
            Some(a) if price > a => {}
            Some(a) => unmet.push(format!(
                "Price below its 50-day average ({}{})",
                i.csym(),
                money(a)
            )),
            None => unmet.push("Too little price history for a 50-day average".to_string()),
        }
        match i.tech.volume_ratio {
            Some(v) if v >= SHORT_VOLUME_RATIO => {
                met.push(format!("Volume picking up (20/60-day ratio {v:.2})"))
            }
            Some(v) => unmet.push(format!(
                "Volume not picking up (20/60-day ratio {v:.2}, needs 1.2)"
            )),
            None => unmet.push("Too little volume history".to_string()),
        }
    }
    let inv = invalidation(i);
    if unmet.is_empty() && inv.is_some_and(|v| v >= price) {
        unmet.push("Price is already at its invalidation level".to_string());
    }
    if unmet.is_empty() {
        if let Some(days) = earnings_soon(i) {
            let mut r = vec![format!(
                "Results due in {days} day{}: waiting for them (rules met otherwise)",
                if days == 1 { "" } else { "s" }
            )];
            r.extend(met);
            return finish(i, Kind::Watch, r, false);
        }
        return finish(i, Kind::Buy, met, false);
    }
    let mut r = unmet;
    r.extend(met);
    finish(i, Kind::Watch, r, false)
}

fn decide_held(i: &Inputs, h: &HoldingContext) -> Action {
    let composite = i.composite.unwrap_or(0.0);
    let price = i.tech.close.or(i.price).unwrap_or(0.0);

    // EXIT.
    let mut gates = hard_gates(i);
    gates.extend(small_cap_gates(i));
    let gated = !gates.is_empty();
    let mut exit = gates;
    if composite < EXIT_COMPOSITE {
        exit.push(format!(
            "Composite score {composite:.0} (below {EXIT_COMPOSITE:.0})"
        ));
    }
    exit.extend(
        h.thesis_breaches
            .iter()
            .map(|b| format!("Your thesis limit is broken: {b}")),
    );
    let inv = invalidation(i);
    if let Some(level) = inv
        && price < level
    {
        exit.insert(
            0,
            format!(
                "Closed at {}{} below the invalidation level {}{}",
                i.csym(),
                money(price),
                i.csym(),
                money(level)
            ),
        );
    }
    if !exit.is_empty() {
        return finish(i, Kind::Exit, exit, gated);
    }

    // TRIM.
    let (cap, _) = weight_cap(i);
    let mut trim = Vec::new();
    if let Some(w) = h.weight_pct.filter(|w| *w > cap) {
        trim.push(format!(
            "Weight {w:.1}% is above the {cap:.1}% cap for one stock"
        ));
    }
    let gain_pct = (h.avg_cost > 0.0).then(|| (price / h.avg_cost - 1.0) * 100.0);
    let stretched = match (i.pe.filter(|p| *p > 0.0), i.pe_reference()) {
        (Some(pe), Some((r, what))) if pe > PE_STRETCH_TRIM * r => {
            Some(format!("P/E {pe:.1} is above 1.5x {what} ({r:.1})"))
        }
        (_, None) => i
            .valuation
            .filter(|v| *v < 35.0)
            .map(|v| format!("Valuation pillar {v:.0} (stretched)")),
        _ => None,
    };
    if let (Some(s), Some(g)) = (stretched, gain_pct.filter(|g| *g > TRIM_GAIN_PCT)) {
        trim.push(format!("{s}, with a {g:.0}% gain"));
    }
    if let Some(lw) = h.lookthrough_weight_pct.filter(|w| *w > cap)
        && h.weight_pct.is_none_or(|w| w <= cap)
    {
        trim.push(format!(
            "With what your funds hold of it, the company is {lw:.1}% of the portfolio (cap {cap:.1}%)"
        ));
    }
    if !trim.is_empty() {
        return finish(i, Kind::Trim, trim, false);
    }

    // ADD.
    let trend_up = match i.horizon {
        Horizon::Long => matches!(
            (i.tech.sma50, i.tech.sma200),
            (Some(f), Some(s)) if price > s && f > s
        ),
        Horizon::Short => matches!(
            (i.tech.sma50, i.tech.volume_ratio),
            (Some(f), Some(v)) if price > f && v >= SHORT_VOLUME_RATIO
        ),
    };
    // The cap is on the company: direct weight plus what funds hold of it,
    // when the look-through weight is known. A target is on the direct weight.
    let combined = h
        .weight_pct
        .map(|w| h.lookthrough_weight_pct.map_or(w, |lw| lw.max(w)));
    let room = match (h.weight_pct, combined, h.target_weight_pct) {
        (Some(w), Some(c), Some(t)) => w < t && c < cap,
        (Some(_), Some(c), None) => c < cap,
        _ => false,
    };
    if composite >= BUY_COMPOSITE && trend_up && room {
        let mut r = vec![
            format!("Composite score {composite:.0} (65 or more)"),
            "Trend up: price above a rising average".to_string(),
        ];
        match (h.weight_pct, combined) {
            (Some(w), Some(c)) if c > w => r.push(format!(
                "Weight {w:.1}% ({c:.1}% with what your funds hold) leaves room under the {cap:.1}% cap"
            )),
            (Some(w), _) => r.push(format!(
                "Weight {w:.1}% leaves room under the {cap:.1}% cap"
            )),
            _ => {}
        }
        if let Some(days) = earnings_soon(i) {
            r.insert(
                0,
                format!(
                    "Results due in {days} day{}: holding until they are out",
                    if days == 1 { "" } else { "s" }
                ),
            );
            return finish(i, Kind::Hold, r, false);
        }
        return finish(i, Kind::Add, r, false);
    }

    // HOLD.
    let mut r = vec![format!(
        "Composite score {composite:.0}: no exit, trim or add rule triggered"
    )];
    if composite >= BUY_COMPOSITE && !trend_up {
        r.push(match i.horizon {
            Horizon::Long => {
                "Trend not up (price and 50-day average not both above the 200-day), so no add"
                    .to_string()
            }
            Horizon::Short => {
                "Price not above its 50-day average on rising volume, so no add".to_string()
            }
        });
    } else if composite >= BUY_COMPOSITE && !room && h.weight_pct.is_some() {
        r.push("No room left under the weight cap or your target, so no add".to_string());
    }
    if h.weight_pct.is_none() {
        r.push(
            "Portfolio weight unknown: run `stockwise portfolio review` for weight checks".into(),
        );
    } else if let Some(level) = inv {
        r.push(format!(
            "Price {}{} is above the invalidation level {}{}",
            i.csym(),
            money(price),
            i.csym(),
            money(level)
        ));
    }
    finish(i, Kind::Hold, r, false)
}

// ---------------------------------------------------------------------------
// Builders: from a research bundle (single stock) and from a plain quote
// (portfolio, picks).

/// The action for a researched stock you don't hold (or, with `holding`,
/// one you do).
pub fn for_research(
    r: &crate::research::Research,
    v: &crate::verdict::DecisionVerdict,
    profile: &crate::config::EffectiveProfile,
    horizon: Horizon,
    holding: Option<HoldingContext>,
) -> Action {
    let today = chrono::Local::now().date_naive();
    let mut i = inputs_from_research(r, v, horizon, today).with_profile(profile);
    i.holding = holding;
    decide(&i)
}

/// Inputs from a full research bundle and its 6-pillar score.
pub fn inputs_from_research(
    r: &crate::research::Research,
    v: &crate::verdict::DecisionVerdict,
    horizon: Horizon,
    today: NaiveDate,
) -> Inputs {
    let mut i = Inputs::new(&r.symbol, &v.currency, today);
    i.horizon = horizon;
    // Same test as `inputs_from_quote`: an ETF or fund says it is not a
    // listed company instead of failing on data coverage.
    i.is_stock = crate::market::AssetClass::of(&r.symbol) == crate::market::AssetClass::Stock
        && !crate::offmarket::is_fund(&r.symbol)
        && !crate::offmarket::is_sgb(&r.symbol)
        && !r
            .quote
            .quote_type
            .as_deref()
            .is_some_and(|t| !t.eq_ignore_ascii_case("EQUITY"))
        && !fund_by_name(&r.symbol, &r.quote);
    i.price = r.price();
    // The key-field coverage from the data layer (`meta.coverage`), or the
    // pillar-input coverage when the bundle carries no provenance.
    i.coverage = if r.meta.sources.is_empty() && r.meta.missing.is_empty() {
        v.data_coverage
    } else {
        r.meta.coverage
    };
    i.missing = r.meta.missing.clone();
    if i.missing.is_empty() {
        i.missing = v
            .pillars
            .iter()
            .filter(|p| p.score.is_none())
            .map(|p| p.name.to_string())
            .collect();
    }
    i.composite = v.overall_score;
    let pillar = |name: &str| {
        v.pillars
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.score)
    };
    i.safety = pillar("Balance Sheet");
    i.valuation = pillar("Valuation");
    i.pillars = v.pillars.iter().filter_map(|p| p.score).collect();
    if i.is_stock {
        let sc = crate::scorecard::generate_scorecard_report(r);
        i.altman_distress = sc.altman.zone == Some(crate::scorecard::AltmanZone::Distress)
            || sc.altman.z_double_prime_zone == Some(crate::scorecard::AltmanZone::Distress);
    }
    i.book_value = r.fin.book_value.or(r.quote.book_value);
    i.eps = r.fin.trailing_eps.or(r.quote.eps_trailing_twelve_months);
    i.profit_margin = r.fin.profit_margin;
    i.debt_to_equity = r.fin.de.map(|d| d / 100.0);
    i.is_financial = r.is_financial();
    i.pledged = r.holdings.last().and_then(|h| h.pledged);
    i.pe = r.fin.trailing_pe.or(r.quote.trailing_pe);
    i.pe_own_median = own_median_pe(r);
    i.pe_sector = r.quote.sector_pe;
    i.tech = Technicals::from_bars(
        &r.prices.close,
        &r.prices.high,
        &r.prices.low,
        &r.prices.volume,
    );
    if !(r.symbol.ends_with(".NS") || r.symbol.ends_with(".BO")) {
        i.tech.circuit_hits = None;
    }
    if i.tech.sma200.is_none() {
        i.tech.sma200 = r.quote.two_hundred_day_average;
    }
    if i.tech.sma50.is_none() {
        i.tech.sma50 = r.quote.fifty_day_average;
    }
    i.next_earnings = r
        .calendar
        .next_earnings
        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
        .map(|d| d.date_naive());
    i.market_cap = r.quote.market_cap;
    i.adv_floor = adv_floor(&r.symbol, &v.currency, r.quote.market_cap);
    i.auditor_flag = auditor_flag(r.filings.iter().map(|f| f.title.as_str()));
    i.surveillance = surveillance(r.meta.warnings.iter().chain(r.warnings.iter()));
    i
}

/// Inputs from a quote, its daily bars and its long-term score: what
/// `portfolio review` and `picks` have without a full research fetch.
pub fn inputs_from_quote(
    q: &crate::api::Quote,
    bars: &crate::research::Series,
    score: &crate::longterm::LongTermScore,
    horizon: Horizon,
    today: NaiveDate,
) -> Inputs {
    let symbol = q.symbol.clone().unwrap_or_default();
    let currency = q.currency.clone().unwrap_or_else(|| "USD".into());
    let mut i = Inputs::new(&symbol, &currency, today);
    i.horizon = horizon;
    i.is_stock = crate::market::AssetClass::of(&symbol) == crate::market::AssetClass::Stock
        && !crate::offmarket::is_fund(&symbol)
        && !crate::offmarket::is_sgb(&symbol)
        && !q
            .quote_type
            .as_deref()
            .is_some_and(|t| !t.eq_ignore_ascii_case("EQUITY"))
        && !fund_by_name(&symbol, q);
    i.price = q.regular_market_price;
    let meta = crate::provenance::for_fundamentals(q);
    i.coverage = meta.coverage;
    i.missing = meta.missing.clone();
    i.composite = score.total();
    i.safety = score.pillar_data[5].then_some(score.safety_score);
    i.valuation = score.pillar_data[0].then_some(score.valuation_score);
    let all = [
        score.valuation_score,
        score.growth_score,
        score.quality_score,
        score.momentum_score,
        score.dividend_score,
        score.safety_score,
    ];
    i.pillars = all
        .iter()
        .zip(score.pillar_data.iter())
        .filter(|(_, has)| **has)
        .map(|(s, _)| *s)
        .collect();
    i.book_value = q.book_value;
    i.eps = q.eps_trailing_twelve_months;
    i.profit_margin = q.profit_margins;
    i.debt_to_equity = q.debt_to_equity.map(|d| d / 100.0);
    i.is_financial = crate::provenance::is_financial(q);
    i.pe = q.trailing_pe;
    i.pe_sector = q.sector_pe;
    i.tech = Technicals::from_bars(&bars.close, &bars.high, &bars.low, &bars.volume);
    if !(symbol.ends_with(".NS") || symbol.ends_with(".BO")) {
        i.tech.circuit_hits = None;
    }
    if i.tech.sma200.is_none() {
        i.tech.sma200 = q.two_hundred_day_average;
    }
    if i.tech.sma50.is_none() {
        i.tech.sma50 = q.fifty_day_average;
    }
    // A plain quote carries no earnings date; callers that know it set
    // `next_earnings` themselves.
    i.market_cap = q.market_cap;
    i.adv_floor = adv_floor(&symbol, &currency, q.market_cap);
    if let Some(m) = &q.meta {
        i.surveillance = surveillance(m.warnings.iter());
    }
    i
}

/// The stock's median trailing P/E at each quarter end in the price history
/// (trailing four quarters of EPS over that day's close), when at least
/// three quarters can be measured.
pub fn own_median_pe(r: &crate::research::Research) -> Option<f64> {
    let q = &r.quarters;
    if q.len() < 4 || r.prices.dates.is_empty() {
        return None;
    }
    // Reported EPS, else net income over today's share count (buybacks make
    // older quarters' EPS a little low, so their P/E a little high).
    let shares = r.fin.shares.filter(|s| *s > 0.0);
    let eps_of = |p: &crate::research::Period| p.eps.or_else(|| Some(p.net_income? / shares?));
    let mut pes = Vec::new();
    for k in 3..q.len() {
        let eps: Option<f64> = q[k - 3..=k].iter().map(eps_of).sum();
        let Some(eps) = eps.filter(|e| *e > 0.0) else {
            continue;
        };
        let end = q[k].end;
        let idx = r.prices.dates.partition_point(|d| *d <= end);
        if idx == 0 || idx > r.prices.close.len() {
            continue;
        }
        // Only quarter ends inside the history (not before its first bar or after its last).
        if r.prices.dates[0] > end || r.prices.dates.last().is_some_and(|last| end > *last) {
            continue;
        }
        let (close_major, _) =
            crate::market::to_major(r.prices.close[idx - 1], r.currency.as_deref().unwrap_or(""));
        pes.push(close_major / eps);
    }
    median(&mut pes).filter(|_| pes.len() >= 3)
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// The `picks` liquidity floor for a small or micro cap, in the quote
/// currency (US and India only).
pub fn adv_floor(symbol: &str, currency: &str, mcap: Option<f64>) -> Option<f64> {
    use crate::market::{CapFilter, CapTier, Market};
    let market = if currency.eq_ignore_ascii_case("INR") {
        Market::In
    } else if currency.eq_ignore_ascii_case("USD") {
        Market::Us
    } else {
        return None;
    };
    let tier = CapTier::classify(symbol, mcap, Some(currency), None);
    let cap = match tier {
        CapTier::Micro => CapFilter::Micro,
        _ => CapFilter::Small,
    };
    let f = crate::picks::Floors::for_market(market, Some(cap), None, None);
    (f.min_adv > 0.0).then_some(f.min_adv)
}

/// An auditor resignation or a qualified / adverse opinion in filing titles.
pub fn auditor_flag<'a>(titles: impl IntoIterator<Item = &'a str>) -> Option<String> {
    for t in titles {
        let l = t.to_ascii_lowercase();
        let auditor = l.contains("auditor") || l.contains("statutory audit");
        let has_word = |phrase: &str| {
            l.match_indices(phrase).any(|(idx, _)| {
                let before = l[..idx].chars().next_back();
                let after = l[idx + phrase.len()..].chars().next();
                !before.is_some_and(|c| c.is_ascii_alphanumeric())
                    && !after.is_some_and(|c| c.is_ascii_alphanumeric())
            })
        };
        let bad_opinion = has_word("qualified opinion")
            || has_word("adverse opinion")
            || has_word("disclaimer of opinion")
            || (has_word("modified opinion") && !has_word("unmodified"));
        if (auditor && l.contains("resign")) || bad_opinion {
            return Some(t.trim().to_string());
        }
    }
    None
}

/// An exchange surveillance stage or suspension among data warnings.
pub fn surveillance<'a>(warnings: impl IntoIterator<Item = &'a String>) -> Option<String> {
    warnings
        .into_iter()
        .find(|w| {
            w.starts_with("NSE surveillance")
                || w.contains("Trading suspended")
                || w.contains("Delisted from")
        })
        .cloned()
}

/// The thesis limits in `notes.json` that a stock breaks.
pub fn thesis_breaches(
    note: Option<&crate::notes::Note>,
    pe: Option<f64>,
    roe: Option<f64>,
    weight_pct: Option<f64>,
) -> Vec<String> {
    let Some(n) = note else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let (Some(max), Some(pe)) = (n.max_pe, pe)
        && pe > max
    {
        out.push(format!("P/E {pe:.1} above your limit of {max:.1}"));
    }
    if let (Some(min), Some(roe)) = (n.min_roe_pct, roe)
        && roe * 100.0 < min
    {
        out.push(format!(
            "ROE {:.1}% below your limit of {min:.1}%",
            roe * 100.0
        ));
    }
    if let (Some(max), Some(w)) = (n.max_weight_pct, weight_pct)
        && w > max
    {
        out.push(format!("weight {w:.1}% above your limit of {max:.1}%"));
    }
    out
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Checks one action object: a known kind, one to three reasons, the
    /// disclaimer and methodology, a review date and (for BUY/ADD) an
    /// invalidation price below the price; NO CALL only below 60% coverage
    /// or without a price, composite or stock.
    pub fn check_action(a: &serde_json::Value) {
        let kind = a["kind"].as_str().expect("action.kind");
        let kind = Kind::parse(kind).unwrap_or_else(|| panic!("unknown action kind {kind}"));
        let reasons = a["reasons"].as_array().expect("action.reasons");
        assert!(
            (1..=3).contains(&reasons.len()),
            "action needs 1 to 3 reasons: {a}"
        );
        assert!(
            a["disclaimer"]
                .as_str()
                .is_some_and(|d| d.contains("not investment advice")),
            "action without the disclaimer: {a}"
        );
        assert_eq!(a["methodology"].as_str(), Some(METHODOLOGY), "{a}");
        assert!(a["confidence"].is_string(), "{a}");
        if kind == Kind::NoCall {
            return;
        }
        let coverage = a["coverage"].as_f64().expect("action.coverage");
        assert!(
            coverage >= MIN_COVERAGE,
            "{} given at {coverage}% coverage",
            kind.label()
        );
        assert!(
            a["review_by"].is_string(),
            "action without review date: {a}"
        );
        if matches!(kind, Kind::Buy | Kind::Add | Kind::Hold | Kind::Trim) {
            assert!(
                a["invalidation_price"].as_f64().is_some_and(|v| v > 0.0),
                "{} without invalidation: {a}",
                kind.label()
            );
        }
    }

    /// Action words in output must come from this module: every `ACTION:`
    /// line is in the headline format, every JSON `action` object is well
    /// formed, and no bare upper-case call (BUY, SELL, EXIT, ...) appears
    /// outside them.
    pub fn assert_action_well_formed(text: &str) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            walk(&v);
        }
        for line in text.lines() {
            // IPO verdicts come from `crate::ipo_verdict`, in its headline format.
            if let Some(at) = line.find("IPO VERDICT: ") {
                let rest = &line[at + "IPO VERDICT: ".len()..];
                let label = rest
                    .split([' ', '·'])
                    .take_while(|w| !w.is_empty())
                    .collect::<Vec<_>>();
                let head = rest.split(" · ").next().unwrap_or_default();
                let head = head.split(" — ").next().unwrap_or_default();
                let kind = crate::ipo_verdict::Kind::parse(head).unwrap_or_else(|| {
                    panic!("IPO VERDICT line with unknown kind ({label:?}): {line}")
                });
                use crate::ipo_verdict::Kind as K;
                match kind {
                    K::NoCall | K::Listed => {
                        assert!(
                            rest.contains(" — "),
                            "{} without its reason: {line}",
                            kind.label()
                        )
                    }
                    _ => assert!(
                        rest.contains(" confidence") && rest.contains("review by"),
                        "IPO VERDICT line not in the engine's format: {line}"
                    ),
                }
                continue;
            }
            if let Some(at) = line.find("ACTION: ") {
                let rest = &line[at + "ACTION: ".len()..];
                if rest.starts_with("NO CALL") {
                    assert!(rest.contains(" — "), "NO CALL without its reason: {line}");
                    continue;
                }
                let word = rest.split(' ').next().unwrap_or_default();
                let kind = Kind::parse(word)
                    .unwrap_or_else(|| panic!("ACTION line with unknown kind: {line}"));
                assert_ne!(kind, Kind::NoCall);
                assert!(
                    rest.contains(" confidence") && rest.contains("review by"),
                    "ACTION line not in the engine's format: {line}"
                );
                if matches!(kind, Kind::Buy | Kind::Add | Kind::Hold | Kind::Trim) {
                    assert!(
                        rest.contains("invalid below "),
                        "{} ACTION line without invalidation price: {line}",
                        kind.label()
                    );
                }
                continue;
            }
            // Outside action lines: no free-standing calls. A line that names
            // two or more kinds lists the engine's vocabulary (help text,
            // docs), it does not make a call.
            let named = [
                "BUY", "WATCH", "AVOID", "ADD", "HOLD", "TRIM", "EXIT", "NO CALL",
            ]
            .iter()
            .filter(|k| line.contains(*k))
            .count();
            if named >= 2 {
                continue;
            }
            for call in [
                "BUY",
                "SELL",
                "STRONG BUY",
                "STRONG SELL",
                "EXIT NOW",
                "ACCUMULATE",
            ] {
                let mut from = 0;
                while let Some(i) = line[from..].find(call) {
                    let s = from + i;
                    let e = s + call.len();
                    // The IPO engine's label "APPLY – SELL ON LISTING".
                    if call == "SELL" && line[e..].starts_with(" ON LISTING") {
                        from = e;
                        continue;
                    }
                    let before = line[..s].chars().next_back();
                    let after = line[e..].chars().next();
                    let whole = !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                        && !after.is_some_and(|c| c.is_alphanumeric() || c == '_');
                    assert!(
                        !whole || line.contains("\"kind\""),
                        "call {call:?} outside an engine action: {line}"
                    );
                    from = e;
                }
            }
        }
    }

    fn walk(v: &serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, val) in m {
                    if k == "action" && val.is_object() && val.get("kind").is_some() {
                        check_action(val);
                    }
                    if k == "verdict" && val.is_object() && val.get("methodology").is_some() {
                        crate::ipo_verdict::testing::check_verdict(val);
                    }
                    walk(val);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(walk),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn an_etf_labelled_equity_is_not_a_stock() {
        let q = |sym: &str, name: &str| crate::api::Quote {
            symbol: Some(sym.into()),
            long_name: Some(name.into()),
            quote_type: Some("EQUITY".into()),
            ..Default::default()
        };
        assert!(fund_by_name(
            "NIFTYBEES.NS",
            &q("NIFTYBEES.NS", "Nippon India ETF Nifty 50 BeES")
        ));
        assert!(fund_by_name(
            "SETFNIF50.NS",
            &q("SETFNIF50.NS", "SBI-ETF Nifty 50")
        ));
        assert!(!fund_by_name(
            "TCS.NS",
            &q("TCS.NS", "Tata Consultancy Services Limited")
        ));
        assert!(!fund_by_name(
            "BEETEL.NS",
            &q("BEETEL.NS", "Beetel Teletech Limited")
        ));
        // Its NO CALL says why, without a list of company fields.
        let mut i = Inputs::new(
            "NIFTYBEES.NS",
            "INR",
            NaiveDate::from_ymd_opt(2026, 11, 21).unwrap(),
        );
        i.is_stock = false;
        i.missing = vec!["ROE".into(), "D/E".into()];
        let a = decide(&i);
        assert!(a.missing.is_empty());
        assert!(
            a.headline().contains("Not a listed company stock"),
            "{}",
            a.headline()
        );
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// A healthy large cap in an uptrend, cheap against its history: BUY.
    fn good() -> Inputs {
        let mut i = Inputs::new("GOOD.NS", "INR", day("2026-11-20"));
        i.max_stock_weight = 10.0;
        i.risk_per_trade = 1.0;
        i.price = Some(1000.0);
        i.coverage = 90.0;
        i.composite = Some(72.0);
        i.safety = Some(70.0);
        i.valuation = Some(66.0);
        i.pillars = vec![72.0, 70.0, 66.0, 68.0, 60.0, 75.0];
        i.book_value = Some(300.0);
        i.eps = Some(50.0);
        i.profit_margin = Some(0.15);
        i.debt_to_equity = Some(0.3);
        i.pledge_pct = Some(0.0);
        i.pe = Some(20.0);
        i.pe_own_median = Some(22.0);
        i.market_cap = Some(5.0e12);
        i.tech = Technicals {
            close: Some(1000.0),
            sma50: Some(950.0),
            sma200: Some(900.0),
            atr14: Some(20.0),
            low20: Some(960.0),
            high_close20: Some(1010.0),
            volume_ratio: Some(1.3),
            avg_traded_value: Some(5.0e9),
            circuit_hits: Some(0),
        };
        i
    }

    fn held(mut i: Inputs, weight: f64, avg_cost: f64) -> Inputs {
        i.holding = Some(HoldingContext {
            shares: 100.0,
            avg_cost,
            weight_pct: Some(weight),
            ..Default::default()
        });
        i
    }

    fn json(a: &Action) -> serde_json::Value {
        serde_json::to_value(a).unwrap()
    }

    fn check(a: &Action) {
        check_action(&json(a));
        assert_action_well_formed(&a.render(""));
        assert_action_well_formed(&serde_json::json!({ "action": a }).to_string());
    }

    #[test]
    fn healthy_uptrend_cheap_stock_is_buy() {
        let a = decide(&good());
        assert_eq!(a.kind, Kind::Buy, "{a:?}");
        check(&a);
        // max(200-day 900, 1000 - 2.5 x 20 = 950)
        assert_eq!(a.invalidation_price, Some(950.0));
        assert_eq!(a.confidence, Confidence::High);
        assert_eq!(a.review_by.as_deref(), Some("2027-02-18"));
        assert!(
            a.headline().starts_with(
                "ACTION: BUY · high confidence · invalid below ₹950.00 · review by 18 Feb 2027"
            ),
            "{}",
            a.headline()
        );
        // 1% risk / (50 / 1000) = 20% > 10% cap.
        let s = a.size_hint.unwrap();
        assert_eq!((s.max_weight_pct, s.limited_by), (10.0, "max_stock_weight"));
    }

    #[test]
    fn size_hint_follows_risk_per_trade_when_under_cap() {
        let mut i = good();
        i.risk_per_trade = 0.25; // 0.25% / 5% = 5%
        i.portfolio_value = Some(1_000_000.0);
        let s = decide(&i).size_hint.unwrap();
        assert_eq!((s.max_weight_pct, s.limited_by), (5.0, "risk_per_trade"));
        assert_eq!(s.shares, Some(50.0));
    }

    #[test]
    fn coverage_below_sixty_is_no_call_with_missing_fields() {
        let mut i = good();
        i.coverage = 59.0;
        i.missing = vec!["ROE".into(), "D/E".into()];
        let a = decide(&i);
        assert_eq!(a.kind, Kind::NoCall);
        assert_eq!(a.headline(), "ACTION: NO CALL — missing: ROE, D/E");
        assert!(a.invalidation_price.is_none() && a.review_by.is_none());
        check(&a);
        i.coverage = 60.0;
        assert_ne!(decide(&i).kind, Kind::NoCall);
    }

    #[test]
    fn non_stock_and_no_price_are_no_call() {
        let mut i = good();
        i.is_stock = false;
        assert_eq!(decide(&i).kind, Kind::NoCall);
        let mut i = good();
        i.price = None;
        i.tech.close = None;
        let a = decide(&i);
        assert_eq!(a.kind, Kind::NoCall);
        assert!(a.missing.contains(&"price".to_string()));
        let mut i = good();
        i.composite = None;
        assert_eq!(decide(&i).kind, Kind::NoCall);
    }

    #[test]
    fn every_hard_gate_gives_avoid() {
        let cases: Vec<(&str, Box<dyn Fn(&mut Inputs)>)> = vec![
            ("Altman", Box::new(|i| i.altman_distress = true)),
            (
                "Negative shareholders",
                Box::new(|i| i.book_value = Some(-5.0)),
            ),
            (
                "Loss-making",
                Box::new(|i| {
                    i.eps = Some(-2.0);
                    i.debt_to_equity = Some(1.6);
                }),
            ),
            ("pledge 30%", Box::new(|i| i.pledge_pct = Some(30.0))),
            ("Composite score 44", Box::new(|i| i.composite = Some(44.0))),
        ];
        for (what, set) in cases {
            let mut i = good();
            set(&mut i);
            let a = decide(&i);
            assert_eq!(a.kind, Kind::Avoid, "{what}: {a:?}");
            assert!(a.reasons[0].contains(what), "{what}: {:?}", a.reasons);
            assert!(a.invalidation_price.is_none());
            check(&a);
        }
    }

    #[test]
    fn gate_edges_do_not_trip() {
        let mut i = good();
        i.eps = Some(-2.0);
        i.debt_to_equity = Some(1.5); // not above 1.5
        assert_ne!(decide(&i).kind, Kind::Avoid);
        let mut i = good();
        i.debt_to_equity = Some(3.0); // profitable: leverage alone is no gate
        assert_ne!(decide(&i).kind, Kind::Avoid);
        let mut i = good();
        i.composite = Some(45.0);
        assert_eq!(decide(&i).kind, Kind::Watch);
        let mut i = good();
        i.pledge_pct = Some(25.0);
        assert_eq!(decide(&i).kind, Kind::Buy);
        let mut i = good();
        i.is_financial = true;
        i.eps = Some(-1.0);
        i.debt_to_equity = Some(9.0);
        assert_ne!(decide(&i).kind, Kind::Avoid, "banks carry debt by design");
    }

    #[test]
    fn each_unmet_buy_condition_gives_watch() {
        let cases: Vec<(&str, Box<dyn Fn(&mut Inputs)>)> = vec![
            ("below 65", Box::new(|i| i.composite = Some(64.0))),
            ("Safety pillar is Weak", Box::new(|i| i.safety = Some(40.0))),
            (
                "below its 200-day",
                Box::new(|i| {
                    i.tech.sma200 = Some(1050.0);
                }),
            ),
            ("more than 1.2x", Box::new(|i| i.pe = Some(27.0))),
            (
                "No P/E history",
                Box::new(|i| {
                    i.pe_own_median = None;
                    i.pe_sector = None;
                }),
            ),
            ("No positive P/E", Box::new(|i| i.pe = None)),
        ];
        for (what, set) in cases {
            let mut i = good();
            set(&mut i);
            let a = decide(&i);
            assert_eq!(a.kind, Kind::Watch, "{what}: {a:?}");
            assert!(
                a.reasons.iter().any(|r| r.contains(what)),
                "{what}: {:?}",
                a.reasons
            );
            assert!(a.invalidation_price.is_none());
            check(&a);
        }
    }

    #[test]
    fn sector_pe_counts_when_own_history_is_missing() {
        let mut i = good();
        i.pe_own_median = None;
        i.pe_sector = Some(17.0); // 20 <= 1.2 x 17 = 20.4
        assert_eq!(decide(&i).kind, Kind::Buy);
        i.pe_sector = Some(16.0); // 19.2
        assert_eq!(decide(&i).kind, Kind::Watch);
    }

    #[test]
    fn market_median_is_the_last_resort_reference() {
        let mut i = good();
        i.pe_own_median = None;
        i.pe_market = Some(18.0); // 20 <= 21.6
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Buy);
        assert!(
            a.reasons.iter().any(|r| r.contains("market median")),
            "{:?}",
            a.reasons
        );
        i.pe_sector = Some(15.0); // the sector figure wins over the market: 20 > 18
        assert_eq!(decide(&i).kind, Kind::Watch);
    }

    #[test]
    fn earnings_within_five_days_turns_buy_into_watch() {
        let mut i = good();
        i.next_earnings = Some(day("2026-11-25"));
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Watch);
        assert!(a.reasons[0].contains("Results due in 5 days"));
        i.next_earnings = Some(day("2026-11-26"));
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Buy);
        assert_eq!(a.review_by.as_deref(), Some("2026-11-26"));
    }

    #[test]
    fn short_horizon_needs_50_day_and_volume() {
        let mut i = good();
        i.horizon = Horizon::Short;
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Buy, "{a:?}");
        // max(1000 - 1.5 x 20 = 970, 20-day low 960)
        assert_eq!(a.invalidation_price, Some(970.0));
        // 10 sessions from Friday 20 Nov 2026.
        assert_eq!(a.review_by.as_deref(), Some("2026-12-04"));
        check(&a);

        let mut j = i.clone();
        j.tech.volume_ratio = Some(1.1);
        let a = decide(&j);
        assert_eq!(a.kind, Kind::Watch);
        assert!(
            a.reasons
                .iter()
                .any(|r| r.contains("Volume not picking up"))
        );

        let mut j = i.clone();
        j.tech.sma50 = Some(1001.0);
        let a = decide(&j);
        assert_eq!(a.kind, Kind::Watch);
        assert!(a.reasons.iter().any(|r| r.contains("50-day")));

        let mut j = i;
        j.tech.low20 = Some(990.0);
        assert_eq!(invalidation(&j), Some(990.0));
    }

    #[test]
    fn small_cap_gates_avoid() {
        let small = || {
            let mut i = good();
            i.market_cap = Some(4.0e10); // ₹4,000 crore
            i.adv_floor = Some(1.0e7);
            i
        };
        assert!(small().is_small_cap());
        assert_eq!(decide(&small()).kind, Kind::Buy);
        let cases: Vec<(&str, Box<dyn Fn(&mut Inputs)>)> = vec![
            ("pledge 12%", Box::new(|i| i.pledge_pct = Some(12.0))),
            (
                "below the",
                Box::new(|i| i.tech.avg_traded_value = Some(5.0e6)),
            ),
            (
                "price band 3 times",
                Box::new(|i| i.tech.circuit_hits = Some(3)),
            ),
            (
                "auditor",
                Box::new(|i| i.auditor_flag = Some("Resignation of Statutory Auditor".into())),
            ),
            (
                "surveillance",
                Box::new(|i| i.surveillance = Some("NSE surveillance: ASM (Stage 1)".into())),
            ),
        ];
        for (what, set) in cases {
            let mut i = small();
            set(&mut i);
            let a = decide(&i);
            assert_eq!(a.kind, Kind::Avoid, "{what}");
            assert!(a.reasons[0].contains(what), "{what}: {:?}", a.reasons);
            // The same facts on a large cap are not gates.
            let mut big = good();
            set(&mut big);
            assert_eq!(decide(&big).kind, Kind::Buy, "{what} on a large cap");
        }
        // Two band hits are allowed.
        let mut i = small();
        i.tech.circuit_hits = Some(2);
        assert_eq!(decide(&i).kind, Kind::Buy);
    }

    #[test]
    fn small_cap_size_is_half_the_cap() {
        let mut i = good();
        i.market_cap = Some(4.0e10);
        let s = decide(&i).size_hint.unwrap();
        assert_eq!(
            (s.max_weight_pct, s.limited_by),
            (5.0, "small_cap_half_cap")
        );
        // US: $1.5B is small, $3B is not.
        let mut us = good();
        us.currency = "USD".into();
        us.market_cap = Some(1.5e9);
        assert!(us.is_small_cap());
        us.market_cap = Some(3.0e9);
        assert!(!us.is_small_cap());
    }

    #[test]
    fn holding_paths_exit_trim_add_hold() {
        // ADD: strong, trend up, room under the cap.
        let a = decide(&held(good(), 4.0, 900.0));
        assert_eq!(a.kind, Kind::Add, "{a:?}");
        check(&a);
        // Trailing invalidation from the 20-day high close: max(900, 1010 - 50).
        assert_eq!(a.invalidation_price, Some(960.0));

        // HOLD: composite in between.
        let mut i = held(good(), 4.0, 900.0);
        i.composite = Some(55.0);
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Hold);
        check(&a);

        // HOLD, not ADD, at the cap.
        let mut i = held(good(), 10.0, 900.0);
        i.holding.as_mut().unwrap().weight_pct = Some(9.9);
        i.holding.as_mut().unwrap().target_weight_pct = Some(8.0);
        assert_eq!(decide(&i).kind, Kind::Hold);

        // ADD becomes HOLD before results.
        let mut i = held(good(), 4.0, 900.0);
        i.next_earnings = Some(day("2026-11-22"));
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Hold);
        assert!(a.reasons[0].contains("Results due"));
    }

    #[test]
    fn holding_exits() {
        // Close below the invalidation level.
        let mut i = held(good(), 4.0, 900.0);
        i.tech.close = Some(890.0);
        i.price = Some(890.0);
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Exit);
        assert!(
            a.reasons[0].contains("below the invalidation level"),
            "{:?}",
            a.reasons
        );
        check(&a);

        // A hard gate appears.
        let mut i = held(good(), 4.0, 900.0);
        i.altman_distress = true;
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Exit);
        assert!(a.tax_note.is_none());

        // Thesis limit.
        let mut i = held(good(), 4.0, 900.0);
        i.holding.as_mut().unwrap().thesis_breaches =
            vec!["P/E 20.0 above your limit of 18.0".into()];
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Exit);
        assert!(a.reasons[0].contains("thesis"));

        // Composite below 40 (a holding at 44 is not an exit).
        let mut i = held(good(), 4.0, 900.0);
        i.composite = Some(39.0);
        assert_eq!(decide(&i).kind, Kind::Exit);
        i.composite = Some(44.0);
        assert_eq!(decide(&i).kind, Kind::Hold);
    }

    #[test]
    fn trim_over_cap_computes_shares() {
        let mut i = held(good(), 15.0, 900.0);
        i.portfolio_value = Some(666_667.0);
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Trim);
        // 100 shares at 15%, cap 10%: sell 34 (ceil of 33.3).
        let s = a.size_hint.clone().unwrap();
        assert_eq!(s.shares, Some(34.0));
        assert_eq!(s.max_weight_pct, 10.0);
        assert!(a.render("").contains("sell about 34 shares"));
        check(&a);
    }

    #[test]
    fn trim_on_stretched_valuation_with_big_gain() {
        let mut i = held(good(), 4.0, 600.0); // +67%
        i.pe = Some(40.0); // > 1.5 x 22
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Trim, "{a:?}");
        assert!(a.reasons[0].contains("67% gain"));
        // Same valuation with a smaller gain: no trim.
        let mut i = held(good(), 4.0, 800.0);
        i.pe = Some(40.0);
        assert_ne!(decide(&i).kind, Kind::Trim);
    }

    #[test]
    fn trim_on_lookthrough_weight() {
        let mut i = held(good(), 8.0, 900.0);
        i.holding.as_mut().unwrap().lookthrough_weight_pct = Some(12.5);
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Trim);
        assert!(a.reasons[0].contains("funds hold"));
        // 12.5% combined vs 10% cap => 2.5% reduction out of 8% direct => ceil(100 * 2.5 / 8.0) = 32 shares.
        let s = a.size_hint.as_ref().unwrap();
        assert_eq!(s.shares, Some(32.0));
        assert_eq!(s.limited_by, "max_stock_weight");

        // When indirect exposure alone is >= cap (2% direct, 12.5% combined => 10.5% indirect),
        // selling all direct shares is capped by direct_holding_only.
        i.holding.as_mut().unwrap().weight_pct = Some(2.0);
        let a2 = decide(&i);
        let s2 = a2.size_hint.as_ref().unwrap();
        assert_eq!(s2.shares, Some(100.0));
        assert_eq!(s2.limited_by, "direct_holding_only");
        assert!(a2.render("").contains("sell all 100 shares directly held"));
    }

    #[test]
    fn add_counts_lookthrough_weight_against_the_cap() {
        // 4% direct, 9% with what funds hold, 10% cap: 1 point of room left.
        let mut i = held(good(), 4.0, 900.0);
        i.portfolio_value = Some(2_500_000.0);
        i.holding.as_mut().unwrap().lookthrough_weight_pct = Some(9.0);
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Add, "{a:?}");
        assert!(
            a.reasons
                .iter()
                .any(|r| r.contains("9.0% with what your funds hold"))
        );
        // The direct holding may grow to 10% - 5% indirect = 5%: 125 shares
        // at 1,000 in a 2,500,000 portfolio, less the 100 held.
        let s = a.size_hint.clone().unwrap();
        assert_eq!((s.max_weight_pct, s.limited_by), (5.0, "max_stock_weight"));
        assert_eq!(s.shares, Some(25.0));
        check(&a);

        // Combined exposure at the cap: no room, so no add.
        i.holding.as_mut().unwrap().lookthrough_weight_pct = Some(10.0);
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Hold, "{a:?}");
        assert!(a.reasons.iter().any(|r| r.contains("No room left")));

        // Without look-through data the direct weight decides, as before.
        i.holding.as_mut().unwrap().lookthrough_weight_pct = None;
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Add);
        assert!(a.size_hint.unwrap().max_weight_pct > 5.0);
    }

    #[test]
    fn add_size_hint_caps_at_portfolio_target_and_missing_level_downgrades() {
        let mut i = held(good(), 4.0, 900.0);
        i.portfolio_value = Some(2_500_000.0);
        i.holding.as_mut().unwrap().target_weight_pct = Some(5.0);
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Add);
        let s = a.size_hint.unwrap();
        assert_eq!((s.max_weight_pct, s.limited_by), (5.0, "target_weight"));
        // 5% of 2,500,000 at ₹1,000 = 125 shares total, minus 100 held = 25 shares.
        assert_eq!(s.shares, Some(25.0));

        // Held stock with no sma200 and no atr14 cannot set an invalidation price -> NO CALL.
        let mut no_inv = held(good(), 4.0, 900.0);
        no_inv.tech.sma200 = None;
        no_inv.tech.atr14 = None;
        let a_held = decide(&no_inv);
        assert_eq!(a_held.kind, Kind::NoCall);

        // Short-horizon unheld BUY candidate without atr14 or low20 -> WATCH.
        let mut short_unheld = good();
        short_unheld.horizon = Horizon::Short;
        short_unheld.tech.atr14 = None;
        short_unheld.tech.low20 = None;
        let a_unheld = decide(&short_unheld);
        assert_eq!(a_unheld.kind, Kind::Watch);
    }

    #[test]
    fn tax_note_when_long_term_is_near() {
        let mut i = held(good(), 15.0, 900.0);
        i.holding.as_mut().unwrap().next_long_term = Some((12, day("2026-12-02"), Some(1875.0)));
        let a = decide(&i);
        assert_eq!(a.kind, Kind::Trim);
        assert_eq!(
            a.tax_note.as_deref(),
            Some(
                "a lot turns long-term on 02 Dec 2026 — waiting 12 days saves about ₹1,875.00 in tax"
            )
        );
        // Beyond 30 days: no note.
        i.holding.as_mut().unwrap().next_long_term = Some((31, day("2026-12-21"), Some(1875.0)));
        assert!(decide(&i).tax_note.is_none());
        // No tax note on a hard-gate exit.
        let mut i = held(good(), 4.0, 900.0);
        i.book_value = Some(-1.0);
        i.holding.as_mut().unwrap().next_long_term = Some((5, day("2026-11-25"), Some(100.0)));
        assert!(decide(&i).tax_note.is_none());
    }

    #[test]
    fn confidence_levels() {
        assert_eq!(decide(&good()).confidence, Confidence::High);
        let mut i = good();
        i.coverage = 75.0;
        assert_eq!(decide(&i).confidence, Confidence::Medium);
        i.coverage = 65.0;
        assert_eq!(decide(&i).confidence, Confidence::Low);
        let mut i = good();
        i.pillars = vec![80.0, 70.0, 30.0, 40.0];
        assert_eq!(decide(&i).confidence, Confidence::Low, "mixed pillars");
        let mut i = good();
        i.pillars = vec![80.0, 70.0, 50.0, 40.0];
        assert_eq!(decide(&i).confidence, Confidence::Medium, "one weak pillar");
    }

    #[test]
    fn decide_is_deterministic() {
        let i = good();
        assert_eq!(decide(&i), decide(&i));
    }

    #[test]
    fn technicals_from_bars() {
        let n = 260;
        let close: Vec<f64> = (0..n).map(|k| 100.0 + k as f64 * 0.5).collect();
        let high: Vec<f64> = close.iter().map(|c| c + 1.0).collect();
        let low: Vec<f64> = close.iter().map(|c| c - 1.0).collect();
        let mut vol = vec![1000.0; n];
        for v in vol.iter_mut().skip(n - 20) {
            *v = 2000.0;
        }
        let t = Technicals::from_bars(&close, &high, &low, &vol);
        assert_eq!(t.close, Some(229.5));
        assert!((t.sma200.unwrap() - (100.0 + 0.5 * 159.5)).abs() < 1e-9);
        assert!(t.atr14.unwrap() > 1.9 && t.atr14.unwrap() < 2.1);
        assert_eq!(t.low20, Some(close[n - 21] - 1.0));
        assert_eq!(t.high_close20, Some(229.5));
        // 20-day 2000 vs 60-day (40 x 1000 + 20 x 2000) / 60.
        assert!((t.volume_ratio.unwrap() - 2000.0 / (80000.0 / 60.0)).abs() < 1e-9);
        assert_eq!(t.circuit_hits, Some(0));

        // Three 5% up-moves that close at the high.
        let mut c: Vec<f64> = vec![100.0; 40];
        let mut h: Vec<f64> = vec![101.0; 40];
        let l = vec![99.0; 40];
        for k in [30, 33, 36] {
            let up = c[k - 1] * 1.05;
            h[k] = up;
            for x in c.iter_mut().skip(k) {
                *x = up;
            }
        }
        for k in 0..40 {
            h[k] = h[k].max(c[k]);
        }
        let t = Technicals::from_bars(&c, &h, &l, &[1.0; 40]);
        assert_eq!(t.circuit_hits, Some(3));

        // 4.96% moves do not count as 5% circuit hits.
        let mut c496: Vec<f64> = vec![100.0; 40];
        let mut h496: Vec<f64> = vec![101.0; 40];
        for k in [30, 33, 36] {
            let up = c496[k - 1] * 1.0496;
            h496[k] = up;
            for x in c496.iter_mut().skip(k) {
                *x = up;
            }
        }
        for k in 0..40 {
            h496[k] = h496[k].max(c496[k]);
        }
        let t496 = Technicals::from_bars(&c496, &h496, &l, &[1.0; 40]);
        assert_eq!(t496.circuit_hits, Some(0));
    }

    #[test]
    fn auditor_and_surveillance_detection() {
        assert!(auditor_flag(["Resignation of Statutory Auditors"]).is_some());
        assert!(auditor_flag(["Audit report with qualified opinion"]).is_some());
        assert!(auditor_flag(["Adverse opinion on standalone financial results"]).is_some());
        assert!(auditor_flag(["Independent auditor's report with unqualified opinion"]).is_none());
        assert!(auditor_flag(["Unmodified opinion on financial results"]).is_none());
        assert!(auditor_flag(["Board meeting outcome", "Unqualified opinion"]).is_none());
        let w = ["NSE surveillance: ASM (Long Term ASM Stage 1)".to_string()];
        assert!(surveillance(w.iter()).is_some());
        assert!(surveillance(["Yahoo failed".to_string()].iter()).is_none());
    }

    #[test]
    fn thesis_breach_lines() {
        let n = crate::notes::Note {
            max_pe: Some(18.0),
            min_roe_pct: Some(15.0),
            max_weight_pct: Some(8.0),
            ..Default::default()
        };
        let b = thesis_breaches(Some(&n), Some(20.0), Some(0.12), Some(9.0));
        assert_eq!(b.len(), 3, "{b:?}");
        assert!(thesis_breaches(Some(&n), Some(15.0), Some(0.2), Some(5.0)).is_empty());
        assert!(thesis_breaches(None, Some(99.0), None, None).is_empty());
    }

    #[test]
    fn well_formed_guard_catches_bad_actions() {
        for bad in [
            "STRONG BUY this now",
            "ACTION: BUY now",
            "ACTION: BUY · high confidence · review by 1 Jan 2027",
            "ACTION: HOLD · medium confidence · review by 1 Jan 2027",
            "ACTION: MOON · high confidence · review by 1 Jan 2027",
            "SELL",
        ] {
            let caught = std::panic::catch_unwind(|| assert_action_well_formed(bad)).is_err();
            assert!(caught, "{bad:?} should be flagged");
        }
        let missing_reasons = serde_json::json!({"action": {"kind": "BUY", "reasons": [], "disclaimer": DISCLAIMER, "methodology": METHODOLOGY, "confidence": "high", "coverage": 90.0, "review_by": "2027-01-01", "invalidation_price": 1.0}});
        assert!(
            std::panic::catch_unwind(|| assert_action_well_formed(&missing_reasons.to_string()))
                .is_err()
        );
        let low_coverage = serde_json::json!({"action": {"kind": "BUY", "reasons": ["x"], "disclaimer": DISCLAIMER, "methodology": METHODOLOGY, "confidence": "low", "coverage": 50.0, "review_by": "2027-01-01", "invalidation_price": 1.0}});
        assert!(
            std::panic::catch_unwind(|| assert_action_well_formed(&low_coverage.to_string()))
                .is_err()
        );
        for k in ["BUY", "ADD", "HOLD", "TRIM"] {
            let no_level = serde_json::json!({"action": {"kind": k, "reasons": ["x"], "disclaimer": DISCLAIMER, "methodology": METHODOLOGY, "confidence": "low", "coverage": 90.0, "review_by": "2027-01-01"}});
            assert!(
                std::panic::catch_unwind(|| assert_action_well_formed(&no_level.to_string()))
                    .is_err(),
                "{k} without invalidation_price must be rejected"
            );
        }
        // Plain descriptions of flows and the sell preview are fine.
        assert_action_well_formed(
            "Recent insider open-market buying; FII selling; `portfolio sell` preview",
        );
    }

    #[test]
    fn gbp_pence_prices_do_not_misclassify_large_caps_or_inflate_own_pe() {
        let mut i = Inputs::new("AZN.L", "GBp", day("2026-11-21"));
        i.market_cap = Some(27_100_000_000.0); // £27.1B (~$34.4B), large cap
        assert!(!i.is_small_cap());

        let mut r = crate::research::Research::default();
        r.currency = Some("GBp".into());
        for k in 0..7 {
            let d = day("2025-03-31") + chrono::Duration::days(k * 90);
            r.prices.dates.push(d);
            r.prices.close.push(117.0); // 117p = £1.17
            r.quarters.push(crate::research::Period {
                end: d,
                eps: Some(0.025), // 4Q trailing EPS = £0.10
                ..Default::default()
            });
        }
        let pe = own_median_pe(&r).unwrap();
        assert!((pe - 11.7).abs() < 1e-6, "expected 11.7, got {pe}");

        // Quarter ends after the final price bar are excluded rather than reusing the last close.
        r.prices.dates.truncate(5);
        r.prices.close.truncate(5);
        assert!(own_median_pe(&r).is_none());
    }
}
