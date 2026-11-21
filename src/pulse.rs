//! What moved in the portfolio and watchlist today, and why: the shared
//! data behind `stockwise today`, `stockwise hot`, the after-close digest,
//! the Telegram bot and the MCP `today` tool.
//!
//! For each holding and watchlist name: the day move, the day P&L in the
//! portfolio's base currency, the move in excess of the benchmark in units
//! of the stock's own excess-return volatility (σ), the volume against its
//! 20-day average, fresh headlines and filings, dates in the next few days,
//! and a one-line "why": the most important fresh headline or filing, "with
//! the market" for an ordinary move, or "unexplained" for an unusual move
//! (the same test `events` uses) with no news behind it.

use crate::api::{Quote, YahooClient};
use crate::market::Market;
use crate::news::NewsItem;
use crate::research::Series;
use serde::Serialize;

/// Why a stock moved.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Why {
    /// A fresh headline or filing.
    News {
        title: String,
        source: String,
        official: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        link: Option<String>,
    },
    /// An ordinary move, in line with the benchmark.
    Market { bench_pct: f64 },
    /// An unusual move with no news found.
    Unexplained { sigma: f64 },
    /// A small move with no news.
    Quiet,
}

impl Why {
    /// One line for tables and messages.
    pub fn line(&self) -> String {
        match self {
            Why::News {
                title,
                source,
                official,
                ..
            } => {
                let t = clip(title, 90);
                if *official {
                    format!("[{source}] {t}")
                } else {
                    t
                }
            }
            Why::Market { bench_pct } => format!("with the market ({bench_pct:+.1}%)"),
            Why::Unexplained { sigma } => {
                format!("unexplained — {sigma:.1}σ move, no news or filing found")
            }
            Why::Quiet => "little changed, no news".into(),
        }
    }
}

/// `text` cut to `max` characters with an ellipsis.
pub fn clip(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        text
    } else {
        format!("{}…", text.chars().take(max).collect::<String>().trim_end())
    }
}

/// One holding or watchlist name.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub held: bool,
    /// Share of the portfolio value (0-1); 0 for watchlist names.
    pub weight: f64,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub change_pct: Option<f64>,
    /// Day P&L in the base currency (holdings only).
    pub day_pnl: Option<f64>,
    /// The benchmark's day change, in percent.
    pub bench_pct: Option<f64>,
    /// Excess move over the benchmark in σ of the stock's excess returns.
    pub sigma: Option<f64>,
    /// Volume against the 20-day average.
    pub volume_x: Option<f64>,
    /// The day's open against the previous close, in percent.
    pub gap_pct: Option<f64>,
    /// Price against the 52-week range: `high` at or above the high,
    /// `low` at or below the low.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_break: Option<&'static str>,
    pub why: Why,
    /// Fresh items, most important first (at most 3).
    pub news: Vec<NewsItem>,
    /// Dates in the next few days.
    pub events: Vec<crate::calendar::Event>,
    /// The rule-based action, once the actions engine is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

/// An index for the market backdrop.
#[derive(Debug, Clone, Serialize)]
pub struct IndexMove {
    pub symbol: String,
    pub name: String,
    pub price: Option<f64>,
    pub change_pct: Option<f64>,
}

/// Everything `today` shows.
#[derive(Debug, Clone, Serialize)]
pub struct Pulse {
    /// Unix seconds.
    pub as_of: i64,
    pub base_currency: String,
    pub value: Option<f64>,
    pub day_pnl: Option<f64>,
    pub day_pnl_pct: Option<f64>,
    pub rows: Vec<Row>,
    pub backdrop: Vec<IndexMove>,
    pub warnings: Vec<String>,
}

impl Pulse {
    pub fn held(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.held)
    }

    pub fn watched(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| !r.held)
    }

    /// Holdings sorted by day P&L, biggest gain first.
    pub fn by_impact(&self) -> Vec<&Row> {
        let mut v: Vec<&Row> = self.held().collect();
        v.sort_by(|a, b| {
            b.day_pnl
                .unwrap_or(0.0)
                .total_cmp(&a.day_pnl.unwrap_or(0.0))
        });
        v
    }

    /// The `n` largest gains and losses in P&L terms.
    pub fn contributors(&self, n: usize) -> (Vec<&Row>, Vec<&Row>) {
        let sorted = self.by_impact();
        let up: Vec<&Row> = sorted
            .iter()
            .filter(|r| r.day_pnl.is_some_and(|p| p > 0.0))
            .take(n)
            .copied()
            .collect();
        let down: Vec<&Row> = sorted
            .iter()
            .rev()
            .filter(|r| r.day_pnl.is_some_and(|p| p < 0.0))
            .take(n)
            .copied()
            .collect();
        (up, down)
    }
}

/// The benchmark index for a symbol's market.
pub fn benchmark(symbol: &str) -> &'static str {
    if symbol.ends_with(".NS") || symbol.ends_with(".BO") {
        "^NSEI"
    } else {
        "^GSPC"
    }
}

/// Indices for the backdrop of `market`.
pub fn backdrop_symbols(market: Market) -> &'static [(&'static str, &'static str)] {
    match market {
        Market::In => &[
            ("^NSEI", "NIFTY 50"),
            ("^BSESN", "SENSEX"),
            ("^NSEBANK", "BANK NIFTY"),
            ("^CNXSC", "NIFTY SMALLCAP 100"),
            ("^INDIAVIX", "INDIA VIX"),
        ],
        _ => &[
            ("^GSPC", "S&P 500"),
            ("^IXIC", "NASDAQ"),
            ("^DJI", "DOW"),
            ("^RUT", "RUSSELL 2000"),
            ("^VIX", "VIX"),
        ],
    }
}

/// σ of today's excess move and the volume multiple, from daily bars of
/// the stock and its benchmark.
pub fn move_stats(p: &Series, b: &Series) -> (Option<f64>, Option<f64>) {
    let (sigma, volume_x, _) = move_stats_with_sd(p, b);
    (sigma, volume_x)
}

/// Like [`move_stats`], also returning the prior excess-return standard
/// deviation (`excess_sd`) so callers do not reconstruct it from mismatched
/// quote and chart returns.
pub fn move_stats_with_sd(p: &Series, b: &Series) -> (Option<f64>, Option<f64>, Option<f64>) {
    let excess = crate::events::excess_returns(p, b);
    let sd = match (excess.split_last(), crate::events::excess_sd(&excess)) {
        (Some((_, prior)), Some(_)) if !prior.is_empty() => {
            crate::events::excess_sd(prior).filter(|sd| *sd > 0.0)
        }
        _ => None,
    };
    let sigma = match (excess.split_last(), sd) {
        (Some(((_, last), _)), Some(sd)) => Some(last / sd),
        _ => None,
    };
    let n = p.volume.len();
    let volume_x = (n > 21)
        .then(|| {
            let avg = p.volume[n - 21..n - 1].iter().sum::<f64>() / 20.0;
            (avg > 0.0 && p.volume[n - 1] > 0.0).then(|| p.volume[n - 1] / avg)
        })
        .flatten();
    (sigma, volume_x, sd)
}

/// The one-line reason for a move. Pure: `news` are fresh items for the
/// symbol, most important first.
pub fn explain(
    change_pct: Option<f64>,
    bench_pct: Option<f64>,
    sigma: Option<f64>,
    volume_x: Option<f64>,
    news: &[NewsItem],
) -> Why {
    explain_with_sd(change_pct, bench_pct, sigma, None, volume_x, news)
}

/// Like [`explain`], using `excess_sd` directly when known.
pub fn explain_with_sd(
    change_pct: Option<f64>,
    bench_pct: Option<f64>,
    sigma: Option<f64>,
    excess_sd: Option<f64>,
    volume_x: Option<f64>,
    news: &[NewsItem],
) -> Why {
    // A filing first, then any headline of some weight — a headline only
    // for a move worth explaining (1% or 1σ), so a flat day isn't pinned on
    // whatever was published.
    let notable =
        change_pct.is_some_and(|c| c.abs() >= 1.0) || sigma.is_some_and(|z| z.abs() >= 1.0);
    let best = news
        .iter()
        .find(|n| n.official && n.importance >= 1.5)
        .or_else(|| news.iter().find(|n| notable && n.importance >= 2.0));
    if let Some(n) = best {
        return Why::News {
            title: n.title.clone(),
            source: if n.official {
                n.source.clone()
            } else {
                n.publisher.clone()
            },
            official: n.official,
            link: n.link.clone(),
        };
    }
    let Some(change) = change_pct else {
        return Why::Quiet;
    };
    // The same test `events` uses for an unusual move: 2.5σ (at least 3%)
    // in excess of the benchmark, or a 1.5% move on 3× volume.
    if let Some(sd) = excess_sd.filter(|s| s.is_finite() && *s > 0.0) {
        let excess = (change - bench_pct.unwrap_or(0.0)) / 100.0;
        if crate::events::is_unusual_move(excess, sd, volume_x) {
            return Why::Unexplained {
                sigma: (excess / sd).abs(),
            };
        }
    } else if let Some(z) = sigma {
        let sd = if z != 0.0 {
            ((change - bench_pct.unwrap_or(0.0)) / 100.0 / z).abs()
        } else {
            f64::INFINITY
        };
        let excess = (change - bench_pct.unwrap_or(0.0)) / 100.0;
        if sd.is_finite() && crate::events::is_unusual_move(excess, sd, volume_x) {
            return Why::Unexplained { sigma: z.abs() };
        }
    }
    match bench_pct {
        Some(b) if change.abs() >= 0.5 && change.signum() == b.signum() && b.abs() >= 0.3 => {
            Why::Market { bench_pct: b }
        }
        _ => news
            .first()
            .filter(|_| notable)
            .map(|n| Why::News {
                title: n.title.clone(),
                source: n.publisher.clone(),
                official: n.official,
                link: n.link.clone(),
            })
            .unwrap_or(Why::Quiet),
    }
}

/// What to gather.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// News window, in days.
    pub days: u32,
    pub watchlist: bool,
    /// Fetch headlines and filings.
    pub news: bool,
    /// Look up dates in the next `event_days` days.
    pub events: bool,
    pub event_days: i64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            days: 2,
            watchlist: true,
            news: true,
            events: true,
            event_days: 3,
        }
    }
}

fn quote_for<'a>(quotes: &'a [Quote], symbol: &str) -> Option<&'a Quote> {
    quotes.iter().find(|q| {
        q.symbol
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(symbol))
    })
}

/// Gather the pulse for the portfolio (and watchlist).
pub async fn gather(client: &YahooClient, market: Market, opts: Options) -> anyhow::Result<Pulse> {
    let portfolio = crate::portfolio::Portfolio::load()?;
    let watchlist = if opts.watchlist {
        crate::watchlist::Watchlist::load()
            .map(|w| w.symbols)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let held: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let mut symbols = held.clone();
    for w in watchlist {
        if !symbols.contains(&w) {
            symbols.push(w);
        }
    }
    let mut warnings = Vec::new();
    // Actions (`crate::action`) for each row: quote-only rules, the same as
    // the Action column of `portfolio`.
    let notes = crate::commands::load_notes();
    let profile = crate::config::effective_profile();
    let today = chrono::Local::now().date_naive();
    let base = crate::commands::market_currency(market);
    let val = crate::commands::value_holdings(client, &symbols, market, Some(base)).await?;
    let totals = val.totals(&portfolio);
    let value = (totals.value > 0.0).then_some(totals.value);
    if !totals.unpriced.is_empty() {
        warnings.push(format!("No price for {}", totals.unpriced.join(", ")));
    }
    // The bulk quote has no ROE, margins or debt: complete the holdings'
    // quotes first, or every holding's action would be NO CALL (coverage),
    // unlike `portfolio review`.
    let mut enriched = crate::commands::enriched_quotes(client, &portfolio, &val).await;
    let watched: Vec<Quote> = symbols
        .iter()
        .filter(|s| !held.contains(*s) && !crate::offmarket::is_off_market(s))
        .filter_map(|s| val.quote(s).cloned())
        .collect();
    enriched.extend(crate::commands::enrich_quotes(client, watched).await);

    // Benchmarks and backdrop.
    let mut markets: Vec<Market> = symbols
        .iter()
        .filter_map(|s| crate::alerts::alert_market(s))
        .filter(|m| matches!(m, Market::In | Market::Us))
        .collect();
    markets.push(market);
    markets.sort_by_key(|m| m.id());
    markets.dedup();
    let mut backdrop_syms: Vec<(&str, &str)> = Vec::new();
    for m in &markets {
        backdrop_syms.extend(backdrop_symbols(*m));
    }
    let refs: Vec<&str> = backdrop_syms.iter().map(|(s, _)| *s).collect();
    let index_quotes = client.get_quote(&refs).await.unwrap_or_default();
    let backdrop: Vec<IndexMove> = backdrop_syms
        .iter()
        .map(|(s, name)| {
            let q = quote_for(&index_quotes, s);
            IndexMove {
                symbol: s.to_string(),
                name: name.to_string(),
                price: q.and_then(|q| q.regular_market_price),
                change_pct: q.and_then(|q| q.regular_market_change_percent),
            }
        })
        .collect();
    let bench_pct = |sym: &str| {
        backdrop
            .iter()
            .find(|i| i.symbol == benchmark(sym))
            .and_then(|i| i.change_pct)
    };

    // Daily bars: the stock's and each benchmark's.
    let mut bench_bars: Vec<(&str, Series)> = Vec::new();
    for b in ["^NSEI", "^GSPC"] {
        if symbols.iter().any(|s| benchmark(s) == b)
            && let Ok(c) = client.get_chart(b, "1y", "1d").await
        {
            bench_bars.push((b, Series::from_chart(&c)));
        }
    }
    let stock_syms: Vec<String> = symbols
        .iter()
        .filter(|s| !crate::market::is_namespaced(s))
        .cloned()
        .collect();
    // In chunks with `join_all` rather than a buffered stream: the stream's
    // borrowing closures make the future fail `Send` inside `tokio::spawn`.
    let mut bars: Vec<(String, Option<Series>)> = Vec::new();
    for chunk in stock_syms.chunks(8) {
        let futs: Vec<_> = chunk
            .iter()
            .map(|s| async move {
                let c = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    client.get_chart(s, "1y", "1d"),
                )
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|c| Series::from_chart(&c));
                (s.clone(), c)
            })
            .collect();
        bars.extend(futures_util::future::join_all(futs).await);
    }

    // News.
    let mut news_items: Vec<NewsItem> = Vec::new();
    if opts.news {
        let entries: Vec<crate::news::FeedEntry> = stock_syms
            .iter()
            .map(|s| crate::news::FeedEntry {
                symbol: s.clone(),
                company: val.name(s).or_else(|| {
                    val.quote(s)
                        .and_then(|q| q.long_name.clone().or(q.short_name.clone()))
                }),
                weight: 0.0,
            })
            .collect();
        let (items, failures) = crate::news::feed(client, &entries, opts.days, true).await;
        news_items = items;
        if !failures.is_empty() && news_items.is_empty() {
            warnings.push(format!("News sources failed: {}", failures.len()));
        }
    }

    // Dates coming up.
    let mut events: Vec<crate::calendar::Event> = Vec::new();
    if opts.events {
        let mut cal = crate::calendar::Calendar::new(client);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
        for s in &stock_syms {
            if tokio::time::Instant::now() >= deadline {
                warnings.push("Calendar lookups stopped after 25 s".into());
                break;
            }
            events.extend(cal.events_for(s).await);
        }
        cal.save();
        let now = chrono::Utc::now();
        events = crate::calendar::upcoming(events, opts.event_days.max(0) as u32, |s| {
            crate::calendar::exchange_today(s, now)
        });
    }

    let mut rows = Vec::new();
    for s in &symbols {
        let q = val.quote(s);
        let is_held = held.contains(s);
        let h = portfolio.holding(s);
        let price = val.price(s);
        let change_pct = q.and_then(|q| q.regular_market_change_percent);
        let prev = q.and_then(|q| {
            q.regular_market_previous_close
                .or_else(|| Some(q.regular_market_price? - q.regular_market_change?))
        });
        let day_pnl = match (is_held, h, price, prev) {
            (true, Some(h), Some(p), Some(pc)) if pc > 0.0 => val.to_base(s, h.shares * (p - pc)),
            _ => None,
        };
        let weight = match (h, price, value) {
            (Some(h), Some(p), Some(v)) if v > 0.0 => {
                val.to_base(s, h.shares * p).map(|x| x / v).unwrap_or(0.0)
            }
            _ => 0.0,
        };
        let series = bars
            .iter()
            .find(|(x, _)| x == s)
            .and_then(|(_, b)| b.as_ref());
        let bench = bench_bars
            .iter()
            .find(|(b, _)| *b == benchmark(s))
            .map(|(_, b)| b);
        let (sigma, volume_x, excess_sd) = match (series, bench) {
            (Some(p), Some(b)) => move_stats_with_sd(p, b),
            (Some(p), None) => move_stats_with_sd(p, &Series::default()),
            _ => (None, None, None),
        };
        let gap_pct = q.and_then(|q| {
            let open = q.regular_market_open.filter(|o| *o > 0.0)?;
            let pc = prev.filter(|p| *p > 0.0)?;
            Some((open / pc - 1.0) * 100.0)
        });
        let range_break = q.and_then(|q| {
            let p = q.regular_market_price?;
            if q.fifty_two_week_high.is_some_and(|h| p >= h * 0.999) {
                Some("high")
            } else if q.fifty_two_week_low.is_some_and(|l| p <= l * 1.001) {
                Some("low")
            } else {
                None
            }
        });
        let mut news: Vec<NewsItem> = news_items
            .iter()
            .filter(|n| n.symbol.as_deref() == Some(s.as_str()))
            .cloned()
            .collect();
        news.sort_by(|a, b| b.importance.total_cmp(&a.importance));
        news.truncate(3);
        let bp = bench_pct(s);
        let why = explain_with_sd(change_pct, bp, sigma, excess_sd, volume_x, &news);
        rows.push(Row {
            symbol: s.clone(),
            name: val.name(s).or_else(|| q.and_then(|q| q.short_name.clone())),
            held: is_held,
            weight,
            price,
            currency: val.currency(s),
            change_pct,
            day_pnl,
            bench_pct: bp,
            sigma,
            volume_x,
            gap_pct,
            range_break,
            why,
            news,
            events: events.iter().filter(|e| &e.symbol == s).cloned().collect(),
            action: enriched.get(s).or(q).map(|q| {
                crate::commands::action_line(
                    q,
                    h,
                    is_held.then_some(weight * 100.0),
                    portfolio.targets.get(s).copied(),
                    &notes,
                    &profile,
                    today,
                )
            }),
        });
    }
    let day_pnl: Option<f64> = {
        let v: Vec<f64> = rows.iter().filter_map(|r| r.day_pnl).collect();
        (!v.is_empty()).then(|| v.iter().sum())
    };
    let day_pnl_pct = match (day_pnl, value) {
        (Some(p), Some(v)) if v - p > 0.0 => Some(p / (v - p) * 100.0),
        _ => None,
    };
    Ok(Pulse {
        as_of: chrono::Utc::now().timestamp(),
        base_currency: val.base().to_string(),
        value,
        day_pnl,
        day_pnl_pct,
        rows,
        backdrop,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn news(title: &str, official: bool, importance: f64) -> NewsItem {
        NewsItem {
            symbol: Some("X.NS".into()),
            title: title.into(),
            publisher: "Pub".into(),
            link: None,
            ts: Some(0),
            source: if official { "NSE" } else { "Google News" }.into(),
            official,
            kind: crate::events::Kind::Other,
            importance,
        }
    }

    #[test]
    fn a_filing_explains_a_move_before_any_headline() {
        let items = [
            news("Analyst note", false, 3.0),
            news("Outcome of Board Meeting", true, 2.0),
        ];
        let w = explain(Some(-4.0), Some(0.2), Some(-3.0), None, &items);
        assert!(matches!(&w, Why::News { official: true, .. }), "{w:?}");
        assert!(w.line().starts_with("[NSE] Outcome"));
    }

    #[test]
    fn an_unusual_move_without_news_is_unexplained() {
        // −5% against a flat market, σ of excess returns 1.25% → 4σ.
        let w = explain(Some(-5.0), Some(0.0), Some(-4.0), Some(1.2), &[]);
        assert_eq!(w, Why::Unexplained { sigma: 4.0 });
        assert!(w.line().contains("unexplained"));
    }

    #[test]
    fn an_ordinary_move_with_the_market_says_so() {
        let w = explain(Some(-1.1), Some(-0.9), Some(-0.2), None, &[]);
        assert_eq!(w, Why::Market { bench_pct: -0.9 });
        assert_eq!(
            explain(Some(0.1), Some(-0.9), Some(0.5), None, &[]),
            Why::Quiet
        );
    }

    #[test]
    fn move_stats_read_sigma_and_volume() {
        let start = chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let mut p = Series::default();
        let mut b = Series::default();
        for i in 0..80 {
            let d = start + chrono::Duration::days(i);
            let wiggle = if i % 2 == 0 { 1.005 } else { 0.995 };
            p.dates.push(d);
            p.close.push(100.0 * wiggle);
            p.volume.push(1000.0);
            b.dates.push(d);
            b.close.push(100.0);
        }
        // Last day: −6% on 3× volume.
        *p.close.last_mut().unwrap() = p.close[78] * 0.94;
        *p.volume.last_mut().unwrap() = 3000.0;
        let (sigma, vol) = move_stats(&p, &b);
        assert!(sigma.unwrap() < -5.0, "{sigma:?}");
        assert!((vol.unwrap() - 3.0).abs() < 1e-9);

        // Using `excess_sd` directly classifies a live +8% move as unusual even if yesterday's z was 0.1.
        assert_eq!(
            explain_with_sd(Some(8.0), Some(0.0), Some(0.1), Some(0.01), None, &[]),
            Why::Unexplained { sigma: 8.0 }
        );
    }
}
