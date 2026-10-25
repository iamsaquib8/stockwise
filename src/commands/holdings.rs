//! Your data: watchlist, alerts, import/export, wealth history, rebalancing, tax, and dividends.

use super::portfolio::{ESTIMATED_DATE_HINT, Valuation, portfolio_title, value_holdings};
use super::*;
use crate::portfolio::{self as pf, ImportOptions, ImportOutcome, ImportRow, SyncChange};
use crate::tax::{self, Country};

pub async fn cmd_watchlist(action: &str, symbols: &[String], market: Market) -> Result<()> {
    let mut wl = Watchlist::load()?;

    match action {
        "add" => {
            for sym in symbols {
                let resolved = market::resolve_symbol(sym, market);
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
    market: Market,
) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let Some(cond) = AlertCondition::parse(condition) else {
        bail!(
            "Unknown alert condition '{}'. Use one of: {}",
            condition,
            AlertCondition::ALL.map(|c| c.name()).join(", ")
        );
    };
    let (target, period) = crate::alerts::validate(cond, target, period)?;
    let mut store = AlertStore::load()?;
    store.add_with_period(&resolved, cond, target, period);
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
            "  {} Checked on daily bars by `stockwise alert check` and `alert watch`.",
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

pub async fn cmd_export(what: &str, _market: Market) -> Result<()> {
    match what {
        "portfolio" => {
            let portfolio = Portfolio::load()?;
            if portfolio.holdings.is_empty() {
                println!("  {}", "Portfolio is empty.".dimmed());
                return Ok(());
            }
            let client = YahooClient::new().await?;
            let symbols: Vec<String> = portfolio
                .holdings
                .iter()
                .map(|h| h.symbol.clone())
                .collect();
            let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
            let quotes = client.get_quote(&sym_refs).await?;
            let path = "portfolio_export.csv";
            let mut csv = String::from("Symbol,Shares,Avg Cost,Current Price,Value,P&L,P&L %\n");
            for h in &portfolio.holdings {
                let price = quotes
                    .iter()
                    .find(|q| q.symbol.as_deref() == Some(&h.symbol))
                    .and_then(|q| q.regular_market_price)
                    .unwrap_or(0.0);
                let value = h.shares * price;
                let cost = h.shares * h.avg_cost;
                let pnl = value - cost;
                let pnl_pct = if cost > 0.0 {
                    (pnl / cost) * 100.0
                } else {
                    0.0
                };
                csv.push_str(&format!(
                    "{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}%\n",
                    h.symbol, h.shares, h.avg_cost, price, value, pnl, pnl_pct
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
            let quotes = client.get_quote(&sym_refs).await?;
            let path = "watchlist_export.csv";
            let mut csv = String::from("Symbol,Price,Change,Change %,Volume,Market Cap\n");
            for q in &quotes {
                csv.push_str(&format!(
                    "{},{:.2},{:.2},{:.2}%,{},{:.0}\n",
                    q.symbol.as_deref().unwrap_or(""),
                    q.regular_market_price.unwrap_or(0.0),
                    q.regular_market_change.unwrap_or(0.0),
                    q.regular_market_change_percent.unwrap_or(0.0),
                    q.regular_market_volume.unwrap_or(0),
                    q.market_cap.unwrap_or(0.0)
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

pub async fn cmd_import(source: &str, file: &str, replace: bool, market: Market) -> Result<()> {
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
                    "    2. Click {} to get the holdings statement",
                    "Download".bold()
                );
                println!("    3. Open the .xlsx file and save it as CSV");
                println!("    4. Run: stockwise import groww <path-to-file.csv>");
            }
            "upstox" => {
                println!(
                    "    1. Open {} → Portfolio → Holdings",
                    "pro.upstox.com".cyan()
                );
                println!("    2. Click {} and save the CSV file", "Download".bold());
                println!("    3. Run: stockwise import upstox <path-to-file.csv>");
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

    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", file))?;
    match source {
        "groww" => {
            return super::imports::import_rows(
                "groww",
                file,
                replace,
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
                super::imports::parse_upstox(&content, Market::In)?,
            )
            .await;
        }
        "cas" => {
            return super::imports::import_rows(
                "cas",
                file,
                replace,
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

    let header = lines[0].to_lowercase();
    let cols = parse_csv_header(&header);
    // Column names per format, most specific first (matched as substrings).
    let (sym_names, qty_names, price_names, expected): (&[&str], &[&str], &[&str], &str) =
        match source {
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
                "Kite CSV columns. Expected: Instrument, Qty., Avg. cost",
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
                "IndMoney CSV columns. Expected: Symbol/Stock Name, Quantity, Avg Buy Price",
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
                "CSV columns. Your CSV needs Symbol, Quantity and Average Price columns",
            ),
        };
    let (Some(si), Some(qi), Some(pi)) = (
        find_col(&cols, sym_names),
        find_col(&cols, qty_names),
        find_col(&cols, price_names),
    ) else {
        bail!("Could not detect {}. Found: {}", expected, header);
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
    for line in &lines[1..] {
        let fields = parse_csv_row(line);
        if fields.len() <= si.max(qi).max(pi) {
            continue;
        }
        let symbol_raw = field(&fields, si);
        if symbol_raw.is_empty() {
            continue;
        }
        let qty = number(field(&fields, qi));
        let price = number(field(&fields, pi));
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
        rows.push(ImportRow {
            symbol: market::resolve_symbol(&symbol_raw, market),
            qty,
            price,
            date,
            sell,
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

    apply_import_rows(source, file, rows, side_col.is_some(), replace).await
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
) -> Result<()> {
    let mut portfolio = Portfolio::load()?;
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
        for sym in symbols.iter().filter(|s| dated(s)) {
            let since = rows
                .iter()
                .filter(|r| &r.symbol == sym)
                .filter_map(|r| r.date.clone())
                .min()
                .unwrap_or_else(pf::today);
            let (s, _, _) = super::portfolio::fetch_actions(&client, sym, &since)
                .await
                .with_context(|| {
                    format!(
                        "Could not check {} for splits, which a tradebook needs; nothing was changed. Try again when online",
                        sym
                    )
                })?;
            splits.push((sym.clone(), s));
        }
    }

    // A holdings snapshot with more shares than StockWise has may just be a
    // split not yet applied to your lots: leave those for `portfolio actions`.
    let snapshot: Vec<(String, f64)> = symbols
        .iter()
        .filter(|s| !dated(s))
        .map(|s| {
            let qty = rows.iter().filter(|r| &r.symbol == s).map(|r| r.qty).sum();
            (s.clone(), qty)
        })
        .collect();
    let blocked = super::portfolio::blocked_by_pending_splits(&portfolio, &snapshot).await;
    let rows: Vec<ImportRow> = rows
        .into_iter()
        .filter(|r| !blocked.iter().any(|(s, _)| *s == r.symbol))
        .collect();

    let outcomes = portfolio.apply_import(
        &rows,
        ImportOptions {
            source: &format!("import:{}", source),
            replace,
            tradebook,
            splits: &splits,
        },
    );
    let updated = outcomes
        .iter()
        .filter(|(_, o)| {
            !matches!(
                o,
                ImportOutcome::Unmatched { .. } | ImportOutcome::WouldReplace
            )
        })
        .count();
    if updated > 0 {
        portfolio.save()?;
    }

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
                "  {} {} — skipped: sells of {} shares have no earlier buy in this file. Import a \
                 tradebook that covers every buy, or record the sale with `stockwise portfolio sell`.",
                "!".yellow(),
                sym,
                unmatched
            ),
            ImportOutcome::WouldReplace => println!(
                "  {} {} — skipped: it has lots or sales from elsewhere (manual entries, another \
                 import) that this file would replace. Re-run with --replace to rebuild it from the file.",
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
    let mut history = WealthHistory::load()?;

    if portfolio.holdings.is_empty() {
        println!("\n  {}", "Portfolio is empty. Add holdings first.".dimmed());
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
    history.add_snapshot(total_value, total_cost, portfolio.holdings.len());
    history.save()?;

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

// ── Rebalance ──

pub async fn cmd_rebalance(targets: &[(String, f64)], market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        println!("\n  {}", "Portfolio is empty.".dimmed());
        return Ok(());
    }

    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let client = YahooClient::new().await?;
    let val = value_holdings(&client, &symbols, market, None).await?;

    // Weights only make sense in one currency: value everything in the base.
    let mut total_value = 0.0;
    let mut holdings_value: Vec<(String, f64, f64)> = Vec::new(); // symbol, value, price (base currency)
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
        let Some(price) = val.to_base(&h.symbol, price) else {
            bail!(
                "No exchange rate for {} — can't weigh it against your other holdings",
                h.symbol
            );
        };
        let value = h.shares * price;
        total_value += value;
        holdings_value.push((h.symbol.clone(), value, price));
    }

    let cur = Some(val.base());
    let csym = market::currency_symbol(cur);

    print_header("Portfolio Rebalance");
    print_kv("Total Value", &format!("{}{:.2}", csym, total_value));

    // If no targets given, show equal-weight rebalance
    let target_pct = if targets.is_empty() {
        let equal = 100.0 / holdings_value.len() as f64;
        holdings_value
            .iter()
            .map(|(s, _, _)| (s.clone(), equal))
            .collect::<Vec<_>>()
    } else {
        targets.to_vec()
    };

    // Wide enough for `MF:<ISIN>`.
    let sw = holdings_value
        .iter()
        .map(|(s, _, _)| s.len() + 1)
        .max()
        .unwrap_or(0)
        .max(14);
    println!();
    println!(
        "  {:<sw$} {:>10} {:>8} {:>8} {:>12}",
        "Symbol".bold(),
        "Current".bold(),
        "Now%".bold(),
        "Target%".bold(),
        "Action".bold()
    );
    println!("  {}", "─".repeat(42 + sw).dimmed());

    for (sym, value, price) in &holdings_value {
        let current_pct = (value / total_value) * 100.0;
        let tgt = target_pct
            .iter()
            .find(|(s, _)| s == sym)
            .map(|(_, p)| *p)
            .unwrap_or(100.0 / holdings_value.len() as f64);
        let target_value = total_value * (tgt / 100.0);
        let diff = target_value - value;
        let diff_shares = if *price > 0.0 { diff / price } else { 0.0 };

        // Fund units are bought and redeemed in fractions; shares whole.
        let units = crate::offmarket::unit_word(sym);
        let qty = |q: f64| {
            if crate::offmarket::is_fund(sym) {
                format!("{q:.3}")
            } else {
                format!("{q:.0}")
            }
        };
        let action = if diff.abs() < 1.0 {
            "OK".green().to_string()
        } else if diff > 0.0 {
            let q = if crate::offmarket::is_fund(sym) {
                diff_shares
            } else {
                diff_shares.ceil()
            };
            format!("BUY {} {}", qty(q), units).green().to_string()
        } else {
            let q = if crate::offmarket::is_fund(sym) {
                diff_shares.abs()
            } else {
                diff_shares.abs().floor()
            };
            format!("SELL {} {}", qty(q), units).red().to_string()
        };

        println!(
            "  {:<sw$} {:>10} {:>8} {:>8} {:>12}",
            sym.cyan(),
            format!("{}{:.2}", csym, value),
            format!("{:.1}%", current_pct),
            format!("{:.1}%", tgt),
            action
        );
    }
    println!();
    val.print_price_notes();
    if !val.off.marks.is_empty() {
        println!();
    }
    Ok(())
}

// ── Earnings Calendar ──

pub async fn cmd_harvest(market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        println!("\n  {}", "Portfolio is empty.".dimmed());
        return Ok(());
    }

    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let client = YahooClient::new().await?;
    let val = value_holdings(&client, &symbols, market, None).await?;

    print_header("Tax-Loss Harvesting Opportunities");
    let mut found = false;

    println!(
        "  {:<14} {:>10} {:>10} {:>12} {:>10}",
        "Symbol".bold(),
        "Avg Cost".bold(),
        "Price".bold(),
        "Loss".bold(),
        "Loss %".bold()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for h in &portfolio.holdings {
        // Never suggest selling something just because its price is missing.
        let Some(price) = val.price(&h.symbol) else {
            continue;
        };
        let cur = val.currency(&h.symbol);
        let csym = market::currency_symbol(cur.as_deref());
        let pnl = (price - h.avg_cost) * h.shares;
        let pnl_pct = if h.avg_cost > 0.0 {
            ((price / h.avg_cost) - 1.0) * 100.0
        } else {
            0.0
        };

        if pnl < 0.0 {
            found = true;
            println!(
                "  {:<14} {:>10} {:>10} {:>12} {:>10}",
                h.symbol.cyan(),
                format!("{}{:.2}", csym, h.avg_cost),
                format!("{}{:.2}", csym, price),
                format!("-{}{:.2}", csym, pnl.abs()).red(),
                format!("{:.2}%", pnl_pct).red(),
            );
        }
    }

    if !found {
        println!(
            "\n  {} No losing positions — nothing to harvest!",
            "✓".green()
        );
    } else {
        println!(
            "\n  {} Sell these positions to realize losses and offset capital gains.",
            "→".yellow()
        );
        println!(
            "  {}",
            "Consult a tax advisor. Wash sale rules may apply."
                .dimmed()
                .italic()
        );
    }
    let unpriced: Vec<&str> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.as_str())
        .filter(|s| val.price(s).is_none())
        .collect();
    if !unpriced.is_empty() {
        println!(
            "  {} No live price for {} — not checked.",
            "!".yellow(),
            unpriced.join(", ")
        );
    }
    println!();
    val.print_price_notes();
    Ok(())
}

// ── Tax ──

fn money(csym: &str, amount: f64) -> String {
    if amount < 0.0 {
        format!("-{}{:.0}", csym, -amount)
    } else {
        format!("{}{:.0}", csym, amount)
    }
}

fn signed_tax_money(csym: &str, amount: f64) -> String {
    if amount >= 0.0 {
        format!("+{}{:.0}", csym, amount).green().to_string()
    } else {
        format!("-{}{:.0}", csym, -amount).red().to_string()
    }
}

/// Print the set-off and tax for one set of gains.
fn print_tax_estimate(country: Country, b: &tax::Buckets) {
    let e = tax::estimate(country, *b);
    let c = country.currency_symbol();
    let m = |x: f64| format!("{}{:.2}", c, x);
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
}

/// Whether Indian capital-gains tax treats `symbol` like a listed share
/// (Sec. 111A/112A): shares and equity-oriented funds do; debt and other
/// funds, funds of unknown category and SGBs don't (the reason says how
/// they are taxed instead).
enum IndiaRules {
    Listed,
    EquityFund,
    Other(String),
}

fn india_rules(symbol: &str, val: &Valuation) -> IndiaRules {
    use crate::offmarket;
    if offmarket::is_fund(symbol) {
        let mark = val.mark(symbol);
        return match mark.and_then(|m| m.category.as_deref()) {
            Some(cat)
                if offmarket::is_equity_oriented(
                    cat,
                    mark.and_then(|m| m.name.as_deref()).unwrap_or(""),
                ) =>
            {
                IndiaRules::EquityFund
            }
            Some(cat) => IndiaRules::Other(format!(
                "{cat} — not an equity-oriented fund: units bought from 1 Apr 2023 are taxed at \
                 your slab rate, older units at 12.5% after 24 months"
            )),
            None => IndiaRules::Other(
                "fund not found in AMFI's NAV file, so whether it is equity-oriented is unknown"
                    .into(),
            ),
        };
    }
    if offmarket::is_sgb(symbol) {
        return IndiaRules::Other(
            "Sovereign Gold Bond — tax-free when RBI redeems it; sold on the exchange, 12.5% \
             after 12 months (no ₹1.25 lakh exemption), else your slab rate"
                .into(),
        );
    }
    IndiaRules::Listed
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
        let gain = s.gain() * scale;
        r.realized.add(gain, long);
        r.sales.push(TaxSaleRow {
            symbol: s.symbol.clone(),
            qty: s.qty,
            bought: s.buy_date.clone(),
            bought_estimated: s.buy_date_estimated,
            sold: s.sell_date.clone(),
            gain: round(gain, 2),
            term: terms.then_some(if long { "long" } else { "short" }),
        });
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
        let (mut st, mut lt) = (0.0, 0.0);
        for l in &h.lots {
            let gain = l.qty * (price - l.price) * scale;
            let long = date(&l.date).is_some_and(|d| country.is_long_term(d, today));
            r.unrealized.add(gain, long);
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
            short_term_gain: round(st, 2),
            long_term_gain: round(lt, 2),
            estimated_dates: h.has_estimated_dates(),
        });
    }
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
        "total_tax": round(e.total(), 2),
        "loss_carried_forward": round(e.st_carry + e.lt_carry, 2),
    })
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
        println!(
            "\n  {}",
            "Portfolio is empty and no sales are recorded this tax year.".dimmed()
        );
        return Ok(());
    }

    // Holdings, plus funds sold this year: their AMFI category decides the
    // rules they are taxed under.
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
    let val = if symbols.is_empty() {
        Valuation::new(Vec::new(), &[], market, None)
    } else {
        value_holdings(&YahooClient::new().await?, &symbols, market, None).await?
    };
    let report = tax_report(country, &portfolio, &val, today);
    let realized = report.realized;
    let unrealized = report.unrealized;
    match fmt {
        Format::Json => {
            let exemption_left = (country == Country::India).then(|| {
                round(
                    country.exemption() - tax::estimate(country, realized).exemption_used,
                    2,
                )
            });
            return output::print_json(&serde_json::json!({
                "country": country.name(),
                "tax_year": country.tax_year_label(today),
                "currency": country.tax_currency(),
                "sales": report.sales,
                "holdings": report.holdings,
                "realized": estimate_json(country, realized),
                "if_sold_today": estimate_json(country, realized.plus(unrealized)),
                "ltcg_exemption_left": exemption_left,
                "unpriced": report.unpriced,
                "other_currency": report.other_currency,
                "equity_funds": report.equity_funds,
                "other_rules": report.other_rules,
            }));
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
        println!(
            "  {}",
            "No sales recorded this tax year. Record one with `stockwise portfolio sell`.".dimmed()
        );
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
                    Some("long") => "LT",
                    Some(_) => "ST",
                    None => "",
                },
            );
        }
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
    let caveat = match country {
        Country::India => {
            "ESTIMATE for listed shares with STT paid (Sec. 111A/112A rates from 23 Jul 2024), FIFO lots. \
             Excludes cess and surcharge, pre-2018 grandfathering, brought-forward losses, and bonus-share \
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

pub async fn cmd_divcal(market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let portfolio = Portfolio::load()?;
    let wl = Watchlist::load()?;
    let mut all: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    for s in &wl.symbols {
        if !all.contains(s) {
            all.push(s.clone());
        }
    }
    if all.is_empty() {
        all = market.universe();
    }

    let refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&refs).await?;

    print_header("Dividend Dashboard");
    print_dividends_received(&portfolio);
    let mut payers = Vec::new();
    for q in &quotes {
        let dy = q.trailing_annual_dividend_yield.unwrap_or(0.0);
        if dy > 0.001 {
            payers.push((
                q.symbol.as_deref().unwrap_or("???").to_string(),
                dy,
                q.regular_market_price.unwrap_or(0.0),
                q.currency.clone(),
            ));
        }
    }
    payers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    if payers.is_empty() {
        println!("  {}", "No dividend-paying stocks found.".dimmed());
        return Ok(());
    }

    let csym = market.currency_symbol();
    println!(
        "  {:<14} {:>8} {:>12} {:>14}",
        "Symbol".bold(),
        "Yield".bold(),
        "Annual/Sh".bold(),
        "Income/₹1L".bold()
    );
    println!("  {}", "─".repeat(52).dimmed());

    let mut total_yield_weighted = 0.0;
    for (sym, dy, price, _cur) in &payers {
        let annual = dy * price;
        let income_1l = 100000.0 * dy;
        println!(
            "  {:<14} {:>8} {:>12} {:>14}",
            sym.cyan(),
            format!("{:.2}%", dy * 100.0).green(),
            format!("{}{:.2}", csym, annual),
            format!("{}{:.0}/yr", csym, income_1l)
        );
        total_yield_weighted += dy;
    }

    print_section("Portfolio Dividend Summary");
    let avg_yield = if !payers.is_empty() {
        total_yield_weighted / payers.len() as f64
    } else {
        0.0
    };
    print_kv(
        "Dividend Payers",
        &format!("{}/{}", payers.len(), all.len()),
    );
    print_kv("Avg Yield", &format!("{:.2}%", avg_yield * 100.0));
    print_kv(
        "Income on ₹10L",
        &format!(
            "{}{:.0}/year ({}{:.0}/month)",
            csym,
            1000000.0 * avg_yield,
            csym,
            1000000.0 * avg_yield / 12.0
        ),
    );
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

pub async fn cmd_pcorr(_market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.len() < 2 {
        println!("\n  {}", "Need 2+ holdings for correlation.".dimmed());
        return Ok(());
    }

    let client = YahooClient::new().await?;
    let mut all_closes: Vec<(String, Vec<f64>)> = Vec::new();
    for h in &portfolio.holdings {
        if let Ok(chart) = client.get_chart(&h.symbol, "6mo", "1d").await {
            let closes: Vec<f64> = chart
                .indicators
                .quote
                .first()
                .and_then(|q| q.close.as_ref())
                .map(|c| c.iter().filter_map(|v| *v).collect())
                .unwrap_or_default();
            if closes.len() > 20 {
                all_closes.push((h.symbol.clone(), closes));
            }
        }
    }

    if all_closes.len() < 2 {
        println!("  {}", "Not enough data.".dimmed());
        return Ok(());
    }

    print_header("Portfolio Correlation (6M)");
    print!("  {:<12}", "");
    for (sym, _) in &all_closes {
        print!(" {:>10}", sym.cyan());
    }
    println!();
    println!("  {}", "─".repeat(12 + all_closes.len() * 11).dimmed());

    let mut high_corr_pairs = Vec::new();
    for (i, (sym_a, ca)) in all_closes.iter().enumerate() {
        print!("  {:<12}", sym_a.cyan());
        for (j, (sym_b, cb)) in all_closes.iter().enumerate() {
            if i == j {
                print!(" {:>10}", "1.000".bold());
            } else {
                let corr = technical::correlation(ca, cb).unwrap_or(0.0);
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

    if !high_corr_pairs.is_empty() {
        print_section("Diversification Warning");
        for (a, b, c) in &high_corr_pairs {
            println!(
                "  {} {} + {} correlation {:.2} — consider reducing one",
                "⚠".yellow(),
                a.cyan(),
                b.cyan(),
                c
            );
        }
    } else {
        println!("\n  {} Portfolio is well-diversified.", "✓".green());
    }
    println!();
    Ok(())
}

// ── Returns Calculator ──
