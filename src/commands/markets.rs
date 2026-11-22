//! Market-wide views: movers, sectors, news, sentiment, screeners, rankings, and IPOs.

use super::*;

/// One stock in `movers --json/--csv`. `side` is `gainer` or `loser`;
/// `rank` starts at 1 within each side.
#[derive(Debug, Serialize)]
pub(crate) struct MoverRow {
    pub(crate) side: &'static str,
    pub(crate) rank: usize,
    pub(crate) symbol: String,
    pub(crate) price: f64,
    pub(crate) change_percent: f64,
    pub(crate) volume: Option<u64>,
    pub(crate) currency: Option<String>,
    pub(crate) cap_tier: &'static str,
}

impl CsvRow for MoverRow {
    const COLUMNS: &'static [&'static str] = &[
        "side",
        "rank",
        "symbol",
        "price",
        "change_percent",
        "volume",
        "currency",
        "cap_tier",
    ];
}

/// Top `n` risers and fallers by today's change. Only stocks with a positive
/// change enter the gainers list and only negative changes enter losers, so a
/// broad up day never pads "Top Losers" with rising names (and vice versa).
/// Quotes without a price or change are left out rather than ranked as 0%.
pub(crate) fn mover_rows(quotes: &[crate::api::Quote], n: usize) -> (Vec<MoverRow>, Vec<MoverRow>) {
    let mut ranked: Vec<(&crate::api::Quote, f64, f64)> = quotes
        .iter()
        .filter_map(|q| Some((q, q.regular_market_price?, q.regular_market_change_percent?)))
        .collect();
    ranked.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let row = |side, rank, (q, price, pct): &(&crate::api::Quote, f64, f64)| MoverRow {
        side,
        rank,
        symbol: q.symbol.clone().unwrap_or_default(),
        price: *price,
        change_percent: *pct,
        volume: q.regular_market_volume,
        currency: q.currency.clone(),
        cap_tier: market::cap_tier_of(q).as_str(),
    };
    let gainers = ranked
        .iter()
        .filter(|(_, _, pct)| *pct > 0.0)
        .take(n)
        .enumerate()
        .map(|(i, r)| row("gainer", i + 1, r))
        .collect();
    let losers = ranked
        .iter()
        .rev()
        .filter(|(_, _, pct)| *pct < 0.0)
        .take(n)
        .enumerate()
        .map(|(i, r)| row("loser", i + 1, r))
        .collect();
    (gainers, losers)
}

/// `--universe` names NSE indices: fail before any request elsewhere.
pub(super) fn check_universe(market: Market, universe: Option<market::Universe>) -> Result<()> {
    if universe.is_some() && market != Market::In {
        bail!(
            "--universe lists NSE index members; use it with the Indian market (-m in), not {}",
            market.label()
        );
    }
    Ok(())
}

/// What a scan covers: the symbols, its title and, for an NSE index, the
/// members' quotes that came with the one bulk request.
pub(super) struct Scan {
    pub(super) symbols: Vec<String>,
    pub(super) scope: String,
    bulk: Vec<crate::api::Quote>,
}

impl Scan {
    /// Whether NSE listed the members just now (not a built-in or saved list).
    fn live(&self) -> bool {
        !self.bulk.is_empty()
    }
}

/// Longest age of a `market_snapshots` row a `--cap` scan will use.
const SNAPSHOT_MAX_AGE_SECS: i64 = 30 * 60;

/// Least value traded on the last session (rupees) for a stock to be in a
/// plain India scan: ten lakh. Below it, prices move on a handful of
/// trades. `--universe all` has no floor.
const DEFAULT_MIN_TRADED_INR: f64 = 1_000_000.0;

/// Symbols a scan covers and its title.
///
/// India without `--universe`: every stock listed on BSE that traded at
/// least [`DEFAULT_MIN_TRADED_INR`] on the last session, from BSE's
/// end-of-day file; NIFTY 500 when that file is unavailable. `--universe
/// bse` / `all` take the BSE main board or every BSE stock from the same
/// file. `--universe <NSE index>` takes the index's members (one bulk NSE
/// request that also prices them; niftyindices.com's list, then the saved
/// or built-in one, when NSE can't answer). Other markets use their
/// built-in list. NSE index members are saved for the India cap tiers and
/// the prices feed `market_snapshots`.
pub(super) async fn scan_symbols(
    client: &YahooClient,
    market: Market,
    universe: Option<market::Universe>,
) -> Result<Scan> {
    match universe {
        Some(u) => match u.bse_groups() {
            Some(groups) => scan_bse(client, groups, 0.0, u.nse_index())
                .await
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "BSE's end-of-day file is unavailable (it needs cache.db and bseindia.com); \
                         try --universe nifty500"
                    )
                }),
            None => scan_nse_index(client, u.nse_index(), u.fallback()).await,
        },
        None if market == Market::In => {
            let all = crate::providers::bse_bhav::EQUITY_GROUPS;
            if let Some(scan) =
                scan_bse(client, all, DEFAULT_MIN_TRADED_INR, market.label()).await
            {
                return Ok(scan);
            }
            let u = market::Universe::Nifty500;
            let mut scan = scan_nse_index(client, u.nse_index(), Some(market.universe())).await?;
            scan.scope = market.label().to_string();
            Ok(scan)
        }
        None => Ok(Scan {
            symbols: market.universe(),
            scope: market.label().to_string(),
            bulk: Vec::new(),
        }),
    }
}

/// The stocks of BSE's last end-of-day file in `groups` that traded at
/// least `min_traded` rupees. NIFTY 500 members keep their NSE symbol
/// (`RELIANCE.NS`: NSE prices, cap tiers, the symbol holdings use); the
/// rest are BSE symbols (`XYZ.BO`). `None` when the file is unavailable.
async fn scan_bse(
    client: &YahooClient,
    groups: &[&str],
    min_traded: f64,
    scope: &str,
) -> Option<Scan> {
    let (date, tickers) = client.bse_listed(groups, min_traded).await?;
    let nifty500: std::collections::HashSet<String> = Market::In
        .universe()
        .iter()
        .filter_map(|s| s.strip_suffix(".NS").map(str::to_ascii_uppercase))
        .collect();
    let mut symbols: Vec<String> = tickers
        .into_iter()
        .map(|t| {
            let suffix = if nifty500.contains(&t) { "NS" } else { "BO" };
            format!("{}.{}", t, suffix)
        })
        .collect();
    // NIFTY 500 members are always in: one may trade thinly on BSE on a
    // day (or be listed on NSE alone) and still be liquid on NSE.
    let have: std::collections::HashSet<&String> = symbols.iter().collect();
    let extra: Vec<String> = Market::In
        .universe()
        .into_iter()
        .filter(|s| !have.contains(s))
        .collect();
    symbols.extend(extra);
    let floor = if min_traded > 0.0 {
        format!(
            " that traded \u{20B9}{:.0} lakh or more",
            min_traded / 100_000.0
        )
    } else {
        String::new()
    };
    eprintln!(
        "  \u{24d8} {} stocks: those listed on BSE{} on {}, and NIFTY 500 (--universe narrows or widens this).",
        symbols.len(),
        floor,
        date.format("%d %b %Y")
    );
    Some(Scan {
        symbols,
        scope: scope.to_string(),
        bulk: Vec::new(),
    })
}

/// Longest age of a `quote_cache` row an index scan will use.
const QUOTE_CACHE_MAX_AGE_SECS: i64 = 15 * 60;

/// Symbols and quotes of a scan (see [`scan_symbols`]): Yahoo's quotes,
/// which carry the fundamentals scoring needs, or NSE's snapshot when
/// Yahoo can't answer. Quotes stored in `quote_cache` in the last 15
/// minutes are reused; only the rest are fetched, then stored.
pub(crate) async fn universe_quotes(
    client: &YahooClient,
    market: Market,
    universe: Option<market::Universe>,
) -> Result<Vec<crate::api::Quote>> {
    check_universe(market, universe)?;
    let scan = scan_symbols(client, market, universe).await?;
    let since = chrono::Utc::now().timestamp() - QUOTE_CACHE_MAX_AGE_SECS;
    let mut cached: std::collections::HashMap<String, crate::api::Quote> =
        crate::db::with_db(|d| d.quotes_since(&scan.symbols, since))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(sym, json)| Some((sym, serde_json::from_str(&json).ok()?)))
            .collect();
    let missing: Vec<String> = scan
        .symbols
        .iter()
        .filter(|s| !cached.contains_key(*s))
        .cloned()
        .collect();
    if !cached.is_empty() {
        eprintln!(
            "  \u{24d8} {} quotes from cache.db (updated in the last {} min){}.",
            cached.len(),
            QUOTE_CACHE_MAX_AGE_SECS / 60,
            if missing.is_empty() {
                String::new()
            } else {
                format!("; fetching {} more", missing.len())
            }
        );
    }
    if !missing.is_empty() {
        let part = Scan {
            symbols: missing,
            scope: scan.scope.clone(),
            bulk: scan.bulk.clone(),
        };
        let fetched = match scan_quotes(client, market, &part, None, false).await {
            Ok(q) => q,
            // Score what cache.db has rather than fail the whole scan.
            Err(e) if !cached.is_empty() => {
                eprintln!(
                    "  \u{24d8} Could not fetch {} quotes ({:#}); scoring the {} from cache.db.",
                    part.symbols.len(),
                    e,
                    cached.len()
                );
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        // NSE's snapshot rows lack fundamentals: keep only Yahoo's.
        let rows: Vec<(String, String)> = fetched
            .iter()
            .filter(|q| q.forward_pe.is_some() || q.fifty_day_average.is_some())
            .filter_map(|q| Some((q.symbol.clone()?, serde_json::to_string(q).ok()?)))
            .collect();
        if !rows.is_empty() {
            crate::db::with_db(|d| d.put_quotes(&rows));
        }
        // A Yahoo failure returns NSE's whole snapshot: never let its
        // fundamentals-free rows replace fresh cached quotes.
        for q in fetched {
            if let Some(sym) = q.symbol.clone() {
                cached.entry(sym).or_insert(q);
            }
        }
    }
    Ok(scan
        .symbols
        .iter()
        .filter_map(|s| cached.remove(s))
        .collect())
}

/// One NSE index's members and their bulk quotes; `fallback` is the
/// built-in member list used when NSE can't answer.
async fn scan_nse_index(
    client: &YahooClient,
    index: &str,
    fallback: Option<Vec<String>>,
) -> Result<Scan> {
    let snap = match client.index_snapshot(index).await {
        Ok(s) if s.members.iter().any(|q| q.symbol.is_some()) => Ok(s),
        Ok(_) => Err(anyhow::anyhow!("NSE listed no members for {}", index)),
        Err(e) => Err(e),
    };
    match snap {
        Ok(snap) => {
            let symbols: Vec<String> = snap
                .members
                .iter()
                .filter_map(|q| q.symbol.clone())
                .collect();
            market::IndiaMembership::record(index, &symbols);
            store_bulk_snapshots(&snap.members);
            Ok(Scan {
                symbols,
                scope: index.to_string(),
                bulk: snap.members,
            })
        }
        Err(e) => match csv_members(client, index).await {
            Ok(symbols) => {
                eprintln!(
                    "  \u{24d8} NSE did not answer ({:#}); using niftyindices.com's list of {} {} members.",
                    e,
                    symbols.len(),
                    index
                );
                // The CSV is alphabetical: only the bands whose order
                // does not matter are saved as cap-tier membership.
                if matches!(index, "NIFTY MIDCAP 150" | "NIFTY SMALLCAP 250") {
                    market::IndiaMembership::record(index, &symbols);
                }
                Ok(Scan {
                    symbols,
                    scope: index.to_string(),
                    bulk: Vec::new(),
                })
            }
            Err(csv_err) => scan_fallback(
                index,
                fallback,
                anyhow::anyhow!("{:#}; niftyindices.com: {:#}", e, csv_err),
            ),
        },
    }
}

/// How long a member list read from niftyindices.com is reused. Indices
/// change members twice a year.
const CSV_MEMBERS_MAX_AGE_MS: i64 = 24 * 3600 * 1000;

/// An index's members from niftyindices.com, kept in `cache.db` for a day
/// (under `csv:<index>`, apart from the cap-tier lists) so the next scans
/// ask nobody.
async fn csv_members(client: &YahooClient, index: &str) -> Result<Vec<String>> {
    let key = format!("csv:{}", index);
    let now = chrono::Utc::now().timestamp_millis();
    if let Some((members, at)) = crate::db::with_db(|d| d.index_list(&key)).flatten()
        && !members.is_empty()
        && now - at < CSV_MEMBERS_MAX_AGE_MS
    {
        return Ok(members);
    }
    let members = client.index_members_csv(index).await?;
    crate::db::with_db(|d| d.put_index_lists(&[(key, false, members.clone())]));
    Ok(members)
}

/// The saved or built-in list when neither NSE nor niftyindices.com can
/// list an index's members.
fn scan_fallback(index: &str, fallback: Option<Vec<String>>, e: anyhow::Error) -> Result<Scan> {
    match fallback {
        Some(list) => {
            eprintln!(
                "  \u{24d8} {}",
                tf(
                    Msg::UniverseFallback,
                    &[&index, &format!("{:#}", e), &list.len()]
                )
            );
            Ok(Scan {
                symbols: list,
                scope: index.to_string(),
                bulk: Vec::new(),
            })
        }
        None => Err(e.context(format!(
            "NSE's {} member list is unavailable and StockWise has no built-in copy of it",
            index
        ))),
    }
}

/// Save an NSE bulk snapshot's prices into `market_snapshots` (best effort;
/// fundamentals already stored for a symbol are kept).
fn store_bulk_snapshots(quotes: &[crate::api::Quote]) {
    let members = market::IndiaMembership::load();
    let rows: Vec<crate::db::Snapshot> = quotes
        .iter()
        .filter(|q| q.is_usable())
        .filter_map(|q| crate::worker::snapshot_from_quote("in", q))
        .map(|mut r| {
            if r.cap_tier.is_none() {
                let t = market::CapTier::classify(&r.symbol, r.mcap, Some("INR"), Some(&members));
                r.cap_tier = (t != market::CapTier::Unknown).then(|| t.as_str().to_string());
            }
            r
        })
        .collect();
    if !rows.is_empty() {
        crate::db::with_db(|d| d.upsert_snapshots(&rows));
    }
}

/// Quotes of a `--cap` scan from fresh `market_snapshots` rows, when the
/// worker or an earlier scan stored enough of the market's built-in list:
/// no request at all. `None` means "scan live".
fn cached_cap_quotes(
    market: Market,
    cap: market::CapFilter,
    scanned: &[String],
) -> Option<Vec<crate::api::Quote>> {
    let tier = cap.tier()?;
    if !matches!(market, Market::In | Market::Us) {
        return None;
    }
    let label = crate::value_name(market);
    let since = chrono::Utc::now().timestamp() - SNAPSHOT_MAX_AGE_SECS;
    let filter = crate::db::SnapshotFilter {
        market: Some(label),
        updated_since: Some(since),
        ..Default::default()
    };
    let (all, rows) = crate::db::with_db(|d| {
        let all = d.query_snapshots(&filter)?;
        let rows = d.query_snapshots(&crate::db::SnapshotFilter {
            cap_tier: Some(tier),
            ..filter.clone()
        })?;
        Ok((all, rows))
    })?;
    // Only the symbols this scan covers (not a bigger index scanned
    // earlier), and only when most of them are stored.
    let wanted: std::collections::HashSet<String> =
        scanned.iter().map(|s| s.to_ascii_uppercase()).collect();
    let in_scan = |r: &&crate::db::Snapshot| wanted.contains(&r.symbol.to_ascii_uppercase());
    if all.iter().filter(in_scan).count() * 2 < wanted.len() {
        return None;
    }
    Some(
        rows.iter()
            .filter(in_scan)
            .map(crate::worker::quote_from_snapshot)
            .collect(),
    )
}

/// Quotes a scan ranks. `--cap` uses fresh cached snapshots when it can; an
/// NSE index scan uses the bulk snapshot (`prefer_bulk`, for scans that need
/// only price and change) or falls back to it when Yahoo can't answer;
/// everything else is a live Yahoo scan. The result is cut to `cap`.
pub(super) async fn scan_quotes(
    client: &YahooClient,
    market: Market,
    scan: &Scan,
    cap: Option<market::CapFilter>,
    prefer_bulk: bool,
) -> Result<Vec<crate::api::Quote>> {
    scan_quotes_src(client, market, scan, cap, prefer_bulk)
        .await
        .map(|(q, _)| q)
}

/// [`scan_quotes`], and whether the quotes came from `cache.db`.
async fn scan_quotes_src(
    client: &YahooClient,
    market: Market,
    scan: &Scan,
    cap: Option<market::CapFilter>,
    prefer_bulk: bool,
) -> Result<(Vec<crate::api::Quote>, bool)> {
    if scan.bulk.is_empty()
        && let Some(c) = cap
        && let Some(cached) = cached_cap_quotes(market, c, &scan.symbols)
    {
        eprintln!(
            "  \u{24d8} {} from cache.db (updated in the last {} min).",
            c.tier().map_or("all", market::CapTier::as_str),
            SNAPSHOT_MAX_AGE_SECS / 60
        );
        return Ok((cached, true));
    }
    if prefer_bulk && !scan.bulk.is_empty() {
        return Ok((market::filter_by_cap(scan.bulk.clone(), cap), false));
    }
    // Price-only scans (movers, heatmap) outside Indian market hours: the
    // last close of every stock comes from one BSE file, no Yahoo calls.
    // A `--cap` tier needs market caps, which the file lacks.
    let no_tier = cap.is_none_or(|c| c.tier().is_none());
    if prefer_bulk
        && no_tier
        && market == Market::In
        && !crate::clock::is_open(Market::In, chrono::Utc::now())
        && let Some(q) = bse_eod(client, &scan.symbols, "the market is closed", true).await
    {
        return Ok((q, false));
    }
    let refs: Vec<&str> = scan.symbols.iter().map(String::as_str).collect();
    let live = client.get_quote(&refs).await;
    // Yahoo down: the keyed providers priced the built-in list (their quotes
    // carry `meta.sources`). If the `--cap` filter leaves none of it, the
    // screener lists companies in that tier instead.
    let yahoo_down = match &live {
        Err(_) => true,
        Ok(q) => {
            q.is_empty()
                || (q.iter().any(|x| x.meta.is_some())
                    && market::filter_by_cap(q.clone(), cap).is_empty())
        }
    };
    if scan.bulk.is_empty()
        && yahoo_down
        && let Some(q) = screener_quotes(client, market, cap).await
    {
        return Ok((q, false));
    }
    match live {
        Ok(q) => {
            // Keep the scan's quotes as snapshots: the next `--cap` scan in
            // the half hour reads them instead of asking again.
            if market == Market::In {
                store_bulk_snapshots(&q);
            }
            Ok((market::filter_by_cap(q, cap), false))
        }
        Err(e) if !scan.bulk.is_empty() => {
            eprintln!(
                "  \u{24d8} Yahoo Finance did not answer ({:#}); using NSE's index snapshot (no P/E or market cap).",
                e
            );
            Ok((market::filter_by_cap(scan.bulk.clone(), cap), false))
        }
        // Price-only scans only: scans that score fundamentals, and callers
        // with their own stored-price fallback, get the error.
        Err(e) if prefer_bulk && market == Market::In && no_tier => {
            match bse_eod(client, &scan.symbols, "Yahoo Finance did not answer", false).await {
                Some(q) => Ok((q, false)),
                None => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// The scan's symbols at BSE's last close (bhavcopy), when the file covers
/// at least half of them; says so on stderr. `why` names the reason. With
/// `current_only`, a file older than the last completed session is not
/// used (between the close and BSE posting the day's file, Yahoo has the
/// newer prices).
async fn bse_eod(
    client: &YahooClient,
    symbols: &[String],
    why: &str,
    current_only: bool,
) -> Option<Vec<crate::api::Quote>> {
    let (date, quotes, current) = client.bse_eod_quotes(symbols).await?;
    if quotes.len() * 2 < symbols.len() || (current_only && !current) {
        return None;
    }
    eprintln!(
        "  \u{24d8} {}: {} closing prices of {} from BSE's bhavcopy (no P/E or market cap).",
        why,
        quotes.len(),
        date.format("%d %b %Y")
    );
    Some(quotes)
}

/// Most companies a screener fallback asks for.
const SCREENER_LIMIT: usize = 200;

/// A US scan Yahoo could not answer: FMP's screener lists companies in the
/// `--cap` tier, and the keyed providers price them (one FMP batch request),
/// the screener's market cap, sector and industry filling what a quote
/// lacks. `None` without a keyed provider or when they have nothing.
async fn screener_quotes(
    client: &YahooClient,
    market: Market,
    cap: Option<market::CapFilter>,
) -> Option<Vec<crate::api::Quote>> {
    if market != Market::Us || !client.has_router() {
        return None;
    }
    let (min, max) = cap
        .and_then(market::CapFilter::tier)
        .map_or((None, None), market::CapTier::usd_range);
    let query = crate::providers::router::ScreenQuery {
        market_cap_min: min.or(max.is_none().then_some(market::USD_LARGE_MIN)),
        market_cap_max: max,
        country: Some("US".into()),
        limit: SCREENER_LIMIT,
        ..Default::default()
    };
    let (rows, source) = client.provider_screener(&query).await.ok()?;
    let symbols: Vec<String> = rows.iter().filter_map(|r| r.symbol.clone()).collect();
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let mut quotes = client.get_quote(&refs).await.ok()?;
    for q in &mut quotes {
        if let Some(row) = rows.iter().find(|r| r.symbol == q.symbol) {
            crate::providers::router::fill_quote(q, row);
        }
    }
    eprintln!(
        "  \u{24d8} Yahoo Finance did not answer; {} companies listed by {}'s screener and priced by the keyed providers.",
        quotes.len(),
        source
    );
    Some(market::filter_by_cap(quotes, cap))
}

/// Scan title with the `--cap` tier appended: `NIFTY 500 (mid cap)`.
pub(crate) fn cap_scope(scope: &str, cap: Option<market::CapFilter>) -> String {
    match cap.and_then(market::CapFilter::tier) {
        Some(t) => format!("{scope} ({} cap)", t.as_str()),
        None => scope.to_string(),
    }
}

/// When the movers' prices are from: the local time now, the session the
/// latest trade belongs to (exchange-local) and whether the market is open.
pub(crate) fn movers_as_of(
    market: Market,
    now: chrono::DateTime<chrono::Utc>,
    last_trade: Option<i64>,
) -> String {
    use crate::clock;
    let mut parts = vec![format!("As of {}", clock::format_local(market, now))];
    match last_trade.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)) {
        Some(t) => parts.push(format!(
            "session {} (last trade {} {})",
            clock::session_date(market, t).format("%a %d %b %Y"),
            clock::local(market, t).format("%H:%M"),
            clock::tz_label(market)
        )),
        None => parts.push(format!(
            "session {}",
            clock::session_date(market, now).format("%a %d %b %Y")
        )),
    }
    parts.push(if clock::is_open(market, now) {
        "market open".to_string()
    } else {
        format!("market closed ({})", clock::closed_reason(market, now))
    });
    parts.join(" · ")
}

pub async fn cmd_movers(
    market: Market,
    universe: Option<market::Universe>,
    cap: Option<market::CapFilter>,
    fmt: Format,
) -> Result<()> {
    check_universe(market, universe)?;
    let client = YahooClient::new().await?;
    let scan = scan_symbols(&client, market, universe).await?;
    let quotes = scan_quotes(&client, market, &scan, cap, true).await?;
    let scope = cap_scope(&scan.scope, cap);
    let (gainers, losers) = mover_rows(&quotes, 10);
    match fmt {
        Format::Json => {
            return output::print_json(&serde_json::json!({
                "gainers": gainers,
                "losers": losers,
            }));
        }
        Format::Csv => {
            let all: Vec<MoverRow> = gainers.into_iter().chain(losers).collect();
            return output::print_csv(&all);
        }
        Format::Table => {}
    }

    let last_trade = quotes.iter().filter_map(|q| q.regular_market_time).max();
    println!(
        "\n  {}",
        movers_as_of(market, chrono::Utc::now(), last_trade).dimmed()
    );
    for (title, rows, empty) in [
        ("Top Gainers", &gainers, "No stocks rose today"),
        ("Top Losers", &losers, "No stocks fell today"),
    ] {
        print_header(&format!("{} — {}", title, scope));
        if rows.is_empty() {
            println!("  {}", empty.dimmed());
            continue;
        }
        println!(
            "  {:<14} {:>10} {:>10} {:>14} {:>6}",
            "Symbol".bold(),
            "Price".bold(),
            "Change %".bold(),
            "Volume".bold(),
            "Cap".bold()
        );
        println!("  {}", "─".repeat(59).dimmed());
        for r in rows.iter() {
            let pct = format!("{:+.2}%", r.change_percent);
            println!(
                "  {:<14} {:>10} {:>10} {:>14} {:>6}",
                r.symbol.cyan(),
                format_asset_price(r.price, r.currency.as_deref(), &r.symbol),
                if r.side == "gainer" {
                    pct.green()
                } else {
                    pct.red()
                },
                r.volume.map_or("N/A".to_string(), format_volume),
                market::CapTier::parse(r.cap_tier)
                    .map_or("", market::CapTier::badge)
                    .dimmed()
            );
        }
    }
    println!();
    Ok(())
}

// ── Sectors ──

/// One sector in `sectors --json/--csv`, best performer first.
#[derive(Debug, Serialize)]
pub(crate) struct SectorRow {
    pub(crate) sector: String,
    pub(crate) symbol: String,
    pub(crate) price: Option<f64>,
    pub(crate) change_percent: Option<f64>,
    pub(crate) currency: Option<String>,
}

impl CsvRow for SectorRow {
    const COLUMNS: &'static [&'static str] =
        &["sector", "symbol", "price", "change_percent", "currency"];
}

/// The NSE index behind an Indian sector and its built-in member list.
fn india_sector_index(name: &str) -> Option<(&'static str, &'static str, Option<Vec<String>>)> {
    let (_, short) = market::INDIA_SECTOR_INDICES
        .iter()
        .find(|(sym, short)| short.eq_ignore_ascii_case(name) || sym.eq_ignore_ascii_case(name))?;
    let (index, u) = match *short {
        "IT" => ("NIFTY IT", Some(market::Universe::NiftyIt)),
        "Bank" => ("NIFTY BANK", Some(market::Universe::BankNifty)),
        "Pharma" => ("NIFTY PHARMA", None),
        "Auto" => ("NIFTY AUTO", None),
        "FMCG" => ("NIFTY FMCG", None),
        "Metal" => ("NIFTY METAL", None),
        "Realty" => ("NIFTY REALTY", None),
        "Energy" => ("NIFTY ENERGY", None),
        _ => return None,
    };
    Some((short, index, u.and_then(market::Universe::fallback)))
}

/// Sector index members saved by the last live listing: the offline
/// fallback for sectors without a built-in list.
#[derive(Debug, Default, Serialize, serde::Deserialize)]
struct SectorMembers(std::collections::BTreeMap<String, Vec<String>>);

impl SectorMembers {
    const FILE: &'static str = "sector_members.json";

    fn get(index: &str) -> Option<Vec<String>> {
        let m: SectorMembers = crate::store::load(Self::FILE).ok()?;
        m.0.get(index).filter(|l| !l.is_empty()).cloned()
    }

    fn save(index: &str, symbols: &[String]) {
        if symbols.is_empty() {
            return;
        }
        let _ = crate::store::update(Self::FILE, |m: &mut SectorMembers| {
            m.0.insert(index.to_string(), symbols.to_vec());
            Ok(())
        });
    }
}

/// Quotes served from `cache.db` with each one's age in seconds.
struct StoredQuotes {
    quotes: Vec<crate::api::Quote>,
    age_secs: std::collections::HashMap<String, i64>,
}

/// The last stored `market_snapshots` rows of `symbols`, of any age: the
/// offline answer when no live quote is available. `None` when none match.
fn stored_quotes(symbols: &[String], cap: Option<market::CapFilter>) -> Option<StoredQuotes> {
    let filter = crate::db::SnapshotFilter {
        market: Some("in".to_string()),
        cap_tier: cap.and_then(market::CapFilter::tier),
        ..Default::default()
    };
    let rows = crate::db::with_db(|d| d.query_snapshots(&filter))?;
    let wanted: std::collections::HashSet<String> =
        symbols.iter().map(|s| s.to_ascii_uppercase()).collect();
    let now = chrono::Utc::now().timestamp();
    let rows: Vec<_> = rows
        .iter()
        .filter(|r| wanted.contains(&r.symbol.to_ascii_uppercase()))
        .collect();
    let age_secs = rows
        .iter()
        .map(|r| (r.symbol.to_ascii_uppercase(), (now - r.updated_at).max(0)))
        .collect();
    let quotes: Vec<crate::api::Quote> = rows
        .into_iter()
        .map(crate::worker::quote_from_snapshot)
        .collect();
    (!quotes.is_empty()).then_some(StoredQuotes { quotes, age_secs })
}

/// One member in `sectors <NAME> --json/--csv`: a `screen` row, plus the
/// age of the price when it came from `cache.db` instead of a live quote.
#[derive(Debug, Serialize)]
pub(crate) struct SectorMemberRow {
    #[serde(flatten)]
    row: ScreenRow,
    price_age_secs: Option<i64>,
}

impl CsvRow for SectorMemberRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "price",
        "change_percent",
        "pe",
        "market_cap",
        "dividend_yield",
        "currency",
        "cap_tier",
        "price_age_secs",
    ];
}

/// `sectors <NAME>`: the sector index's members with their cap tier,
/// biggest mover first, cut to `--cap`.
async fn cmd_sector_members(
    market: Market,
    name: &str,
    cap: Option<market::CapFilter>,
    fmt: Format,
) -> Result<()> {
    if market != Market::In {
        bail!(
            "sector members are listed for the Indian market only (-m in), not {}",
            market.label()
        );
    }
    let Some((short, index, fallback)) = india_sector_index(name) else {
        let names: Vec<&str> = market::INDIA_SECTOR_INDICES
            .iter()
            .map(|(_, n)| *n)
            .collect();
        bail!(
            "Sector '{}' not found; try one of: {}",
            name,
            names.join(", ")
        );
    };
    let client = YahooClient::new().await?;
    // The last live listing beats the built-in list (an index rebalances).
    let fallback = SectorMembers::get(index).or(fallback);
    let scan = scan_nse_index(&client, index, fallback).await?;
    if scan.live() {
        SectorMembers::save(index, &scan.symbols);
        // NSE answered, so one more request is cheap: NSE's snapshot quotes
        // carry no market cap, and tiers come from the saved membership.
        if market::IndiaMembership::load().is_empty() {
            let seed = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                scan_nse_index(&client, "NIFTY 500", None),
            )
            .await;
            let why = match seed {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(format!("{:#}", e)),
                Err(_) => Some("timed out".to_string()),
            };
            if let Some(why) = why
                && market::IndiaMembership::load().is_empty()
            {
                eprintln!(
                    "  \u{24d8} Cap tiers unavailable (NIFTY 500 membership: {why}); members show as unknown and a specific --cap may list none. Retry later."
                );
            }
        }
    }
    let mut ages = std::collections::HashMap::new();
    let mut quotes = match scan_quotes_src(&client, market, &scan, cap, true).await {
        Ok((q, from_cache)) => {
            if from_cache && let Some(st) = stored_quotes(&scan.symbols, cap) {
                ages = st.age_secs;
            }
            q
        }
        Err(e) => match stored_quotes(&scan.symbols, cap) {
            Some(st) => {
                let oldest = st.age_secs.values().max().copied().unwrap_or(0);
                eprintln!(
                    "  \u{24d8} No live quotes ({:#}); using prices stored in cache.db (oldest {} min old).",
                    e,
                    oldest / 60
                );
                eprintln!(
                    "  \u{24d8} Showing {} stored quotes after any cap filter; the sector list has {} members. Members without stored quotes may be missing.",
                    st.quotes.len(),
                    scan.symbols.len()
                );
                ages = st.age_secs;
                st.quotes
            }
            None => return Err(e),
        },
    };
    let member_rows = |quotes: &[crate::api::Quote]| -> Vec<SectorMemberRow> {
        quotes
            .iter()
            .map(|q| SectorMemberRow {
                row: ScreenRow::from(q),
                price_age_secs: q
                    .symbol
                    .as_ref()
                    .and_then(|s| ages.get(&s.to_ascii_uppercase()).copied()),
            })
            .collect()
    };
    quotes.sort_by(|a, b| {
        let key = |q: &crate::api::Quote| q.regular_market_change_percent.unwrap_or(f64::MIN);
        key(b).total_cmp(&key(a))
    });
    match fmt {
        Format::Json => {
            return output::print_json(&member_rows(&quotes));
        }
        Format::Csv => {
            return output::print_csv(&member_rows(&quotes));
        }
        Format::Table => {}
    }
    print_header(&format!("{} — {}", short, cap_scope(index, cap)));
    if quotes.is_empty() {
        println!("  {}", "No members in this tier.".dimmed());
        return Ok(());
    }
    println!(
        "  {:<14} {:>10} {:>10} {:>14} {:>6}",
        "Symbol".bold(),
        "Price".bold(),
        "Change %".bold(),
        "Market Cap".bold(),
        "Cap".bold()
    );
    println!("  {}", "─".repeat(59).dimmed());
    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = format!("{:+.2}%", pct);
        println!(
            "  {:<14} {:>10} {:>10} {:>14} {:>6}",
            sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            if pct >= 0.0 {
                pct_str.green()
            } else {
                pct_str.red()
            },
            q.market_cap
                .map_or("N/A".to_string(), |v| format_large_number(v, cur)),
            market::cap_tier_of(q).badge().dimmed()
        );
    }
    println!();
    Ok(())
}

pub async fn cmd_sectors(
    market: Market,
    name: Option<String>,
    cap: Option<market::CapFilter>,
    fmt: Format,
) -> Result<()> {
    market.require_sectors("sectors")?;
    if let Some(n) = name {
        return cmd_sector_members(market, &n, cap, fmt).await;
    }
    if cap.is_some() {
        bail!(
            "--cap filters a sector's members: name one, e.g. `stockwise sectors bank --cap mid`"
        );
    }
    let client = YahooClient::new().await?;
    let sectors = market.sectors();
    let title = format!("{} Sector Performance", market.label());
    let symbols: Vec<&str> = sectors.iter().map(|(s, _)| *s).collect();
    let mut quotes = client.get_quote(&symbols).await?;
    // Best first; sectors without a change today go last.
    quotes.sort_by(|a, b| {
        let key = |q: &crate::api::Quote| q.regular_market_change_percent.unwrap_or(f64::MIN);
        key(b)
            .partial_cmp(&key(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if !fmt.is_table() {
        let rows: Vec<SectorRow> = quotes
            .iter()
            .map(|q| {
                let sym = q.symbol.clone().unwrap_or_default();
                let name = sectors
                    .iter()
                    .find(|(s, _)| *s == sym)
                    .map_or(sym.clone(), |(_, n)| n.to_string());
                SectorRow {
                    sector: name,
                    symbol: sym,
                    price: q.regular_market_price,
                    change_percent: q.regular_market_change_percent,
                    currency: q.currency.clone(),
                }
            })
            .collect();
        return match fmt {
            Format::Csv => output::print_csv(&rows),
            _ => output::print_json(&rows),
        };
    }

    print_header(&title);
    println!(
        "  {:<18} {:>10} {:>10} {:>20}",
        "Sector".bold(),
        "Price".bold(),
        "Change %".bold(),
        "".bold()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let name = sectors
            .iter()
            .find(|(s, _)| *s == sym)
            .map(|(_, n)| *n)
            .unwrap_or(sym);
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let bar_len = (pct.abs() * 3.0).min(20.0) as usize;
        let bar = if pct >= 0.0 {
            "█".repeat(bar_len).green().to_string()
        } else {
            "█".repeat(bar_len).red().to_string()
        };
        let pct_str = if pct >= 0.0 {
            format!("{:+.2}%", pct).green().to_string()
        } else {
            format!("{:+.2}%", pct).red().to_string()
        };
        println!(
            "  {:<18} {:>10} {:>10} {}",
            name.bold(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            pct_str,
            bar
        );
    }
    println!();
    Ok(())
}

// ── News ──

// ── Multi-source news helpers ──────────────────────────────────

#[derive(Debug, Clone)]
pub(super) struct NewsItem {
    title: String,
    publisher: String,
    timestamp: Option<i64>,   // unix seconds
    source_tag: &'static str, // e.g. "Yahoo", "Google News"
}

/// Strip CDATA wrappers and XML entities from an RSS field
pub(super) fn rss_clean(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(s);
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .trim()
        .to_string()
}

/// Extract text between two markers (first occurrence)
pub(super) fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)?;
    Some(&s[start..start + end])
}

/// Parse RSS XML into NewsItems
pub(super) fn parse_rss(xml: &str, source_tag: &'static str) -> Vec<NewsItem> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let end = chunk.find("</item>").unwrap_or(chunk.len());
            let item = &chunk[..end];

            // Title: try CDATA first, then plain
            let title_raw = between(item, "<title><![CDATA[", "]]></title>")
                .or_else(|| between(item, "<title>", "</title>"))?;
            let mut title = rss_clean(title_raw);

            // Publisher from <source ...>Name</source> or hardcode source_tag.
            // The tag body starts with the rest of the opening tag (attributes
            // like url="..."), so take everything after the first '>'.
            let publisher = between(item, "<source", "</source>")
                .and_then(|s| s.split_once('>').map(|(_, name)| name))
                .map(rss_clean)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| source_tag.to_string());

            // Google News repeats the publisher as a " - Publisher" suffix
            let suffix = format!(" - {}", publisher);
            if title.ends_with(&suffix) {
                title.truncate(title.len() - suffix.len());
            }
            if title.is_empty() {
                return None;
            }

            // Timestamp from <pubDate>
            let timestamp = between(item, "<pubDate>", "</pubDate>")
                .and_then(|d| chrono::DateTime::parse_from_rfc2822(d.trim()).ok())
                .map(|d| d.timestamp());

            Some(NewsItem {
                title,
                publisher,
                timestamp,
                source_tag,
            })
        })
        .collect()
}

/// Fetch Yahoo Finance RSS for a symbol
pub(super) async fn fetch_yahoo_rss(client: &YahooClient, symbol: &str) -> Vec<NewsItem> {
    let url = crate::api::build_url(
        "https://feeds.finance.yahoo.com/rss/2.0/headline",
        None,
        &[("s", symbol), ("region", "US"), ("lang", "en-US")],
    );
    match client.raw_get_text(&url).await {
        Ok(xml) => parse_rss(&xml, "Yahoo RSS"),
        Err(_) => vec![],
    }
}

/// Fetch Google News RSS for a query
pub(super) async fn fetch_google_news_rss(client: &YahooClient, query: &str) -> Vec<NewsItem> {
    let url = crate::api::build_url(
        "https://news.google.com/rss/search",
        None,
        &[("q", query), ("hl", "en"), ("gl", "US"), ("ceid", "US:en")],
    );
    match client.raw_get_text(&url).await {
        Ok(xml) => parse_rss(&xml, "Google News"),
        Err(_) => vec![],
    }
}

/// Fetch Yahoo Finance search API news (JSON)
pub(super) async fn fetch_yahoo_json_news(
    client: &YahooClient,
    query: &str,
    count: usize,
) -> Vec<NewsItem> {
    let count = count.to_string();
    let url = crate::api::build_url(
        "https://query2.finance.yahoo.com/v1/finance/search",
        None,
        &[("q", query), ("quotesCount", "0"), ("newsCount", &count)],
    );
    let resp = match client.raw_get(&url).await {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    resp.get("news")
        .and_then(|n| n.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let title = item.get("title").and_then(|t| t.as_str())?.to_string();
                    let publisher = item
                        .get("publisher")
                        .and_then(|p| p.as_str())
                        .unwrap_or("Yahoo Finance")
                        .to_string();
                    let timestamp = item.get("providerPublishTime").and_then(|t| t.as_i64());
                    Some(NewsItem {
                        title,
                        publisher,
                        timestamp,
                        source_tag: "Yahoo Finance",
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Deduplicate news items: remove near-duplicate titles, sort newest first
pub(super) fn collate_news(mut items: Vec<NewsItem>) -> Vec<NewsItem> {
    // Sort newest first (None timestamps go to the end)
    items.sort_by_key(|i| std::cmp::Reverse(i.timestamp.unwrap_or(0)));

    // Dedup: fingerprint as owned Strings so there are no borrow issues
    let mut seen: Vec<Vec<String>> = Vec::new();
    let mut result = Vec::new();

    for item in items {
        let words: Vec<String> = item
            .title
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|w| w.to_lowercase())
            .collect();
        let is_dup = seen.iter().any(|existing| {
            if existing.is_empty() || words.is_empty() {
                return false;
            }
            let matches = words.iter().filter(|w| existing.contains(w)).count();
            matches as f64 / words.len().min(existing.len()) as f64 > 0.6
        });
        if !is_dup {
            seen.push(words);
            result.push(item);
        }
    }
    result
}

pub(super) fn print_news_items(items: &[NewsItem], limit: usize) {
    if items.is_empty() {
        println!("  {}", "No recent news found.".dimmed());
        return;
    }
    let source_colors: &[(&str, &str)] = &[
        ("Yahoo Finance", "cyan"),
        ("Yahoo RSS", "blue"),
        ("Google News", "green"),
    ];
    for item in items.iter().take(limit) {
        let ts = item
            .timestamp
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let tag = source_colors
            .iter()
            .find(|(s, _)| *s == item.source_tag)
            .map(|(_, c)| match *c {
                "cyan" => format!("[{}]", item.source_tag).cyan().to_string(),
                "blue" => format!("[{}]", item.source_tag).blue().to_string(),
                "green" => format!("[{}]", item.source_tag).green().to_string(),
                _ => format!("[{}]", item.source_tag),
            })
            .unwrap_or_else(|| format!("[{}]", item.source_tag));
        println!();
        println!("  {} {}", "●".cyan(), item.title.bold());
        println!(
            "    {} {} · {} · {}",
            "└".dimmed(),
            item.publisher.dimmed(),
            ts.dimmed(),
            tag
        );
    }
}

// ───────────────────────────────────────────────────────────────

/// `news SYMBOL`: headlines and filings from every source, merged and
/// de-duplicated, newest first, in local time with links.
pub async fn cmd_news(
    symbol: &str,
    market: Market,
    days: u32,
    limit: usize,
    json: bool,
) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    // The company name makes Google News match the company, not the ticker.
    let company = client
        .get_quote(&[resolved.as_str()])
        .await
        .ok()
        .and_then(|q| q.into_iter().next())
        .and_then(|q| q.long_name.or(q.short_name));
    let fetched = crate::news::fetch(
        &client,
        &crate::news::Query {
            symbol: resolved.clone(),
            company: company.clone(),
            days: days.max(1),
            filings: true,
        },
    )
    .await;
    if json {
        return output::print_json(&serde_json::json!({
            "symbol": resolved,
            "company": company,
            "days": days,
            "sources_ok": fetched.sources_ok,
            "sources_tried": fetched.sources_tried,
            "failures": fetched.failures,
            "items": fetched.items.iter().take(limit).collect::<Vec<_>>(),
        }));
    }
    let name = company
        .as_deref()
        .map(|c| format!(" ({})", crate::research::clean_company_name(c)))
        .unwrap_or_default();
    print_header(&format!(
        "News: {}{} — last {} day{}, {} of {} sources",
        resolved,
        name,
        days,
        if days == 1 { "" } else { "s" },
        fetched.sources_ok,
        fetched.sources_tried
    ));
    print_feed_items(&fetched.items, limit);
    for f in &fetched.failures {
        println!("  {} {}", "!".yellow(), f.dimmed());
    }
    println!();
    Ok(())
}

pub(super) fn print_feed_items(items: &[crate::news::NewsItem], limit: usize) {
    if items.is_empty() {
        println!("  {}", "No recent news found.".dimmed());
        return;
    }
    for item in items.iter().take(limit) {
        let tag = if item.official {
            format!("[{}]", item.source).yellow().bold().to_string()
        } else {
            format!("[{}]", item.source).cyan().to_string()
        };
        println!();
        println!("  {} {}", "●".cyan(), item.title.bold());
        println!(
            "    {} {} · {} · {}",
            "└".dimmed(),
            item.publisher.dimmed(),
            item.local_time().unwrap_or_default().dimmed(),
            tag
        );
        if let Some(link) = &item.link {
            println!("      {}", link.dimmed());
        }
    }
}

// ── Risk Analysis ──

/// What the sentiment gauge reads for one market.
struct SentimentSpec {
    /// The volatility index and its name.
    vix: (&'static str, &'static str),
    /// Upper bounds of the extreme-greed, greed, neutral and fear bands of
    /// the volatility index; above the last is extreme fear.
    vix_bands: [f64; 4],
    /// The broad index the momentum and trend readings use.
    index: (&'static str, &'static str),
    /// A smaller-company index, compared with `index` for breadth.
    breadth: (&'static str, &'static str),
    /// Other indices shown with their day change.
    others: &'static [(&'static str, &'static str)],
    /// News searches for the headline count.
    yahoo_query: &'static str,
    google_query: &'static str,
}

/// The gauge's inputs for `market`: India VIX, Nifty 50, Bank Nifty and
/// Nifty Midcap 50 for India; VIX, S&P 500, Nasdaq and Russell 2000 for
/// the US.
fn sentiment_spec(market: Market) -> Result<SentimentSpec> {
    match market {
        // India VIX sits higher than the US VIX for the same mood: its
        // median since 2010 is in the mid-to-high teens and stress pushes it
        // past 30 (above 80 in March 2020).
        Market::In => Ok(SentimentSpec {
            vix: ("^INDIAVIX", "India VIX"),
            vix_bands: [14.0, 18.0, 24.0, 32.0],
            index: ("^NSEI", "NIFTY 50"),
            breadth: ("^NSEMDCP50", "NIFTY MIDCAP 50"),
            others: &[("^NSEBANK", "BANK NIFTY")],
            yahoo_query: "Nifty Sensex today",
            google_query: "Indian stock market today Nifty Sensex",
        }),
        Market::Us => Ok(SentimentSpec {
            vix: ("^VIX", "VIX"),
            vix_bands: [15.0, 20.0, 25.0, 35.0],
            index: ("^GSPC", "S&P 500"),
            breadth: ("^RUT", "Russell 2000"),
            others: &[("^IXIC", "Nasdaq")],
            yahoo_query: "stock market today",
            google_query: "US stock market today",
        }),
        _ => bail!(
            "sentiment reads India VIX or the VIX: use -m in or -m us, not {}",
            market.label()
        ),
    }
}

/// The volatility index on the 0 (extreme fear) … 100 (extreme greed)
/// scale, by `bands` (see [`SentimentSpec::vix_bands`]).
fn vix_score(v: f64, bands: [f64; 4]) -> f64 {
    match v {
        x if x < bands[0] => 90.0,
        x if x < bands[1] => 70.0,
        x if x < bands[2] => 50.0,
        x if x < bands[3] => 30.0,
        _ => 10.0,
    }
}

/// The gauge's label for a 0 … 100 reading.
fn mood(score: f64) -> &'static str {
    match score {
        s if s > 60.0 => "Greed",
        s if s < 40.0 => "Fear",
        _ => "Neutral",
    }
}

/// Headlines whose wording reads positive and negative (the shared
/// lexicon of `events`, whole words with negation).
fn headline_tally<'a>(titles: impl IntoIterator<Item = &'a str>) -> (usize, usize) {
    titles
        .into_iter()
        .map(crate::events::lexicon_sentiment)
        .fold((0, 0), |(p, n), s| {
            if s > 0.1 {
                (p + 1, n)
            } else if s < -0.1 {
                (p, n + 1)
            } else {
                (p, n)
            }
        })
}

pub async fn cmd_sentiment(market: Market) -> Result<()> {
    let spec = sentiment_spec(market)?;
    let client = YahooClient::new().await?;
    let mut syms: Vec<&str> = vec![spec.vix.0, spec.index.0, spec.breadth.0];
    syms.extend(spec.others.iter().map(|(s, _)| *s));
    let quotes = client.get_quote(&syms).await?;
    // Match by symbol: Yahoo may reorder or drop symbols.
    let quote = |sym: &str| {
        quotes.iter().find(|q| {
            q.symbol
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(sym))
        })
    };
    let day_change = |sym: &str| quote(sym).and_then(|q| q.regular_market_change_percent);

    let mut score = 50.0_f64;
    let mut factors = Vec::new();

    if let Some(v) = quote(spec.vix.0).and_then(|q| q.regular_market_price) {
        let vs = vix_score(v, spec.vix_bands);
        score = (score + vs) / 2.0;
        factors.push(format!("{}: {:.1} → {}", spec.vix.1, v, mood(vs)));
    } else {
        factors.push(format!("{}: — (no quote)", spec.vix.1));
    }

    // Today's move of the broad index
    if let Some(chg) = day_change(spec.index.0) {
        let mom_score = (50.0 + chg * 10.0).clamp(0.0, 100.0);
        score = (score + mom_score) / 2.0;
        factors.push(format!("{}: {:+.2}% today", spec.index.1, chg));
    }

    // Breadth: smaller companies against the broad index today
    if let (Some(small), Some(broad)) = (day_change(spec.breadth.0), day_change(spec.index.0)) {
        let gap = small - broad;
        let breadth_score = (50.0 + gap * 10.0).clamp(0.0, 100.0);
        score = score * 0.8 + breadth_score * 0.2;
        factors.push(format!(
            "{} vs {}: {:+.2} pp today",
            spec.breadth.1, spec.index.1, gap
        ));
    }

    // One-month trend of the broad index
    if let Ok(c) = client.get_chart(spec.index.0, "1mo", "1d").await {
        let closes = chart_closes(&c);
        if let (true, Some(first), Some(last)) = (closes.len() > 5, closes.first(), closes.last()) {
            let monthly_ret = technical::percent_change(*first, *last);
            let trend_score = (50.0 + monthly_ret * 5.0).clamp(0.0, 100.0);
            score = (score + trend_score) / 2.0;
            factors.push(format!("{} 1M trend: {:+.2}%", spec.index.1, monthly_ret));
        }
    }

    // Headline wording from two news sources
    let (yahoo_news, google_news) = tokio::join!(
        fetch_yahoo_json_news(&client, spec.yahoo_query, 10),
        fetch_google_news_rss(&client, spec.google_query),
    );
    let collated_news = collate_news([yahoo_news, google_news].concat());
    let (news_pos, news_neg) = headline_tally(collated_news.iter().map(|n| n.title.as_str()));
    if news_pos + news_neg > 0 {
        let news_score = (news_pos as f64 / (news_pos + news_neg) as f64) * 100.0;
        score = (score * 0.7 + news_score * 0.3).clamp(0.0, 100.0);
        let label = if news_score > 55.0 {
            "mostly positive"
        } else if news_score < 45.0 {
            "mostly negative"
        } else {
            "mixed"
        };
        factors.push(format!(
            "Headline wording: {} ({} positive / {} negative of {})",
            label,
            news_pos,
            news_neg,
            collated_news.len()
        ));
    }

    let (label, color) = match score {
        s if s >= 80.0 => ("EXTREME GREED", "green"),
        s if s >= 60.0 => ("GREED", "green"),
        s if s >= 40.0 => ("NEUTRAL", "yellow"),
        s if s >= 20.0 => ("FEAR", "red"),
        _ => ("EXTREME FEAR", "red"),
    };

    print_header(&format!(
        "{} Market Sentiment — Fear & Greed",
        market.label()
    ));

    // Gauge
    let gauge_width = 50;
    let pos = (score / 100.0 * gauge_width as f64) as usize;
    let mut gauge = String::new();
    for i in 0..gauge_width {
        let ch = if i == pos {
            "▼".bold().to_string()
        } else if i < 10 {
            "█".red().to_string()
        } else if i < 20 {
            "█".red().dimmed().to_string()
        } else if i < 30 {
            "█".yellow().to_string()
        } else if i < 40 {
            "█".green().dimmed().to_string()
        } else {
            "█".green().to_string()
        };
        gauge.push_str(&ch);
    }
    println!("\n  {}  {}  {}", "Fear".red(), gauge, "Greed".green());
    let score_display = match color {
        "green" => format!("{:.0}", score).green().bold().to_string(),
        "red" => format!("{:.0}", score).red().bold().to_string(),
        _ => format!("{:.0}", score).yellow().bold().to_string(),
    };
    println!("  {:>28} {} {}", "", score_display, label.bold());

    print_section("Indices");
    let mut shown: Vec<(&str, &str)> = vec![spec.vix, spec.index, spec.breadth];
    shown.extend(spec.others.iter().copied());
    for (sym, name) in shown {
        let q = quote(sym);
        let price = q
            .and_then(|q| q.regular_market_price)
            .map_or("—".to_string(), |p| format!("{p:.2}"));
        let chg = q.and_then(|q| q.regular_market_change_percent).map_or(
            "—".dimmed().to_string(),
            |c| {
                let s = format!("{c:+.2}%");
                if c >= 0.0 {
                    s.green().to_string()
                } else {
                    s.red().to_string()
                }
            },
        );
        println!("  {:<18} {:>12} {:>10}", name, price, chg);
    }

    print_section("Contributing Factors");
    for f in &factors {
        println!("  {} {}", "→".cyan(), f);
    }
    println!(
        "  {}",
        format!(
            "{} bands: below {} extreme greed, {}–{} greed, {}–{} neutral, {}–{} fear, above {} extreme fear.",
            spec.vix.1,
            spec.vix_bands[0],
            spec.vix_bands[0],
            spec.vix_bands[1],
            spec.vix_bands[1],
            spec.vix_bands[2],
            spec.vix_bands[2],
            spec.vix_bands[3],
            spec.vix_bands[3]
        )
        .dimmed()
    );

    if !collated_news.is_empty() {
        print_section("Top Market Headlines");
        print_news_items(&collated_news, 5);
    }
    println!(
        "\n  {}",
        "A reading of market mood from public data, not a signal to trade.".dimmed()
    );
    println!();
    Ok(())
}

// ── Tax Harvest ──

/// One match in `screen --json/--csv`.
#[derive(Debug, Serialize)]
pub(crate) struct ScreenRow {
    pub(crate) symbol: String,
    pub(crate) name: Option<String>,
    pub(crate) price: Option<f64>,
    pub(crate) change_percent: Option<f64>,
    pub(crate) pe: Option<f64>,
    pub(crate) market_cap: Option<f64>,
    pub(crate) dividend_yield: Option<f64>,
    pub(crate) currency: Option<String>,
    pub(crate) cap_tier: &'static str,
}

impl CsvRow for ScreenRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "price",
        "change_percent",
        "pe",
        "market_cap",
        "dividend_yield",
        "currency",
        "cap_tier",
    ];
}

impl From<&crate::api::Quote> for ScreenRow {
    fn from(q: &crate::api::Quote) -> Self {
        ScreenRow {
            symbol: q.symbol.clone().unwrap_or_default(),
            name: q.short_name.clone().or_else(|| q.long_name.clone()),
            price: q.regular_market_price,
            change_percent: q.regular_market_change_percent,
            pe: q.trailing_pe,
            market_cap: q.market_cap,
            dividend_yield: q.dividend_yield_fraction(),
            currency: q.currency.clone(),
            cap_tier: market::cap_tier_of(q).as_str(),
        }
    }
}

/// The quotes a preset screen keeps, largest first (momentum: biggest
/// gainers first), and the screen's title.
pub(crate) fn screen_matches<'a>(
    category: &str,
    quotes: &'a [crate::api::Quote],
) -> Result<(String, Vec<&'a crate::api::Quote>)> {
    let (title, mut filtered): (String, Vec<&crate::api::Quote>) = match category {
        "undervalued" => ("Undervalued Stocks (P/E < 15)".into(),
            quotes.iter().filter(|q| q.trailing_pe.is_some_and(|pe| pe > 0.0 && pe < 15.0)).collect()),
        "growth" => ("Growth Stocks (EPS Forward > EPS TTM)".into(),
            quotes.iter().filter(|q| matches!((q.eps_trailing_twelve_months, q.eps_forward), (Some(ttm), Some(fwd)) if fwd > ttm && ttm > 0.0)).collect()),
        "dividend" => ("Dividend Stocks (Yield > 2%)".into(),
            quotes.iter().filter(|q| q.dividend_yield_fraction().is_some_and(|d| d > 0.02)).collect()),
        "momentum" => {
            let mut s: Vec<&crate::api::Quote> = quotes.iter().collect();
            s.retain(|q| q.regular_market_change_percent.is_some());
            s.sort_by(|a, b| b.regular_market_change_percent.partial_cmp(&a.regular_market_change_percent).unwrap_or(std::cmp::Ordering::Equal));
            s.truncate(15);
            ("Momentum Leaders (top daily gainers)".into(), s)
        }
        "bluechip" => ("Blue Chip (Market Cap > $100B / ₹5L Cr)".into(),
            // $100B for USD listings, ₹5L Cr (5e12) for INR listings.
            quotes.iter().filter(|q| {
                let floor = if market::is_inr(q.currency.as_deref()) { 5e12 } else { 1e11 };
                q.market_cap.is_some_and(|mc| mc > floor)
            }).collect()),
        _ => bail!("Unknown screen '{}'. Use one of: {}", category, market::SCREENER_CATEGORIES.join(", ")),
    };

    if category != "momentum" {
        filtered.sort_by(|a, b| {
            b.market_cap
                .unwrap_or(0.0)
                .partial_cmp(&a.market_cap.unwrap_or(0.0))
                .unwrap()
        });
    }

    Ok((title, filtered))
}

/// Most rows a `screen` table prints; `--json` and `--csv` carry them all.
const SCREEN_ROWS: usize = 50;

pub async fn cmd_screen(
    category: &str,
    market: Market,
    universe: Option<market::Universe>,
    cap: Option<market::CapFilter>,
    fmt: Format,
) -> Result<()> {
    market.require_stocks("screen")?;
    check_universe(market, universe)?;
    let client = YahooClient::new().await?;
    let scan = scan_symbols(&client, market, universe).await?;
    let quotes = scan_quotes(&client, market, &scan, cap, false).await?;
    let scope = cap_scope(&scan.scope, cap);

    let (title, filtered) = screen_matches(category, &quotes)?;

    if !fmt.is_table() {
        let rows: Vec<ScreenRow> = filtered.iter().map(|q| ScreenRow::from(*q)).collect();
        return match fmt {
            Format::Csv => output::print_csv(&rows),
            _ => output::print_json(&rows),
        };
    }

    let title = if universe.is_some() || cap.is_some_and(|c| c.tier().is_some()) {
        format!("{} — {}", title, scope)
    } else {
        title
    };
    print_header(&title);
    if filtered.is_empty() {
        println!("  {}", "No stocks match this screen.".dimmed());
        return Ok(());
    }
    println!(
        "  {:<14} {:>10} {:>10} {:>10} {:>14} {:>6}",
        "Symbol".bold(),
        "Price".bold(),
        "Change %".bold(),
        "P/E".bold(),
        "Market Cap".bold(),
        "Cap".bold()
    );
    println!("  {}", "─".repeat(69).dimmed());

    for q in filtered.iter().take(SCREEN_ROWS) {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 {
            format!("+{:.2}%", pct).green().to_string()
        } else {
            format!("{:.2}%", pct).red().to_string()
        };
        println!(
            "  {:<14} {:>10} {:>10} {:>10} {:>14} {:>6}",
            sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            pct_str,
            q.trailing_pe
                .map_or("N/A".to_string(), |v| format!("{:.1}", v)),
            q.market_cap
                .map_or("N/A".to_string(), |v| format_large_number(v, cur)),
            market::cap_tier_of(q).badge().dimmed()
        );
    }
    if filtered.len() > SCREEN_ROWS {
        println!(
            "  {}",
            format!(
                "... {} more (--json or --csv lists them all) ...",
                filtered.len() - SCREEN_ROWS
            )
            .dimmed()
        );
    }
    println!("\n  {} {} stocks matched", "→".cyan(), filtered.len());

    // AI screen analysis
    let mut ai = crate::ai::AiClient::connect().await;
    if ai.auto_available().await && !filtered.is_empty() {
        let mut data = format!("Screen: {}\n", category);
        for q in filtered.iter().take(8) {
            let sym = q.symbol.as_deref().unwrap_or("?");
            data.push_str(&format!(
                "{}: price={:.2}, P/E={}, chg={:+.2}%\n",
                sym,
                q.regular_market_price.unwrap_or(0.0),
                q.trailing_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                q.regular_market_change_percent.unwrap_or(0.0)
            ));
        }
        print_section(&format!("AI Analysis ({})", ai.provider().name()));
        if let Ok(analysis) = ai.analyze_screen(&data).await {
            print_ai_answer(&analysis);
        }
    }

    println!();
    Ok(())
}

// ── Dashboard ──

pub async fn cmd_picks(
    market: Market,
    universe: Option<market::Universe>,
    cap: Option<market::CapFilter>,
    floors: crate::picks::Floors,
    horizon: crate::picks::Horizon,
    top: usize,
    fmt: Format,
) -> Result<()> {
    market.require_stocks("picks")?;
    let client = YahooClient::new().await?;
    // A cap tier is ranked within itself: large caps against large caps.
    let run = crate::picks::run(&client, market, universe, cap, floors, horizon, top).await?;
    let bench_level = benchmark_level(&client, market).await;
    let scope = cap_scope(universe.map_or(market.label(), |u| u.nse_index()), cap);
    log_picks(market, &scope, horizon, &run.picks, bench_level);

    if !fmt.is_table() {
        return match fmt {
            Format::Csv => output::print_csv(&run.picks),
            _ => output::print_json(&run),
        };
    }

    let title = match horizon {
        crate::picks::Horizon::Long => "Highest Scores, long term",
        crate::picks::Horizon::Short => "Highest Scores, short term (weeks to months)",
    };
    print_header(&format!("{title} — {scope}"));
    println!(
        "  {} {} stocks ranked against each other on value, quality, growth, trend and risk",
        "⟳".yellow(),
        run.scanned
    );
    if run.below_floor > 0 {
        println!(
            "  {} {} more left out: market cap under {} or under {} traded a day (--min-mcap / --min-adv)",
            "ⓘ".dimmed(),
            run.below_floor,
            crate::picks::describe_amount(market, floors.min_mcap),
            crate::picks::describe_amount(market, floors.min_adv)
        );
    }
    if run.thin_data > 0 {
        println!(
            "  {} {} more left out: less than {:.0}% of their data came through",
            "ⓘ".dimmed(),
            run.thin_data,
            crate::picks::MIN_COVERAGE * 100.0
        );
    }
    println!();
    println!(
        "  {:<14} {:>10} {:>8} {:>6} {:>6} {:>8} {:>7} {:>6} {:>5} {:>5}  {:<12} {}",
        "Symbol".bold(),
        "Price".bold(),
        "Chg%".bold(),
        "Score".bold(),
        "Value".bold(),
        "Quality".bold(),
        "Growth".bold(),
        "Trend".bold(),
        "Risk".bold(),
        "Data".bold(),
        "Action".bold(),
        "Top fifth on".bold()
    );
    println!("  {}", "─".repeat(115).dimmed());

    let pillar = |v: Option<f64>| v.map_or("–".to_string(), |v| format!("{:.0}", v));
    for p in &run.picks {
        let pct = p.change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 {
            format!("{:>8}", format!("+{:.2}%", pct))
                .green()
                .to_string()
        } else {
            format!("{:>8}", format!("{:.2}%", pct)).red().to_string()
        };
        let score = format!("{:>6.0}", p.score);
        let score_str = if p.score >= 75.0 {
            score.green().bold().to_string()
        } else if p.score >= 60.0 {
            score.yellow().to_string()
        } else {
            score.dimmed().to_string()
        };
        println!(
            "  {:<14} {:>10} {} {} {:>6} {:>8} {:>7} {:>6} {:>5} {:>4.0}%  {} {}",
            p.symbol.cyan(),
            format_price(p.price.unwrap_or(0.0), p.currency.as_deref()),
            pct_str,
            score_str,
            pillar(p.value),
            pillar(p.quality),
            pillar(p.growth),
            pillar(p.trend),
            pillar(p.risk),
            p.coverage * 100.0,
            super::portfolio::action_cell(p.action.as_ref()),
            p.strongest().join(", ").dimmed()
        );
        if let Some(a) = &p.action {
            println!(
                "  {:<14}   {}",
                "",
                super::actions::format_action_line(a).dimmed()
            );
        }
    }
    if !run.flagged.is_empty() {
        let names: Vec<String> = run
            .flagged
            .iter()
            .take(6)
            .map(|p| format!("{} ({})", p.symbol, p.flags.join(", ")))
            .collect();
        println!(
            "\n  {} {} left out to check data: {}{}",
            "⚠".yellow(),
            run.flagged.len(),
            names.join("; "),
            if run.flagged.len() > 6 { "; …" } else { "" }
        );
    }
    let trend_lines: [&str; 2] = match horizon {
        crate::picks::Horizon::Long => [
            "Growth: forward against trailing EPS (3+ analysts), revenue growth. Trend: 12-1 month momentum per",
            "unit of volatility, price against the 200-day average. Risk: low volatility, drawdown and beta.",
        ],
        crate::picks::Horizon::Short => [
            "Growth: forward against trailing EPS, revenue growth. Trend (45% of the score): 3-month momentum per",
            "unit of volatility, price against the 50-day average, 52-week range, volume against usual. Risk: 20%.",
        ],
    };
    for line in [
        "Each number is a percentile rank among the stocks scanned (100 is the highest). Value: earnings",
        "yield, book value to price, dividend yield. Quality: return on equity, profit margin, low debt.",
        trend_lines[0],
        trend_lines[1],
        "Losses and negative book value rank last. Data: share of the score that had data; the rest pulls",
        "the score toward 50. With a positive --top, at most 3 stocks per known sector. `picks review`: past lists.",
        "Action: rule-based (methodology 2), as if not held: BUY / WATCH / AVOID or NO CALL on thin data. The rank",
        "is relative to this scan; the action applies fixed rules. `stockwise verdict SYM` shows the reasons.",
    ] {
        println!("  {}", line.dimmed().italic());
    }
    if cap.is_none() && matches!(market, Market::In | Market::Us) {
        println!(
            "  {}",
            "Small caps rank against each other with `--cap small` (or `--cap micro`), with floors sized for them."
                .dimmed()
                .italic()
        );
    }
    println!("  {}", crate::action::DISCLAIMER.dimmed().italic());
    println!("  {}", run.meta.footer().dimmed());
    println!();
    Ok(())
}

/// The benchmark's level now, logged with each list so `picks review`
/// compares the stock and the index at the same moment.
pub(crate) async fn benchmark_level(client: &YahooClient, market: Market) -> Option<f64> {
    let (sym, _) = crate::picks::benchmark(market)?;
    client
        .get_quote(&[sym])
        .await
        .ok()?
        .into_iter()
        .next()?
        .regular_market_price
        .filter(|p| *p > 0.0)
}

/// Append a list to the track record; a failure to save never fails the scan.
/// Each pick is filed under the exchange session its price came from (a US
/// quote from Monday afternoon in New York is Monday's, wherever this runs).
pub(crate) fn log_picks(
    market: Market,
    scope: &str,
    horizon: crate::picks::Horizon,
    picks: &[crate::picks::Pick],
    benchmark_price: Option<f64>,
) {
    let now = chrono::Utc::now();
    let entries: Vec<crate::picks_log::Entry> = picks
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let at = p
                .quoted_at
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .unwrap_or(now);
            Some(crate::picks_log::Entry {
                date: crate::clock::session_date(market, at),
                kind: "pick".into(),
                horizon: horizon.as_str().into(),
                market: market.code().into(),
                universe: scope.into(),
                symbol: p.symbol.clone(),
                rank: i + 1,
                score: p.score,
                price: p.price.filter(|x| *x > 0.0)?,
                currency: p.currency.clone(),
                benchmark_price,
                methodology: crate::picks::METHODOLOGY.into(),
            })
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    if let Err(e) = crate::picks_log::record(entries) {
        eprintln!("  ⓘ Could not save this list to the track record: {e:#}");
    }
}

/// Shares one share listed on `since` became through splits and bonus
/// issues after that date; `None` when Yahoo's actions could not be read,
/// or when one of them is malformed (a split that can't be applied must not
/// read as "no split" and show a false loss).
async fn split_factor(client: &YahooClient, symbol: &str, since: chrono::NaiveDate) -> Option<f64> {
    let start = since.and_hms_opt(0, 0, 0)?.and_utc().timestamp();
    let events = client.get_corporate_actions(symbol, start).await.ok()?;
    let mut factor = 1.0;
    for s in events.splits.values() {
        let day = chrono::DateTime::from_timestamp(s.date, 0)?.date_naive();
        if day <= since {
            continue;
        }
        if !(s.numerator > 0.0 && s.denominator > 0.0) {
            return None;
        }
        factor *= s.numerator / s.denominator;
    }
    Some(factor)
}

/// The track record of `market`'s `horizon` lists logged in the last
/// `since_days`: each pick measured from its first listing against the
/// market's benchmark. Shared by `picks review` and the MCP tool.
pub(crate) async fn picks_review_report(
    client: &YahooClient,
    market: Market,
    since_days: u32,
    horizon: crate::picks::Horizon,
) -> Result<crate::picks_log::Review> {
    use futures_util::StreamExt;
    market.require_stocks("picks review")?;
    let today = crate::clock::session_date(market, chrono::Utc::now());
    let since = today
        .checked_sub_days(chrono::Days::new(since_days as u64))
        .ok_or_else(|| anyhow::anyhow!("--since {since_days} days reaches before any date"))?;
    let log =
        crate::picks_log::load().context("Could not read the track record (signal_log.json)")?;
    let mut first: std::collections::BTreeMap<String, chrono::NaiveDate> = Default::default();
    for e in log.iter().filter(|e| {
        e.kind == "pick"
            && e.market == market.code()
            && e.horizon == horizon.as_str()
            && e.date >= since
    }) {
        let d = first.entry(e.symbol.clone()).or_insert(e.date);
        *d = (*d).min(e.date);
    }
    if first.is_empty() {
        bail!(
            "No picks logged for this market in the last {} days. Every `stockwise picks` run is \
             logged; run one now and review it later.",
            since_days
        );
    }
    let refs: Vec<&str> = first.keys().map(String::as_str).collect();
    let quotes = client
        .get_quote(&refs)
        .await
        .context("Could not fetch current prices for the logged picks")?;
    // Picks are dated by the session of their quote, so days held count to
    // the session of each quote's own timestamp (falling back to `today` when
    // a quote carries no timestamp).
    let mut prices: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut quote_dates: std::collections::HashMap<String, chrono::NaiveDate> =
        std::collections::HashMap::new();
    for q in quotes {
        let (Some(sym), Some(px)) = (q.symbol, q.regular_market_price) else {
            continue;
        };
        if let Some(d) = q
            .regular_market_time
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|at| crate::clock::session_date(market, at))
            .filter(|d| *d <= today)
        {
            quote_dates.insert(sym.clone(), d);
        }
        prices.insert(sym, px);
    }
    // Owned jobs: a stream over borrowed keys trips the `Send` bound of the
    // MCP handler that calls this.
    let jobs: Vec<(String, chrono::NaiveDate)> =
        first.iter().map(|(s, d)| (s.clone(), *d)).collect();
    let splits: std::collections::HashMap<String, Option<f64>> = futures_util::stream::iter(jobs)
        .map(|(sym, date): (String, chrono::NaiveDate)| async move {
            let f = split_factor(client, &sym, date).await;
            (sym, f)
        })
        .buffer_unordered(6)
        .collect()
        .await;
    // The benchmark leg compares the level logged with each list with the
    // live level now: matched observations, no daily-close fallback.
    let bench = crate::picks::benchmark(market).map(|(_, name)| name);
    let bench_now = match bench {
        Some(_) => benchmark_level(client, market).await,
        None => None,
    };
    Ok(crate::picks_log::review_with_dates(
        &log,
        market.code(),
        horizon.as_str(),
        since,
        today,
        &prices,
        &quote_dates,
        &splits,
        bench.map(|name| crate::picks_log::Benchmark {
            name,
            now: bench_now,
        }),
    ))
}

/// `picks review`: every pick logged in the last `since_days`, measured from
/// its first listing against the market's benchmark.
pub async fn cmd_picks_review(
    market: Market,
    since_days: u32,
    horizon: crate::picks::Horizon,
    fmt: Format,
) -> Result<()> {
    market.require_stocks("picks review")?;
    let client = YahooClient::new().await?;
    let review = picks_review_report(&client, market, since_days, horizon).await?;
    let since = review.since;
    match fmt {
        Format::Csv => return output::print_csv(&review.rows),
        Format::Json => return output::print_json(&review),
        _ => {}
    }
    print_header(&format!(
        "Picks Track Record — {}, {} term (since {})",
        market.label(),
        horizon.as_str(),
        since.format("%d %b %Y")
    ));
    println!(
        "  {} each pick from the day it was first listed; price return, dividends not included",
        "ⓘ".dimmed()
    );
    if let Some(b) = &review.benchmark {
        println!("  {} benchmark: {}", "ⓘ".dimmed(), b);
    }
    println!();
    println!(
        "  {:<14} {:>11} {:>5} {:>6} {:>10} {:>10} {:>8} {:>8} {:>8}",
        "Symbol".bold(),
        "Listed".bold(),
        "Days".bold(),
        "Score".bold(),
        "Then".bold(),
        "Now".bold(),
        "Return".bold(),
        "Index".bold(),
        "Excess".bold()
    );
    println!("  {}", "─".repeat(90).dimmed());
    let pct = |v: Option<f64>| match v {
        Some(v) if v >= 0.0 => format!("{:>8}", format!("+{:.1}%", v * 100.0))
            .green()
            .to_string(),
        Some(v) => format!("{:>8}", format!("{:.1}%", v * 100.0))
            .red()
            .to_string(),
        None => format!("{:>8}", "–"),
    };
    for r in &review.rows {
        println!(
            "  {:<14} {:>11} {:>5} {:>6.0} {:>10} {:>10} {} {} {}",
            r.symbol.cyan(),
            r.picked_on.format("%d %b %Y").to_string(),
            r.days,
            r.score,
            format_price(r.entry_price, r.currency.as_deref()),
            format_price(r.price, r.currency.as_deref()),
            pct(Some(r.return_pct)),
            pct(r.benchmark_pct),
            pct(r.excess_pct)
        );
    }
    println!();
    let share = |v: Option<f64>| v.map_or("–".to_string(), |v| format!("{:.0}%", v * 100.0));
    println!(
        "  {} picks · rose: {} · beat the index: {} · mean return {} (median {}) · mean excess {} (median {})",
        review.rows.len(),
        share(review.hit_rate),
        share(review.beat_rate),
        pct(review.mean_return).trim(),
        pct(review.median_return).trim(),
        pct(review.mean_excess).trim(),
        pct(review.median_excess).trim()
    );
    if !review.unverified.is_empty() {
        println!(
            "  {} left out, splits since listing could not be checked: {}",
            "⚠".yellow(),
            review.unverified.join(", ")
        );
    }
    if !review.unpriced.is_empty() {
        println!(
            "  {} no current price for: {}",
            "⚠".yellow(),
            review.unpriced.join(", ")
        );
    }
    if review.methodologies.len() > 1 {
        println!(
            "  {} lists from different scoring versions ({}); see docs/methodology.md",
            "ⓘ".dimmed(),
            review.methodologies.join(", ")
        );
    }
    println!();
    Ok(())
}

// ── Sentiment / Fear & Greed ──

/// One stock of `earnings`: its next results date, if any source lists one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EarningsRow {
    pub(crate) symbol: String,
    pub(crate) date: Option<chrono::NaiveDate>,
    pub(crate) days_until: Option<i64>,
    /// The source marks the date as an estimate (or gives a window).
    pub(crate) estimate: bool,
    pub(crate) source: Option<crate::calendar::Source>,
}

/// The next results date of each symbol on or after its exchange's today
/// (NSE's filed date first, else Yahoo's; see
/// [`crate::calendar::next_results_in`]), soonest first; symbols without a
/// date last, by symbol.
pub(crate) fn earnings_rows(
    events: &[(String, Vec<crate::calendar::Event>)],
    today_for: impl Fn(&str) -> chrono::NaiveDate,
) -> Vec<EarningsRow> {
    use crate::calendar::EventKind;
    let mut rows: Vec<EarningsRow> = events
        .iter()
        .map(|(symbol, evs)| {
            let today = today_for(symbol);
            let date = crate::calendar::next_results_in(evs, today);
            let event = date.and_then(|d| {
                let mut on_day: Vec<&crate::calendar::Event> = evs
                    .iter()
                    .filter(|e| e.kind == EventKind::Results && e.date == d)
                    .collect();
                // NSE's filing wins over Yahoo's estimate for the same day.
                on_day.sort_by_key(|e| e.source != crate::calendar::Source::Nse);
                on_day.first().copied()
            });
            EarningsRow {
                symbol: symbol.clone(),
                date,
                days_until: date.map(|d| (d - today).num_days()),
                estimate: event.is_some_and(|e| e.estimate),
                source: event.map(|e| e.source),
            }
        })
        .collect();
    rows.sort_by(|a, b| match (a.date, b.date) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.symbol.cmp(&b.symbol)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.symbol.cmp(&b.symbol),
    });
    rows
}

pub async fn cmd_earnings(_market: Market) -> Result<()> {
    use crate::calendar::Calendar;
    let client = YahooClient::new().await?;

    // Stocks in the portfolio and the watchlist (funds, indices and coins
    // have no results).
    let portfolio = Portfolio::load()?;
    let wl = Watchlist::load()?;
    let mut all_symbols: Vec<String> = Vec::new();
    for s in portfolio
        .holdings
        .iter()
        .map(|h| &h.symbol)
        .chain(wl.symbols.iter())
    {
        if market::AssetClass::of(s) == market::AssetClass::Stock && !all_symbols.contains(s) {
            all_symbols.push(s.clone());
        }
    }

    if all_symbols.is_empty() {
        println!(
            "\n  {}",
            "Add stocks to portfolio or watchlist first.".dimmed()
        );
        return Ok(());
    }

    let mut cal = Calendar::new(&client);
    let pending = cal.nse_to_fetch(&all_symbols);
    if pending >= 3 {
        eprintln!(
            "  {} Reading NSE filings for {} stocks (about {} s; cached for a day)…",
            "→".dimmed(),
            pending,
            pending * 2
        );
    }
    let mut events = Vec::new();
    for s in &all_symbols {
        events.push((s.clone(), cal.events_for(s).await));
    }
    cal.save();

    let now = chrono::Utc::now();
    let rows = earnings_rows(&events, |s| crate::calendar::exchange_today(s, now));
    // EPS figures alongside; a failed quote only leaves them blank.
    let sym_refs: Vec<&str> = all_symbols.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&sym_refs).await.unwrap_or_default();
    let quote = |sym: &str| quotes.iter().find(|q| q.symbol.as_deref() == Some(sym));

    print_header("Upcoming Results");
    println!(
        "  {:<14} {:>12} {:>9} {:>10} {:>10}  {}",
        "Symbol".bold(),
        "Next results".bold(),
        "In".bold(),
        "EPS TTM".bold(),
        "EPS Fwd".bold(),
        "Source".bold()
    );
    println!("  {}", "─".repeat(70).dimmed());
    for r in &rows {
        let q = quote(&r.symbol);
        let date = match r.date {
            Some(d) if r.estimate => format!("~{}", d.format("%d %b %Y")),
            Some(d) => d.format("%d %b %Y").to_string(),
            None => "—".to_string(),
        };
        let days = match r.days_until {
            Some(0) => "today".to_string(),
            Some(1) => "tomorrow".to_string(),
            Some(n) => format!("{n} days"),
            None => "—".to_string(),
        };
        let eps = |v: Option<f64>| v.map_or("—".to_string(), |v| format!("{:.2}", v));
        println!(
            "  {:<14} {:>12} {:>9} {:>10} {:>10}  {}",
            r.symbol.cyan(),
            date,
            days,
            eps(q.and_then(|q| q.eps_trailing_twelve_months)),
            eps(q.and_then(|q| q.eps_forward)),
            r.source.map_or("—".to_string(), |s| s.to_string()).dimmed()
        );
    }

    let dated = rows.iter().filter(|r| r.date.is_some()).count();
    println!(
        "\n  {} {} of {} stocks have a results date listed. ~ marks an estimated date.",
        "→".cyan(),
        dated,
        rows.len()
    );
    if let Some(why) = cal.nse_note()
        && crate::calendar::has_indian(&all_symbols)
    {
        println!("  {} {}", "!".yellow(), why.dimmed());
    }
    if !cal.yahoo_missing.is_empty() {
        println!(
            "  {} {}",
            "!".yellow(),
            format!(
                "Yahoo Finance had no calendar for {}",
                cal.yahoo_missing.join(", ")
            )
            .dimmed()
        );
    }
    println!();
    Ok(())
}

// ── Long-Term Wealth Bot ──

/// Look-back windows of the relative-strength columns, in months.
const RS_MONTHS: [u32; 3] = [1, 3, 6];

/// Each `(symbol, name)` with the quote whose symbol matches, `None` when
/// Yahoo returned none. Never by position: Yahoo may drop or reorder
/// symbols, and pairing by index would put one sector's numbers under
/// another's name.
pub(crate) fn pair_by_symbol<'a, 'q>(
    names: &[(&'a str, &'a str)],
    quotes: &'q [crate::api::Quote],
) -> Vec<(&'a str, &'a str, Option<&'q crate::api::Quote>)> {
    names
        .iter()
        .map(|&(sym, name)| {
            let q = quotes.iter().find(|q| {
                q.symbol
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(sym))
            });
            (sym, name, q)
        })
        .collect()
}

/// The index sectors are measured against.
fn sector_benchmark(market: Market) -> Option<(&'static str, &'static str)> {
    match market {
        Market::In => Some(("^NSEI", "NIFTY 50")),
        Market::Us => Some(("^GSPC", "S&P 500")),
        Market::Global(x) => Some(x.info().benchmark()),
        _ => None,
    }
}

/// Percent change from the close on or before `months` before the last
/// close to the last close (closes oldest first). `None` when the history
/// does not reach back that far.
pub(crate) fn period_return(closes: &[(chrono::NaiveDate, f64)], months: u32) -> Option<f64> {
    let &(last_date, last) = closes.last()?;
    let start = last_date.checked_sub_months(chrono::Months::new(months))?;
    let idx = closes.partition_point(|(d, _)| *d <= start);
    let (_, base) = closes.get(idx.checked_sub(1)?)?;
    (*base > 0.0 && last.is_finite()).then(|| (last / base - 1.0) * 100.0)
}

/// How much better (positive) or worse a sector did than the benchmark,
/// percent: `(1 + sector) / (1 + benchmark) - 1`.
pub(crate) fn relative_strength(sector: Option<f64>, bench: Option<f64>) -> Option<f64> {
    let (s, b) = (sector?, bench?);
    let denom = 1.0 + b / 100.0;
    (denom > 0.0).then(|| ((1.0 + s / 100.0) / denom - 1.0) * 100.0)
}

/// One sector of `rotation` / `sectorcmp`, matched to its own quote and
/// history by symbol.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SectorReading {
    pub(crate) name: String,
    pub(crate) symbol: String,
    pub(crate) price: Option<f64>,
    pub(crate) day: Option<f64>,
    pub(crate) currency: Option<String>,
    /// The sector's own return over [`RS_MONTHS`].
    pub(crate) returns: [Option<f64>; 3],
    /// Relative strength against the benchmark over [`RS_MONTHS`].
    pub(crate) rs: [Option<f64>; 3],
}

impl SectorReading {
    /// "Leading" when ahead of the benchmark over both one and three
    /// months, "Lagging" when behind over both, "Mixed" otherwise.
    fn trend(&self) -> Option<&'static str> {
        match (self.rs[0]?, self.rs[1]?) {
            (a, b) if a > 0.0 && b > 0.0 => Some("Leading"),
            (a, b) if a < 0.0 && b < 0.0 => Some("Lagging"),
            _ => Some("Mixed"),
        }
    }
}

/// Readings for `names`: each quote and chart is found by its symbol, so a
/// dropped quote or chart leaves only that sector's cells empty.
/// `histories` holds daily closes by symbol; `bench` the benchmark's.
pub(crate) fn sector_readings(
    names: &[(&str, &str)],
    quotes: &[crate::api::Quote],
    histories: &[(String, Vec<(chrono::NaiveDate, f64)>)],
    bench: &[(chrono::NaiveDate, f64)],
) -> Vec<SectorReading> {
    pair_by_symbol(names, quotes)
        .into_iter()
        .map(|(sym, name, q)| {
            let closes = histories
                .iter()
                .find(|(s, _)| s.eq_ignore_ascii_case(sym))
                .map(|(_, c)| c.as_slice())
                .unwrap_or_default();
            let returns = RS_MONTHS.map(|m| period_return(closes, m));
            let rs = RS_MONTHS.map(|m| {
                let (s_ret, b_ret) = shared_period_returns(closes, bench, m)?;
                relative_strength(Some(s_ret), Some(b_ret))
            });
            SectorReading {
                name: name.to_string(),
                symbol: sym.to_string(),
                price: q.and_then(|q| q.regular_market_price),
                day: q.and_then(|q| q.regular_market_change_percent),
                currency: q.and_then(|q| q.currency.clone()),
                returns,
                rs,
            }
        })
        .collect()
}

/// Sector and benchmark returns over a window whose start and end dates are
/// shared by both series. Returns `None` if the two series do not have
/// matching end and start dates.
fn shared_period_returns(
    sector: &[(chrono::NaiveDate, f64)],
    bench: &[(chrono::NaiveDate, f64)],
    months: u32,
) -> Option<(f64, f64)> {
    let &(end_date, s_end) = sector
        .iter()
        .rev()
        .find(|(d, _)| bench.binary_search_by_key(d, |(bd, _)| *bd).is_ok())?;
    let b_end_idx = bench.binary_search_by_key(&end_date, |(bd, _)| *bd).ok()?;
    let b_end = bench[b_end_idx].1;
    let target_start = end_date.checked_sub_months(chrono::Months::new(months))?;
    let s_idx = sector.partition_point(|(d, _)| *d <= target_start);
    let &(s_start_date, s_base) = sector.get(s_idx.checked_sub(1)?)?;
    let b_idx = bench.partition_point(|(d, _)| *d <= target_start);
    let &(b_start_date, b_base) = bench.get(b_idx.checked_sub(1)?)?;
    if s_start_date != b_start_date
        || s_base <= 0.0
        || b_base <= 0.0
        || !s_end.is_finite()
        || !b_end.is_finite()
    {
        return None;
    }
    Some((
        (s_end / s_base - 1.0) * 100.0,
        (b_end / b_base - 1.0) * 100.0,
    ))
}

/// Daily closes of `symbols` over the past year, by symbol; a failed chart
/// is left out.
async fn yearly_closes(
    client: &YahooClient,
    symbols: Vec<String>,
) -> Vec<(String, Vec<(chrono::NaiveDate, f64)>)> {
    use futures_util::StreamExt;
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
        .filter(|(_, c)| std::future::ready(!c.is_empty()))
        .collect()
        .await
}

/// A signed percentage, green or red; "—" when missing.
fn pct_cell(v: Option<f64>, decimals: usize) -> String {
    match v.filter(|v| v.is_finite()) {
        Some(v) => {
            let s = format!("{:+.*}%", decimals, v);
            if v >= 0.0 {
                s.green().to_string()
            } else {
                s.red().to_string()
            }
        }
        None => "—".dimmed().to_string(),
    }
}

/// Fetch every sector's quote and a year of daily closes, plus the
/// benchmark's.
async fn fetch_sector_readings(
    client: &YahooClient,
    names: &[(&str, &str)],
    bench: &str,
) -> Result<Vec<SectorReading>> {
    let syms: Vec<&str> = names.iter().map(|(s, _)| *s).collect();
    let mut chart_syms: Vec<String> = syms.iter().map(|s| s.to_string()).collect();
    chart_syms.push(bench.to_string());
    let (quotes, histories) =
        tokio::join!(client.get_quote(&syms), yearly_closes(client, chart_syms));
    let quotes = quotes?;
    let bench_closes = histories
        .iter()
        .find(|(s, _)| s == bench)
        .map(|(_, c)| c.clone())
        .unwrap_or_default();
    Ok(sector_readings(names, &quotes, &histories, &bench_closes))
}

pub async fn cmd_rotation(market: Market) -> Result<()> {
    market.require_sectors("rotation")?;
    let (bench, bench_name) = sector_benchmark(market)
        .with_context(|| format!("rotation has no benchmark for {}", market.label()))?;
    let client = YahooClient::new().await?;
    let mut rows = fetch_sector_readings(&client, market.sectors(), bench).await?;
    // Strongest over three months first; missing figures last.
    rows.sort_by(|a, b| match (a.rs[1], b.rs[1]) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    print_header(&format!("{} Sector Rotation", market.label()));
    println!(
        "  {:<16} {:>12} {:>9} {:>9} {:>9} {:>9}  {}",
        "Sector".bold(),
        "Price".bold(),
        "Day".bold(),
        "RS 1M".bold(),
        "RS 3M".bold(),
        "RS 6M".bold(),
        "Trend".bold()
    );
    println!("  {}", "─".repeat(78).dimmed());
    for r in &rows {
        let price = r.price.map_or("—".to_string(), |p| {
            format_price(p, r.currency.as_deref())
        });
        let trend = match r.trend() {
            Some("Leading") => "Leading".green().to_string(),
            Some("Lagging") => "Lagging".red().to_string(),
            Some(t) => t.yellow().to_string(),
            None => "—".dimmed().to_string(),
        };
        println!(
            "  {:<16} {:>12} {:>9} {:>9} {:>9} {:>9}  {}",
            r.name.bold(),
            price,
            pct_cell(r.day, 2),
            pct_cell(r.rs[0], 1),
            pct_cell(r.rs[1], 1),
            pct_cell(r.rs[2], 1),
            trend
        );
    }
    println!(
        "\n  {} RS: the sector's return relative to the {} over 1, 3 and 6 months of daily closes.",
        "→".cyan(),
        bench_name
    );
    println!(
        "  {} Leading: ahead of the {} over both 1 and 3 months; Lagging: behind over both. Past performance only.",
        "→".cyan(),
        bench_name
    );
    println!();
    Ok(())
}

// ── Quality-Growth Matrix ──

pub async fn cmd_matrix(market: Market, universe: Option<market::Universe>) -> Result<()> {
    market.require_stocks("matrix")?;
    let client = YahooClient::new().await?;
    let quotes = universe_quotes(&client, market, universe).await?;

    print_header("Quality vs Growth Matrix");

    // Quadrants: High Quality + High Growth = STARS, etc.
    let mut stars = Vec::new();
    let mut quality_plays = Vec::new();
    let mut growth_plays = Vec::new();
    let mut laggards = Vec::new();

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let roe = q.return_on_equity.unwrap_or(0.0);
        let margin = q.profit_margins.unwrap_or(0.0);
        let rev_growth = q.revenue_growth.unwrap_or(0.0);
        let quality = roe * 50.0 + margin * 50.0; // combined quality score
        let growth = rev_growth * 100.0;

        let entry = (
            sym.to_string(),
            quality,
            growth,
            q.trailing_pe.unwrap_or(0.0),
        );
        if quality > 10.0 && growth > 10.0 {
            stars.push(entry);
        } else if quality > 10.0 {
            quality_plays.push(entry);
        } else if growth > 10.0 {
            growth_plays.push(entry);
        } else {
            laggards.push(entry);
        }
    }

    let print_quad = |name: &str, _color: &str, items: &[(String, f64, f64, f64)]| {
        print_section(name);
        if items.is_empty() {
            println!("  {}", "None".dimmed());
            return;
        }
        for (sym, q, g, pe) in items.iter().take(8) {
            let sym_str = sym.cyan();
            println!(
                "  {:<14} Quality: {:>5.1}  Growth: {:>5.1}%  P/E: {:>5.1}",
                sym_str, q, g, pe
            );
        }
    };

    print_quad("Stars (High Quality + High Growth)", "green", &stars);
    print_quad(
        "Quality Plays (High Quality, Low Growth)",
        "cyan",
        &quality_plays,
    );
    print_quad(
        "Growth Plays (High Growth, Lower Quality)",
        "yellow",
        &growth_plays,
    );
    print_quad("Laggards", "red", &laggards);
    println!();
    Ok(())
}

// ── Earnings Surprise ──

/// Most stocks `surprise` asks Yahoo about (one request each): the largest
/// by market cap when the scan holds more.
const SURPRISE_MAX: usize = 50;

/// Within this many percent of the estimate counts as in line.
const SURPRISE_IN_LINE_PCT: f64 = 1.0;

/// The latest reported quarter of one stock, if Yahoo has it.
#[derive(Debug, Clone)]
pub(crate) struct SurpriseRow {
    pub(crate) symbol: String,
    pub(crate) latest: Option<crate::research::Surprise>,
}

impl SurpriseRow {
    /// Surprise in percent (Yahoo gives a fraction).
    fn pct(&self) -> Option<f64> {
        self.latest
            .as_ref()
            .map(|s| s.surprise_pct * 100.0)
            .filter(|v| v.is_finite())
    }
}

/// The latest quarter of each stock from its `earningsHistory`, largest
/// surprise first; stocks without one last, by symbol.
pub(crate) fn surprise_rows(
    histories: Vec<(String, Vec<crate::research::Surprise>)>,
) -> Vec<SurpriseRow> {
    let mut rows: Vec<SurpriseRow> = histories
        .into_iter()
        .map(|(symbol, mut h)| {
            h.sort_by_key(|s| s.quarter);
            SurpriseRow {
                symbol,
                latest: h.pop(),
            }
        })
        .collect();
    rows.sort_by(|a, b| match (a.pct(), b.pct()) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.symbol.cmp(&b.symbol),
    });
    rows
}

/// Parse all reported quarters from Yahoo `earningsHistory`, retaining
/// quarters that report `epsActual` even when `epsEstimate` is absent (with
/// `estimate` and `surprise_pct` set to `NaN` so `surprise_rows` keeps the
/// latest reported quarter instead of falling back to an older quarter).
pub(crate) fn parse_reported_quarters(
    summary: &serde_json::Value,
) -> Vec<crate::research::Surprise> {
    let raw = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.get("raw").or(Some(x)))
            .and_then(|r| r.as_f64())
    };
    let mut out: Vec<crate::research::Surprise> = summary
        .get("earningsHistory")
        .and_then(|e| e.get("history"))
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    let quarter = chrono::DateTime::from_timestamp(raw(h, "quarter")? as i64, 0)?
                        .date_naive();
                    let actual = raw(h, "epsActual")?;
                    let estimate = raw(h, "epsEstimate").unwrap_or(f64::NAN);
                    let surprise_pct = if estimate.is_finite() {
                        raw(h, "surprisePercent").unwrap_or_else(|| {
                            if estimate.abs() > 1e-9 {
                                (actual - estimate) / estimate.abs()
                            } else {
                                0.0
                            }
                        })
                    } else {
                        f64::NAN
                    };
                    Some(crate::research::Surprise {
                        quarter,
                        actual,
                        estimate,
                        surprise_pct,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|s| s.quarter);
    out
}

pub async fn cmd_surprise(market: Market, universe: Option<market::Universe>) -> Result<()> {
    use futures_util::StreamExt;
    market.require_stocks("surprise")?;
    let client = YahooClient::new().await?;
    let mut quotes = universe_quotes(&client, market, universe).await?;
    let scanned = quotes.len();
    // One quoteSummary request per stock: the largest when there are many.
    quotes.sort_by(|a, b| {
        b.market_cap
            .unwrap_or(0.0)
            .total_cmp(&a.market_cap.unwrap_or(0.0))
    });
    quotes.truncate(SURPRISE_MAX);
    let symbols: Vec<String> = quotes.iter().filter_map(|q| q.symbol.clone()).collect();
    let client_ref = &client;
    let histories: Vec<(String, Vec<crate::research::Surprise>)> =
        futures_util::stream::iter(symbols)
            .map(|sym| async move {
                let h = client_ref
                    .quote_summary(&sym, &["earningsHistory"])
                    .await
                    .map(|v| parse_reported_quarters(&v))
                    .unwrap_or_default();
                (sym, h)
            })
            .buffer_unordered(6)
            .collect()
            .await;
    let rows = surprise_rows(histories);

    print_header("Earnings Surprise — latest quarter");
    println!(
        "  {:<14} {:>12} {:>11} {:>11} {:>11}",
        "Symbol".bold(),
        "Quarter".bold(),
        "EPS actual".bold(),
        "EPS est.".bold(),
        "Surprise".bold()
    );
    println!("  {}", "─".repeat(63).dimmed());
    let (mut above, mut below, mut in_line) = (0, 0, 0);
    for r in &rows {
        let pct = r.pct();
        match pct {
            Some(p) if p.abs() < SURPRISE_IN_LINE_PCT => in_line += 1,
            Some(p) if p > 0.0 => above += 1,
            Some(_) => below += 1,
            None => {}
        }
        let (quarter, actual, est) = match &r.latest {
            Some(s) => (
                s.quarter.format("%b %Y").to_string(),
                format!("{:.2}", s.actual),
                if s.estimate.is_finite() {
                    format!("{:.2}", s.estimate)
                } else {
                    "—".into()
                },
            ),
            None => ("—".into(), "—".into(), "—".into()),
        };
        println!(
            "  {:<14} {:>12} {:>11} {:>11} {:>11}",
            r.symbol.cyan(),
            quarter,
            actual,
            est,
            pct_cell(pct, 1)
        );
    }

    print_section("Summary");
    println!(
        "  {} {} above the estimate, {} below, {} within {:.0}%",
        "→".cyan(),
        above,
        below,
        in_line,
        SURPRISE_IN_LINE_PCT
    );
    let missing = rows.iter().filter(|r| r.latest.is_none()).count();
    if missing > 0 {
        println!(
            "  {} {}",
            "·".dimmed(),
            format!(
                "{missing} without an estimate on Yahoo Finance (common for smaller Indian stocks)."
            )
            .dimmed()
        );
    }
    if scanned > SURPRISE_MAX {
        println!(
            "  {} {}",
            "·".dimmed(),
            format!(
                "The {SURPRISE_MAX} largest of {scanned} stocks by market cap; --universe narrows the scan."
            )
            .dimmed()
        );
    }
    println!(
        "  {} {}",
        "·".dimmed(),
        "Source: Yahoo Finance earningsHistory (reported EPS against the analyst consensus)."
            .dimmed()
    );
    println!();
    Ok(())
}

// ── Dividend Calendar ──

/// Daily closes of a chart, oldest first, gaps dropped.
fn chart_closes(chart: &crate::api::ChartData) -> Vec<f64> {
    chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default()
}

/// The 10-year government bond yield, percent: CCIL / RBI for India,
/// `^TNX` for the US, none elsewhere.
async fn ten_year_yield(client: &YahooClient, market: Market) -> Option<f64> {
    match market {
        Market::In => crate::gsec::yields(std::time::Duration::from_secs(8))
            .await
            .ok()
            .and_then(|set| crate::gsec::ten_year(&set.rows).and_then(|r| r.ytm)),
        Market::Us => client
            .get_quote(&["^TNX"])
            .await
            .ok()
            .and_then(|q| q.first().and_then(|q| q.regular_market_price)),
        _ => None,
    }
}

/// Save the session's breadth and valuation and rank them against the
/// sessions saved before (per market and scan scope). Returns the session
/// date and the comparison; no comparison with `cache.db` off, no date
/// when the quotes carry no trade time. A failed save is said on stderr.
fn record_reading(
    market: Market,
    universe: Option<market::Universe>,
    quotes: &[crate::api::Quote],
    breadth: &crate::conditions::Breadth,
    valuation: &crate::conditions::Valuation,
) -> (Option<String>, Option<crate::conditions::History>) {
    // The session the quotes are from: the latest trade time, as a date
    // on the market's own calendar (a weekend run records Friday).
    let as_of = quotes
        .iter()
        .filter_map(|q| q.regular_market_time)
        .max()
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| crate::clock::local(market, t).date_naive().to_string());
    // Breadth and valuation have no past to look up: save this session's
    // and rank it against the ones saved before.
    let history = as_of.as_ref().and_then(|date| {
        let scope = universe.map_or("default", |u| u.nse_index());
        let key = crate::value_name(market);
        let today = crate::conditions::Reading::of(date, breadth, valuation);
        let past: Vec<crate::conditions::Reading> =
            crate::db::with_db(|d| d.conditions_history(&key, scope))?
                .iter()
                .filter_map(|json| serde_json::from_str(json).ok())
                .collect();
        // The read above worked, so cache.db is on: a failed save is a
        // fault worth saying, or the record silently stops growing.
        let saved = serde_json::to_string(&today)
            .ok()
            .and_then(|json| crate::db::with_db(|d| d.put_conditions(&key, scope, date, &json)))
            .is_some();
        if !saved {
            eprintln!(
                "  \u{24d8} Could not save this session's breadth and valuation to cache.db; it will be missing from later comparisons."
            );
        }
        Some(crate::conditions::history(&past, &today))
    });
    (as_of, history)
}

/// Record today's `conditions` session without printing the report, for
/// the daily daemon: the same scan and the same record as a plain
/// `conditions` run. Returns the session date and how many earlier
/// sessions are on record; `None` when nothing could be saved.
pub(super) async fn record_conditions(
    client: &YahooClient,
    market: Market,
) -> Result<Option<(String, usize)>> {
    let bond_yield = ten_year_yield(client, market).await;
    let quotes = universe_quotes(client, market, None).await?;
    let breadth = crate::conditions::breadth(&quotes);
    let valuation = crate::conditions::valuation(&quotes, bond_yield);
    let (as_of, history) = record_reading(market, None, &quotes, &breadth, &valuation);
    Ok(as_of.zip(history).map(|(date, h)| (date, h.sessions)))
}

/// The `conditions` report of a market (and, for India, a `--universe`),
/// saving the session's breadth and valuation.
pub(crate) async fn conditions_report(
    client: &YahooClient,
    market: Market,
    universe: Option<market::Universe>,
) -> Result<crate::conditions::Report> {
    market.require_stocks("conditions")?;
    let (index_symbol, index_name, vol): (&str, &str, Option<(&str, &str)>) = match market {
        Market::In => ("^NSEI", "NIFTY 50", Some(("^INDIAVIX", "India VIX"))),
        Market::Us => ("^GSPC", "S&P 500", Some(("^VIX", "VIX"))),
        Market::Global(x) => {
            let (symbol, name) = x.info().benchmark();
            (symbol, name, None)
        }
        _ => bail!("`conditions` works on stock markets"),
    };
    let trend = client
        .get_chart(index_symbol, "10y", "1d")
        .await
        .ok()
        .and_then(|c| crate::conditions::trend(&chart_closes(&c)));
    let volatility = match vol {
        Some((symbol, name)) => client
            .get_chart(symbol, "5y", "1d")
            .await
            .ok()
            .and_then(|c| crate::conditions::volatility(name, &chart_closes(&c))),
        None => None,
    };
    let bond_yield = ten_year_yield(client, market).await;
    let quotes = universe_quotes(client, market, universe).await?;
    let breadth = crate::conditions::breadth(&quotes);
    let valuation = crate::conditions::valuation(&quotes, bond_yield);
    let (as_of, history) = record_reading(market, universe, &quotes, &breadth, &valuation);
    Ok(crate::conditions::Report {
        market: market.label().to_string(),
        index: index_name.to_string(),
        trend,
        volatility,
        breadth,
        valuation,
        scanned: quotes.len(),
        as_of,
        history,
    })
}

/// `conditions`: trend, volatility, breadth and valuation of a market.
/// Readings only: no call on whether to invest.
pub async fn cmd_conditions(
    market: Market,
    universe: Option<market::Universe>,
    json: bool,
) -> Result<()> {
    let client = YahooClient::new().await?;
    let report = conditions_report(&client, market, universe).await?;
    if json {
        return output::print_json(&report);
    }

    print_header(&format!("Market Conditions — {}", report.market));
    let pct = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{:.0}%", v));
    let signed = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{:+.1}%", v));
    let num = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{:.1}", v));

    print_section(&format!("Trend — {}", report.index));
    match &report.trend {
        Some(t) => {
            print_kv("Level", &format!("{:.2}", t.last));
            print_kv("Against 200-day average", &signed(t.vs_200_day_pct));
            print_kv(
                "  days with a lower distance",
                &match t.vs_200_day_percentile {
                    Some(p) => format!("{:.0}% of the last {:.0} years", p, t.history_years),
                    None => "n/a".to_string(),
                },
            );
            print_kv("Against 50-day average", &signed(t.vs_50_day_pct));
            print_kv(
                "Below 52-week high",
                &format!("{:.1}%", t.from_52_week_high_pct),
            );
            print_kv("Below highest close", &format!("{:.1}%", t.from_peak_pct));
            print_kv("One-year change", &signed(t.return_1y_pct));
        }
        None => println!("  {}", "No index history available.".dimmed()),
    }

    if let Some(v) = &report.volatility {
        print_section("Volatility");
        print_kv(&v.name, &format!("{:.2}", v.last));
        print_kv(
            "  days it was lower",
            &format!(
                "{:.0}% of the last {:.0} years",
                v.percentile, v.history_years
            ),
        );
    }

    let b = &report.breadth;
    print_section(&format!("Breadth — {} stocks", b.stocks));
    print_kv(
        "Above 200-day average",
        &format!(
            "{} of {} with one",
            pct(b.above_200_day_pct),
            b.with_200_day_average
        ),
    );
    print_kv("Above 50-day average", &pct(b.above_50_day_pct));
    print_kv("Within 5% of 52-week high", &pct(b.near_52_week_high_pct));
    print_kv("Within 5% of 52-week low", &pct(b.near_52_week_low_pct));
    print_kv("Up on the day", &pct(b.advancing_pct));

    let v = &report.valuation;
    print_section(&format!("Valuation — {} profitable stocks", v.stocks));
    print_kv("Aggregate P/E", &num(v.aggregate_pe));
    print_kv("Median P/E", &num(v.median_pe));
    print_kv(
        "Earnings yield",
        &v.earnings_yield_pct
            .map_or("n/a".to_string(), |e| format!("{:.2}%", e)),
    );
    print_kv(
        "10-year government yield",
        &v.bond_yield_pct
            .map_or("n/a".to_string(), |e| format!("{:.2}%", e)),
    );
    print_kv(
        "Earnings minus bond yield",
        &v.earnings_minus_bond_pct
            .map_or("n/a".to_string(), |e| format!("{:+.2} points", e)),
    );
    print_kv(
        "Median dividend yield",
        &v.median_dividend_yield_pct
            .map_or("n/a".to_string(), |e| format!("{:.2}%", e)),
    );

    if let Some(h) = &report.history {
        print_section("Breadth and valuation against saved sessions");
        match &h.since {
            None => println!(
                "  {}",
                "First session on record. `conditions` and the daily `daemon longterm` each save one; ranks appear after 20."
                    .dimmed()
            ),
            Some(since) => {
                print_kv("Sessions on record", &format!("{} since {}", h.sessions, since));
                // No figure today (no bond yield for this market, say) is
                // "n/a"; a figure without enough sessions behind it waits.
                let rank = |today: Option<f64>, p: Option<f64>| match (today, p) {
                    (None, _) => "n/a".to_string(),
                    (Some(_), Some(p)) => format!("lower on {:.0}% of them", p),
                    (Some(_), None) => format!(
                        "after {} sessions with this figure",
                        crate::conditions::MIN_HISTORY
                    ),
                };
                print_kv(
                    "Above 200-day average",
                    &rank(report.breadth.above_200_day_pct, h.above_200_day_percentile),
                );
                print_kv(
                    "Aggregate P/E",
                    &rank(report.valuation.aggregate_pe, h.aggregate_pe_percentile),
                );
                print_kv(
                    "Earnings minus bond yield",
                    &rank(
                        report.valuation.earnings_minus_bond_pct,
                        h.earnings_minus_bond_percentile,
                    ),
                );
                if let Some(prev) = &h.previous {
                    let change = |now: Option<f64>, then: Option<f64>, unit: &str| {
                        match now.zip(then) {
                            Some((n, t)) => format!("{:+.1}{}", n - t, unit),
                            None => "n/a".to_string(),
                        }
                    };
                    print_kv(
                        &format!("Change since {}", prev.date),
                        &format!(
                            "above 200-day {}, aggregate P/E {}",
                            change(report.breadth.above_200_day_pct, prev.above_200_day_pct, " points"),
                            change(report.valuation.aggregate_pe, prev.aggregate_pe, "")
                        ),
                    );
                }
            }
        }
    }

    print_section("In words");
    for line in crate::conditions::describe(&report) {
        println!("  {}", line);
    }
    println!(
        "\n  {}",
        "These are readings of the data, not a forecast and not a signal to invest or to wait."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

/// Rows a heatmap shows from each end (biggest gains, biggest falls) when
/// the scan is larger than both together.
const HEATMAP_EDGE: usize = 30;

/// A quote's day change when it is a real number.
fn finite_change(q: &crate::api::Quote) -> Option<f64> {
    q.regular_market_change_percent.filter(|v| v.is_finite())
}

/// Sort by day change, largest first, with quotes that have no change (or
/// a NaN one) last. Never panics: `total_cmp` orders every float.
pub(crate) fn sort_by_change(quotes: &mut [crate::api::Quote]) {
    quotes.sort_by(|a, b| match (finite_change(a), finite_change(b)) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

/// One heatmap row: the symbol without its exchange suffix, a bar as long
/// as the move and the change; "—" and no bar when there is no change.
pub(crate) fn heatmap_line(symbol: &str, pct: Option<f64>) -> String {
    let short = symbol.split('.').next().unwrap_or(symbol);
    match pct.filter(|v| v.is_finite()) {
        Some(pct) => {
            let blocks = "█".repeat((pct.abs() * 3.0).clamp(1.0, 15.0) as usize);
            let text = format!("{:+.2}%", pct);
            if pct >= 0.0 {
                format!("  {:<12} {} {}", short, blocks.green(), text.green())
            } else {
                format!("  {:<12} {} {}", short, blocks.red(), text.red())
            }
        }
        None => format!("  {:<12} {}", short, "—".dimmed()),
    }
}

/// The heatmap's lines: both ends of a long scan with the count left out in
/// between, then the quotes without a change.
pub(crate) fn heatmap_lines(quotes: &mut [crate::api::Quote]) -> Vec<String> {
    sort_by_change(quotes);
    let ranked = quotes.iter().filter(|q| finite_change(q).is_some()).count();
    let mut lines = Vec::new();
    for (i, q) in quotes[..ranked].iter().enumerate() {
        if ranked > 2 * HEATMAP_EDGE {
            if i == HEATMAP_EDGE {
                lines.push(format!(
                    "  {}",
                    format!("... {} more in between ...", ranked - 2 * HEATMAP_EDGE).dimmed()
                ));
            }
            if (HEATMAP_EDGE..ranked - HEATMAP_EDGE).contains(&i) {
                continue;
            }
        }
        lines.push(heatmap_line(
            q.symbol.as_deref().unwrap_or("???"),
            finite_change(q),
        ));
    }
    let missing = &quotes[ranked..];
    for q in missing.iter().take(HEATMAP_EDGE) {
        lines.push(heatmap_line(q.symbol.as_deref().unwrap_or("???"), None));
    }
    if missing.len() > HEATMAP_EDGE {
        lines.push(format!(
            "  {}",
            format!(
                "... {} more without a change ...",
                missing.len() - HEATMAP_EDGE
            )
            .dimmed()
        ));
    }
    lines
}

pub async fn cmd_heatmap(
    market: Market,
    universe: Option<market::Universe>,
    cap: Option<market::CapFilter>,
) -> Result<()> {
    check_universe(market, universe)?;
    let client = YahooClient::new().await?;
    let scan = scan_symbols(&client, market, universe).await?;
    let mut quotes = scan_quotes(&client, market, &scan, cap, true).await?;
    let scope = cap_scope(&scan.scope, cap);

    print_header(&format!("{} Market Heatmap", scope));
    println!();
    for line in heatmap_lines(&mut quotes) {
        println!("{line}");
    }
    println!();
    Ok(())
}

// ── Patterns (candlestick pattern detection) ──

pub async fn cmd_sectorcmp(sector1: &str, sector2: &str, market: Market) -> Result<()> {
    market.require_sectors("sectorcmp")?;
    let (bench, bench_name) = sector_benchmark(market)
        .with_context(|| format!("sectorcmp has no benchmark for {}", market.label()))?;
    let sectors = market.sectors();
    let find = |name: &str| -> Option<(&'static str, &'static str)> {
        let lower = name.to_lowercase();
        sectors
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .or_else(|| {
                sectors
                    .iter()
                    .find(|(_, n)| n.to_lowercase().contains(&lower))
            })
            .copied()
    };
    let s1 = find(sector1).context(format!("Sector '{}' not found", sector1))?;
    let s2 = find(sector2).context(format!("Sector '{}' not found", sector2))?;
    let client = YahooClient::new().await?;
    let pair = [s1, s2];
    let quotes = client.get_quote(&[s1.0, s2.0]).await?;
    // Each sector's own quote, matched by symbol.
    let matched = pair_by_symbol(&pair, &quotes);
    if matched.iter().all(|(_, _, q)| q.is_none()) {
        bail!("Could not fetch either sector");
    }
    let chart_syms = vec![s1.0.to_string(), s2.0.to_string(), bench.to_string()];
    let histories = yearly_closes(&client, chart_syms).await;
    let bench_closes = histories
        .iter()
        .find(|(s, _)| s == bench)
        .map(|(_, c)| c.clone())
        .unwrap_or_default();
    let readings = sector_readings(&pair, &quotes, &histories, &bench_closes);
    let (a, b) = (&readings[0], &readings[1]);
    let (qa, qb) = (matched[0].2, matched[1].2);

    print_header(&format!("{} vs {}", a.name, b.name));
    println!(
        "  {:<20} {:>14} {:>14}",
        "Metric".bold(),
        a.name.bold().cyan(),
        b.name.bold().cyan()
    );
    println!("  {}", "─".repeat(50).dimmed());

    let money = |q: Option<&crate::api::Quote>, f: fn(&crate::api::Quote) -> Option<f64>| {
        q.and_then(|q| f(q).map(|v| format_price(v, q.currency.as_deref())))
            .unwrap_or_else(|| "—".to_string())
    };
    let mut rows: Vec<(String, String, String)> = vec![
        (
            "Price".into(),
            money(qa, |q| q.regular_market_price),
            money(qb, |q| q.regular_market_price),
        ),
        ("Day change".into(), pct_cell(a.day, 2), pct_cell(b.day, 2)),
    ];
    for (i, m) in RS_MONTHS.iter().enumerate() {
        rows.push((
            format!("Return {m}M"),
            pct_cell(a.returns[i], 1),
            pct_cell(b.returns[i], 1),
        ));
    }
    for (i, m) in RS_MONTHS.iter().enumerate() {
        rows.push((
            format!("RS {m}M vs {bench_name}"),
            pct_cell(a.rs[i], 1),
            pct_cell(b.rs[i], 1),
        ));
    }
    let pe = |q: Option<&crate::api::Quote>| {
        q.and_then(|q| q.trailing_pe)
            .map_or("—".into(), |v| format!("{:.1}", v))
    };
    rows.push(("P/E".into(), pe(qa), pe(qb)));
    rows.push((
        "50-Day MA".into(),
        money(qa, |q| q.fifty_day_average),
        money(qb, |q| q.fifty_day_average),
    ));
    rows.push((
        "52W High".into(),
        money(qa, |q| q.fifty_two_week_high),
        money(qb, |q| q.fifty_two_week_high),
    ));
    rows.push((
        "52W Low".into(),
        money(qa, |q| q.fifty_two_week_low),
        money(qb, |q| q.fifty_two_week_low),
    ));

    for (label, x, y) in &rows {
        println!("  {:<20} {:>14} {:>14}", label.dimmed(), x, y);
    }

    // What the numbers say, without a call to act.
    match (a.returns[1], b.returns[1]) {
        (Some(x), Some(y)) => {
            let (lead, lag) = if x >= y { (a, b) } else { (b, a) };
            println!(
                "\n  {} Over 3 months {} returned {:+.1}% and {} {:+.1}%.",
                "→".cyan(),
                lead.name.bold(),
                x.max(y),
                lag.name.bold(),
                x.min(y)
            );
        }
        _ => println!(
            "\n  {} {}",
            "→".cyan(),
            "Not enough history to compare the 3-month returns.".dimmed()
        ),
    }
    println!();
    Ok(())
}

// ── Options Overview (Put/Call data from quote) ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rss_google_news_source_url_shows_publisher_only() {
        let xml = r#"<rss><channel><item><title>Share Market Today: Nifty 50, Bank Nifty - Liquide Blog</title>
<link>https://x</link><pubDate>Mon, 02 Nov 2026 11:32:00 GMT</pubDate>
<source url="https://blog.liquide.life">Liquide Blog</source></item></channel></rss>"#;
        let items = parse_rss(xml, "Google News");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].publisher, "Liquide Blog");
        assert_eq!(items[0].title, "Share Market Today: Nifty 50, Bank Nifty");
    }

    #[test]
    fn parse_rss_source_without_attributes_or_tag() {
        let xml = r#"<rss><channel><item><title>L&amp;T wins mega order - Economic Times</title>
<source>Economic Times</source></item>
<item><title>Plain headline</title></item></channel></rss>"#;
        let items = parse_rss(xml, "Yahoo RSS");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].publisher, "Economic Times");
        assert_eq!(items[0].title, "L&T wins mega order");
        assert_eq!(items[1].publisher, "Yahoo RSS");
        assert_eq!(items[1].title, "Plain headline");
    }

    #[test]
    fn universes_are_for_the_indian_market_only() {
        assert!(check_universe(Market::In, Some(market::Universe::Nifty50)).is_ok());
        assert!(check_universe(Market::Us, None).is_ok());
        let err = check_universe(Market::Us, Some(market::Universe::BankNifty))
            .unwrap_err()
            .to_string();
        assert!(err.contains("-m in"), "{err}");
    }

    fn q(sym: &str, price: f64, pct: Option<f64>) -> crate::api::Quote {
        crate::api::Quote {
            symbol: Some(sym.into()),
            regular_market_price: Some(price),
            regular_market_change_percent: pct,
            ..Default::default()
        }
    }

    fn day(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Daily closes from `start` rising by `step` a day for `n` days.
    fn ramp(start: &str, first: f64, step: f64, n: i64) -> Vec<(chrono::NaiveDate, f64)> {
        (0..n)
            .map(|i| {
                (
                    day(start) + chrono::Duration::days(i),
                    first + step * i as f64,
                )
            })
            .collect()
    }

    #[test]
    fn rotation_pairs_quotes_by_symbol_when_one_is_dropped() {
        let names = [
            ("^CNXIT", "IT"),
            ("^NSEBANK", "Bank"),
            ("^CNXPHARMA", "Pharma"),
        ];
        // Yahoo dropped Bank and returned the rest out of order.
        let quotes = vec![
            q("^CNXPHARMA", 300.0, Some(-1.0)),
            q("^cnxit", 100.0, Some(2.0)),
        ];
        let paired = pair_by_symbol(&names, &quotes);
        assert_eq!(paired.len(), 3);
        assert_eq!(paired[0].1, "IT");
        assert_eq!(
            paired[0].2.and_then(|q| q.regular_market_price),
            Some(100.0)
        );
        assert_eq!(paired[1].1, "Bank");
        assert!(paired[1].2.is_none());
        assert_eq!(paired[2].1, "Pharma");
        assert_eq!(
            paired[2].2.and_then(|q| q.regular_market_price),
            Some(300.0)
        );

        // Readings keep each sector's own quote and history, and leave the
        // dropped one empty ("—" when printed).
        let histories = vec![
            (
                "^CNXPHARMA".to_string(),
                ramp("2026-01-01", 100.0, 1.0, 200),
            ),
            ("^CNXIT".to_string(), ramp("2026-01-01", 100.0, 0.5, 200)),
        ];
        let bench = ramp("2026-01-01", 100.0, 0.0, 200);
        let r = sector_readings(&names, &quotes, &histories, &bench);
        assert_eq!(r[0].name, "IT");
        assert_eq!(r[0].day, Some(2.0));
        assert_eq!(r[1].name, "Bank");
        assert_eq!((r[1].price, r[1].day, r[1].rs), (None, None, [None; 3]));
        assert_eq!(r[2].name, "Pharma");
        assert_eq!(r[2].price, Some(300.0));
        // Pharma rose faster than IT against a flat benchmark.
        assert!(r[2].rs[0].unwrap() > r[0].rs[0].unwrap());
        assert!(r[0].rs[2].unwrap() > 0.0);
        assert_eq!(r[1].trend(), None);
        assert_eq!(r[2].trend(), Some("Leading"));
        assert_eq!(pct_cell(r[1].rs[0], 1), "—".dimmed().to_string());
    }

    #[test]
    fn period_return_and_relative_strength() {
        let closes = vec![
            (day("2026-01-30"), 100.0),
            (day("2026-02-27"), 110.0),
            (day("2026-03-31"), 121.0),
        ];
        // One month before 31 Mar is 28 Feb: the close on or before it.
        assert!((period_return(&closes, 1).unwrap() - 10.0).abs() < 1e-9);
        assert!((period_return(&closes, 2).unwrap() - 21.0).abs() < 1e-9);
        // Not enough history.
        assert_eq!(period_return(&closes, 6), None);
        assert_eq!(period_return(&[], 1), None);
        // +10% against +5%: 4.76% better.
        let rs = relative_strength(Some(10.0), Some(5.0)).unwrap();
        assert!((rs - (1.10 / 1.05 - 1.0) * 100.0).abs() < 1e-9);
        assert_eq!(relative_strength(None, Some(5.0)), None);
        assert_eq!(relative_strength(Some(1.0), None), None);
    }

    #[test]
    fn heatmap_with_nan_does_not_panic_and_sorts_it_last() {
        let mut quotes = vec![
            q("A.NS", 1.0, Some(1.0)),
            q("NAN.NS", 1.0, Some(f64::NAN)),
            q("B.NS", 1.0, Some(-2.0)),
            q("NONE.NS", 1.0, None),
            q("C.NS", 1.0, Some(3.0)),
        ];
        let lines = heatmap_lines(&mut quotes);
        let order: Vec<&str> = quotes.iter().filter_map(|q| q.symbol.as_deref()).collect();
        assert_eq!(&order[..3], ["C.NS", "A.NS", "B.NS"]);
        assert!(order[3..].contains(&"NAN.NS") && order[3..].contains(&"NONE.NS"));
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("+3.00%"));
        assert!(lines[2].contains("-2.00%"));
        for line in &lines[3..] {
            assert!(line.contains('—'), "{line}");
            assert!(!line.contains("NaN") && !line.contains('█'), "{line}");
        }
        assert!(heatmap_line("X.NS", Some(f64::INFINITY)).contains('—'));
    }

    #[test]
    fn heatmap_keeps_both_ends_of_a_long_scan() {
        let mut quotes: Vec<crate::api::Quote> = (0..100)
            .map(|i| q(&format!("S{i}"), 1.0, Some(i as f64 - 50.0)))
            .collect();
        quotes.push(q("NAN", 1.0, Some(f64::NAN)));
        let lines = heatmap_lines(&mut quotes);
        // 30 + gap line + 30 + the one without a change
        assert_eq!(lines.len(), 2 * HEATMAP_EDGE + 2);
        assert!(lines[HEATMAP_EDGE].contains("40 more in between"));
        assert!(lines.last().unwrap().contains('—'));
    }

    #[test]
    fn earnings_rows_sort_by_next_date() {
        use crate::calendar::{Event, EventKind, Source};
        let ev = |sym: &str, date: &str, source, estimate| Event {
            symbol: sym.into(),
            date: day(date),
            kind: EventKind::Results,
            detail: String::new(),
            source,
            estimate,
        };
        let events = vec![
            (
                "TCS.NS".to_string(),
                vec![
                    ev("TCS.NS", "2026-10-09", Source::Yahoo, false), // past
                    ev("TCS.NS", "2027-01-12", Source::Yahoo, true),
                    ev("TCS.NS", "2027-01-10", Source::Nse, false),
                ],
            ),
            (
                "AAPL".to_string(),
                vec![ev("AAPL", "2026-12-01", Source::Yahoo, true)],
            ),
            ("ZZZ".to_string(), vec![]),
        ];
        let rows = earnings_rows(&events, |_| day("2026-11-21"));
        let syms: Vec<&str> = rows.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(syms, ["AAPL", "TCS.NS", "ZZZ"]);
        assert_eq!(rows[0].days_until, Some(10));
        assert!(rows[0].estimate);
        // NSE's filed date wins over Yahoo's later estimate.
        assert_eq!(rows[1].date, Some(day("2027-01-10")));
        assert_eq!(rows[1].source, Some(Source::Nse));
        assert!(!rows[1].estimate);
        assert_eq!((rows[2].date, rows[2].days_until), (None, None));
    }

    #[test]
    fn surprise_rows_use_the_latest_quarter() {
        use crate::research::Surprise;
        let s = |q: &str, actual, estimate, pct| Surprise {
            quarter: day(q),
            actual,
            estimate,
            surprise_pct: pct,
        };
        let rows = surprise_rows(vec![
            (
                "AAA".into(),
                vec![
                    s("2026-09-30", 1.1, 1.0, 0.10),
                    s("2026-06-30", 0.5, 1.0, -0.5),
                ],
            ),
            ("BBB".into(), vec![]),
            ("CCC".into(), vec![s("2026-09-30", 2.0, 1.6, 0.25)]),
            ("DDD".into(), vec![s("2026-09-30", 0.9, 1.0, f64::NAN)]),
        ]);
        let syms: Vec<&str> = rows.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(syms, ["CCC", "AAA", "BBB", "DDD"]);
        let aaa = &rows[1];
        assert_eq!(aaa.latest.as_ref().unwrap().quarter, day("2026-09-30"));
        assert!((aaa.pct().unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(rows[3].pct(), None);
    }

    #[test]
    fn movers_label_says_when_and_whether_open() {
        // Saturday 21 Nov 2026, 10:00 IST
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-21T04:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        // Last trade Friday 20 Nov 15:29 IST
        let last = chrono::DateTime::parse_from_rfc3339("2026-11-20T09:59:00Z")
            .unwrap()
            .timestamp();
        let label = movers_as_of(Market::In, now, Some(last));
        assert!(label.starts_with("As of Sat 21 Nov 10:00 IST"), "{label}");
        assert!(
            label.contains("session Fri 20 Nov 2026 (last trade 15:29 IST)"),
            "{label}"
        );
        assert!(label.contains("market closed (weekend)"), "{label}");
        let none = movers_as_of(Market::In, now, None);
        assert!(none.contains("session Sat 21 Nov 2026"), "{none}");
    }

    #[test]
    fn vix_bands_per_market() {
        let india = sentiment_spec(Market::In).unwrap();
        let us = sentiment_spec(Market::Us).unwrap();
        assert_eq!(india.vix.0, "^INDIAVIX");
        assert_eq!(india.index.0, "^NSEI");
        assert_eq!(us.vix.0, "^VIX");
        // India VIX's bands sit higher: 33 is fear on the VIX but extreme
        // fear only on India VIX past 32.
        assert_eq!(vix_score(13.0, india.vix_bands), 90.0);
        assert_eq!(vix_score(16.0, india.vix_bands), 70.0);
        assert_eq!(vix_score(22.0, india.vix_bands), 50.0);
        assert_eq!(vix_score(26.0, us.vix_bands), 30.0);
        assert_eq!(vix_score(26.0, india.vix_bands), 30.0);
        assert_eq!(vix_score(33.0, india.vix_bands), 10.0);
        assert_eq!(vix_score(33.0, us.vix_bands), 30.0);
        assert!(sentiment_spec(Market::Crypto).is_err());
    }

    #[test]
    fn headline_tally_uses_the_shared_lexicon() {
        let (p, n) = headline_tally([
            "Plant commissioned ahead of schedule",
            "Windfall tax announced",
            "Shares plunge after downgrade",
            "No growth in exports",
        ]);
        assert_eq!((p, n), (1, 2));
    }

    #[test]
    fn session_date_does_not_trail_east_asia_or_india_from_west() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-21T04:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            crate::clock::session_date(Market::In, at),
            chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap()
        );
    }
}
