//! `portfolio sell --preview`: what a sale would do, without recording it.
//! The maths is in [`crate::sellpreview`]; this module gathers prices and the
//! tax position and prints the result.

use super::holdings::{tax_report, tax_valuation};
use super::portfolio::{ESTIMATED_DATE_HINT, Valuation, held_symbol, signed_money, value_holdings};
use super::*;
use crate::offmarket::{self, IndiaClass, PriceSource};
use crate::portfolio::{self as pf};
use crate::sellpreview::{self, ExitLoadTerms, Rules, SellPreview, TaxContext};
use crate::tax::{self, Country};

/// A preview with the currencies it is in.
pub(crate) struct PreviewOutcome {
    pub preview: SellPreview,
    pub country: Country,
    /// The quote currency of the price and lot amounts.
    pub quote_currency: Option<String>,
    /// The currency of the tax figures.
    pub tax_currency: &'static str,
    pub notes: Vec<String>,
}

/// The country whose rules fit a quote currency (INR, USD, GBP).
fn country_for_currency(currency: Option<&str>) -> Option<Country> {
    match currency? {
        "INR" => Some(Country::India),
        "USD" => Some(Country::Us),
        "GBP" | "GBp" => Some(Country::Uk),
        _ => None,
    }
}

/// The live price (a fund's NAV) to sell at, and its currency.
pub(crate) async fn live_sale_price(
    resolved: &str,
    market: Market,
) -> Result<(f64, Option<String>)> {
    let client = YahooClient::new().await?;
    let val = value_holdings(&client, &[resolved.to_string()], market, None).await?;
    if val.price_source(resolved) == Some(PriceSource::GoldEstimate) {
        bail!(
            "No market price for {resolved} (only an estimate from gold spot); give the sale price"
        );
    }
    let price = val
        .price(resolved)
        .with_context(|| format!("No live price for {resolved}; give the sale price"))?;
    Ok((price, val.currency(resolved)))
}

/// Preview selling `qty` of `resolved` at `price` on `date`. Reads the
/// portfolio and live data; writes nothing.
pub(crate) async fn build_sell_preview(
    portfolio: &Portfolio,
    resolved: &str,
    qty: f64,
    price: f64,
    price_currency: Option<String>,
    date: chrono::NaiveDate,
    market: Market,
) -> Result<PreviewOutcome> {
    let Some(holding) = portfolio.holding(resolved) else {
        bail!("{resolved} is not in your portfolio");
    };
    let mut notes = Vec::new();
    // The same data `tax` uses: prices, AMFI categories, 31 Jan 2018 prices.
    let val = match tax_valuation(&views::LiveSource::new(), portfolio, market).await {
        Ok(v) => v,
        Err(e) => {
            notes.push(format!(
                "Could not load prices ({e}); tax already realized this year may be understated."
            ));
            Valuation::new(Vec::new(), &[], market, None)
        }
    };
    let quote_currency = price_currency.or_else(|| val.currency(resolved));
    let cur = quote_currency.as_deref();
    let country = country_for_currency(cur).unwrap_or(Country::India);
    let scale = cur.and_then(|c| country.currency_scale(c));

    let mut rules = Rules::Listed;
    let mut exit_load = None;
    if country == Country::India {
        match offmarket::india_class(resolved, val.mark(resolved)) {
            IndiaClass::Listed => {}
            IndiaClass::EquityFund => exit_load = Some(ExitLoadTerms::equity_default()),
            IndiaClass::Vda => {
                rules = Rules::Other("Crypto: flat 30% under Sec. 115BBH".into());
            }
            IndiaClass::Other(reason) => rules = Rules::Other(reason),
        }
    }
    if scale.is_none() {
        rules = Rules::Other(format!(
            "quoted in {}; the estimate covers {} only",
            cur.unwrap_or("an unknown currency"),
            country.name()
        ));
    }
    let report = tax_report(country, portfolio, &val, date);
    let ctx = TaxContext {
        country,
        rules,
        realized: report.realized,
        scale: scale.unwrap_or(1.0),
        fmv_2018: val.fmv_2018(resolved),
        exit_load,
    };
    let preview =
        sellpreview::preview_sell(holding, qty, price, date, &ctx).map_err(anyhow::Error::msg)?;
    Ok(PreviewOutcome {
        preview,
        country,
        quote_currency,
        tax_currency: country.tax_currency(),
        notes,
    })
}

pub async fn cmd_portfolio_sell_preview(
    symbol: &str,
    shares: f64,
    price: Option<f64>,
    date: Option<&str>,
    market: Market,
) -> Result<()> {
    let portfolio = Portfolio::load()?;
    let resolved = held_symbol(&portfolio, symbol, market);
    if portfolio.holding(&resolved).is_none() {
        bail!("{} is not in your portfolio", resolved);
    }
    let today = pf::today();
    let date = match date {
        Some(d) => pf::parse_date(d)?,
        None => pf::parse_date(&today)?,
    };
    let (price, cur) = match price {
        Some(p) => (p, pf::symbol_currency(&resolved).map(str::to_string)),
        None if date.to_string() != today => bail!(
            "Give the sale price for a past sale: stockwise portfolio sell {symbol} {shares} <PRICE> --date {date} --preview"
        ),
        None => live_sale_price(&resolved, market).await?,
    };
    let out = build_sell_preview(&portfolio, &resolved, shares, price, cur, date, market).await?;
    print_sell_preview(&out);
    Ok(())
}

fn money(amount: f64, currency: Option<&str>) -> String {
    format_price(amount, currency)
}

pub(crate) fn print_sell_preview(out: &PreviewOutcome) {
    let pv = &out.preview;
    let qc = out.quote_currency.as_deref();
    let tc = Some(out.tax_currency);
    let terms = out.country.long_term_months().is_some();
    print_header(&format!("Sell preview — {} (nothing is saved)", pv.symbol));
    println!(
        "  Sell {} at {} on {}, oldest lots first (FIFO)\n",
        pv.qty,
        money(pv.price, qc),
        pv.date
    );
    println!(
        "  {:>4} {:<12} {:>6} {:>9} {:>11} {:>12} {:>13} {:>5}",
        "Lot".bold(),
        "Bought".bold(),
        "Days".bold(),
        "Qty".bold(),
        "Cost/unit".bold(),
        "Cost".bold(),
        "Gain/loss".bold(),
        if terms { "Term" } else { "" }.bold(),
    );
    println!("  {}", "─".repeat(72).dimmed());
    for l in &pv.lines {
        let bought = if l.date_estimated {
            format!("{}*", l.bought)
        } else {
            l.bought.clone()
        };
        let term = match l.long_term {
            Some(true) if l.grandfathered => "LT†",
            Some(true) => "LT",
            Some(false) => "ST",
            None => "",
        };
        println!(
            "  {:>4} {:<12} {:>6} {:>9.2} {:>11} {:>12} {:>13} {:>5}",
            format!("#{}", l.lot_no),
            bought,
            l.days_held,
            l.qty,
            money(l.unit_cost, qc),
            money(l.cost, qc),
            signed_money(l.gain, qc),
            term,
        );
    }
    println!("  {}", "─".repeat(72).dimmed());
    print_kv("Proceeds", &money(pv.proceeds, qc));
    print_kv("Cost", &money(pv.cost, qc));
    print_kv("Gain/loss", &signed_money(pv.gain, qc));
    if pv.lines.iter().any(|l| l.grandfathered) {
        println!(
            "  {}",
            "† Bought by 31 Jan 2018: cost raised to the 31 Jan 2018 price where that lowers the gain (Sec. 112A)."
                .dimmed()
        );
    }
    if pv.lines.iter().any(|l| l.date_estimated) {
        println!("  {}", ESTIMATED_DATE_HINT.dimmed());
    }

    match (&pv.tax, &pv.other_rules) {
        (Some(t), _) => {
            print_section(&format!(
                "Capital-gains tax added — {} ({})",
                out.country.name(),
                out.country
                    .tax_year_label(chrono::Local::now().date_naive())
            ));
            if terms {
                print_kv(
                    "Short-term gain",
                    &format!(
                        "{} → tax {} at {}%",
                        signed_money(t.st_gain, tc),
                        money(t.st_tax, tc),
                        out.country.st_rate()
                    ),
                );
                print_kv(
                    "Long-term gain",
                    &format!(
                        "{} → tax {} at {}%",
                        signed_money(t.lt_gain, tc),
                        money(t.lt_tax, tc),
                        out.country.lt_rate()
                    ),
                );
            } else {
                print_kv("Gain", &signed_money(t.st_gain + t.lt_gain, tc));
                print_kv("Tax", &money(t.st_tax + t.lt_tax, tc));
            }
            if out.country.exemption() > 0.0 {
                print_kv(
                    if out.country == Country::India {
                        "Sec. 112A exemption"
                    } else {
                        "Annual exempt amount"
                    },
                    &format!(
                        "{} left this year, this sale uses {}",
                        money(t.exemption_left_before, tc),
                        money(t.exemption_used, tc)
                    ),
                );
            }
            if t.cess > 0.0 {
                print_kv(
                    &format!("Cess ({}%)", tax::INDIA_CESS_RATE * 100.0),
                    &money(t.cess, tc),
                );
            }
            print_kv("TOTAL TAX ADDED", &money(t.total, tc).bold().to_string());
            println!(
                "  {}",
                "Estimate on top of gains already recorded this tax year; surcharge and your slab \
                 rate are not included."
                    .dimmed()
            );
        }
        (None, Some(reason)) => {
            print_section("Tax");
            println!("  {} Not estimated here: {}", "!".yellow(), reason);
        }
        (None, None) => {}
    }

    for c in &pv.countdown {
        println!(
            "\n  {} Lot #{} ({} units bought {}, {} days held) turns long-term in {} days (first long-term day {}). \
             At this price, selling then instead of now would lower the tax by {}.",
            "⚠".yellow(),
            c.lot_no,
            c.qty,
            c.bought,
            c.days_held,
            c.days_to_long_term,
            c.long_term_on,
            money(c.tax_saved, tc),
        );
    }

    if let (Some(days), Some(pct)) = (pv.exit_load_days, pv.exit_load_pct) {
        let early: Vec<&sellpreview::LotLine> =
            pv.lines.iter().filter(|l| l.days_held < days).collect();
        if early.is_empty() {
            println!(
                "\n  {} No departing unit is inside a typical {days}-day exit-load window.",
                "·".dimmed()
            );
        } else {
            let units: f64 = early.iter().map(|l| l.qty).sum();
            println!(
                "\n  {} {} units are held under {} days: a typical {}% exit load would take about {} \
                 (check the scheme's factsheet for its actual load and window).",
                "⚠".yellow(),
                units,
                days,
                pct,
                money(pv.exit_load_total, qc),
            );
        }
    }
    for n in &out.notes {
        println!("  {} {}", "!".yellow(), n);
    }
    println!(
        "\n  {}",
        "Preview only: portfolio.json is unchanged. Drop --preview to record the sale.".dimmed()
    );
}

/// JSON for MCP and `--json`-style callers.
pub(crate) fn sell_preview_json(out: &PreviewOutcome) -> serde_json::Value {
    serde_json::json!({
        "preview": true,
        "country": out.country.name(),
        "quote_currency": out.quote_currency,
        "tax_currency": out.tax_currency,
        "result": out.preview,
        "notes": out.notes,
    })
}
