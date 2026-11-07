//! 6-pillar score for a stock (US or Indian).
//!
//! Six pillars (valuation, growth, balance sheet, smart money, analyst
//! consensus, technical trend) are each scored 0–100 from fixed rules over the
//! data StockWise fetched, and combined into a 0–100 composite. The score
//! describes the data. It is not a recommendation: there is no buy/sell call,
//! no entry zone, no stop-loss and no target price. A pillar with no data shows
//! "no data" and is left out of the composite, and `data_coverage` says how
//! much of the model's input was present.

use crate::insights::{Impact, InsightCard};
use crate::market::AssetClass;
use crate::research::Research;
use colored::Colorize;

/// Printed under every score view.
pub const DISCLAIMER: &str = "A score summarises the data; it is not a recommendation or investment advice. \
     StockWise is not a SEBI-registered investment adviser or research analyst.";

/// The composite is only shown when pillars holding at least this share of the
/// total weight have data.
const MIN_COMPOSITE_WEIGHT: f64 = 0.5;

/// Pillar weights, in display order. The composite and `--explain` both read
/// the weights from the pillars built from these.
const W_VALUATION: f64 = 0.20;
const W_GROWTH: f64 = 0.25;
const W_BALANCE: f64 = 0.15;
const W_OWNERSHIP: f64 = 0.15;
const W_ANALYST: f64 = 0.10;
const W_TECHNICAL: f64 = 0.15;

/// Descriptive band for one pillar score. It describes the score; it does not
/// tell the reader what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Strong,
    Moderate,
    Weak,
    NoData,
}

impl Band {
    pub fn of(score: Option<f64>) -> Self {
        match score {
            None => Band::NoData,
            Some(s) if s >= 65.0 => Band::Strong,
            Some(s) if s >= 45.0 => Band::Moderate,
            Some(_) => Band::Weak,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Band::Strong => "Strong",
            Band::Moderate => "Moderate",
            Band::Weak => "Weak",
            Band::NoData => "No data",
        }
    }

    fn painted(&self) -> String {
        let l = format!("{:<8}", self.label());
        match self {
            Band::Strong => l.green().bold().to_string(),
            Band::Moderate => l.yellow().bold().to_string(),
            Band::Weak => l.red().bold().to_string(),
            Band::NoData => l.dimmed().to_string(),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PillarScore {
    pub name: &'static str,
    /// 0 to 100, or `None` when none of the pillar's inputs were available.
    pub score: Option<f64>,
    /// Share of the composite, 0.0 to 1.0.
    pub weight: f64,
    pub band: Band,
    /// How many of the pillar's inputs had data.
    pub inputs_present: usize,
    pub inputs_possible: usize,
    pub summary: String,
}

/// How many pillars fall in each band.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct BandCounts {
    pub strong: usize,
    pub moderate: usize,
    pub weak: usize,
    pub no_data: usize,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct DecisionVerdict {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub currency: String,
    /// Weighted composite of the pillars that have data, 0 to 100. `None` when
    /// too little data is available (see `note`).
    pub overall_score: Option<f64>,
    /// 0 to 100: how much of the model's input was present, weighted by pillar
    /// weight. Not a measure of how likely the score is to be right.
    pub data_coverage: f64,
    pub pillar_bands: BandCounts,
    pub pillars: Vec<PillarScore>,
    /// Plain statement of why the composite is missing, when it is.
    pub note: Option<String>,
    /// Facts from the data that raised pillar scores.
    pub bull_points: Vec<String>,
    /// Facts from the data that lowered pillar scores.
    pub bear_points: Vec<String>,
    pub next_catalyst: Option<String>,
    /// Data sources that failed while fetching, from the research bundle.
    pub warnings: Vec<String>,
    pub disclaimer: &'static str,
}

/// Collects the notes and the input count for one pillar while it is scored.
struct Builder {
    score: f64,
    notes: Vec<String>,
    present: usize,
    possible: usize,
}

impl Builder {
    fn new(base: f64, possible: usize) -> Self {
        Builder {
            score: base,
            notes: Vec::new(),
            present: 0,
            possible,
        }
    }

    fn add(&mut self, delta: f64, note: String) {
        self.score += delta;
        self.notes.push(note);
    }

    fn finish(self, name: &'static str, weight: f64, lo: f64, hi: f64) -> PillarScore {
        let score = (self.present > 0).then(|| self.score.clamp(lo, hi));
        PillarScore {
            name,
            score,
            weight,
            band: Band::of(score),
            inputs_present: self.present,
            inputs_possible: self.possible,
            summary: if self.present == 0 {
                "No data".to_string()
            } else if self.notes.is_empty() {
                "No strong signal either way".to_string()
            } else {
                self.notes.join(", ")
            },
        }
    }
}

fn no_data_pillar(name: &'static str, weight: f64, possible: usize, why: &str) -> PillarScore {
    PillarScore {
        name,
        score: None,
        weight,
        band: Band::NoData,
        inputs_present: 0,
        inputs_possible: possible,
        summary: why.to_string(),
    }
}

fn valuation(r: &Research) -> PillarScore {
    let mut b = Builder::new(50.0, 3);

    if let Some(peg) = r.fin.peg {
        b.present += 1;
        if peg > 0.0 && peg <= 1.0 {
            b.add(25.0, format!("Low PEG ({:.2}x)", peg));
        } else if peg > 0.0 && peg <= 1.5 {
            b.add(15.0, format!("Moderate PEG ({:.2}x)", peg));
        } else if peg >= 2.5 {
            b.add(-20.0, format!("Elevated PEG ({:.2}x)", peg));
        }
    }

    if r.fin.trailing_pe.is_some() || r.fin.forward_pe.is_some() {
        b.present += 1;
    }
    if let (Some(t_pe), Some(f_pe)) = (r.fin.trailing_pe, r.fin.forward_pe) {
        if t_pe > 0.0 && f_pe > 0.0 {
            let comp = (t_pe - f_pe) / t_pe;
            if comp >= 0.15 {
                b.add(
                    15.0,
                    format!("Forward P/E below trailing ({:.1}x vs {:.1}x)", f_pe, t_pe),
                );
            } else if comp <= -0.15 {
                b.add(-15.0, "Forward earnings expected to decline".into());
            }
        }
    } else if let Some(pe) = r.fin.trailing_pe {
        if pe > 0.0 && pe < 15.0 {
            b.add(15.0, format!("Low P/E multiple ({:.1}x)", pe));
        } else if pe > 50.0 {
            b.add(-15.0, format!("High P/E multiple ({:.1}x)", pe));
        }
    }

    if let Some(pb) = r.fin.pb {
        b.present += 1;
        if pb < 1.5 && r.fin.roe.unwrap_or(0.0) > 0.10 {
            b.add(10.0, format!("Low P/B ({:.1}x) with ROE above 10%", pb));
        }
    }
    b.finish("Valuation", W_VALUATION, 10.0, 95.0)
}

fn growth(r: &Research) -> PillarScore {
    let mut b = Builder::new(50.0, 4);

    if let Some(rg) = r.fin.revenue_growth {
        b.present += 1;
        if rg >= 0.15 {
            b.add(20.0, format!("Revenue growth {:+.1}%", rg * 100.0));
        } else if rg >= 0.06 {
            b.add(10.0, format!("Moderate revenue growth {:+.1}%", rg * 100.0));
        } else if rg < 0.0 {
            b.add(-20.0, format!("Revenue contraction {:+.1}%", rg * 100.0));
        }
    }

    if let Some(eg) = r.fin.earnings_growth {
        b.present += 1;
        if eg >= 0.20 {
            b.add(20.0, format!("Earnings growth {:+.1}%", eg * 100.0));
        } else if eg < -0.10 {
            b.add(-20.0, format!("Earnings decline {:+.1}%", eg * 100.0));
        }
    }

    if let Some(pm) = r.fin.profit_margin {
        b.present += 1;
        if pm >= 0.18 {
            b.add(10.0, format!("High net margin {:.1}%", pm * 100.0));
        } else if pm < 0.0 {
            b.add(-20.0, "Negative net margin".into());
        }
    }

    if let Some(roe) = r.fin.roe {
        b.present += 1;
        if roe >= 0.20 {
            b.add(10.0, format!("High ROE {:.1}%", roe * 100.0));
        }
    }
    b.finish("Growth & Profitability", W_GROWTH, 10.0, 95.0)
}

fn balance_sheet(r: &Research) -> PillarScore {
    let mut b = Builder::new(55.0, 2);

    if !r.is_financial() {
        if let Some(de) = r.fin.de {
            b.present += 1;
            if de <= 30.0 {
                b.add(25.0, format!("Low debt-to-equity ({:.1}%)", de));
            } else if de <= 80.0 {
                b.add(10.0, format!("Manageable leverage ({:.1}%)", de));
            } else if de >= 160.0 {
                b.add(-25.0, format!("High leverage ({:.1}%)", de));
            }
        }
        if let Some(cr) = r.fin.current_ratio {
            b.present += 1;
            if cr >= 1.5 {
                b.add(10.0, format!("Healthy working capital ({:.2}x)", cr));
            } else if cr < 1.0 {
                b.add(-15.0, "Current ratio under 1.0x".into());
            }
        }
    } else {
        // Debt ratios do not apply to banks and insurers: count as covered.
        b.present = b.possible;
        b.add(15.0, "Financial sector capital structure".into());
    }

    if let Some(cash) = r.fin.total_cash
        && cash > 10_000_000_000.0
    {
        b.add(10.0, "Substantial cash reserves".into());
    }
    b.finish("Balance Sheet", W_BALANCE, 15.0, 95.0)
}

fn ownership(r: &Research) -> PillarScore {
    let mut b = Builder::new(50.0, 3);

    if r.holdings.len() >= 2 {
        b.present += 1;
        let n = r.holdings.len();
        if let (Some(fii_curr), Some(fii_prev)) = (r.holdings[n - 1].fii, r.holdings[n - 2].fii) {
            let d = fii_curr - fii_prev;
            if d >= 0.05 {
                b.add(
                    20.0,
                    format!("FII stake up ({:.2}% → {:.2}%)", fii_prev, fii_curr),
                );
            } else if d <= -0.05 {
                b.add(
                    -15.0,
                    format!("FII stake down ({:.2}% → {:.2}%)", fii_prev, fii_curr),
                );
            }
        }
        if let Some(false) = r.holdings[n - 1].pledged {
            b.add(10.0, "0% promoter pledge".into());
        } else if let Some(true) = r.holdings[n - 1].pledged {
            b.add(-20.0, "Promoter shares pledged".into());
        }
    }

    if r.fin.inst_pct.is_some() || r.fin.inst_net_buy_pct.is_some() {
        b.present += 1;
    }
    if let Some(inst_pct) = r.fin.inst_pct
        && inst_pct >= 0.70
    {
        b.add(15.0, format!("Institutions own {:.1}%", inst_pct * 100.0));
    }
    if let Some(net_buy) = r.fin.inst_net_buy_pct {
        if net_buy >= 0.015 {
            b.add(15.0, "Institutions net adding (6M)".into());
        } else if net_buy <= -0.015 {
            b.add(-15.0, "Institutions net reducing (6M)".into());
        }
    }

    if !r.insiders.is_empty() {
        b.present += 1;
    }
    let insider_buys: f64 = r
        .insiders
        .iter()
        .filter(|t| t.is_buy)
        .map(|t| t.value)
        .sum();
    let insider_sells: f64 = r
        .insiders
        .iter()
        .filter(|t| !t.is_buy)
        .map(|t| t.value)
        .sum();
    if insider_buys > 100_000.0 && insider_buys > insider_sells {
        b.add(15.0, "Recent insider open-market buying".into());
    } else if insider_sells > 5_000_000.0 && insider_buys < 50_000.0 {
        b.add(-10.0, "Insider selling observed".into());
    }
    b.finish("Smart Money & Ownership", W_OWNERSHIP, 10.0, 95.0)
}

fn analyst(r: &Research) -> PillarScore {
    let mut b = Builder::new(50.0, 2);

    if let Some(rec) = r.fin.rec_mean {
        b.present += 1;
        let note = format!("Analyst mean rating {:.1} of 5 (1 = most positive)", rec);
        if rec <= 1.75 {
            b.add(25.0, note);
        } else if rec <= 2.25 {
            b.add(15.0, note);
        } else if rec >= 3.5 {
            b.add(-20.0, note);
        }
    }

    if let Some(target) = r.fin.target_mean {
        b.present += 1;
        if let Some(p) = r.price()
            && p > 0.0
        {
            let upside = (target - p) / p;
            let note = format!(
                "Analyst mean target {:+.0}% vs price (third-party data)",
                upside * 100.0
            );
            if upside >= 0.18 {
                b.add(25.0, note);
            } else if upside >= 0.08 {
                b.add(10.0, note);
            } else if upside <= -0.05 {
                b.add(-15.0, note);
            }
        }
    }
    b.finish("Analyst Consensus", W_ANALYST, 10.0, 95.0)
}

fn technical(r: &Research) -> PillarScore {
    let mut b = Builder::new(50.0, 2);

    if let (Some(p), Some(ma50), Some(ma200)) = (
        r.price(),
        r.quote.fifty_day_average,
        r.quote.two_hundred_day_average,
    ) {
        b.present += 1;
        if p > ma50 && ma50 > ma200 {
            b.add(25.0, "Price above 50-day above 200-day average".into());
        } else if p < ma50 && ma50 < ma200 {
            b.add(-25.0, "Price below 50-day below 200-day average".into());
        } else if p > ma200 {
            b.add(10.0, "Price above 200-day average".into());
        } else {
            b.add(-10.0, "Price below 200-day average".into());
        }
    }

    if r.prices.len() >= 20
        && let Some(rsi) = crate::technical::rsi(&r.prices.close, 14)
    {
        b.present += 1;
        if (45.0..=68.0).contains(&rsi) {
            b.add(15.0, format!("RSI {:.1} (mid range)", rsi));
        } else if rsi <= 32.0 {
            b.add(10.0, format!("RSI {:.1} (oversold zone)", rsi));
        } else if rsi >= 75.0 {
            b.add(-15.0, format!("RSI {:.1} (overbought zone)", rsi));
        }
    }
    b.finish("Technical Trend", W_TECHNICAL, 10.0, 95.0)
}

const NOT_EQUITY: &str = "Not applicable to this asset type";

/// Compute the 6-pillar score for one symbol.
pub fn generate_verdict(r: &Research, cards: &[InsightCard]) -> DecisionVerdict {
    let price = r.price().unwrap_or(0.0);
    let class = AssetClass::of(&r.symbol);
    let equity = class == AssetClass::Stock;

    let pillars = if equity {
        vec![
            valuation(r),
            growth(r),
            balance_sheet(r),
            ownership(r),
            analyst(r),
            technical(r),
        ]
    } else {
        // Crypto, FX and indices have no earnings, balance sheet, owners or
        // analysts: only the price trend can be scored.
        vec![
            no_data_pillar("Valuation", W_VALUATION, 3, NOT_EQUITY),
            no_data_pillar("Growth & Profitability", W_GROWTH, 4, NOT_EQUITY),
            no_data_pillar("Balance Sheet", W_BALANCE, 2, NOT_EQUITY),
            no_data_pillar("Smart Money & Ownership", W_OWNERSHIP, 3, NOT_EQUITY),
            no_data_pillar("Analyst Consensus", W_ANALYST, 2, NOT_EQUITY),
            technical(r),
        ]
    };

    let (overall_score, note) = composite(&pillars, r, class);
    let data_coverage = coverage(&pillars);

    let mut counts = BandCounts::default();
    for p in &pillars {
        match p.band {
            Band::Strong => counts.strong += 1,
            Band::Moderate => counts.moderate += 1,
            Band::Weak => counts.weak += 1,
            Band::NoData => counts.no_data += 1,
        }
    }

    let drivers = |impact: Impact| -> Vec<String> {
        cards
            .iter()
            .filter(|c| c.impact == impact)
            .take(4)
            .map(|c| format!("{}: {}", c.title, c.description))
            .collect()
    };

    let next_catalyst = if let Some(ed) = r.calendar.next_earnings {
        let date_str = chrono::DateTime::from_timestamp(ed, 0)
            .map(|d| d.format("%d %b %Y").to_string())
            .unwrap_or_default();
        Some(format!("Quarterly earnings release on {}", date_str))
    } else if let Some(xd) = r.calendar.ex_dividend {
        let date_str = chrono::DateTime::from_timestamp(xd, 0)
            .map(|d| d.format("%d %b %Y").to_string())
            .unwrap_or_default();
        Some(format!("Ex-dividend date on {}", date_str))
    } else {
        None
    };

    DecisionVerdict {
        symbol: r.symbol.clone(),
        name: r.name.clone(),
        price,
        currency: r.currency.clone().unwrap_or_else(|| "USD".into()),
        overall_score,
        data_coverage,
        pillar_bands: counts,
        pillars,
        note,
        bull_points: drivers(Impact::Positive),
        bear_points: drivers(Impact::Negative),
        next_catalyst,
        warnings: r.warnings.clone(),
        disclaimer: DISCLAIMER,
    }
}

/// Weighted mean over the pillars that have data (weights rescaled), or `None`
/// with the reason when they hold less than half of the total weight.
fn composite(
    pillars: &[PillarScore],
    r: &Research,
    class: AssetClass,
) -> (Option<f64>, Option<String>) {
    let scored: Vec<&PillarScore> = pillars.iter().filter(|p| p.score.is_some()).collect();
    let weight: f64 = scored.iter().map(|p| p.weight).sum();
    let total: f64 = pillars.iter().map(|p| p.weight).sum();
    if scored.is_empty() || weight < MIN_COMPOSITE_WEIGHT * total {
        let why = match class {
            AssetClass::Stock => format!(
                "Not enough data for a composite score: {} of {} pillars have data.",
                scored.len(),
                pillars.len()
            ),
            _ => format!(
                "Not enough data for a composite score: {} is not a stock, so only the technical trend is scored.",
                r.symbol
            ),
        };
        return (None, Some(why));
    }
    let sum: f64 = scored
        .iter()
        .map(|p| p.score.unwrap_or(0.0) * p.weight)
        .sum();
    (Some(sum / weight), None)
}

/// `Σ weight × inputs present ÷ inputs possible`, as a percentage.
fn coverage(pillars: &[PillarScore]) -> f64 {
    let total: f64 = pillars.iter().map(|p| p.weight).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let got: f64 = pillars
        .iter()
        .filter(|p| p.inputs_possible > 0)
        .map(|p| p.weight * p.inputs_present as f64 / p.inputs_possible as f64)
        .sum();
    got / total * 100.0
}

/// Terminal visual score bar e.g. [████████░░] 78/100
fn score_bar(score: f64) -> String {
    let filled_len = ((score / 100.0) * 10.0).round() as usize;
    let filled = "█".repeat(filled_len.min(10));
    let empty = "░".repeat(10 - filled_len.min(10));
    if score >= 65.0 {
        format!("{}{}", filled.green(), empty.dimmed())
    } else if score >= 45.0 {
        format!("{}{}", filled.yellow(), empty.dimmed())
    } else {
        format!("{}{}", filled.red(), empty.dimmed())
    }
}

/// "Strong on 3 · Moderate on 2 · Weak on 1" plus a no-data count when needed.
fn band_summary(c: &BandCounts) -> String {
    let mut parts = vec![
        format!("Strong on {}", c.strong),
        format!("Moderate on {}", c.moderate),
        format!("Weak on {}", c.weak),
    ];
    if c.no_data > 0 {
        parts.push(format!("no data on {}", c.no_data));
    }
    parts.join(" · ")
}

fn composite_line(v: &DecisionVerdict) -> String {
    match v.overall_score {
        Some(s) => format!("{:.0}/100  {}", s, score_bar(s)),
        None => "not available".dimmed().to_string(),
    }
}

macro_rules! outln {
    ($o:expr, $($arg:tt)*) => {{
        use std::fmt::Write as _;
        let _ = writeln!($o, $($arg)*);
    }};
}

/// Compact score block for the bare-symbol snapshot and `deep`.
pub fn print_snapshot(v: &DecisionVerdict, symbol_hint: &str) {
    print!("{}", render_snapshot(v, symbol_hint));
}

/// [`print_snapshot`] as a string.
pub fn render_snapshot(v: &DecisionVerdict, symbol_hint: &str) -> String {
    let mut o = String::new();
    outln!(
        o,
        "  Composite score: {}  (data coverage {:.0}%)",
        composite_line(v),
        v.data_coverage
    );
    let pillars: Vec<String> = v
        .pillars
        .iter()
        .map(|p| match p.score {
            Some(s) => format!("{} {:.0}", p.name, s),
            None => format!("{} no data", p.name),
        })
        .collect();
    outln!(o, "  {}", band_summary(&v.pillar_bands));
    outln!(o, "  {}", pillars.join(" · ").dimmed());
    if let Some(n) = &v.note {
        outln!(o, "  {}", n.dimmed());
    }
    outln!(
        o,
        "  {}",
        format!(
            "Run `stockwise verdict {symbol_hint}` for the breakdown. {}",
            v.disclaimer
        )
        .dimmed()
        .italic()
    );
    o
}

/// Print the 6-pillar score card in the terminal.
pub fn print_verdict(v: &DecisionVerdict) {
    print_verdict_with(v, false);
}

/// Print the 6-pillar score card, optionally with the `--explain` notes.
pub fn print_verdict_with(v: &DecisionVerdict, explain: bool) {
    print!("{}", render_verdict(v, explain));
}

/// [`print_verdict_with`] as a string.
pub fn render_verdict(v: &DecisionVerdict, explain: bool) -> String {
    let mut o = String::new();
    let csym = if v.currency == "INR" { "₹" } else { "$" };
    let border = "═".repeat(64);
    let subline = "─".repeat(64);

    outln!(o, "\n{}", border.cyan());
    outln!(o, "  6-PILLAR SCORE: {} ({})", v.symbol.bold(), v.name);
    outln!(o, "  Current Price: {}{:.2}", csym, v.price);
    outln!(o, "{}", border.cyan());

    outln!(o, "\n  Composite score:  {}", composite_line(v));
    outln!(o, "  Data coverage:    {:.0}%", v.data_coverage);
    outln!(o, "  Pillars:          {}", band_summary(&v.pillar_bands));
    if let Some(n) = &v.note {
        outln!(o, "  {}", n.yellow());
    }

    outln!(
        o,
        "\n  {}",
        "PILLAR SCORES (weight in composite)".bold().yellow()
    );
    outln!(o, "  {}", subline.dimmed());
    for p in &v.pillars {
        let score = match p.score {
            Some(s) => format!("{:>3.0}/100 {}", s, score_bar(s)),
            None => format!("{:>7} {}", "—", "░".repeat(10).dimmed()),
        };
        outln!(
            o,
            "  {:<24} {:>3.0}%  {} {}  {}",
            p.name.bold(),
            p.weight * 100.0,
            score,
            p.band.painted(),
            p.summary.dimmed()
        );
    }

    if !v.warnings.is_empty() {
        outln!(o, "\n  {}", "DATA NOTES".bold().yellow());
        outln!(o, "  {}", subline.dimmed());
        for w in &v.warnings {
            outln!(o, "  {} {}", "!".yellow(), w);
        }
    }

    if !v.bull_points.is_empty() {
        outln!(o, "\n  {}", "POSITIVE DRIVERS IN THE DATA".bold().green());
        outln!(o, "  {}", subline.dimmed());
        for b in &v.bull_points {
            outln!(o, "  {} {}", "✔".green(), b);
        }
    }

    if !v.bear_points.is_empty() {
        outln!(o, "\n  {}", "RISKS IN THE DATA".bold().red());
        outln!(o, "  {}", subline.dimmed());
        for b in &v.bear_points {
            outln!(o, "  {} {}", "✖".red(), b);
        }
    }

    if let Some(cat) = &v.next_catalyst {
        outln!(
            o,
            "\n  {} Next scheduled event: {}",
            "◆".magenta().bold(),
            cat.bold()
        );
    }

    if explain {
        outln!(
            o,
            "\n  {}",
            "HOW TO READ THIS SCORE (--explain)".bold().cyan()
        );
        outln!(o, "  {}", subline.dimmed());
        for line in explain_lines(v) {
            outln!(o, "  {}", line);
        }
    }

    outln!(o, "\n  {}\n", v.disclaimer.dimmed().italic());
    o
}

/// The `--explain` text. The weights come from the pillars themselves, so they
/// cannot drift from the composite.
pub fn explain_lines(v: &DecisionVerdict) -> Vec<String> {
    let weights: Vec<String> = v
        .pillars
        .iter()
        .map(|p| format!("{} {:.0}%", p.name, p.weight * 100.0))
        .collect();
    vec![
        format!(
            "• Composite (0–100): weighted average of the pillar scores: {}.",
            weights.join(", ")
        ),
        format!(
            "  A pillar with no data is left out and the other weights are rescaled. The composite is shown only when pillars with data hold at least {:.0}% of the weight.",
            MIN_COMPOSITE_WEIGHT * 100.0
        ),
        "• Pillar score: 50 is neutral. Each rule moves it up or down by a fixed amount, e.g. a low PEG adds 25, and the result is kept between 10 and 95.".to_string(),
        "• Bands: Strong is 65 and above, Moderate is 45 to 64, Weak is below 45.".to_string(),
        "• Data coverage: for each pillar, the share of its inputs that had data, weighted by the pillar weight. Inputs are PEG, P/E and P/B (valuation); revenue growth, earnings growth, net margin and ROE (growth); debt-to-equity and current ratio (balance sheet); shareholding history, institutional ownership and insider trades (smart money); analyst rating and target (analyst); 50/200-day averages and 20+ price bars (technical).".to_string(),
        "• Failed data sources lower coverage and are listed under DATA NOTES.".to_string(),
        "• Scores describe the data. They are not a prediction and not a recommendation.".to_string(),
    ]
}

#[cfg(test)]
pub(crate) mod testing {
    /// Words and phrases that make output read as advice. Output of every
    /// score view must contain none of them.
    pub const FORBIDDEN: &[&str] = &[
        "buy",
        "sell",
        "hold",
        "accumulate",
        "take profit",
        "entry zone",
        "suggested entry",
        "stop-loss",
        "stop loss",
        "target price",
        "risk / reward",
        "risk-reward",
        "reduce",
    ];

    /// Panics when `text` contains advice wording. "buying" and "selling" are
    /// allowed as plain descriptions of insider and institutional flows.
    pub fn assert_neutral(text: &str) {
        let lower = text.to_lowercase();
        for word in FORBIDDEN {
            let mut from = 0;
            while let Some(i) = lower[from..].find(word) {
                let at = from + i;
                let after = lower[at + word.len()..].chars().next();
                let before = lower[..at].chars().next_back();
                let whole = !after.is_some_and(|c| c.is_alphanumeric())
                    && !before.is_some_and(|c| c.is_alphanumeric());
                assert!(
                    !whole,
                    "advice wording {word:?} in output: {}",
                    &text[at.saturating_sub(40)..(at + 60).min(text.len())]
                );
                from = at + word.len();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::assert_neutral;
    use super::*;
    use crate::insights::generate_rich_insights;
    use crate::research::{Holding, InsiderTxn, Research};

    /// A stock with every input the model reads.
    fn full_stock() -> Research {
        let mut r = Research::default();
        r.symbol = "AAPL".into();
        r.name = "Apple Inc.".into();
        r.currency = Some("USD".into());
        r.quote.regular_market_price = Some(200.0);
        r.quote.fifty_day_average = Some(190.0);
        r.quote.two_hundred_day_average = Some(180.0);
        r.fin.peg = Some(0.85);
        r.fin.trailing_pe = Some(28.0);
        r.fin.forward_pe = Some(22.0);
        r.fin.pb = Some(1.2);
        r.fin.revenue_growth = Some(0.18);
        r.fin.earnings_growth = Some(0.24);
        r.fin.profit_margin = Some(0.25);
        r.fin.roe = Some(0.35);
        r.fin.de = Some(40.0);
        r.fin.current_ratio = Some(1.8);
        r.fin.inst_pct = Some(0.75);
        r.fin.rec_mean = Some(1.6);
        r.fin.target_mean = Some(240.0);
        r.holdings = vec![
            Holding {
                date: chrono::NaiveDate::from_ymd_opt(2025, 3, 31).unwrap(),
                promoter: None,
                fii: Some(20.0),
                dii: None,
                mf: None,
                retail: None,
                pledged: Some(false),
            },
            Holding {
                date: chrono::NaiveDate::from_ymd_opt(2025, 6, 30).unwrap(),
                promoter: None,
                fii: Some(21.0),
                dii: None,
                mf: None,
                retail: None,
                pledged: Some(false),
            },
        ];
        r.insiders = vec![InsiderTxn {
            ts: 0,
            name: "A".into(),
            relation: "Director".into(),
            is_buy: true,
            shares: 1000.0,
            value: 500_000.0,
        }];
        let closes: Vec<f64> = (0..60)
            .map(|i| 150.0 + i as f64 * 0.8 + (i % 3) as f64)
            .collect();
        r.prices.close = closes.clone();
        r.prices.high = closes.iter().map(|c| c + 1.0).collect();
        r.prices.low = closes.iter().map(|c| c - 1.0).collect();
        r.prices.open = closes.clone();
        r.prices.volume = vec![1000.0; 60];
        r
    }

    #[test]
    fn full_data_scores_well_with_high_coverage() {
        let r = full_stock();
        let cards = generate_rich_insights(&r);
        let v = generate_verdict(&r, &cards);
        assert!(v.overall_score.unwrap() >= 70.0, "{:?}", v.overall_score);
        assert!(v.data_coverage > 80.0, "coverage {}", v.data_coverage);
        assert_eq!(v.pillar_bands.no_data, 0);
        assert!(v.note.is_none());
    }

    #[test]
    fn empty_research_has_low_coverage_and_no_composite() {
        let v = generate_verdict(&Research::default(), &[]);
        assert!(v.data_coverage < 30.0, "coverage {}", v.data_coverage);
        assert_eq!(v.overall_score, None);
        assert!(v.note.as_deref().unwrap().contains("Not enough data"));
        assert!(v.pillars.iter().all(|p| p.band == Band::NoData));
    }

    #[test]
    fn coverage_follows_the_data_that_is_present() {
        let full = generate_verdict(&full_stock(), &[]).data_coverage;
        let mut r = full_stock();
        r.fin.peg = None;
        r.fin.trailing_pe = None;
        r.fin.forward_pe = None;
        r.fin.pb = None;
        r.fin.rec_mean = None;
        r.fin.target_mean = None;
        let less = generate_verdict(&r, &[]).data_coverage;
        assert!(less < full, "{less} should be below {full}");
        // Valuation (20%) and analyst (10%) lost all inputs.
        assert!((full - less - 30.0).abs() < 1.0, "{full} vs {less}");
    }

    #[test]
    fn pillar_without_data_is_left_out_of_composite() {
        let mut r = full_stock();
        r.fin.rec_mean = None;
        r.fin.target_mean = None;
        let v = generate_verdict(&r, &[]);
        let analyst = v
            .pillars
            .iter()
            .find(|p| p.name == "Analyst Consensus")
            .unwrap();
        assert_eq!(analyst.score, None);
        assert_eq!(analyst.band, Band::NoData);
        assert_eq!(v.pillar_bands.no_data, 1);
        let (sum, w) = v
            .pillars
            .iter()
            .filter_map(|p| p.score.map(|s| (s * p.weight, p.weight)))
            .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
        assert!((v.overall_score.unwrap() - sum / w).abs() < 1e-9);
    }

    #[test]
    fn crypto_and_fx_get_only_the_technical_pillar() {
        for sym in ["BTC-USD", "EURUSD=X", "^NSEI"] {
            let mut r = full_stock();
            r.symbol = sym.into();
            let v = generate_verdict(&r, &generate_rich_insights(&r));
            assert_eq!(v.overall_score, None, "{sym}");
            let scored: Vec<_> = v.pillars.iter().filter(|p| p.score.is_some()).collect();
            assert_eq!(scored.len(), 1, "{sym}");
            assert_eq!(scored[0].name, "Technical Trend");
            assert!(v.note.as_deref().unwrap().contains("not a stock"));
            assert!(v.data_coverage <= 15.0 + 1e-9, "{sym}: {}", v.data_coverage);
        }
    }

    #[test]
    fn explain_weights_match_the_composite() {
        let v = generate_verdict(&full_stock(), &[]);
        let total: f64 = v.pillars.iter().map(|p| p.weight).sum();
        assert!((total - 1.0).abs() < 1e-9);
        let text = explain_lines(&v).join("\n");
        for (name, pct) in [
            ("Valuation", 20),
            ("Growth & Profitability", 25),
            ("Balance Sheet", 15),
            ("Smart Money & Ownership", 15),
            ("Analyst Consensus", 10),
            ("Technical Trend", 15),
        ] {
            assert!(
                text.contains(&format!("{name} {pct}%")),
                "{name} {pct}%\n{text}"
            );
        }
        assert!(!text.contains("Profitability (20%)"));
        // Every weight in the explain text is the weight the composite used.
        let expected: f64 = v.pillars.iter().map(|p| p.score.unwrap() * p.weight).sum();
        assert!((v.overall_score.unwrap() - expected).abs() < 1e-9);
    }

    #[test]
    fn bands_follow_thresholds() {
        assert_eq!(Band::of(Some(65.0)), Band::Strong);
        assert_eq!(Band::of(Some(64.9)), Band::Moderate);
        assert_eq!(Band::of(Some(45.0)), Band::Moderate);
        assert_eq!(Band::of(Some(44.9)), Band::Weak);
        assert_eq!(Band::of(None), Band::NoData);
    }

    #[test]
    fn json_has_scores_and_no_advice_fields() {
        let r = full_stock();
        let v = generate_verdict(&r, &generate_rich_insights(&r));
        let json = serde_json::to_value(&v).unwrap();
        let obj = json.as_object().unwrap();
        for gone in [
            "action",
            "entry_zone",
            "stop_loss",
            "target_price",
            "upside_pct",
            "risk_pct",
            "risk_reward",
            "confidence",
            "short_term",
            "medium_term",
            "long_term",
        ] {
            assert!(!obj.contains_key(gone), "{gone} should be gone");
        }
        for kept in [
            "symbol",
            "overall_score",
            "data_coverage",
            "pillars",
            "disclaimer",
        ] {
            assert!(obj.contains_key(kept), "{kept} missing");
        }
        assert_neutral(&json.to_string());
    }

    #[test]
    fn text_output_is_neutral_for_every_shape_of_data() {
        let mut shapes = vec![Research::default(), full_stock()];
        let mut bearish = full_stock();
        bearish.quote.regular_market_price = Some(100.0);
        bearish.fin.peg = Some(3.0);
        bearish.fin.revenue_growth = Some(-0.1);
        bearish.fin.earnings_growth = Some(-0.3);
        bearish.fin.profit_margin = Some(-0.05);
        bearish.fin.de = Some(200.0);
        bearish.fin.rec_mean = Some(4.0);
        bearish.fin.target_mean = Some(80.0);
        bearish.quote.fifty_day_average = Some(120.0);
        bearish.quote.two_hundred_day_average = Some(140.0);
        shapes.push(bearish);
        let mut crypto = full_stock();
        crypto.symbol = "BTC-USD".into();
        shapes.push(crypto);
        for r in shapes {
            let v = generate_verdict(&r, &generate_rich_insights(&r));
            assert_neutral(&explain_lines(&v).join("\n"));
            assert_neutral(&serde_json::to_string(&v).unwrap());
            assert_neutral(&render_verdict(&v, true));
            assert_neutral(&render_snapshot(&v, &r.symbol));
            let mut text = String::new();
            for p in &v.pillars {
                text.push_str(&format!("{} {}\n", p.name, p.summary));
            }
            text.push_str(&v.bull_points.join("\n"));
            text.push_str(&v.bear_points.join("\n"));
            text.push_str(&v.note.clone().unwrap_or_default());
            text.push_str(v.disclaimer);
            assert_neutral(&text);
        }
    }

    #[test]
    fn neutral_guard_catches_advice() {
        for bad in [
            "STRONG BUY",
            "Sell / avoid",
            "Entry Zone ₹1",
            "Stop-Loss Level",
            "Target Price: 5",
            "take profit",
        ] {
            let caught = std::panic::catch_unwind(|| assert_neutral(bad)).is_err();
            assert!(caught, "{bad:?} should be flagged");
        }
        // Flow descriptions are fine.
        assert_neutral("Recent insider open-market buying; FII selling");
    }

    #[test]
    fn disclaimer_says_not_a_recommendation() {
        let v = generate_verdict(&Research::default(), &[]);
        assert!(v.disclaimer.contains("not a recommendation"));
    }
}
