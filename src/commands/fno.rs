//! F&O: option chains with IV and greeks, put/call ratio, max pain, and
//! payoff analysis of multi-leg strategies. NSE for Indian symbols, BSE for
//! SENSEX/BANKEX and `.BO` stocks, Yahoo Finance for US ones (see
//! `derivatives.rs`). The expected-moves view (`options moves`) lives in
//! `risk.rs`.

use super::*;
use crate::derivatives::{self, Assumptions, ChainSource, ContractView, OptionChain};
use crate::greeks::OptionKind;
use crate::payoff;

/// Default risk-free rates (annual, in percent) when `--rate` is not given:
/// roughly the Indian 91-day T-bill and US 3-month Treasury yields.
const DEFAULT_RATE_IN: f64 = 6.5;
const DEFAULT_RATE_US: f64 = 4.0;

fn source_label(chain: &OptionChain) -> &'static str {
    match chain.source {
        ChainSource::Nse => "NSE",
        ChainSource::Bse => "BSE",
        ChainSource::Yahoo => "Yahoo Finance",
    }
}

/// `--rate` default for the chain's market, in percent.
fn default_rate_pct(chain: &OptionChain) -> f64 {
    match chain.source {
        ChainSource::Nse | ChainSource::Bse => DEFAULT_RATE_IN,
        ChainSource::Yahoo => DEFAULT_RATE_US,
    }
}

fn days_to_expiry(chain: &OptionChain) -> f64 {
    chain.years_to_expiry(chrono::Utc::now()) * 365.0
}

fn chain_header(title: &str, chain: &OptionChain) {
    let cur = chain.currency.as_deref();
    print_header(&format!(
        "{}: {} — expiry {}",
        title,
        chain.symbol,
        chain.expiry.format("%d %b %Y")
    ));
    print_kv(
        "Underlying",
        &chain
            .underlying
            .map(|p| format_price(p, cur).bold().to_string())
            .unwrap_or_else(|| "N/A".dimmed().to_string()),
    );
    let days = days_to_expiry(chain);
    print_kv(
        "Time to expiry",
        &if days <= 0.0 {
            "expired".into()
        } else if chain.as_of.is_some() {
            format!("{:.1} days (from the data time)", days)
        } else {
            format!("{:.1} days", days)
        },
    );
    print_kv(
        "Source",
        &format!(
            "{}{}",
            source_label(chain),
            chain
                .as_of
                .map(|t| format!(
                    " · as of {}",
                    t.with_timezone(&chrono::Local).format("%d %b %Y %H:%M")
                ))
                .unwrap_or_default()
        ),
    );
}

fn expiries_line(chain: &OptionChain) -> String {
    let today = chrono::Utc::now().date_naive();
    let upcoming: Vec<String> = chain
        .expiries
        .iter()
        .filter(|d| **d >= today)
        .take(6)
        .map(|d| d.to_string())
        .collect();
    upcoming.join(", ")
}

fn opt_num(v: Option<f64>, decimals: usize, width: usize) -> String {
    match v {
        Some(x) => format!("{:>width$.decimals$}", x),
        None => format!("{:>width$}", "—"),
    }
}

fn opt_count(v: Option<u64>, width: usize) -> String {
    match v {
        Some(x) => format!("{:>width$}", format_volume(x)),
        None => format!("{:>width$}", "—"),
    }
}

fn print_side(
    title: &str,
    rows: &[(f64, Option<&ContractView>)],
    atm: Option<f64>,
    oi_change: bool,
) {
    print_section(title);
    println!(
        "  {:>10} {:>9} {:>8} {:>8} {:>8} {:>7} {:>7} {:>8} {:>8} {:>7} {:>7}",
        "Strike".bold(),
        "LTP".bold(),
        "OI".bold(),
        if oi_change { "ΔOI" } else { "" }.bold(),
        "Volume".bold(),
        "IV".bold(),
        "Delta".bold(),
        "Gamma".bold(),
        "Theta".bold(),
        "Vega".bold(),
        "Rho".bold()
    );
    println!("  {}", "─".repeat(100).dimmed());
    for (strike, view) in rows {
        let strike_txt = format!("{:>10.2}", strike);
        let strike_cell = if Some(*strike) == atm {
            strike_txt.bold().yellow().to_string()
        } else {
            strike_txt
        };
        let Some(v) = view else {
            println!("  {} {}", strike_cell, "no contract".dimmed());
            continue;
        };
        let c = &v.contract;
        let g = v.greeks;
        let oi_change = match c.oi_change {
            Some(x) if oi_change => {
                let s = format!("{:>8}", format!("{:+}", x));
                if x >= 0 {
                    s.green().to_string()
                } else {
                    s.red().to_string()
                }
            }
            _ => format!("{:>8}", ""),
        };
        println!(
            "  {} {} {} {} {} {} {} {} {} {} {}",
            strike_cell,
            opt_num(c.last, 2, 9),
            opt_count(c.open_interest, 8),
            oi_change,
            opt_count(c.volume, 8),
            match v.iv {
                Some(iv) => format!("{:>6.1}%", iv * 100.0),
                None => format!("{:>7}", "—"),
            },
            opt_num(g.map(|g| g.delta), 3, 7),
            opt_num(g.map(|g| g.gamma), 4, 8),
            opt_num(g.map(|g| g.theta), 2, 8),
            opt_num(g.map(|g| g.vega), 2, 7),
            opt_num(g.map(|g| g.rho), 2, 7),
        );
    }
}

/// The rate used (a fraction), years to expiry, and the `strikes` rows
/// around the money with IV and greeks.
fn chain_views(
    chain: &OptionChain,
    strikes: usize,
    rate_pct: Option<f64>,
    div_yield_pct: f64,
) -> (f64, f64, Vec<derivatives::RowView>) {
    let rate = rate_pct.unwrap_or(default_rate_pct(chain)) / 100.0;
    let years = chain.years_to_expiry(chrono::Utc::now());
    // Greeks need a live spot and time left; without them the chain still prints.
    let assumptions = chain
        .underlying
        .filter(|_| years > 0.0)
        .map(|spot| Assumptions {
            spot,
            years,
            rate,
            div_yield: div_yield_pct / 100.0,
        });
    let rows = derivatives::near_the_money(&chain.rows, chain.underlying, strikes);
    // A zero spot makes the IV solver and greeks return None per contract.
    let views = derivatives::row_views(
        rows,
        &assumptions.unwrap_or(Assumptions {
            spot: 0.0,
            years: 0.0,
            rate,
            div_yield: 0.0,
        }),
    );
    (rate, years, views)
}

/// `options chain --json`.
pub(crate) fn chain_json(
    chain: &OptionChain,
    strikes: usize,
    rate_pct: Option<f64>,
    div_yield_pct: f64,
) -> serde_json::Value {
    let (rate, years, views) = chain_views(chain, strikes, rate_pct, div_yield_pct);
    serde_json::json!({
        "symbol": chain.symbol,
        "source": chain.source,
        "underlying": chain.underlying,
        "currency": chain.currency,
        "as_of": chain.as_of,
        "expiry": chain.expiry,
        "expiries": chain.expiries,
        "lot_size": chain.lot_size,
        "days_to_expiry": years * 365.0,
        "rate": rate,
        "div_yield": div_yield_pct / 100.0,
        "rows": views,
    })
}

/// `options chain` — strikes around the money with OI, volume, IV and greeks.
pub async fn cmd_options_chain(
    symbol: &str,
    expiry: Option<chrono::NaiveDate>,
    strikes: usize,
    rate_pct: Option<f64>,
    div_yield_pct: f64,
    market: Market,
    json: bool,
) -> Result<()> {
    let chain = derivatives::fetch_chain(symbol, market, expiry).await?;
    if json {
        let out = chain_json(&chain, strikes, rate_pct, div_yield_pct);
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    let (rate, years, views) = chain_views(&chain, strikes, rate_pct, div_yield_pct);
    let rows = derivatives::near_the_money(&chain.rows, chain.underlying, strikes);

    chain_header("Option chain", &chain);
    print_kv(
        "Rate / dividend yield",
        &format!(
            "{:.2}% / {:.2}%{}",
            rate * 100.0,
            div_yield_pct,
            if rate_pct.is_none() {
                " (default; set --rate)"
            } else {
                ""
            }
        ),
    );
    print_kv("Upcoming expiries", &expiries_line(&chain));
    if let Some(lot) = chain.lot_size {
        print_kv("Lot size", &lot.to_string());
    }
    if chain.underlying.is_none() || years <= 0.0 {
        println!(
            "\n  {}",
            "No live underlying price or time left to expiry: IV is the source's and greeks are skipped."
                .yellow()
        );
    }

    let atm = chain
        .underlying
        .and_then(|s| derivatives::atm_index(rows, s))
        .map(|i| rows[i].strike);
    // NSE and BSE report the day's change in open interest; Yahoo does not.
    let oi_change = chain.source != ChainSource::Yahoo;
    let calls: Vec<(f64, Option<&ContractView>)> =
        views.iter().map(|r| (r.strike, r.call.as_ref())).collect();
    let puts: Vec<(f64, Option<&ContractView>)> =
        views.iter().map(|r| (r.strike, r.put.as_ref())).collect();
    print_side("Calls", &calls, atm, oi_change);
    print_side("Puts", &puts, atm, oi_change);

    if rows.len() < chain.rows.len() {
        println!(
            "\n  {}",
            format!(
                "Showing {} of {} strikes around the money (-n 0 for all).",
                rows.len(),
                chain.rows.len()
            )
            .dimmed()
        );
    }
    println!(
        "  {}",
        "IV is solved from the bid/ask mid (else last price) with Black-Scholes at the data time;\n  theta is per calendar day, vega and rho per 1 point."
            .dimmed()
    );
    if chain.source == ChainSource::Yahoo {
        println!(
            "  {}",
            "US equity options are American-style; these greeks are the European approximation."
                .dimmed()
        );
    }
    println!();
    Ok(())
}

fn pcr_reading(ratio: f64) -> &'static str {
    if ratio < 0.7 {
        "call-heavy — more open calls than puts"
    } else if ratio <= 1.3 {
        "balanced"
    } else {
        "put-heavy — more open puts than calls (often hedging)"
    }
}

/// `options pcr --json`.
pub(crate) fn pcr_json(chain: &OptionChain) -> serde_json::Value {
    let walls = |kind| -> Vec<serde_json::Value> {
        derivatives::top_oi(&chain.rows, kind, 3)
            .iter()
            .map(|(s, oi)| serde_json::json!({"strike": s, "open_interest": oi}))
            .collect()
    };
    serde_json::json!({
        "symbol": chain.symbol,
        "source": chain.source,
        "underlying": chain.underlying,
        "expiry": chain.expiry,
        "pcr": derivatives::pcr(&chain.rows),
        "top_call_oi": walls(OptionKind::Call),
        "top_put_oi": walls(OptionKind::Put),
    })
}

/// `options pcr` — put/call ratio and the strikes with the most open interest.
pub async fn cmd_options_pcr(
    symbol: &str,
    expiry: Option<chrono::NaiveDate>,
    market: Market,
    json: bool,
) -> Result<()> {
    let chain = derivatives::fetch_chain(symbol, market, expiry).await?;
    let p = derivatives::pcr(&chain.rows);
    let top_calls = derivatives::top_oi(&chain.rows, OptionKind::Call, 3);
    let top_puts = derivatives::top_oi(&chain.rows, OptionKind::Put, 3);

    if json {
        println!("{}", serde_json::to_string_pretty(&pcr_json(&chain))?);
        return Ok(());
    }

    chain_header("Put/call ratio", &chain);
    print_section("Open interest");
    print_kv("Calls", &format_volume(p.call_oi));
    print_kv("Puts", &format_volume(p.put_oi));
    print_kv(
        "PCR (OI)",
        &p.oi_ratio
            .map(|r| format!("{} — {}", format!("{:.2}", r).bold(), pcr_reading(r)))
            .unwrap_or_else(|| "N/A (no call open interest)".dimmed().to_string()),
    );
    print_section("Volume");
    print_kv("Calls", &format_volume(p.call_volume));
    print_kv("Puts", &format_volume(p.put_volume));
    print_kv(
        "PCR (volume)",
        &p.volume_ratio
            .map(|r| format!("{:.2}", r))
            .unwrap_or_else(|| "N/A (no call volume)".dimmed().to_string()),
    );

    let cur = chain.currency.as_deref();
    let wall = |v: &[(f64, u64)]| -> String {
        if v.is_empty() {
            return "—".into();
        }
        v.iter()
            .map(|(s, oi)| format!("{} ({})", format_price(*s, cur), format_volume(*oi)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    print_section("Largest open interest");
    print_kv("Calls (resistance)", &wall(&top_calls));
    print_kv("Puts (support)", &wall(&top_puts));
    println!(
        "\n  {}",
        "Descriptive positioning for this expiry, not a trading signal.".dimmed()
    );
    println!();
    Ok(())
}

fn max_pain_of(chain: &OptionChain) -> Result<derivatives::MaxPain> {
    derivatives::max_pain(&chain.rows).with_context(|| {
        format!(
            "No open interest for {} on {} — max pain needs open contracts",
            chain.symbol, chain.expiry
        )
    })
}

/// `options maxpain --json`.
pub(crate) fn maxpain_json(chain: &OptionChain, mp: &derivatives::MaxPain) -> serde_json::Value {
    serde_json::json!({
        "symbol": chain.symbol,
        "source": chain.source,
        "underlying": chain.underlying,
        "expiry": chain.expiry,
        "max_pain": mp.strike,
        "curve": mp.curve,
    })
}

/// `options maxpain` for an already fetched chain.
pub(crate) fn maxpain_view(chain: &OptionChain) -> Result<serde_json::Value> {
    Ok(maxpain_json(chain, &max_pain_of(chain)?))
}

/// `options maxpain` — the settlement price where option buyers collect least.
pub async fn cmd_options_maxpain(
    symbol: &str,
    expiry: Option<chrono::NaiveDate>,
    market: Market,
    json: bool,
) -> Result<()> {
    let chain = derivatives::fetch_chain(symbol, market, expiry).await?;
    let mp = max_pain_of(&chain)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&maxpain_json(&chain, &mp))?
        );
        return Ok(());
    }

    let cur = chain.currency.as_deref();
    chain_header("Max pain", &chain);
    print_kv(
        "Max pain strike",
        &format_price(mp.strike, cur).bold().yellow().to_string(),
    );
    if let Some(spot) = chain.underlying {
        let gap = (mp.strike - spot) / spot * 100.0;
        print_kv("Distance from spot", &format!("{:+.2}%", gap));
    }

    print_section("Option holders' payout if expiry settles at each strike");
    let i = mp
        .curve
        .iter()
        .position(|p| p.strike == mp.strike)
        .unwrap_or(0);
    let lo = i.saturating_sub(7);
    let hi = (i + 8).min(mp.curve.len());
    let window = &mp.curve[lo..hi];
    let max = window.iter().map(|p| p.total).fold(0.0, f64::max);
    for p in window {
        let len = if max > 0.0 {
            ((p.total / max) * 40.0).round() as usize
        } else {
            0
        };
        let bar = "█".repeat(len.max(1));
        let strike = format!("{:>10.2}", p.strike);
        let (strike, bar) = if p.strike == mp.strike {
            (strike.bold().yellow().to_string(), bar.yellow().to_string())
        } else {
            (strike, bar.dimmed().to_string())
        };
        println!(
            "  {} {} {}",
            strike,
            bar,
            format_volume(p.total as u64).dimmed()
        );
    }
    println!(
        "\n  {}",
        "Payout = Σ open interest × intrinsic value (in underlying points). Max pain assumes open\n  interest stays where it is; it describes positioning and is not a price forecast."
            .dimmed()
    );
    println!();
    Ok(())
}

/// What `options payoff` builds and how it prices it.
pub struct PayoffArgs {
    pub legs: Vec<payoff::LegSpec>,
    pub strategy: Option<payoff::Preset>,
    /// Strike distance for presets (default: two strike steps).
    pub width: Option<f64>,
    pub expiry: Option<chrono::NaiveDate>,
    pub lots: u32,
    pub lot_size: Option<u32>,
    pub rate_pct: Option<f64>,
    pub div_yield_pct: f64,
    /// Volatility for the probability of profit (default: at-the-money IV).
    pub iv_pct: Option<f64>,
    pub today: bool,
    pub json: bool,
}

/// One priced leg and where its price came from.
#[derive(Serialize)]
struct LegView {
    #[serde(flatten)]
    leg: payoff::Leg,
    /// `mid`, `ltp` (last trade, no two-sided quote), `manual` (`@PRICE`)
    /// or `spot` (an underlying leg at the current price).
    price_from: &'static str,
    iv_from: Option<&'static str>,
    delta: Option<f64>,
}

/// Smallest gap between listed strikes near the money.
fn strike_step(rows: &[derivatives::ChainRow], atm: usize) -> Option<f64> {
    let lo = atm.saturating_sub(5);
    let hi = (atm + 6).min(rows.len());
    rows[lo..hi]
        .windows(2)
        .map(|w| w[1].strike - w[0].strike)
        .filter(|g| *g > 1e-9)
        .min_by(f64::total_cmp)
}

fn nearest_row(rows: &[derivatives::ChainRow], strike: f64) -> usize {
    derivatives::atm_index(rows, strike).expect("chain has rows")
}

fn listed_near(rows: &[derivatives::ChainRow], strike: f64) -> String {
    let i = nearest_row(rows, strike);
    rows[i.saturating_sub(1)..(i + 2).min(rows.len())]
        .iter()
        .map(|r| format!("{}", r.strike))
        .collect::<Vec<_>>()
        .join(", ")
}

/// "-₹1,234.50"-style amount with its sign.
fn money(v: f64, cur: Option<&str>) -> String {
    if v < 0.0 {
        format!("-{}", format_price(-v, cur))
    } else {
        format_price(v, cur)
    }
}

fn pl_colored(v: f64, cur: Option<&str>) -> String {
    let s = money(v, cur);
    if v >= 0.0 {
        s.green().to_string()
    } else {
        s.red().to_string()
    }
}

/// A priced multi-leg position: everything `options payoff` shows.
pub(crate) struct PayoffAnalysis {
    chain: OptionChain,
    spot: f64,
    years: f64,
    rate: f64,
    div_yield: f64,
    width: f64,
    lot_size: Option<u32>,
    lot_from: Option<&'static str>,
    /// Lot size × lots: per-unit amounts times this are position amounts.
    scale: f64,
    legs: Vec<LegView>,
    notes: Vec<String>,
    premium: f64,
    ext: payoff::Extremes,
    breakevens: Vec<f64>,
    ranges: Vec<(f64, Option<f64>)>,
    pop: Option<f64>,
    pop_iv: Option<f64>,
    pop_iv_from: &'static str,
    greeks: Option<crate::greeks::Greeks>,
    /// The payoff grid: underlying prices, P/L at expiry and (when every
    /// leg has an IV) P/L today.
    prices: Vec<f64>,
    at_expiry: Vec<f64>,
    today: Option<Vec<f64>>,
}

impl PayoffAnalysis {
    /// `options payoff --json`.
    pub(crate) fn json(&self, args: &PayoffArgs) -> serde_json::Value {
        let scale = self.scale;
        serde_json::json!({
            "symbol": self.chain.symbol,
            "source": self.chain.source,
            "underlying": self.spot,
            "currency": self.chain.currency,
            "as_of": self.chain.as_of,
            "expiry": self.chain.expiry,
            "days_to_expiry": self.years * 365.0,
            "strategy": args.strategy,
            "width": args.strategy.filter(|p| *p != payoff::Preset::Straddle).map(|_| self.width),
            "lot_size": self.lot_size,
            "lot_size_from": self.lot_from,
            "lots": args.lots,
            "multiplier": self.scale,
            "rate": self.rate,
            "div_yield": self.div_yield,
            "legs": self.legs,
            "net_premium": self.premium,
            "net_premium_total": round(self.premium * scale, 2),
            "premium_type": if self.premium >= 0.0 { "debit" } else { "credit" },
            "max_profit": round_opt(self.ext.max_profit.map(|v| v * scale), 2),
            "max_loss": round_opt(self.ext.max_loss.map(|v| v * scale), 2),
            "breakevens": self.breakevens.iter().map(|b| round(*b, 2)).collect::<Vec<_>>(),
            "profit_ranges": self.ranges.iter().map(|(a, b)| serde_json::json!({"from": round(*a, 2), "to": round_opt(*b, 2)})).collect::<Vec<_>>(),
            "pop": self.pop,
            "pop_iv": self.pop_iv,
            "pop_iv_from": self.pop_iv_from,
            "greeks": self.greeks.map(|g| serde_json::json!({
                "delta": round(g.delta * scale, 4),
                "gamma": round(g.gamma * scale, 6),
                "theta": round(g.theta * scale, 2),
                "vega": round(g.vega * scale, 2),
                "rho": round(g.rho * scale, 2),
            })),
            "notes": self.notes,
            "curve": self.prices.iter().enumerate().map(|(i, p)| serde_json::json!({
                "price": round(*p, 2),
                "expiry": round(self.at_expiry[i], 2),
                "today": self.today.as_ref().map(|t| round(t[i], 2)),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Fetch the chain and price the position `args` describes.
pub(crate) async fn payoff_analysis(
    symbol: &str,
    args: &PayoffArgs,
    market: Market,
) -> Result<PayoffAnalysis> {
    use payoff::{Instrument, Leg, StrikeSpec};
    if args.legs.is_empty() && args.strategy.is_none() {
        bail!(
            "Describe the position with --leg (e.g. --leg \"buy call 24500\" --leg \"sell call 24800\") \
             or a preset (--strategy bull-call|bear-put|straddle|strangle|iron-condor|butterfly|covered-call)"
        );
    }
    if args.lots == 0 {
        bail!("--lots must be at least 1");
    }
    if let Some(w) = args.width
        && !(w.is_finite() && w > 0.0)
    {
        bail!("--width must be a positive number of points");
    }

    let chain = derivatives::fetch_chain(symbol, market, args.expiry).await?;
    let now = chrono::Utc::now();
    let spot = chain.underlying.with_context(|| {
        format!(
            "{} has no live underlying price in the chain; payoff needs one",
            chain.symbol
        )
    })?;
    let years = chain.years_to_expiry(now);
    if years <= 0.0 {
        bail!(
            "The {} expiry has passed; pick a later one with -e (listed: {})",
            chain.expiry,
            expiries_line(&chain)
        );
    }
    let rows = &chain.rows;
    if rows.is_empty() {
        bail!("{} lists no strikes for {}", chain.symbol, chain.expiry);
    }
    let rate = args.rate_pct.unwrap_or(default_rate_pct(&chain)) / 100.0;
    let assumptions = Assumptions {
        spot,
        years,
        rate,
        div_yield: args.div_yield_pct / 100.0,
    };
    let atm = derivatives::atm_index(rows, spot).expect("chain has rows");
    let step = strike_step(rows, atm).unwrap_or(spot * 0.01);
    let width = args.width.unwrap_or(2.0 * step);

    // Every leg as (side, qty, instrument, row index or none, manual price).
    let mut wanted: Vec<(payoff::Side, u32, Instrument, Option<usize>, Option<f64>)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if let Some(preset) = args.strategy {
        for (side, qty, inst, strike) in payoff::preset_legs(preset, rows[atm].strike, width) {
            let row = strike.map(|k| {
                let i = nearest_row(rows, k);
                if (rows[i].strike - k).abs() > 1e-6 {
                    notes.push(format!(
                        "{} is not listed; using the nearest strike, {}",
                        k, rows[i].strike
                    ));
                }
                i
            });
            wanted.push((side, qty, inst, row, None));
        }
    }
    for spec in &args.legs {
        let row = match spec.strike {
            None => None,
            Some(StrikeSpec::Price(k)) => {
                let i = nearest_row(rows, k);
                if (rows[i].strike - k).abs() > 1e-6 {
                    bail!(
                        "No {} strike for {} on {}. Nearest listed: {}",
                        k,
                        chain.symbol,
                        chain.expiry,
                        listed_near(rows, k)
                    );
                }
                Some(i)
            }
            Some(StrikeSpec::Atm(n)) => {
                let i = atm as i64 + n as i64;
                if i < 0 || i >= rows.len() as i64 {
                    bail!("atm{:+} is outside the listed strikes", n);
                }
                Some(i as usize)
            }
        };
        wanted.push((spec.side, spec.qty, spec.instrument, row, spec.price));
    }

    let mut legs: Vec<LegView> = Vec::new();
    for (side, qty, instrument, row, manual) in wanted {
        let Some(i) = row else {
            legs.push(LegView {
                leg: Leg {
                    side,
                    qty,
                    instrument,
                    strike: None,
                    price: manual.unwrap_or(spot),
                    iv: None,
                },
                price_from: if manual.is_some() { "manual" } else { "spot" },
                iv_from: None,
                delta: Some(1.0),
            });
            continue;
        };
        let r = &rows[i];
        let (kind, contract) = match instrument {
            Instrument::Call => (OptionKind::Call, r.call.as_ref()),
            _ => (OptionKind::Put, r.put.as_ref()),
        };
        let label = format!(
            "{} {}",
            r.strike,
            if kind == OptionKind::Call {
                "call"
            } else {
                "put"
            }
        );
        let contract = contract
            .with_context(|| format!("{} has no {label} on {}", chain.symbol, chain.expiry))?;
        let (price, price_from) = match (manual, contract.bid, contract.ask) {
            (Some(p), ..) => (p, "manual"),
            (None, Some(b), Some(a)) if b > 0.0 && a >= b => ((a + b) / 2.0, "mid"),
            (None, ..) => (
                contract.last.filter(|p| *p > 0.0).with_context(|| {
                    format!(
                        "No price for the {label}: no two-sided quote and no trade. Give one with @, \
                         e.g. --leg \"{} {} {} @PRICE\"",
                        if side == payoff::Side::Buy { "buy" } else { "sell" },
                        if kind == OptionKind::Call { "call" } else { "put" },
                        r.strike
                    )
                })?,
                "ltp",
            ),
        };
        let view = derivatives::contract_view(contract, kind, r.strike, &assumptions);
        legs.push(LegView {
            leg: Leg {
                side,
                qty,
                instrument,
                strike: Some(r.strike),
                price,
                iv: view.iv,
            },
            price_from,
            iv_from: view.iv_from,
            delta: view.greeks.map(|g| g.delta),
        });
    }
    let plain: Vec<Leg> = legs.iter().map(|l| l.leg.clone()).collect();

    // Lot size: the flag, else the chain's; unknown means per-unit amounts.
    let (lot_size, lot_from) = match (args.lot_size, chain.lot_size) {
        (Some(n), _) => (Some(n), Some("flag")),
        (None, Some(n)) => (
            Some(n),
            Some(if chain.source == ChainSource::Yahoo {
                "contract"
            } else {
                "chain"
            }),
        ),
        (None, None) => (None, None),
    };
    let scale = lot_size.unwrap_or(1) as f64 * args.lots as f64;

    let m = payoff::Market {
        t: years,
        rate,
        div_yield: assumptions.div_yield,
    };
    let atm_iv = {
        let r = &rows[atm];
        let ivs: Vec<f64> = [
            (r.call.as_ref(), OptionKind::Call),
            (r.put.as_ref(), OptionKind::Put),
        ]
        .iter()
        .filter_map(|(c, k)| derivatives::contract_view((*c)?, *k, r.strike, &assumptions).iv)
        .collect();
        (!ivs.is_empty()).then(|| ivs.iter().sum::<f64>() / ivs.len() as f64)
    };
    let (pop_iv, pop_iv_from) = match (args.iv_pct, atm_iv) {
        (Some(v), _) => (Some(v / 100.0), "flag"),
        (None, Some(v)) => (Some(v), "atm"),
        (None, None) => (None, "none"),
    };
    let premium = payoff::net_premium(&plain);
    let ext = payoff::extremes(&plain);
    let breakevens = payoff::breakevens(&plain);
    let ranges = payoff::profit_ranges(&plain);
    let pop = pop_iv.and_then(|v| payoff::probability_of_profit(&plain, spot, v, &m));
    let greeks = payoff::net_greeks(&plain, spot, &m);

    // Price grid: every strike and spot, padded by ~2.5 standard deviations.
    let mut marks: Vec<f64> = plain.iter().filter_map(|l| l.strike).collect();
    marks.push(spot);
    let lo_mark = marks.iter().copied().fold(f64::INFINITY, f64::min);
    let hi_mark = marks.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sd_move = spot * pop_iv.unwrap_or(0.2) * years.sqrt();
    let pad = (2.5 * sd_move)
        .max(spot * 0.03)
        .max((hi_mark - lo_mark) * 0.5);
    let (x0, x1) = ((lo_mark - pad).max(0.0), hi_mark + pad);
    const POINTS: usize = 120;
    let prices: Vec<f64> = (0..POINTS)
        .map(|i| x0 + (x1 - x0) * i as f64 / (POINTS - 1) as f64)
        .collect();
    let at_expiry: Vec<f64> = prices
        .iter()
        .map(|s| payoff::pl_at_expiry(&plain, *s) * scale)
        .collect();
    let today: Option<Vec<f64>> = prices
        .iter()
        .map(|s| payoff::pl_before_expiry(&plain, *s, &m).map(|v| v * scale))
        .collect();

    Ok(PayoffAnalysis {
        chain,
        spot,
        years,
        rate,
        div_yield: assumptions.div_yield,
        width,
        lot_size,
        lot_from,
        scale,
        legs,
        notes,
        premium,
        ext,
        breakevens,
        ranges,
        pop,
        pop_iv,
        pop_iv_from,
        greeks,
        prices,
        at_expiry,
        today,
    })
}

/// `options payoff` — a multi-leg position priced from the live chain:
/// net premium, max profit/loss, breakevens, probability of profit, net
/// greeks and a payoff chart. Analysis only: nothing is ordered.
pub async fn cmd_options_payoff(symbol: &str, args: PayoffArgs, market: Market) -> Result<()> {
    use payoff::Instrument;
    let a = payoff_analysis(symbol, &args, market).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&a.json(&args))?);
        return Ok(());
    }
    let PayoffAnalysis {
        chain,
        spot,
        lot_size,
        lot_from,
        scale,
        legs,
        notes,
        premium,
        ext,
        breakevens,
        pop,
        pop_iv,
        pop_iv_from,
        greeks,
        prices,
        at_expiry,
        today,
        ..
    } = a;
    let cur = chain.currency.as_deref();

    let title = match args.strategy {
        Some(p) => format!(
            "Payoff ({})",
            serde_json::to_value(p)?.as_str().unwrap_or("strategy")
        ),
        None => "Payoff".to_string(),
    };
    chain_header(&title, &chain);
    let (lot_word, unit_word) = if chain.source == ChainSource::Yahoo {
        ("contract", "shares")
    } else {
        ("lot", "units")
    };
    print_kv(
        "Size",
        &match lot_size {
            Some(n) => format!(
                "{} {lot_word}{} × {} {unit_word}{}",
                args.lots,
                if args.lots == 1 { "" } else { "s" },
                n,
                match lot_from {
                    Some("chain") =>
                        format!(" (lot size from the {} order book)", source_label(&chain)),
                    Some("contract") => " (standard US contract)".into(),
                    _ => String::new(),
                }
            ),
            None => format!(
                "{} × 1 unit — lot size unknown (no order book in the data); pass --lot-size",
                args.lots
            ),
        },
    );

    print_section("Legs");
    println!(
        "  {:<5} {:>4}  {:<6} {:>10} {:>10}  {:<6} {:>7} {:>7}",
        "Side".bold(),
        "Qty".bold(),
        "Type".bold(),
        "Strike".bold(),
        "Price".bold(),
        "From".bold(),
        "IV".bold(),
        "Delta".bold()
    );
    for l in &legs {
        let side = match l.leg.side {
            payoff::Side::Buy => "BUY".green().to_string(),
            payoff::Side::Sell => "SELL".red().to_string(),
        };
        let kind = match l.leg.instrument {
            Instrument::Call => "CALL",
            Instrument::Put => "PUT",
            Instrument::Underlying => "UNDER",
        };
        println!(
            "  {:<5} {:>4}  {:<6} {:>10} {:>10.2}  {:<6} {} {}",
            side,
            l.leg.qty,
            kind,
            l.leg
                .strike
                .map(|k| format!("{:.2}", k))
                .unwrap_or_else(|| "—".into()),
            l.leg.price,
            if l.price_from == "ltp" {
                "ltp".yellow().to_string()
            } else {
                l.price_from.to_string()
            },
            match l.leg.iv {
                Some(iv) => format!("{:>6.1}%", iv * 100.0),
                None => format!("{:>7}", "—"),
            },
            opt_num(l.delta, 3, 7)
        );
    }

    print_section("At expiry");
    print_kv(
        if premium >= 0.0 {
            "Net debit"
        } else {
            "Net credit"
        },
        &format!(
            "{} per unit → {}",
            format_price(premium.abs(), cur),
            format_price(premium.abs() * scale, cur).bold()
        ),
    );
    print_kv(
        "Max profit",
        &match ext.max_profit {
            Some(v) => pl_colored(v * scale, cur),
            None => "unlimited (grows as the underlying rises)"
                .green()
                .to_string(),
        },
    );
    print_kv(
        "Max loss",
        &match ext.max_loss {
            Some(v) => pl_colored(v * scale, cur),
            None => "unlimited (grows as the underlying rises)"
                .red()
                .to_string(),
        },
    );
    print_kv(
        "Breakevens",
        &if breakevens.is_empty() {
            "none".dimmed().to_string()
        } else {
            breakevens
                .iter()
                .map(|b| format!("{:.2} ({:+.2}%)", b, (b / spot - 1.0) * 100.0))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    print_kv(
        "Probability of profit",
        &match (pop, pop_iv) {
            (Some(p), Some(v)) => format!(
                "{} (lognormal at {:.1}% IV{})",
                format!("{:.1}%", p * 100.0).bold(),
                v * 100.0,
                if pop_iv_from == "atm" {
                    ", at the money"
                } else {
                    ""
                }
            ),
            _ => "N/A (no implied volatility; pass --iv)"
                .dimmed()
                .to_string(),
        },
    );
    print_section("Position greeks now");
    match greeks {
        Some(g) => {
            print_kv(
                "Delta",
                &format!("{:+.2} units of the underlying", g.delta * scale),
            );
            print_kv("Gamma", &format!("{:+.4} per point", g.gamma * scale));
            print_kv("Theta", &format!("{} per day", money(g.theta * scale, cur)));
            print_kv(
                "Vega",
                &format!("{} per vol point", money(g.vega * scale, cur)),
            );
        }
        None => println!("  {}", "N/A — a leg has no implied volatility".dimmed()),
    }

    println!();
    println!(
        "  {}  {}{}",
        "P/L by underlying price at expiry".bold(),
        "━ profit ".green(),
        "━ loss".red()
    );
    if args.today && today.is_some() {
        println!("  {}", "━ today (Black-Scholes at each leg's IV)".yellow());
    }
    println!();
    let today_curve = today.as_deref().filter(|_| args.today);
    for line in charts::payoff_chart(&prices, &at_expiry, today_curve, spot, 60, 12) {
        println!("{line}");
    }
    if args.today && today.is_none() {
        println!(
            "  {}",
            "No today curve: a leg has no implied volatility.".dimmed()
        );
    }

    for n in &notes {
        println!("\n  {}", n.yellow());
    }
    if legs.iter().any(|l| l.price_from == "ltp") {
        println!(
            "\n  {}",
            "ltp = last traded price, used where there is no two-sided quote; it can be stale."
                .yellow()
        );
    }
    println!(
        "\n  {}",
        "Prices are bid/ask mids where quoted. Excludes brokerage, taxes and exchange charges.\n  \
         Probability of profit assumes a lognormal price at the IV shown; it is a model, not a forecast.\n  \
         Analysis only — StockWise places no orders."
            .dimmed()
    );
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivatives::ChainRow;

    fn rows(strikes: &[f64]) -> Vec<ChainRow> {
        strikes
            .iter()
            .map(|k| ChainRow {
                strike: *k,
                call: None,
                put: None,
            })
            .collect()
    }

    #[test]
    fn strike_steps_and_neighbours() {
        // Wider strikes far from the money don't hide the 50-point step.
        let r = rows(&[
            21000.0, 22000.0, 22500.0, 22550.0, 22600.0, 22650.0, 22700.0, 23500.0,
        ]);
        assert_eq!(strike_step(&r, 4), Some(50.0));
        assert_eq!(listed_near(&r, 22610.0), "22550, 22600, 22650");
        assert_eq!(listed_near(&r, 20000.0), "21000, 22000");
        assert_eq!(strike_step(&rows(&[100.0]), 0), None);
    }

    #[test]
    fn signed_money() {
        assert_eq!(money(-1234.5, Some("INR")), "-₹1234.50");
        assert_eq!(money(12.0, Some("USD")), "$12.00");
    }
}
