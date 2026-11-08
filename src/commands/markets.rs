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

/// Symbols a scan covers and its title: the members of an NSE index with
/// `--universe` (one bulk NSE request that also prices them; the built-in
/// list when NSE can't answer), else the market's built-in list. Index
/// members are saved for the India cap tiers and the prices feed
/// `market_snapshots`.
pub(super) async fn scan_symbols(
    client: &YahooClient,
    market: Market,
    universe: Option<market::Universe>,
) -> Result<Scan> {
    let Some(u) = universe else {
        return Ok(Scan {
            symbols: market.universe(),
            scope: market.label().to_string(),
            bulk: Vec::new(),
        });
    };
    scan_nse_index(client, u.nse_index(), u.fallback()).await
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
        Err(e) => match fallback {
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
        },
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
    let refs: Vec<&str> = scan.symbols.iter().map(String::as_str).collect();
    let live = client.get_quote(&refs).await;
    if scan.bulk.is_empty()
        && live.as_ref().map_or(true, |q| q.is_empty())
        && let Some(q) = screener_quotes(client, market, cap).await
    {
        return Ok((q, false));
    }
    match live {
        Ok(q) => Ok((market::filter_by_cap(q, cap), false)),
        Err(e) if !scan.bulk.is_empty() => {
            eprintln!(
                "  \u{24d8} Yahoo Finance did not answer ({:#}); using NSE's index snapshot (no P/E or market cap).",
                e
            );
            Ok((market::filter_by_cap(scan.bulk.clone(), cap), false))
        }
        Err(e) => Err(e),
    }
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
fn cap_scope(scope: &str, cap: Option<market::CapFilter>) -> String {
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
            dividend_yield: q.trailing_annual_dividend_yield,
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
            quotes.iter().filter(|q| q.trailing_annual_dividend_yield.is_some_and(|d| d > 0.02)).collect()),
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

    for q in &filtered {
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

/// The `picks` score for one quote: 50 plus or minus fixed amounts for eight
/// factors, and the names of the factors that raised it. The same rules for
/// every stock; a description of the data, not a recommendation.
fn score_quote(q: &crate::api::Quote) -> (f64, Vec<&'static str>) {
    let mut score = 50.0_f64;
    let mut reasons: Vec<&str> = Vec::new();
    // P/E
    if let Some(pe) = q.trailing_pe {
        if pe > 0.0 && pe < 20.0 {
            score += 10.0;
            reasons.push("P/E under 20");
        } else if pe > 40.0 {
            score -= 10.0;
        }
    }
    // Forward > Trailing (earnings growing)
    if let (Some(t), Some(f)) = (q.trailing_pe, q.forward_pe)
        && f < t
        && t > 0.0
        && f > 0.0
    {
        score += 8.0;
        reasons.push("Forward P/E below trailing");
    }
    // Moving averages
    if let (Some(p), Some(ma50), Some(ma200)) = (
        q.regular_market_price,
        q.fifty_day_average,
        q.two_hundred_day_average,
    ) {
        if p > ma50 && ma50 > ma200 {
            score += 12.0;
            reasons.push("Price above 50- and 200-day averages");
        } else if p < ma50 && ma50 < ma200 {
            score -= 10.0;
        }
    }
    // Analyst rating
    if let Some(rec) = q.recommendation_mean {
        if rec <= 2.0 {
            score += 10.0;
            reasons.push("Analyst mean rating 2.0 or better");
        } else if rec >= 4.0 {
            score -= 8.0;
        }
    }
    // Revenue growth
    if let Some(g) = q.revenue_growth
        && g > 0.15
    {
        score += 8.0;
        reasons.push("Revenue growth >15%");
    }
    // Profit margin
    if let Some(m) = q.profit_margins
        && m > 0.2
    {
        score += 5.0;
        reasons.push("Net margin above 20%");
    }
    // Dividend
    if let Some(d) = q.trailing_annual_dividend_yield
        && d > 0.02
    {
        score += 3.0;
        reasons.push("Dividend >2%");
    }
    // Volume
    if let (Some(v), Some(avg)) = (q.regular_market_volume, q.average_daily_volume_3_month)
        && avg > 0
        && v as f64 / avg as f64 > 1.3
    {
        score += 5.0;
        reasons.push("Volume 1.3x average or more");
    }
    (score.clamp(0.0, 100.0), reasons)
}

pub async fn cmd_picks(market: Market) -> Result<()> {
    market.require_stocks("picks")?;
    let client = YahooClient::new().await?;
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header(&format!(
        "Highest Scores in the Large-Cap Universe — {} Market",
        market.label()
    ));
    println!(
        "  {} Scoring {} stocks across 8 factors...\n",
        "⟳".yellow(),
        quotes.len()
    );

    let mut scored: Vec<(&crate::api::Quote, f64, Vec<&str>)> = Vec::new();
    for q in &quotes {
        let (score, reasons) = score_quote(q);
        scored.push((q, score, reasons));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    println!(
        "  {:<14} {:>10} {:>8} {:>7} {}",
        "Symbol".bold(),
        "Price".bold(),
        "Chg%".bold(),
        "Score".bold(),
        "Factors that raised the score".bold()
    );
    println!("  {}", "─".repeat(72).dimmed());

    for (q, score, reasons) in scored.iter().take(15) {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 {
            format!("+{:.2}%", pct).green().to_string()
        } else {
            format!("{:.2}%", pct).red().to_string()
        };
        let score_str = if *score >= 75.0 {
            format!("{:.0}", score).green().bold().to_string()
        } else if *score >= 60.0 {
            format!("{:.0}", score).yellow().to_string()
        } else {
            format!("{:.0}", score).dimmed().to_string()
        };
        let reasons_str = reasons.join(", ");
        println!(
            "  {:<14} {:>10} {:>8} {:>7} {}",
            sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            pct_str,
            score_str,
            reasons_str.dimmed()
        );
    }
    println!(
        "\n  {}",
        "Scores apply the same eight rules to every stock. They describe the data; they are not a recommendation."
            .dimmed()
            .italic()
    );
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

pub async fn cmd_matrix(market: Market) -> Result<()> {
    market.require_stocks("matrix")?;
    let client = YahooClient::new().await?;
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;

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

pub async fn cmd_surprise(market: Market) -> Result<()> {
    market.require_stocks("surprise")?;
    let client = YahooClient::new().await?;
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;

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

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let eps_ttm = q.eps_trailing_twelve_months.unwrap_or(0.0);
        let eps_fwd = q.eps_forward.unwrap_or(0.0);
        if eps_ttm <= 0.0 {
            continue;
        }
        let growth = ((eps_fwd / eps_ttm) - 1.0) * 100.0;
        let pe = q.trailing_pe.unwrap_or(0.0);
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

    for q in &quotes {
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

    #[test]
    fn picks_score_names_its_factors_without_a_call() {
        let q = crate::api::Quote {
            symbol: Some("TCS.NS".into()),
            regular_market_price: Some(100.0),
            fifty_day_average: Some(95.0),
            two_hundred_day_average: Some(90.0),
            trailing_pe: Some(18.0),
            forward_pe: Some(15.0),
            recommendation_mean: Some(1.8),
            revenue_growth: Some(0.2),
            profit_margins: Some(0.25),
            ..Default::default()
        };
        let (score, reasons) = score_quote(&q);
        assert!(score > 75.0, "{score}");
        assert!(reasons.contains(&"Price above 50- and 200-day averages"));
        crate::verdict::testing::assert_neutral(&reasons.join(", "));
        let (bare, none) = score_quote(&crate::api::Quote::default());
        assert_eq!(bare, 50.0);
        assert!(none.is_empty());
    }
}
