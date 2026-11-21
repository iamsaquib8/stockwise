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
async fn scan_quotes(
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

pub async fn cmd_news(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    // Fetch from 3 sources concurrently
    let company_query = resolved.split('.').next().unwrap_or(&resolved);
    let google_query = format!("{} stock", company_query);

    let (yahoo_json, yahoo_rss, google) = tokio::join!(
        fetch_yahoo_json_news(&client, &resolved, 10),
        fetch_yahoo_rss(&client, &resolved),
        fetch_google_news_rss(&client, &google_query),
    );

    let all: Vec<NewsItem> = [yahoo_json, yahoo_rss, google].concat();
    let collated = collate_news(all);

    print_header(&format!("News: {} — {} sources", resolved, 3));
    print_news_items(&collated, 15);
    println!();
    Ok(())
}

// ── Risk Analysis ──

pub async fn cmd_sentiment() -> Result<()> {
    let client = YahooClient::new().await?;
    // Fetch VIX, S&P500, and some breadth proxies
    let quotes = client
        .get_quote(&["^VIX", "^GSPC", "^IXIC", "^DJI", "SPY", "QQQ"])
        .await?;

    let vix = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some("^VIX"))
        .and_then(|q| q.regular_market_price);
    let sp_chg = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some("^GSPC"))
        .and_then(|q| q.regular_market_change_percent);

    let mut score = 50.0_f64;
    let mut factors = Vec::new();

    // VIX: <15 extreme greed, 15-20 greed, 20-25 neutral, 25-35 fear, >35 extreme fear
    if let Some(v) = vix {
        let vix_score = match v {
            x if x < 15.0 => 90.0,
            x if x < 20.0 => 70.0,
            x if x < 25.0 => 50.0,
            x if x < 35.0 => 30.0,
            _ => 10.0,
        };
        score = (score + vix_score) / 2.0;
        factors.push(format!(
            "VIX: {:.1} → {}",
            v,
            if vix_score > 60.0 {
                "Greed"
            } else if vix_score < 40.0 {
                "Fear"
            } else {
                "Neutral"
            }
        ));
    }

    // Market momentum
    if let Some(chg) = sp_chg {
        let mom_score = (50.0 + chg * 10.0).clamp(0.0, 100.0);
        score = (score + mom_score) / 2.0;
        factors.push(format!("S&P 500: {:+.2}% today", chg));
    }

    // Get broader market momentum (20-day)
    let chart = client.get_chart("^GSPC", "1mo", "1d").await;
    if let Ok(c) = chart {
        let closes: Vec<f64> = c
            .indicators
            .quote
            .first()
            .and_then(|q| q.close.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();
        if closes.len() > 5 {
            let monthly_ret = technical::percent_change(closes[0], *closes.last().unwrap());
            let trend_score = (50.0 + monthly_ret * 5.0).clamp(0.0, 100.0);
            score = (score + trend_score) / 2.0;
            factors.push(format!("1M trend: {:+.2}%", monthly_ret));
        }
    }

    // News sentiment from multiple sources
    let (yahoo_news, google_news) = tokio::join!(
        fetch_yahoo_json_news(&client, "stock market today", 10),
        fetch_google_news_rss(&client, "US India stock market today"),
    );
    let all_news: Vec<NewsItem> = [yahoo_news, google_news].concat();
    let collated_news = collate_news(all_news);

    // Simple keyword-based sentiment scoring
    let bullish_words = [
        "rally", "surge", "gain", "rise", "high", "bull", "growth", "record", "up", "buy",
        "strong", "positive",
    ];
    let bearish_words = [
        "fall",
        "drop",
        "crash",
        "decline",
        "fear",
        "bear",
        "loss",
        "low",
        "sell",
        "weak",
        "negative",
        "recession",
    ];
    let mut news_bull = 0i32;
    let mut news_bear = 0i32;
    for item in &collated_news {
        let t = item.title.to_lowercase();
        for w in &bullish_words {
            if t.contains(w) {
                news_bull += 1;
            }
        }
        for w in &bearish_words {
            if t.contains(w) {
                news_bear += 1;
            }
        }
    }
    if news_bull + news_bear > 0 {
        let news_score =
            ((news_bull as f64 / (news_bull + news_bear) as f64) * 100.0).clamp(0.0, 100.0);
        score = (score * 0.7 + news_score * 0.3).clamp(0.0, 100.0);
        let label = if news_score > 55.0 {
            "bullish"
        } else if news_score < 45.0 {
            "bearish"
        } else {
            "neutral"
        };
        factors.push(format!(
            "News sentiment: {} ({} positive / {} negative headlines)",
            label, news_bull, news_bear
        ));
    }

    let (label, color) = match score {
        s if s >= 80.0 => ("EXTREME GREED", "green"),
        s if s >= 60.0 => ("GREED", "green"),
        s if s >= 40.0 => ("NEUTRAL", "yellow"),
        s if s >= 20.0 => ("FEAR", "red"),
        _ => ("EXTREME FEAR", "red"),
    };

    print_header("Market Sentiment — Fear & Greed");

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

    print_section("Contributing Factors");
    for f in &factors {
        println!("  {} {}", "→".cyan(), f);
    }

    if !collated_news.is_empty() {
        print_section("Top Market Headlines");
        print_news_items(&collated_news, 5);
    }
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
        "  {:<14} {:>10} {:>8} {:>6} {:>6} {:>8} {:>7} {:>6} {:>5} {:>5}  {}",
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
        "Top fifth on".bold()
    );
    println!("  {}", "─".repeat(102).dimmed());

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
            "  {:<14} {:>10} {} {} {:>6} {:>8} {:>7} {:>6} {:>5} {:>4.0}%  {}",
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
            p.strongest().join(", ").dimmed()
        );
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
/// issues after that date; `None` when Yahoo's actions could not be read.
async fn split_factor(client: &YahooClient, symbol: &str, since: chrono::NaiveDate) -> Option<f64> {
    let day = since.format("%Y-%m-%d").to_string();
    let (splits, _, _) = crate::commands::portfolio::fetch_actions(client, symbol, &day)
        .await
        .ok()?;
    Some(
        splits
            .iter()
            .filter(|s| s.date.as_str() > day.as_str() && s.denominator > 0.0)
            .map(|s| s.ratio())
            .product(),
    )
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
    let today = chrono::Local::now().date_naive();
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
    let prices: std::collections::HashMap<String, f64> = client
        .get_quote(&refs)
        .await
        .context("Could not fetch current prices for the logged picks")?
        .into_iter()
        .filter_map(|q| Some((q.symbol.clone()?, q.regular_market_price?)))
        .collect();
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
    let (bench, bench_now) = match crate::picks::benchmark(market) {
        Some((sym, name)) => {
            let range = if since_days > 330 { "5y" } else { "2y" };
            let closes = client
                .get_chart(sym, range, "1d")
                .await
                .ok()
                .map(|c| (name, c.dated_closes()));
            (closes, benchmark_level(client, market).await)
        }
        None => (None, None),
    };
    Ok(crate::picks_log::review(
        &log,
        market.code(),
        horizon.as_str(),
        since,
        today,
        &prices,
        &splits,
        bench.as_ref().map(|(n, s)| crate::picks_log::Benchmark {
            name: n,
            closes: s.as_slice(),
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

pub async fn cmd_earnings(_market: Market) -> Result<()> {
    let client = YahooClient::new().await?;

    // Check portfolio + watchlist stocks
    let portfolio = Portfolio::load()?;
    let wl = Watchlist::load()?;
    let mut all_symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    for s in &wl.symbols {
        if !all_symbols.contains(s) {
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

    let sym_refs: Vec<&str> = all_symbols.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header("Earnings Overview");
    println!(
        "  {:<14} {:>10} {:>10} {:>10} {:>10}",
        "Symbol".bold(),
        "Price".bold(),
        "EPS TTM".bold(),
        "EPS Fwd".bold(),
        "P/E Fwd".bold()
    );
    println!("  {}", "─".repeat(58).dimmed());

    let mut growing = 0;
    let mut declining = 0;
    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let eps_ttm = q.eps_trailing_twelve_months;
        let eps_fwd = q.eps_forward;
        let fwd_pe = q.forward_pe;

        let growth_indicator = match (eps_ttm, eps_fwd) {
            (Some(t), Some(f)) if f > t && t > 0.0 => {
                growing += 1;
                "▲".green().to_string()
            }
            (Some(t), Some(f)) if f < t && t > 0.0 => {
                declining += 1;
                "▼".red().to_string()
            }
            _ => "—".dimmed().to_string(),
        };

        println!(
            "  {:<14} {:>10} {:>10} {:>10} {:>10} {}",
            sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            eps_ttm.map_or("—".to_string(), |v| format!("{:.2}", v)),
            eps_fwd.map_or("—".to_string(), |v| format!("{:.2}", v)),
            fwd_pe.map_or("—".to_string(), |v| format!("{:.1}", v)),
            growth_indicator,
        );
    }

    print_section("Summary");
    println!("  {} {} stocks with growing earnings", "▲".green(), growing);
    println!(
        "  {} {} stocks with declining earnings",
        "▼".red(),
        declining
    );
    println!();
    Ok(())
}

// ── Long-Term Wealth Bot ──

pub async fn cmd_rotation(market: Market) -> Result<()> {
    market.require_sectors("rotation")?;
    let client = YahooClient::new().await?;
    let sectors = market.sectors();
    let title = format!("{} Sector Rotation", market.label());
    let syms: Vec<&str> = sectors.iter().map(|(s, _)| *s).collect();
    let quotes = client.get_quote(&syms).await?;

    print_header(&title);
    println!(
        "  {:<16} {:>10} {:>10} {:>10} {:>10}",
        "Sector".bold(),
        "Price".bold(),
        "Change%".bold(),
        "vs 50MA".bold(),
        "Signal".bold()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for (i, q) in quotes.iter().enumerate() {
        let name = sectors.get(i).map(|(_, n)| *n).unwrap_or("???");
        let cur = q.currency.as_deref();
        let price = q.regular_market_price.unwrap_or(0.0);
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let ma50 = q.fifty_day_average.unwrap_or(price);
        let vs_ma = if ma50.abs() > f64::EPSILON {
            ((price - ma50) / ma50) * 100.0
        } else {
            0.0
        };
        let signal = if vs_ma > 3.0 && pct > 0.0 {
            "OVERWEIGHT".green().bold().to_string()
        } else if vs_ma < -3.0 {
            "UNDERWEIGHT".red().to_string()
        } else {
            "NEUTRAL".yellow().to_string()
        };
        let pct_str = if pct >= 0.0 {
            format!("{:+.2}%", pct).green().to_string()
        } else {
            format!("{:+.2}%", pct).red().to_string()
        };
        let ma_str = if vs_ma >= 0.0 {
            format!("{:+.1}%", vs_ma).green().to_string()
        } else {
            format!("{:+.1}%", vs_ma).red().to_string()
        };
        println!(
            "  {:<16} {:>10} {:>10} {:>10} {:>10}",
            name.bold(),
            format_price(price, cur),
            pct_str,
            ma_str,
            signal
        );
    }
    println!(
        "\n  {} Overweight sectors above 50-day MA with positive momentum.",
        "→".cyan()
    );
    println!(
        "  {} Underweight sectors below 50-day MA trending down.",
        "→".cyan()
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

/// Rows `surprise` shows from each end (fastest growth, steepest fall)
/// when more stocks than both together have the figures.
const SURPRISE_EDGE: usize = 25;

pub async fn cmd_surprise(market: Market, universe: Option<market::Universe>) -> Result<()> {
    market.require_stocks("surprise")?;
    let client = YahooClient::new().await?;
    let quotes = universe_quotes(&client, market, universe).await?;

    print_header("Earnings Momentum");
    println!(
        "  {:<14} {:>10} {:>10} {:>10} {:>8} {:>10}",
        "Symbol".bold(),
        "EPS TTM".bold(),
        "EPS Fwd".bold(),
        "Growth%".bold(),
        "P/E".bold(),
        "Signal".bold()
    );
    println!("  {}", "─".repeat(66).dimmed());

    let mut accelerating = Vec::new();
    let mut decelerating = Vec::new();

    // Stocks with both figures (a missing forward EPS is not a fall to
    // zero), by expected growth. A whole-market scan shows both ends.
    let mut rows: Vec<(&crate::api::Quote, f64, f64, f64)> = quotes
        .iter()
        .filter_map(|q| {
            let ttm = q.eps_trailing_twelve_months.filter(|e| *e > 0.0)?;
            let fwd = q.eps_forward?;
            Some((q, ttm, fwd, (fwd / ttm - 1.0) * 100.0))
        })
        .collect();
    rows.sort_by(|a, b| b.3.total_cmp(&a.3));
    let total = rows.len();
    let cut = total > 2 * SURPRISE_EDGE;
    for (i, (q, eps_ttm, eps_fwd, growth)) in rows.into_iter().enumerate() {
        let (eps_ttm, eps_fwd, growth) = (eps_ttm, eps_fwd, growth);
        let sym = q.symbol.as_deref().unwrap_or("???");
        let pe = q.trailing_pe.unwrap_or(0.0);
        let shown = !cut || i < SURPRISE_EDGE || i >= total - SURPRISE_EDGE;
        if cut && i == SURPRISE_EDGE {
            println!(
                "  {}",
                format!("... {} more in between ...", total - 2 * SURPRISE_EDGE).dimmed()
            );
        }
        let signal = if growth > 15.0 {
            accelerating.push(sym.to_string());
            "ACCELERATING".green().bold().to_string()
        } else if growth > 0.0 {
            "Growing".green().to_string()
        } else if growth > -10.0 {
            "Slowing".yellow().to_string()
        } else {
            decelerating.push(sym.to_string());
            "DECELERATING".red().bold().to_string()
        };
        let g_str = if growth >= 0.0 {
            format!("+{:.1}%", growth).green().to_string()
        } else {
            format!("{:.1}%", growth).red().to_string()
        };
        if !shown {
            continue;
        }
        println!(
            "  {:<14} {:>10.2} {:>10.2} {:>10} {:>8.1} {:>10}",
            sym.cyan(),
            eps_ttm,
            eps_fwd,
            g_str,
            pe,
            signal
        );
    }

    print_section("Summary");
    println!(
        "  {} {} stocks with accelerating earnings",
        "▲".green(),
        accelerating.len()
    );
    println!(
        "  {} {} stocks with decelerating earnings",
        "▼".red(),
        decelerating.len()
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
    quotes.sort_by(|a, b| {
        b.regular_market_change_percent
            .unwrap_or(0.0)
            .partial_cmp(&a.regular_market_change_percent.unwrap_or(0.0))
            .unwrap()
    });

    print_header(&format!("{} Market Heatmap", scope));
    println!();

    // A whole-market scan is thousands of rows: show both ends.
    let total = quotes.len();
    if total > 2 * HEATMAP_EDGE {
        quotes.drain(HEATMAP_EDGE..total - HEATMAP_EDGE);
    }
    for (i, q) in quotes.iter().enumerate() {
        if total > 2 * HEATMAP_EDGE && i == HEATMAP_EDGE {
            println!(
                "  {}",
                format!("... {} more in between ...", total - 2 * HEATMAP_EDGE).dimmed()
            );
        }
        let sym = q.symbol.as_deref().unwrap_or("???");
        let short = sym.split('.').next().unwrap_or(sym);
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let block_count = (pct.abs() * 3.0).clamp(1.0, 15.0) as usize;
        let blocks = "█".repeat(block_count);
        let (colored_blocks, pct_str) = if pct >= 0.0 {
            (
                blocks.green().to_string(),
                format!("{:+.2}%", pct).green().to_string(),
            )
        } else {
            (
                blocks.red().to_string(),
                format!("{:+.2}%", pct).red().to_string(),
            )
        };
        println!("  {:<12} {} {}", short, colored_blocks, pct_str);
    }
    println!();
    Ok(())
}

// ── Patterns (candlestick pattern detection) ──

pub async fn cmd_sectorcmp(sector1: &str, sector2: &str, market: Market) -> Result<()> {
    market.require_sectors("sectorcmp")?;
    let client = YahooClient::new().await?;
    let sectors = market.sectors();
    let find = |name: &str| -> Option<&str> {
        sectors
            .iter()
            .find(|(_, n)| n.to_lowercase().contains(&name.to_lowercase()))
            .map(|(s, _)| *s)
    };
    let sym1 = find(sector1).context(format!("Sector '{}' not found", sector1))?;
    let sym2 = find(sector2).context(format!("Sector '{}' not found", sector2))?;
    let quotes = client.get_quote(&[sym1, sym2]).await?;
    if quotes.len() < 2 {
        bail!("Could not fetch both sectors");
    }

    print_header(&format!("{} vs {}", sector1, sector2));
    println!(
        "  {:<20} {:>14} {:>14}",
        "Metric".bold(),
        sector1.bold().cyan(),
        sector2.bold().cyan()
    );
    println!("  {}", "─".repeat(50).dimmed());

    let rows: Vec<(&str, Box<dyn Fn(&crate::api::Quote) -> String>)> = vec![
        (
            "Price",
            Box::new(|q: &crate::api::Quote| {
                format_price(q.regular_market_price.unwrap_or(0.0), q.currency.as_deref())
            }),
        ),
        (
            "Change %",
            Box::new(|q| {
                let v = q.regular_market_change_percent.unwrap_or(0.0);
                if v >= 0.0 {
                    format!("{:+.2}%", v).green().to_string()
                } else {
                    format!("{:+.2}%", v).red().to_string()
                }
            }),
        ),
        (
            "P/E",
            Box::new(|q| q.trailing_pe.map_or("—".into(), |v| format!("{:.1}", v))),
        ),
        (
            "50-Day MA",
            Box::new(|q| format_optional_price(q.fifty_day_average, q.currency.as_deref())),
        ),
        (
            "52W High",
            Box::new(|q| format_optional_price(q.fifty_two_week_high, q.currency.as_deref())),
        ),
        (
            "52W Low",
            Box::new(|q| format_optional_price(q.fifty_two_week_low, q.currency.as_deref())),
        ),
    ];

    for (label, func) in &rows {
        println!(
            "  {:<20} {:>14} {:>14}",
            (*label).dimmed(),
            func(&quotes[0]),
            func(&quotes[1])
        );
    }

    // Verdict
    let pct1 = quotes[0].regular_market_change_percent.unwrap_or(0.0);
    let pct2 = quotes[1].regular_market_change_percent.unwrap_or(0.0);
    let winner = if pct1 > pct2 { sector1 } else { sector2 };
    println!(
        "\n  {} {} is outperforming today.",
        "→".cyan(),
        winner.green().bold()
    );
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
}
