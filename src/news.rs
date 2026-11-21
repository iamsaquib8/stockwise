//! Headlines and exchange filings for a stock, and a merged feed across
//! holdings and the watchlist.
//!
//! Sources: Yahoo Finance search news and RSS (by ticker), Google News (by
//! company name, in the stock's own locale, with a `when:` recency window),
//! and official filings — NSE announcements for `.NS`, BSE announcements for
//! `.BO`, SEC EDGAR for US tickers. Items are de-duplicated across sources
//! (near-identical titles), ranked by importance, and carry their link and
//! a unix time (shown in the reader's local time).

use crate::api::YahooClient;
use crate::research::{Headline, url_encode};
use serde::Serialize;
use std::time::Duration;

/// Per-source timeout, so one slow feed does not hold up the rest.
const SOURCE_TIMEOUT: Duration = Duration::from_secs(8);

/// One headline or filing.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NewsItem {
    /// The holding or watchlist symbol it was fetched for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub title: String,
    pub publisher: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    /// Which feed: `Yahoo Finance`, `Yahoo RSS`, `Google News`, `NSE`,
    /// `BSE` or `SEC EDGAR`.
    pub source: String,
    /// An exchange or regulator filing.
    pub official: bool,
    /// The event category (`results`, `orders`, ...), from the title.
    pub kind: crate::events::Kind,
    /// 0-10: category, officialness and recency.
    pub importance: f64,
}

impl NewsItem {
    fn from_headline(h: Headline, source: &str) -> Self {
        let kind = crate::events::classify(&format!(
            "{} {}",
            h.category.as_deref().unwrap_or(""),
            h.title
        ));
        Self {
            symbol: None,
            title: h.title,
            publisher: h.publisher,
            link: h.link,
            ts: (h.ts > 0).then_some(h.ts),
            source: source.into(),
            official: h.official,
            kind,
            importance: 0.0,
        }
    }

    /// Local date and time, e.g. `21 Nov 16:05`.
    pub fn local_time(&self) -> Option<String> {
        let t = chrono::DateTime::from_timestamp(self.ts?, 0)?;
        Some(
            t.with_timezone(&chrono::Local)
                .format("%d %b %H:%M")
                .to_string(),
        )
    }

    /// Hours since publication at `now`.
    pub fn age_hours(&self, now: i64) -> Option<f64> {
        self.ts.map(|t| ((now - t) as f64 / 3600.0).max(0.0))
    }
}

/// How much a category matters, before recency.
fn kind_weight(k: crate::events::Kind) -> f64 {
    use crate::events::Kind;
    match k {
        Kind::Results => 3.0,
        Kind::Regulatory => 2.5,
        Kind::Orders | Kind::Deal | Kind::CorporateAction | Kind::Insider => 2.0,
        Kind::Analyst | Kind::Management | Kind::Capital => 1.5,
        Kind::Product | Kind::PriceMove => 1.0,
        Kind::Other => 0.5,
    }
}

/// Importance at `now`: category, +1.5 for a filing, and a recency factor
/// that halves every 24 hours.
pub fn importance(item: &NewsItem, now: i64) -> f64 {
    let base = kind_weight(item.kind) + if item.official { 1.5 } else { 0.0 } + 1.0;
    let recency = item
        .age_hours(now)
        .map_or(0.3, |h| 0.5f64.powf(h / 24.0));
    (base * recency * 2.0).min(10.0)
}

/// Words that identify a title, for de-duplication.
fn fingerprint(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3)
        .map(str::to_lowercase)
        .collect()
}

/// Remove near-duplicates (over 60% of the shorter title's words shared),
/// keeping the official or earlier-seen copy, then sort newest first.
pub fn collate(mut items: Vec<NewsItem>) -> Vec<NewsItem> {
    // Filings first so they win ties; then newest first.
    items.sort_by_key(|i| (!i.official, std::cmp::Reverse(i.ts.unwrap_or(0))));
    let mut seen: Vec<Vec<String>> = Vec::new();
    let mut out: Vec<NewsItem> = Vec::new();
    for item in items {
        let words = fingerprint(&item.title);
        let dup = seen.iter().any(|w| {
            if w.is_empty() || words.is_empty() {
                return false;
            }
            let shared = words.iter().filter(|x| w.contains(x)).count();
            // Short titles ("Board approves dividend" / "... buyback") need
            // three shared words before they count as the same story.
            shared >= 3.min(words.len().min(w.len()))
                && shared as f64 / words.len().min(w.len()) as f64 > 0.6
                && (shared >= 3 || words == *w)
        });
        let same_link = item
            .link
            .as_ref()
            .is_some_and(|l| out.iter().any(|o| o.link.as_ref() == Some(l)));
        if !dup && !same_link {
            seen.push(words);
            out.push(item);
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.ts.unwrap_or(0)));
    out
}

/// Whether `symbol` is an Indian listing (NSE or BSE).
fn is_india(symbol: &str) -> bool {
    symbol.ends_with(".NS") || symbol.ends_with(".BO")
}

/// What to fetch for one symbol.
#[derive(Debug, Clone)]
pub struct Query {
    pub symbol: String,
    /// Company name for Google News (the ticker alone matches noise).
    pub company: Option<String>,
    /// Recency window in days (Google's `when:` and the filing range).
    pub days: u32,
    /// Also fetch exchange / SEC filings.
    pub filings: bool,
}

/// Items for one symbol, and how many sources answered out of how many
/// were asked.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Fetched {
    pub items: Vec<NewsItem>,
    pub sources_ok: usize,
    pub sources_tried: usize,
    /// Sources that failed, with the reason.
    pub failures: Vec<String>,
}

async fn timed<T>(
    f: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(SOURCE_TIMEOUT, f)
        .await
        .map_err(|_| anyhow::anyhow!("timed out"))?
}

/// Yahoo Finance search news for a ticker.
async fn yahoo_json(client: &YahooClient, symbol: &str) -> anyhow::Result<Vec<NewsItem>> {
    let url = crate::api::build_url(
        "https://query2.finance.yahoo.com/v1/finance/search",
        None,
        &[("q", symbol), ("quotesCount", "0"), ("newsCount", "12")],
    );
    let v = client.raw_get(&url).await?;
    Ok(v.get("news")
        .and_then(|n| n.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| {
                    let title = n.get("title")?.as_str()?.to_string();
                    let h = Headline {
                        ts: n.get("providerPublishTime").and_then(|t| t.as_i64()).unwrap_or(0),
                        title,
                        summary: None,
                        publisher: n
                            .get("publisher")
                            .and_then(|p| p.as_str())
                            .unwrap_or("Yahoo Finance")
                            .to_string(),
                        link: n.get("link").and_then(|l| l.as_str()).map(str::to_string),
                        official: false,
                        category: None,
                    };
                    Some(NewsItem::from_headline(h, "Yahoo Finance"))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Yahoo Finance RSS for a ticker, in the stock's region.
async fn yahoo_rss(client: &YahooClient, symbol: &str) -> anyhow::Result<Vec<NewsItem>> {
    let (region, lang) = if is_india(symbol) {
        ("IN", "en-IN")
    } else {
        ("US", "en-US")
    };
    let url = format!(
        "https://feeds.finance.yahoo.com/rss/2.0/headline?s={}&region={region}&lang={lang}",
        url_encode(symbol)
    );
    let xml = client.raw_get_text(&url).await?;
    Ok(crate::research::parse_rss(&xml, "Yahoo Finance")
        .into_iter()
        .map(|h| NewsItem::from_headline(h, "Yahoo RSS"))
        .collect())
}

/// The Google News search URL for a company: quoted name, `when:` window,
/// the stock's own locale.
pub fn google_news_url(company: &str, india: bool, days: u32) -> String {
    let q = if india {
        format!("\"{company}\" when:{days}d")
    } else {
        format!("\"{company}\" stock when:{days}d")
    };
    let locale = if india {
        "hl=en-IN&gl=IN&ceid=IN:en"
    } else {
        "hl=en-US&gl=US&ceid=US:en"
    };
    format!(
        "https://news.google.com/rss/search?q={}&{locale}",
        url_encode(&q)
    )
}

async fn google(
    client: &YahooClient,
    company: &str,
    india: bool,
    days: u32,
) -> anyhow::Result<Vec<NewsItem>> {
    let xml = client
        .raw_get_text(&google_news_url(company, india, days))
        .await?;
    Ok(crate::research::parse_rss(&xml, "Google News")
        .into_iter()
        .map(|h| NewsItem::from_headline(h, "Google News"))
        .collect())
}

/// Exchange or regulator filings of `symbol` over `days` days: NSE for
/// `.NS`, BSE for `.BO`, SEC EDGAR for US tickers. Empty for others.
pub async fn filings(
    client: &YahooClient,
    symbol: &str,
    days: i64,
) -> anyhow::Result<Vec<Headline>> {
    if let Some(base) = symbol.strip_suffix(".NS") {
        crate::research::fetch_nse_filings(client, base, days).await
    } else if symbol.ends_with(".BO") {
        let list = client.bse_announcements(symbol, days).await?;
        Ok(list
            .into_iter()
            .map(|a| Headline {
                // BSE gives a date: count it as filed at 17:00 IST.
                ts: a
                    .date
                    .and_hms_opt(11, 30, 0)
                    .map_or(0, |d| d.and_utc().timestamp()),
                title: if a.headline.is_empty() {
                    a.category.clone()
                } else {
                    a.headline
                },
                summary: None,
                publisher: "BSE filing".into(),
                link: a.attachment,
                official: true,
                category: (!a.category.is_empty()).then_some(a.category),
            })
            .collect())
    } else if crate::alerts::alert_market(symbol) == Some(crate::market::Market::Us) {
        crate::providers::edgar::filings::recent(client, symbol, days).await
    } else {
        Ok(Vec::new())
    }
}

fn filing_source(symbol: &str) -> &'static str {
    if symbol.ends_with(".NS") {
        "NSE"
    } else if symbol.ends_with(".BO") {
        "BSE"
    } else {
        "SEC EDGAR"
    }
}

/// Every source for one symbol at once; failures are counted, not fatal.
pub async fn fetch(client: &YahooClient, q: &Query) -> Fetched {
    let company = q
        .company
        .as_deref()
        .map(crate::research::clean_company_name)
        .filter(|c| !c.is_empty());
    let india = is_india(&q.symbol);
    let base = q.symbol.split('.').next().unwrap_or(&q.symbol).to_string();
    let wants_filings = q.filings && (india || crate::alerts::alert_market(&q.symbol) == Some(crate::market::Market::Us));
    let (yj, yr, gn, fl) = tokio::join!(
        timed(yahoo_json(client, &q.symbol)),
        timed(yahoo_rss(client, &q.symbol)),
        async {
            match &company {
                Some(c) => Some(timed(google(client, c, india, q.days.max(1))).await),
                // No name: the bare ticker with "stock" is still better than nothing.
                None => Some(timed(google(client, &base, india, q.days.max(1))).await),
            }
        },
        async {
            if wants_filings {
                Some(timed(filings(client, &q.symbol, q.days.max(1) as i64)).await)
            } else {
                None
            }
        },
    );
    let mut out = Fetched::default();
    let mut take = |name: &str, r: Option<anyhow::Result<Vec<NewsItem>>>| {
        let Some(r) = r else { return };
        out.sources_tried += 1;
        match r {
            Ok(items) => {
                out.sources_ok += 1;
                out.items.extend(items);
            }
            Err(e) => out.failures.push(format!("{name}: {e:#}")),
        }
    };
    take("Yahoo Finance", Some(yj));
    take("Yahoo RSS", Some(yr));
    take("Google News", gn);
    let src = filing_source(&q.symbol);
    take(
        src,
        fl.map(|r| {
            r.map(|v| {
                v.into_iter()
                    .filter(|h| !crate::events::is_routine_filing(h))
                    .map(|h| NewsItem::from_headline(h, src))
                    .collect()
            })
        }),
    );
    let now = chrono::Utc::now().timestamp();
    let cutoff = now - i64::from(q.days.max(1)) * 86_400;
    for item in &mut out.items {
        item.symbol = Some(q.symbol.clone());
        item.importance = importance(item, now);
    }
    out.items.retain(|i| i.ts.is_none_or(|t| t >= cutoff));
    out.items = collate(std::mem::take(&mut out.items));
    out
}

/// A symbol in a feed and how much it matters to the reader: its
/// portfolio weight (0-1); watchlist names count a little.
#[derive(Debug, Clone)]
pub struct FeedEntry {
    pub symbol: String,
    pub company: Option<String>,
    pub weight: f64,
}

/// Watchlist names rank as if they were this share of the portfolio.
pub const WATCHLIST_WEIGHT: f64 = 0.02;

/// Rank items by weight × importance, de-duplicated, newest first among
/// equals. Pure.
pub fn rank_feed(items: Vec<NewsItem>, weights: &[(String, f64)]) -> Vec<NewsItem> {
    let weight = |s: &Option<String>| {
        s.as_ref()
            .and_then(|s| weights.iter().find(|(w, _)| w == s).map(|(_, w)| *w))
            .unwrap_or(WATCHLIST_WEIGHT)
    };
    let mut items = collate(items);
    // A tiny weight still lets a filing of a small holding beat noise.
    let score = |i: &NewsItem| i.importance * (0.2 + weight(&i.symbol).sqrt());
    items.sort_by(|a, b| {
        score(b)
            .total_cmp(&score(a))
            .then(b.ts.unwrap_or(0).cmp(&a.ts.unwrap_or(0)))
    });
    items
}

/// The merged feed for `entries`, at most `concurrency` symbols at a time.
pub async fn feed(
    client: &YahooClient,
    entries: &[FeedEntry],
    days: u32,
    filings: bool,
) -> (Vec<NewsItem>, Vec<String>) {
    let queries: Vec<Query> = entries
        .iter()
        .map(|e| Query {
            symbol: e.symbol.clone(),
            company: e.company.clone(),
            days,
            filings,
        })
        .collect();
    // Six symbols at a time, with `join_all` (a buffered stream of borrowing
    // closures is not `Send` inside `tokio::spawn`).
    let mut results: Vec<Fetched> = Vec::new();
    for chunk in queries.chunks(6) {
        let futs: Vec<_> = chunk.iter().map(|q| fetch(client, q)).collect();
        results.extend(futures_util::future::join_all(futs).await);
    }
    let mut items = Vec::new();
    let mut failures = Vec::new();
    for r in results {
        items.extend(r.items);
        failures.extend(r.failures);
    }
    let weights: Vec<(String, f64)> = entries
        .iter()
        .map(|e| (e.symbol.clone(), e.weight))
        .collect();
    (rank_feed(items, &weights), failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, ts: i64, official: bool, symbol: &str) -> NewsItem {
        let mut i = NewsItem::from_headline(
            Headline {
                ts,
                title: title.into(),
                summary: None,
                publisher: "P".into(),
                link: None,
                official,
                category: None,
            },
            "Test",
        );
        i.symbol = Some(symbol.into());
        i
    }

    #[test]
    fn google_news_uses_the_company_name_locale_and_window() {
        let u = google_news_url("Reliance Industries", true, 2);
        assert!(u.contains("%22Reliance%20Industries%22") || u.contains("%22Reliance+Industries%22"), "{u}");
        assert!(u.contains("when%3A2d"), "{u}");
        assert!(u.ends_with("hl=en-IN&gl=IN&ceid=IN:en"));
        assert!(google_news_url("Apple", false, 1).contains("gl=US"));
    }

    #[test]
    fn duplicates_merge_and_filings_win() {
        let items = vec![
            item("Reliance Q2 profit rises 9% on retail strength", 100, false, "R"),
            item("Reliance Q2 profit rises 9%, retail strength helps", 90, false, "R"),
            item("Outcome of Board Meeting: Reliance Q2 profit rises 9% retail strength", 80, true, "R"),
            item("Unrelated headline about telecom tariffs", 70, false, "R"),
        ];
        let out = collate(items);
        assert_eq!(out.len(), 2, "{out:#?}");
        assert!(out.iter().any(|i| i.official));
    }

    #[test]
    fn importance_halves_daily_and_favours_results_and_filings() {
        let now = 1_000_000;
        let fresh = item("Q2 results: net profit up 12%", now, false, "X");
        let old = item("Q2 results: net profit up 12%", now - 48 * 3600, false, "X");
        let filed = item("Q2 results: net profit up 12%", now, true, "X");
        let noise = item("Five stocks to watch", now, false, "X");
        assert!(importance(&fresh, now) > importance(&old, now) * 3.5);
        assert!(importance(&filed, now) > importance(&fresh, now));
        assert!(importance(&fresh, now) > importance(&noise, now));
    }

    #[test]
    fn the_feed_ranks_by_weight_times_importance() {
        let mut big = item("Board approves dividend", 100, true, "BIG.NS");
        big.importance = 4.0;
        let mut small = item("Board approves buyback", 100, true, "SMALL.NS");
        small.importance = 4.0;
        let mut watch = item("Wins large defence order", 100, false, "WATCH.NS");
        watch.importance = 5.0;
        let ranked = rank_feed(
            vec![small, watch, big],
            &[("BIG.NS".into(), 0.30), ("SMALL.NS".into(), 0.01)],
        );
        let order: Vec<&str> = ranked.iter().map(|i| i.symbol.as_deref().unwrap()).collect();
        assert_eq!(order, ["BIG.NS", "WATCH.NS", "SMALL.NS"]);
    }
}
