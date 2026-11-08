//! `income` — 12-month trailing and projected dividend, coupon and passive-income
//! calendar, Yield-on-Cost (YoC), and dividend payout safety checks.

use super::*;
use crate::calendar::EventKind;
use crate::income::*;
use chrono::{Local, NaiveDate};

/// CSV row for upcoming income/dividend events.
#[derive(Debug, Clone, Serialize)]
pub struct UpcomingIncomeCsvRow {
    pub date: String,
    pub symbol: String,
    pub when: String,
    pub event: String,
    pub held_shares: f64,
    pub detail: String,
}

impl CsvRow for UpcomingIncomeCsvRow {
    const COLUMNS: &'static [&'static str] =
        &["date", "symbol", "when", "event", "held_shares", "detail"];
}

/// Monthly dividend, coupon and passive-income calendar.
pub async fn cmd_income(months: u32, by_holding: bool, market: Market, fmt: Format) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        return match fmt {
            Format::Json => output::print_json(&serde_json::json!({
                "message": "No holdings found in portfolio",
                "holdings": [],
                "monthly_schedule": []
            })),
            Format::Csv => output::print_csv(&Vec::<IncomeHoldingCsvRow>::new()),
            Format::Table => {
                println!();
                println!("  {} No holdings found in your portfolio.", "i".cyan());
                println!(
                    "    Add holdings with `stockwise portfolio add <SYMBOL> <SHARES> <PRICE>`"
                );
                println!("    or import from your broker with `stockwise import <broker> <file>`.");
                println!();
                Ok(())
            }
        };
    }

    let client = YahooClient::new().await?;
    let raw_symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let resolved = resolve_symbols(&raw_symbols, market);
    let sym_refs: Vec<&str> = resolved.iter().map(String::as_str).collect();

    let quotes = client.get_quote(&sym_refs).await.unwrap_or_default();
    let today = Local::now().date_naive();

    // Determine portfolio base currency (INR for .NS/.BO, USD default for US)
    let base_currency = if resolved
        .iter()
        .any(|s| s.ends_with(".NS") || s.ends_with(".BO"))
    {
        "INR"
    } else {
        "USD"
    };

    // Prepare holding data
    let mut holdings_data = Vec::with_capacity(portfolio.holdings.len());

    for (idx, holding) in portfolio.holdings.iter().enumerate() {
        let res_sym = resolved
            .get(idx)
            .map(String::as_str)
            .unwrap_or(&holding.symbol);
        let q_opt = quotes.iter().find(|q| {
            q.symbol.as_deref().is_some_and(|s| {
                s.eq_ignore_ascii_case(res_sym) || s.eq_ignore_ascii_case(&holding.symbol)
            })
        });

        let current_price = q_opt
            .and_then(|q| q.regular_market_price)
            .filter(|p| *p > 0.0)
            .unwrap_or(holding.avg_cost);

        let div_yield_frac = q_opt
            .and_then(|q| q.dividend_yield_fraction())
            .unwrap_or(0.0);
        let annual_div_rate = q_opt
            .and_then(|q| q.trailing_annual_dividend_rate)
            .filter(|r| *r > 0.0)
            .unwrap_or(current_price * div_yield_frac);

        // Compute payout ratio from dividend per share and trailing EPS
        let payout_ratio =
            q_opt.and_then(|q| match (q.eps_trailing_twelve_months, annual_div_rate) {
                (Some(eps), div) if eps > 0.0 && div > 0.0 => Some(div / eps),
                (Some(eps), div) if eps <= 0.0 && div > 0.0 => Some(2.0), // Loss-making with dividend
                _ => None,
            });

        holdings_data.push(HoldingIncomeData {
            holding,
            current_price,
            annual_dividend_per_share: annual_div_rate,
            payout_ratio,
            fcf: None,
            currency: q_opt
                .and_then(|q| q.currency.as_deref())
                .or(Some(base_currency)),
        });
    }

    let report = generate_income_report(&portfolio, &holdings_data, base_currency, today, months);

    match fmt {
        Format::Json => output::print_json(&report),
        Format::Csv => {
            if by_holding {
                let rows: Vec<IncomeHoldingCsvRow> = report
                    .holdings
                    .iter()
                    .map(|h| IncomeHoldingCsvRow {
                        symbol: h.symbol.clone(),
                        shares: h.shares,
                        current_price: h.current_price,
                        avg_cost: h.avg_cost,
                        annual_dividend_per_share: h.annual_dividend_per_share,
                        annual_gross_income: h.annual_gross_income,
                        current_yield_pct: h.current_yield_pct,
                        yoc_pct: h.yoc_pct,
                        annual_wht: h.annual_wht_amount,
                        annual_net_income: h.annual_net_income,
                        payout_ratio_pct: h
                            .payout_ratio_pct
                            .map_or("N/A".to_string(), |p| format!("{:.1}%", p)),
                        safety_badge: h.payout_safety.badge().to_string(),
                    })
                    .collect();
                output::print_csv(&rows)
            } else {
                let rows: Vec<MonthlyIncomeCsvRow> = report
                    .monthly_schedule
                    .iter()
                    .map(|m| MonthlyIncomeCsvRow {
                        month: m.month.clone(),
                        label: m.label.clone(),
                        is_projected: m.is_projected,
                        gross_amount: m.gross_amount,
                        wht_amount: m.wht_amount,
                        net_amount: m.net_amount,
                    })
                    .collect();
                output::print_csv(&rows)
            }
        }
        Format::Table => {
            print_income_table(&report, by_holding);
            Ok(())
        }
    }
}

/// Upcoming ex-dividend and payment dates radar for held and watchlisted stocks.
pub async fn cmd_income_upcoming(days: u32, market: Market, fmt: Format) -> Result<()> {
    let portfolio = Portfolio::load()?;
    let watchlist = Watchlist::load().unwrap_or_default();

    let mut all_symbols = Vec::new();
    for h in &portfolio.holdings {
        all_symbols.push(h.symbol.clone());
    }
    for w in &watchlist.symbols {
        if !all_symbols.contains(w) {
            all_symbols.push(w.clone());
        }
    }

    if all_symbols.is_empty() {
        return match fmt {
            Format::Json => output::print_json(&Vec::<UpcomingIncomeEvent>::new()),
            Format::Csv => output::print_csv(&Vec::<UpcomingIncomeCsvRow>::new()),
            Format::Table => {
                println!();
                println!(
                    "  {} No symbols found in portfolio or watchlist.",
                    "i".cyan()
                );
                println!();
                Ok(())
            }
        };
    }

    let client = YahooClient::new().await?;
    let resolved = resolve_symbols(&all_symbols, market);
    let (_rows, upcoming_events, notes) = calendar::calendar_rows(&client, &resolved, days).await;

    // Filter to ExDividend and DividendPaid events
    let div_events: Vec<&crate::calendar::Event> = upcoming_events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ExDividend | EventKind::DividendPaid))
        .collect();

    // Map to UpcomingIncomeEvent with holding quantities
    let today = Local::now().date_naive();
    let mut upcoming_list: Vec<UpcomingIncomeEvent> = Vec::with_capacity(div_events.len());

    for ev in div_events {
        let holding = portfolio.holdings.iter().find(|h| {
            ev.symbol.eq_ignore_ascii_case(&h.symbol)
                || ev.symbol.starts_with(&format!("{}.", h.symbol))
        });
        let held_shares = holding.map_or(0.0, |h| h.shares);
        let days_away = (ev.date - today).num_days();

        let kind_str = match ev.kind {
            EventKind::ExDividend => "Ex-Dividend Date",
            EventKind::DividendPaid => "Payment Date",
            _ => "Dividend",
        };

        let status_str = if days_away == 0 {
            "Today".to_string()
        } else if days_away == 1 {
            "Tomorrow".to_string()
        } else if days_away > 1 {
            format!("in {} days", days_away)
        } else {
            format!("{} days ago", -days_away)
        };

        upcoming_list.push(UpcomingIncomeEvent {
            symbol: ev.symbol.clone(),
            event_kind: kind_str.to_string(),
            date: ev.date,
            days_away,
            detail: ev.detail.clone(),
            held_shares,
            status: status_str,
        });
    }

    upcoming_list.sort_by_key(|e| e.date);

    match fmt {
        Format::Json => output::print_json(&upcoming_list),
        Format::Csv => {
            let cal_rows: Vec<UpcomingIncomeCsvRow> = upcoming_list
                .iter()
                .map(|u| UpcomingIncomeCsvRow {
                    date: u.date.to_string(),
                    symbol: u.symbol.clone(),
                    when: u.status.clone(),
                    event: u.event_kind.clone(),
                    held_shares: u.held_shares,
                    detail: u.detail.clone(),
                })
                .collect();
            output::print_csv(&cal_rows)
        }
        Format::Table => {
            println!();
            println!(
                "  {} Upcoming Dividend & Income Radar (Next {} Days)",
                "→".cyan(),
                days
            );
            println!();

            if upcoming_list.is_empty() {
                println!(
                    "  {} No upcoming dividend ex-dates or payment dates found in the next {} days.",
                    "✓".green(),
                    days
                );
                println!();
            } else {
                println!(
                    "  {:<12}  {:<12}  {:<8}  {:<18}  {:<12}  {:<30}",
                    "Date", "Symbol", "When", "Event", "Held Shares", "Details"
                );
                println!("  {}", "─".repeat(95));
                for u in &upcoming_list {
                    let held_str = if u.held_shares > 0.0 {
                        format!("{:.2}", u.held_shares).bold().to_string()
                    } else {
                        "Watchlist".dimmed().to_string()
                    };
                    println!(
                        "  {:<12}  {:<12}  {:<8}  {:<18}  {:<12}  {:<30}",
                        u.date.to_string(),
                        u.symbol.cyan(),
                        u.status.yellow(),
                        u.event_kind,
                        held_str,
                        u.detail.dimmed()
                    );
                }
                println!();
            }

            for note in &notes {
                println!("  {} {}", "!".yellow(), note);
            }
            Ok(())
        }
    }
}

/// An upcoming dividend or coupon event for radar reporting.
#[derive(Debug, Clone, Serialize)]
pub struct UpcomingIncomeEvent {
    pub symbol: String,
    pub event_kind: String,
    pub date: NaiveDate,
    pub days_away: i64,
    pub detail: String,
    pub held_shares: f64,
    pub status: String,
}

/// Print formatted ANSI table for the income and dividend report.
fn print_income_table(report: &IncomeReport, by_holding: bool) {
    let cur = Some(report.currency.as_str());

    println!();
    println!(
        "  {} 12-Month Passive Income, Dividends & Cashflow Calendar",
        "→".cyan()
    );
    println!();

    // 1. Monthly schedule visualization bar chart
    let max_gross = report
        .monthly_schedule
        .iter()
        .map(|m| m.gross_amount)
        .fold(0.0, f64::max);

    println!(
        "  Monthly Cashflow Schedule (Trailing 12M & Projected {}M):",
        report.forecast_months
    );
    println!(
        "  {:<10}  {:>14}  {:>12}  {:<24}  {:<10}",
        "Month", "Gross Amount", "Net Income", "Volume Chart", "Type"
    );
    println!("  {}", "─".repeat(78));

    for m in &report.monthly_schedule {
        let type_tag = if m.is_projected {
            "Projected".cyan()
        } else {
            "Received".dimmed()
        };

        let bar_width = 18;
        let filled_len = if max_gross > 0.0 && m.gross_amount > 0.0 {
            ((m.gross_amount / max_gross) * bar_width as f64).round() as usize
        } else {
            0
        }
        .min(bar_width);

        let bar_str = format!(
            "[{}{}]",
            "█".repeat(filled_len).green(),
            "░".repeat(bar_width - filled_len).dimmed()
        );

        let net_color = if m.net_amount > 0.0 {
            format_price(m.net_amount, cur).green()
        } else {
            format_price(m.net_amount, cur).dimmed()
        };

        println!(
            "  {:<10}  {:>14}  {:>12}  {:<24}  {:<10}",
            m.label,
            format_price(m.gross_amount, cur),
            net_color,
            bar_str,
            type_tag
        );
    }
    println!();

    // 2. Summary KPI Metrics
    print_section("Passive Income & Yield Analytics");
    print_kv(
        "Portfolio Market Value",
        &format_price(report.portfolio_market_value, cur),
    );
    print_kv(
        "Portfolio Cost Basis",
        &format_price(report.portfolio_cost_basis, cur),
    );
    print_kv(
        "Trailing 12M Income Received",
        &format_price(report.trailing_12m_gross, cur),
    );
    print_kv(
        "Projected Annual Gross Income",
        &format_price(report.projected_annual_gross, cur)
            .green()
            .bold()
            .to_string(),
    );
    print_kv(
        "Projected Withholding Tax / TDS",
        &format!("-{}", format_price(report.projected_annual_wht, cur))
            .yellow()
            .to_string(),
    );
    print_kv(
        "Projected Annual Net Spendable Income",
        &format_price(report.projected_annual_net, cur)
            .green()
            .bold()
            .to_string(),
    );
    print_kv(
        "Portfolio Yield-on-Cost (YoC)",
        &format!("{:.2}%", report.portfolio_yoc_pct)
            .cyan()
            .bold()
            .to_string(),
    );
    print_kv(
        "Portfolio Current Market Yield",
        &format!("{:.2}%", report.portfolio_current_yield_pct),
    );
    print_kv(
        "Portfolio Net Spendable Yield",
        &format!("{:.2}%", report.portfolio_net_yield_pct)
            .dimmed()
            .to_string(),
    );
    println!();

    // 3. Holding Breakdown Table
    print_section("Holding Breakdown & Yield-on-Cost (YoC)");
    println!(
        "  {:<12}  {:>8}  {:>10}  {:>10}  {:>8}  {:>8}  {:>12}  {:>12}  {:<8}",
        "Symbol", "Shares", "Price", "Cost/Sh", "Yield", "YoC", "Gross / Yr", "Net / Yr", "Safety"
    );
    println!("  {}", "─".repeat(95));

    for h in &report.holdings {
        let badge_colored = match h.payout_safety {
            PayoutSafety::Safe => "SAFE".green().bold(),
            PayoutSafety::Caution => "CAUTION".yellow().bold(),
            PayoutSafety::Warning => "WARNING".red().bold(),
            PayoutSafety::Unknown => "N/A".dimmed(),
        };

        println!(
            "  {:<12}  {:>8.2}  {:>10}  {:>10}  {:>7.2}%  {:>7.2}%  {:>12}  {:>12}  {:<8}",
            h.symbol.cyan(),
            h.shares,
            format_price(h.current_price, cur),
            format_price(h.avg_cost, cur),
            h.current_yield_pct,
            h.yoc_pct,
            format_price(h.annual_gross_income, cur).green(),
            format_price(h.annual_net_income, cur),
            badge_colored
        );

        if by_holding && h.lots_yoc.len() > 1 {
            for lot in &h.lots_yoc {
                println!(
                    "    ↳ Lot #{:<2}  bought {}  {:>6.2} sh @ {:>8}  (Lot YoC: {})",
                    lot.lot_no,
                    lot.purchase_date.dimmed(),
                    lot.qty,
                    format_price(lot.cost_per_share, cur),
                    format!("{:.2}%", lot.yoc_pct).cyan()
                );
            }
        }
    }
    println!();

    // 4. Payout Safety & Sustainability Warnings
    if !report.payout_warnings.is_empty() {
        print_section("Dividend Sustainability & Payout Safety Alerts");
        for warn in &report.payout_warnings {
            println!("  {} {}", "▲".yellow(), warn);
        }
        println!();
    }

    println!(
        "  {}",
        "Note: Dividend projections are historical estimates based on trailing declared rates; not guaranteed returns or financial advice.".dimmed()
    );
    println!();
}
