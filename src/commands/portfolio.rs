//! The `portfolio` command: lots, FIFO sales, corporate actions, named
//! portfolios, and live valuation in one base currency (also used by
//! `dashboard`, `wealth`, `rebalance` and the long-term daemon).

use super::*;
use crate::api::Quote;
use crate::offmarket::{self, Mark, OffMarket, PriceSource};
use crate::portfolio::{self as pf, DividendEvent, Fx, Lot, Split};
use crate::tax;

// ── Valuation ──

/// The market's own currency, used for totals when holdings are mixed.
pub(crate) fn market_currency(market: Market) -> &'static str {
    match market {
        Market::In | Market::Commodity => "INR",
        Market::Us | Market::Crypto | Market::Fx => "USD",
        Market::Global(x) => x.info().currency,
    }
}

/// Live prices for a set of holdings plus the exchange rates to total them
/// in one base currency, so INR and USD amounts are never added raw.
/// Mutual funds are priced at AMFI's NAV and SGBs from NSE's list (see
/// [`offmarket`]); `off` says how, and why some could not be.
pub(crate) struct Valuation {
    pub quotes: Vec<Quote>,
    pub fx: Fx,
    pub off: OffMarket,
}

/// Cost and value in the base currency, leaving out holdings without a live
/// price or an exchange rate (named here instead of being valued at 0).
#[derive(Default)]
pub(crate) struct Totals {
    pub cost: f64,
    pub value: f64,
    pub unpriced: Vec<String>,
    pub unconverted: Vec<String>,
}

impl Totals {
    pub fn pnl_pct(&self) -> f64 {
        if self.cost > 0.0 {
            (self.value - self.cost) / self.cost * 100.0
        } else {
            0.0
        }
    }

    /// Warn about holdings left out of the totals.
    pub fn print_exclusions(&self) {
        if !self.unpriced.is_empty() {
            // Funds and SGBs get their own note saying why (see
            // `Valuation::print_price_notes`); `search` can't find them.
            let hint = if self.unpriced.iter().all(|s| offmarket::is_off_market(s)) {
                ""
            } else {
                " Check the symbol with `stockwise search`."
            };
            println!(
                "  {} No live price for {} — excluded from totals.{}",
                "!".yellow(),
                self.unpriced.join(", "),
                hint
            );
        }
        if !self.unconverted.is_empty() {
            println!(
                "  {} No exchange rate for {} — excluded from totals.",
                "!".yellow(),
                self.unconverted.join(", ")
            );
        }
    }
}

impl Valuation {
    pub fn quote(&self, symbol: &str) -> Option<&Quote> {
        self.quotes
            .iter()
            .find(|q| q.symbol.as_deref() == Some(symbol))
    }

    pub fn price(&self, symbol: &str) -> Option<f64> {
        self.quote(symbol).and_then(|q| q.regular_market_price)
    }

    /// How a holding outside Yahoo was priced (funds, SGBs).
    pub fn mark(&self, symbol: &str) -> Option<&Mark> {
        self.off.mark(symbol)
    }

    /// Where `symbol`'s price came from; None when it has none.
    pub fn price_source(&self, symbol: &str) -> Option<PriceSource> {
        self.price(symbol)?;
        Some(self.mark(symbol).map_or(PriceSource::Quote, |m| m.source))
    }

    /// A readable name: the fund's scheme or the SGB's series, else the
    /// quote's name.
    pub fn name(&self, symbol: &str) -> Option<String> {
        self.mark(symbol).and_then(|m| m.name.clone()).or_else(|| {
            let q = self.quote(symbol)?;
            q.short_name.clone().or_else(|| q.long_name.clone())
        })
    }

    /// The NAV's date for a fund priced at AMFI's NAV.
    pub fn nav_date(&self, symbol: &str) -> Option<chrono::NaiveDate> {
        self.mark(symbol)
            .filter(|m| m.source == PriceSource::AmfiNav)
            .and_then(|m| m.as_of)
    }

    /// One line per fund or SGB saying what it is and how it was priced
    /// ("NAV as of 19 Oct 2026"), then the notes (unknown codes, estimates,
    /// interest left out).
    pub fn price_notes(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .off
            .marks
            .iter()
            .map(|m| {
                let how = match m.source {
                    PriceSource::AmfiNav => match m.as_of {
                        Some(d) => format!("NAV as of {} (AMFI, end of day)", d.format("%d %b %Y")),
                        None => "AMFI NAV (end of day)".to_string(),
                    },
                    PriceSource::NseSgb => match m.as_of {
                        Some(d) => format!("NSE last price, saved {}", d.format("%d %b %Y")),
                        None => "NSE last price".to_string(),
                    },
                    PriceSource::GoldEstimate => "estimated from gold spot".to_string(),
                    PriceSource::Quote => "market quote".to_string(),
                };
                match &m.name {
                    Some(name) => format!("{} {} — {}", pad_end(&m.symbol, 14), name, how),
                    None => format!("{} {}", pad_end(&m.symbol, 14), how),
                }
            })
            .collect();
        lines.extend(self.off.notes.iter().map(offmarket::Note::line));
        lines
    }

    /// [`Valuation::price_notes`], dimmed, under a short heading.
    pub fn print_price_notes(&self) {
        let lines = self.price_notes();
        if lines.is_empty() {
            return;
        }
        println!("  {}", "Funds and gold bonds".dimmed().bold());
        for line in lines {
            println!("  {}", line.dimmed());
        }
    }

    /// The quote's currency, or a guess from the symbol's exchange suffix.
    pub fn currency(&self, symbol: &str) -> Option<String> {
        self.quote(symbol)
            .and_then(|q| q.currency.clone())
            .or_else(|| pf::symbol_currency(symbol).map(str::to_string))
    }

    pub fn base(&self) -> &str {
        &self.fx.base
    }

    /// `amount` (in `symbol`'s currency) in the base currency.
    pub fn to_base(&self, symbol: &str, amount: f64) -> Option<f64> {
        self.currency(symbol)
            .and_then(|c| self.fx.convert(amount, &c))
    }

    pub fn totals(&self, portfolio: &Portfolio) -> Totals {
        let mut t = Totals::default();
        for h in &portfolio.holdings {
            let Some(price) = self.price(&h.symbol) else {
                t.unpriced.push(h.symbol.clone());
                continue;
            };
            match (
                self.to_base(&h.symbol, h.shares * price),
                self.to_base(&h.symbol, h.cost_basis()),
            ) {
                (Some(v), Some(c)) => {
                    t.value += v;
                    t.cost += c;
                }
                _ => t.unconverted.push(h.symbol.clone()),
            }
        }
        t
    }

    /// "1 USD = ₹84.12" for each currency that was converted.
    pub fn fx_note(&self) -> Option<String> {
        let used = self.fx.used();
        if used.is_empty() {
            return None;
        }
        let base = self.base();
        let parts: Vec<String> = used
            .iter()
            .map(|(cur, rate)| {
                if *rate >= 1.0 {
                    format!("1 {} = {}", cur, format_price(*rate, Some(base)))
                } else {
                    format!("1 {} = {}", base, format_price(1.0 / rate, Some(cur)))
                }
            })
            .collect();
        Some(format!(
            "Totals in {} — converted at {}",
            base,
            parts.join(", ")
        ))
    }
}

/// Yahoo FX symbol giving the rate from `currency` to `base`, and whether to
/// invert it. USD↔INR always comes from USDINR=X.
pub(super) fn fx_pair(currency: &str, base: &str) -> (String, bool) {
    if currency == "INR" && base == "USD" {
        ("USDINR=X".into(), true)
    } else {
        (format!("{}{}=X", currency, base), false)
    }
}

/// Price `symbols` and fetch the exchange rates to total them. The base
/// currency is `base` if given; otherwise the holdings' own currency when
/// they all share one, or the market's currency when they are mixed.
/// `MF:` funds and SGBs are priced from AMFI and NSE (see [`offmarket`]).
pub(crate) async fn value_holdings(
    src: &impl views::DataSource,
    symbols: &[String],
    market: Market,
    base: Option<&str>,
) -> Result<Valuation> {
    // Yahoo has no `MF:` symbols; asking would only cost a lookup.
    let refs: Vec<&str> = symbols
        .iter()
        .map(String::as_str)
        .filter(|s| !market::is_namespaced(s))
        .collect();
    let mut quotes = if refs.is_empty() {
        Vec::new()
    } else {
        src.quotes(&refs).await?
    };
    let off = offmarket::price(src, symbols, &quotes).await;
    off.merge_into(&mut quotes);
    let mut v = Valuation::new(quotes, symbols, market, base);
    v.off = off;
    v.add_rates(src, &v.currencies(symbols)).await;
    Ok(v)
}

impl Valuation {
    /// Value `symbols` at `quotes` (which may hold other symbols too). Totals
    /// go in `base`, else the holdings' one currency, else the market's.
    /// Call [`Valuation::add_rates`] before totalling mixed currencies.
    pub fn new(quotes: Vec<Quote>, symbols: &[String], market: Market, base: Option<&str>) -> Self {
        let mut v = Valuation {
            quotes,
            fx: Fx::new(market_currency(market)),
            off: OffMarket::default(),
        };
        let currencies = v.currencies(symbols);
        let base = match base {
            Some(b) => b.to_uppercase(),
            None if currencies.len() == 1 => currencies[0].clone(),
            None => market_currency(market).to_string(),
        };
        v.fx = Fx::new(&base);
        v
    }

    /// The distinct currencies `symbols` are priced in.
    pub fn currencies(&self, symbols: &[String]) -> Vec<String> {
        let mut currencies: Vec<String> = symbols.iter().filter_map(|s| self.currency(s)).collect();
        currencies.sort();
        currencies.dedup();
        currencies
    }

    /// Fetch exchange rates into the base for `currencies` not yet known.
    /// A failed lookup is not fatal: amounts in that currency are left out
    /// of totals and named, instead of being added in the wrong currency.
    pub async fn add_rates(&mut self, src: &impl views::DataSource, currencies: &[String]) {
        let base = self.fx.base.clone();
        let mut missing: Vec<String> = currencies
            .iter()
            .filter(|c| self.fx.rate(c).is_none())
            .map(|c| if c == "GBp" { "GBP".into() } else { c.clone() })
            .collect();
        missing.sort();
        missing.dedup();
        if missing.is_empty() {
            return;
        }
        let pairs: Vec<(String, String, bool)> = missing
            .iter()
            .map(|c| {
                let (sym, invert) = fx_pair(c, &base);
                (c.clone(), sym, invert)
            })
            .collect();
        let refs: Vec<&str> = pairs.iter().map(|p| p.1.as_str()).collect();
        let fx_quotes = src.quotes(&refs).await.unwrap_or_default();
        for (cur, sym, invert) in &pairs {
            if let Some(px) = fx_quotes
                .iter()
                .find(|q| q.symbol.as_deref() == Some(sym.as_str()))
                .and_then(|q| q.regular_market_price)
            {
                self.fx.set_rate(cur, if *invert { 1.0 / px } else { px });
            }
        }
    }
}

/// Splits and dividends for `symbol` from `since` (YYYY-MM-DD) to today,
/// oldest first, and the currency the dividends are paid in.
pub(super) async fn fetch_actions(
    client: &YahooClient,
    symbol: &str,
    since: &str,
) -> Result<(Vec<Split>, Vec<DividendEvent>, Option<String>)> {
    let to_date =
        |ts: i64| chrono::DateTime::from_timestamp(ts, 0).map(|d| d.date_naive().to_string());
    let since_ts = chrono::NaiveDate::parse_from_str(since, "%Y-%m-%d")
        .map(|d| {
            d.and_hms_opt(0, 0, 0)
                .expect("midnight")
                .and_utc()
                .timestamp()
        })
        .unwrap_or(0);
    let events = client.get_corporate_actions(symbol, since_ts).await?;
    let today = pf::today();
    let mut splits: Vec<Split> = events
        .splits
        .values()
        .filter(|s| s.numerator > 0.0 && s.denominator > 0.0)
        .filter_map(|s| {
            Some(Split {
                date: to_date(s.date)?,
                numerator: s.numerator,
                denominator: s.denominator,
            })
        })
        .collect();
    splits.sort_by(|a, b| a.date.cmp(&b.date));
    let mut divs: Vec<DividendEvent> = events
        .dividends
        .values()
        .filter(|d| d.amount > 0.0)
        .filter_map(|d| {
            Some(DividendEvent {
                ex_date: to_date(d.date)?,
                per_share: d.amount,
            })
        })
        .filter(|d| d.ex_date <= today)
        .collect();
    divs.sort_by(|a, b| a.ex_date.cmp(&b.ex_date));
    Ok((splits, divs, events.currency))
}

/// Splits Yahoo reports for `symbol` that are not yet applied to its lots.
pub(super) async fn pending_splits(
    client: &YahooClient,
    portfolio: &Portfolio,
    symbol: &str,
) -> Result<Vec<Split>> {
    let Some(h) = portfolio.holding(symbol) else {
        return Ok(Vec::new());
    };
    let Some(since) = h.lots.iter().map(|l| l.units_date()).min() else {
        return Ok(Vec::new());
    };
    let (splits, _, _) = fetch_actions(client, symbol, since).await?;
    Ok(splits
        .into_iter()
        .filter(|s| h.lots.iter().any(|l| s.pending_for(l)))
        .collect())
}

fn describe_splits(splits: &[Split]) -> String {
    splits
        .iter()
        .map(|s| format!("{}:{} on {}", s.numerator, s.denominator, s.date))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Of `snapshot` (symbol, quantity from a broker or holdings file), the
/// holdings whose quantity went up while a split or bonus is not yet applied
/// to their lots — syncing those would count the new shares twice. Returns
/// (symbol, reason). Without a network connection nothing is blocked.
pub(super) async fn blocked_by_pending_splits(
    portfolio: &Portfolio,
    snapshot: &[(String, f64)],
) -> Vec<(String, String)> {
    let grew: Vec<&(String, f64)> = snapshot
        .iter()
        .filter(|(sym, qty)| {
            portfolio
                .holding(sym)
                .is_some_and(|h| *qty > h.shares + 1e-9)
        })
        .collect();
    if grew.is_empty() {
        return Vec::new();
    }
    let Ok(client) = YahooClient::new().await else {
        return Vec::new();
    };
    let mut blocked = Vec::new();
    for (sym, _) in grew {
        if let Ok(splits) = pending_splits(&client, portfolio, sym).await
            && !splits.is_empty()
        {
            blocked.push((
                sym.clone(),
                format!(
                    "split/bonus {} not applied to your lots yet — run `stockwise portfolio actions {} --apply`, then sync again",
                    describe_splits(&splits),
                    sym
                ),
            ));
        }
    }
    blocked
}

pub(super) fn portfolio_title(title: &str) -> String {
    match pf::active() {
        Some(name) => format!("{} — {}", title, name),
        None => title.to_string(),
    }
}

fn signed_money(amount: f64, currency: Option<&str>) -> String {
    let sym = market::currency_symbol(currency);
    if amount >= 0.0 {
        format!("+{}{:.2}", sym, amount).green().to_string()
    } else {
        format!("-{}{:.2}", sym, amount.abs()).red().to_string()
    }
}

fn signed_pct(pct: f64) -> String {
    if pct >= 0.0 {
        format!("+{:.2}%", pct).green().to_string()
    } else {
        format!("{:.2}%", pct).red().to_string()
    }
}

fn xirr_cell(r: Option<f64>) -> String {
    match r {
        Some(r) if r >= 0.0 => format!("{:+.1}%", r * 100.0).green().to_string(),
        Some(r) => format!("{:+.1}%", r * 100.0).red().to_string(),
        None => "—".dimmed().to_string(),
    }
}

/// "2y 3m" / "45d" for a holding period.
fn held_for(from: &str, today: chrono::NaiveDate) -> String {
    let Ok(d) = chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d") else {
        return "?".into();
    };
    let days = (today - d).num_days().max(0);
    if days < 60 {
        format!("{}d", days)
    } else {
        let months = days * 12 / 365;
        if months < 12 {
            format!("{}m", months)
        } else {
            format!("{}y {}m", months / 12, months % 12)
        }
    }
}

/// The symbol of an existing holding: `symbol` resolved for `market`, or as
/// typed when that is what the portfolio holds (`AAPL` while the market is
/// India, where it would resolve to `AAPL.NS`). Sales count too.
fn held_symbol(portfolio: &Portfolio, symbol: &str, market: Market) -> String {
    let resolved = market::resolve_symbol(symbol, market);
    let held = |s: &str| {
        portfolio.holding(s).is_some() || portfolio.realized.iter().any(|r| r.symbol == s)
    };
    let raw = symbol.trim().to_uppercase();
    if !held(&resolved) && held(&raw) {
        raw
    } else {
        resolved
    }
}

pub(super) const ESTIMATED_DATE_HINT: &str = "* purchase date unknown (upgraded or imported without dates) — fix with `stockwise portfolio lots SYMBOL --lot N --date YYYY-MM-DD`";

// ── Portfolio ──

// ── `portfolio --json/--csv`, `/api/portfolio` and the TUI ──

/// One holding as `portfolio --json/--csv` prints it, in the holding's own
/// currency. Price-derived fields are null when there is no live price.
#[derive(Debug, Serialize)]
pub(crate) struct HoldingRow {
    pub(crate) symbol: String,
    pub(crate) shares: f64,
    pub(crate) avg_cost: f64,
    pub(crate) cost_basis: f64,
    pub(crate) price: Option<f64>,
    pub(crate) value: Option<f64>,
    pub(crate) pnl: Option<f64>,
    pub(crate) pnl_percent: Option<f64>,
    /// Share of the portfolio's value, compared in the base currency.
    pub(crate) weight_percent: Option<f64>,
    /// Money-weighted annual return of the holding's buys, sales and dividends.
    pub(crate) xirr_percent: Option<f64>,
    pub(crate) currency: Option<String>,
    /// The scheme name for a fund, the series for an SGB, else the quote's name.
    pub(crate) name: Option<String>,
    /// `quote`, `amfi_nav` (end-of-day NAV), `nse_sgb`, or `gold_estimate`
    /// (an SGB valued at 1 g of gold at spot); null without a price.
    pub(crate) price_source: Option<PriceSource>,
    /// Date of the NAV a fund is valued at.
    pub(crate) nav_date: Option<chrono::NaiveDate>,
}

impl CsvRow for HoldingRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "shares",
        "avg_cost",
        "cost_basis",
        "price",
        "value",
        "pnl",
        "pnl_percent",
        "weight_percent",
        "xirr_percent",
        "currency",
        "name",
        "price_source",
        "nav_date",
    ];
}

/// Totals over the priced holdings, converted to the base currency.
#[derive(Debug, Default, Serialize)]
pub(crate) struct PortfolioTotals {
    pub(crate) cost_basis: f64,
    pub(crate) value: f64,
    pub(crate) pnl: f64,
    pub(crate) pnl_percent: f64,
    pub(crate) currency: Option<String>,
}

/// `portfolio --json`: every holding, totals in one currency, and the
/// holdings left out of the totals — without a live price (`unpriced`) or
/// without an exchange rate (`unconverted`). Never valued at 0. `notes`
/// explain fund and SGB prices: unknown codes, estimates, what is left out.
#[derive(Debug, Default, Serialize)]
pub(crate) struct PortfolioReport {
    pub(crate) holdings: Vec<HoldingRow>,
    pub(crate) totals: PortfolioTotals,
    pub(crate) unpriced: Vec<String>,
    pub(crate) unconverted: Vec<String>,
    pub(crate) notes: Vec<String>,
}

fn pct_of(part: f64, whole: f64) -> f64 {
    if whole > 0.0 {
        part / whole * 100.0
    } else {
        0.0
    }
}

pub(crate) fn portfolio_report(
    portfolio: &Portfolio,
    val: &Valuation,
    today: chrono::NaiveDate,
) -> PortfolioReport {
    let totals = val.totals(portfolio);
    let holdings = portfolio
        .holdings
        .iter()
        .map(|h| {
            let price = val.price(&h.symbol);
            let cost = h.cost_basis();
            let value = price.map(|p| h.shares * p);
            HoldingRow {
                symbol: h.symbol.clone(),
                shares: h.shares,
                avg_cost: h.avg_cost,
                cost_basis: round(cost, 2),
                price,
                value: round_opt(value, 2),
                pnl: value.map(|v| round(v - cost, 2)),
                pnl_percent: value.map(|v| round(pct_of(v - cost, cost), 2)),
                weight_percent: value
                    .and_then(|v| val.to_base(&h.symbol, v))
                    .map(|v| round(pct_of(v, totals.value), 2)),
                xirr_percent: value
                    .and_then(|v| pf::xirr(&portfolio.cash_flows(&h.symbol), v, today))
                    .map(|r| round(r * 100.0, 2)),
                currency: val.currency(&h.symbol),
                name: val.name(&h.symbol),
                price_source: val.price_source(&h.symbol),
                nav_date: val.nav_date(&h.symbol),
            }
        })
        .collect();
    PortfolioReport {
        holdings,
        totals: PortfolioTotals {
            cost_basis: round(totals.cost, 2),
            value: round(totals.value, 2),
            pnl: round(totals.value - totals.cost, 2),
            pnl_percent: round(totals.pnl_pct(), 2),
            currency: Some(val.base().to_string()),
        },
        unpriced: totals.unpriced,
        unconverted: totals.unconverted,
        notes: val.off.notes.iter().map(offmarket::Note::line).collect(),
    }
}

/// Value the active portfolio for `portfolio --json/--csv` and the HTTP API.
/// An empty portfolio needs no network.
pub(crate) async fn load_portfolio_report(
    src: &impl views::DataSource,
    market: Market,
    base: Option<&str>,
) -> Result<PortfolioReport> {
    let portfolio = src.portfolio().map_err(|e| e.context(views::LocalData))?;
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = value_holdings(src, &symbols, market, base).await?;
    Ok(portfolio_report(
        &portfolio,
        &val,
        chrono::Local::now().date_naive(),
    ))
}

pub async fn cmd_portfolio_show(market: Market, base: Option<&str>, fmt: Format) -> Result<()> {
    if !fmt.is_table() {
        let portfolio = Portfolio::load()?;
        let report = if portfolio.holdings.is_empty() {
            portfolio_report(
                &portfolio,
                &Valuation::new(Vec::new(), &[], market, base),
                chrono::Local::now().date_naive(),
            )
        } else {
            load_portfolio_report(&YahooClient::new().await?, market, base).await?
        };
        return match fmt {
            Format::Csv => output::print_csv(&report.holdings),
            _ => output::print_json(&report),
        };
    }
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
        return Ok(());
    }

    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let client = YahooClient::new().await?;
    let mut val = value_holdings(&client, &symbols, market, base).await?;
    let today = chrono::Local::now().date_naive();

    print_header(&portfolio_title(t(Msg::YourPortfolio)));
    // Wide enough for `MF:<ISIN>` (and the `*` of estimated dates).
    let sw = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.len() + 1)
        .max()
        .unwrap_or(0)
        .max(12);
    let rule = "─".repeat(74 + sw);
    // Width-aware padding: translated labels (e.g. Hindi) aren't one column per char.
    println!(
        "  {} {} {} {} {} {} {} {}",
        pad_end(t(Msg::Symbol), sw).bold(),
        pad_start(t(Msg::Shares), 8).bold(),
        pad_start(t(Msg::AvgCost), 10).bold(),
        pad_start(t(Msg::Price), 10).bold(),
        pad_start(t(Msg::Value), 12).bold(),
        pad_start(t(Msg::PnL), 11).bold(),
        pad_start(t(Msg::PnLPct), 8).bold(),
        pad_start("XIRR", 8).bold(),
    );
    println!("  {}", rule.dimmed());

    let mut any_estimated = false;
    for h in &portfolio.holdings {
        let cur = val.currency(&h.symbol);
        let cur = cur.as_deref();
        let mut label = h.symbol.clone();
        if h.has_estimated_dates() {
            label.push('*');
            any_estimated = true;
        }
        // No live price (delisted, renamed, or a bad symbol): show N/A and
        // leave it out of the totals rather than valuing it at 0.
        let Some(price) = val.price(&h.symbol) else {
            println!(
                "  {:<sw$} {:>8.2} {:>10} {:>10} {:>12} {:>11} {:>8} {:>8}",
                label.yellow(),
                h.shares,
                format_price(h.avg_cost, cur),
                "N/A",
                "N/A",
                "—",
                "—",
                "—",
            );
            continue;
        };
        let value = h.shares * price;
        let cost = h.cost_basis();
        let pnl = value - cost;
        let pnl_pct = if cost > 0.0 { pnl / cost * 100.0 } else { 0.0 };
        let xirr = pf::xirr(&portfolio.cash_flows(&h.symbol), value, today);
        println!(
            "  {:<sw$} {:>8.2} {:>10} {:>10} {:>12} {:>11} {:>8} {:>8}",
            label.cyan(),
            h.shares,
            format_price(h.avg_cost, cur),
            format_price(price, cur),
            format_price(value, cur),
            signed_money(pnl, cur),
            signed_pct(pnl_pct),
            xirr_cell(xirr),
        );
    }

    let totals = val.totals(&portfolio);
    let base_currency = val.base().to_string();
    let base = Some(base_currency.as_str());
    let portfolio_xirr = {
        // Money-weighted return across everything ever held (closed
        // positions too), converted at today's rates. Holdings left out of
        // the totals are left out here as well.
        let excluded: Vec<&String> = totals.unpriced.iter().chain(&totals.unconverted).collect();
        let mut flows = Vec::new();
        for sym in portfolio.symbols_with_history() {
            if excluded.contains(&&sym) {
                continue;
            }
            for (d, amount) in portfolio.cash_flows(&sym) {
                match val.to_base(&sym, amount) {
                    Some(a) => flows.push((d, a)),
                    None => continue,
                }
            }
        }
        pf::xirr(&flows, totals.value, today)
    };
    println!("  {}", rule.dimmed());
    println!(
        "  {} {:>8} {:>10} {:>10} {:>12} {:>11} {:>8} {:>8}",
        pad_end(t(Msg::Total), sw).bold(),
        "",
        format_price(totals.cost, base),
        "",
        format_price(totals.value, base).bold(),
        signed_money(totals.value - totals.cost, base),
        signed_pct(totals.pnl_pct()),
        xirr_cell(portfolio_xirr),
    );
    if let Some(note) = val.fx_note() {
        println!("  {}", note.dimmed());
    }

    // Realized P&L and dividends, all time, in the base currency. Closed
    // positions may be in a currency no open holding uses.
    let mut history_currencies: Vec<String> = portfolio
        .realized
        .iter()
        .map(|s| &s.symbol)
        .chain(portfolio.dividends.iter().map(|d| &d.symbol))
        .filter_map(|sym| val.currency(sym))
        .collect();
    history_currencies.sort();
    history_currencies.dedup();
    val.add_rates(&client, &history_currencies).await;
    let mut unconverted: Vec<&str> = portfolio
        .realized
        .iter()
        .map(|s| s.symbol.as_str())
        .chain(portfolio.dividends.iter().map(|d| d.symbol.as_str()))
        .filter(|sym| val.to_base(sym, 1.0).is_none())
        .collect();
    unconverted.sort();
    unconverted.dedup();
    let realized: f64 = portfolio
        .realized
        .iter()
        .filter_map(|s| val.to_base(&s.symbol, s.gain()))
        .sum();
    let dividends: f64 = portfolio
        .dividends
        .iter()
        .filter_map(|d| val.to_base(&d.symbol, d.amount))
        .sum();
    if !portfolio.realized.is_empty() {
        print_kv(
            &format!("Realized P&L ({} sale lots)", portfolio.realized.len()),
            &signed_money(realized, base),
        );
    }
    if !portfolio.dividends.is_empty() {
        print_kv("Dividends received", &format_price(dividends, base));
    }
    println!();
    totals.print_exclusions();
    val.print_price_notes();
    if !unconverted.is_empty() {
        println!(
            "  {} No exchange rate for {} — left out of realized P&L and dividends.",
            "!".yellow(),
            unconverted.join(", ")
        );
    }
    if any_estimated {
        println!("  {}", ESTIMATED_DATE_HINT.dimmed());
    }

    if portfolio.holdings.len() > 1 {
        print_section("Allocation");
        let alloc_items: Vec<(&str, f64)> = portfolio
            .holdings
            .iter()
            .filter_map(|h| {
                let v = val.to_base(&h.symbol, h.shares * val.price(&h.symbol)?)?;
                Some((h.symbol.as_str(), v))
            })
            .collect();
        for line in charts::allocation_chart(&alloc_items) {
            println!("{}", line);
        }
    }
    println!();
    Ok(())
}

pub async fn cmd_portfolio_add(
    symbol: &str,
    shares: f64,
    cost: f64,
    date: Option<&str>,
    market: Market,
) -> Result<()> {
    if !(shares > 0.0 && shares.is_finite()) {
        bail!("Number of shares must be positive");
    }
    if !(cost >= 0.0 && cost.is_finite()) {
        bail!("Cost per share can't be negative");
    }
    let dated = date.is_some();
    let date = match date {
        Some(d) => pf::parse_date(d)?.to_string(),
        None => pf::today(),
    };
    let resolved = market::resolve_symbol(symbol, market);
    let mut portfolio = Portfolio::load()?;
    portfolio.add_lot(&resolved, Lot::new(shares, cost, &date, "manual"));
    portfolio.save()?;
    let cur = pf::symbol_currency(&resolved);
    let units = offmarket::unit_word(&resolved);
    println!(
        "  {} Added {} {} of {} at {} (bought {})",
        "✓".green(),
        shares,
        units,
        resolved,
        format_price(cost, cur),
        date
    );
    if let Some(h) = portfolio.holding(&resolved)
        && h.lots.len() > 1
    {
        println!(
            "  {} Now {} {} in {} lots, average cost {}",
            "·".dimmed(),
            h.shares,
            units,
            h.lots.len(),
            format_price(h.avg_cost, cur)
        );
    }
    if !dated {
        println!(
            "  {}",
            "Dated today. Bought earlier? Add --date YYYY-MM-DD — it decides LTCG/STCG and XIRR."
                .dimmed()
        );
    }
    Ok(())
}

pub async fn cmd_portfolio_remove(symbol: &str, market: Market) -> Result<()> {
    let mut portfolio = Portfolio::load()?;
    let resolved = held_symbol(&portfolio, symbol, market);
    if !portfolio.remove(&resolved) {
        bail!("{} is not in your portfolio", resolved);
    }
    portfolio.save()?;
    println!("  {} Removed {} from portfolio", "✓".green(), resolved);
    println!(
        "  {}",
        "To record a sale (and its P&L for `tax`), use `stockwise portfolio sell` instead."
            .dimmed()
    );
    Ok(())
}

pub async fn cmd_portfolio_sell(
    symbol: &str,
    shares: f64,
    price: Option<f64>,
    date: Option<&str>,
    market: Market,
) -> Result<()> {
    let mut portfolio = Portfolio::load()?;
    let resolved = held_symbol(&portfolio, symbol, market);
    if portfolio.holding(&resolved).is_none() {
        bail!("{} is not in your portfolio", resolved);
    }
    let today = pf::today();
    let date = match date {
        Some(d) => pf::parse_date(d)?.to_string(),
        None => today.clone(),
    };
    let mut cur = pf::symbol_currency(&resolved).map(str::to_string);
    if price.is_none() && date != today {
        bail!(
            "Give the sale price for a past sale: stockwise portfolio sell {} {} <PRICE> --date {}",
            symbol,
            shares,
            date
        );
    }
    // Lots must be in the sale date's share count: with a split or bonus not
    // yet applied, FIFO would match the wrong number of shares.
    // Fund and SGB units never split.
    let client = YahooClient::new().await;
    let checked = match &client {
        _ if offmarket::is_off_market(&resolved) => Some(Vec::new()),
        Ok(c) => pending_splits(c, &portfolio, &resolved).await.ok(),
        Err(_) => None,
    };
    match checked {
        Some(splits) => {
            let before: Vec<Split> = splits.into_iter().filter(|s| s.date <= date).collect();
            if !before.is_empty() {
                bail!(
                    "{} had a split/bonus ({}) that isn't applied to your lots yet, so the share \
                     counts wouldn't match. Run `stockwise portfolio actions {} --apply` first.",
                    resolved,
                    describe_splits(&before),
                    resolved
                );
            }
        }
        None => println!(
            "  {} Couldn't check for splits or bonuses (offline?) — run `stockwise portfolio actions` later.",
            "!".yellow()
        ),
    }
    let price = match price {
        Some(p) => p,
        None => {
            // A fund is redeemed at its NAV: the same price `portfolio` shows.
            let client = client?;
            let val =
                value_holdings(&client, std::slice::from_ref(&resolved), market, None).await?;
            if let Some(c) = val.currency(&resolved) {
                cur = Some(c);
            }
            // An estimate is no sale price: SGBs NSE shows no trade for need one typed.
            if val.price_source(&resolved) == Some(PriceSource::GoldEstimate) {
                bail!(
                    "No market price for {} (only an estimate from gold spot); give the sale price: \
                     stockwise portfolio sell {} {} <PRICE>",
                    resolved,
                    symbol,
                    shares
                );
            }
            for note in val
                .off
                .notes
                .iter()
                .filter(|n| n.symbols.contains(&resolved))
            {
                println!("  {} {}", "!".yellow(), note.line());
            }
            val.price(&resolved).with_context(|| {
                format!(
                    "No live price for {}; give the sale price: stockwise portfolio sell {} {} <PRICE>",
                    resolved, symbol, shares
                )
            })?
        }
    };
    let sales = portfolio.sell_fifo(&resolved, shares, price, &date, "manual")?;
    portfolio.save()?;

    let cur = cur.as_deref();
    println!(
        "  {} Sold {} {} at {} on {} (oldest lots first)",
        "✓".green(),
        shares,
        resolved.cyan(),
        format_price(price, cur),
        date
    );
    let sold = pf::parse_date(&date)?;
    let mut total = 0.0;
    for s in &sales {
        let long = chrono::NaiveDate::parse_from_str(&s.buy_date, "%Y-%m-%d")
            .is_ok_and(|b| tax::is_long_term(b, sold, 12));
        total += s.gain();
        println!(
            "    lot bought {}{}: {} × ({} − {}) = {}  {}",
            s.buy_date,
            if s.buy_date_estimated { "*" } else { "" },
            s.qty,
            format_price(s.sell_price, cur),
            format_price(s.buy_price, cur),
            signed_money(s.gain(), cur),
            if long { "long-term" } else { "short-term" }.dimmed()
        );
    }
    print_kv("Realized P&L", &signed_money(total, cur));
    if sales.iter().any(|s| s.buy_date_estimated) {
        println!("  {}", ESTIMATED_DATE_HINT.dimmed());
    }
    println!(
        "  {}",
        "Kept for `stockwise tax`; `stockwise portfolio lots` shows the history.".dimmed()
    );
    Ok(())
}

pub async fn cmd_portfolio_lots(
    symbol: &str,
    fix_lot: Option<usize>,
    fix_date: Option<&str>,
    market: Market,
) -> Result<()> {
    let mut portfolio = Portfolio::load()?;
    let resolved = held_symbol(&portfolio, symbol, market);
    if let (Some(n), Some(d)) = (fix_lot, fix_date) {
        let d = pf::parse_date(d)?.to_string();
        portfolio.set_lot_date(&resolved, n, &d)?;
        portfolio.save()?;
        println!(
            "  {} Lot #{} of {} now dated {}",
            "✓".green(),
            n,
            resolved,
            d
        );
    }
    let holding = portfolio.holding(&resolved);
    let sales: Vec<&pf::Sale> = portfolio
        .realized
        .iter()
        .filter(|s| s.symbol == resolved)
        .collect();
    let dividends: Vec<&pf::DividendReceived> = portfolio
        .dividends
        .iter()
        .filter(|d| d.symbol == resolved)
        .collect();
    if holding.is_none() && sales.is_empty() {
        bail!("{} is not in your portfolio", resolved);
    }

    // The live price is a nice-to-have here; lots still show without it.
    // A fund's is its NAV.
    let quote = match YahooClient::new().await {
        Ok(client) => value_holdings(&client, std::slice::from_ref(&resolved), market, None)
            .await
            .ok()
            .and_then(|v| v.quote(&resolved).cloned()),
        Err(_) => None,
    };
    let price = quote.as_ref().and_then(|q| q.regular_market_price);
    let cur = quote
        .as_ref()
        .and_then(|q| q.currency.clone())
        .or_else(|| pf::symbol_currency(&resolved).map(str::to_string));
    let cur = cur.as_deref();
    let today = chrono::Local::now().date_naive();
    let long_term = |date: &str| {
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .is_ok_and(|d| tax::is_long_term(d, today, 12))
    };

    print_header(&portfolio_title(&format!("Lots — {}", resolved)));
    if let Some(h) = holding {
        println!(
            "  {:>3} {:<12} {:>9} {:>10} {:>12} {:>8} {:>5}  {}",
            "#".bold(),
            "Bought".bold(),
            "Qty".bold(),
            "Price".bold(),
            "Cost".bold(),
            "Held".bold(),
            "Term".bold(),
            "Source".bold(),
        );
        println!("  {}", "─".repeat(78).dimmed());
        for (i, l) in h.lots.iter().enumerate() {
            let date = if l.date_estimated {
                format!("{}*", l.date)
            } else {
                l.date.clone()
            };
            let mut source = l.source.clone();
            if !l.adjustments.is_empty() {
                source.push_str(&format!(" (adjusted ×{})", l.adjustments.len()));
            }
            println!(
                "  {:>3} {:<12} {:>9.2} {:>10} {:>12} {:>8} {:>5}  {}",
                i + 1,
                date,
                l.qty,
                format_price(l.price, cur),
                format_price(l.cost(), cur),
                held_for(&l.date, today),
                if long_term(&l.date) { "LT" } else { "ST" },
                source.dimmed(),
            );
        }
        println!("  {}", "─".repeat(78).dimmed());
        print_kv(
            "Holding",
            &format!(
                "{} shares, average cost {}, invested {}",
                h.shares,
                format_price(h.avg_cost, cur),
                format_price(h.cost_basis(), cur)
            ),
        );
        if let Some(p) = price {
            let value = h.shares * p;
            print_kv(
                "Value",
                &format!(
                    "{} at {} ({})",
                    format_price(value, cur),
                    format_price(p, cur),
                    signed_money(value - h.cost_basis(), cur)
                ),
            );
        }
    }
    if !sales.is_empty() {
        print_section("Sales (FIFO)");
        println!(
            "  {:<12} {:>9} {:<12} {:>10} {:>10} {:>12} {:>5}",
            "Sold".bold(),
            "Qty".bold(),
            "Bought".bold(),
            "Buy".bold(),
            "Sell".bold(),
            "Gain".bold(),
            "Term".bold(),
        );
        for s in &sales {
            let long = match (
                chrono::NaiveDate::parse_from_str(&s.buy_date, "%Y-%m-%d"),
                chrono::NaiveDate::parse_from_str(&s.sell_date, "%Y-%m-%d"),
            ) {
                (Ok(b), Ok(d)) => tax::is_long_term(b, d, 12),
                _ => false,
            };
            println!(
                "  {:<12} {:>9.2} {:<12} {:>10} {:>10} {:>12} {:>5}",
                s.sell_date,
                s.qty,
                s.buy_date,
                format_price(s.buy_price, cur),
                format_price(s.sell_price, cur),
                signed_money(s.gain(), cur),
                if long { "LT" } else { "ST" },
            );
        }
        let total: f64 = sales.iter().map(|s| s.gain()).sum();
        print_kv("Realized P&L", &signed_money(total, cur));
    }
    if !dividends.is_empty() {
        print_section("Dividends received");
        for d in &dividends {
            println!(
                "  {:<12} {:>9.2} shares × {:<10} = {}",
                d.ex_date,
                d.qty,
                format_price(d.per_share, cur),
                format_price(d.amount, cur)
            );
        }
    }
    let value_now = holding.zip(price).map(|(h, p)| h.shares * p);
    if holding.is_none() || value_now.is_some() {
        let flows = portfolio.cash_flows(&resolved);
        print_kv(
            "XIRR",
            &xirr_cell(pf::xirr(&flows, value_now.unwrap_or(0.0), today)),
        );
    }
    if holding.is_some_and(|h| h.has_estimated_dates()) {
        println!("\n  {}", ESTIMATED_DATE_HINT.dimmed());
    }
    println!();
    Ok(())
}

/// Splits, bonuses and dividends since the first purchase of each holding.
/// Shows what would change; only `apply` writes, and applying twice is a no-op.
pub async fn cmd_portfolio_actions(
    symbol: Option<&str>,
    apply: bool,
    market: Market,
) -> Result<()> {
    let mut portfolio = Portfolio::load()?;
    // Fully sold symbols too: dividends received before the sale count.
    let history = portfolio.symbols_with_history();
    let targets: Vec<String> = match symbol {
        Some(s) => {
            let resolved = held_symbol(&portfolio, s, market);
            if !history.contains(&resolved) {
                bail!("{} is not in your portfolio", resolved);
            }
            if offmarket::is_off_market(&resolved) {
                println!(
                    "\n  {}",
                    format!("{resolved}: fund and SGB units have no splits or dividends to apply.")
                        .dimmed()
                );
                return Ok(());
            }
            vec![resolved]
        }
        // Fund and SGB units never split, and Yahoo has no data for them.
        None => history
            .into_iter()
            .filter(|s| !offmarket::is_off_market(s))
            .collect(),
    };
    if targets.is_empty() {
        println!(
            "\n  {}",
            "No holdings with corporate actions to check.".dimmed()
        );
        return Ok(());
    }

    let client = YahooClient::new().await?;
    let today = pf::today();
    print_header(&portfolio_title("Corporate Actions"));

    let (mut split_lots, mut new_divs, mut div_total) =
        (0usize, 0usize, Vec::<(String, f64)>::new());
    // Already applied or recorded by an earlier --apply: counted, not listed.
    let (mut done_splits, mut done_divs) = (0usize, 0usize);
    let mut failed = Vec::new();
    for sym in &targets {
        let first = portfolio
            .holding(sym)
            .into_iter()
            .flat_map(|h| h.lots.iter().map(|l| l.date.clone()))
            .chain(
                portfolio
                    .realized
                    .iter()
                    .filter(|s| &s.symbol == sym)
                    .map(|s| s.buy_date.clone()),
            )
            .min()
            .unwrap_or_else(|| today.clone());
        let (splits, divs, quote_currency) = match fetch_actions(&client, sym, &first).await {
            Ok(a) => a,
            Err(e) => {
                failed.push(format!("{}: {:#}", sym, e));
                continue;
            }
        };
        let currency = quote_currency.or_else(|| pf::symbol_currency(sym).map(str::to_string));
        let cur = currency.as_deref();

        let mut lines = Vec::new();
        for split in &splits {
            let Some(h) = portfolio.holding(sym) else {
                continue;
            };
            let pending: Vec<&Lot> = h.lots.iter().filter(|l| split.pending_for(l)).collect();
            let applied = h.lots.iter().any(|l| l.adjustments.contains(&split.id()));
            let what = format!(
                "Split/bonus {}:{} on {}",
                split.numerator, split.denominator, split.date
            );
            if !pending.is_empty() {
                split_lots += pending.len();
                let detail: Vec<String> = pending
                    .iter()
                    .map(|l| {
                        format!(
                            "{} → {} @ {} → {}",
                            l.qty,
                            l.qty * split.ratio(),
                            format_price(l.price, cur),
                            format_price(l.price / split.ratio(), cur)
                        )
                    })
                    .collect();
                lines.push(format!(
                    "  {} {} — {} lot(s): {}",
                    if apply { "✓".green() } else { "+".yellow() },
                    what,
                    pending.len(),
                    detail.join("; ")
                ));
            } else if applied {
                done_splits += 1;
            }
        }
        for ev in &divs {
            match portfolio.dividend_due(sym, ev, &splits) {
                Some(qty) => {
                    new_divs += 1;
                    let amount = qty * ev.per_share;
                    match div_total.iter_mut().find(|(c, _)| Some(c.as_str()) == cur) {
                        Some((_, t)) => *t += amount,
                        None => div_total.push((cur.unwrap_or("").to_string(), amount)),
                    }
                    lines.push(format!(
                        "  {} Dividend {}/share on {} — {} shares = {}",
                        if apply { "✓".green() } else { "+".yellow() },
                        format_price(ev.per_share, cur),
                        ev.ex_date,
                        qty,
                        format_price(amount, cur)
                    ));
                }
                None if portfolio
                    .dividends
                    .iter()
                    .any(|d| &d.symbol == sym && d.ex_date == ev.ex_date) =>
                {
                    done_divs += 1;
                }
                None => {}
            }
        }
        if !lines.is_empty() {
            println!("\n  {}", sym.cyan().bold());
            for l in &lines {
                println!("{}", l);
            }
        }
        if apply {
            for split in &splits {
                portfolio.apply_split(sym, split);
            }
            for ev in &divs {
                portfolio.record_dividend(sym, ev, &splits, cur);
            }
        }
    }

    if failed.len() == targets.len() {
        bail!("Could not fetch corporate actions: {}", failed.join("; "));
    }
    let pending = split_lots + new_divs;
    println!();
    let totals: Vec<String> = div_total
        .iter()
        .map(|(c, t)| format_price(*t, Some(c.as_str())))
        .collect();
    let earlier = if done_splits + done_divs > 0 {
        format!(
            " ({} split(s) and {} dividend(s) applied earlier)",
            done_splits, done_divs
        )
    } else {
        String::new()
    };
    if pending == 0 {
        println!(
            "  {} Up to date — no new splits, bonuses or dividends since your purchases{}.",
            "✓".green(),
            earlier
        );
    } else if apply {
        portfolio.save()?;
        println!(
            "  {} Adjusted {} lot(s) for splits/bonuses and recorded {} dividend(s){}.",
            "✓".green().bold(),
            split_lots,
            new_divs,
            if totals.is_empty() {
                String::new()
            } else {
                format!(" ({})", totals.join(", "))
            }
        );
    } else {
        println!(
            "  {} lot(s) to adjust for splits/bonuses, {} dividend(s) to record. Nothing changed yet —",
            split_lots, new_divs
        );
        println!(
            "  run {} to apply them (safe to repeat).",
            "stockwise portfolio actions --apply".cyan()
        );
    }
    if !failed.is_empty() {
        println!("  {} Skipped: {}", "!".yellow(), failed.join("; "));
    }
    if split_lots > 0 {
        println!(
            "\n  {}",
            "Splits and bonuses multiply quantity and divide price, keeping cost and purchase date. \
             Indian tax treats bonus shares as bought at zero cost on the allotment date, so check \
             bonus lots against your tax statement."
                .dimmed()
        );
    }
    println!();
    Ok(())
}

/// The default portfolio and every named one, with the active one marked.
pub fn cmd_portfolio_list() -> Result<()> {
    let active = pf::active().map(str::to_string);
    let mut names: Vec<Option<String>> = vec![None];
    names.extend(pf::list_names()?.into_iter().map(Some));
    if active.is_some() && !names.contains(&active) {
        names.push(active.clone());
    }
    print_header("Portfolios");
    for name in &names {
        let marker = if *name == active {
            "*".green().bold()
        } else {
            " ".normal()
        };
        let label = name.as_deref().unwrap_or("default");
        match Portfolio::load_named(name.as_deref()) {
            Ok(p) => {
                let lots: usize = p.holdings.iter().map(|h| h.lots.len()).sum();
                println!(
                    "  {} {:<20} {:>3} holdings  {:>4} lots  {:>4} sales  {}",
                    marker,
                    label.cyan(),
                    p.holdings.len(),
                    lots,
                    p.realized.len(),
                    pf::file_name(name.as_deref()).dimmed()
                );
            }
            Err(e) => println!("  {} {:<20} {} {:#}", marker, label, "unreadable:".red(), e),
        }
    }
    println!(
        "\n  {}",
        "Use one with --portfolio NAME (or STOCKWISE_PORTFOLIO=NAME); adding a holding to a new name creates it."
            .dimmed()
    );
    println!();
    Ok(())
}
