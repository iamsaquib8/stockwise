//! The `portfolio` command: lots, FIFO sales, corporate actions, named
//! portfolios, and live valuation in one base currency (also used by
//! `dashboard`, `wealth`, `rebalance` and the long-term daemon).

use super::*;
use crate::api::Quote;
use crate::funds;
use crate::offmarket::{self, Mark, OffMarket, PriceSource};
use crate::portfolio::{self as pf, DividendEvent, Fx, Lot, Split};
use crate::rebalance::{HoldingInput, RebalanceActionType};
use crate::tax;
use crate::xray::{self, XrayInputHolding};

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
    /// 31 Jan 2018 prices (Section 112A grandfathering) for the symbols whose
    /// old lots need one; only `tax_valuation` fills it.
    pub fmv_2018: Vec<(String, f64)>,
    /// Daily exchange rates into the base, to cost purchases, sales and
    /// dividends at the rate of their own day ([`Valuation::load_history`]).
    pub hist: crate::fx_history::FxHistory,
}

/// Cost and value in the base currency, leaving out holdings without a live
/// price or an exchange rate (named here instead of being valued at 0).
#[derive(Default)]
pub(crate) struct Totals {
    /// Cost in the base currency at each purchase's own exchange rate.
    pub cost: f64,
    pub value: f64,
    /// The same cost at today's exchange rate: `cost_today - cost` is what
    /// currency moves added to the value ([`Totals::fx_effect`]).
    pub cost_today: f64,
    pub unpriced: Vec<String>,
    pub unconverted: Vec<String>,
    /// Holdings whose cost had no historical rate and was converted at
    /// today's (named in a note).
    pub cost_at_today_fx: Vec<String>,
}

impl Totals {
    /// Gain or loss from exchange-rate moves since the purchases, in the base.
    pub fn fx_effect(&self) -> f64 {
        self.cost_today - self.cost
    }

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
        if !self.cost_at_today_fx.is_empty() {
            println!(
                "  {} No exchange rate for the purchase dates of {}: their cost is converted at today's rate.",
                "·".dimmed(),
                self.cost_at_today_fx.join(", ")
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

    /// The 31 Jan 2018 price of `symbol`, when it was looked up and found.
    pub fn fmv_2018(&self, symbol: &str) -> Option<f64> {
        self.fmv_2018
            .iter()
            .find(|(s, _)| s == symbol)
            .map(|(_, v)| *v)
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

    /// Market-cap tier of a priced stock; `Unknown` for funds and the rest.
    pub fn cap_tier(&self, symbol: &str) -> market::CapTier {
        self.quote(symbol)
            .map_or(market::CapTier::Unknown, market::cap_tier_of)
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
                self.holding_cost_base(h),
                self.to_base(&h.symbol, h.cost_basis()),
            ) {
                (Some(v), Some((c, historical)), Some(today)) => {
                    t.value += v;
                    t.cost += c;
                    t.cost_today += today;
                    if !historical {
                        t.cost_at_today_fx.push(h.symbol.clone());
                    }
                }
                _ => t.unconverted.push(h.symbol.clone()),
            }
        }
        t
    }

    /// Units of base per unit of `symbol`'s currency on `date`: the day's
    /// reference rate when [`Valuation::load_history`] found one (true),
    /// else today's rate (false).
    pub fn rate_on(&self, symbol: &str, date: chrono::NaiveDate) -> Option<(f64, bool)> {
        let cur = self.currency(symbol)?;
        self.currency_rate_on(&cur, date)
    }

    /// [`Valuation::rate_on`] for a currency.
    pub fn currency_rate_on(&self, cur: &str, date: chrono::NaiveDate) -> Option<(f64, bool)> {
        if let Some(r) = self.hist.rate_on(cur, date) {
            return Some((r, true));
        }
        self.fx.rate(cur).map(|r| (r, cur == self.base()))
    }

    /// What `lot` of `symbol` cost in the base currency: what was paid when
    /// it was bought in the base (`portfolio add --currency`), else its cost
    /// at the purchase date's rate, else at today's. The flag is false when
    /// today's rate had to be used.
    pub fn lot_cost_base(&self, symbol: &str, lot: &Lot) -> Option<(f64, bool)> {
        let cur = self.currency(symbol)?;
        let cost = lot.cost();
        let (major_cost, _) = market::to_major(cost, &cur);
        if let Some(paid) = lot.base_cost(self.base(), major_cost) {
            return Some((paid, true));
        }
        let date = pf::parse_date(&lot.date).ok();
        let (rate, historical) = match date {
            Some(d) => self.currency_rate_on(&cur, d)?,
            None => (self.fx.rate(&cur)?, false),
        };
        Some((cost * rate, historical))
    }

    /// [`Valuation::lot_cost_base`] summed over a holding's lots.
    pub fn holding_cost_base(&self, h: &pf::Holding) -> Option<(f64, bool)> {
        h.lots.iter().try_fold((0.0, true), |(sum, all), l| {
            let (c, hist) = self.lot_cost_base(&h.symbol, l)?;
            Some((sum + c, all && hist))
        })
    }

    /// A sale's gain in the base currency: proceeds less charges at the sale
    /// date's rate, less the cost at the purchase's (what was paid in the
    /// base when the lot records it).
    pub fn sale_gain_base(&self, sale: &pf::Sale) -> Option<f64> {
        let cur = self.currency(&sale.symbol)?;
        let sold = pf::parse_date(&sale.sell_date).ok();
        let bought = pf::parse_date(&sale.buy_date).ok();
        let rate = |d: Option<chrono::NaiveDate>| match d {
            Some(d) => self.currency_rate_on(&cur, d).map(|r| r.0),
            None => self.fx.rate(&cur),
        };
        let proceeds = (sale.qty * sale.sell_price - sale.charges()) * rate(sold)?;
        let cost_local = sale.qty * sale.buy_price;
        let (major_cost, _) = market::to_major(cost_local, &cur);
        let cost = match sale
            .fx_currency
            .as_deref()
            .zip(sale.buy_fx_rate)
            .filter(|(c, r)| c.eq_ignore_ascii_case(self.base()) && *r > 0.0)
        {
            Some((_, r)) => major_cost * r,
            None => cost_local * rate(bought)?,
        };
        Some(proceeds - cost)
    }

    /// A dividend in the base currency at its ex-date's rate.
    pub fn dividend_base(&self, d: &pf::DividendReceived) -> Option<f64> {
        let cur = d.currency.clone().or_else(|| self.currency(&d.symbol))?;
        let rate = match pf::parse_date(&d.ex_date).ok() {
            Some(day) => self.currency_rate_on(&cur, day)?.0,
            None => self.fx.rate(&cur)?,
        };
        Some(d.amount * rate)
    }

    /// `symbol`'s cash flows (see [`Portfolio::cash_flow_items`]) in the
    /// base currency, each at its own date's rate (a purchase paid in the
    /// base at what was paid). `None` when a flow has no rate at all.
    pub fn flows_base(
        &self,
        portfolio: &Portfolio,
        symbol: &str,
    ) -> Option<Vec<(chrono::NaiveDate, f64)>> {
        self.flow_items_base(portfolio, symbol)
            .map(|v| v.into_iter().map(|(d, a, _)| (d, a)).collect())
    }

    /// [`Valuation::flows_base`] with what each flow was.
    pub fn flow_items_base(
        &self,
        portfolio: &Portfolio,
        symbol: &str,
    ) -> Option<Vec<(chrono::NaiveDate, f64, pf::FlowKind)>> {
        let own = self.currency(symbol);
        portfolio
            .cash_flow_items(symbol)
            .into_iter()
            .map(|f| {
                let cur = f.currency.clone().or_else(|| own.clone())?;
                if let Some((paid, rate)) = &f.paid
                    && paid.eq_ignore_ascii_case(self.base())
                    && *rate > 0.0
                {
                    let (major, _) = market::to_major(f.amount, &cur);
                    return Some((f.date, major * rate, f.kind));
                }
                let (rate, _) = self.currency_rate_on(&cur, f.date)?;
                Some((f.date, f.amount * rate, f.kind))
            })
            .collect()
    }

    /// Load daily exchange rates for every currency `portfolio` has
    /// bought, sold or been paid dividends in, from its first such day.
    /// Without a network connection and nothing cached, conversions fall
    /// back to today's rate.
    pub async fn load_history(&mut self, portfolio: &Portfolio) {
        let mut needs: Vec<(String, chrono::NaiveDate)> = Vec::new();
        for sym in portfolio.symbols_with_history() {
            let own = self.currency(&sym);
            for f in portfolio.cash_flow_items(&sym) {
                if let Some(cur) = f.currency.or_else(|| own.clone()) {
                    needs.push((cur, f.date));
                }
            }
        }
        if needs.iter().all(|(c, _)| self.hist.has(c)) {
            return;
        }
        self.hist = crate::fx_history::FxHistory::load(self.base(), &needs).await;
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
/// invert it. Yahoo quotes the currency that ranks first below as the base of
/// the pair (EURUSD=X, USDINR=X); other pairs keep `currency` first.
pub(super) fn fx_pair(currency: &str, base: &str) -> (String, bool) {
    let (currency, base) = (currency.to_uppercase(), base.to_uppercase());
    let (currency, base) = (currency.as_str(), base.as_str());
    const QUOTE_ORDER: [&str; 5] = ["EUR", "GBP", "AUD", "NZD", "USD"];
    let rank = |c: &str| {
        QUOTE_ORDER
            .iter()
            .position(|q| q.eq_ignore_ascii_case(c))
            .unwrap_or(QUOTE_ORDER.len())
    };
    if rank(currency) <= rank(base) {
        (format!("{}{}=X", currency, base), false)
    } else {
        (format!("{}{}=X", base, currency), true)
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
            fmv_2018: Vec::new(),
            hist: Default::default(),
        };
        let currencies = v.currencies(symbols);
        let base = match base {
            Some(b) => b.to_uppercase(),
            None if currencies.len() == 1 => currencies[0].clone(),
            None => market_currency(market).to_string(),
        };
        v.fx = Fx::new(&base);
        v.hist = crate::fx_history::FxHistory::new(&base);
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
pub(crate) async fn fetch_actions(
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
                bonus: None,
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

/// Ask NSE which of the `splits` Yahoo reported for an Indian `symbol` are
/// bonus issues, and mark those (see [`crate::calendar::check_bonuses`]).
/// Non-Indian symbols and an empty list cost no request. Returns false when
/// NSE could not be asked, so the splits stay splits.
pub(super) async fn mark_bonuses(
    cal: &mut crate::calendar::Calendar<'_>,
    symbol: &str,
    splits: &mut [Split],
) -> bool {
    if splits.is_empty() || crate::calendar::nse_symbol(symbol).is_none() {
        return true;
    }
    let actions = cal.nse_actions(symbol).await;
    !matches!(
        crate::calendar::check_bonuses(symbol, splits, actions.as_deref()),
        crate::calendar::BonusCheck::Unavailable
    )
}

/// The note printed when NSE could not tell bonus issues from splits.
pub(super) fn nse_offline_note(cal: &crate::calendar::Calendar<'_>, symbols: &[String]) -> String {
    format!(
        "NSE could not be reached{}, so the splits of {} were applied as splits. \
         If one was a bonus issue, Indian tax treats the bonus shares as bought at \
         zero cost on the allotment date: check those lots against your tax statement.",
        cal.nse_note()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default(),
        symbols.join(", ")
    )
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

pub(crate) fn signed_money(amount: f64, currency: Option<&str>) -> String {
    let sym = market::currency_symbol(currency);
    let body = format!(
        "{sym}{}",
        crate::display::group_number(&format!("{:.2}", amount.abs()), market::is_inr(currency))
    );
    if amount >= 0.0 {
        format!("+{body}").green().to_string()
    } else {
        format!("-{body}").red().to_string()
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
pub(crate) fn held_symbol(portfolio: &Portfolio, symbol: &str, market: Market) -> String {
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
    /// `mega|large|mid|small|micro`, `unknown` for funds and anything without a market cap.
    pub(crate) cap_tier: &'static str,
    /// Cost in the base currency at each purchase date's exchange rate.
    pub(crate) cost_basis_base: Option<f64>,
    pub(crate) value_base: Option<f64>,
    /// What exchange-rate moves since the purchases added, in the base
    /// currency (0 for a holding in the base currency).
    pub(crate) fx_effect: Option<f64>,
    /// The quote-based rule action (`crate::action`); `portfolio review`
    /// gives the full one. Null without a quote (funds, SGBs, no price).
    pub(crate) action: Option<crate::action::Action>,
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
        "cap_tier",
        "cost_basis_base",
        "value_base",
        "fx_effect",
        "action",
    ];
}

/// Totals over the priced holdings, converted to the base currency.
#[derive(Debug, Default, Serialize)]
pub(crate) struct PortfolioTotals {
    /// At each purchase date's exchange rate.
    pub(crate) cost_basis: f64,
    pub(crate) value: f64,
    pub(crate) pnl: f64,
    pub(crate) pnl_percent: f64,
    pub(crate) currency: Option<String>,
    /// The part of `pnl` from exchange-rate moves since the purchases.
    pub(crate) fx_effect: f64,
    /// Holdings whose purchase-date rate was not found (cost at today's rate).
    pub(crate) cost_at_today_fx: Vec<String>,
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
    /// Splits and bonuses not yet applied to the lots (valued as if applied).
    pub(crate) warnings: Vec<String>,
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
    let mut actions = super::actions::quick_actions(portfolio, val);
    let holdings = portfolio
        .holdings
        .iter()
        .map(|h| {
            let price = val.price(&h.symbol);
            let cost = h.cost_basis();
            let value = price.map(|p| h.shares * p);
            let cost_base = val.holding_cost_base(h).map(|c| c.0);
            let value_base = value.and_then(|v| val.to_base(&h.symbol, v));
            let fx_effect = cost_base
                .zip(val.to_base(&h.symbol, cost))
                .map(|(hist, now)| round(now - hist, 2));
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
                // No annual return from guessed purchase dates.
                xirr_percent: value
                    .filter(|_| !h.has_estimated_dates())
                    .and_then(|v| pf::xirr(&portfolio.cash_flows(&h.symbol), v, today))
                    .map(|r| round(r * 100.0, 2)),
                currency: val.currency(&h.symbol),
                name: val.name(&h.symbol),
                price_source: val.price_source(&h.symbol),
                nav_date: val.nav_date(&h.symbol),
                cap_tier: val.cap_tier(&h.symbol).as_str(),
                cost_basis_base: round_opt(cost_base, 2),
                value_base: round_opt(value_base, 2),
                fx_effect,
                action: actions.remove(&h.symbol),
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
            fx_effect: round(totals.fx_effect(), 2),
            cost_at_today_fx: totals.cost_at_today_fx,
        },
        unpriced: totals.unpriced,
        unconverted: totals.unconverted,
        notes: val.off.notes.iter().map(offmarket::Note::line).collect(),
        warnings: Vec::new(),
    }
}

/// Value the active portfolio for `portfolio --json/--csv` and the HTTP API.
/// An empty portfolio needs no network.
pub(crate) async fn load_portfolio_report(
    src: &impl views::DataSource,
    market: Market,
    base: Option<&str>,
) -> Result<PortfolioReport> {
    load_portfolio_report_named(src, market, base, None).await
}

pub(crate) async fn load_portfolio_report_named(
    src: &impl views::DataSource,
    market: Market,
    base: Option<&str>,
    portfolio_name: Option<&str>,
) -> Result<PortfolioReport> {
    let portfolio = match portfolio_name {
        None => src.portfolio().map_err(|e| e.context(views::LocalData))?,
        Some(name) => {
            let norm = pf::normalize_name(Some(name))?;
            match norm {
                None => src.portfolio().map_err(|e| e.context(views::LocalData))?,
                Some(n) => {
                    Portfolio::load_named(Some(&n)).map_err(|e| e.context(views::LocalData))?
                }
            }
        }
    };
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let mut val = value_holdings(src, &symbols, market, base).await?;
    let pending = crate::corp_actions::pending_splits_live(&portfolio).await;
    let portfolio = pending.adjusted(&portfolio);
    if !cfg!(test) {
        val.load_history(&portfolio).await;
    }
    let mut report = portfolio_report(&portfolio, &val, chrono::Local::now().date_naive());
    report.warnings = pending.warnings();
    Ok(report)
}

/// Said once when prices cannot be fetched.
const OFFLINE_NOTE: &str = "Offline: prices unavailable; showing cost basis only.";

/// Prices for `symbols`. Without a client, or when the quote request fails
/// (no network, Yahoo blocked), an empty valuation: every holding is
/// unpriced but still listed. The flag says whether prices are live.
pub(crate) async fn value_or_offline(
    src: Option<&impl views::DataSource>,
    symbols: &[String],
    market: Market,
    base: Option<&str>,
) -> (Valuation, bool) {
    if let Some(src) = src
        && let Ok(val) = value_holdings(src, symbols, market, base).await
    {
        return (val, true);
    }
    (Valuation::new(Vec::new(), symbols, market, base), false)
}

/// `portfolio` without prices: shares, average cost and cost basis from the
/// local file, per currency.
fn print_offline_portfolio(portfolio: &Portfolio) {
    print_header(&portfolio_title(t(Msg::YourPortfolio)));
    println!("  {} {}\n", "!".yellow(), OFFLINE_NOTE);
    let sw = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.len() + 1)
        .max()
        .unwrap_or(0)
        .max(12);
    println!(
        "  {} {} {} {} {}",
        pad_end(t(Msg::Symbol), sw).bold(),
        pad_start(t(Msg::Shares), 8).bold(),
        pad_start(t(Msg::AvgCost), 10).bold(),
        pad_start(t(Msg::Price), 10).bold(),
        pad_start("Cost basis", 14).bold(),
    );
    println!("  {}", "─".repeat(48 + sw).dimmed());
    let mut by_currency: std::collections::BTreeMap<String, f64> = Default::default();
    let mut any_estimated = false;
    for h in &portfolio.holdings {
        let cur = pf::symbol_currency(&h.symbol);
        let mut label = h.symbol.clone();
        if h.has_estimated_dates() {
            label.push('*');
            any_estimated = true;
        }
        let cost = h.cost_basis();
        *by_currency
            .entry(cur.unwrap_or("?").to_string())
            .or_default() += cost;
        println!(
            "  {:<sw$} {:>8.2} {:>10} {:>10} {:>14}",
            label.cyan(),
            h.shares,
            format_price(h.avg_cost, cur),
            "N/A",
            format_price(cost, cur),
        );
    }
    println!("  {}", "─".repeat(48 + sw).dimmed());
    for (cur, total) in &by_currency {
        let cur = (cur != "?").then_some(cur.as_str());
        println!(
            "  {} {:>8} {:>10} {:>10} {:>14}",
            pad_end(t(Msg::Total), sw).bold(),
            "",
            "",
            "",
            format_price(*total, cur).bold(),
        );
    }
    if any_estimated {
        println!("  {}", ESTIMATED_DATE_HINT.dimmed());
    }
    println!();
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
            let symbols: Vec<String> = portfolio
                .holdings
                .iter()
                .map(|h| h.symbol.clone())
                .collect();
            let client = YahooClient::new().await.ok();
            let (mut val, live) = value_or_offline(client.as_ref(), &symbols, market, base).await;
            if !live {
                eprintln!("  {OFFLINE_NOTE}");
            }
            let pending = match client.as_ref().filter(|_| live) {
                Some(c) => crate::corp_actions::pending_splits(c, &portfolio).await,
                None => Default::default(),
            };
            let portfolio = pending.adjusted(&portfolio);
            if live {
                val.load_history(&portfolio).await;
            }
            let mut report = portfolio_report(&portfolio, &val, chrono::Local::now().date_naive());
            report.warnings = pending.warnings();
            report
        };
        return match fmt {
            Format::Csv => output::print_csv(&report.holdings),
            _ => output::print_json(&report),
        };
    }
    let loaded = Portfolio::load()?;
    if loaded.holdings.is_empty() {
        println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
        return Ok(());
    }

    let symbols: Vec<String> = loaded.holdings.iter().map(|h| h.symbol.clone()).collect();
    let client = YahooClient::new().await.ok();
    let (mut val, live) = value_or_offline(client.as_ref(), &symbols, market, base).await;
    let Some(client) = client.filter(|_| live) else {
        print_offline_portfolio(&loaded);
        return Ok(());
    };
    let today = chrono::Local::now().date_naive();
    // A split or bonus Yahoo reports but the lots have not taken yet would
    // show as a fake loss: value a copy with it applied, and say so.
    let pending = crate::corp_actions::pending_splits(&client, &loaded).await;
    let portfolio = pending.adjusted(&loaded);
    // Rates for closed positions and dividends in currencies no open
    // holding uses, before anything is totalled; then each day's rate.
    let mut history_currencies: Vec<String> = portfolio
        .symbols_with_history()
        .iter()
        .filter_map(|sym| val.currency(sym))
        .chain(
            portfolio
                .dividends
                .iter()
                .filter_map(|d| d.currency.clone()),
        )
        .collect();
    history_currencies.sort();
    history_currencies.dedup();
    val.add_rates(&client, &history_currencies).await;
    val.load_history(&portfolio).await;

    let base_currency = val.base().to_string();
    let base = Some(base_currency.as_str());
    let any_foreign = portfolio
        .holdings
        .iter()
        .any(|h| val.currency(&h.symbol).is_some_and(|c| c != base_currency));

    print_header(&portfolio_title(t(Msg::YourPortfolio)));
    // Wide enough for `MF:<ISIN>` (and the `*` of estimated dates).
    let sw = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.len() + 1)
        .max()
        .unwrap_or(0)
        .max(12);
    let fx_w = if any_foreign { 12 } else { 0 };
    let rule = "─".repeat(81 + 13 + sw + fx_w);
    let actions = super::actions::quick_actions(&portfolio, &val);
    let fx_head = if any_foreign {
        format!(" {}", pad_start("FX effect", 11).bold())
    } else {
        String::new()
    };
    // Width-aware padding: translated labels (e.g. Hindi) aren't one column per char.
    println!(
        "  {} {} {} {} {} {} {}{} {} {} {}",
        pad_end(t(Msg::Symbol), sw).bold(),
        pad_start(t(Msg::Shares), 8).bold(),
        pad_start(t(Msg::AvgCost), 10).bold(),
        pad_start(t(Msg::Price), 10).bold(),
        pad_start(t(Msg::Value), 12).bold(),
        pad_start(t(Msg::PnL), 11).bold(),
        pad_start(t(Msg::PnLPct), 8).bold(),
        fx_head,
        pad_start("XIRR", 8).bold(),
        pad_start("Cap", 6).bold(),
        pad_end("Action", 12).bold(),
    );
    println!("  {}", rule.dimmed());

    let mut any_estimated = false;
    for h in &portfolio.holdings {
        let cur = val.currency(&h.symbol);
        let cur = cur.as_deref();
        let mut label = h.symbol.clone();
        if !pending.for_symbol(&h.symbol).is_empty() {
            label.push('†');
        }
        if h.has_estimated_dates() {
            label.push('*');
            any_estimated = true;
        }
        let fx_blank = if any_foreign {
            format!(" {:>11}", "—")
        } else {
            String::new()
        };
        // No live price (delisted, renamed, or a bad symbol): show N/A and
        // leave it out of the totals rather than valuing it at 0.
        let Some(price) = val.price(&h.symbol) else {
            println!(
                "  {:<sw$} {:>8.2} {:>10} {:>10} {:>12} {:>11} {:>8}{} {:>8} {:>6}",
                label.yellow(),
                h.shares,
                format_price(h.avg_cost, cur),
                "N/A",
                "N/A",
                "—",
                "—",
                fx_blank,
                "—",
                "",
            );
            continue;
        };
        let value = h.shares * price;
        let cost = h.cost_basis();
        let pnl = value - cost;
        let pnl_pct = if cost > 0.0 { pnl / cost * 100.0 } else { 0.0 };
        // An annual return from guessed purchase dates would be made up.
        let xirr = if h.has_estimated_dates() {
            None
        } else {
            pf::xirr(&portfolio.cash_flows(&h.symbol), value, today)
        };
        let fx_cell = if !any_foreign {
            String::new()
        } else if cur == base {
            fx_blank.clone()
        } else {
            match (
                val.holding_cost_base(h),
                val.to_base(&h.symbol, h.cost_basis()),
            ) {
                (Some((hist, true)), Some(now)) => {
                    format!(" {:>11}", signed_money(now - hist, base))
                }
                _ => fx_blank.clone(),
            }
        };
        println!(
            "  {:<sw$} {:>8.2} {:>10} {:>10} {:>12} {:>11} {:>8}{} {:>8} {:>6} {}",
            label.cyan(),
            h.shares,
            format_price(h.avg_cost, cur),
            format_price(price, cur),
            format_price(value, cur),
            signed_money(pnl, cur),
            signed_pct(pnl_pct),
            fx_cell,
            xirr_cell(xirr),
            val.cap_tier(&h.symbol).badge().dimmed(),
            action_cell(actions.get(&h.symbol)),
        );
    }

    let totals = val.totals(&portfolio);
    let (portfolio_xirr, xirr_left_out) = portfolio_xirr(&portfolio, &val, &totals, today);
    let fx_total = if any_foreign {
        format!(" {:>11}", signed_money(totals.fx_effect(), base))
    } else {
        String::new()
    };
    println!("  {}", rule.dimmed());
    // The Avg Cost column has no meaning for a mix of holdings: the total
    // cost is printed below instead.
    println!(
        "  {} {:>8} {:>10} {:>10} {:>12} {:>11} {:>8}{} {:>8} {:>6}",
        pad_end(t(Msg::Total), sw).bold(),
        "",
        "",
        "",
        format_price(totals.value, base).bold(),
        signed_money(totals.value - totals.cost, base),
        signed_pct(totals.pnl_pct()),
        fx_total,
        xirr_cell(portfolio_xirr),
        "",
    );
    print_kv("Cost basis", &format_price(totals.cost, base));
    if !actions.is_empty() {
        println!(
            "  {}",
            "Action: quote-based rules (methodology 1). `stockwise portfolio review` adds price history, look-through and tax notes."
                .dimmed()
        );
    }
    if let Some(note) = val.fx_note() {
        println!("  {}", note.dimmed());
    }
    if any_foreign {
        println!(
            "  {}",
            format!(
                "Cost in {} at each purchase date's exchange rate. FX effect: what exchange-rate moves since then added (P&L and P&L % on the right of the total include it).",
                base_currency
            )
            .dimmed()
        );
    }

    print_total_return(&client, &portfolio, &val, &totals, today).await;

    let prof = crate::config::effective_profile();
    if prof.mode == crate::config::ProfileMode::Ultra && totals.value > 0.0 {
        let heat_per_pos = prof.risk_per_trade;
        let total_heat = (portfolio.holdings.len() as f64) * heat_per_pos;
        print_kv(
            "Portfolio Heat (1R/holding)",
            &format!(
                "{:.1}% ({} holdings × {:.2}% risk_per_trade = {})",
                total_heat,
                portfolio.holdings.len(),
                heat_per_pos,
                format_price(totals.value * total_heat / 100.0, base)
            ),
        );
    }
    println!();
    pending.print();
    totals.print_exclusions();
    val.print_price_notes();
    let regular_funds = portfolio
        .holdings
        .iter()
        .filter(|h| {
            val.mark(&h.symbol)
                .and_then(|m| m.name.as_deref())
                .is_some_and(|n| {
                    let n = n.to_lowercase();
                    n.contains("regular plan") && !n.contains("direct")
                })
        })
        .count();
    if regular_funds > 0 {
        println!(
            "  {} {} Regular-plan fund(s) held: `stockwise mf bleed` shows the Direct-plan TER gap and the units that can switch.",
            "·".dimmed(),
            regular_funds
        );
    }
    if !xirr_left_out.is_empty() {
        println!(
            "  {} No exchange rate for {} — left out of realized P&L, dividends and XIRR.",
            "!".yellow(),
            xirr_left_out.join(", ")
        );
    }
    if any_estimated {
        println!("  {}", ESTIMATED_DATE_HINT.dimmed());
        println!(
            "  {}",
            "XIRR is not shown for holdings with an unknown purchase date.".dimmed()
        );
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
    print_hint(
        "By market, sector and tax term, with risk and observations: stockwise portfolio analyze",
    );
    Ok(())
}

/// Money-weighted return across everything ever held (closed positions in
/// any currency too), each flow at its own date's exchange rate. Open
/// holdings left out of the totals are left out here as well. Returns the
/// rate and the symbols that had no exchange rate.
pub(crate) fn portfolio_xirr(
    portfolio: &Portfolio,
    val: &Valuation,
    totals: &Totals,
    today: chrono::NaiveDate,
) -> (Option<f64>, Vec<String>) {
    let excluded: Vec<&String> = totals.unpriced.iter().chain(&totals.unconverted).collect();
    let mut flows = Vec::new();
    let mut left_out = Vec::new();
    for sym in portfolio.symbols_with_history() {
        if excluded.contains(&&sym) {
            continue;
        }
        match val.flows_base(portfolio, &sym) {
            Some(f) => flows.extend(f),
            None => left_out.push(sym),
        }
    }
    flows.sort_by_key(|f| f.0);
    (pf::xirr(&flows, totals.value, today), left_out)
}

/// The first day of the financial year holding `today`: 1 April in India
/// (an INR base), else 1 January.
pub(crate) fn fy_start(today: chrono::NaiveDate, base: &str) -> chrono::NaiveDate {
    use chrono::Datelike;
    let ymd = |y, m| chrono::NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(today);
    if base.eq_ignore_ascii_case("INR") {
        if today.month() >= 4 {
            ymd(today.year(), 4)
        } else {
            ymd(today.year() - 1, 4)
        }
    } else {
        ymd(today.year(), 1)
    }
}

/// "FY 2026-27" for an INR base, "2026" otherwise.
fn fy_label(start: chrono::NaiveDate, base: &str) -> String {
    use chrono::Datelike;
    if base.eq_ignore_ascii_case("INR") {
        format!("FY {}-{:02}", start.year(), (start.year() + 1) % 100)
    } else {
        start.year().to_string()
    }
}

/// Realized P&L and dividends in the base currency, all time and since
/// `since`, each at its own date's exchange rate.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub(crate) struct Income {
    pub realized: f64,
    pub realized_since: f64,
    pub sales: usize,
    pub sales_since: usize,
    pub dividends: f64,
    pub dividends_since: f64,
    pub charges: f64,
}

pub(crate) fn income_since(
    portfolio: &Portfolio,
    val: &Valuation,
    since: chrono::NaiveDate,
) -> Income {
    let mut out = Income::default();
    let after = |d: &str| pf::parse_date(d).is_ok_and(|d| d >= since);
    for s in &portfolio.realized {
        let Some(g) = val.sale_gain_base(s) else {
            continue;
        };
        out.realized += g;
        out.sales += 1;
        if s.charges() > 0.0
            && let Some(c) = val.to_base(&s.symbol, s.charges())
        {
            out.charges += c;
        }
        if after(&s.sell_date) {
            out.realized_since += g;
            out.sales_since += 1;
        }
    }
    for d in &portfolio.dividends {
        let Some(a) = val.dividend_base(d) else {
            continue;
        };
        out.dividends += a;
        if after(&d.ex_date) {
            out.dividends_since += a;
        }
    }
    out
}

/// How much the holdings now held moved over a period, at today's rates.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PeriodChange {
    pub label: &'static str,
    pub change: f64,
    pub pct: f64,
    /// Holdings with a price at the period's start, of those priced now.
    pub covered: usize,
    pub of: usize,
}

/// Change of each holding's value since a past close, at today's share
/// count and exchange rate. `then(symbol)` is the holding's price at the
/// period start.
pub(crate) fn period_change(
    label: &'static str,
    portfolio: &Portfolio,
    val: &Valuation,
    then: impl Fn(&str) -> Option<f64>,
) -> Option<PeriodChange> {
    let (mut change, mut before, mut covered, mut of) = (0.0, 0.0, 0, 0);
    for h in &portfolio.holdings {
        let Some(now) = val.price(&h.symbol) else {
            continue;
        };
        of += 1;
        let Some(past) = then(&h.symbol).filter(|p| *p > 0.0) else {
            continue;
        };
        let (Some(v_now), Some(v_then)) = (
            val.to_base(&h.symbol, h.shares * now),
            val.to_base(&h.symbol, h.shares * past),
        ) else {
            continue;
        };
        change += v_now - v_then;
        before += v_then;
        covered += 1;
    }
    (covered > 0 && before > 0.0).then(|| PeriodChange {
        label,
        change,
        pct: change / before * 100.0,
        covered,
        of,
    })
}

/// The close on or before `date` in `closes` (oldest first).
fn close_on_or_before(closes: &[(chrono::NaiveDate, f64)], date: chrono::NaiveDate) -> Option<f64> {
    let idx = closes.partition_point(|(d, _)| *d <= date);
    closes.get(idx.checked_sub(1)?).map(|c| c.1)
}

/// Unrealized, realized (this financial year and all time) and dividends,
/// then how the current holdings moved over a day, a week, a month and
/// the year to date.
async fn print_total_return(
    client: &YahooClient,
    portfolio: &Portfolio,
    val: &Valuation,
    totals: &Totals,
    today: chrono::NaiveDate,
) {
    use futures_util::StreamExt;
    let base_currency = val.base().to_string();
    let base = Some(base_currency.as_str());
    let since = fy_start(today, &base_currency);
    let fy = fy_label(since, &base_currency);
    let inc = income_since(portfolio, val, since);
    let unrealized = totals.value - totals.cost;
    print_section(&format!("Total return ({})", base_currency));
    print_kv("Unrealized", &signed_money(unrealized, base));
    if inc.sales > 0 {
        print_kv(
            &format!("Realized, {} ({} sale lots)", fy, inc.sales_since),
            &signed_money(inc.realized_since, base),
        );
        print_kv(
            &format!("Realized, all time ({} sale lots)", inc.sales),
            &signed_money(inc.realized, base),
        );
        if inc.charges > 0.0 {
            print_kv(
                "  of which charges",
                &format!("-{}", format_price(inc.charges, base)),
            );
        }
    }
    if !portfolio.dividends.is_empty() {
        print_kv(
            &format!("Dividends, {}", fy),
            &format_price(inc.dividends_since, base),
        );
        print_kv("Dividends, all time", &format_price(inc.dividends, base));
    }
    print_kv(
        "Total, all time",
        &signed_money(unrealized + inc.realized + inc.dividends, base),
    );

    // Past closes of the holdings priced by Yahoo (funds and SGBs have none).
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .filter(|h| val.price(&h.symbol).is_some() && val.mark(&h.symbol).is_none())
        .map(|h| h.symbol.clone())
        .collect();
    let histories: Vec<(String, Vec<(chrono::NaiveDate, f64)>)> =
        futures_util::stream::iter(symbols)
            .map(|sym| async move {
                let closes = client
                    .get_chart(&sym, "1y", "1d")
                    .await
                    .map(|c| c.dated_closes())
                    .unwrap_or_default();
                (sym, closes)
            })
            .buffer_unordered(6)
            .collect()
            .await;
    let past = |sym: &str, date: chrono::NaiveDate| {
        histories
            .iter()
            .find(|(s, _)| s == sym)
            .and_then(|(_, c)| close_on_or_before(c, date))
    };
    let ytd_start = {
        use chrono::Datelike;
        chrono::NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today)
            - chrono::Duration::days(1)
    };
    let changes: Vec<PeriodChange> = [
        period_change("1 day", portfolio, val, |s| {
            val.quote(s).and_then(|q| q.regular_market_previous_close)
        }),
        period_change("1 week", portfolio, val, |s| {
            past(s, today - chrono::Duration::days(7))
        }),
        period_change("1 month", portfolio, val, |s| {
            past(s, today - chrono::Months::new(1))
        }),
        period_change("Year to date", portfolio, val, |s| past(s, ytd_start)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !changes.is_empty() {
        let cells: Vec<String> = changes
            .iter()
            .map(|c| {
                let cover = if c.covered < c.of {
                    format!(" [{}/{}]", c.covered, c.of)
                } else {
                    String::new()
                };
                format!(
                    "{} {} ({}){}",
                    c.label,
                    signed_money(c.change, base),
                    signed_pct(c.pct),
                    cover
                )
            })
            .collect();
        print_kv("Change of current holdings", &cells.join("  "));
        println!(
            "  {}",
            "At today's share count and exchange rate; [n/m]: holdings with a price at the start."
                .dimmed()
        );
    }
}

/// A price paid in `paid` expressed in the holding's quote unit
/// (`quote`, pence for `GBp`), given `rate` from the Yahoo pair that
/// [`fx_pair`] names for (`paid`, the quote's major currency); `inverted`
/// as `fx_pair` returns it.
pub(crate) fn convert_paid(cost: f64, rate: f64, inverted: bool, quote: &str) -> f64 {
    let major = if inverted { cost / rate } else { cost * rate };
    match market::minor_unit(quote) {
        Some((_, per)) => major * per,
        None => major,
    }
}

/// The close of `pair` on `date`, or on the nearest trading day before it
/// (up to ten days back), and that day.
async fn rate_on(
    client: &YahooClient,
    pair: &str,
    date: chrono::NaiveDate,
) -> Result<(f64, chrono::NaiveDate)> {
    let end = date
        .and_hms_opt(23, 59, 59)
        .map(|t| t.and_utc().timestamp())
        .unwrap_or_default();
    let start = end - 12 * 86_400;
    let chart = client.get_chart_between(pair, start, end, "1d").await?;
    let stamps = chart.timestamp.clone().unwrap_or_default();
    let closes = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.clone())
        .unwrap_or_default();
    stamps
        .iter()
        .zip(closes)
        .filter_map(|(t, c)| {
            let day = chrono::DateTime::from_timestamp(*t, 0)?.date_naive();
            Some((c?, day)).filter(|(c, d)| *c > 0.0 && *d <= date)
        })
        .next_back()
        .ok_or_else(|| anyhow::anyhow!("No {} rate on or before {}", pair, date))
}

pub async fn cmd_portfolio_add(
    symbol: &str,
    shares: f64,
    cost: f64,
    date: Option<&str>,
    force: bool,
    paid_in: Option<&str>,
    charges: Option<crate::trade_charges::ChargesArg>,
    market: Market,
) -> Result<()> {
    use crate::trade_charges::{self as tc, ChargesArg};
    if !(shares > 0.0 && shares.is_finite()) {
        bail!("Number of shares must be positive");
    }
    if let Some(ChargesArg::Amount(c)) = charges
        && !(c >= 0.0 && c.is_finite())
    {
        bail!("Charges can't be negative");
    }
    if !(cost >= 0.0 && cost.is_finite()) {
        bail!("Cost per share can't be negative");
    }
    let dated = date.is_some();
    let date = match date {
        Some(d) => pf::parse_date(d)?.to_string(),
        None => pf::today(),
    };
    // A purchase of a merged-away ticker (HDFC, MINDTREE) before its merger
    // is recorded in the acquirer's shares, at the same cost and date.
    let (mut symbol, mut shares, mut cost) = (symbol.to_string(), shares, cost);
    let mut units_as_of = None;
    if market == Market::In
        && let Some(swap) = pf::merger_swap(&symbol, Some(&date), shares, cost)
    {
        println!(
            "  {} {} was merged into {}: recorded as {} {} at {} each ({} for {}, same total cost)",
            "·".dimmed(),
            swap.merger.old,
            swap.merger.new,
            swap.qty,
            swap.merger.new,
            format_price(
                swap.price,
                pf::symbol_currency(&market::resolve_symbol(&swap.symbol, market))
            ),
            swap.merger.new_shares,
            swap.merger.old_shares
        );
        (symbol, shares, cost) = (swap.symbol, swap.qty, swap.price);
        // The quantity is the acquirer's share count as of the merger, so
        // splits dated up to then must not be applied to the lot again.
        units_as_of = Some(swap.merger.through.to_string());
    }
    let resolved = market::resolve_symbol(&symbol, market);
    check_symbols_before_add(std::slice::from_ref(&resolved), market, force).await?;
    // `--charges auto`: an Indian delivery buy's statutory charges, in rupees.
    let charges: Option<f64> = match charges {
        None => None,
        Some(ChargesArg::Amount(c)) => Some(c),
        Some(ChargesArg::Auto) => {
            if !tc::auto_applies(&resolved) || paid_in.is_some() {
                bail!(
                    "`--charges auto` estimates Indian (NSE/BSE) delivery charges in rupees; \
                     for {} give the total charged instead (--charges 12.50)",
                    resolved
                );
            }
            let est = tc::estimate_delivery(false, shares * cost, pf::parse_date(&date)?);
            println!(
                "  {} Estimated charges {}: {} (brokerage not included)",
                "·".dimmed(),
                format_price(est.total(), Some("INR")),
                tc::breakdown(&est)
            );
            Some(est.total())
        }
    };
    // A price paid in another currency (a US share bought with rupees):
    // convert it at that day's rate, so the cost is in the holding's own.
    // The currency the cost ends up in: the instrument's quoted currency
    // when `--currency` looked it up, else the one its suffix implies.
    let mut cost_currency: Option<String> = None;
    // What was paid in the other currency, per unit of the holding's: kept
    // on the lot so its cost in that currency is exact, not re-converted.
    let mut paid_fx: Option<(String, f64)> = None;
    let mut charges = charges;
    if let Some(paid) = paid_in {
        // The currency the instrument is quoted in, from its quote (a few
        // London listings trade in USD), else from its suffix.
        let client = YahooClient::new().await?;
        let quoted = client
            .get_quote(&[resolved.as_str()])
            .await
            .ok()
            .and_then(|q| q.into_iter().next())
            .and_then(|q| q.currency);
        // No guessing from the suffix: a USD-quoted London listing would be
        // stored in pence. Without the quote's currency, refuse.
        let Some(quote) = quoted else {
            bail!(
                "Can't tell which currency {} is quoted in (no quote from Yahoo Finance); \
                 enter the price in that currency instead of using --currency",
                resolved
            );
        };
        cost_currency = Some(quote.clone());
        let quote = quote.as_str();
        let (_, major) = market::to_major(1.0, quote);
        let paid_cost = cost;
        if paid.eq_ignore_ascii_case(&major) {
            cost = convert_paid(cost, 1.0, false, quote);
            charges = charges.map(|c| convert_paid(c, 1.0, false, quote));
        } else {
            let (pair, inverted) = fx_pair(paid, &major);
            let day = pf::parse_date(&date)?;
            let (rate, on) = rate_on(&client, &pair, day).await?;
            cost = convert_paid(cost, rate, inverted, quote);
            let (major_cost, _) = market::to_major(cost, quote);
            if major_cost > 0.0 {
                paid_fx = Some((paid.to_uppercase(), paid_cost / major_cost));
            }
            // Charges are kept in the holding's quote unit, like its price.
            charges = charges.map(|c| convert_paid(c, rate, inverted, quote));
            let per = if inverted { rate } else { 1.0 / rate };
            println!(
                "  {} {} {:.2} a share is {} at {:.4} {} per {} ({} close of {})",
                "·".dimmed(),
                paid.to_uppercase(),
                paid_cost,
                format_price(cost, Some(quote)),
                per,
                paid.to_uppercase(),
                major,
                on,
                pair
            );
        }
    }
    let holding = Portfolio::update(|p| {
        let mut lot = Lot::new(shares, cost, &date, "manual");
        lot.units_as_of = units_as_of;
        if let Some((cur, rate)) = &paid_fx {
            lot.fx_currency = Some(cur.clone());
            lot.fx_rate = Some(*rate);
        }
        lot.charges = charges.filter(|c| *c > 0.0);
        p.add_lot(&resolved, lot);
        Ok(p.holding(&resolved).cloned())
    })?;
    let cur = cost_currency
        .as_deref()
        .or_else(|| pf::symbol_currency(&resolved));
    let units = offmarket::unit_word(&resolved);
    let where_ = if offmarket::is_off_market(&resolved) {
        String::new()
    } else {
        format!(
            "; {}{}",
            market::market_of_symbol(&resolved, market).label(),
            cur.map_or(String::new(), |c| format!(", {}", c))
        )
    };
    println!(
        "  {} Added {} {} of {} at {} (bought {}{})",
        "✓".green(),
        shares,
        units,
        resolved,
        format_price(cost, cur),
        date,
        where_
    );
    if let Some(h) = &holding
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

pub async fn cmd_portfolio_remove(symbol: &str, yes: bool, market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    let resolved = held_symbol(&portfolio, symbol, market);
    let lots = portfolio.holding(&resolved).map_or(0, |h| h.lots.len());
    if lots > 1 && !yes {
        use std::io::{BufRead, IsTerminal, Write};
        if !std::io::stdin().is_terminal() {
            bail!(
                "{} has {} lots; pass --yes to remove them all (or `portfolio lots {} --lot N --delete` for one)",
                resolved,
                lots,
                resolved
            );
        }
        print!(
            "  Remove {} and all {} of its lots, without recording a sale? [y/N] ",
            resolved, lots
        );
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !answer.trim().to_ascii_lowercase().starts_with('y') {
            println!("  Nothing removed.");
            return Ok(());
        }
    }
    Portfolio::update(|p| {
        // Another command may have changed the holding while the question
        // was open; remove only what was confirmed.
        let now = p.holding(&resolved).map_or(0, |h| h.lots.len());
        if !yes && now != lots && now > 1 {
            bail!(
                "{} changed while you were asked ({} lots now, {} before); nothing removed, run the command again",
                resolved,
                now,
                lots
            );
        }
        if !p.remove(&resolved) {
            bail!("{} is not in your portfolio", resolved);
        }
        Ok(())
    })?;
    println!("  {} Removed {} from portfolio", "✓".green(), resolved);
    println!("  {}", RESTORE_HINT.dimmed());
    println!(
        "  {}",
        "To record a sale (and its P&L for `tax`), use `stockwise portfolio sell` instead."
            .dimmed()
    );
    Ok(())
}

/// The hint printed after a sale; names the symbol so the command can be
/// copied and run as-is.
fn sell_history_hint(symbol: &str) -> String {
    format!(
        "Kept for `stockwise tax`; `stockwise portfolio lots {}` shows the history.",
        symbol
    )
}

pub async fn cmd_portfolio_sell(
    symbol: &str,
    shares: f64,
    price: Option<f64>,
    date: Option<&str>,
    charges: Option<crate::trade_charges::ChargesArg>,
    market: Market,
) -> Result<()> {
    use crate::trade_charges::{self as tc, ChargesArg};
    if let Some(ChargesArg::Amount(c)) = charges
        && !(c >= 0.0 && c.is_finite())
    {
        bail!("Charges can't be negative");
    }
    let portfolio = Portfolio::load()?;
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
    // `--charges auto`: an Indian delivery sell's statutory charges.
    let charges: Option<f64> = match charges {
        None => None,
        Some(ChargesArg::Amount(c)) => Some(c),
        Some(ChargesArg::Auto) => {
            if !tc::auto_applies(&resolved) {
                bail!(
                    "`--charges auto` estimates Indian (NSE/BSE) delivery charges; \
                     for {} give the total charged instead (--charges 12.50)",
                    resolved
                );
            }
            let est = tc::estimate_delivery(true, shares * price, pf::parse_date(&date)?);
            println!(
                "  {} Estimated charges {}: {} (brokerage not included)",
                "·".dimmed(),
                format_price(est.total(), Some("INR")),
                tc::breakdown(&est)
            );
            Some(est.total())
        }
    };
    // The checks above ran on the portfolio as loaded; sell from the file as
    // it is now, so a change made meanwhile is kept (and a sale that no
    // longer fits is refused).
    let sales = Portfolio::update(|p| {
        let n = p.realized.len();
        let mut sales = p.sell_fifo(&resolved, shares, price, &date, "manual")?;
        // The sale's own charges, shared by the lots it sold from.
        if let Some(c) = charges.filter(|c| *c > 0.0) {
            for (s, saved) in sales.iter_mut().zip(p.realized[n..].iter_mut()) {
                let part = c * s.qty / shares;
                s.add_charges(part);
                saved.add_charges(part);
            }
        }
        Ok(sales)
    })?;

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
    println!("  {}", sell_history_hint(&resolved).dimmed());
    Ok(())
}

/// Printed after a change made by hand, so a mistake is one command away
/// from undone.
const RESTORE_HINT: &str =
    "The previous version is kept: `stockwise portfolio restore` lists the backups.";

/// A correction asked for with `portfolio lots`.
#[derive(Debug, Default)]
pub struct LotChange {
    pub lot: Option<usize>,
    pub date: Option<String>,
    pub qty: Option<f64>,
    pub price: Option<f64>,
    pub delete: bool,
    pub undo_sale: Option<usize>,
}

/// One saved version in `portfolio restore`'s list.
#[derive(Debug, serde::Serialize)]
pub struct BackupRow {
    pub number: usize,
    /// Local time the version was saved, `YYYY-MM-DD HH:MM`.
    pub saved: String,
    /// `None` when the file can't be read as a portfolio.
    pub holdings: Option<usize>,
    pub sales: Option<usize>,
}

impl output::CsvRow for BackupRow {
    const COLUMNS: &'static [&'static str] = &["number", "saved", "holdings", "sales"];
}

fn backup_rows(path: &std::path::Path) -> Vec<BackupRow> {
    pf::backups_of(path)
        .into_iter()
        .map(|(number, file)| {
            let saved = std::fs::metadata(&file)
                .and_then(|m| m.modified())
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let p = Portfolio::read_only(&file).ok();
            BackupRow {
                number,
                saved,
                holdings: p.as_ref().map(|p| p.holdings.len()),
                sales: p.as_ref().map(|p| p.realized.len()),
            }
        })
        .collect()
}

/// `portfolio restore`: list the backups, or bring backup `n` back.
pub fn cmd_portfolio_restore(n: Option<usize>, format: Format) -> Result<()> {
    let path = crate::store::path(&pf::file_name(pf::active()))?;
    let Some(n) = n else {
        let rows = backup_rows(&path);
        match format {
            Format::Json => return output::print_json(&rows),
            Format::Csv => return output::print_csv(&rows),
            Format::Table => {}
        }
        let backups = &rows;
        print_header(&portfolio_title("Backups"));
        if backups.is_empty() {
            println!(
                "  {}",
                "No backups yet: one is kept each time the portfolio changes.".dimmed()
            );
            println!();
            return Ok(());
        }
        println!(
            "  {:>3} {:<20} {:>8} {:>6}",
            "#".bold(),
            "Saved".bold(),
            "Holdings".bold(),
            "Sales".bold()
        );
        for r in backups {
            match (r.holdings, r.sales) {
                (Some(h), Some(s)) => {
                    println!("  {:>3} {:<20} {:>8} {:>6}", r.number, r.saved, h, s)
                }
                _ => println!("  {:>3} {:<20} {}", r.number, r.saved, "unreadable".red()),
            }
        }
        println!(
            "\n  {}",
            "`stockwise portfolio restore N` brings one back; the current version becomes backup 1."
                .dimmed()
        );
        println!();
        return Ok(());
    };
    let _lock = crate::store::lock(&path)?;
    let p = Portfolio::restore_backup(&path, n)?;
    println!(
        "  {} Restored backup #{} ({} holdings, {} sales)",
        "✓".green(),
        n,
        p.holdings.len(),
        p.realized.len()
    );
    println!(
        "  {}",
        "The version it replaced is now backup 1: `stockwise portfolio restore 1` undoes this."
            .dimmed()
    );
    Ok(())
}

/// Apply a [`LotChange`] to the saved portfolio and say what changed.
fn apply_lot_change(resolved: &str, change: &LotChange) -> Result<Option<Portfolio>> {
    let out = apply_lot_change_inner(resolved, change)?;
    if let Some(p) = &out
        && p.imported.iter().any(|t| t.symbol == resolved)
    {
        println!(
            "  {}",
            format!(
                "Note: {resolved} has trades kept from an imported tradebook. Importing a \
                 tradebook for it again rebuilds its lots and sales from the kept trades and \
                 replaces this correction. To keep it, re-import only with `--replace` and a \
                 file holding the complete corrected history."
            )
            .yellow()
        );
    }
    Ok(out)
}

fn apply_lot_change_inner(resolved: &str, change: &LotChange) -> Result<Option<Portfolio>> {
    if let Some(n) = change.undo_sale {
        let (p, sale) = Portfolio::update(|p| {
            let sale = p.undo_sale(resolved, n)?;
            Ok((p.clone(), sale))
        })?;
        println!(
            "  {} Undid sale #{} of {}: {} shares bought {} are back in your lots",
            "✓".green(),
            n,
            resolved,
            sale.qty,
            sale.buy_date
        );
        if sale.origin.is_none() {
            // Recorded before 0.9: the sale keeps no split history, so a split
            // after it can only be found online.
            println!(
                "  {}",
                format!(
                    "This sale was recorded by an older StockWise and does not say which splits \
                     its lot had. Run `stockwise portfolio actions {resolved} --apply` to bring \
                     the restored shares into today's share count."
                )
                .yellow()
            );
        }
        println!("  {}", RESTORE_HINT.dimmed());
        return Ok(Some(p));
    }
    let Some(n) = change.lot else {
        return Ok(None);
    };
    if change.delete {
        let (p, lot) = Portfolio::update(|p| {
            let lot = p.delete_lot(resolved, n)?;
            Ok((p.clone(), lot))
        })?;
        println!(
            "  {} Deleted lot #{} of {} ({} bought {}); no sale was recorded",
            "✓".green(),
            n,
            resolved,
            lot.qty,
            lot.date
        );
        println!("  {}", RESTORE_HINT.dimmed());
        return Ok(Some(p));
    }
    if change.date.is_none() && change.qty.is_none() && change.price.is_none() {
        bail!("--lot {n} needs --date, --qty, --price or --delete");
    }
    let date = change
        .date
        .as_deref()
        .map(|d| pf::parse_date(d).map(|d| d.to_string()))
        .transpose()?;
    let p = Portfolio::update(|p| {
        if change.qty.is_some() || change.price.is_some() {
            p.edit_lot(resolved, n, change.qty, change.price)?;
        }
        if let Some(d) = &date {
            p.set_lot_date(resolved, n, d)?;
        }
        Ok(p.clone())
    })?;
    let mut what = Vec::new();
    if let Some(d) = &date {
        what.push(format!("dated {d}"));
    }
    if let Some(q) = change.qty {
        what.push(format!("{q} shares"));
    }
    if let Some(pr) = change.price {
        what.push(format!("price {pr}"));
    }
    println!(
        "  {} Lot #{} of {} now {}",
        "✓".green(),
        n,
        resolved,
        what.join(", ")
    );
    if date.is_some() {
        println!(
            "  {}",
            "Lots are kept oldest first, so the lot numbers may have changed.".dimmed()
        );
    }
    println!("  {}", RESTORE_HINT.dimmed());
    Ok(Some(p))
}

pub async fn cmd_portfolio_lots(symbol: &str, change: LotChange, market: Market) -> Result<()> {
    let mut portfolio = Portfolio::load()?;
    let resolved = held_symbol(&portfolio, symbol, market);
    if let Some(p) = apply_lot_change(&resolved, &change)? {
        portfolio = p;
        if portfolio.holding(&resolved).is_none() && portfolio.sales_of(&resolved).is_empty() {
            // The change removed the last lot and there is no history left.
            println!();
            return Ok(());
        }
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
            if let Some(account) = l.account_key() {
                source.push_str(&format!(" [{account}]"));
            }
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
            "  {:>3} {:<12} {:>9} {:<12} {:>10} {:>10} {:>12} {:>5}",
            "#".bold(),
            "Sold".bold(),
            "Qty".bold(),
            "Bought".bold(),
            "Buy".bold(),
            "Sell".bold(),
            "Gain".bold(),
            "Term".bold(),
        );
        for (i, s) in sales.iter().enumerate() {
            let long = match (
                chrono::NaiveDate::parse_from_str(&s.buy_date, "%Y-%m-%d"),
                chrono::NaiveDate::parse_from_str(&s.sell_date, "%Y-%m-%d"),
            ) {
                (Ok(b), Ok(d)) => tax::is_long_term(b, d, 12),
                _ => false,
            };
            println!(
                "  {:>3} {:<12} {:>9.2} {:<12} {:>10} {:>10} {:>12} {:>5}",
                i + 1,
                s.sell_date,
                s.qty,
                s.buy_date,
                format_price(s.buy_price, cur),
                format_price(s.sell_price, cur),
                signed_money(s.gain(), cur),
                if s.is_intraday() {
                    "Intra"
                } else if long {
                    "LT"
                } else {
                    "ST"
                },
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
    // `portfolio` is a working copy for the report; `--apply` re-applies the
    // fetched actions to the saved file under a lock (see `plan`).
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
    let mut cal = crate::calendar::Calendar::new(&client);
    let today = pf::today();
    print_header(&portfolio_title("Corporate Actions"));

    let (mut split_lots, mut bonus_lots, mut new_divs, mut div_total) =
        (0usize, 0usize, 0usize, Vec::<(String, f64)>::new());
    // Indian symbols whose splits NSE could not classify.
    let mut nse_offline: Vec<String> = Vec::new();
    // Already applied or recorded by an earlier --apply: counted, not listed.
    let (mut done_splits, mut done_divs) = (0usize, 0usize);
    let mut plan = Vec::new();
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
        let (mut splits, divs, quote_currency) = match fetch_actions(&client, sym, &first).await {
            Ok(a) => a,
            Err(e) => {
                failed.push(format!("{}: {:#}", sym, e));
                continue;
            }
        };
        // Yahoo reports an Indian bonus as a split: NSE's filings tell them apart.
        let pending_any = portfolio.holding(sym).is_some_and(|h| {
            splits
                .iter()
                .any(|s| h.lots.iter().any(|l| s.pending_for(l)))
        });
        if pending_any && !mark_bonuses(&mut cal, sym, &mut splits).await {
            nse_offline.push(sym.clone());
        }
        let currency = quote_currency.or_else(|| pf::symbol_currency(sym).map(str::to_string));
        let cur = currency.as_deref();

        let mut lines = Vec::new();
        for split in &splits {
            let Some(h) = portfolio.holding(sym) else {
                continue;
            };
            let pending: Vec<&Lot> = h.lots.iter().filter(|l| split.pending_for(l)).collect();
            let applied = h.lots.iter().any(|l| split.applied_to(l));
            let bonus = split.bonus.as_deref().filter(|_| split.ratio() > 1.0);
            let what = match bonus {
                Some(_) => format!(
                    "Bonus {}:{} (ex-date {})",
                    split.numerator - split.denominator,
                    split.denominator,
                    split.date
                ),
                None => format!(
                    "Split/bonus {}:{} on {}",
                    split.numerator, split.denominator, split.date
                ),
            };
            if !pending.is_empty() {
                let detail: Vec<String> = match bonus {
                    Some(allotted) => {
                        bonus_lots += pending.len();
                        let qty: f64 = pending.iter().map(|l| l.qty).sum();
                        vec![format!(
                            "+{} bonus shares at {} cost, dated {}; older lots keep their cost",
                            qty * (split.ratio() - 1.0),
                            format_price(0.0, cur),
                            allotted
                        )]
                    }
                    None => {
                        split_lots += pending.len();
                        pending
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
                            .collect()
                    }
                };
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
            // Applied here too, so later symbols and dividends see the
            // adjusted lots; applied again to the saved file below.
            for split in &splits {
                portfolio.apply_split(sym, split);
            }
            for ev in &divs {
                portfolio.record_dividend(sym, ev, &splits, cur);
            }
            plan.push((sym.clone(), splits, divs, currency.clone()));
        }
    }

    cal.save();
    if failed.len() == targets.len() {
        bail!("Could not fetch corporate actions: {}", failed.join("; "));
    }
    let pending = split_lots + bonus_lots + new_divs;
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
        // The network work is done: apply it to the file as it is now. Both
        // steps are idempotent, so a lot already adjusted is skipped.
        Portfolio::update(|p| {
            for (sym, splits, divs, currency) in &plan {
                for split in splits {
                    p.apply_split(sym, split);
                }
                for ev in divs {
                    p.record_dividend(sym, ev, splits, currency.as_deref());
                }
            }
            Ok(())
        })?;
        let bonus = if bonus_lots > 0 {
            format!(", added bonus shares for {} lot(s)", bonus_lots)
        } else {
            String::new()
        };
        println!(
            "  {} Adjusted {} lot(s) for splits/bonuses{} and recorded {} dividend(s){}.",
            "✓".green().bold(),
            split_lots,
            bonus,
            new_divs,
            if totals.is_empty() {
                String::new()
            } else {
                format!(" ({})", totals.join(", "))
            }
        );
    } else {
        println!(
            "  {} lot(s) to adjust for splits/bonuses, {} lot(s) to receive bonus shares, {} dividend(s) to record. Nothing changed yet —",
            split_lots, bonus_lots, new_divs
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
            "Splits multiply quantity and divide price, keeping cost and purchase date.".dimmed()
        );
    }
    if bonus_lots > 0 {
        println!(
            "  {}",
            "Indian bonus issues (checked against NSE filings) are recorded as a new lot at zero \
             cost, dated on the allotment date (NSE's ex-date), as Indian tax treats them; the \
             older lots keep their cost and purchase date."
                .dimmed()
        );
    }
    if !nse_offline.is_empty() {
        println!(
            "  {} {}",
            "!".yellow(),
            nse_offline_note(&cal, &nse_offline).dimmed()
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

// ── Target Allocations & Rebalance ──

/// Set a target weight for a symbol or sector.
/// The key a target weight is saved under:
/// 1. a held symbol (exact, or its bare ticker) → that holding's symbol;
/// 2. a key with `.` or `:` (TCS.NS, MF:122639) → as given, upper-cased;
/// 3. a short upper-case ticker → resolved for `market`;
/// 4. anything else ("Technology", "Financial Services") → a sector key.
pub(crate) fn resolve_target_key(p: &Portfolio, key: &str, market: Market) -> String {
    if let Some(h) = p.holdings.iter().find(|h| {
        h.symbol.eq_ignore_ascii_case(key)
            || h.symbol
                .split_once('.')
                .is_some_and(|(bare, _)| bare.eq_ignore_ascii_case(key))
    }) {
        h.symbol.clone()
    } else if key.contains('.') || key.contains(':') {
        key.trim().to_uppercase()
    } else if key
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        && key.len() <= 10
    {
        market::resolve_symbol(key, market)
    } else {
        key.trim().to_string()
    }
}

pub fn cmd_portfolio_target_set(key: &str, percentage: f64, market: Market) -> Result<()> {
    if !(0.0..=100.0).contains(&percentage) {
        bail!(
            "Target percentage must be between 0 and 100, got {}",
            percentage
        );
    }
    let mut p = Portfolio::load()?;

    let resolved_key = resolve_target_key(&p, key, market);

    p = Portfolio::update(|p| {
        p.set_target(resolved_key.clone(), percentage);
        Ok(p.clone())
    })?;

    let total: f64 = p.targets.values().sum();
    println!(
        "\n  {} Target for {} set to {:.1}%",
        "✓".green(),
        resolved_key.cyan(),
        percentage
    );
    println!(
        "  Total saved targets: {:.1}% ({} target{})",
        total,
        p.targets.len(),
        if p.targets.len() == 1 { "" } else { "s" }
    );
    if total > 100.0 + 1e-6 {
        println!(
            "  {}",
            format!(
                "Note: Targets sum to {:.1}% (exceeds 100%). Weights will be normalized during rebalancing.",
                total
            )
            .yellow()
        );
    } else if total < 100.0 - 1e-6 {
        let remaining = 100.0 - total;
        println!(
            "  {}",
            format!(
                "Note: Remaining {:.1}% will be distributed among untargeted holdings during rebalancing.",
                remaining
            )
            .dimmed()
        );
    }
    println!();
    Ok(())
}

/// Show saved target allocations and drift tolerance.
pub fn cmd_portfolio_target_show(market: Market, json: bool) -> Result<()> {
    let p = Portfolio::load()?;
    let _ = market;
    if json {
        let targets_list: Vec<serde_json::Value> = p
            .targets
            .iter()
            .map(|(k, &v)| {
                serde_json::json!({
                    "key": k,
                    "target_pct": v,
                })
            })
            .collect();
        let total: f64 = p.targets.values().sum();
        let out = serde_json::json!({
            "targets": targets_list,
            "total_target_pct": total,
            "drift_tolerance_pct": p.drift_tolerance,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    print_header("Portfolio Target Allocations");
    if p.targets.is_empty() {
        println!(
            "  {}",
            "No target allocations saved. Rebalancing defaults to equal weight across all holdings."
                .dimmed()
        );
        println!(
            "\n  {} Set a target with: stockwise portfolio target set <SYMBOL|SECTOR> <PERCENT>",
            "Tip:".yellow()
        );
        println!();
        return Ok(());
    }

    let kw = p
        .targets
        .keys()
        .map(|k| k.len())
        .max()
        .unwrap_or(12)
        .max(12);
    println!();
    println!("  {:<kw$} {:>10}", "Target Key".bold(), "Weight%".bold());
    println!("  {}", "─".repeat(kw + 13).dimmed());

    for (k, &pct) in &p.targets {
        println!("  {:<kw$} {:>9.1}%", k.cyan(), pct);
    }
    println!("  {}", "─".repeat(kw + 13).dimmed());
    let total: f64 = p.targets.values().sum();
    println!("  {:<kw$} {:>9.1}%", "Total".bold(), total);
    println!();

    if let Some(drift) = p.drift_tolerance {
        println!(
            "  Drift Tolerance: ±{:.1}% (rebalancing ignores drifts within this band)",
            drift
        );
    } else {
        println!("  Drift Tolerance: Not set (default: 0.0%)");
    }
    println!();
    Ok(())
}

/// Remove a target weight by symbol or sector.
pub fn cmd_portfolio_target_remove(key: &str) -> Result<()> {
    if Portfolio::update(|p| Ok(p.remove_target(key)))? {
        println!("\n  {} Removed target for {}\n", "✓".green(), key.cyan());
    } else {
        println!("\n  {} No target found matching '{}'\n", "✗".red(), key);
    }
    Ok(())
}

/// Clear all saved target weights.
pub fn cmd_portfolio_target_clear() -> Result<()> {
    Portfolio::update(|p| {
        p.clear_targets();
        Ok(())
    })?;
    println!(
        "\n  {} Cleared all target allocations. Rebalancing will use equal weight.\n",
        "✓".green()
    );
    Ok(())
}

/// Set default drift tolerance band in percent.
pub fn cmd_portfolio_target_drift(percentage: f64) -> Result<()> {
    if !(0.0..=50.0).contains(&percentage) {
        bail!(
            "Drift tolerance must be between 0.0% and 50.0%, got {}",
            percentage
        );
    }
    Portfolio::update(|p| {
        p.set_drift_tolerance(Some(percentage));
        Ok(())
    })?;
    println!(
        "\n  {} Default drift tolerance set to ±{:.1}%\n",
        "✓".green(),
        percentage
    );
    Ok(())
}

/// Compute a rebalance plan using `src` (either live Yahoo or test fixtures).
pub(crate) async fn rebalance_plan(
    src: &impl views::DataSource,
    portfolio: &Portfolio,
    targets: &[(String, f64)],
    inflow: Option<f64>,
    drift: Option<f64>,
    market: Market,
) -> Result<(crate::rebalance::RebalancePlan, Valuation)> {
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = value_holdings(src, &symbols, market, None).await?;

    let mut holding_inputs = Vec::with_capacity(portfolio.holdings.len());
    for h in &portfolio.holdings {
        let Some(price) = val.price(&h.symbol) else {
            let why = val
                .off
                .notes
                .iter()
                .find(|n| n.symbols.contains(&h.symbol))
                .map(|n| format!(" ({})", n.text))
                .unwrap_or_default();
            bail!(
                "No live price for {} — fix or remove it before rebalancing{}",
                h.symbol,
                why
            );
        };
        let Some(price_base) = val.to_base(&h.symbol, price) else {
            bail!(
                "No exchange rate for {} — can't weigh it against your other holdings",
                h.symbol
            );
        };
        let value = h.shares * price_base;
        let sector = val.quote(&h.symbol).and_then(|q| q.sector.clone());
        let is_fund = crate::offmarket::is_fund(&h.symbol);
        holding_inputs.push(HoldingInput {
            symbol: h.symbol.clone(),
            shares: h.shares,
            value,
            price: price_base,
            sector,
            is_fund,
        });
    }

    let mut configured_targets = std::collections::BTreeMap::new();
    if !targets.is_empty() {
        for (k, pct) in targets {
            configured_targets.insert(k.clone(), *pct);
        }
    } else {
        configured_targets = portfolio.targets.clone();
    }

    let prof = crate::config::effective_profile();
    let drift_tolerance = drift
        .or(portfolio.drift_tolerance)
        .unwrap_or(prof.default_drift_pct);
    let mut plan = crate::rebalance::compute_rebalance_plan(
        &holding_inputs,
        &configured_targets,
        inflow,
        drift_tolerance,
        val.base(),
    )?;
    for it in &plan.items {
        let is_fund = holding_inputs
            .iter()
            .find(|h| h.symbol == it.symbol)
            .is_some_and(|h| h.is_fund);
        if !is_fund
            && (it.target_pct > prof.max_stock_weight + 1e-6
                || it.projected_pct > prof.max_stock_weight + 1e-6)
        {
            plan.warnings.push(format!(
                "Profile guardrail ({}): {} target/projected weight ({:.1}% / {:.1}%) exceeds configured max_stock_weight ({:.1}%).",
                prof.spec, it.symbol, it.target_pct, it.projected_pct, prof.max_stock_weight
            ));
        }
    }
    Ok((plan, val))
}

/// Rebalance portfolio towards target allocations or equal weight.
///
/// Supports:
/// - Target weights saved in portfolio or passed on CLI (`targets`)
/// - Inflow-only mode (`--inflow <cash>`) to invest fresh capital without selling
/// - Drift tolerance band (`--drift <pct>`) to suppress minor churn
/// - Machine-readable JSON output (`--json`)
pub async fn cmd_rebalance(
    targets: &[(String, f64)],
    inflow: Option<f64>,
    drift: Option<f64>,
    json: bool,
    market: Market,
) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        if json {
            let plan = crate::rebalance::RebalancePlan::empty(market_currency(market));
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
        }
        return Ok(());
    }

    let client = YahooClient::new().await?;
    let (plan, val) = rebalance_plan(&client, &portfolio, targets, inflow, drift, market).await?;
    let drift_tolerance = plan.drift_tolerance;

    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }

    let cur = Some(val.base());
    let csym = market::currency_symbol(cur);

    if let Some(cash) = inflow {
        print_header(&format!(
            "Portfolio Rebalance (Inflow: {}{:.2})",
            csym, cash
        ));
        print_kv("Current Value", &format!("{}{:.2}", csym, plan.total_value));
        print_kv(
            "Projected Value",
            &format!("{}{:.2}", csym, plan.projected_value),
        );
    } else {
        print_header("Portfolio Rebalance");
        print_kv("Total Value", &format!("{}{:.2}", csym, plan.total_value));
    }

    if drift_tolerance > 0.0 {
        print_kv("Drift Tolerance", &format!("±{:.1}%", drift_tolerance));
    }

    for warning in &plan.warnings {
        println!("  {} {}", "ℹ".cyan(), warning.dimmed());
    }

    let sw = plan
        .items
        .iter()
        .map(|it| it.symbol.len() + 1)
        .max()
        .unwrap_or(12)
        .max(12);

    println!();
    if inflow.is_some() {
        println!(
            "  {:<sw$} {:>10} {:>7} {:>7} {:>10} {:>12} {:>16}",
            "Symbol".bold(),
            "Current".bold(),
            "Now%".bold(),
            "Target%".bold(),
            "Projected%".bold(),
            "Allocation".bold(),
            "Action".bold()
        );
        println!("  {}", "─".repeat(68 + sw).dimmed());

        for it in &plan.items {
            let action_styled = match it.action_type {
                RebalanceActionType::Buy => it.action_display.green().to_string(),
                RebalanceActionType::Sell => it.action_display.red().to_string(),
                RebalanceActionType::Ok => it.action_display.dimmed().to_string(),
            };
            println!(
                "  {:<sw$} {:>10} {:>6.1}% {:>6.1}% {:>9.1}% {:>12} {:>16}",
                it.symbol.cyan(),
                format!("{}{:.2}", csym, it.current_value),
                it.current_pct,
                it.target_pct,
                it.projected_pct,
                format!("{}{:.2}", csym, it.diff_value),
                action_styled
            );
        }
    } else {
        println!(
            "  {:<sw$} {:>10} {:>8} {:>8} {:>16}",
            "Symbol".bold(),
            "Current".bold(),
            "Now%".bold(),
            "Target%".bold(),
            "Action".bold()
        );
        println!("  {}", "─".repeat(48 + sw).dimmed());

        for it in &plan.items {
            let action_styled = match it.action_type {
                RebalanceActionType::Buy => it.action_display.green().to_string(),
                RebalanceActionType::Sell => it.action_display.red().to_string(),
                RebalanceActionType::Ok => it.action_display.green().to_string(),
            };
            println!(
                "  {:<sw$} {:>10} {:>7.1}% {:>7.1}% {:>16}",
                it.symbol.cyan(),
                format!("{}{:.2}", csym, it.current_value),
                it.current_pct,
                it.target_pct,
                action_styled
            );
        }
    }

    println!();
    val.print_price_notes();
    if !val.off.marks.is_empty() {
        println!();
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// The look-through X-Ray of `portfolio` (splits already applied), valued
/// by `val`: what `portfolio xray` prints and `portfolio review` reads for
/// each company's combined weight.
pub(crate) async fn build_xray(
    client: &YahooClient,
    portfolio: &Portfolio,
    val: &Valuation,
    market: Market,
    active_name: Option<&str>,
) -> (
    xray::PortfolioXrayReport,
    Vec<XrayInputHolding>,
    std::collections::HashMap<String, funds::FundProfile>,
) {
    let mut fund_profiles = std::collections::HashMap::new();
    for h in &portfolio.holdings {
        let is_fund = offmarket::is_fund(&h.symbol);
        let quote = val.quote(&h.symbol);
        let quote_type = quote.and_then(|q| q.quote_type.as_deref());
        let is_name_etf = quote.is_some_and(|q| {
            let matches_etf = |s: &str| s.to_ascii_uppercase().contains(" ETF");
            q.short_name.as_deref().is_some_and(matches_etf)
                || q.long_name.as_deref().is_some_and(matches_etf)
        });
        let is_etf = quote_type == Some("ETF")
            || quote_type == Some("MUTUALFUND")
            || xray::is_known_etf(&h.symbol)
            || is_name_etf;

        if is_fund || is_etf || (!offmarket::is_sgb(&h.symbol) && quote_type.is_none()) {
            if let Some(fp) = funds::load_fund_disclosure(&h.symbol) {
                fund_profiles.insert(h.symbol.clone(), fp);
            } else if let Ok(fp) = funds::fetch_fund_profile(&client, &h.symbol, market).await
                && (!fp.holdings.is_empty()
                    || (!fp.sectors.is_empty()
                        && crate::index_weights::for_fund(&h.symbol, fp.name.as_deref()).is_none()))
            {
                fund_profiles.insert(h.symbol.clone(), fp);
            }
            // Otherwise an index ETF or index fund is looked through with the
            // shipped index weights (`index_weights`), inside `compute_xray`.
        }
    }

    let mut input_holdings = Vec::with_capacity(portfolio.holdings.len());
    let mut excluded: Vec<xray::ExcludedHolding> = Vec::new();
    for h in &portfolio.holdings {
        // No price, or no rate into the base: left out and named, never
        // valued at 0 or added in its own currency as if it were the base.
        let Some(price) = val.price(&h.symbol) else {
            excluded.push(xray::ExcludedHolding {
                symbol: h.symbol.clone(),
                reason: "no price".into(),
            });
            continue;
        };
        let Some(price_base) = val.to_base(&h.symbol, price) else {
            excluded.push(xray::ExcludedHolding {
                symbol: h.symbol.clone(),
                reason: format!(
                    "no {} to {} exchange rate",
                    val.currency(&h.symbol).unwrap_or_else(|| "?".into()),
                    val.base()
                ),
            });
            continue;
        };
        let value = h.shares * price_base;

        let quote = val.quote(&h.symbol);
        let quote_type = quote.and_then(|q| q.quote_type.as_deref());
        let is_name_etf = quote.is_some_and(|q| {
            let matches_etf = |s: &str| s.to_ascii_uppercase().contains(" ETF");
            q.short_name.as_deref().is_some_and(matches_etf)
                || q.long_name.as_deref().is_some_and(matches_etf)
        });
        let is_etf = quote_type == Some("ETF")
            || quote_type == Some("MUTUALFUND")
            || xray::is_known_etf(&h.symbol)
            || is_name_etf;
        let is_fund =
            offmarket::is_fund(&h.symbol) || fund_profiles.contains_key(&h.symbol) || is_etf;
        let is_sgb = offmarket::is_sgb(&h.symbol);

        let mut sector = quote.and_then(|q| q.sector.clone());
        if sector.is_none() && is_etf {
            sector = xray::default_etf_sector(&h.symbol).map(str::to_string);
        }
        if sector.is_none() && !is_fund && !is_sgb && !offmarket::is_off_market(&h.symbol) {
            let url = crate::api::build_url(
                "https://query2.finance.yahoo.com/v10/finance/quoteSummary",
                Some(&h.symbol),
                &[("modules", "assetProfile"), ("crumb", client.crumb())],
            );
            if let Ok(v) = client.raw_get(&url).await {
                sector = v
                    .pointer("/quoteSummary/result/0/assetProfile/sector")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
            }
        }
        let market_cap = quote.and_then(|q| q.market_cap);
        let currency = val.currency(&h.symbol);
        let fund_category = val
            .off
            .mark(&h.symbol)
            .and_then(|m| m.category.clone())
            .or_else(|| {
                fund_profiles
                    .get(&h.symbol)
                    .and_then(|fp| fp.category.clone())
            })
            .or_else(|| {
                if is_etf {
                    xray::default_etf_category(&h.symbol).map(str::to_string)
                } else {
                    None
                }
            });

        let name = val
            .name(&h.symbol)
            .or_else(|| fund_profiles.get(&h.symbol).and_then(|fp| fp.name.clone()))
            .or_else(|| quote.and_then(|q| q.long_name.clone().or_else(|| q.short_name.clone())))
            .or_else(|| {
                if is_etf {
                    xray::default_etf_name(&h.symbol).map(str::to_string)
                } else {
                    None
                }
            });

        input_holdings.push(XrayInputHolding {
            symbol: h.symbol.clone(),
            name,
            shares: h.shares,
            price: price_base,
            value,
            sector,
            market_cap,
            currency,
            is_fund,
            is_sgb,
            fund_category,
        });
    }

    let p_name = active_name.unwrap_or("default");
    let prof = crate::config::effective_profile();
    let mut report = xray::compute_xray_with_threshold(
        p_name,
        val.base(),
        &input_holdings,
        &fund_profiles,
        prof.max_stock_weight,
    );
    report.excluded = excluded;
    (report, input_holdings, fund_profiles)
}

/// A coloured Action cell: `HOLD (med)`, `EXIT (high)`, `NO CALL`.
pub(crate) fn action_cell(a: Option<&crate::action::Action>) -> String {
    use crate::action::Kind;
    let Some(a) = a else {
        return "—".dimmed().to_string();
    };
    let cell = format!("{:<12}", a.cell());
    match a.kind {
        Kind::Exit | Kind::Avoid => cell.red().bold().to_string(),
        Kind::Trim | Kind::Watch => cell.yellow().to_string(),
        Kind::Add | Kind::Buy => cell.green().to_string(),
        Kind::Hold => cell.cyan().to_string(),
        Kind::NoCall => cell.dimmed().to_string(),
    }
}

/// `portfolio xray` — look-through overlap across stocks, ETFs, and funds,
/// plus asset-class, market-cap, and sector concentration.
pub async fn cmd_portfolio_xray(
    portfolio_name: Option<&str>,
    market: Market,
    base: Option<&str>,
    json: bool,
) -> Result<()> {
    let active_name = portfolio_name.or(pf::active());
    let portfolio = Portfolio::load_named(active_name)?;
    if portfolio.holdings.is_empty() {
        if json {
            let report = xray::compute_xray(
                active_name.unwrap_or("default"),
                market_currency(market),
                &[],
                &std::collections::HashMap::new(),
            );
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(());
        }
        println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
        return Ok(());
    }

    let client = YahooClient::new().await?;
    // A split or bonus not yet applied to the lots: value the share count
    // after it, so the look-through is not halved.
    let pending = crate::corp_actions::pending_splits(&client, &portfolio).await;
    let portfolio = pending.adjusted(&portfolio);
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = value_holdings(&client, &symbols, market, base).await?;

    let (report, input_holdings, fund_profiles) =
        build_xray(&client, &portfolio, &val, market, active_name).await;
    let prof = crate::config::effective_profile();

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    print_header(&portfolio_title("Portfolio X-Ray"));
    println!(
        "  {} {}   {} {}   {} {}",
        "Total Portfolio Value:".dimmed(),
        format_price(report.total_value, Some(val.base())).bold(),
        "Base Currency:".dimmed(),
        val.base().cyan(),
        "Profile:".dimmed(),
        prof.spec.cyan()
    );
    println!();
    pending.print();
    if !report.excluded.is_empty() {
        let names: Vec<String> = report
            .excluded
            .iter()
            .map(|e| format!("{} ({})", e.symbol, e.reason))
            .collect();
        println!(
            "  {} Left out of the X-Ray: {}.",
            "!".yellow(),
            names.join(", ")
        );
    }
    if !pending.is_empty() || !report.excluded.is_empty() {
        println!();
    }

    // Concentration Alerts
    if !report.concentrations_above_10pct.is_empty() {
        println!(
            "  {} {}",
            "!".red().bold(),
            format!(
                "High Concentration Risk: {} holding(s) exceed your {:.1}% single-stock guardrail (max_stock_weight)!",
                report.concentrations_above_10pct.len(),
                prof.max_stock_weight
            )
            .red()
            .bold()
        );
        for c in &report.concentrations_above_10pct {
            let via_str = if !c.via_funds.is_empty() {
                let f_items: Vec<String> = c
                    .via_funds
                    .iter()
                    .map(|f| format!("{}: {:.2}%", f.fund_symbol, f.effective_weight_pct))
                    .collect();
                format!(" (via funds: {})", f_items.join(", "))
            } else {
                String::new()
            };
            println!(
                "    • {:<12} {:>6.2}% effective  [Direct: {:>5.2}%, Funds: {:>5.2}%]{}",
                c.symbol.bold().yellow(),
                c.effective_weight_pct,
                c.direct_weight_pct,
                c.fund_weight_pct,
                via_str.dimmed()
            );
        }
        println!();
    } else {
        println!(
            "  {} {}",
            "✓".green().bold(),
            format!(
                "No individual company exceeds your {:.1}% concentration threshold (max_stock_weight).",
                prof.max_stock_weight
            )
            .green()
        );
        println!();
    }

    // Indian business groups: one promoter behind several holdings.
    if !report.business_groups.is_empty() {
        print_section("Business groups (direct + through funds)");
        for g in &report.business_groups {
            let line = format!(
                "  {:<38} {:>6.2}%  {}",
                g.group,
                g.weight_pct,
                g.companies.join(", ")
            );
            if g.is_concentrated {
                println!("{}", line.yellow());
            } else {
                println!("{}", line);
            }
        }
        if report.business_groups.iter().any(|g| g.is_concentrated) {
            println!(
                "  {} {}",
                "!".yellow().bold(),
                format!(
                    "Over {:.0}% of the portfolio shares one promoter group: group-level news moves these holdings together.",
                    crate::business_groups::GROUP_NOTICE_PCT
                )
                .yellow()
            );
        }
        println!();
    }

    if report.market_cap_breakdown.small_cap_pct > prof.max_smallcap_weight + 1e-6 {
        println!(
            "  {} {}",
            "!".yellow().bold(),
            format!(
                "Profile guardrail ({}): Small-cap equity exposure ({:.2}%) exceeds your configured max_smallcap_weight ({:.1}%).",
                prof.spec,
                report.market_cap_breakdown.small_cap_pct,
                prof.max_smallcap_weight
            )
            .yellow()
        );
        println!();
    }

    if prof.mode == crate::config::ProfileMode::Passive {
        let regular_count = input_holdings
            .iter()
            .filter(|h| {
                h.is_fund
                    && h.name.as_deref().is_some_and(|n| {
                        let lower = n.to_ascii_lowercase();
                        lower.contains("regular") && !lower.contains("direct")
                    })
            })
            .count();
        if regular_count > 0 {
            println!(
                "  {} {}",
                "ℹ".cyan().bold(),
                format!(
                    "Passive guardrail: {} Regular-plan mutual fund(s) detected. Run `stockwise mf bleed` to inspect the Direct-plan TER difference.",
                    regular_count
                )
                .cyan()
            );
            println!();
        }
    }

    // Top Effective Exposures
    print_section("Top Effective Stock Exposures (Look-Through)");
    println!(
        "  {:<12} {:<24} {:>12} {:>12} {:>12} {:>9}  {}",
        "Symbol".bold(),
        "Company".bold(),
        "Direct".bold(),
        "Via Funds".bold(),
        "Total Value".bold(),
        "Weight".bold(),
        "Contributing Funds".bold()
    );
    println!("  {}", "─".repeat(95).dimmed());

    for c in report.effective_holdings.iter().take(15) {
        let sym_display = if c.is_concentrated {
            format!("{} *", c.symbol).bold().yellow()
        } else {
            c.symbol.cyan()
        };
        let name_display = truncate(c.name.as_deref().unwrap_or("—"), 24);
        let via_display = if c.via_funds.is_empty() {
            "—".dimmed().to_string()
        } else {
            let f_list: Vec<&str> = c.via_funds.iter().map(|f| f.fund_symbol.as_str()).collect();
            truncate(&f_list.join(", "), 20)
        };

        println!(
            "  {:<12} {:<24} {:>12} {:>12} {:>12} {:>8.2}%  {}",
            sym_display,
            name_display,
            format_price(c.direct_value, Some(val.base())),
            format_price(c.fund_value, Some(val.base())),
            format_price(c.total_value, Some(val.base())).bold(),
            c.effective_weight_pct,
            via_display
        );
    }
    if report.effective_holdings.len() > 15 {
        println!(
            "  {}",
            format!(
                "... and {} more positions (use --json for full list)",
                report.effective_holdings.len() - 15
            )
            .dimmed()
        );
    }
    if report.effective_holdings.iter().any(|h| h.is_concentrated) {
        println!(
            "  {}",
            format!(
                "* Exceeds {:.1}% concentration limit (max_stock_weight)",
                prof.max_stock_weight
            )
            .yellow()
            .dimmed()
        );
    }
    println!();

    // Asset Class Split
    print_section("Asset Class Allocation");
    println!(
        "  {:<20} {:>14} {:>10}",
        "Asset Class".bold(),
        "Value".bold(),
        "Weight".bold()
    );
    println!("  {}", "─".repeat(46).dimmed());
    let classes = [
        (
            "Equity",
            report.asset_class_split.equity_value,
            report.asset_class_split.equity_pct,
        ),
        (
            "Debt",
            report.asset_class_split.debt_value,
            report.asset_class_split.debt_pct,
        ),
        (
            "Gold / Commodity",
            report.asset_class_split.commodity_value,
            report.asset_class_split.commodity_pct,
        ),
        (
            "Cash / Liquid",
            report.asset_class_split.cash_value,
            report.asset_class_split.cash_pct,
        ),
    ];
    for (name, val_amt, pct_amt) in classes {
        if val_amt > 0.0 || pct_amt > 0.0 {
            println!(
                "  {:<20} {:>14} {:>9.2}%",
                name,
                format_price(val_amt, Some(val.base())),
                pct_amt
            );
        }
    }
    println!();

    // Market Cap Breakdown
    print_section("Market Cap Breakdown (Equity)");
    println!(
        "  {:<20} {:>14} {:>10}",
        "Market Cap".bold(),
        "Value".bold(),
        "Weight".bold()
    );
    println!("  {}", "─".repeat(46).dimmed());
    let mc_rows = [
        (
            "Large Cap",
            report.market_cap_breakdown.large_cap_value,
            report.market_cap_breakdown.large_cap_pct,
        ),
        (
            "Mid Cap",
            report.market_cap_breakdown.mid_cap_value,
            report.market_cap_breakdown.mid_cap_pct,
        ),
        (
            "Small Cap",
            report.market_cap_breakdown.small_cap_value,
            report.market_cap_breakdown.small_cap_pct,
        ),
        (
            "Unclassified",
            report.market_cap_breakdown.unclassified_value,
            report.market_cap_breakdown.unclassified_pct,
        ),
    ];
    for (name, val_amt, pct_amt) in mc_rows {
        if val_amt > 0.0 || pct_amt > 0.0 {
            println!(
                "  {:<20} {:>14} {:>9.2}%",
                name,
                format_price(val_amt, Some(val.base())),
                pct_amt
            );
        }
    }
    println!();

    // Sector Concentration
    print_section("Sector Concentration");
    println!(
        "  {:<28} {:>14} {:>10}",
        "Sector".bold(),
        "Value".bold(),
        "Weight".bold()
    );
    println!("  {}", "─".repeat(54).dimmed());
    for s in &report.sector_allocation {
        if s.value > 0.0 {
            println!(
                "  {:<28} {:>14} {:>9.2}%",
                truncate(&s.sector, 28),
                format_price(s.value, Some(val.base())),
                s.weight_pct
            );
        }
    }
    println!();

    // Funds Analyzed
    if !report.funds_analyzed.is_empty() {
        print_section("Funds & ETFs Analyzed");
        for f in &report.funds_analyzed {
            if f.look_through_available {
                let shipped = !fund_profiles.contains_key(&f.symbol)
                    && crate::index_weights::for_fund(&f.symbol, f.name.as_deref()).is_some();
                let source = if shipped {
                    format!(", index weights as of {}", crate::index_weights::AS_OF)
                } else {
                    String::new()
                };
                println!(
                    "  • {:<12} {:<30} (look-through: {} holdings, {:.1}% coverage{})",
                    f.symbol.cyan(),
                    truncate(f.name.as_deref().unwrap_or("—"), 30),
                    f.holdings_count,
                    f.coverage_pct,
                    source
                );
            } else {
                let note = if xray::is_known_etf(&f.symbol) {
                    "(no holdings known: categorized by ETF profile)"
                } else {
                    "(no holdings known: categorized by scheme category)"
                };
                println!(
                    "  • {:<12} {:<30} {}",
                    f.symbol.cyan(),
                    truncate(f.name.as_deref().unwrap_or("—"), 30).dimmed(),
                    note.dimmed()
                );
            }
        }
        println!();
    }

    val.print_price_notes();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ChartData, Quote, SearchResult};

    /// A quote source that is always down.
    struct Down;
    impl views::DataSource for Down {
        async fn quotes(&self, _: &[&str]) -> Result<Vec<Quote>> {
            bail!("Cannot reach Yahoo Finance")
        }
        async fn chart(&self, _: &str, _: &str, _: &str) -> Result<ChartData> {
            bail!("down")
        }
        async fn search(&self, _: &str) -> Result<Vec<SearchResult>> {
            bail!("down")
        }
    }

    fn one_holding() -> Portfolio {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", pf::Lot::new(10.0, 3500.0, "2024-01-02", "manual"));
        p
    }

    #[test]
    fn sell_hint_names_the_symbol() {
        let hint = sell_history_hint("TCS.NS");
        assert!(hint.contains("`stockwise portfolio lots TCS.NS`"), "{hint}");
    }

    #[tokio::test]
    async fn offline_valuation_keeps_every_holding_unpriced() {
        let p = one_holding();
        let symbols = vec!["TCS.NS".to_string()];
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();

        // The quote request fails.
        let (val, live) = value_or_offline(Some(&Down), &symbols, Market::In, None).await;
        assert!(!live);
        // No client at all (Yahoo's cookie handshake failed).
        let (val2, live2) = value_or_offline(None::<&Down>, &symbols, Market::In, None).await;
        assert!(!live2);

        for v in [val, val2] {
            let r = portfolio_report(&p, &v, today);
            assert_eq!(r.holdings.len(), 1);
            let h = &r.holdings[0];
            assert_eq!(
                (h.shares, h.avg_cost, h.cost_basis),
                (10.0, 3500.0, 35000.0)
            );
            assert!(h.price.is_none() && h.value.is_none());
            assert_eq!(r.unpriced, vec!["TCS.NS".to_string()]);
            let json = serde_json::to_value(&r).unwrap();
            assert!(json["holdings"][0]["price"].is_null());
        }
    }

    fn day(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn usd_quote(symbol: &str, price: f64) -> Quote {
        Quote {
            symbol: Some(symbol.into()),
            regular_market_price: Some(price),
            currency: Some("USD".into()),
            ..Default::default()
        }
    }

    /// AAPL in an INR portfolio: USDINR 75 on the purchase date, 84 today.
    fn usd_valuation(p: &Portfolio) -> Valuation {
        let symbols: Vec<String> = p.holdings.iter().map(|h| h.symbol.clone()).collect();
        let mut v = Valuation::new(
            vec![usd_quote("AAPL", 100.0)],
            &symbols,
            Market::In,
            Some("INR"),
        );
        v.fx.set_rate("USD", 84.0);
        v.hist.insert(
            "USD",
            vec![
                (day("2024-01-02"), 75.0),
                (day("2025-06-02"), 80.0),
                (day("2026-11-19"), 84.0),
            ],
        );
        v
    }

    #[test]
    fn a_usd_lot_at_the_same_price_gains_the_rupee_fall() {
        let mut p = Portfolio::default();
        p.add_lot("AAPL", pf::Lot::new(10.0, 100.0, "2024-01-02", "manual"));
        let v = usd_valuation(&p);
        let t = v.totals(&p);
        assert!((t.value - 84_000.0).abs() < 1e-6);
        assert!((t.cost - 75_000.0).abs() < 1e-6);
        // About +12% in rupees with the dollar price unchanged.
        assert!((t.pnl_pct() - 12.0).abs() < 1e-9, "{}", t.pnl_pct());
        assert!((t.fx_effect() - 9_000.0).abs() < 1e-6);
        assert!(t.cost_at_today_fx.is_empty());
        let r = portfolio_report(&p, &v, day("2026-11-21"));
        assert_eq!(r.totals.fx_effect, 9_000.0);
        assert_eq!(r.holdings[0].cost_basis_base, Some(75_000.0));
        assert_eq!(r.holdings[0].fx_effect, Some(9_000.0));
    }

    #[test]
    fn a_lot_paid_in_rupees_keeps_what_was_paid() {
        let mut p = Portfolio::default();
        let mut lot = pf::Lot::new(10.0, 100.0, "2024-01-02", "manual");
        // Paid ₹76 per dollar (the bank's rate, not the reference rate).
        lot.fx_currency = Some("INR".into());
        lot.fx_rate = Some(76.0);
        p.add_lot("AAPL", lot);
        let v = usd_valuation(&p);
        assert!((v.totals(&p).cost - 76_000.0).abs() < 1e-6);
    }

    #[test]
    fn without_history_cost_falls_back_to_today_and_says_so() {
        let mut p = Portfolio::default();
        p.add_lot("AAPL", pf::Lot::new(10.0, 100.0, "2010-01-04", "manual"));
        let v = usd_valuation(&p);
        let t = v.totals(&p);
        assert!((t.cost - 84_000.0).abs() < 1e-6);
        assert_eq!(t.cost_at_today_fx, vec!["AAPL".to_string()]);
    }

    #[test]
    fn realized_dividends_and_xirr_use_each_dates_rate_and_keep_closed_positions() {
        let mut p = Portfolio::default();
        p.add_lot("AAPL", pf::Lot::new(10.0, 100.0, "2024-01-02", "manual"));
        // A closed position: bought at 75, sold at 80 (same dollar price),
        // with $5 of charges on the sale.
        p.add_lot("MSFT", pf::Lot::new(1.0, 100.0, "2024-01-02", "manual"));
        p.sell_fifo("MSFT", 1.0, 100.0, "2025-06-02", "manual")
            .unwrap();
        p.realized[0].add_charges(5.0);
        let mut v = usd_valuation(&p);
        v.quotes.push(usd_quote("MSFT", 100.0));
        let sale = p.realized[0].clone();
        // 95 × 80 − 100 × 75.
        assert!((v.sale_gain_base(&sale).unwrap() - 100.0).abs() < 1e-6);
        assert!((sale.gain() + 5.0).abs() < 1e-9);
        let inc = income_since(&p, &v, day("2025-04-01"));
        assert_eq!((inc.sales, inc.sales_since), (1, 1));
        assert!((inc.charges - 5.0 * 84.0).abs() < 1e-6);
        let today = day("2026-11-21");
        let t = v.totals(&p);
        let (x, left_out) = portfolio_xirr(&p, &v, &t, today);
        assert!(left_out.is_empty());
        // Rupee XIRR includes the rupee fall on both positions.
        assert!(x.unwrap() > 0.03, "{x:?}");
    }

    #[test]
    fn financial_year_starts_in_april_for_rupees() {
        assert_eq!(fy_start(day("2026-11-21"), "INR"), day("2026-04-01"));
        assert_eq!(fy_start(day("2026-02-10"), "INR"), day("2025-04-01"));
        assert_eq!(fy_start(day("2026-02-10"), "USD"), day("2026-01-01"));
        assert_eq!(fy_label(day("2026-04-01"), "INR"), "FY 2026-27");
    }

    #[test]
    fn period_change_counts_only_holdings_with_a_past_price() {
        let mut p = Portfolio::default();
        p.add_lot("AAPL", pf::Lot::new(10.0, 100.0, "2024-01-02", "manual"));
        p.add_lot("MSFT", pf::Lot::new(1.0, 100.0, "2024-01-02", "manual"));
        let mut v = usd_valuation(&p);
        v.quotes.push(usd_quote("MSFT", 100.0));
        let c = period_change("1 week", &p, &v, |s| (s == "AAPL").then_some(80.0)).unwrap();
        assert_eq!((c.covered, c.of), (1, 2));
        assert!((c.pct - 25.0).abs() < 1e-9);
        assert!((c.change - 20.0 * 10.0 * 84.0).abs() < 1e-6);
    }

    #[test]
    fn xirr_is_hidden_for_estimated_dates() {
        let mut p = one_holding();
        p.holdings[0].lots[0].date_estimated = true;
        let symbols = vec!["TCS.NS".to_string()];
        let q = Quote {
            symbol: Some("TCS.NS".into()),
            regular_market_price: Some(4000.0),
            currency: Some("INR".into()),
            ..Default::default()
        };
        let v = Valuation::new(vec![q], &symbols, Market::In, None);
        let r = portfolio_report(&p, &v, day("2026-11-21"));
        assert!(r.holdings[0].xirr_percent.is_none());
    }

    #[test]
    fn a_pending_split_shows_no_fake_loss() {
        let mut p = Portfolio::default();
        p.add_lot(
            "RELIANCE.NS",
            pf::Lot::new(10.0, 2800.0, "2024-01-10", "manual"),
        );
        let pending = crate::corp_actions::PendingSplits {
            by_symbol: vec![(
                "RELIANCE.NS".into(),
                vec![Split::new("2024-10-28", 2.0, 1.0)],
            )],
            unchecked: vec![],
        };
        let q = Quote {
            symbol: Some("RELIANCE.NS".into()),
            regular_market_price: Some(1400.0),
            currency: Some("INR".into()),
            ..Default::default()
        };
        let symbols = vec!["RELIANCE.NS".to_string()];
        let v = Valuation::new(vec![q], &symbols, Market::In, None);
        // Unadjusted: a fake 50% loss.
        assert!((v.totals(&p).pnl_pct() + 50.0).abs() < 1e-9);
        let adjusted = pending.adjusted(&p);
        assert!(v.totals(&adjusted).pnl_pct().abs() < 1e-9);
        let mut r = portfolio_report(&adjusted, &v, day("2026-11-21"));
        r.warnings = pending.warnings();
        assert!(r.warnings[0].contains("split pending"));
        assert_eq!(r.holdings[0].shares, 20.0);
    }

    #[test]
    fn a_partial_sale_takes_its_share_of_the_lot_charges() {
        let mut p = Portfolio::default();
        let mut lot = pf::Lot::new(10.0, 100.0, "2024-01-02", "manual");
        lot.charges = Some(20.0);
        p.add_lot("TCS.NS", lot);
        let sales = p
            .sell_fifo("TCS.NS", 4.0, 110.0, "2025-01-02", "manual")
            .unwrap();
        assert!((sales[0].charges() - 8.0).abs() < 1e-9);
        // 4 × 10 − 8.
        assert!((sales[0].gain() - 32.0).abs() < 1e-9);
        let left = &p.holding("TCS.NS").unwrap().lots[0];
        assert!((left.charges.unwrap() - 12.0).abs() < 1e-9);
        // Undoing the sale gives the charges back to a lot.
        assert_eq!(sales[0].to_lot().charges, Some(8.0));
    }

    #[test]
    fn pme_leaves_out_a_delisted_holding_on_both_sides() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", pf::Lot::new(10.0, 100.0, "2024-01-02", "manual"));
        // Delisted: no price. Valued at 0 it would be a 100% loss.
        p.add_lot("GONE.NS", pf::Lot::new(10.0, 100.0, "2024-01-02", "manual"));
        let symbols = vec!["TCS.NS".to_string(), "GONE.NS".to_string()];
        let q = Quote {
            symbol: Some("TCS.NS".into()),
            regular_market_price: Some(120.0),
            currency: Some("INR".into()),
            ..Default::default()
        };
        let v = Valuation::new(vec![q], &symbols, Market::In, None);
        let closes = vec![(day("2024-01-02"), 100.0), (day("2025-01-02"), 120.0)];
        let r = crate::commands::planning::pme_report_from(
            &p,
            &v,
            &closes,
            day("2025-01-02"),
            ("^NSEI".into(), "NIFTY 50".into()),
            false,
        )
        .unwrap();
        assert_eq!(r.summary.excluded, vec!["GONE.NS".to_string()]);
        assert_eq!(r.summary.total_invested, 1000.0);
        assert_eq!(r.summary.actual_value, 1200.0);
        // Matched the index exactly once the delisted holding is out.
        assert_eq!(r.summary.value_added, 0.0);
        assert_eq!(r.summary.benchmark_kind, "price");
    }

    #[test]
    fn pme_with_a_price_index_leaves_dividends_out() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", pf::Lot::new(10.0, 100.0, "2024-01-02", "manual"));
        p.dividends.push(pf::DividendReceived {
            symbol: "TCS.NS".into(),
            ex_date: "2024-06-01".into(),
            per_share: 5.0,
            qty: 10.0,
            amount: 50.0,
            currency: None,
        });
        let symbols = vec!["TCS.NS".to_string()];
        let q = Quote {
            symbol: Some("TCS.NS".into()),
            regular_market_price: Some(120.0),
            currency: Some("INR".into()),
            ..Default::default()
        };
        let v = Valuation::new(vec![q], &symbols, Market::In, None);
        let closes = vec![(day("2024-01-02"), 100.0), (day("2025-01-02"), 120.0)];
        let price_only = crate::commands::planning::pme_report_from(
            &p,
            &v,
            &closes,
            day("2025-01-02"),
            ("^NSEI".into(), "NIFTY 50".into()),
            false,
        )
        .unwrap();
        assert_eq!(price_only.summary.total_distributed, 0.0);
        assert!(!price_only.summary.dividends_included);
        let tri = crate::commands::planning::pme_report_from(
            &p,
            &v,
            &closes,
            day("2025-01-02"),
            ("^SP500TR".into(), "TR".into()),
            true,
        )
        .unwrap();
        assert_eq!(tri.summary.total_distributed, 50.0);
    }

    use super::fx_pair;

    #[test]
    fn a_price_paid_in_another_currency_is_converted_to_the_quote_unit() {
        // ₹15,000 at USDINR 83.5 (fx_pair(INR, USD) → USDINR=X, inverted).
        let (pair, inverted) = fx_pair("INR", "USD");
        assert_eq!((pair.as_str(), inverted), ("USDINR=X", true));
        assert!(
            (convert_paid(15_000.0, 83.5, inverted, "USD") - 179.640_718_562_874_25).abs() < 1e-9
        );
        // $150 for a London share at GBPUSD 1.25: £120 = 12,000 pence.
        let (pair, inverted) = fx_pair("USD", "GBP");
        assert_eq!((pair.as_str(), inverted), ("GBPUSD=X", true));
        assert!((convert_paid(150.0, 1.25, inverted, "GBp") - 12_000.0).abs() < 1e-9);
        // Pounds for a pence quote: only the unit changes.
        assert_eq!(convert_paid(1.5, 1.0, false, "GBp"), 150.0);
        // €100 for a US share at EURUSD 1.08: $108.
        let (pair, inverted) = fx_pair("EUR", "USD");
        assert_eq!((pair.as_str(), inverted), ("EURUSD=X", false));
        assert!((convert_paid(100.0, 1.08, inverted, "USD") - 108.0).abs() < 1e-9);
    }

    #[test]
    fn fx_pair_follows_yahoo_quotation() {
        let cases = [
            ("INR", "USD", "USDINR=X", true),
            ("USD", "INR", "USDINR=X", false),
            ("USD", "EUR", "EURUSD=X", true),
            ("EUR", "USD", "EURUSD=X", false),
            ("USD", "GBP", "GBPUSD=X", true),
            ("USD", "AUD", "AUDUSD=X", true),
            ("USD", "CAD", "USDCAD=X", false),
            ("JPY", "USD", "USDJPY=X", true),
            ("GBP", "EUR", "EURGBP=X", true),
            ("EUR", "INR", "EURINR=X", false),
            ("usd", "eur", "EURUSD=X", true),
        ];
        for (cur, base, sym, invert) in cases {
            assert_eq!(
                fx_pair(cur, base),
                (sym.to_string(), invert),
                "{cur}->{base}"
            );
        }
    }
}
