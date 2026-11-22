//! Where the rule-based actions of [`crate::action`] meet the commands:
//! `portfolio review`, the holding context behind `verdict` / `deep` / a bare
//! symbol, the Action column of `portfolio` and `checkup`, and `picks`.
//!
//! Nothing here decides an action: it gathers the inputs and calls
//! [`crate::action::decide`].

use super::*;
use crate::action::{self, Action, HoldingContext, Horizon, Kind};
use crate::api::Quote;
use crate::notes::NoteStore;
use crate::portfolio as pf;
use crate::research::Series;
use chrono::NaiveDate;
use futures_util::StreamExt;

/// Price-chart requests at a time while reviewing holdings.
const CHART_CONCURRENCY: usize = 6;

fn notes() -> NoteStore {
    crate::store::load(crate::notes::FILE).unwrap_or_default()
}

/// The next lot of `h` to turn long-term after `today`, under the tax rules
/// of the investor's residence: days, date and the tax that waiting saves
/// (short-term minus long-term rate on the lot's gain, quote currency).
fn next_long_term(
    h: &pf::Holding,
    price: f64,
    currency: Option<&str>,
    market: Market,
    today: NaiveDate,
) -> Option<(i64, NaiveDate, Option<f64>)> {
    let home = super::portfolio_analyze::residence(market);
    let (country, rule) = super::portfolio_analyze::term_rule(home, &h.symbol, currency, None);
    let default = country.and_then(|c| c.long_term_months());
    let mut best: Option<(i64, NaiveDate, Option<f64>)> = None;
    for lot in h.lots.iter().filter(|l| !l.date_estimated) {
        let Ok(bought) = NaiveDate::parse_from_str(&lot.date, "%Y-%m-%d") else {
            continue;
        };
        let Some(months) = rule.months_for(default, bought) else {
            continue;
        };
        let Some(on) = crate::sellpreview::long_term_from(bought, months) else {
            continue;
        };
        if on <= today || best.is_some_and(|(_, d, _)| d <= on) {
            continue;
        }
        let diff_minor =
            crate::tax::paise::from_rupees(price) - crate::tax::paise::from_rupees(lot.price);
        let gain_minor = crate::tax::paise::times(diff_minor, lot.qty).max(0);
        let saves = country.map(|c| {
            let saved_minor =
                crate::tax::paise::times(gain_minor, (c.st_rate() - c.lt_rate()) / 100.0);
            crate::tax::paise::to_rupees(saved_minor)
        });
        best = Some(((on - today).num_days(), on, saves));
    }
    best
}

/// The holding context for `symbol` in the active portfolio, or `None` when
/// it is not held. Weight is left unknown: the single-stock views do not
/// value the whole portfolio (`portfolio review` does).
pub(crate) fn holding_context_for(
    symbol: &str,
    price: Option<f64>,
    currency: Option<&str>,
    pe: Option<f64>,
    roe: Option<f64>,
    market: Market,
) -> Result<Option<HoldingContext>> {
    let portfolio = Portfolio::load()?;
    let Some(h) = portfolio.holding(symbol) else {
        return Ok(None);
    };
    if h.shares <= 0.0 {
        return Ok(None);
    }
    let today = chrono::Local::now().date_naive();
    let notes = notes();
    Ok(Some(HoldingContext {
        shares: h.shares,
        avg_cost: h.avg_cost,
        weight_pct: None,
        lookthrough_weight_pct: None,
        target_weight_pct: portfolio.targets.get(&h.symbol).copied(),
        thesis_breaches: action::thesis_breaches(notes.get(symbol), pe, roe, None),
        next_long_term: price.and_then(|p| next_long_term(h, p, currency, market, today)),
    }))
}

/// Re-decide a verdict's action as a holding when the active portfolio has
/// the stock.
pub(crate) fn apply_holding(
    v: &mut crate::verdict::DecisionVerdict,
    r: &crate::research::Research,
    market: Market,
) {
    if cfg!(test) {
        return;
    }
    match holding_context_for(
        &r.symbol,
        r.price(),
        r.currency.as_deref(),
        r.fin.trailing_pe.or(r.quote.trailing_pe),
        r.fin.roe,
        market,
    ) {
        Ok(Some(h)) => {
            crate::verdict::set_holding(v, r, &crate::config::effective_profile(), h);
        }
        Ok(None) => {}
        Err(e) => {
            let msg = format!("Active portfolio could not be read ({e}); holding status unknown");
            let mut no = Action::undecided(&v.currency);
            no.horizon = v.action.horizon;
            no.coverage = v.action.coverage;
            no.reasons = vec![msg.clone()];
            v.action = no;
            v.profile_notes.push(msg);
        }
    }
}

/// Structured quote-only action for a holding or watched stock (`today`,
/// `hot`, the digest and the Telegram bot).
pub(crate) fn row_action(
    q: &Quote,
    h: Option<&pf::Holding>,
    weight_pct: Option<f64>,
    target_weight_pct: Option<f64>,
    notes: &NoteStore,
    profile: &crate::config::EffectiveProfile,
    today: NaiveDate,
) -> Action {
    match h {
        Some(h) => quick_holding_action(q, h, weight_pct, target_weight_pct, notes, profile, today),
        None => {
            let score = crate::longterm::score_for_longterm(q, None);
            let i = action::inputs_from_quote(q, &Series::default(), &score, Horizon::Long, today)
                .with_profile(profile);
            action::decide(&i)
        }
    }
}

/// Compact one-line summary of an [`Action`] that retains its invalidation
/// price for `BUY`, `ADD`, `HOLD` and `TRIM` alongside its reasons.
pub(crate) fn format_action_line(a: &Action) -> String {
    if a.kind == Kind::NoCall {
        return a.headline().trim_start_matches("ACTION: ").to_string();
    }
    let level = a
        .invalidation_price
        .map(|p| {
            format!(
                " (invalid below {}{p:.2})",
                crate::market::currency_symbol(Some(&a.currency))
            )
        })
        .unwrap_or_default();
    let reasons = a.reasons.join("; ");
    if reasons.is_empty() {
        format!("{}{level}", a.label)
    } else {
        format!("{}{level} — {reasons}", a.label)
    }
}

/// The one-line action `today`, `hot`, the digest and the Telegram bot show:
/// the quote-only rules (see [`quick_holding_action`]) for a holding, or for
/// a watched stock not held, including the invalidation level when present.
pub(crate) fn action_line(
    q: &Quote,
    h: Option<&pf::Holding>,
    weight_pct: Option<f64>,
    target_weight_pct: Option<f64>,
    notes: &NoteStore,
    profile: &crate::config::EffectiveProfile,
    today: NaiveDate,
) -> String {
    let a = row_action(q, h, weight_pct, target_weight_pct, notes, profile, today);
    format_action_line(&a)
}

/// The user's notes (thesis limits), for callers outside this module.
pub(crate) fn load_notes() -> NoteStore {
    notes()
}

/// A quick, quote-only action for a holding (no price history: the 50- and
/// 200-day averages come from the quote and there is no ATR). For the
/// Action column of `portfolio` and `checkup`; `portfolio review` fetches
/// the history for the full rules.
pub(crate) fn quick_holding_action(
    q: &Quote,
    h: &pf::Holding,
    weight_pct: Option<f64>,
    target_weight_pct: Option<f64>,
    notes: &NoteStore,
    profile: &crate::config::EffectiveProfile,
    today: NaiveDate,
) -> Action {
    let score = crate::longterm::score_for_longterm(q, None);
    let mut i = action::inputs_from_quote(q, &Series::default(), &score, Horizon::Long, today)
        .with_profile(profile);
    i.holding = Some(HoldingContext {
        shares: h.shares,
        avg_cost: h.avg_cost,
        weight_pct,
        target_weight_pct,
        thesis_breaches: action::thesis_breaches(
            notes.get(&h.symbol),
            q.trailing_pe,
            q.return_on_equity,
            weight_pct,
        ),
        ..Default::default()
    });
    action::decide(&i)
}

/// Fundamentals for the listed stocks among the holdings: the bulk quote
/// carries no ROE, margins, debt or sector, so each stock's quote is
/// completed (`quoteSummary`, then the exchange and keyed fallbacks) before
/// the rules read it. By symbol; funds, SGBs and failed lookups are absent.
pub(crate) async fn enriched_quotes(
    client: &YahooClient,
    portfolio: &Portfolio,
    val: &super::portfolio::Valuation,
) -> std::collections::HashMap<String, Quote> {
    if cfg!(test) {
        return Default::default();
    }
    let todo: Vec<Quote> = portfolio
        .holdings
        .iter()
        .filter(|h| !crate::offmarket::is_off_market(&h.symbol))
        .filter_map(|h| val.quote(&h.symbol).cloned())
        .filter(|q| {
            q.quote_type
                .as_deref()
                .is_none_or(|t| t.eq_ignore_ascii_case("EQUITY"))
        })
        .collect();
    enrich_quotes(client, todo).await
}

/// Complete listed stocks' quotes (`quoteSummary`, then the exchange and
/// keyed fallbacks) so the action rules can read fundamentals. By symbol;
/// funds, ETFs and indices are left out.
pub(crate) async fn enrich_quotes(
    client: &YahooClient,
    quotes: Vec<Quote>,
) -> std::collections::HashMap<String, Quote> {
    if cfg!(test) {
        return Default::default();
    }
    let todo: Vec<Quote> = quotes
        .into_iter()
        .filter(|q| {
            q.quote_type
                .as_deref()
                .is_none_or(|t| t.eq_ignore_ascii_case("EQUITY"))
        })
        .collect();
    futures_util::stream::iter(todo.into_iter().map(|mut q| async move {
        let mut meta = crate::provenance::Meta::default();
        crate::research::enrich_quote_with(client, &mut q, false, &mut meta).await;
        let m = q.meta.get_or_insert_with(Default::default);
        m.merge(&meta);
        (q.symbol.clone().unwrap_or_default(), q)
    }))
    .buffered(CHART_CONCURRENCY)
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .collect()
}

/// Quick actions for every holding of `portfolio` valued by `val`, by
/// symbol, reading `enriched` quotes where there are some. Off-market funds
/// and SGBs get NO CALL.
pub(crate) fn quick_actions(
    portfolio: &Portfolio,
    val: &super::portfolio::Valuation,
    enriched: &std::collections::HashMap<String, Quote>,
) -> std::collections::HashMap<String, Action> {
    let profile = crate::config::effective_profile();
    let notes = notes();
    let today = chrono::Local::now().date_naive();
    let totals = val.totals(portfolio);
    let complete = totals.unpriced.is_empty() && totals.unconverted.is_empty();
    portfolio
        .holdings
        .iter()
        .map(|h| {
            let weight = complete
                .then(|| {
                    val.price(&h.symbol)
                        .and_then(|p| val.to_base(&h.symbol, h.shares * p))
                        .filter(|_| totals.value > 0.0)
                        .map(|v| v / totals.value * 100.0)
                })
                .flatten();
            let a = match enriched.get(&h.symbol).or_else(|| val.quote(&h.symbol)) {
                Some(q) => quick_holding_action(
                    q,
                    h,
                    weight,
                    portfolio.targets.get(&h.symbol).copied(),
                    &notes,
                    &profile,
                    today,
                ),
                None => {
                    let currency = val
                        .currency(&h.symbol)
                        .unwrap_or_else(|| val.base().to_string());
                    let mut i =
                        action::Inputs::new(&h.symbol, &currency, today).with_profile(&profile);
                    i.is_stock = crate::market::AssetClass::of(&h.symbol)
                        == crate::market::AssetClass::Stock
                        && !crate::offmarket::is_fund(&h.symbol)
                        && !crate::offmarket::is_sgb(&h.symbol);
                    i.holding = Some(HoldingContext {
                        shares: h.shares,
                        avg_cost: h.avg_cost,
                        weight_pct: weight,
                        target_weight_pct: portfolio.targets.get(&h.symbol).copied(),
                        ..Default::default()
                    });
                    action::decide(&i)
                }
            };
            (h.symbol.clone(), a)
        })
        .collect()
}

/// One holding in `portfolio review`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReviewRow {
    pub(crate) symbol: String,
    pub(crate) name: Option<String>,
    pub(crate) shares: f64,
    pub(crate) price: Option<f64>,
    pub(crate) currency: Option<String>,
    pub(crate) weight_pct: Option<f64>,
    pub(crate) pnl_pct: Option<f64>,
    pub(crate) action: Action,
}

/// How many holdings got each action.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct ReviewCounts {
    pub(crate) exit: usize,
    pub(crate) trim: usize,
    pub(crate) add: usize,
    pub(crate) hold: usize,
    pub(crate) no_call: usize,
}

/// `portfolio review --json`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Review {
    pub(crate) horizon: Horizon,
    pub(crate) base_currency: String,
    pub(crate) value: f64,
    pub(crate) counts: ReviewCounts,
    /// EXIT, TRIM, ADD, HOLD, then NO CALL; largest weight first within each.
    pub(crate) holdings: Vec<ReviewRow>,
    pub(crate) warnings: Vec<String>,
    pub(crate) methodology: &'static str,
    pub(crate) disclaimer: &'static str,
}

/// [`review`] for the MCP tool.
pub(crate) async fn portfolio_review_report(
    src: &impl views::DataSource,
    client: Option<&YahooClient>,
    market: Market,
    base: Option<&str>,
    horizon: Horizon,
) -> Result<Review> {
    review(src, client, market, base, horizon).await
}

/// Review every holding of the active portfolio with the full rules: a
/// year of daily bars, the long-term score, weight, look-through weight
/// (with a client), notes, targets and the next long-term date.
pub(crate) async fn review(
    src: &impl views::DataSource,
    client: Option<&YahooClient>,
    market: Market,
    base: Option<&str>,
    horizon: Horizon,
) -> Result<Review> {
    let g = super::portfolio_analyze::gather_inputs(src, client, market, base, true).await?;
    let report = super::portfolio_analyze::analysis_report(&g, market);
    let a = &report.analysis;
    let profile = crate::config::effective_profile();
    let notes = notes();
    let today = chrono::Local::now().date_naive();

    // What funds and ETFs hold of the same companies.
    let mut lookthrough = std::collections::HashMap::new();
    if let Some(c) = client
        && !cfg!(test)
    {
        let (x, _, _) =
            super::portfolio::build_xray(c, &g.portfolio, &g.val, market, pf::active()).await;
        for e in x.effective_holdings.iter().filter(|e| e.fund_value > 0.0) {
            lookthrough.insert(
                crate::xray::canonical_ticker(&e.symbol),
                e.effective_weight_pct,
            );
        }
    }

    let enriched = match client {
        Some(c) => enriched_quotes(c, &g.portfolio, &g.val).await,
        None => Default::default(),
    };
    let stocks: Vec<String> = a
        .positions
        .iter()
        .filter(|p| g.val.quote(&p.symbol).is_some())
        .map(|p| p.symbol.clone())
        .collect();
    let bars: std::collections::HashMap<String, (Series, Option<NaiveDate>)> =
        futures_util::stream::iter(stocks.into_iter().map(|sym| async move {
            let series = src
                .chart(&sym, "1y", "1d")
                .await
                .map(|c| Series::from_chart(&c))
                .unwrap_or_default();
            let next = src.next_results(&sym, today).await;
            (sym, (series, next))
        }))
        .buffered(CHART_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect();

    let complete_portfolio = a.left_out.is_empty();
    let mut rows = Vec::new();
    for p in &a.positions {
        let Some(h) = g.portfolio.holding(&p.symbol) else {
            continue;
        };
        let q = enriched.get(&p.symbol).or_else(|| g.val.quote(&p.symbol));
        let currency = p.currency.clone();
        let weight_pct = complete_portfolio.then_some(p.weight_pct).flatten();
        let action = match q {
            None => {
                let mut i =
                    action::Inputs::new(&p.symbol, currency.as_deref().unwrap_or("USD"), today)
                        .with_profile(&profile);
                i.horizon = horizon;
                i.is_stock = crate::market::AssetClass::of(&p.symbol)
                    == crate::market::AssetClass::Stock
                    && !crate::offmarket::is_fund(&p.symbol)
                    && !crate::offmarket::is_sgb(&p.symbol);
                i.holding = Some(HoldingContext {
                    shares: h.shares,
                    avg_cost: h.avg_cost,
                    weight_pct,
                    target_weight_pct: g.portfolio.targets.get(&p.symbol).copied(),
                    ..Default::default()
                });
                action::decide(&i)
            }
            Some(q) => {
                let (series, next) = bars.get(&p.symbol).cloned().unwrap_or_default();
                let closes = (!series.close.is_empty()).then_some(series.close.as_slice());
                let score = crate::longterm::score_for_longterm(q, closes);
                let mut i = action::inputs_from_quote(q, &series, &score, horizon, today)
                    .with_profile(&profile);
                i.next_earnings = next;
                let to_base = g.val.to_base(&p.symbol, 1.0).filter(|r| *r > 0.0);
                i.portfolio_value = complete_portfolio
                    .then(|| to_base.map(|r| a.value / r))
                    .flatten();
                i.holding = Some(HoldingContext {
                    shares: h.shares,
                    avg_cost: h.avg_cost,
                    weight_pct,
                    lookthrough_weight_pct: complete_portfolio
                        .then(|| {
                            lookthrough
                                .get(&crate::xray::canonical_ticker(&p.symbol))
                                .copied()
                        })
                        .flatten(),
                    target_weight_pct: g.portfolio.targets.get(&p.symbol).copied(),
                    thesis_breaches: action::thesis_breaches(
                        notes.get(&p.symbol),
                        q.trailing_pe,
                        q.return_on_equity,
                        weight_pct,
                    ),
                    next_long_term: q
                        .regular_market_price
                        .and_then(|pr| next_long_term(h, pr, currency.as_deref(), market, today)),
                });
                action::decide(&i)
            }
        };
        rows.push(ReviewRow {
            symbol: p.symbol.clone(),
            name: p.name.clone(),
            shares: p.shares,
            price: p.value.map(|v| v / p.shares.max(f64::MIN_POSITIVE)),
            currency,
            weight_pct: p.weight_pct.map(|w| round(w, 2)),
            pnl_pct: p.pnl_pct.map(|v| round(v, 2)),
            action,
        });
    }
    sort_review(&mut rows);
    let mut counts = ReviewCounts::default();
    for r in &rows {
        match r.action.kind {
            Kind::Exit => counts.exit += 1,
            Kind::Trim => counts.trim += 1,
            Kind::Add => counts.add += 1,
            Kind::Hold => counts.hold += 1,
            _ => counts.no_call += 1,
        }
    }
    Ok(Review {
        horizon,
        base_currency: a.base_currency.clone(),
        value: round(a.value, 2),
        counts,
        holdings: rows,
        warnings: report.warnings.clone(),
        methodology: action::METHODOLOGY,
        disclaimer: action::DISCLAIMER,
    })
}

/// EXIT, TRIM, ADD, HOLD, NO CALL; the larger weight first within a kind.
fn sort_review(rows: &mut [ReviewRow]) {
    rows.sort_by(|a, b| {
        a.action
            .kind
            .review_rank()
            .cmp(&b.action.kind.review_rank())
            .then(
                b.weight_pct
                    .unwrap_or(0.0)
                    .total_cmp(&a.weight_pct.unwrap_or(0.0)),
            )
            .then(a.symbol.cmp(&b.symbol))
    });
}

/// The text of `portfolio review`.
pub(crate) fn render_review(r: &Review) -> String {
    let mut o = String::new();
    let c = &r.counts;
    o.push_str(&format!(
        "  {} exit · {} trim · {} add · {} hold · {} no call   ({} horizon, methodology {})\n\n",
        c.exit,
        c.trim,
        c.add,
        c.hold,
        c.no_call,
        r.horizon.as_str(),
        r.methodology
    ));
    for row in &r.holdings {
        let weight = row
            .weight_pct
            .map_or(String::new(), |w| format!("  {w:.1}% of portfolio"));
        let pnl = row
            .pnl_pct
            .map_or(String::new(), |p| format!("  P&L {p:+.1}%"));
        let name = row.name.as_deref().unwrap_or_default();
        o.push_str(&format!(
            "  {}  {}{weight}{pnl}\n",
            row.symbol.bold(),
            name.dimmed()
        ));
        let block = crate::verdict::render_action(&row.action);
        for line in block.lines() {
            // The disclaimer is printed once, at the end.
            if line.trim_start().starts_with("Methodology") {
                continue;
            }
            o.push_str("  ");
            o.push_str(line);
            o.push('\n');
        }
        o.push('\n');
    }
    for w in &r.warnings {
        o.push_str(&format!("  {} {w}\n", "!".yellow()));
    }
    o.push_str(&format!("  {}\n", r.disclaimer.dimmed().italic()));
    o
}

/// `stockwise portfolio review`.
pub async fn cmd_portfolio_review(
    market: Market,
    base: Option<&str>,
    horizon: Horizon,
    fmt: Format,
) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        bail!("{}", t(Msg::PortfolioEmpty));
    }
    let client = YahooClient::new().await?;
    let src = views::LiveSource::new();
    if fmt.is_table() {
        eprintln!(
            "  {} Reviewing {} holdings (prices, a year of daily bars, look-through)...",
            "⟳".yellow(),
            portfolio.holdings.len()
        );
    }
    let r = review(&src, Some(&client), market, base, horizon).await?;
    match fmt {
        Format::Json => output::print_json(&r),
        Format::Csv => output::print_csv(&review_csv(&r)),
        Format::Table => {
            print_header(&format!("Portfolio review ({} horizon)", horizon.as_str()));
            print!("{}", render_review(&r));
            Ok(())
        }
    }
}

/// One `portfolio review --csv` row.
#[derive(Debug, Serialize)]
pub(crate) struct ReviewCsv {
    symbol: String,
    action: &'static str,
    confidence: &'static str,
    weight_pct: Option<f64>,
    pnl_pct: Option<f64>,
    invalidation_price: Option<f64>,
    review_by: Option<String>,
    size_shares: Option<f64>,
    reasons: String,
    tax_note: Option<String>,
    disclaimer: &'static str,
}

impl CsvRow for ReviewCsv {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "action",
        "confidence",
        "weight_pct",
        "pnl_pct",
        "invalidation_price",
        "review_by",
        "size_shares",
        "reasons",
        "tax_note",
        "disclaimer",
    ];
}

fn review_csv(r: &Review) -> Vec<ReviewCsv> {
    r.holdings
        .iter()
        .map(|row| ReviewCsv {
            symbol: row.symbol.clone(),
            action: row.action.label,
            confidence: row.action.confidence.label(),
            weight_pct: row.weight_pct,
            pnl_pct: row.pnl_pct,
            invalidation_price: row.action.invalidation_price,
            review_by: row.action.review_by.clone(),
            size_shares: row.action.size_hint.as_ref().and_then(|s| s.shares),
            reasons: row.action.reasons.join("; "),
            tax_note: row.action.tax_note.clone(),
            disclaimer: row.action.disclaimer,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::testing::assert_action_well_formed;

    fn row(sym: &str, kind: Kind, weight: f64) -> ReviewRow {
        let mut a = Action::undecided("INR");
        a.kind = kind;
        a.label = kind.label();
        ReviewRow {
            symbol: sym.into(),
            name: None,
            shares: 1.0,
            price: Some(1.0),
            currency: Some("INR".into()),
            weight_pct: Some(weight),
            pnl_pct: None,
            action: a,
        }
    }

    #[test]
    fn review_is_sorted_exit_trim_add_hold() {
        let mut rows = vec![
            row("A", Kind::Hold, 5.0),
            row("B", Kind::NoCall, 9.0),
            row("C", Kind::Add, 3.0),
            row("D", Kind::Exit, 1.0),
            row("E", Kind::Trim, 12.0),
            row("F", Kind::Hold, 8.0),
        ];
        sort_review(&mut rows);
        let order: Vec<&str> = rows.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(order, ["D", "E", "C", "F", "A", "B"]);
    }

    #[test]
    fn review_csv_has_its_columns() {
        let r = Review {
            horizon: Horizon::Short,
            base_currency: "INR".into(),
            value: 1.0,
            counts: ReviewCounts::default(),
            holdings: vec![row("A", Kind::NoCall, 1.0)],
            warnings: vec![],
            methodology: action::METHODOLOGY,
            disclaimer: action::DISCLAIMER,
        };
        let rows = review_csv(&r);
        let (missing, extra) = crate::output::missing_columns(&rows[0]);
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{missing:?} {extra:?}"
        );
        assert_eq!(rows[0].disclaimer, action::DISCLAIMER);
        assert_action_well_formed(&serde_json::to_string(&r).unwrap());
        assert_action_well_formed(&render_review(&r));
    }

    #[test]
    fn next_long_term_finds_the_earliest_future_lot() {
        let mut p = Portfolio::default();
        for (q, price, date) in [
            (10.0, 100.0, "2025-12-01"),
            (5.0, 120.0, "2026-01-10"),
            (5.0, 90.0, "2024-01-10"),
        ] {
            p.add_lot("TCS.NS", pf::Lot::new(q, price, date, "manual"));
        }
        let h = p.holding("TCS.NS").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let (days, on, saves) = next_long_term(h, 150.0, Some("INR"), Market::In, today).unwrap();
        assert_eq!(on, NaiveDate::from_ymd_opt(2026, 12, 2).unwrap());
        assert_eq!(days, 11);
        // 10 x 50 gain x (20% - 12.5%).
        assert!((saves.unwrap() - 37.5).abs() < 1e-9, "{saves:?}");

        // Exact minor-unit arithmetic avoids penny drift (`100.25 - 100.15` -> ₹0.08).
        let mut p2 = Portfolio::default();
        p2.add_lot("TCS.NS", pf::Lot::new(10.0, 100.15, "2025-12-01", "manual"));
        let h2 = p2.holding("TCS.NS").unwrap();
        let (_, _, drift_free) =
            next_long_term(h2, 100.25, Some("INR"), Market::In, today).unwrap();
        assert_eq!(drift_free, Some(0.08));
    }
}
