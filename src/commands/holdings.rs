//! Your data: watchlist, alerts, import/export, wealth history, rebalancing, tax, and dividends.

use super::portfolio::{ESTIMATED_DATE_HINT, Valuation, portfolio_title, value_holdings};
use super::*;
use crate::portfolio::{self as pf, ImportOptions, ImportOutcome, ImportRow, SyncChange};
use crate::tax::{self, Country};
use crate::tax_export;

pub async fn cmd_watchlist(
    action: &str,
    symbols: &[String],
    force: bool,
    market: Market,
) -> Result<()> {
    let mut wl = Watchlist::load()?;

    match action {
        "add" => {
            let resolved_all = resolve_symbols(symbols, market);
            check_symbols_before_add(&resolved_all, market, force).await?;
            for resolved in resolved_all {
                if wl.add(&resolved) {
                    println!("  {} Added {} to watchlist", "✓".green(), resolved.cyan());
                } else {
                    println!("  {} {} already in watchlist", "·".dimmed(), resolved);
                }
            }
            wl.save()?;
        }
        "remove" | "rm" => {
            for sym in symbols {
                let resolved = market::resolve_symbol(sym, market);
                if wl.remove(&resolved) {
                    println!("  {} Removed {}", "✓".green(), resolved);
                } else {
                    println!("  {} {} not found", "✗".red(), resolved);
                }
            }
            wl.save()?;
        }
        "show" | "list" | "" => return cmd_watchlist_show(Format::Table).await,
        _ => bail!(
            "Unknown watch action '{}'. Use add, remove, or show",
            action
        ),
    }
    Ok(())
}

/// One watchlist entry as `watch --json/--csv` prints it.
#[derive(Debug, Serialize)]
pub(crate) struct WatchRow {
    pub(crate) symbol: String,
    pub(crate) price: Option<f64>,
    pub(crate) change: Option<f64>,
    pub(crate) change_percent: Option<f64>,
    pub(crate) volume: Option<u64>,
    pub(crate) currency: Option<String>,
}

impl CsvRow for WatchRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "price",
        "change",
        "change_percent",
        "volume",
        "currency",
    ];
}

/// One row per watched symbol, in watchlist order; nulls when Yahoo has no quote.
pub(crate) fn watch_rows(symbols: &[String], quotes: &[crate::api::Quote]) -> Vec<WatchRow> {
    symbols
        .iter()
        .map(|sym| {
            let q = quotes.iter().find(|q| q.symbol.as_deref() == Some(sym));
            WatchRow {
                symbol: sym.clone(),
                price: q.and_then(|q| q.regular_market_price),
                change: q.and_then(|q| q.regular_market_change),
                change_percent: q.and_then(|q| q.regular_market_change_percent),
                volume: q.and_then(|q| q.regular_market_volume),
                currency: q.and_then(|q| q.currency.clone()),
            }
        })
        .collect()
}

/// `watch [show]`: the watchlist with live prices.
pub async fn cmd_watchlist_show(fmt: Format) -> Result<()> {
    let wl = Watchlist::load()?;
    if wl.symbols.is_empty() && fmt.is_table() {
        println!(
            "\n  {}",
            "Watchlist is empty. Use 'stockwise watch add <SYMBOL>' to add.".dimmed()
        );
        return Ok(());
    }

    let quotes = if wl.symbols.is_empty() {
        Vec::new()
    } else {
        let sym_refs: Vec<&str> = wl.symbols.iter().map(|s| s.as_str()).collect();
        views::quotes_with_off_market(&YahooClient::new().await?, &sym_refs)
            .await?
            .0
    };
    let rows = watch_rows(&wl.symbols, &quotes);
    match fmt {
        Format::Json => return output::print_json(&rows),
        Format::Csv => return output::print_csv(&rows),
        Format::Table => {}
    }

    print_header("Watchlist");
    println!(
        "  {:<12} {:>10} {:>12} {:>10} {:>14}",
        "Symbol".bold(),
        "Price".bold(),
        "Change".bold(),
        "Change %".bold(),
        "Volume".bold(),
    );
    println!("  {}", "─".repeat(62).dimmed());

    for row in &rows {
        let cur = row.currency.as_deref();
        let Some(price) = row.price else {
            println!(
                "  {:<12} {:>10} {:>12} {:>10} {:>14}",
                row.symbol.yellow(),
                "N/A",
                "—",
                "—",
                "—"
            );
            continue;
        };
        // A fund's NAV has no intraday change: "—", not +0.00.
        let change_str = match row.change {
            Some(c) if c >= 0.0 => format!("+{:.2}", c).green().to_string(),
            Some(c) => format!("{:.2}", c).red().to_string(),
            None => "—".to_string(),
        };
        let pct_str = match row.change_percent {
            Some(p) if p >= 0.0 => format!("+{:.2}%", p).green().to_string(),
            Some(p) => format!("{:.2}%", p).red().to_string(),
            None => "—".to_string(),
        };
        println!(
            "  {:<12} {:>10} {:>12} {:>10} {:>14}",
            row.symbol.cyan(),
            format_price(price, cur),
            change_str,
            pct_str,
            row.volume.map_or("N/A".to_string(), format_volume),
        );
    }
    println!();
    Ok(())
}

// ── Movers ──

/// `alert add`: a price level or a technical condition. `target` is the
/// condition's number; `period` the RSI or volume-average length.
pub async fn cmd_alert_add(
    symbol: &str,
    condition: &str,
    target: f64,
    period: Option<usize>,
    opts: crate::alerts::AddOptions,
    market: Market,
) -> Result<()> {
    let force = opts.force;
    let resolved = market::resolve_symbol(symbol, market);
    let Some(cond) = AlertCondition::parse(condition) else {
        bail!(
            "Unknown alert condition '{}'. Use one of: {}",
            condition,
            AlertCondition::ALL.map(|c| c.name()).join(", ")
        );
    };
    let (target, period) = crate::alerts::validate(cond, target, period)?;
    if opts.on_close && !cond.is_cross() {
        bail!(
            "--on-close applies to moving-average crosses and breakouts (cross-above-sma, ..., breakout-high, breakdown-low)"
        );
    }
    if opts.cross && !cond.is_level() {
        bail!("--cross applies to price levels (above, below)");
    }
    let mut alert = crate::alerts::Alert::new(&resolved, cond, target, period);
    alert.on_close = opts.on_close;
    if opts.cross {
        alert.mode = crate::alerts::AlertMode::Cross;
    }
    let mut store = AlertStore::load()?;
    if let Some((i, _)) = store.duplicate_of(&alert) {
        bail!(
            "Alert #{} already watches {} {} — nothing added",
            i,
            resolved,
            alert.describe(crate::i18n::lang())
        );
    }
    check_symbols_before_add(std::slice::from_ref(&resolved), market, force).await?;
    store.push(alert);
    store.save()?;
    let alert = store.alerts.last().expect("alert was just added");
    println!(
        "  {} Alert #{}: {} {}",
        "✓".green(),
        store.alerts.len() - 1,
        resolved.cyan(),
        alert.describe(crate::i18n::lang())
    );
    if cond == AlertCondition::ResultsIn {
        println!(
            "  {} Uses the results dates from `stockwise calendar`.",
            "→".dimmed()
        );
    } else if !cond.is_price() {
        println!(
            "  {} Checked on daily bars by `stockwise alert check` and `alert watch`{}.",
            "→".dimmed(),
            if opts.on_close {
                ", on completed sessions only"
            } else {
                ""
            }
        );
    } else if opts.cross {
        println!(
            "  {} Notifies only when the price crosses the level, not while it stays beyond it.",
            "→".dimmed()
        );
    }
    Ok(())
}

/// `alert remove N`.
pub fn cmd_alert_remove(index: usize) -> Result<()> {
    let mut store = AlertStore::load()?;
    if store.remove_by_index(index) {
        store.save()?;
        println!("  {} Removed alert #{}", "✓".green(), index);
        Ok(())
    } else {
        bail!("No alert #{} — see `stockwise alert list`", index);
    }
}

/// One alert as `alert list --json/--csv` prints it. `index` is the number
/// `alert remove` takes; `condition` the CLI name (`above`, `rsi-below`,
/// ...), `target` its number and `period` the RSI or volume-average length.
#[derive(Debug, Serialize)]
pub(crate) struct AlertRow {
    pub(crate) index: usize,
    pub(crate) symbol: String,
    pub(crate) condition: String,
    pub(crate) target: f64,
    pub(crate) created_at: String,
    pub(crate) period: Option<usize>,
    pub(crate) description: String,
}

impl CsvRow for AlertRow {
    const COLUMNS: &'static [&'static str] = &[
        "index",
        "symbol",
        "condition",
        "target",
        "created_at",
        "period",
        "description",
    ];
}

pub(crate) fn alert_rows(store: &AlertStore) -> Vec<AlertRow> {
    store
        .alerts
        .iter()
        .enumerate()
        .map(|(i, a)| AlertRow {
            index: i,
            symbol: a.symbol.clone(),
            condition: a.condition.to_string(),
            target: a.target,
            period: a.period(),
            description: a.describe(crate::i18n::Lang::En),
            created_at: a.created_at.clone(),
        })
        .collect()
}

/// `alert [list]`.
pub fn cmd_alert_list(fmt: Format) -> Result<()> {
    let store = AlertStore::load()?;
    let rows = alert_rows(&store);
    match fmt {
        Format::Json => return output::print_json(&rows),
        Format::Csv => return output::print_csv(&rows),
        Format::Table => {}
    }
    if rows.is_empty() {
        println!("\n  {}", NO_ALERTS.dimmed());
        return Ok(());
    }
    print_header(t(Msg::AlertsTitle));
    for (a, alert) in rows.iter().zip(&store.alerts) {
        println!(
            "  #{} {} {}  {}",
            a.index,
            a.symbol.cyan(),
            alert.describe(crate::i18n::lang()),
            format!("(set {})", a.created_at).dimmed()
        );
    }
    println!();
    Ok(())
}

pub(crate) const NO_ALERTS: &str = "No alerts. Use 'stockwise alert add <SYMBOL> above/below <PRICE>' \
     or a technical condition such as 'rsi-below 30' (see 'stockwise alert add --help').";

/// One alert checked against live data, as `alert check --json/--csv`
/// prints it. `status` is `triggered`, `waiting`, `no_price` (no live price
/// for a price alert) or `no_data` (too little history, or no results
/// date). `value` is what the condition compared — the price, RSI, day
/// change %, volume multiple or days to results; for crosses and breakouts
/// the close — and `reference` the moving average, previous high/low or
/// average volume it was compared with.
#[derive(Debug, Serialize)]
pub(crate) struct AlertCheckRow {
    pub(crate) index: usize,
    pub(crate) symbol: String,
    pub(crate) condition: String,
    pub(crate) target: f64,
    pub(crate) price: Option<f64>,
    pub(crate) currency: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) period: Option<usize>,
    pub(crate) description: String,
    pub(crate) value: Option<f64>,
    pub(crate) reference: Option<f64>,
}

impl CsvRow for AlertCheckRow {
    const COLUMNS: &'static [&'static str] = &[
        "index",
        "symbol",
        "condition",
        "target",
        "price",
        "currency",
        "status",
        "period",
        "description",
        "value",
        "reference",
    ];
}

/// Each alert evaluated against its symbol's market data (see
/// [`views::alert_data`]). Missing data never reads as triggered.
pub(crate) fn alert_check_rows(
    alerts: &[crate::alerts::Alert],
    data: &std::collections::HashMap<String, crate::alerts::MarketData>,
) -> Vec<AlertCheckRow> {
    let empty = crate::alerts::MarketData::default();
    alerts
        .iter()
        .enumerate()
        .map(|(i, alert)| {
            let d = data.get(&alert.symbol).unwrap_or(&empty);
            let reading = alert.evaluate(d);
            let status = match reading {
                Some(r) if r.hit => "triggered",
                Some(_) => "waiting",
                None if alert.condition.is_price() => "no_price",
                None => "no_data",
            };
            AlertCheckRow {
                index: i,
                symbol: alert.symbol.clone(),
                condition: alert.condition.to_string(),
                target: alert.target,
                period: alert.period(),
                description: alert.describe(crate::i18n::Lang::En),
                price: d.price,
                currency: d.currency.clone(),
                value: reading.map(|r| output::round(r.value, 4)),
                reference: reading
                    .and_then(|r| r.reference)
                    .map(|v| output::round(v, 4)),
                status,
            }
        })
        .collect()
}

/// `alert check`: every alert against live prices, daily bars and the
/// results calendar.
pub async fn cmd_alert_check(fmt: Format) -> Result<()> {
    let store = AlertStore::load()?;
    if store.alerts.is_empty() && fmt.is_table() {
        println!("  {}", "No alerts configured.".dimmed());
        return Ok(());
    }
    let rows = if store.alerts.is_empty() {
        Vec::new()
    } else {
        views::alerts(&YahooClient::new().await?).await?
    };
    match fmt {
        Format::Json => return output::print_json(&rows),
        Format::Csv => return output::print_csv(&rows),
        Format::Table => {}
    }

    print_header("Alert Check");
    for (row, alert) in rows.iter().zip(&store.alerts) {
        let words = alert.describe(crate::i18n::lang());
        let now = row
            .price
            .map(|p| format_price(p, row.currency.as_deref()))
            .unwrap_or_else(|| "N/A".to_string());
        let reading = row
            .value
            .and_then(|value| {
                alert.reading_text(&crate::alerts::Reading {
                    hit: false,
                    value,
                    reference: row.reference,
                })
            })
            .map(|r| format!(" | {r}"))
            .unwrap_or_default();
        let status = match row.status {
            "triggered" => "TRIGGERED".red().bold().to_string(),
            "waiting" => "waiting".dimmed().to_string(),
            "no_price" => "no live price, skipped".yellow().to_string(),
            _ if alert.condition == AlertCondition::ResultsIn => {
                "no upcoming results date".yellow().to_string()
            }
            _ => "not enough data, skipped".yellow().to_string(),
        };
        println!(
            "  {} {} | Now: {}{} | {}",
            row.symbol.cyan(),
            words,
            now,
            reading,
            status
        );
    }
    let triggered = rows.iter().filter(|r| r.status == "triggered").count();
    if triggered == 0 {
        println!("\n  {}", "No alerts triggered.".dimmed());
    } else {
        println!("\n  {} {} alert(s) triggered!", "⚠".yellow(), triggered);
    }
    println!();
    Ok(())
}

// ── Screener ──

pub async fn cmd_export(what: &str, market: Market) -> Result<()> {
    match what {
        "portfolio" => {
            let portfolio = Portfolio::load()?;
            if portfolio.holdings.is_empty() {
                // Export output may be piped into a file: the hint goes to stderr.
                eprintln!(
                    "  {}",
                    crate::i18n::t(crate::i18n::Msg::PortfolioEmpty).dimmed()
                );
                return Ok(());
            }
            let client = YahooClient::new().await?;
            let symbols: Vec<String> = portfolio
                .holdings
                .iter()
                .map(|h| h.symbol.clone())
                .collect();
            // Funds at their NAV, SGBs from NSE; no price = empty cells, never 0.
            let val = value_holdings(&client, &symbols, market, None).await?;
            let path = "portfolio_export.csv";
            let mut csv = String::from("Symbol,Shares,Avg Cost,Current Price,Value,P&L,P&L %\n");
            for h in &portfolio.holdings {
                let cost = h.shares * h.avg_cost;
                let cells = match val.price(&h.symbol) {
                    Some(price) => {
                        let value = h.shares * price;
                        let pnl = value - cost;
                        let pnl_pct = if cost > 0.0 {
                            format!("{:.2}%", pnl / cost * 100.0)
                        } else {
                            String::new()
                        };
                        format!("{price:.2},{value:.2},{pnl:.2},{pnl_pct}")
                    }
                    None => ",,,".to_string(),
                };
                csv.push_str(&format!(
                    "{},{:.2},{:.2},{}\n",
                    h.symbol, h.shares, h.avg_cost, cells
                ));
            }
            std::fs::write(path, &csv)?;
            println!("  {} Exported portfolio to {}", "✓".green(), path.cyan());
        }
        "watchlist" => {
            let wl = Watchlist::load()?;
            if wl.symbols.is_empty() {
                println!("  {}", "Watchlist is empty.".dimmed());
                return Ok(());
            }
            let client = YahooClient::new().await?;
            let sym_refs: Vec<&str> = wl.symbols.iter().map(|s| s.as_str()).collect();
            let quotes = views::quotes_with_off_market(&client, &sym_refs).await?.0;
            let path = "watchlist_export.csv";
            let mut csv = String::from("Symbol,Price,Change,Change %,Volume,Market Cap\n");
            // Missing values are empty cells, never 0.
            let num = |v: Option<f64>, places: usize| {
                v.map_or(String::new(), |v| format!("{v:.places$}"))
            };
            for q in &quotes {
                csv.push_str(&format!(
                    "{},{},{},{},{},{}\n",
                    q.symbol.as_deref().unwrap_or(""),
                    num(q.regular_market_price, 2),
                    num(q.regular_market_change, 2),
                    q.regular_market_change_percent
                        .map_or(String::new(), |p| format!("{p:.2}%")),
                    q.regular_market_volume
                        .map_or(String::new(), |v| v.to_string()),
                    num(q.market_cap, 0)
                ));
            }
            std::fs::write(path, &csv)?;
            println!("  {} Exported watchlist to {}", "✓".green(), path.cyan());
        }
        _ => bail!("Unknown export '{}'. Use portfolio or watchlist", what),
    }
    Ok(())
}

// ── Alerts ──

/// Column names a generic stock-holdings import looks for.
struct ImportColumns {
    symbol: &'static [&'static str],
    qty: &'static [&'static str],
    price: &'static [&'static str],
    /// For the error message.
    expected: &'static str,
}

/// Column names per format, most specific first (matched as substrings).
fn import_columns(source: &str) -> ImportColumns {
    let (symbol, qty, price, expected): (&[&str], &[&str], &[&str], &str) = match source {
        // Kite holdings: Instrument, Qty., Avg. cost, LTP, ...
        // Kite tradebook: symbol, trade_date, trade_type, quantity, price, ...
        "kite" | "zerodha" => (
            &["instrument", "tradingsymbol", "symbol", "stock"],
            &["qty", "qty.", "quantity", "shares"],
            &[
                "avg. cost",
                "avg cost",
                "average cost",
                "avg_price",
                "average price",
                "buy avg",
                "buy avg.",
                "price",
            ],
            "Kite CSV/Excel columns. Expected: Instrument, Qty., Avg. cost",
        ),
        // IndMoney: Stock Name, Symbol/ISIN, Quantity, Avg Buy Price, ...
        "indmoney" => (
            &[
                "symbol",
                "isin",
                "stock symbol",
                "scrip",
                "stock name",
                "name",
            ],
            &["quantity", "qty", "shares", "units"],
            &[
                "avg buy price",
                "avg price",
                "average price",
                "buy price",
                "avg. buy price",
                "average cost",
            ],
            "IndMoney CSV/Excel columns. Expected: Symbol/Stock Name, Quantity, Avg Buy Price",
        ),
        // "csv" or any unrecognized format: generic columns.
        _ => (
            &[
                "symbol",
                "stock",
                "instrument",
                "name",
                "ticker",
                "scrip",
                "tradingsymbol",
            ],
            &["quantity", "qty", "qty.", "shares", "units"],
            &[
                "avg price",
                "avg. price",
                "average price",
                "avg cost",
                "avg. cost",
                "buy price",
                "price",
                "cost",
            ],
            "CSV/Excel columns. Your file needs Symbol, Quantity and Average Price columns",
        ),
    };
    ImportColumns {
        symbol,
        qty,
        price,
        expected,
    }
}

/// The header row of a generic holdings export, in its first 40 lines (an
/// `.xlsx` export may start with account details), and the columns of the
/// symbol, quantity and price.
fn find_stock_header(
    lines: &[&str],
    columns: &ImportColumns,
) -> Option<(usize, Vec<String>, usize, usize, usize)> {
    // Scan the first 40 lines for the header row so .xlsx exports with account preamble work seamlessly.
    lines.iter().take(40).enumerate().find_map(|(idx, line)| {
        let c = parse_csv_header(&line.to_lowercase());
        let si = find_col(&c, columns.symbol)?;
        let qi = find_col(&c, columns.qty)?;
        let pi = find_col(&c, columns.price)?;
        Some((idx, c, si, qi, pi))
    })
}

/// Whether a workbook sheet (as CSV) has the header `source`'s importer needs.
pub(super) fn sheet_has_header(source: &str, csv: &str) -> bool {
    match source {
        "groww" => super::imports::parse_groww(csv, Market::In).is_ok(),
        "upstox" => super::imports::parse_upstox(csv, Market::In).is_ok(),
        "cas" => super::imports::parse_cas(csv).is_ok(),
        _ => {
            let lines: Vec<&str> = csv.lines().collect();
            find_stock_header(&lines, &import_columns(source)).is_some()
        }
    }
}

pub async fn cmd_import(
    source: &str,
    file: &str,
    replace: bool,
    sheet: Option<&str>,
    account: Option<&str>,
    market: Market,
) -> Result<()> {
    let path = std::path::Path::new(file);
    if !path.exists() {
        println!();
        println!("  {} How to export your holdings:", "→".cyan());
        match source {
            "kite" | "zerodha" => {
                println!(
                    "    1. Login to {} → Portfolio → Holdings",
                    "kite.zerodha.com".cyan()
                );
                println!(
                    "    2. Click the {} icon (top-right of holdings table)",
                    "download/export".bold()
                );
                println!("    3. Save the CSV file");
                println!("    4. Run: stockwise import kite <path-to-file.csv>");
            }
            "indmoney" => {
                println!("    1. Open {} app → Portfolio → Stocks", "IndMoney".cyan());
                println!(
                    "    2. Tap {} → Download Statement / Export",
                    "⋮ (menu)".bold()
                );
                println!("    3. Save the CSV/Excel file");
                println!("    4. Run: stockwise import indmoney <path-to-file.csv>");
            }
            "groww" => {
                println!("    1. Open {} → Stocks → Holdings", "groww.in".cyan());
                println!(
                    "    2. Click {} to get the holdings statement (.xlsx or .csv)",
                    "Download".bold()
                );
                println!("    3. Run: stockwise import groww <path-to-file.xlsx>");
            }
            "upstox" => {
                println!(
                    "    1. Open {} → Portfolio → Holdings",
                    "pro.upstox.com".cyan()
                );
                println!(
                    "    2. Click {} and save the .xlsx or .csv file",
                    "Download".bold()
                );
                println!("    3. Run: stockwise import upstox <path-to-file.xlsx>");
            }
            "cas" => {
                println!(
                    "    1. Request a detailed CAS from {} or {} (emailed as a password-protected PDF)",
                    "camsonline.com".cyan(),
                    "kfintech.com".cyan()
                );
                println!(
                    "    2. Convert it with casparser: {}",
                    "pip install casparser && casparser CAS.pdf -p <password> -o cas.json".bold()
                );
                println!("    3. Run: stockwise import cas cas.json");
            }
            _ => {
                println!("    Provide a CSV with columns: Symbol, Quantity, Average Price");
                println!("    Run: stockwise import csv <path-to-file.csv>");
            }
        }
        bail!("File not found: {}", file);
    }

    let raw_bytes = std::fs::read(path).with_context(|| format!("Failed to read {}", file))?;
    let content = if raw_bytes.starts_with(b"PK\x03\x04") {
        let sheets = crate::xlsx::read_workbook(&raw_bytes)
            .with_context(|| format!("Failed to read Excel (.xlsx) workbook {}", file))?;
        // Only one sheet is read: rows of another sheet (a mutual-fund tab, a
        // summary) have other columns and must not be parsed with these.
        let chosen = crate::xlsx::pick_sheet(&sheets, sheet, |csv| sheet_has_header(source, csv))?;
        if sheets.len() > 1 {
            let skipped: Vec<&str> = sheets
                .iter()
                .filter(|s| !std::ptr::eq(*s, chosen))
                .map(|s| s.name.as_str())
                .collect();
            println!(
                "  {} Using sheet \"{}\" (skipped: {}). Pick another with --sheet NAME.",
                "·".dimmed(),
                chosen.name,
                skipped.join(", ")
            );
        }
        chosen.to_csv()
    } else {
        if sheet.is_some() {
            bail!("--sheet only applies to .xlsx workbooks, and {file} is not one");
        }
        String::from_utf8(raw_bytes)
            .with_context(|| format!("{} is not valid UTF-8 text or an .xlsx workbook", file))?
    };
    match source {
        "groww" => {
            return super::imports::import_rows(
                "groww",
                file,
                replace,
                account,
                // Groww and Upstox list Indian stocks, whatever -m says.
                super::imports::parse_groww(&content, Market::In)?,
            )
            .await;
        }
        "upstox" => {
            return super::imports::import_rows(
                "upstox",
                file,
                replace,
                account,
                super::imports::parse_upstox(&content, Market::In)?,
            )
            .await;
        }
        "cas" => {
            return super::imports::import_rows(
                "cas",
                file,
                replace,
                account,
                super::imports::parse_cas(&content)?,
            )
            .await;
        }
        _ => {}
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < 2 {
        bail!("{} is empty or has no data rows", file);
    }

    let columns = import_columns(source);
    let ImportColumns { expected, .. } = columns;
    let Some((header_idx, cols, si, qi, pi)) = find_stock_header(&lines, &columns) else {
        bail!(
            "Could not detect {}. Found: {}",
            expected,
            lines[0].to_lowercase()
        );
    };
    // Optional: a purchase/trade date per row, and buy/sell for tradebooks.
    let date_col = find_col_exact(
        &cols,
        &[
            "trade_date",
            "trade date",
            "buy date",
            "buy_date",
            "purchase date",
            "purchase_date",
            "date of purchase",
            "acquisition date",
            "date acquired",
            "date",
        ],
    );
    let side_col = find_col_exact(
        &cols,
        &[
            "trade_type",
            "trade type",
            "transaction type",
            "transaction_type",
            "type",
            "side",
            "buy/sell",
            "action",
        ],
    );
    let month_first = market == Market::Us;
    let field = |fields: &[String], i: usize| -> String {
        fields
            .get(i)
            .map(|f| f.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let number = |raw: String| raw.replace(',', "").parse::<f64>().unwrap_or(0.0);

    let mut rows = Vec::new();
    let mut bad_dates = Vec::new();
    let mut merged: Vec<(&'static pf::Merger, usize)> = Vec::new();
    for line in &lines[header_idx + 1..] {
        let fields = parse_csv_row(line);
        if fields.len() <= si.max(qi).max(pi) {
            continue;
        }
        let symbol_raw = field(&fields, si);
        if symbol_raw.is_empty() {
            continue;
        }
        let mut qty = number(field(&fields, qi));
        let mut price = number(field(&fields, pi));
        // "NaN"/"inf" parse as f64 but can't be saved as JSON.
        if !(qty.is_finite() && qty > 0.0 && price.is_finite() && price > 0.0) {
            continue;
        }
        let date = match date_col.map(|i| field(&fields, i)) {
            Some(raw) if !raw.is_empty() => match pf::parse_trade_date(&raw, month_first) {
                Some(d) => Some(d.to_string()),
                None => {
                    bad_dates.push(raw);
                    None
                }
            },
            _ => None,
        };
        let sell = side_col.is_some_and(|i| {
            matches!(
                field(&fields, i).to_lowercase().as_str(),
                "sell" | "s" | "sold"
            )
        });
        // A merged-away ticker (HDFC, MINDTREE) is restated in the acquirer's
        // shares, keeping the cost and date; a plain rename only maps the name.
        let mut symbol_raw = symbol_raw;
        let mut units_as_of = None;
        if market == Market::In
            && let Some(swap) = pf::merger_swap(&symbol_raw, date.as_deref(), qty, price)
        {
            match merged
                .iter_mut()
                .find(|(m, _)| std::ptr::eq(*m, swap.merger))
            {
                Some((_, n)) => *n += 1,
                None => merged.push((swap.merger, 1)),
            }
            (symbol_raw, qty, price) = (swap.symbol, swap.qty, swap.price);
            // The restated quantity is the acquirer's share count as of the
            // merger: its splits up to then are already in the ratio.
            units_as_of = Some(swap.merger.through.to_string());
        }
        rows.push(ImportRow {
            symbol: market::resolve_symbol(&symbol_raw, market),
            qty,
            price,
            date,
            sell,
            units_as_of,
        });
    }
    if rows.is_empty() {
        bail!("No holdings found in {}. Check the format.", file);
    }
    // A wrong date would silently change tax and XIRR: refuse the file.
    if !bad_dates.is_empty() {
        bail!(
            "Unrecognized dates in {}: {}. Use YYYY-MM-DD (or DD-MM-YYYY), then import again — nothing was changed.",
            file,
            bad_dates.join(", ")
        );
    }

    for (m, n) in &merged {
        println!(
            "  {} {} row(s) of {} dated up to {} restated as {} at {} for {} (quantity and price adjusted; cost and dates unchanged)",
            "·".dimmed(),
            n,
            m.old,
            m.through,
            m.new,
            m.new_shares,
            m.old_shares
        );
    }
    apply_import_rows(source, file, rows, side_col.is_some(), replace, account).await
}

/// Splits and bonuses for each dated symbol, from its first trade. A symbol
/// Yahoo has no chart for (HTTP 404, such as a renamed ticker) is not an
/// error: it is returned in the second list and imported without a split
/// check. Any other failure stops the import, so nothing is changed.
pub(super) async fn fetch_import_splits(
    client: &YahooClient,
    rows: &[ImportRow],
    symbols: &[&String],
) -> Result<(Vec<(String, Vec<crate::portfolio::Split>)>, Vec<String>)> {
    let mut found = Vec::new();
    let mut unchecked = Vec::new();
    for sym in symbols {
        let since = rows
            .iter()
            .filter(|r| &r.symbol == *sym)
            .filter_map(|r| r.date.clone())
            .min()
            .unwrap_or_else(pf::today);
        match super::portfolio::fetch_actions(client, sym, &since).await {
            Ok((s, _, _)) => found.push(((*sym).clone(), s)),
            Err(e) if crate::api::is_not_found(&e) => unchecked.push((*sym).clone()),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "Could not check {} for splits, which a tradebook needs; nothing was changed. Try again when online",
                        sym
                    )
                });
            }
        }
    }
    Ok((found, unchecked))
}

/// `import --account NAME` checked and lower-cased: letters, digits, `-`,
/// `_` and `.`, up to 40 characters. `None` stays `None` (the broker's name).
pub fn account_name(name: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = name else {
        return Ok(None);
    };
    let name = raw.trim().to_lowercase();
    if name.is_empty()
        || name.len() > 40
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!(
            "--account {raw:?}: use letters, digits, '-', '_' or '.' (up to 40), e.g. `--account spouse`"
        );
    }
    Ok(Some(name))
}

/// Apply parsed import rows (from any source) to the portfolio and report
/// what changed: dated rows rebuild a symbol's lots, undated rows sync its
/// quantity and keep existing lots. Re-importing the same rows changes
/// nothing.
pub(super) async fn apply_import_rows(
    source: &str,
    file: &str,
    rows: Vec<ImportRow>,
    tradebook: bool,
    replace: bool,
    account: Option<&str>,
) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if let Some(a) = account {
        println!(
            "  {} Account: {} (lots and sales from this file are kept apart from other accounts)",
            "·".dimmed(),
            a.cyan()
        );
    }
    let dated = |sym: &str| {
        rows.iter()
            .any(|r| r.symbol == sym && (r.date.is_some() || r.sell))
    };
    let mut symbols: Vec<String> = rows.iter().map(|r| r.symbol.clone()).collect();
    symbols.dedup();
    symbols.sort();
    symbols.dedup();

    // A tradebook's quantities are as traded: fetch each symbol's splits so
    // sells after a split or bonus match the right number of shares.
    let mut splits = Vec::new();
    if tradebook {
        let client = YahooClient::new().await?;
        let dated_symbols: Vec<&String> = symbols.iter().filter(|s| dated(s)).collect();
        let (mut found, unchecked) = fetch_import_splits(&client, &rows, &dated_symbols).await?;
        // Indian bonus issues are replayed as zero-cost lots, not as splits.
        let mut cal = crate::calendar::Calendar::new(&client);
        let mut nse_offline = Vec::new();
        for (sym, sym_splits) in found.iter_mut() {
            if !super::portfolio::mark_bonuses(&mut cal, sym, sym_splits).await {
                nse_offline.push(sym.clone());
            }
        }
        cal.save();
        if !nse_offline.is_empty() {
            println!(
                "  {} {}",
                "!".yellow(),
                super::portfolio::nse_offline_note(&cal, &nse_offline)
            );
        }
        splits = found;
        for sym in unchecked {
            println!(
                "  {} {} — split check skipped: Yahoo has no price history for it, so its quantities are used as in the file",
                "!".yellow(),
                sym
            );
        }
    }

    // A holdings snapshot with more shares than StockWise has may just be a
    // split not yet applied to your lots: leave those for `portfolio actions`.
    let import_source = format!("import:{source}");
    let resolved_account = account.or_else(|| pf::account_of_source(&import_source));
    let snapshot: Vec<(String, f64)> = symbols
        .iter()
        .filter(|s| !dated(s))
        .map(|s| {
            let qty = rows.iter().filter(|r| &r.symbol == s).map(|r| r.qty).sum();
            (s.clone(), qty)
        })
        .collect();
    let fetched_blocked = super::portfolio::blocked_by_pending_splits_in_account(
        &portfolio,
        &snapshot,
        resolved_account,
    )
    .await;

    // The splits and pending-split checks above took time: apply the rows to
    // the file as it is now, under a lock, so a change saved meanwhile (a
    // `portfolio add`, a sync) is kept.
    let (outcomes, current, blocked) = Portfolio::update(|p| {
        let mut blocked = Vec::new();
        for (sym, sp, reason) in &fetched_blocked {
            let lots = p.lots_in_account(sym, resolved_account);
            let held: f64 = lots.iter().map(|l| l.qty).sum();
            let qty = snapshot
                .iter()
                .find(|(s, _)| s == sym)
                .map_or(0.0, |(_, q)| *q);
            if !lots.is_empty()
                && qty > held + 1e-9
                && sp.iter().any(|s| lots.iter().any(|l| s.pending_for(l)))
            {
                blocked.push((sym.clone(), reason.clone()));
            }
        }
        let rows: Vec<ImportRow> = rows
            .iter()
            .filter(|r| !blocked.iter().any(|(s, _)| *s == r.symbol))
            .cloned()
            .collect();
        let outcomes = p.apply_import(
            &rows,
            ImportOptions {
                source: &import_source,
                replace,
                tradebook,
                splits: &splits,
                account,
            },
        );
        Ok((outcomes, p.clone(), blocked))
    })?;
    let rows: Vec<ImportRow> = rows
        .into_iter()
        .filter(|r| !blocked.iter().any(|(s, _)| *s == r.symbol))
        .collect();
    let portfolio = current;
    let updated = outcomes
        .iter()
        .filter(|(_, o)| {
            !matches!(
                o,
                ImportOutcome::Unmatched { .. } | ImportOutcome::WouldReplace
            )
        })
        .count();

    let mut undated = false;
    for (sym, outcome) in &outcomes {
        let cur = pf::symbol_currency(sym);
        let now = portfolio.holding(sym).map_or_else(
            || "none left".to_string(),
            |h| format!("{} shares @ {}", h.shares, format_price(h.avg_cost, cur)),
        );
        match outcome {
            ImportOutcome::Synced(SyncChange::New) => {
                undated = true;
                println!("  {} {} — {} (new)", "✓".green(), sym.cyan(), now);
            }
            ImportOutcome::Synced(SyncChange::Unchanged) => {
                println!("  {} {} — {}, unchanged", "·".dimmed(), sym, now);
            }
            ImportOutcome::Synced(SyncChange::Added(n)) => {
                undated = true;
                println!(
                    "  {} {} — +{} shares, now {}",
                    "✓".green(),
                    sym.cyan(),
                    n,
                    now
                );
            }
            ImportOutcome::Synced(SyncChange::Reduced(n)) => println!(
                "  {} {} — {} fewer shares (taken from the oldest lots, not recorded as a sale), now {}",
                "✓".green(),
                sym.cyan(),
                n,
                now
            ),
            ImportOutcome::Lots {
                lots,
                sales,
                splits,
            } => {
                let mut extra = String::new();
                if *sales > 0 {
                    extra.push_str(&format!(", {} sale(s) recorded", sales));
                }
                if *splits > 0 {
                    extra.push_str(&format!(", adjusted for {} split(s)/bonus(es)", splits));
                }
                println!(
                    "  {} {} — {} lot(s){}, now {}",
                    "✓".green(),
                    sym.cyan(),
                    lots,
                    extra,
                    now
                );
            }
            ImportOutcome::Unmatched { unmatched } => println!(
                "  {} {} — not applied yet: sells of {} shares have no earlier buy in the trades \
                 imported so far. Import the earlier tradebook (the trades from this file are kept), \
                 or use --replace to rebuild from this file alone.",
                "!".yellow(),
                sym,
                unmatched
            ),
            ImportOutcome::WouldReplace => println!(
                "  {} {} — skipped: it has lots or sales from elsewhere (manual entries, another \
                 import, or a tradebook imported by an older version), or sales and trades kept \
                 from an earlier tradebook of this source that a file of lots alone would delete. \
                 Re-run with --replace to rebuild it from this file and delete those.",
                "!".yellow(),
                sym
            ),
        }
    }
    for (sym, reason) in &blocked {
        println!("  {} {} — skipped: {}", "!".yellow(), sym, reason);
    }

    if updated == 0 {
        bail!(
            "Nothing imported from {} — see the skipped holdings above.",
            file
        );
    }
    println!(
        "\n  {} Imported {} rows: {} holdings updated.",
        "✓".green().bold(),
        rows.len(),
        updated
    );
    if undated {
        println!(
            "  {}",
            "This file has no purchase dates, so new shares are dated today (marked *). Add a \
             date column, or fix dates with `stockwise portfolio lots SYMBOL --lot N --date YYYY-MM-DD`."
                .dimmed()
        );
    }
    println!(
        "  Run {} to see your portfolio.",
        "stockwise portfolio show".cyan()
    );
    println!();
    Ok(())
}

pub(super) fn parse_csv_header(header: &str) -> Vec<String> {
    parse_csv_row(header)
        .iter()
        .map(|s| s.trim().trim_matches('"').to_lowercase())
        .collect()
}

pub(super) fn parse_csv_row(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

/// Like [`find_col`] but the column name must match exactly, for short
/// names such as "date" or "type" that would match too much as substrings.
pub(super) fn find_col_exact(cols: &[String], names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|name| cols.iter().position(|c| c == name))
}

pub(super) fn find_col(cols: &[String], names: &[&str]) -> Option<usize> {
    for name in names {
        if let Some(i) = cols.iter().position(|c| c.contains(name)) {
            return Some(i);
        }
    }
    None
}

// ── Angel One Trading ──

pub async fn cmd_wealth(market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;

    if portfolio.holdings.is_empty() {
        println!(
            "\n  {}",
            crate::i18n::t(crate::i18n::Msg::PortfolioEmpty).dimmed()
        );
        return Ok(());
    }

    // Fetch current values and snapshot
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let client = YahooClient::new().await?;
    // Always the market's currency, so the history never mixes currencies
    // as holdings come and go.
    let base = super::portfolio::market_currency(market);
    let val = value_holdings(&client, &symbols, market, Some(base)).await?;
    let totals = val.totals(&portfolio);
    let (total_value, total_cost) = (totals.value, totals.cost);
    // A snapshot with a holding valued at 0 would show as a fake crash in the
    // history forever, so refuse to save one.
    if !totals.unpriced.is_empty() || !totals.unconverted.is_empty() {
        let missing: Vec<String> = totals
            .unpriced
            .iter()
            .chain(&totals.unconverted)
            .cloned()
            .collect();
        bail!(
            "No live price or exchange rate for {}; snapshot not saved. Fix or remove the holding, then retry.",
            missing.join(", ")
        );
    }

    // Save today's snapshot
    let history = WealthHistory::update(|h| {
        h.add_snapshot(total_value, total_cost, portfolio.holdings.len());
        Ok(h.clone())
    })?;

    let cur = Some(val.base());
    let csym = market::currency_symbol(cur);

    print_header("Wealth Tracker");

    if history.snapshots.len() >= 2 {
        let values: Vec<f64> = history.snapshots.iter().map(|s| s.total_value).collect();
        let color = if *values.last().unwrap() >= values[0] {
            "green"
        } else {
            "red"
        };
        for line in charts::line_chart(&values, 55, 8, color, "Portfolio Value Over Time") {
            println!("{}", line);
        }
    }

    print_section("Current Snapshot");
    print_kv(
        "Total Value",
        &format!("{}{:.2}", csym, total_value).bold().to_string(),
    );
    print_kv("Total Invested", &format!("{}{:.2}", csym, total_cost));
    let pnl = total_value - total_cost;
    let pnl_pct = if total_cost > 0.0 {
        (pnl / total_cost) * 100.0
    } else {
        0.0
    };
    let pnl_str = if pnl >= 0.0 {
        format!("+{}{:.2} (+{:.2}%)", csym, pnl, pnl_pct)
            .green()
            .bold()
            .to_string()
    } else {
        format!("-{}{:.2} ({:.2}%)", csym, pnl.abs(), pnl_pct)
            .red()
            .bold()
            .to_string()
    };
    print_kv("Total P&L", &pnl_str);
    if let Some(note) = val.fx_note() {
        println!("  {}", note.dimmed());
    }
    print_kv("Snapshots", &history.snapshots.len().to_string());
    val.print_price_notes();

    if history.snapshots.len() >= 2 {
        let first = &history.snapshots[0];
        let growth = ((total_value / first.total_value) - 1.0) * 100.0;
        print_kv(
            "Since First Track",
            &format!("{:+.2}% (from {})", growth, first.date),
        );
    }

    println!(
        "\n  {}",
        "Run 'stockwise wealth' daily to build your growth chart."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

// ── Earnings Calendar ──

/// One tax-loss or tax-gain harvesting candidate row for `harvest --json/--csv`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HarvestRow {
    pub(crate) symbol: String,
    pub(crate) shares: f64,
    pub(crate) avg_cost: f64,
    pub(crate) price: f64,
    pub(crate) pnl: f64,
    pub(crate) pnl_pct: f64,
    pub(crate) st_pnl: f64,
    pub(crate) lt_pnl: f64,
    pub(crate) estimated_tax_saved: f64,
    pub(crate) harvest_type: &'static str,
    /// US loss candidates: the latest purchase of the symbol in the last 30
    /// days that this sale would not sell. Selling now would make the loss a
    /// wash sale, disallowed in proportion to those shares.
    pub(crate) recent_buy: Option<String>,
    /// US loss candidates: the last day of the wash-sale window if the
    /// shares were sold today (30 days on). A purchase of the same shares up
    /// to this day would make the loss a wash sale.
    pub(crate) wash_sale_window_end: Option<String>,
    /// The lots the sale takes (numbers from `portfolio lots`), when they
    /// are not simply the oldest: record it with `portfolio sell --lots`.
    pub(crate) lots: Option<String>,
    /// The command that records this sale (printed on screen; left out of
    /// `--json` and `--csv`, which carry `lots` instead).
    #[serde(skip)]
    pub(crate) record: Option<String>,
}

impl CsvRow for HarvestRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "shares",
        "avg_cost",
        "price",
        "pnl",
        "pnl_pct",
        "st_pnl",
        "lt_pnl",
        "estimated_tax_saved",
        "harvest_type",
        "recent_buy",
        "wash_sale_window_end",
        "lots",
    ];
}

/// A holding with a losing lot that FIFO does not let you sell on its own:
/// the older lots come first, and selling them books a net gain.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct FifoBlockedRow {
    pub(crate) symbol: String,
    /// Shares you would have to sell to reach the first losing lot.
    pub(crate) shares_to_sell: f64,
    /// What that sale books in all: positive is a net gain.
    pub(crate) net_result: f64,
    /// The losing lot's own loss.
    pub(crate) lot_loss: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HarvestSummary {
    pub(crate) country: &'static str,
    pub(crate) currency: &'static str,
    pub(crate) total_st_loss_harvestable: f64,
    pub(crate) total_lt_loss_harvestable: f64,
    pub(crate) estimated_loss_tax_saved: f64,
    pub(crate) ltcg_exemption_limit: f64,
    pub(crate) ltcg_exemption_remaining: f64,
    pub(crate) loss_candidates: Vec<HarvestRow>,
    pub(crate) gain_harvest_candidates: Vec<HarvestRow>,
    /// India: holdings with a losing lot that FIFO makes unsellable on its
    /// own without booking a net gain.
    pub(crate) fifo_blocked: Vec<FifoBlockedRow>,
    pub(crate) unpriced: Vec<String>,
    /// India: holdings left out because they are not taxed like listed
    /// shares (non-equity ETFs, debt and other funds, SGBs), so neither the
    /// ₹1.25 lakh exemption nor the 111A/112A rates apply to them.
    pub(crate) other_rules: Vec<OtherRulesRow>,
    /// Holdings in a currency this country does not tax in (US shares for
    /// India), left out like `tax in` does.
    pub(crate) other_currency: Vec<String>,
    /// Losses brought forward from earlier years, counted with this year's.
    pub(crate) carried_forward: CarriedForward,
    /// Long-term gain that can still be booked this year without tax: the
    /// exemption left plus any losses that would absorb it first.
    pub(crate) tax_free_gain_room: f64,
    /// Rules to keep in mind when acting on the rows.
    pub(crate) notes: Vec<String>,
}

/// Capital losses brought forward from earlier years (`harvest --carry-st
/// / --carry-lt`), in the tax currency.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct CarriedForward {
    pub short_term: f64,
    pub long_term: f64,
}

impl CarriedForward {
    fn buckets(self) -> tax::Buckets {
        tax::Buckets {
            st_loss: self.short_term.max(0.0),
            lt_loss: self.long_term.max(0.0),
            ..Default::default()
        }
    }
}

/// The sale that would harvest a holding's losses, in the tax currency.
struct HarvestSale {
    shares: f64,
    cost: f64,
    st: f64,
    lt: f64,
    /// Lot numbers sold (1-based), in order.
    lots: Vec<usize>,
}

/// `harvest` for `portfolio`: valued like `tax in`, so a fund sold in full
/// (no longer held, but with realized gains) still has its AMFI mark and
/// lands in the same Sec. 111A/112A totals. Returns the valuation too, for
/// its price notes.
pub(crate) async fn harvest_summary(
    src: &impl views::DataSource,
    portfolio: &Portfolio,
    market: Market,
    today: chrono::NaiveDate,
) -> Result<(HarvestSummary, Valuation)> {
    harvest_summary_with(src, portfolio, market, today, CarriedForward::default()).await
}

/// [`harvest_summary`] with losses brought forward from earlier years.
pub(crate) async fn harvest_summary_with(
    src: &impl views::DataSource,
    portfolio: &Portfolio,
    market: Market,
    today: chrono::NaiveDate,
    carry: CarriedForward,
) -> Result<(HarvestSummary, Valuation)> {
    let val = tax_valuation(src, portfolio, market).await?;
    let summary = compute_harvest_with(portfolio, &val, market, today, carry);
    Ok((summary, val))
}

/// The latest purchase of `h` in the 30 days up to `today` that a US harvest
/// sale would not sell: an open lot at a gain (the sale sells only the losing
/// lots), or shares bought and already sold on an earlier day. Bought within
/// the wash-sale window, it would make the loss a wash sale.
fn recent_buy(
    portfolio: &Portfolio,
    h: &crate::portfolio::Holding,
    lots: &[(f64, f64, f64, bool)],
    today: chrono::NaiveDate,
) -> Option<String> {
    let parse_date = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    let recent = |d: &str| {
        parse_date(d).is_some_and(|d| d <= today && (today - d).num_days() <= tax::WASH_SALE_DAYS)
    };
    let kept_lots = h
        .lots
        .iter()
        .zip(lots)
        // Bonus shares are allotted, not bought, so they are never replacements.
        .filter(|(l, x)| x.2 >= 0.0 && l.source != crate::portfolio::BONUS_SOURCE)
        .map(|(l, _)| l.date.as_str());
    let sold_earlier = portfolio
        .realized
        .iter()
        .filter(|s| {
            s.symbol == h.symbol && !s.bonus_shares && parse_date(&s.sell_date) != Some(today)
        })
        .map(|s| s.buy_date.as_str());
    kept_lots
        .chain(sold_earlier)
        .filter(|d| recent(d))
        .max()
        .map(str::to_string)
}

pub(crate) fn compute_harvest(
    portfolio: &Portfolio,
    val: &Valuation,
    market: Market,
    today: chrono::NaiveDate,
) -> HarvestSummary {
    compute_harvest_with(portfolio, val, market, today, CarriedForward::default())
}

/// The largest long-term gain that adds no tax on top of `realized`
/// (the exemption left, plus losses that absorb it first).
fn tax_free_room(country: Country, realized: tax::Buckets) -> f64 {
    let base = tax::estimate(country, realized).total();
    let adds = |g: f64| {
        let mut b = realized;
        b.add(g, true);
        tax::estimate(country, b).total() - base > 1e-6
    };
    let mut hi = country.exemption() + realized.st_loss + realized.lt_loss + 1.0;
    if !adds(hi) {
        return hi;
    }
    let mut lo = 0.0;
    if adds(lo + 0.01) {
        return 0.0;
    }
    for _ in 0..80 {
        let mid = (lo + hi) / 2.0;
        if adds(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (lo * 100.0).floor() / 100.0
}

/// [`compute_harvest`] with losses brought forward from earlier years.
pub(crate) fn compute_harvest_with(
    portfolio: &Portfolio,
    val: &Valuation,
    market: Market,
    today: chrono::NaiveDate,
    carry: CarriedForward,
) -> HarvestSummary {
    let country = match market {
        Market::Us => Country::Us,
        _ => Country::India,
    };
    compute_harvest_in(portfolio, val, country, today, carry)
}

/// [`compute_harvest_with`] under `country`'s rules (India or the US), for
/// a caller that knows the investor's residence.
pub(crate) fn compute_harvest_in(
    portfolio: &Portfolio,
    val: &Valuation,
    country: Country,
    today: chrono::NaiveDate,
    carry: CarriedForward,
) -> HarvestSummary {
    let report = tax_report(country, portfolio, val, today);
    let realized = report.realized.plus(carry.buckets());
    let realized_est = tax::estimate(country, realized);
    let exemption_limit = country.exemption();
    let mut gain_room = if exemption_limit > 0.0 {
        tax_free_room(country, realized)
    } else {
        0.0
    };
    let tax_free_gain_room = gain_room;
    // Tax on a gain that the exemption covers, including India's cess.
    let gain_rate = country.lt_rate() / 100.0
        * if country == Country::India {
            1.0 + tax::INDIA_CESS_RATE
        } else {
            1.0
        };

    let parse_date = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    let mut losing: Vec<(HarvestRow, HarvestSale)> = Vec::new();
    let mut gain_candidates = Vec::new();
    let mut fifo_blocked = Vec::new();
    let mut other_rules = Vec::new();
    let mut other_currency = Vec::new();
    for h in &portfolio.holdings {
        let Some(price) = val.price(&h.symbol) else {
            continue;
        };
        if country == Country::India
            && let IndiaRules::Other(reason) = india_rules(&h.symbol, val)
        {
            other_rules.push(OtherRulesRow {
                symbol: h.symbol.clone(),
                reason,
            });
            continue;
        }
        // Holdings in another currency are not counted, as in `tax in`.
        let Some(scale) = val
            .currency(&h.symbol)
            .and_then(|cur| country.currency_scale(&cur))
        else {
            other_currency.push(h.symbol.clone());
            continue;
        };
        // Each lot's gain at today's price, oldest first (the order FIFO sells in).
        let lots: Vec<(f64, f64, f64, bool)> = h
            .lots
            .iter()
            .map(|l| {
                let long = parse_date(&l.date).is_some_and(|d| country.is_long_term(d, today));
                let fmv = if country == Country::India {
                    val.fmv_2018(&h.symbol)
                } else {
                    None
                };
                let unit_cost = tax_export::lot_unit_cost(&l.date, long, l.price, price, fmv);
                (
                    l.qty,
                    l.qty * l.price * scale,
                    l.qty * (price - unit_cost) * scale,
                    long,
                )
            })
            .collect();
        let lt_pnl: f64 = lots.iter().filter(|l| l.3).map(|l| l.2).sum();
        let st_pnl: f64 = lots.iter().filter(|l| !l.3).map(|l| l.2).sum();

        // The sale that harvests the most loss. India sells FIFO, so reaching
        // a losing lot means selling every older lot first; the US allows
        // choosing the lots, so only the losing ones are sold.
        let sale = if country == Country::India {
            let (mut shares, mut cost, mut st, mut lt) = (0.0, 0.0, 0.0, 0.0);
            let mut best: Option<HarvestSale> = None;
            let mut first_loss: Option<(f64, f64, f64)> = None;
            for (n, (qty, lot_cost, gain, long)) in lots.iter().enumerate() {
                shares += qty;
                cost += lot_cost;
                if *long {
                    lt += gain;
                } else {
                    st += gain;
                }
                if *gain < 0.0 && first_loss.is_none() {
                    first_loss = Some((shares, st + lt, -gain));
                }
                if st + lt < best.as_ref().map_or(0.0, |b| b.st + b.lt) {
                    best = Some(HarvestSale {
                        shares,
                        cost,
                        st,
                        lt,
                        lots: (1..=n + 1).collect(),
                    });
                }
            }
            if best.is_none()
                && let Some((shares_to_sell, net_result, lot_loss)) = first_loss
            {
                fifo_blocked.push(FifoBlockedRow {
                    symbol: h.symbol.clone(),
                    shares_to_sell: round(shares_to_sell, 4),
                    net_result: round(net_result, 2),
                    lot_loss: round(lot_loss, 2),
                });
            }
            best
        } else {
            let mut sale = HarvestSale {
                shares: 0.0,
                cost: 0.0,
                st: 0.0,
                lt: 0.0,
                lots: Vec::new(),
            };
            // Only the losing lots: specific identification, recorded with
            // `portfolio sell --lots`.
            for (n, (qty, lot_cost, gain, long)) in
                lots.iter().enumerate().filter(|(_, l)| l.2 < 0.0)
            {
                sale.lots.push(n + 1);
                sale.shares += qty;
                sale.cost += lot_cost;
                if *long {
                    sale.lt += gain;
                } else {
                    sale.st += gain;
                }
            }
            (sale.shares > 0.0).then_some(sale)
        };

        if let Some(sale) = sale {
            let net = sale.st + sale.lt;
            let mut row = HarvestRow {
                symbol: h.symbol.clone(),
                shares: sale.shares,
                avg_cost: round(sale.cost / sale.shares / scale, 2),
                price: round(price, 2),
                pnl: round(net, 2),
                pnl_pct: round(
                    if sale.cost > 0.0 {
                        net / sale.cost * 100.0
                    } else {
                        0.0
                    },
                    2,
                ),
                st_pnl: round(sale.st, 2),
                lt_pnl: round(sale.lt, 2),
                estimated_tax_saved: 0.0,
                harvest_type: "tax_loss",
                recent_buy: None,
                wash_sale_window_end: None,
                lots: None,
                record: Some(format!(
                    "stockwise portfolio sell {} {}",
                    h.symbol, sale.shares
                )),
            };
            if country == Country::Us {
                row.recent_buy = recent_buy(portfolio, h, &lots, today);
                row.wash_sale_window_end = Some(tax::wash_sale_window_end(today).to_string());
                let fifo_prefix = sale.lots.iter().enumerate().all(|(i, n)| *n == i + 1);
                if !fifo_prefix {
                    let list = sale
                        .lots
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    row.record = Some(format!(
                        "stockwise portfolio sell {} {} --lots {}",
                        h.symbol, sale.shares, list
                    ));
                    row.lots = Some(list);
                }
            }
            losing.push((row, sale));
        } else if exemption_limit > 0.0 && lt_pnl > 0.0 {
            // The long-term lots are the oldest, so FIFO reaches them first.
            let pnl = lots.iter().map(|l| l.2).sum::<f64>();
            let pnl_pct = if h.avg_cost > 0.0 {
                ((price / h.avg_cost) - 1.0) * 100.0
            } else {
                0.0
            };
            gain_candidates.push((
                HarvestRow {
                    symbol: h.symbol.clone(),
                    shares: round(h.shares, 4),
                    avg_cost: round(h.avg_cost, 2),
                    price: round(price, 2),
                    pnl: round(pnl, 2),
                    pnl_pct: round(pnl_pct, 2),
                    st_pnl: round(st_pnl, 2),
                    lt_pnl: round(lt_pnl, 2),
                    estimated_tax_saved: 0.0,
                    harvest_type: "ltcg_exemption_step_up",
                    recent_buy: None,
                    wash_sale_window_end: None,
                    lots: None,
                    record: None,
                },
                lots.clone(),
                scale,
                crate::offmarket::is_fund(&h.symbol),
            ));
        }
    }

    // Tax saved is what the estimate drops by when each loss is realized on
    // top of this year's gains, so set-off order and the exemption are
    // respected. Biggest losses first; a loss with nothing to offset saves 0
    // and carries forward.
    losing.sort_by(|a, b| a.0.pnl.total_cmp(&b.0.pnl));
    let mut running = realized;
    let (mut total_st_loss, mut total_lt_loss, mut total_tax_saved) = (0.0, 0.0, 0.0);
    let mut loss_candidates = Vec::new();
    for (mut row, sale) in losing {
        let before = tax::estimate(country, running).total();
        running.add(sale.st, false);
        running.add(sale.lt, true);
        let saved = (before - tax::estimate(country, running).total()).max(0.0);
        row.estimated_tax_saved = round(saved, 2);
        total_st_loss += (-sale.st).max(0.0);
        total_lt_loss += (-sale.lt).max(0.0);
        total_tax_saved += saved;
        loss_candidates.push(row);
    }

    // The tax-free room is shared: each candidate, largest long-term gain
    // first, sells the exact share count (oldest lots first, long-term only)
    // whose gain fits in what is left of it.
    gain_candidates.sort_by(|a, b| b.0.lt_pnl.total_cmp(&a.0.lt_pnl));
    let mut gain_harvest_candidates = Vec::new();
    for (mut row, lots, scale, fractional) in gain_candidates {
        if gain_room <= 0.005 {
            break;
        }
        let (mut shares, mut cost, mut gain) = (0.0, 0.0, 0.0);
        for (qty, lot_cost, lot_gain, long) in &lots {
            if !long || *qty <= 0.0 {
                break;
            }
            let per = lot_gain / qty;
            let take = if per <= 0.0 || gain + lot_gain <= gain_room + 1e-9 {
                *qty
            } else {
                let q = (gain_room - gain) / per;
                if fractional {
                    (q * 1000.0 + 1e-6).floor() / 1000.0
                } else {
                    (q + 1e-9).floor()
                }
            };
            shares += take;
            cost += lot_cost / qty * take;
            gain += per * take;
            if take < *qty {
                break;
            }
        }
        if shares <= 0.0 || gain <= 0.0 {
            continue;
        }
        gain_room -= gain;
        row.shares = round(shares, 4);
        row.avg_cost = round(cost / shares / scale, 2);
        row.pnl = round(gain, 2);
        row.pnl_pct = round(if cost > 0.0 { gain / cost * 100.0 } else { 0.0 }, 2);
        row.st_pnl = 0.0;
        row.lt_pnl = round(gain, 2);
        row.estimated_tax_saved = round(gain * gain_rate, 2);
        row.record = Some(format!(
            "stockwise portfolio sell {} {}",
            row.symbol, row.shares
        ));
        gain_harvest_candidates.push(row);
    }
    let mut notes = Vec::new();
    if country == Country::India
        && (!loss_candidates.is_empty() || !gain_harvest_candidates.is_empty())
    {
        notes.push(
            "Repurchase on the next trading day, not the same day: a same-day sale and \
             purchase of the same shares nets out as an intraday trade (speculative income), \
             so no capital gain or loss is booked."
                .to_string(),
        );
    }
    if country == Country::Us && !loss_candidates.is_empty() {
        notes.push(
            "Losses are sold by lot (specific identification): record each sale with the \
             `--lots` shown, so the ledger matches what the broker reports."
                .to_string(),
        );
    }

    HarvestSummary {
        country: match country {
            Country::India => "in",
            Country::Us => "us",
            Country::Uk => "uk",
        },
        currency: match country {
            Country::India => "INR",
            Country::Us => "USD",
            Country::Uk => "GBP",
        },
        total_st_loss_harvestable: round(total_st_loss, 2),
        total_lt_loss_harvestable: round(total_lt_loss, 2),
        estimated_loss_tax_saved: round(total_tax_saved, 2),
        ltcg_exemption_limit: round(exemption_limit, 2),
        ltcg_exemption_remaining: round(
            (exemption_limit - realized_est.exemption_used).max(0.0),
            2,
        ),
        carried_forward: carry,
        tax_free_gain_room: round(tax_free_gain_room, 2),
        notes,
        loss_candidates,
        gain_harvest_candidates,
        fifo_blocked,
        unpriced: report.unpriced,
        other_rules,
        other_currency,
    }
}

pub async fn cmd_harvest(market: Market, fmt: Format, carry: CarriedForward) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        return match fmt {
            Format::Json => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "loss_candidates": [],
                        "gain_harvest_candidates": [],
                    }))?
                );
                Ok(())
            }
            Format::Csv => output::print_csv::<HarvestRow>(&[]),
            Format::Table => {
                println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
                Ok(())
            }
        };
    }

    let today = chrono::Local::now().date_naive();
    let (summary, val) =
        harvest_summary_with(&views::LiveSource::new(), &portfolio, market, today, carry).await?;

    match fmt {
        Format::Json => return output::print_json(&summary),
        Format::Csv => {
            let mut all = summary.loss_candidates.clone();
            all.extend(summary.gain_harvest_candidates.clone());
            return output::print_csv(&all);
        }
        Format::Table => {}
    }

    let csym = market::currency_symbol(Some(summary.currency));
    print_header("Tax-Loss & LTCG Exemption Harvesting");

    print_section("Loss Harvesting Candidates (STCG / LTCG Offset)");
    if summary.loss_candidates.is_empty() {
        if summary.fifo_blocked.is_empty() {
            println!(
                "  {} No losing positions — nothing to loss-harvest!",
                "✓".green()
            );
        } else {
            println!(
                "  {} No loss can be booked on its own: FIFO sells the older lots first.",
                "!".yellow()
            );
        }
    } else {
        println!(
            "  {:<14} {:>8} {:>10} {:>10} {:>12} {:>12} {:>12} {:>11}",
            "Symbol".bold(),
            "Sell".bold(),
            "Avg Cost".bold(),
            "Price".bold(),
            "ST P&L".bold(),
            "LT P&L".bold(),
            "Total P&L".bold(),
            "Est. Saved".bold()
        );
        println!("  {}", "─".repeat(96).dimmed());

        for r in &summary.loss_candidates {
            println!(
                "  {:<14} {:>8} {:>10} {:>10} {:>12} {:>12} {:>12} {:>11}",
                r.symbol.cyan(),
                format!("{}", round(r.shares, 4)),
                format!("{}{:.2}", csym, r.avg_cost),
                format!("{}{:.2}", csym, r.price),
                signed_tax_money(csym, r.st_pnl),
                signed_tax_money(csym, r.lt_pnl),
                signed_tax_money(csym, r.pnl),
                format!("{}{:.0}", csym, r.estimated_tax_saved).green(),
            );
        }
        if let Some(end) = summary
            .loss_candidates
            .iter()
            .find_map(|r| r.wash_sale_window_end.as_deref())
        {
            for r in summary.loss_candidates.iter() {
                if let Some(bought) = &r.recent_buy {
                    println!(
                        "  {} {}: bought on {}, within 30 days — selling now would make this loss a wash sale.",
                        "!".yellow(),
                        r.symbol.cyan(),
                        bought
                    );
                }
            }
            println!(
                "  {}",
                format!(
                    "Wash-sale window: a buy of the same shares on or before {end} would make a loss sold today a wash sale."
                )
                .dimmed()
            );
        }
        println!(
            "\n  {} Harvestable losses: ST {}{:.0} | LT {}{:.0} → Est. tax offset: {}",
            "→".yellow(),
            csym,
            summary.total_st_loss_harvestable,
            csym,
            summary.total_lt_loss_harvestable,
            format!("{}{:.0}", csym, summary.estimated_loss_tax_saved)
                .green()
                .bold()
        );
    }

    for b in &summary.fifo_blocked {
        println!(
            "  {} {}: its losing lot (-{}{:.0}) comes after older lots, so selling {} shares books {} {}{:.0} in all.",
            "!".yellow(),
            b.symbol.cyan(),
            csym,
            b.lot_loss,
            b.shares_to_sell,
            if b.net_result >= 0.0 {
                "a net gain of"
            } else {
                "a net loss of"
            },
            csym,
            b.net_result.abs()
        );
    }

    if summary.ltcg_exemption_limit > 0.0 {
        print_section("Tax-Gain Harvesting (Section 112A ₹1.25L Annual LTCG Exemption)");
        print_kv(
            "Unused LTCG Exemption",
            &format!(
                "{}{:.0} of {}{:.0} remaining this FY",
                csym, summary.ltcg_exemption_remaining, csym, summary.ltcg_exemption_limit
            )
            .bold()
            .to_string(),
        );
        if (summary.tax_free_gain_room - summary.ltcg_exemption_remaining).abs() >= 1.0 {
            print_kv(
                "Tax-free LTCG room",
                &format!(
                    "{}{:.0} (the exemption plus losses that set off first)",
                    csym, summary.tax_free_gain_room
                ),
            );
        }
        if !summary.gain_harvest_candidates.is_empty() && summary.tax_free_gain_room > 0.0 {
            println!(
                "  {}",
                "Long-term winners you can sell & re-buy to step up cost basis tax-free (exact shares, oldest lots first):".dimmed()
            );
            let rate = Country::India.lt_rate() * (1.0 + tax::INDIA_CESS_RATE);
            for r in &summary.gain_harvest_candidates {
                println!(
                    "  • {:<12} sell {:>8} → LTCG {:>10} (saves up to {}{:.0} at {:.0}% incl. cess)",
                    r.symbol.cyan().bold(),
                    r.shares,
                    format!("+{}{:.0}", csym, r.lt_pnl).green(),
                    csym,
                    r.estimated_tax_saved,
                    rate
                );
            }
        }
    }

    let cf = summary.carried_forward;
    if cf.short_term > 0.0 || cf.long_term > 0.0 {
        println!(
            "\n  {} Counted losses brought forward: short-term {}{:.0}, long-term {}{:.0}",
            "ℹ".cyan(),
            csym,
            cf.short_term,
            csym,
            cf.long_term
        );
    }
    let lot_records: Vec<&str> = summary
        .loss_candidates
        .iter()
        .filter_map(|r| r.record.as_deref().filter(|c| c.contains("--lots")))
        .collect();
    if !lot_records.is_empty() {
        println!(
            "\n  {}",
            "Record each loss sale with its lots, so the ledger matches the broker:".dimmed()
        );
        for c in lot_records {
            println!("    {c}");
        }
    }
    for n in &summary.notes {
        println!("\n  {} {}", "ℹ".cyan(), n.dimmed());
    }
    println!(
        "\n  {}",
        "Consult a tax advisor. Brokerage charges apply."
            .dimmed()
            .italic()
    );
    if !summary.other_rules.is_empty() {
        println!(
            "  {} Not checked — taxed under other rules than listed shares (no ₹1.25 lakh exemption):",
            "!".yellow()
        );
        for o in &summary.other_rules {
            println!("    {} {}", o.symbol.cyan(), o.reason.dimmed());
        }
    }
    if !summary.other_currency.is_empty() {
        println!(
            "  {} Not included (not in {}): {}",
            "!".yellow(),
            summary.currency,
            summary.other_currency.join(", ")
        );
    }
    if !summary.unpriced.is_empty() {
        println!(
            "  {} No live price for {} — not checked.",
            "!".yellow(),
            summary.unpriced.join(", ")
        );
    }
    println!();
    val.print_price_notes();
    Ok(())
}

// ── Tax ──

fn money(csym: &str, amount: f64) -> String {
    let (sign, magnitude) = if amount < 0.0 {
        ("-", -amount)
    } else {
        ("", amount)
    };
    let body = crate::display::group_number(&format!("{magnitude:.0}"), csym == "\u{20B9}");
    format!("{sign}{csym}{body}")
}

fn signed_tax_money(csym: &str, amount: f64) -> String {
    let body = format!(
        "{csym}{}",
        crate::display::group_number(&format!("{:.0}", amount.abs()), csym == "\u{20B9}")
    );
    if amount >= 0.0 {
        format!("+{body}").green().to_string()
    } else {
        format!("-{body}").red().to_string()
    }
}

/// Print the set-off and tax for one set of gains.
fn print_tax_estimate(country: Country, b: &tax::Buckets) {
    let e = tax::estimate(country, *b);
    let c = country.currency_symbol();
    let m = |x: f64| {
        format!(
            "{c}{}",
            crate::display::group_number(&format!("{x:.2}"), c == "\u{20B9}")
        )
    };
    if country.long_term_months().is_some() {
        print_kv(
            "Short-term gains / losses",
            &format!("{} / {}", m(b.st_gain), m(b.st_loss)),
        );
        print_kv(
            "Long-term gains / losses",
            &format!("{} / {}", m(b.lt_gain), m(b.lt_loss)),
        );
        if e.st_loss_against_lt > 0.0 {
            print_kv("ST loss set off vs LTCG", &m(e.st_loss_against_lt));
        }
        if e.lt_loss_against_st > 0.0 {
            print_kv("LT loss set off vs STCG", &m(e.lt_loss_against_st));
        }
        print_kv("Taxable STCG", &m(e.st_taxable));
        if country.exemption() > 0.0 {
            print_kv("LTCG after set-off", &m(e.lt_net));
            print_kv(
                &format!("LTCG exemption ({} a year)", money(c, country.exemption())),
                &format!("-{}", m(e.exemption_used)),
            );
        }
        print_kv("Taxable LTCG", &m(e.lt_taxable));
        print_kv(&format!("STCG tax ({}%)", country.st_rate()), &m(e.st_tax));
        print_kv(&format!("LTCG tax ({}%)", country.lt_rate()), &m(e.lt_tax));
        if e.cess > 0.0 {
            print_kv("Health & education cess (4%)", &m(e.cess));
        }
    } else {
        print_kv(
            "Gains / losses",
            &format!(
                "{} / {}",
                m(b.st_gain + b.lt_gain),
                m(b.st_loss + b.lt_loss)
            ),
        );
        print_kv("Annual exempt amount used", &m(e.exemption_used));
        print_kv("Taxable gain", &m(e.st_taxable));
        print_kv(&format!("CGT ({}%)", country.st_rate()), &m(e.st_tax));
    }
    let total = e.total();
    let total_str = if total > 0.0 {
        m(total).red().bold().to_string()
    } else {
        m(total).green().to_string()
    };
    print_kv("TOTAL TAX", &total_str);
    if e.st_carry + e.lt_carry > 0.0 {
        print_kv(
            "Loss to carry forward",
            &if country.long_term_months().is_some() {
                format!("short-term {}, long-term {}", m(e.st_carry), m(e.lt_carry))
            } else {
                m(e.st_carry)
            },
        );
    }
}

/// One sale this tax year in `tax --json`. `term` is `short` or `long`, or
/// null where the country has no holding-period rule (UK).
#[derive(Debug, Serialize)]
pub(crate) struct TaxSaleRow {
    pub(crate) symbol: String,
    pub(crate) qty: f64,
    pub(crate) bought: String,
    pub(crate) bought_estimated: bool,
    pub(crate) sold: String,
    pub(crate) gain: f64,
    pub(crate) term: Option<&'static str>,
    /// The gain uses the Section 112A grandfathered cost (31 Jan 2018 price).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) grandfathered: bool,
}

/// One open holding in `tax --json/--csv`: what selling it today would
/// realize, split by holding period (every lot uses its own date).
#[derive(Debug, Serialize)]
pub(crate) struct TaxHoldingRow {
    pub(crate) symbol: String,
    pub(crate) cost: f64,
    pub(crate) value: f64,
    pub(crate) short_term_gain: f64,
    pub(crate) long_term_gain: f64,
    pub(crate) estimated_dates: bool,
}

impl CsvRow for TaxHoldingRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "cost",
        "value",
        "short_term_gain",
        "long_term_gain",
        "estimated_dates",
    ];
}

/// A holding or sale `tax in` leaves out because Indian tax treats it
/// differently from listed shares (non-equity funds, SGBs).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct OtherRulesRow {
    pub(crate) symbol: String,
    pub(crate) reason: String,
}

/// The tax estimate's inputs, gathered before anything is printed.
pub(crate) struct TaxReport {
    pub(crate) sales: Vec<TaxSaleRow>,
    pub(crate) holdings: Vec<TaxHoldingRow>,
    pub(crate) realized: tax::Buckets,
    pub(crate) unrealized: tax::Buckets,
    pub(crate) unpriced: Vec<String>,
    pub(crate) other_currency: Vec<String>,
    /// Equity-oriented mutual funds, counted at the listed-share rates.
    pub(crate) equity_funds: Vec<String>,
    /// Left out: taxed under other rules than listed shares.
    pub(crate) other_rules: Vec<OtherRulesRow>,
    /// India: sales bought and sold on one day. Speculative business income
    /// at slab rates, kept out of `realized` (Sec. 111A/112A).
    pub(crate) intraday: Vec<TaxSaleRow>,
    pub(crate) intraday_gain: f64,
}

/// Whether Indian capital-gains tax treats `symbol` like a listed share
/// (Sec. 111A/112A): shares and equity-oriented funds do; debt and other
/// funds, funds of unknown category, SGBs, crypto and gold, silver, liquid,
/// debt and international ETFs don't (the reason says how they are taxed
/// instead). The decision is [`crate::offmarket::india_class`], shared with
/// `tax export in` and `tax advance`.
enum IndiaRules {
    Listed,
    EquityFund,
    Other(String),
}

fn india_rules(symbol: &str, val: &Valuation) -> IndiaRules {
    use crate::offmarket::{IndiaClass, india_class};
    match india_class(symbol, val.mark(symbol)) {
        IndiaClass::Listed => IndiaRules::Listed,
        IndiaClass::EquityFund => IndiaRules::EquityFund,
        IndiaClass::Vda => IndiaRules::Other(
            "Crypto — taxed at a flat 30% under Sec. 115BBH, and losses cannot be set off against \
             other income"
                .into(),
        ),
        IndiaClass::Other(reason) => IndiaRules::Other(reason),
    }
}

/// Gains realized this tax year and what selling every open lot at `val`'s
/// prices would add, in the country's currency. Holdings without a live
/// price or in another currency are named, not counted. For India, funds
/// that are not equity-oriented and SGBs are left out too (other rules).
pub(crate) fn tax_report(
    country: Country,
    portfolio: &Portfolio,
    val: &Valuation,
    today: chrono::NaiveDate,
) -> TaxReport {
    let (year_start, year_end) = country.tax_year(today);
    let date = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    let currency = |sym: &str| val.currency(sym);
    let terms = country.long_term_months().is_some();
    let mut r = TaxReport {
        sales: Vec::new(),
        holdings: Vec::new(),
        realized: tax::Buckets::default(),
        unrealized: tax::Buckets::default(),
        unpriced: Vec::new(),
        other_currency: Vec::new(),
        equity_funds: Vec::new(),
        other_rules: Vec::new(),
        intraday: Vec::new(),
        intraday_gain: 0.0,
    };
    // Sort a symbol into listed-share rules or not (India only); true when
    // it counts.
    let counts = |sym: &str, r: &mut TaxReport| -> bool {
        if country != Country::India {
            return true;
        }
        match india_rules(sym, val) {
            IndiaRules::Listed => true,
            IndiaRules::EquityFund => {
                if !r.equity_funds.iter().any(|s| s == sym) {
                    r.equity_funds.push(sym.to_string());
                }
                true
            }
            IndiaRules::Other(reason) => {
                if !r.other_rules.iter().any(|o| o.symbol == sym) {
                    r.other_rules.push(OtherRulesRow {
                        symbol: sym.to_string(),
                        reason,
                    });
                }
                false
            }
        }
    };

    // Money adds up in minor units (exact); the report gets the totals.
    use crate::money::{from_minor, times, to_minor};
    let mut realized = tax::MinorBuckets::default();
    let mut unrealized = tax::MinorBuckets::default();
    let mut intraday_gain: i64 = 0;
    for s in portfolio
        .realized
        .iter()
        .filter(|s| date(&s.sell_date).is_some_and(|d| d >= year_start && d <= year_end))
    {
        let Some(scale) = currency(&s.symbol).and_then(|cur| country.currency_scale(&cur)) else {
            r.other_currency.push(s.symbol.clone());
            continue;
        };
        if !counts(&s.symbol, &mut r) {
            continue;
        }
        let long = match (date(&s.buy_date), date(&s.sell_date)) {
            (Some(b), Some(d)) => country.is_long_term(b, d),
            _ => false,
        };
        let fmv = if country == Country::India {
            val.fmv_2018(&s.symbol)
        } else {
            None
        };
        let unit_cost =
            tax_export::lot_unit_cost(&s.buy_date, long, s.buy_price, s.sell_price, fmv);
        // Net of the sale's recorded charges where they are deductible, as
        // `Sale::gain` is, on the (possibly grandfathered) cost. An Indian
        // capital gain stays before charges, as in `tax export`: the
        // recorded charges include STT, which Sec. 48 does not allow.
        let charges = if country != Country::India || s.is_intraday() {
            to_minor(s.charges())
        } else {
            0
        };
        let gain = times(
            to_minor(s.qty * s.sell_price) - to_minor(s.qty * unit_cost) - charges,
            scale,
        );
        let row = TaxSaleRow {
            grandfathered: fmv.is_some() && unit_cost != s.buy_price,
            symbol: s.symbol.clone(),
            qty: s.qty,
            bought: s.buy_date.clone(),
            bought_estimated: s.buy_date_estimated,
            sold: s.sell_date.clone(),
            gain: from_minor(gain),
            term: terms.then_some(if long { "long" } else { "short" }),
        };
        // India: a same-day buy and sell is speculative income at slab rates.
        if country == Country::India && s.is_intraday() {
            intraday_gain += gain;
            r.intraday.push(TaxSaleRow {
                term: Some("intraday"),
                ..row
            });
            continue;
        }
        realized.add(gain, long);
        r.sales.push(row);
    }

    for h in &portfolio.holdings {
        let Some(scale) = currency(&h.symbol).and_then(|cur| country.currency_scale(&cur)) else {
            r.other_currency.push(h.symbol.clone());
            continue;
        };
        if !counts(&h.symbol, &mut r) {
            continue;
        }
        // A missing price must not count as a 100% capital loss.
        let Some(price) = val.price(&h.symbol) else {
            r.unpriced.push(h.symbol.clone());
            continue;
        };
        let (mut st, mut lt) = (0i64, 0i64);
        for l in &h.lots {
            let long = date(&l.date).is_some_and(|d| country.is_long_term(d, today));
            let fmv = if country == Country::India {
                val.fmv_2018(&h.symbol)
            } else {
                None
            };
            let unit_cost = tax_export::lot_unit_cost(&l.date, long, l.price, price, fmv);
            let gain = times(to_minor(l.qty * price) - to_minor(l.qty * unit_cost), scale);
            unrealized.add(gain, long);
            if long {
                lt += gain;
            } else {
                st += gain;
            }
        }
        r.holdings.push(TaxHoldingRow {
            symbol: h.symbol.clone(),
            cost: round(h.cost_basis() * scale, 2),
            value: round(h.shares * price * scale, 2),
            short_term_gain: from_minor(st),
            long_term_gain: from_minor(lt),
            estimated_dates: h.has_estimated_dates(),
        });
    }
    r.realized = realized.to_buckets();
    r.unrealized = unrealized.to_buckets();
    r.intraday_gain = from_minor(intraday_gain);
    r.other_currency.sort();
    r.other_currency.dedup();
    r
}

/// An estimate as `tax --json` prints it.
fn estimate_json(country: Country, b: tax::Buckets) -> serde_json::Value {
    let e = tax::estimate(country, b);
    serde_json::json!({
        "short_term_gains": round(b.st_gain, 2),
        "short_term_losses": round(b.st_loss, 2),
        "long_term_gains": round(b.lt_gain, 2),
        "long_term_losses": round(b.lt_loss, 2),
        "taxable_short_term": round(e.st_taxable, 2),
        "taxable_long_term": round(e.lt_taxable, 2),
        "exemption_used": round(e.exemption_used, 2),
        "short_term_tax": round(e.st_tax, 2),
        "long_term_tax": round(e.lt_tax, 2),
        "cess": round(e.cess, 2),
        "total_tax": round(e.total(), 2),
        "loss_carried_forward": round(e.st_carry + e.lt_carry, 2),
    })
}

/// `tax --json`: the report with the estimate for gains realized so far and
/// for selling everything today.
pub(crate) fn tax_json(
    country: Country,
    report: &TaxReport,
    today: chrono::NaiveDate,
) -> serde_json::Value {
    let realized = report.realized;
    let exemption_left = (country == Country::India).then(|| {
        round(
            country.exemption() - tax::estimate(country, realized).exemption_used,
            2,
        )
    });
    serde_json::json!({
        "country": country.name(),
        "tax_year": country.tax_year_label(today),
        "currency": country.tax_currency(),
        "sales": report.sales,
        "holdings": report.holdings,
        "realized": estimate_json(country, realized),
        "if_sold_today": estimate_json(country, realized.plus(report.unrealized)),
        "ltcg_exemption_left": exemption_left,
        "unpriced": report.unpriced,
        "other_currency": report.other_currency,
        "equity_funds": report.equity_funds,
        "other_rules": report.other_rules,
        "intraday_sales": report.intraday,
        "intraday_gain": round(report.intraday_gain, 2),
    })
}

/// Prices for `tax`: the holdings, plus funds sold this year (their AMFI
/// category decides the rules they are taxed under). No network when there
/// is nothing to price.
pub(crate) async fn tax_valuation(
    src: &impl views::DataSource,
    portfolio: &Portfolio,
    market: Market,
) -> Result<Valuation> {
    let mut symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    for s in &portfolio.realized {
        if crate::offmarket::is_fund(&s.symbol) && !symbols.contains(&s.symbol) {
            symbols.push(s.symbol.clone());
        }
    }
    let mut val = if symbols.is_empty() {
        Valuation::new(Vec::new(), &[], market, None)
    } else {
        value_holdings(src, &symbols, market, None).await?
    };
    val.fmv_2018 = grandfathering_prices(portfolio).await;
    Ok(val)
}

/// The 31 Jan 2018 price of every rupee symbol with an old lot that Indian
/// grandfathering could apply to: open lots, and long-term sales of this tax
/// year. Looked up once and cached; none when no lot is that old.
pub(crate) async fn grandfathering_prices(portfolio: &Portfolio) -> Vec<(String, f64)> {
    let rupee = |s: &str| pf::symbol_currency(s) == Some("INR");
    let mut symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .filter(|h| {
            h.lots
                .iter()
                .any(|l| tax_export::is_grandfathered_date(&l.date))
        })
        .map(|h| h.symbol.clone())
        .collect();
    let today = chrono::Local::now().date_naive();
    let start = Country::India.tax_year(today).0;
    symbols.extend(tax_export::grandfathered_symbols(
        &portfolio.realized,
        start,
    ));
    symbols.retain(|s| rupee(s) && !s.to_uppercase().ends_with("-INR"));
    symbols.sort();
    symbols.dedup();
    if symbols.is_empty() {
        return Vec::new();
    }
    super::planning::fmv_31_jan_2018(&symbols).await
}

/// Capital-gains estimate for the current tax year: gains realized by
/// `portfolio sell` (or a tradebook import) this year, plus what selling
/// everything today would add. Every lot uses its own purchase date.
pub async fn cmd_tax(country: &str, market: Market, fmt: Format) -> Result<()> {
    let Some(country) = Country::parse(country) else {
        bail!("Unsupported country '{}'. Use in, us, or uk", country);
    };
    let portfolio = Portfolio::load()?;
    let today = chrono::Local::now().date_naive();
    let (year_start, year_end) = country.tax_year(today);
    let any_sale_this_year = portfolio.realized.iter().any(|s| {
        chrono::NaiveDate::parse_from_str(&s.sell_date, "%Y-%m-%d")
            .is_ok_and(|d| d >= year_start && d <= year_end)
    });
    if portfolio.holdings.is_empty() && !any_sale_this_year && fmt.is_table() {
        println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
        println!("  {}", "No sales are recorded this tax year.".dimmed());
        return Ok(());
    }

    let val = tax_valuation(&views::LiveSource::new(), &portfolio, market).await?;
    let mut report = tax_report(country, &portfolio, &val, today);
    // India: gains outside Sec. 111A/112A that have rules of their own; a
    // net loss among them is set off against the listed-share gains.
    let (other_rows, other_totals) =
        super::tax_season::other_gains(&portfolio, |s| val.mark(s).cloned(), today);
    if country == Country::India {
        report.realized.st_loss += tax::paise::to_rupees(other_totals.st_loss_for_set_off);
        report.realized.lt_loss += tax::paise::to_rupees(other_totals.lt_loss_for_set_off);
    }
    let realized = report.realized;
    let unrealized = report.unrealized;
    match fmt {
        Format::Json => {
            let mut v = tax_json(country, &report, today);
            if country == Country::India {
                v["other_heads"] = serde_json::json!({"sales": other_rows, "totals": other_totals});
            }
            return output::print_json(&v);
        }
        Format::Csv => return output::print_csv(&report.holdings),
        Format::Table => {}
    }

    let c = country.currency_symbol();
    let terms = country.long_term_months().is_some();
    let estimated = report.sales.iter().any(|s| s.bought_estimated)
        || report.holdings.iter().any(|h| h.estimated_dates);

    print_header(&portfolio_title(&format!(
        "Tax Estimate — {} ({})",
        country.name(),
        country.tax_year_label(today)
    )));

    // ── Realized this tax year ──
    print_section(&format!(
        "Realized — sales {} to {}",
        year_start.format("%d %b %Y"),
        year_end.format("%d %b %Y")
    ));
    if report.sales.is_empty() {
        // Other sales this year (intraday, other currencies, debt funds,
        // gold ETFs, crypto) are shown in their own sections.
        let shown_below =
            !report.intraday.is_empty() || (country == Country::India && !other_rows.is_empty());
        let msg = if any_sale_this_year {
            match (country, shown_below) {
                (Country::India, true) => {
                    "No listed-share sales this tax year (Sec. 111A/112A); the year's other rupee sales are listed below."
                }
                (Country::India, false) => {
                    "No listed-share sales this tax year (Sec. 111A/112A); recorded sales are in another currency and are excluded from this report."
                }
                (_, true) => {
                    "No capital-gain sales in this tax currency this tax year; intraday sales are listed below."
                }
                (_, false) => {
                    "No sales in this tax currency this tax year; recorded sales are in another currency and are excluded from this report."
                }
            }
        } else {
            "No sales recorded this tax year. Record one with `stockwise portfolio sell`."
        };
        println!("  {}", msg.dimmed());
    } else {
        println!(
            "  {:<14} {:>9} {:<12} {:<12} {:>12} {:>5}",
            "Symbol".bold(),
            "Qty".bold(),
            "Bought".bold(),
            "Sold".bold(),
            "Gain".bold(),
            if terms { "Term" } else { "" }.bold(),
        );
        for s in &report.sales {
            println!(
                "  {:<14} {:>9.2} {:<12} {:<12} {:>12} {:>5}",
                s.symbol.cyan(),
                s.qty,
                format!("{}{}", s.bought, if s.bought_estimated { "*" } else { "" }),
                s.sold,
                signed_tax_money(c, s.gain),
                match s.term {
                    Some("long") if s.grandfathered => "LT†",
                    Some("long") => "LT",
                    Some(_) => "ST",
                    None => "",
                },
            );
        }
        if report.sales.iter().any(|s| s.grandfathered) {
            println!(
                "  {}",
                "† Bought by 31 Jan 2018: cost raised to the 31 Jan 2018 price where that lowers the gain (Section 112A)."
                    .dimmed()
            );
        }
    }

    if !report.intraday.is_empty() {
        println!(
            "\n  {:<14} {:>9} {:<12} {:>12}  {}",
            "Intraday".bold(),
            "Qty".bold(),
            "Sold".bold(),
            "Gain".bold(),
            "(bought and sold the same day)".dimmed()
        );
        for s in &report.intraday {
            println!(
                "  {:<14} {:>9.2} {:<12} {:>12}",
                s.symbol.cyan(),
                s.qty,
                s.sold,
                signed_tax_money(c, s.gain),
            );
        }
        println!(
            "  {} Intraday net {} is speculative business income taxed at your slab rate (Sec. 43(5)); \
             it is not in the estimates below. Speculative losses can only be set off against speculative gains.",
            "·".dimmed(),
            signed_tax_money(c, report.intraday_gain),
        );
    }

    // ── Unrealized: every open lot at today's price ──
    if !portfolio.holdings.is_empty() {
        print_section("Unrealized — if you sold everything today");
        if terms {
            println!(
                "  {:<14} {:>12} {:>12} {:>12} {:>12}",
                "Symbol".bold(),
                "Cost".bold(),
                "Value".bold(),
                "Short-term".bold(),
                "Long-term".bold(),
            );
        } else {
            println!(
                "  {:<14} {:>12} {:>12} {:>12}",
                "Symbol".bold(),
                "Cost".bold(),
                "Value".bold(),
                "Gain".bold(),
            );
        }
        for h in &report.holdings {
            let mut label = h.symbol.clone();
            if h.estimated_dates {
                label.push('*');
            }
            if terms {
                println!(
                    "  {:<14} {:>12} {:>12} {:>12} {:>12}",
                    label.cyan(),
                    money(c, h.cost),
                    money(c, h.value),
                    signed_tax_money(c, h.short_term_gain),
                    signed_tax_money(c, h.long_term_gain),
                );
            } else {
                println!(
                    "  {:<14} {:>12} {:>12} {:>12}",
                    label.cyan(),
                    money(c, h.cost),
                    money(c, h.value),
                    signed_tax_money(c, h.short_term_gain + h.long_term_gain),
                );
            }
        }
    }

    print_section(&format!(
        "Estimated tax on gains realized in {}",
        country.tax_year_label(today)
    ));
    print_tax_estimate(country, &realized);
    if !portfolio.holdings.is_empty() {
        print_section("If you also sold everything today");
        print_tax_estimate(country, &realized.plus(unrealized));
    }

    if country == Country::India {
        super::tax_season::print_other_gains(&other_rows, &other_totals);
        let used = tax::estimate(country, realized).exemption_used;
        let left = country.exemption() - used;
        if left > 0.0 && unrealized.lt_gain > 0.0 {
            println!(
                "\n  {} {} of this year's {} LTCG exemption is unused — booking up to that much \
                 long-term gain this FY is tax-free.",
                "→".cyan(),
                money(c, left),
                money(c, country.exemption())
            );
        }
    }

    println!();
    let other_currency = &report.other_currency;
    let unpriced = &report.unpriced;
    if !other_currency.is_empty() {
        println!(
            "  {} Not included (not in {}): {}. Other countries' shares follow other rules — in India, \
             foreign shares are long-term only after 24 months, and short-term gains are taxed at your slab rate.",
            "!".yellow(),
            country.tax_currency(),
            other_currency.join(", ")
        );
        // Global exchanges whose own rules StockWise does not have.
        let mut unsupported: Vec<&str> = other_currency
            .iter()
            .filter_map(|s| market::exchange_of(s))
            .filter(|x| x.info().tax.is_none())
            .map(|x| x.info().country)
            .collect();
        unsupported.sort_unstable();
        unsupported.dedup();
        if !unsupported.is_empty() {
            println!(
                "  {} StockWise has no capital-gains rules for {} listings (only India, the US and the UK).",
                "!".yellow(),
                unsupported.join(", ")
            );
        }
    }
    if !unpriced.is_empty() {
        println!(
            "  {} No live price for {} — excluded from this estimate.",
            "!".yellow(),
            unpriced.join(", ")
        );
    }
    if !report.equity_funds.is_empty() {
        println!(
            "  {} Equity-oriented mutual funds ({}) are counted like listed shares (Sec. 111A/112A), \
             going by their AMFI category.",
            "·".dimmed(),
            report.equity_funds.join(", ")
        );
    }
    if !report.other_rules.is_empty() {
        println!(
            "  {} Not included — taxed under other rules than listed shares:",
            "!".yellow()
        );
        for o in &report.other_rules {
            println!("    {} {}", o.symbol.cyan(), o.reason.dimmed());
        }
    }
    if estimated {
        println!("  {}", ESTIMATED_DATE_HINT.dimmed());
    }
    if crate::config::effective_profile().auto_explain {
        print_section("How to Read This Estimate (Passive Profile Guide)");
        println!(
            "  {}",
            "• FIFO (First-In, First-Out): When you sell shares or units, the oldest lot in that account is matched first."
                .dimmed()
        );
        if country == Country::India {
            println!(
                "  {}",
                "• Holding period: Listed shares & equity mutual funds held > 12 months are Long-Term (LTCG at 12.5% above ₹1.25L/FY); ≤ 12 months are Short-Term (STCG at 20%)."
                    .dimmed()
            );
        }
    }
    let caveat = match country {
        Country::India => {
            "ESTIMATE for listed shares with STT paid (Sec. 111A/112A rates from 23 Jul 2024), FIFO lots. \
             Includes the 4% cess; excludes surcharge, brought-forward losses, and bonus-share \
             cost rules. Consult a chartered accountant."
        }
        Country::Us => {
            "ESTIMATE at top federal rates (37% short-term, 20% long-term), FIFO lots. Excludes NIIT, \
             state tax, wash sales and the $3,000 loss deduction. Consult a CPA."
        }
        Country::Uk => {
            "ESTIMATE at the 24% higher rate (18% for basic-rate taxpayers). HMRC uses share pooling \
             (Section 104) and the 30-day rule rather than FIFO. Consult an accountant."
        }
    };
    println!("  {}", caveat.dimmed().italic());
    println!();
    Ok(())
}

// ── Import from brokers ──

/// One holding's dividend figures for `divcal`, in its own currency except
/// `value_base` and `income_base`.
struct DivRow {
    symbol: String,
    currency: Option<String>,
    yield_frac: f64,
    annual_per_share: f64,
    income: f64,
    value_base: f64,
    income_base: f64,
}

/// Value-weighted dividend yield of `rows` (value and income in the base
/// currency): total yearly income over total value. Holdings paying nothing
/// count with a yield of 0.
fn weighted_yield(rows: &[(f64, f64)]) -> Option<f64> {
    let value: f64 = rows.iter().map(|r| r.0).sum();
    let income: f64 = rows.iter().map(|r| r.1).sum();
    (value > 0.0).then(|| income / value)
}

pub async fn cmd_divcal(market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    print_header("Dividend Dashboard");
    if portfolio.holdings.is_empty() {
        println!(
            "  {}",
            "No holdings: add them with `stockwise portfolio add`. `stockwise income upcoming` also covers the watchlist."
                .dimmed()
        );
        return Ok(());
    }
    let client = YahooClient::new().await?;
    // A split not yet applied to the lots: count the shares after it.
    let pending = crate::corp_actions::pending_splits(&client, &portfolio).await;
    pending.print();
    let portfolio = pending.adjusted(&portfolio);
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = value_holdings(&client, &symbols, market, None).await?;
    let base = val.base().to_string();

    print_dividends_received(&portfolio);
    let mut rows: Vec<DivRow> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    for h in &portfolio.holdings {
        let (Some(q), Some(price)) = (val.quote(&h.symbol), val.price(&h.symbol)) else {
            excluded.push(format!("{} (no price)", h.symbol));
            continue;
        };
        let Some(value_base) = val.to_base(&h.symbol, h.shares * price) else {
            excluded.push(format!("{} (no exchange rate)", h.symbol));
            continue;
        };
        let dy = q.dividend_yield_fraction().unwrap_or(0.0);
        let annual_per_share = dy * price;
        rows.push(DivRow {
            symbol: h.symbol.clone(),
            currency: val.currency(&h.symbol),
            yield_frac: dy,
            annual_per_share,
            income: h.shares * annual_per_share,
            value_base,
            income_base: value_base * dy,
        });
    }
    let mut payers: Vec<&DivRow> = rows.iter().filter(|r| r.yield_frac > 0.001).collect();
    payers.sort_by(|a, b| b.yield_frac.total_cmp(&a.yield_frac));

    if payers.is_empty() {
        println!("  {}", "No dividend-paying holdings found.".dimmed());
    } else {
        println!(
            "  {:<14} {:>8} {:>14} {:>16}",
            "Symbol".bold(),
            "Yield".bold(),
            "Annual/Sh".bold(),
            "Income/yr".bold()
        );
        println!("  {}", "─".repeat(56).dimmed());
        for r in &payers {
            let cur = r.currency.as_deref();
            println!(
                "  {:<14} {:>8} {:>14} {:>16}",
                r.symbol.cyan(),
                format!("{:.2}%", r.yield_frac * 100.0).green(),
                format_price(r.annual_per_share, cur),
                format_price(r.income, cur),
            );
        }
    }

    print_section("Portfolio Dividend Summary");
    let weighted = weighted_yield(
        &rows
            .iter()
            .map(|r| (r.value_base, r.income_base))
            .collect::<Vec<_>>(),
    );
    let income_base: f64 = rows.iter().map(|r| r.income_base).sum();
    print_kv(
        "Dividend payers",
        &format!("{}/{}", payers.len(), portfolio.holdings.len()),
    );
    if let Some(y) = weighted {
        print_kv(
            "Yield (value-weighted)",
            &format!("{:.2}% of the priced holdings' value", y * 100.0),
        );
        print_kv(
            &format!("Yearly income ({base})"),
            &format!(
                "{} ({} a month), at trailing yields",
                format_price(income_base, Some(&base)),
                format_price(income_base / 12.0, Some(&base))
            ),
        );
    }
    if !excluded.is_empty() {
        println!("  {} Left out: {}", "!".yellow(), excluded.join(", "));
    }
    if let Some(note) = val.fx_note() {
        println!("  {}", note.dimmed());
    }
    println!();
    Ok(())
}

/// Dividends recorded by `portfolio actions --apply`, per currency: this
/// year (last 12 months) and in total.
fn print_dividends_received(portfolio: &Portfolio) {
    if portfolio.dividends.is_empty() {
        if !portfolio.holdings.is_empty() {
            println!(
                "  {}\n",
                "Record the dividends your holdings paid with `stockwise portfolio actions --apply`."
                    .dimmed()
            );
        }
        return;
    }
    let year_ago = (chrono::Local::now().date_naive() - chrono::Duration::days(365)).to_string();
    let mut by_currency: Vec<(Option<String>, f64, f64, usize)> = Vec::new();
    for d in &portfolio.dividends {
        let cur = d
            .currency
            .clone()
            .or_else(|| pf::symbol_currency(&d.symbol).map(str::to_string));
        let recent = if d.ex_date > year_ago { d.amount } else { 0.0 };
        match by_currency.iter_mut().find(|(c, ..)| *c == cur) {
            Some(row) => {
                row.1 += recent;
                row.2 += d.amount;
                row.3 += 1;
            }
            None => by_currency.push((cur, recent, d.amount, 1)),
        }
    }
    print_section("Dividends Received (recorded)");
    for (cur, recent, total, n) in &by_currency {
        // Label each currency when there is more than one.
        let suffix = match (by_currency.len(), cur) {
            (2.., Some(c)) => format!(" ({})", c),
            _ => String::new(),
        };
        print_kv(
            &format!("Last 12 months{}", suffix),
            &format_price(*recent, cur.as_deref()).green().to_string(),
        );
        print_kv(
            &format!("All recorded{}", suffix),
            &format!(
                "{} from {} payouts",
                format_price(*total, cur.as_deref()),
                n
            ),
        );
    }
    println!();
}

// ── Portfolio Correlation Dashboard ──

/// Value-weighted average correlation between different holdings:
/// Σ wᵢwⱼρᵢⱼ / Σ wᵢwⱼ over the pairs with a correlation (i ≠ j).
fn weighted_avg_correlation(weights: &[f64], corr: &[Vec<Option<f64>>]) -> Option<f64> {
    let (mut num, mut den) = (0.0, 0.0);
    for (i, wi) in weights.iter().enumerate() {
        for (j, wj) in weights.iter().enumerate() {
            if i == j {
                continue;
            }
            if let Some(c) = corr.get(i).and_then(|r| r.get(j)).copied().flatten() {
                num += wi * wj * c;
                den += wi * wj;
            }
        }
    }
    (den > 0.0).then(|| num / den)
}

pub async fn cmd_pcorr(market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.len() < 2 {
        println!("\n  {}", "Need 2+ holdings for correlation.".dimmed());
        return Ok(());
    }

    let client = YahooClient::new().await?;
    let pending = crate::corp_actions::pending_splits(&client, &portfolio).await;
    let portfolio = pending.adjusted(&portfolio);
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = value_holdings(&client, &symbols, market, None).await?;
    let mut all_closes: Vec<(String, Vec<(chrono::NaiveDate, f64)>, f64)> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    for h in &portfolio.holdings {
        let Some(weight) = val
            .price(&h.symbol)
            .and_then(|p| val.to_base(&h.symbol, h.shares * p))
        else {
            excluded.push(format!("{} (no price or exchange rate)", h.symbol));
            continue;
        };
        match client.get_chart(&h.symbol, "6mo", "1d").await {
            Ok(chart) => {
                let closes = chart.dated_closes();
                if closes.len() > 20 {
                    all_closes.push((h.symbol.clone(), closes, weight));
                } else {
                    excluded.push(format!("{} (under 20 days of prices)", h.symbol));
                }
            }
            Err(_) => excluded.push(format!("{} (no price history)", h.symbol)),
        }
    }

    if all_closes.len() < 2 {
        println!("  {}", "Not enough data.".dimmed());
        if !excluded.is_empty() {
            println!("  {} Left out: {}", "!".yellow(), excluded.join(", "));
        }
        return Ok(());
    }

    print_header("Portfolio Correlation (6M)");
    print!("  {:<12}", "");
    for (sym, ..) in &all_closes {
        print!(" {:>10}", sym.cyan());
    }
    println!();
    println!("  {}", "─".repeat(12 + all_closes.len() * 11).dimmed());

    let n = all_closes.len();
    let mut matrix: Vec<Vec<Option<f64>>> = vec![vec![None; n]; n];
    let mut high_corr_pairs = Vec::new();
    for (i, (sym_a, ca, _)) in all_closes.iter().enumerate() {
        print!("  {:<12}", sym_a.cyan());
        for (j, (sym_b, cb, _)) in all_closes.iter().enumerate() {
            if i == j {
                print!(" {:>10}", "1.000".bold());
            } else {
                // Returns over the dates both traded: too few shared dates
                // to compare is N/A, not 0.000.
                let Some(corr) = technical::dated_correlation(ca, cb) else {
                    print!(" {:>10}", "N/A".dimmed());
                    continue;
                };
                matrix[i][j] = Some(corr);
                if corr > 0.7 && i < j {
                    high_corr_pairs.push((sym_a.clone(), sym_b.clone(), corr));
                }
                let c = if corr > 0.7 {
                    format!("{:.3}", corr).red().to_string()
                } else if corr > 0.3 {
                    format!("{:.3}", corr).yellow().to_string()
                } else {
                    format!("{:.3}", corr).to_string()
                };
                print!(" {:>10}", c);
            }
        }
        println!();
    }

    let weights: Vec<f64> = all_closes.iter().map(|c| c.2).collect();
    print_section("Summary");
    if let Some(avg) = weighted_avg_correlation(&weights, &matrix) {
        print_kv(
            "Average correlation (value-weighted)",
            &format!("{:.2}", avg),
        );
    }
    if high_corr_pairs.is_empty() {
        println!("  {} No pair has a correlation above 0.7.", "·".dimmed());
    } else {
        println!("  Pairs with a correlation above 0.7 (they tended to move together):");
        for (a, b, c) in &high_corr_pairs {
            println!("  {} {} + {}: {:.2}", "·".dimmed(), a.cyan(), b.cyan(), c);
        }
    }
    if !excluded.is_empty() {
        println!("  {} Left out: {}", "!".yellow(), excluded.join(", "));
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod divcal_pcorr_tests {
    use super::*;

    #[test]
    fn divcal_yield_is_weighted_by_value() {
        // ₹9,00,000 at 1% and ₹1,00,000 at 5%: 1.4%, not the plain average 3%.
        let y = weighted_yield(&[(900_000.0, 9_000.0), (100_000.0, 5_000.0)]).unwrap();
        assert!((y - 0.014).abs() < 1e-12);
        // A holding paying nothing still counts in the value.
        let y = weighted_yield(&[(500_000.0, 10_000.0), (500_000.0, 0.0)]).unwrap();
        assert!((y - 0.01).abs() < 1e-12);
        assert_eq!(weighted_yield(&[]), None);
    }

    #[test]
    fn pcorr_average_is_weighted_and_skips_missing_pairs() {
        let m = vec![
            vec![None, Some(0.9), Some(0.1)],
            vec![Some(0.9), None, None],
            vec![Some(0.1), None, None],
        ];
        // Weights 0.8 / 0.1 / 0.1: (0.8·0.1·0.9 + 0.8·0.1·0.1)·2 / (0.08+0.08)·2 = 0.5.
        let avg = weighted_avg_correlation(&[0.8, 0.1, 0.1], &m).unwrap();
        assert!((avg - 0.5).abs() < 1e-12);
        // Weights tilted to the first pair pull it up.
        let avg = weighted_avg_correlation(&[0.5, 0.45, 0.05], &m).unwrap();
        assert!(avg > 0.8);
        assert_eq!(
            weighted_avg_correlation(&[1.0, 1.0], &[vec![None; 2], vec![None; 2]]),
            None
        );
    }

    #[test]
    fn account_names_are_checked_and_lower_cased() {
        assert_eq!(account_name(None).unwrap(), None);
        assert_eq!(
            account_name(Some(" Spouse ")).unwrap().as_deref(),
            Some("spouse")
        );
        assert_eq!(
            account_name(Some("huf-2")).unwrap().as_deref(),
            Some("huf-2")
        );
        assert!(account_name(Some("")).is_err());
        assert!(account_name(Some("my account")).is_err());
        assert!(account_name(Some("a:b")).is_err());
    }
}
