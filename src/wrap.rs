//! `portfolio wrap`: a one-screen review of a calendar or financial year.
//!
//! Plain inputs only (the portfolio file, prices by date, the benchmark's
//! closes), so the period rule, the return maths and the redaction are
//! testable. `commands::checkup` fetches the prices and prints the card.
//!
//! The period return is a Modified Dietz return: the gain over the period
//! (end value less start value less net money added) over the start value
//! plus the money added, each flow weighted by the share of the period it
//! was invested. It is a money-weighted figure for the period, not
//! annualised. Amounts in other currencies are converted at today's rate.

use crate::portfolio::Portfolio;
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

/// Which kind of year a period is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodKind {
    Calendar,
    IndianFy,
}

/// The year under review. `end` may be after today: the card then covers
/// the year so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Period {
    pub kind: PeriodKind,
    pub label: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
}

fn ymd(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(y, m, d)
}

impl Period {
    pub fn calendar(year: i32) -> Option<Self> {
        Some(Self {
            kind: PeriodKind::Calendar,
            label: year.to_string(),
            start: ymd(year, 1, 1)?,
            end: ymd(year, 12, 31)?,
        })
    }

    /// April `start_year` to March of the next year.
    pub fn indian_fy(start_year: i32) -> Option<Self> {
        Some(Self {
            kind: PeriodKind::IndianFy,
            label: format!("FY {}-{:02}", start_year, (start_year + 1).rem_euclid(100)),
            start: ymd(start_year, 4, 1)?,
            end: ymd(start_year + 1, 3, 31)?,
        })
    }

    /// `--year 2026`.
    pub fn parse_year(s: &str) -> Result<Self, String> {
        let y: i32 = s
            .trim()
            .parse()
            .map_err(|_| format!("`{s}` is not a year: use four digits, e.g. 2026"))?;
        if !(1990..=2100).contains(&y) {
            return Err(format!("{y} is out of range (1990 to 2100)"));
        }
        Self::calendar(y).ok_or_else(|| format!("{y} is not a valid year"))
    }

    /// `--fy 2026-27` (also `2026-2027` or `FY2026-27`).
    pub fn parse_fy(s: &str) -> Result<Self, String> {
        let t = s
            .trim()
            .trim_start_matches(['F', 'f'])
            .trim_start_matches(['Y', 'y']);
        let t = t.trim();
        let bad = || format!("`{s}` is not a financial year: use 2026-27");
        let (a, b) = t.split_once('-').ok_or_else(bad)?;
        let start: i32 = a.trim().parse().map_err(|_| bad())?;
        if a.trim().len() != 4 || !(1990..=2100).contains(&start) {
            return Err(bad());
        }
        let b = b.trim();
        let next_ok = match b.len() {
            2 => b.parse::<i32>().ok() == Some((start + 1) % 100),
            4 => b.parse::<i32>().ok() == Some(start + 1),
            _ => false,
        };
        if !next_ok {
            return Err(format!(
                "`{s}`: the second year must follow the first, e.g. 2026-27"
            ));
        }
        Self::indian_fy(start).ok_or_else(bad)
    }

    /// The default: the Indian financial year that contains `today` when
    /// the portfolio totals in rupees, else the calendar year of `today`.
    pub fn default_for(base_currency: &str, today: NaiveDate) -> Self {
        if base_currency.eq_ignore_ascii_case("INR") {
            let y = if today.month() >= 4 {
                today.year()
            } else {
                today.year() - 1
            };
            Self::indian_fy(y).expect("valid year")
        } else {
            Self::calendar(today.year()).expect("valid year")
        }
    }

    /// The last day covered: the period's end, or today when earlier.
    pub fn as_of(&self, today: NaiveDate) -> NaiveDate {
        self.end.min(today)
    }

    /// The day whose close values the start: the day before the period.
    pub fn opening(&self) -> NaiveDate {
        self.start.pred_opt().expect("valid date")
    }
}

/// The close on `date` or the last one before it (closes oldest first).
pub fn close_on(closes: &[(NaiveDate, f64)], date: NaiveDate) -> Option<f64> {
    let i = closes.partition_point(|(d, _)| *d <= date);
    (i > 0).then(|| closes[i - 1].1)
}

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// One symbol over the period, in its own currency. Values are `None`
/// when shares were held but no price was known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SymbolYear {
    pub symbol: String,
    pub start_value: Option<f64>,
    pub end_value: Option<f64>,
    pub bought: f64,
    pub sold: f64,
    pub dividends: f64,
    pub realized_gain: f64,
    /// Dated flows into (+) and out of (−) the holding, for the period return.
    pub flows: Vec<(NaiveDate, f64)>,
    /// No exchange rate into the base currency: every amount is still in
    /// the symbol's own currency and stays out of the totals.
    pub unconverted: bool,
}

impl SymbolYear {
    /// End value less start value less net money put in.
    pub fn contribution(&self) -> Option<f64> {
        Some(self.end_value? - self.start_value? - self.bought + self.sold + self.dividends)
    }

    /// Contribution over the money at work (start value plus purchases).
    pub fn return_pct(&self) -> Option<f64> {
        let base = self.start_value? + self.bought;
        (base > 0.0).then(|| self.contribution().unwrap_or(0.0) / base * 100.0)
    }
}

/// Shares of `symbol` held at the end of `day`: open lots bought by then
/// and sold lots bought by then but sold after.
pub fn shares_at(p: &Portfolio, symbol: &str, day: NaiveDate) -> f64 {
    let open: f64 = p
        .holding(symbol)
        .map(|h| {
            h.lots
                .iter()
                .filter(|l| date(&l.date).is_some_and(|d| d <= day))
                .map(|l| l.qty)
                .sum()
        })
        .unwrap_or(0.0);
    let sold: f64 = p
        .realized
        .iter()
        .filter(|s| s.symbol.eq_ignore_ascii_case(symbol))
        .filter(|s| date(&s.buy_date).is_some_and(|d| d <= day))
        .filter(|s| date(&s.sell_date).is_some_and(|d| d > day))
        .map(|s| s.qty)
        .sum();
    open + sold
}

/// `symbol` over `period` up to `as_of`, valued at `start_price` (the close
/// before the period) and `end_price` (the close on `as_of`).
pub fn symbol_year(
    p: &Portfolio,
    symbol: &str,
    period: &Period,
    as_of: NaiveDate,
    start_price: Option<f64>,
    end_price: Option<f64>,
) -> SymbolYear {
    let inside = |d: Option<NaiveDate>| d.is_some_and(|d| d >= period.start && d <= as_of);
    let value = |shares: f64, price: Option<f64>| {
        if shares.abs() < 1e-9 {
            Some(0.0)
        } else {
            price.map(|px| shares * px)
        }
    };
    let mut y = SymbolYear {
        symbol: symbol.to_string(),
        start_value: value(shares_at(p, symbol, period.opening()), start_price),
        end_value: value(shares_at(p, symbol, as_of), end_price),
        ..Default::default()
    };
    if let Some(h) = p.holding(symbol) {
        for l in h.lots.iter().filter(|l| inside(date(&l.date))) {
            y.bought += l.cost();
            y.flows.push((date(&l.date).expect("checked"), l.cost()));
        }
    }
    for s in p
        .realized
        .iter()
        .filter(|s| s.symbol.eq_ignore_ascii_case(symbol))
    {
        if inside(date(&s.buy_date)) {
            let cost = s.qty * s.buy_price;
            y.bought += cost;
            y.flows.push((date(&s.buy_date).expect("checked"), cost));
        }
        if inside(date(&s.sell_date)) {
            let proceeds = s.qty * s.sell_price;
            y.sold += proceeds;
            y.realized_gain += s.gain();
            y.flows
                .push((date(&s.sell_date).expect("checked"), -proceeds));
        }
    }
    for d in p
        .dividends
        .iter()
        .filter(|d| d.symbol.eq_ignore_ascii_case(symbol) && inside(date(&d.ex_date)))
    {
        y.dividends += d.amount;
        y.flows
            .push((date(&d.ex_date).expect("checked"), -d.amount));
    }
    y
}

/// Modified Dietz return (percent) from `start` (the close before the
/// period) to `end`: flows are money put in (+) or taken out (−).
pub fn modified_dietz(
    start_value: f64,
    end_value: f64,
    flows: &[(NaiveDate, f64)],
    opening: NaiveDate,
    as_of: NaiveDate,
) -> Option<f64> {
    let days = (as_of - opening).num_days() as f64;
    if days <= 0.0 {
        return None;
    }
    let net: f64 = flows.iter().map(|f| f.1).sum();
    let weighted: f64 = flows
        .iter()
        .map(|(d, a)| a * ((as_of - *d).num_days() as f64 / days).clamp(0.0, 1.0))
        .sum();
    let base = start_value + weighted;
    (base > 0.0).then(|| (end_value - start_value - net) / base * 100.0)
}

/// A winner or laggard.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mover {
    pub symbol: String,
    /// Gain or loss over the period, base currency.
    pub contribution: Option<f64>,
    pub return_pct: Option<f64>,
    /// Share of the period's total gross gains and losses, percent (signed).
    pub share_pct: Option<f64>,
}

/// A slice of the portfolio by cap tier or sector.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slice {
    pub name: String,
    pub weight_pct: f64,
}

/// The tax-year footer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaxFooter {
    pub year: String,
    pub days_left: i64,
    pub currency: String,
    pub exemption_limit: Option<f64>,
    pub exemption_remaining: Option<f64>,
    /// Share of the yearly exemption already used, percent.
    pub exemption_used_pct: Option<f64>,
    /// India: the next advance-tax date and the cumulative share due by it.
    pub next_advance_tax: Option<(NaiveDate, f64)>,
}

/// The year-in-review card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WrapReport {
    pub period: Period,
    pub as_of: NaiveDate,
    pub base_currency: String,
    pub redacted: bool,
    pub return_pct: Option<f64>,
    pub benchmark: String,
    pub benchmark_return_pct: Option<f64>,
    /// Return less the benchmark's, basis points.
    pub alpha_bps: Option<f64>,
    pub start_value: Option<f64>,
    pub end_value: Option<f64>,
    pub capital_deployed: Option<f64>,
    pub withdrawn: Option<f64>,
    pub dividends: Option<f64>,
    pub realized_gain: Option<f64>,
    /// The cash-flow figures as a share of the money at work (start value
    /// plus purchases), percent; these stay in a redacted card.
    pub deployed_pct: Option<f64>,
    pub dividends_pct: Option<f64>,
    pub realized_gain_pct: Option<f64>,
    pub winners: Vec<Mover>,
    pub laggards: Vec<Mover>,
    pub cap_tiers: Vec<Slice>,
    pub top_sectors: Vec<Slice>,
    pub holdings: usize,
    pub effective_bets: f64,
    /// Guardrails clean out of those evaluated.
    pub checkup_clean: Option<(usize, usize)>,
    pub tax: Option<TaxFooter>,
    /// Symbols held in the period without a price, left out of the return.
    pub unpriced: Vec<String>,
    /// Symbols without an exchange rate, left out of every total.
    pub unconverted: Vec<String>,
    pub disclaimer: &'static str,
}

/// Said under every card.
pub const DISCLAIMER: &str = "A record of past figures from your recorded holdings, not a \
     recommendation. Returns are estimates from closing prices.";

/// The parts of the card that need a price book: per-symbol years in base
/// currency (`fx` converts a symbol's currency to the base).
pub struct Totals {
    pub return_pct: Option<f64>,
    pub start_value: f64,
    pub end_value: f64,
    pub bought: f64,
    pub sold: f64,
    pub dividends: f64,
    pub realized_gain: f64,
    pub winners: Vec<Mover>,
    pub laggards: Vec<Mover>,
    pub unpriced: Vec<String>,
    pub unconverted: Vec<String>,
}

/// Sum symbol years (each already in the base currency) into the card's
/// totals, return, winners and laggards (`top` each).
pub fn totals(years: &[SymbolYear], period: &Period, as_of: NaiveDate, top: usize) -> Totals {
    let mut t = Totals {
        return_pct: None,
        start_value: 0.0,
        end_value: 0.0,
        bought: 0.0,
        sold: 0.0,
        dividends: 0.0,
        realized_gain: 0.0,
        winners: Vec::new(),
        laggards: Vec::new(),
        unpriced: Vec::new(),
        unconverted: Vec::new(),
    };
    let mut flows = Vec::new();
    let mut movers = Vec::new();
    for y in years {
        if y.unconverted {
            t.unconverted.push(y.symbol.clone());
            continue;
        }
        t.bought += y.bought;
        t.sold += y.sold;
        t.dividends += y.dividends;
        t.realized_gain += y.realized_gain;
        match (y.start_value, y.end_value) {
            (Some(s), Some(e)) => {
                t.start_value += s;
                t.end_value += e;
                flows.extend(y.flows.iter().copied());
                if let Some(c) = y.contribution().filter(|c| c.abs() > 1e-9) {
                    movers.push((y, c));
                }
            }
            _ => t.unpriced.push(y.symbol.clone()),
        }
    }
    t.return_pct = modified_dietz(t.start_value, t.end_value, &flows, period.opening(), as_of);
    let gross: f64 = movers.iter().map(|(_, c)| c.abs()).sum();
    let mover = |(y, c): &(&SymbolYear, f64)| Mover {
        symbol: y.symbol.clone(),
        contribution: Some(*c),
        return_pct: y.return_pct(),
        share_pct: (gross > 0.0).then(|| c / gross * 100.0),
    };
    movers.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0.symbol.cmp(&b.0.symbol))
    });
    t.winners = movers
        .iter()
        .filter(|m| m.1 > 0.0)
        .take(top)
        .map(mover)
        .collect();
    t.laggards = movers
        .iter()
        .rev()
        .filter(|m| m.1 < 0.0)
        .take(top)
        .map(mover)
        .collect();
    t
}

/// Benchmark return over the period (percent), from its closes.
pub fn benchmark_return(
    closes: &[(NaiveDate, f64)],
    period: &Period,
    as_of: NaiveDate,
) -> Option<f64> {
    let (a, b) = (
        close_on(closes, period.opening())?,
        close_on(closes, as_of)?,
    );
    (a > 0.0).then(|| (b / a - 1.0) * 100.0)
}

/// The next advance-tax date on or after `today` and the cumulative share
/// due by then.
pub fn next_advance_tax(today: NaiveDate) -> Option<(NaiveDate, f64)> {
    let fy = if today.month() >= 4 {
        today.year()
    } else {
        today.year() - 1
    };
    crate::tax::advance_tax_cutoffs(fy)
        .into_iter()
        .find(|(due, ..)| *due >= today)
        .map(|(due, _, _, pct)| (due, pct))
}

/// The same card with every absolute amount removed: returns, shares,
/// ratios and weights stay.
pub fn redact(r: &WrapReport) -> WrapReport {
    let mut out = r.clone();
    out.redacted = true;
    out.start_value = None;
    out.end_value = None;
    out.capital_deployed = None;
    out.withdrawn = None;
    out.dividends = None;
    out.realized_gain = None;
    for m in out.winners.iter_mut().chain(out.laggards.iter_mut()) {
        m.contribution = None;
    }
    if let Some(t) = &mut out.tax {
        t.exemption_limit = None;
        t.exemption_remaining = None;
    }
    out
}

fn money(v: f64, currency: &str) -> String {
    let c = Some(currency);
    let body = crate::display::group_number(&format!("{:.0}", v.abs()), crate::market::is_inr(c));
    let sign = if v < 0.0 { "-" } else { "" };
    format!("{sign}{}{}", crate::market::currency_symbol(c), body)
}

fn signed_money(v: f64, currency: &str) -> String {
    if v > 0.0 {
        format!("+{}", money(v, currency))
    } else {
        money(v, currency)
    }
}

fn pct(v: Option<f64>) -> String {
    v.map_or("–".into(), |v| format!("{v:+.1}%"))
}

/// The card as plain text lines. A redacted card has no amounts in it.
pub fn render(r: &WrapReport) -> Vec<String> {
    let cur = r.base_currency.as_str();
    let amount = |v: Option<f64>, share: Option<f64>, what: &str| match (r.redacted, v) {
        (false, Some(v)) => match share {
            Some(s) => format!("{} ({s:.1}% {what})", money(v, cur)),
            None => money(v, cur),
        },
        _ => share.map_or("–".into(), |s| format!("{s:.1}% {what}")),
    };
    let mut l = Vec::new();
    let through = if r.as_of < r.period.end {
        format!(" (to {})", r.as_of.format("%d %b %Y"))
    } else {
        String::new()
    };
    l.push(format!("Portfolio wrap — {}{}", r.period.label, through));
    l.push(String::new());
    l.push(format!(
        "Return          {}    {} {}    alpha {}",
        pct(r.return_pct),
        r.benchmark,
        pct(r.benchmark_return_pct),
        r.alpha_bps.map_or("–".into(), |a| format!("{a:+.0} bps"))
    ));
    if !r.redacted
        && let (Some(s), Some(e)) = (r.start_value, r.end_value)
    {
        l.push(format!(
            "Value           {} → {}",
            money(s, cur),
            money(e, cur)
        ));
    }
    l.push(format!(
        "Deployed        {}",
        amount(r.capital_deployed, r.deployed_pct, "of the money at work")
    ));
    if !r.redacted
        && let Some(w) = r.withdrawn.filter(|w| *w > 0.0)
    {
        l.push(format!("Sale proceeds   {}", money(w, cur)));
    }
    l.push(format!(
        "Dividends       {}",
        amount(r.dividends, r.dividends_pct, "of the money at work")
    ));
    l.push(format!(
        "Realised gains  {}",
        match (r.redacted, r.realized_gain) {
            (false, Some(g)) => signed_money(g, cur),
            _ => r
                .realized_gain_pct
                .map_or("–".into(), |p| format!("{p:+.1}% of the money at work")),
        }
    ));
    let movers = |title: &str, list: &[Mover], l: &mut Vec<String>| {
        if list.is_empty() {
            return;
        }
        l.push(String::new());
        l.push(title.to_string());
        for m in list {
            let figure = match (r.redacted, m.contribution) {
                (false, Some(c)) => signed_money(c, cur),
                _ => m
                    .share_pct
                    .map_or("–".into(), |s| format!("{s:+.0}% of gains and losses")),
            };
            l.push(format!(
                "  {:<16} {:>8}  {}",
                m.symbol,
                pct(m.return_pct),
                figure
            ));
        }
    };
    movers("Top contributors", &r.winners, &mut l);
    movers("Top detractors", &r.laggards, &mut l);
    l.push(String::new());
    l.push(format!(
        "Diversification {} holdings, {:.1} effective bets",
        r.holdings, r.effective_bets
    ));
    let slices = |s: &[Slice]| {
        s.iter()
            .map(|s| format!("{} {:.0}%", s.name, s.weight_pct))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    if !r.cap_tiers.is_empty() {
        l.push(format!("  Cap tiers     {}", slices(&r.cap_tiers)));
    }
    if !r.top_sectors.is_empty() {
        l.push(format!("  Top sectors   {}", slices(&r.top_sectors)));
    }
    if let Some((clean, total)) = r.checkup_clean {
        l.push(format!("  Checkup       {clean}/{total} guardrails clean"));
    }
    if let Some(t) = &r.tax {
        l.push(String::new());
        let mut line = format!("Tax ({})  {} days left", t.year, t.days_left);
        match (r.redacted, t.exemption_remaining, t.exemption_used_pct) {
            (false, Some(rem), _) => line.push_str(&format!(
                " · {} of the long-term exemption unused",
                money(rem, &t.currency)
            )),
            (_, _, Some(used)) => {
                line.push_str(&format!(" · {used:.0}% of the long-term exemption used"))
            }
            _ => {}
        }
        if let Some((due, share)) = t.next_advance_tax {
            line.push_str(&format!(
                " · next advance tax {} ({share:.0}% cumulative)",
                due.format("%d %b")
            ));
        }
        l.push(line);
    }
    if !r.unconverted.is_empty() {
        l.push(format!(
            "No exchange rate for {}: left out of every figure.",
            r.unconverted.join(", ")
        ));
    }
    if !r.unpriced.is_empty() {
        l.push(format!(
            "No price for {}: left out of the return.",
            r.unpriced.join(", ")
        ));
    }
    l.push(String::new());
    l.push(r.disclaimer.to_string());
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::{DividendReceived, Lot, Sale};

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn period_parsing() {
        let p = Period::parse_fy("2026-27").unwrap();
        assert_eq!((p.start, p.end), (d("2026-04-01"), d("2027-03-31")));
        assert_eq!(p.label, "FY 2026-27");
        assert_eq!(Period::parse_fy("FY2026-2027").unwrap(), p);
        assert_eq!(Period::parse_fy("fy 2026-27").unwrap(), p);
        assert_eq!(Period::parse_fy("1999-00").unwrap().end, d("2000-03-31"));
        for bad in ["2026-28", "2026", "26-27", "2026-2028", "abcd-ef"] {
            assert!(Period::parse_fy(bad).is_err(), "{bad}");
        }
        let y = Period::parse_year("2026").unwrap();
        assert_eq!((y.start, y.end), (d("2026-01-01"), d("2026-12-31")));
        assert!(Period::parse_year("26").is_err());
        assert!(Period::parse_year("next").is_err());
    }

    #[test]
    fn default_period_follows_the_base_currency() {
        let fy = Period::default_for("INR", d("2026-11-17"));
        assert_eq!(fy.label, "FY 2026-27");
        let early = Period::default_for("INR", d("2027-02-10"));
        assert_eq!(early.label, "FY 2026-27");
        let cal = Period::default_for("USD", d("2026-11-17"));
        assert_eq!(cal.kind, PeriodKind::Calendar);
        assert_eq!(cal.label, "2026");
        assert_eq!(cal.as_of(d("2026-11-17")), d("2026-11-17"));
        assert_eq!(cal.as_of(d("2027-01-05")), d("2026-12-31"));
        assert_eq!(cal.opening(), d("2025-12-31"));
    }

    #[test]
    fn closes_on_or_before() {
        let c = vec![(d("2026-01-02"), 10.0), (d("2026-01-05"), 11.0)];
        assert_eq!(close_on(&c, d("2026-01-01")), None);
        assert_eq!(close_on(&c, d("2026-01-04")), Some(10.0));
        assert_eq!(close_on(&c, d("2026-01-05")), Some(11.0));
        assert_eq!(close_on(&c, d("2026-02-01")), Some(11.0));
    }

    #[test]
    fn dietz_matches_simple_return_without_flows() {
        let r = modified_dietz(100.0, 110.0, &[], d("2025-12-31"), d("2026-12-31")).unwrap();
        assert!((r - 10.0).abs() < 1e-9);
        // 100 added half way and the value ends at 220: gain 20 over 150.
        let r = modified_dietz(
            100.0,
            220.0,
            &[(d("2026-07-02"), 100.0)],
            d("2025-12-31"),
            d("2026-12-31"),
        )
        .unwrap();
        assert!((r - 20.0 / (100.0 + 100.0 * 182.0 / 365.0) * 100.0).abs() < 1e-9);
        assert_eq!(
            modified_dietz(0.0, 0.0, &[], d("2026-01-01"), d("2026-01-01")),
            None
        );
    }

    /// Bought 10 before the year, 5 more in it, sold 4 of the old lot in
    /// it, one dividend.
    fn fixture() -> Portfolio {
        let mut p = Portfolio::default();
        p.add_lot("AAA.NS", Lot::new(6.0, 100.0, "2025-06-01", "manual"));
        p.add_lot("AAA.NS", Lot::new(5.0, 120.0, "2026-05-10", "manual"));
        p.realized.push(Sale {
            symbol: "AAA.NS".into(),
            qty: 4.0,
            buy_date: "2025-06-01".into(),
            buy_price: 100.0,
            sell_date: "2026-08-01".into(),
            sell_price: 130.0,
            buy_date_estimated: false,
            source: "manual".into(),
            order_id: None,
            intraday: false,
            account: None,
        });
        p.dividends.push(DividendReceived {
            symbol: "AAA.NS".into(),
            ex_date: "2026-07-01".into(),
            per_share: 2.0,
            qty: 15.0,
            amount: 30.0,
            currency: Some("INR".into()),
        });
        p.dividends.push(DividendReceived {
            symbol: "AAA.NS".into(),
            ex_date: "2025-07-01".into(),
            per_share: 2.0,
            qty: 10.0,
            amount: 20.0,
            currency: Some("INR".into()),
        });
        p
    }

    #[test]
    fn symbol_year_from_lots_sales_and_dividends() {
        let p = fixture();
        let fy = Period::indian_fy(2026).unwrap();
        assert_eq!(shares_at(&p, "AAA.NS", fy.opening()), 10.0);
        assert_eq!(shares_at(&p, "AAA.NS", d("2026-11-17")), 11.0);
        let y = symbol_year(&p, "AAA.NS", &fy, d("2026-11-17"), Some(110.0), Some(140.0));
        assert_eq!(y.start_value, Some(1100.0));
        assert_eq!(y.end_value, Some(1540.0));
        assert_eq!(y.bought, 600.0);
        assert_eq!(y.sold, 520.0);
        assert_eq!(y.dividends, 30.0);
        assert_eq!(y.realized_gain, 120.0);
        // 1540 - 1100 - 600 + 520 + 30.
        assert_eq!(y.contribution(), Some(390.0));
        assert!((y.return_pct().unwrap() - 390.0 / 1700.0 * 100.0).abs() < 1e-9);
        // No price where shares were held: unpriced.
        let y = symbol_year(&p, "AAA.NS", &fy, d("2026-11-17"), None, Some(140.0));
        assert_eq!(y.contribution(), None);
        // Nothing held at the start: a zero start value needs no price.
        let y = symbol_year(
            &p,
            "AAA.NS",
            &Period::calendar(2024).unwrap(),
            d("2024-12-31"),
            None,
            None,
        );
        assert_eq!((y.start_value, y.end_value), (Some(0.0), Some(0.0)));
    }

    fn year(sym: &str, start: f64, end: f64) -> SymbolYear {
        SymbolYear {
            symbol: sym.into(),
            start_value: Some(start),
            end_value: Some(end),
            ..Default::default()
        }
    }

    #[test]
    fn totals_rank_winners_and_laggards() {
        let p = Period::calendar(2026).unwrap();
        let years = vec![
            year("A", 100.0, 150.0),
            year("B", 100.0, 80.0),
            year("C", 100.0, 110.0),
            year("D", 100.0, 100.0),
            SymbolYear {
                symbol: "E".into(),
                start_value: None,
                end_value: Some(5.0),
                ..Default::default()
            },
        ];
        let t = totals(&years, &p, d("2026-12-31"), 2);
        assert_eq!(t.unpriced, ["E"]);
        assert!((t.return_pct.unwrap() - 10.0).abs() < 1e-9);
        let w: Vec<&str> = t.winners.iter().map(|m| m.symbol.as_str()).collect();
        assert_eq!(w, ["A", "C"]);
        assert_eq!(t.laggards[0].symbol, "B");
        // Gross gains and losses: 50 + 20 + 10.
        assert!((t.winners[0].share_pct.unwrap() - 62.5).abs() < 1e-9);
        assert!((t.laggards[0].share_pct.unwrap() + 25.0).abs() < 1e-9);
    }

    #[test]
    fn unconverted_amounts_stay_out_of_totals() {
        let p = Period::calendar(2026).unwrap();
        let mut usd = year("AAPL", 100.0, 120.0);
        usd.bought = 1_000.0;
        usd.unconverted = true;
        let mut inr = year("TCS.NS", 100.0, 110.0);
        inr.bought = 50.0;
        let t = totals(&[usd, inr], &p, d("2026-12-31"), 3);
        assert_eq!(t.unconverted, ["AAPL"]);
        assert_eq!(t.bought, 50.0);
        assert_eq!(t.start_value, 100.0);
    }

    #[test]
    fn benchmark_and_advance_tax() {
        let c = vec![(d("2025-12-31"), 200.0), (d("2026-12-31"), 230.0)];
        let p = Period::calendar(2026).unwrap();
        assert!((benchmark_return(&c, &p, d("2026-12-31")).unwrap() - 15.0).abs() < 1e-9);
        assert_eq!(
            next_advance_tax(d("2026-11-17")),
            Some((d("2026-12-15"), 75.0))
        );
        assert_eq!(
            next_advance_tax(d("2027-01-10")),
            Some((d("2027-03-15"), 100.0))
        );
        assert_eq!(next_advance_tax(d("2027-03-20")), None);
    }

    fn card() -> WrapReport {
        WrapReport {
            period: Period::indian_fy(2026).unwrap(),
            as_of: d("2026-11-17"),
            base_currency: "INR".into(),
            redacted: false,
            return_pct: Some(14.2),
            benchmark: "NIFTY 50".into(),
            benchmark_return_pct: Some(9.1),
            alpha_bps: Some(510.0),
            start_value: Some(1_423_456.0),
            end_value: Some(1_682_345.0),
            capital_deployed: Some(182_400.0),
            withdrawn: Some(52_000.0),
            dividends: Some(8_765.0),
            realized_gain: Some(82_400.0),
            deployed_pct: Some(11.4),
            dividends_pct: Some(0.5),
            realized_gain_pct: Some(5.1),
            winners: vec![Mover {
                symbol: "AAA.NS".into(),
                contribution: Some(97_531.0),
                return_pct: Some(31.0),
                share_pct: Some(45.0),
            }],
            laggards: vec![Mover {
                symbol: "BBB.NS".into(),
                contribution: Some(-24_680.0),
                return_pct: Some(-12.0),
                share_pct: Some(-11.0),
            }],
            cap_tiers: vec![Slice {
                name: "Large".into(),
                weight_pct: 70.0,
            }],
            top_sectors: vec![Slice {
                name: "Technology".into(),
                weight_pct: 30.0,
            }],
            holdings: 12,
            effective_bets: 8.4,
            checkup_clean: Some((14, 16)),
            tax: Some(TaxFooter {
                year: "FY 2026-27".into(),
                days_left: 134,
                currency: "INR".into(),
                exemption_limit: Some(125_000.0),
                exemption_remaining: Some(42_600.0),
                exemption_used_pct: Some(65.9),
                next_advance_tax: Some((d("2026-12-15"), 75.0)),
            }),
            unpriced: vec![],
            unconverted: vec![],
            disclaimer: DISCLAIMER,
        }
    }

    #[test]
    fn plain_card_shows_amounts() {
        let text = render(&card()).join("\n");
        assert!(text.contains("₹14,23,456"), "{text}");
        assert!(text.contains("+₹82,400"), "{text}");
        assert!(text.contains("₹42,600"), "{text}");
        assert!(text.contains("14/16 guardrails clean"));
    }

    #[test]
    fn redacted_card_leaks_no_amounts() {
        let r = redact(&card());
        let text = render(&r).join("\n");
        let json = serde_json::to_string(&r).unwrap();
        for out in [&text, &json] {
            for sym in ["₹", "$", "£", "€", "Rs"] {
                assert!(!out.contains(sym), "`{sym}` in {out}");
            }
            for amount in [
                "1423456",
                "14,23,456",
                "1682345",
                "182400",
                "52000",
                "8765",
                "82400",
                "97531",
                "24680",
                "42600",
                "125000",
                "1,25,000",
            ] {
                assert!(!out.contains(amount), "`{amount}` in {out}");
            }
        }
        // Returns, shares and ratios stay.
        assert!(text.contains("+14.2%"));
        assert!(text.contains("+510 bps"));
        assert!(text.contains("11.4% of the money at work"));
        assert!(text.contains("+45% of gains and losses"));
        assert!(text.contains("66% of the long-term exemption used"));
        assert!(r.redacted);
    }
}
