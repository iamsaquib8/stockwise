//! Practice mode for the simulator: `sim replay`, `sim challenge`,
//! `sim profile` and `sim card`. Paper money only; nothing here talks to a
//! broker. Replay views read prices through `Tape::upto` at the replay date,
//! so no output — table or JSON — can show a bar from the future.

use super::*;
use crate::practice::challenge::{self, Active, Mode, Outcome};
use crate::practice::live::LiveChallenge;
use crate::practice::profile::{ChallengeSummary, Profile, RunResult};
pub use crate::practice::replay::OrderSide;
use crate::practice::replay::{Event, FillReason, Replay};
use crate::practice::scenario::{self, Member, Scenario};
use crate::practice::score::{self, Scorecard};
use crate::practice::tape::{self, DayBar, Tape};
use crate::practice::{card, replay};
use chrono::NaiveDate;
use serde_json::{Value, json};

/// Daily history fetched before a replay's first day, for charts and context.
const LOOKBACK_DAYS: i64 = 370;

fn currency(r: &Replay) -> Option<&'static str> {
    Some(super::portfolio::market_currency(scenario_market(
        &r.scenario,
    )))
}

fn scenario_market(s: &Scenario) -> Market {
    <Market as clap::ValueEnum>::from_str(&s.market, true).unwrap_or(Market::In)
}

/// The index a market's replays and challenges are measured against.
fn default_benchmark(m: Market) -> (&'static str, &'static str) {
    match m {
        Market::Us => ("^GSPC", "S&P 500"),
        Market::Global(x) => x.info().benchmark(),
        _ => ("^NSEI", "NIFTY 50"),
    }
}

/// `₹1234` / `-₹1234` (no decimals) for account amounts.
fn amount(v: f64, cur: Option<&str>) -> String {
    let sym = market::currency_symbol(cur);
    if v < 0.0 {
        format!("-{sym}{:.0}", -v)
    } else {
        format!("{sym}{v:.0}")
    }
}

fn signed(v: f64) -> String {
    let s = format!("{v:+.2}%");
    if v >= 0.0 {
        s.green().to_string()
    } else {
        s.red().to_string()
    }
}

fn disclaimer() -> &'static str {
    t(score::DISCLAIMER)
}

/// The running replay, without its price file. `sim replay end` scores from
/// the marks and fills saved in the replay itself, so it must work even when
/// the price file is gone.
fn load_running_replay() -> Result<Replay> {
    let Some(r) = Replay::load()? else {
        bail!(
            "No replay is running. Start one with `stockwise sim replay start covid-crash-2020` \
             (`stockwise sim replay list` shows them all)."
        );
    };
    Ok(r)
}

fn load_replay() -> Result<(Replay, Tape)> {
    let r = load_running_replay()?;
    let tape = Tape::load()?;
    if !tape.has(&r.scenario.benchmark) {
        bail!(
            "The replay's price file ({}) is missing or damaged. `stockwise sim replay end` still scores the run so far; then start again.",
            tape::FILE
        );
    }
    Ok((r, tape))
}

fn remove_replay_files() -> Result<()> {
    for file in [replay::FILE, tape::FILE] {
        let path = crate::store::path(file)?;
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("Cannot remove {}", path.display()))?;
        }
    }
    Ok(())
}

// ── sim replay list ──

pub fn cmd_replay_list(fmt: Format) -> Result<()> {
    let all = scenario::built_in();
    if fmt == Format::Json {
        let rows: Vec<Value> = all
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "title": s.title,
                    "market": s.market,
                    "from": s.from,
                    "to": s.to,
                    "benchmark": s.benchmark,
                    "benchmark_name": s.benchmark_name,
                    "capital": s.capital,
                    "stocks": s.universe.len(),
                    "description": s.description,
                })
            })
            .collect();
        return output::print_json(&rows);
    }
    print_header("Replay scenarios");
    for s in &all {
        println!(
            "  {}  {}",
            pad_end(&s.name, 18).cyan().bold(),
            s.title.bold()
        );
        println!(
            "  {}  {} → {} · {} · {} stocks vs {}",
            " ".repeat(18),
            s.from,
            s.to,
            scenario_market(s).label(),
            s.universe.len(),
            s.benchmark_name
        );
    }
    println!(
        "\n  {} {}   or   {}",
        "Start:".dimmed(),
        "stockwise sim replay start covid-crash-2020".cyan(),
        "stockwise sim replay start --from 2022-01-03 --to 2022-06-30 --symbols TCS,INFY,HDFCBANK"
            .cyan()
    );
    println!("  {}\n", disclaimer().dimmed());
    Ok(())
}

// ── sim replay start ──

pub struct ReplayStart {
    pub scenario: Option<String>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub symbols: Vec<String>,
    pub benchmark: Option<String>,
    pub capital: Option<f64>,
    pub costs: String,
    pub fmt: Format,
}

/// A scenario from `--from/--to/--symbols` in the active market.
fn custom_scenario(a: &ReplayStart, m: Market) -> Result<Scenario> {
    m.require_bot_support("sim replay")?;
    let (Some(from), Some(to)) = (a.from, a.to) else {
        bail!("A custom replay needs --from DATE and --to DATE (YYYY-MM-DD)");
    };
    let today = chrono::Local::now().date_naive();
    if to >= today {
        bail!("--to must be in the past: a replay hides a future that has already happened");
    }
    if (to - from).num_days() < 14 {
        bail!("A replay needs at least two weeks between --from and --to");
    }
    if a.symbols.is_empty() {
        bail!("Pick the stocks to trade with --symbols, e.g. --symbols TCS,INFY,HDFCBANK");
    }
    let (bench, bench_name) = match (&a.benchmark, m) {
        (Some(b), _) => (market::resolve_symbol(b, m), b.to_uppercase()),
        (None, _) => {
            let (symbol, name) = default_benchmark(m);
            (symbol.to_string(), name.to_string())
        }
    };
    let mut universe: Vec<Member> = Vec::new();
    for s in a.symbols.iter().flat_map(|s| s.split(',')) {
        let symbol = market::resolve_symbol(s.trim(), m);
        if s.trim().is_empty() || universe.iter().any(|u| u.symbol == symbol) {
            continue;
        }
        universe.push(Member {
            symbol,
            // Unknown: a scenario file can name real sectors.
            sector: "Unknown".into(),
        });
    }
    Ok(Scenario {
        name: "custom".into(),
        title: format!("{} {} → {}", m.label(), from, to),
        market: m.code().into(),
        from,
        to,
        benchmark: bench,
        benchmark_name: bench_name,
        capital: 100_000.0,
        description: "Your own replay window.".into(),
        debrief: String::new(),
        universe,
    })
}

async fn fetch_tape(s: &Scenario, quiet: bool) -> Result<(Tape, Vec<String>)> {
    let client = YahooClient::new().await?;
    let start = crate::clock::at(scenario_market(s), s.from, chrono::NaiveTime::MIN).timestamp()
        - LOOKBACK_DAYS * 86_400;
    let end =
        crate::clock::at(scenario_market(s), s.to, chrono::NaiveTime::MIN).timestamp() + 2 * 86_400;
    let mut tape = Tape::default();
    let mut missing = Vec::new();
    let symbols: Vec<&str> = std::iter::once(s.benchmark.as_str())
        .chain(s.universe.iter().map(|m| m.symbol.as_str()))
        .collect();
    if !quiet {
        println!(
            "  {} Fetching {} daily histories once — the replay then runs offline...",
            "⟳".yellow(),
            symbols.len()
        );
    }
    for symbol in symbols {
        match client.get_chart_between(symbol, start, end, "1d").await {
            Ok(chart) => {
                let bars = Tape::day_bars(&backtest::Bars::from_chart(&chart));
                let in_window = bars.iter().any(|b| b.date >= s.from && b.date <= s.to);
                if in_window {
                    let cur = chart.meta.as_ref().and_then(|m| m.currency.clone());
                    tape.insert(symbol, cur, bars);
                } else {
                    missing.push(format!("{symbol}: no prices in the replay window"));
                }
            }
            Err(e) => missing.push(format!("{symbol}: {e:#}")),
        }
    }
    Ok((tape, missing))
}

pub async fn cmd_replay_start(a: ReplayStart, m: Market) -> Result<()> {
    if let Some(r) = Replay::load()? {
        bail!(
            "A replay is already running ({}, day {} of {}). Run `stockwise sim replay end` first.",
            r.scenario.title,
            r.day + 1,
            r.calendar.len()
        );
    }
    let mut s = match &a.scenario {
        Some(name) => {
            if a.from.is_some() || a.to.is_some() || !a.symbols.is_empty() {
                bail!("Give a scenario name or --from/--to/--symbols, not both");
            }
            scenario::load(name)?
        }
        None => custom_scenario(&a, m)?,
    };
    let quiet = a.fmt == Format::Json;
    let (tape, missing) = fetch_tape(&s, quiet).await?;
    if !tape.has(&s.benchmark) {
        bail!(
            "Could not load the benchmark {} ({}): {}",
            s.benchmark_name,
            s.benchmark,
            missing.join("; ")
        );
    }
    s.universe.retain(|m| tape.has(&m.symbol));
    if s.universe.len() < 2 {
        bail!(
            "Too few stocks with prices in this window to play: {}",
            missing.join("; ")
        );
    }
    let capital = a.capital.unwrap_or(s.capital);
    let r = Replay::new(s, &tape, capital, &a.costs)?;
    tape.save()?;
    r.save()?;

    if quiet {
        let mut v = status_json(&r, &tape);
        v["description"] = json!(r.scenario.description);
        v["skipped"] = json!(missing);
        return output::print_json(&v);
    }
    for w in &missing {
        println!("  {} Skipped {w}", "!".yellow());
    }
    print_header(&format!("Replay — {}", r.scenario.title));
    for line in wrap(&r.scenario.description, 70) {
        println!("  {line}");
    }
    println!();
    println!(
        "  {} Prices up to the close of each replay day only — the future stays hidden.",
        "→".dimmed()
    );
    println!(
        "  {} Orders fill at the next day's open. Stops and targets are checked on each day's range.",
        "→".dimmed()
    );
    println!("  {} Costs: {}", "→".dimmed(), r.cost_summary(&tape));
    print_status(&r, &tape);
    println!(
        "  {} {}  ·  {}  ·  {}",
        "Next:".dimmed(),
        "sim replay buy SYMBOL QTY --stop PRICE".cyan(),
        "sim replay next".cyan(),
        "sim challenge list".cyan()
    );
    println!("  {}\n", disclaimer().dimmed());
    Ok(())
}

/// Wrap `text` at `width` columns on spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && display_width(&line) + 1 + display_width(word) > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// ── Status (shared by start, next, status) ──

fn pct_change(bars: &[DayBar], back: usize) -> Option<f64> {
    let last = bars.last()?;
    let then = bars.get(bars.len().checked_sub(back + 1)?)?;
    Some((last.close / then.close - 1.0) * 100.0)
}

/// Everything `sim replay status --json` shows, from data known on the
/// replay date.
pub(crate) fn status_json(r: &Replay, tape: &Tape) -> Value {
    let date = r.date();
    let equity = r.equity();
    let positions: Vec<Value> = r
        .positions
        .iter()
        .map(|(symbol, p)| {
            let close = tape.last_close(symbol, date).unwrap_or(p.avg_price);
            let value = close * p.qty as f64;
            json!({
                "symbol": symbol,
                "sector": r.scenario.sector(symbol),
                "qty": p.qty,
                "avg_price": round(p.avg_price, 2),
                "close": round(close, 2),
                "value": round(value, 2),
                "pnl": round((close - p.avg_price) * p.qty as f64, 2),
                "pnl_percent": round((close / p.avg_price - 1.0) * 100.0, 2),
                "weight_percent": round(if equity > 0.0 { value / equity * 100.0 } else { 0.0 }, 2),
                "stop": p.stop,
                "target": p.target,
                "opened": p.opened,
            })
        })
        .collect();
    let universe: Vec<Value> = r
        .scenario
        .universe
        .iter()
        .map(|m| {
            let bars = tape.upto(&m.symbol, date);
            json!({
                "symbol": m.symbol,
                "sector": m.sector,
                "close": bars.last().map(|b| round(b.close, 2)),
                "change_percent": pct_change(bars, 1).map(|v| round(v, 2)),
                "change_20d_percent": pct_change(bars, 20).map(|v| round(v, 2)),
            })
        })
        .collect();
    let challenge = r.challenge.as_ref().map(|c| challenge_json(c, r.day));
    let last = r.marks.last();
    json!({
        "scenario": r.scenario.name,
        "title": r.scenario.title,
        "market": r.scenario.market,
        "date": date,
        "day": r.day + 1,
        "days": r.calendar.len(),
        "last_day": r.is_last_day(),
        "capital": round(r.capital, 2),
        "cash": round(r.cash, 2),
        "invested": last.map(|m| round(m.invested, 2)),
        "equity": round(equity, 2),
        "return_percent": round(r.return_percent(), 2),
        "max_drawdown_percent": round(r.max_drawdown_percent(), 2),
        "costs_paid": round(r.costs_paid, 2),
        "cost_model": r.costs,
        "benchmark": {
            "symbol": r.scenario.benchmark,
            "name": r.scenario.benchmark_name,
            "close": last.map(|m| round(m.benchmark, 2)),
            "return_percent": round(r.benchmark_return_percent(), 2),
        },
        "positions": positions,
        "orders": r.orders,
        "universe": universe,
        "challenge": challenge,
        "finished_challenges": r.finished_challenges,
        "disclaimer": disclaimer(),
    })
}

fn challenge_json(c: &Active, day: usize) -> Value {
    json!({
        "name": c.def.name,
        "title": c.def.title,
        "description": c.def.description,
        "day": day.saturating_sub(c.start_day),
        "days": c.end_day - c.start_day,
        "started": c.start_date,
        "rules": c.def.rules.iter().map(|r| json!({"rule": r.label(), "goal": r.is_goal(), "hard": r.is_hard()})).collect::<Vec<_>>(),
        "violations": c.violations,
    })
}

/// The status screen as lines (for printing and for the leak test).
pub(crate) fn status_lines(r: &Replay, tape: &Tape) -> Vec<String> {
    let cur = currency(r);
    let date = r.date();
    let equity = r.equity();
    let mut out = Vec::new();
    let bench_close = r.marks.last().map_or(0.0, |m| m.benchmark);
    out.push(format!(
        "  {}  {}  ·  {} {:.2} ({})",
        date.format("%a %d %b %Y").to_string().bold(),
        tf(Msg::PracticeDay, &[&(r.day + 1), &r.calendar.len()]).dimmed(),
        r.scenario.benchmark_name,
        bench_close,
        signed(r.benchmark_return_percent())
    ));
    out.push(format!(
        "  {} {}  ·  {} {}  ·  {} {}  ·  {} {:.1}%",
        t(Msg::PracticeEquity).dimmed(),
        amount(equity, cur).bold(),
        t(Msg::PracticeCash).dimmed(),
        amount(r.cash, cur),
        t(Msg::PracticeReturn).dimmed(),
        signed(r.return_percent()),
        t(Msg::PracticeMaxDrawdown).dimmed(),
        r.max_drawdown_percent()
    ));
    if r.marks.len() > 2 {
        let recent = &r.marks[r.marks.len().saturating_sub(60)..];
        let eq: Vec<f64> = recent.iter().map(|m| m.equity).collect();
        out.push(format!("  {}", sparkline(&eq).cyan()));
    }

    if !r.positions.is_empty() {
        out.push(String::new());
        out.push(format!("  {}", t(Msg::PracticePositions).bold().yellow()));
        out.push(format!(
            "  {:<16} {:>6} {:>11} {:>11} {:>11} {:>7} {:>10} {:>10}",
            "Symbol", "Qty", "Avg", "Close", "P&L", "Weight", "Stop", "Target"
        ));
        for (symbol, p) in &r.positions {
            let close = tape.last_close(symbol, date).unwrap_or(p.avg_price);
            let pnl = (close - p.avg_price) * p.qty as f64;
            let weight = close * p.qty as f64 / equity * 100.0;
            let level = |v: Option<f64>| v.map_or("—".to_string(), |x| format!("{x:.2}"));
            let pnl_s = format!("{:>11}", amount(pnl, cur));
            out.push(format!(
                "  {:<16} {:>6} {:>11.2} {:>11.2} {} {:>6.1}% {:>10} {:>10}",
                symbol.cyan(),
                p.qty,
                p.avg_price,
                close,
                if pnl >= 0.0 {
                    pnl_s.green()
                } else {
                    pnl_s.red()
                },
                weight,
                level(p.stop),
                level(p.target)
            ));
        }
    }
    if !r.orders.is_empty() {
        out.push(String::new());
        out.push(format!("  {}", t(Msg::PracticeOrders).bold().yellow()));
        for o in &r.orders {
            let side = match o.side {
                OrderSide::Buy => "BUY ".green(),
                OrderSide::Sell => "SELL".red(),
            };
            let mut extra = String::new();
            if let Some(s) = o.stop {
                extra.push_str(&format!("  stop {s:.2}"));
            }
            if let Some(tg) = o.target {
                extra.push_str(&format!("  target {tg:.2}"));
            }
            out.push(format!(
                "  #{:<3} {} {:>6} {}{}",
                o.id,
                side,
                o.qty,
                o.symbol.cyan(),
                extra.dimmed()
            ));
        }
    }
    if let Some(c) = &r.challenge {
        out.push(String::new());
        out.push(format!(
            "  {} {} — {}",
            t(Msg::PracticeChallenge).bold().yellow(),
            c.def.title.bold(),
            tf(
                Msg::PracticeDay,
                &[&(r.day - c.start_day), &(c.end_day - c.start_day)]
            )
            .dimmed()
        ));
        for rule in &c.def.rules {
            let tag = if rule.is_goal() { "goal" } else { "rule" };
            out.push(format!(
                "    {} {}",
                format!("[{tag}]").dimmed(),
                rule.label()
            ));
        }
        for v in &c.violations {
            out.push(format!(
                "    {} {} — {} ({})",
                "!".yellow(),
                v.rule,
                v.detail,
                v.date
            ));
        }
    }

    out.push(String::new());
    out.push(format!("  {}", t(Msg::PracticeMarket).bold().yellow()));
    out.push(format!(
        "  {:<16} {:<22} {:>11} {:>8} {:>8}",
        "Symbol", "Sector", "Close", "Day", "20 days"
    ));
    for m in &r.scenario.universe {
        let bars = tape.upto(&m.symbol, date);
        let Some(last) = bars.last() else {
            out.push(format!(
                "  {:<16} {:<22} {:>11}",
                m.symbol,
                m.sector,
                "no price yet".dimmed()
            ));
            continue;
        };
        let change = |back: usize| {
            pct_change(bars, back).map_or("—".dimmed().to_string(), |v| {
                let s = format!("{v:>+7.1}%");
                if v >= 0.0 {
                    s.green().to_string()
                } else {
                    s.red().to_string()
                }
            })
        };
        out.push(format!(
            "  {:<16} {:<22} {:>11.2} {:>8} {:>8}",
            m.symbol.cyan(),
            pad_end(&m.sector, 22),
            last.close,
            change(1),
            change(20)
        ));
    }
    out.push(String::new());
    out
}

fn print_status(r: &Replay, tape: &Tape) {
    for line in status_lines(r, tape) {
        println!("{line}");
    }
}

pub fn cmd_replay_status(fmt: Format) -> Result<()> {
    let (r, tape) = load_replay()?;
    if fmt == Format::Json {
        return output::print_json(&status_json(&r, &tape));
    }
    print_header(&format!("Replay — {}", r.scenario.title));
    print_status(&r, &tape);
    println!("  {}\n", disclaimer().dimmed());
    Ok(())
}

// ── sim replay next ──

fn event_line(e: &Event, cur: Option<&str>) -> String {
    match e {
        Event::Contribution { amount: a } => format!(
            "  {} Added {} (monthly SIP contribution)",
            "+".green(),
            amount(*a, cur)
        ),
        Event::Fill(f) => {
            let what = match (f.side, f.reason) {
                (OrderSide::Buy, _) => "Bought".green().to_string(),
                (OrderSide::Sell, FillReason::Order) => "Sold".to_string(),
                (OrderSide::Sell, FillReason::StopLoss) => "Stop-loss hit, sold".red().to_string(),
                (OrderSide::Sell, FillReason::Target) => "Target reached, sold".green().to_string(),
            };
            let pnl = f.pnl.map_or(String::new(), |p| {
                let s = format!("  P&L {}", amount(p, cur));
                if p >= 0.0 {
                    s.green().to_string()
                } else {
                    s.red().to_string()
                }
            });
            format!(
                "  {} {} {} {} @ {:.2} (charges {:.2}){}",
                "✓".green(),
                what,
                f.qty,
                f.symbol.cyan(),
                f.price,
                f.charges,
                pnl
            )
        }
        Event::Rejected {
            symbol,
            side,
            qty,
            reason,
        } => format!(
            "  {} Not filled: {} {qty} {} — {reason}",
            "✗".red(),
            if *side == OrderSide::Buy {
                "buy"
            } else {
                "sell"
            },
            symbol.cyan()
        ),
        Event::Waiting { symbol, .. } => format!(
            "  {} {} did not trade today; its order waits for the next session",
            "…".dimmed(),
            symbol
        ),
        Event::RuleBroken(v) => format!(
            "  {} {}: {} — {}",
            "!".yellow().bold(),
            if v.hard {
                "Hard rule broken".red().bold().to_string()
            } else {
                "Rule broken".yellow().to_string()
            },
            v.rule,
            v.detail
        ),
        Event::ChallengeOver(o) => outcome_line(o),
        Event::LastDay => format!(
            "  {} Last day of the replay. Run {} for your score.",
            "→".bold(),
            "stockwise sim replay end".cyan()
        ),
    }
}

fn outcome_line(o: &Outcome) -> String {
    let verdict = if o.passed {
        t(Msg::PracticePassed).to_uppercase().green().bold()
    } else {
        t(Msg::PracticeFailed).to_uppercase().red().bold()
    };
    let mut s = format!(
        "  {} {} — {} ({})",
        t(Msg::PracticeChallenge).bold(),
        o.title.bold(),
        verdict,
        o.reason
    );
    for g in &o.goals {
        let mark = if g.met { "✓".green() } else { "✗".red() };
        s.push_str(&format!(
            "\n      {mark} {} — {}",
            g.goal,
            g.detail.dimmed()
        ));
    }
    s
}

fn print_new_badges(ids: &[String]) {
    for id in ids {
        if let Some(b) = score::badge(id) {
            println!(
                "  {} {}",
                "★".yellow().bold(),
                tf(Msg::PracticeNewBadge, &[&t(b.title)]).bold()
            );
        }
    }
}

pub fn cmd_replay_next(days: u32, fmt: Format) -> Result<()> {
    let (mut r, tape) = load_replay()?;
    if r.is_last_day() {
        bail!("The replay is on its last day. Run `stockwise sim replay end` for your score.");
    }
    let cur = currency(&r);
    let mut log: Vec<(NaiveDate, Vec<Event>)> = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..days.max(1) {
        if r.is_last_day() {
            break;
        }
        let events = r.step(&tape)?;
        outcomes.extend(events.iter().filter_map(|e| match e {
            Event::ChallengeOver(o) => Some(o.clone()),
            _ => None,
        }));
        log.push((r.date(), events));
    }
    r.save()?;
    let mut new_badges = Vec::new();
    if !outcomes.is_empty() {
        let mut profile = Profile::load()?;
        let today = chrono::Local::now().date_naive();
        for o in &outcomes {
            new_badges.extend(profile.record_challenge(o, today));
        }
        profile.save()?;
    }

    if fmt == Format::Json {
        let days: Vec<Value> = log
            .iter()
            .map(|(d, ev)| json!({"date": d, "events": ev}))
            .collect();
        return output::print_json(&json!({
            "days": days,
            "new_badges": new_badges,
            "status": status_json(&r, &tape),
        }));
    }
    println!();
    for (d, events) in &log {
        let quiet = events.is_empty();
        println!(
            "  {} {}",
            d.format("%a %d %b %Y").to_string().bold(),
            if quiet {
                "—".dimmed().to_string()
            } else {
                String::new()
            }
        );
        for e in events {
            println!("{}", event_line(e, cur));
        }
    }
    print_new_badges(&new_badges);
    println!();
    for line in status_lines(&r, &tape).into_iter().take(3) {
        println!("{line}");
    }
    if !r.positions.is_empty() || !r.orders.is_empty() {
        println!(
            "  {} {} for positions, orders and the market",
            "→".dimmed(),
            "stockwise sim replay status".cyan()
        );
    }
    println!();
    Ok(())
}

// ── sim replay buy / sell / cancel / set ──

fn resolve_in(r: &Replay, symbol: &str) -> String {
    market::resolve_symbol(symbol, scenario_market(&r.scenario))
}

pub fn cmd_replay_order(
    side: OrderSide,
    symbol: &str,
    qty: Option<u64>,
    stop: Option<f64>,
    target: Option<f64>,
    fmt: Format,
) -> Result<()> {
    let (mut r, tape) = load_replay()?;
    let symbol = resolve_in(&r, symbol);
    let qty = match (side, qty) {
        (_, Some(q)) => q,
        (OrderSide::Sell, None) => {
            let held = r.positions.get(&symbol).map_or(0, |p| p.qty);
            let queued: u64 = r
                .orders
                .iter()
                .filter(|o| o.side == OrderSide::Sell && o.symbol == symbol)
                .map(|o| o.qty)
                .sum();
            held.saturating_sub(queued)
        }
        (OrderSide::Buy, None) => bail!("How many? `stockwise sim replay buy {symbol} QTY`"),
    };
    let (order, warnings) = r.place(&tape, side, &symbol, qty, stop, target)?;
    r.save()?;
    if fmt == Format::Json {
        return output::print_json(&json!({"order": order, "warnings": warnings}));
    }
    let close = tape.last_close(&symbol, r.date()).unwrap_or(0.0);
    let cur = currency(&r);
    println!(
        "\n  {} Queued #{}: {} {} {} at the next open (last close {:.2}, about {}).",
        "✓".green(),
        order.id,
        if side == OrderSide::Buy {
            "BUY".green().bold()
        } else {
            "SELL".red().bold()
        },
        order.qty,
        symbol.cyan(),
        close,
        amount(close * order.qty as f64, cur)
    );
    if side == OrderSide::Buy
        && order.stop.is_none()
        && !r.positions.get(&symbol).is_some_and(|p| p.stop.is_some())
    {
        println!(
            "  {} No stop-loss: decide now where you are wrong (--stop PRICE). The score rewards it.",
            "!".yellow()
        );
    }
    for w in &warnings {
        println!(
            "  {} Rule broken: {} — {}",
            "!".yellow().bold(),
            w.rule,
            w.detail
        );
    }
    println!(
        "  {} {} to move to the next day.\n",
        "→".dimmed(),
        "stockwise sim replay next".cyan()
    );
    Ok(())
}

pub fn cmd_replay_cancel(id: Option<u32>) -> Result<()> {
    let (mut r, _) = load_replay()?;
    let n = r.cancel(id);
    if n == 0 {
        match id {
            Some(i) => bail!("No pending order #{i}"),
            None => {
                println!("\n  {} No pending orders.\n", "→".dimmed());
                return Ok(());
            }
        }
    }
    r.save()?;
    println!("\n  {} Cancelled {n} order(s).\n", "✓".green());
    Ok(())
}

pub fn cmd_replay_set(symbol: &str, stop: Option<f64>, target: Option<f64>) -> Result<()> {
    if stop.is_none() && target.is_none() {
        bail!("Give --stop PRICE, --target PRICE or both");
    }
    let (mut r, tape) = load_replay()?;
    let symbol = resolve_in(&r, symbol);
    r.protect(&tape, &symbol, stop, target)?;
    r.save()?;
    println!("\n  {} Updated {}.\n", "✓".green(), symbol.cyan());
    Ok(())
}

// ── sim replay chart ──

pub fn cmd_replay_chart(symbol: &str, days: usize) -> Result<()> {
    let (r, tape) = load_replay()?;
    let wanted = symbol.trim().to_uppercase();
    let symbol = if ["INDEX", "BENCHMARK"].contains(&wanted.as_str())
        || wanted == r.scenario.benchmark.to_uppercase()
    {
        r.scenario.benchmark.clone()
    } else {
        resolve_in(&r, symbol)
    };
    if symbol != r.scenario.benchmark && r.scenario.sector(&symbol).is_none() {
        bail!(
            "{symbol} is not in this replay. Pick one of: {}, or `index`",
            r.scenario.symbols().join(", ")
        );
    }
    let bars = tape.upto(&symbol, r.date());
    let shown = &bars[bars.len().saturating_sub(days.max(2))..];
    if shown.is_empty() {
        println!("\n  {} No prices for {symbol} yet.\n", "→".dimmed());
        return Ok(());
    }
    let closes: Vec<f64> = shown.iter().map(|b| b.close).collect();
    let first = shown.first().expect("not empty");
    let last = shown.last().expect("not empty");
    let up = last.close >= first.close;
    println!();
    let title = format!("{symbol} · {} → {}", first.date, last.date);
    for line in charts::line_chart(&closes, 60, 12, if up { "green" } else { "red" }, &title) {
        println!("{line}");
    }
    println!(
        "  {} {:.2} → {:.2} ({})\n",
        "Close".dimmed(),
        first.close,
        last.close,
        signed((last.close / first.close - 1.0) * 100.0)
    );
    Ok(())
}

// ── Scorecards ──

fn print_scorecard(title: &str, run: &RunResult, debrief: &str) {
    let c: &Scorecard = &run.scorecard;
    print_header(&format!("{} — {title}", t(Msg::PracticeScorecard)));
    match (c.score, &c.grade) {
        (Some(s), Some(g)) => {
            println!(
                "  {} {}  {}  {} {}",
                t(Msg::PracticeScore).bold(),
                format!("{s}/100").bold(),
                rating_bar(s as f64, 100.0),
                t(Msg::PracticeGrade).bold(),
                g.bold()
            );
            let parts = &c.parts;
            println!(
                "  {}",
                format!(
                    "{} {:.1}/40 · {} {:.1}/20 · {} {:.1}/25 · {} {:.1}/15",
                    t(Msg::PracticeRiskAdjusted),
                    parts.risk_adjusted,
                    t(Msg::PracticeDrawdownControl),
                    parts.drawdown,
                    t(Msg::PracticeDiscipline),
                    parts.discipline,
                    t(Msg::PracticeDiversification),
                    parts.diversification
                )
                .dimmed()
            );
        }
        _ => println!(
            "  {}",
            tf(Msg::PracticeNotScored, &[&score::MIN_DAYS]).yellow()
        ),
    }
    println!();
    let bench = &run.benchmark_name;
    println!(
        "  {:<22} {:>12} {:>16}",
        "",
        t(Msg::PracticeYou).bold(),
        bench.bold()
    );
    let row = |label: &str, you: String, them: String| {
        println!("  {} {:>12} {:>16}", pad_end(label, 22), you, them);
    };
    let ratio = |v: Option<f64>| v.map_or("—".into(), |r| format!("{r:.2}"));
    row(
        t(Msg::PracticeReturn),
        format!("{:+.2}%", c.return_percent),
        format!("{:+.2}%", c.benchmark_return_percent),
    );
    row(
        t(Msg::PracticeMaxDrawdown),
        format!("{:.2}%", c.max_drawdown_percent),
        format!("{:.2}%", c.benchmark_max_drawdown_percent),
    );
    row("Sharpe", ratio(c.sharpe), ratio(c.benchmark_sharpe));
    row("Sortino", ratio(c.sortino), ratio(c.benchmark_sortino));
    row(
        t(Msg::PracticeInvested),
        format!("{:.0}%", c.exposure_percent),
        String::new(),
    );
    println!("  {}", tf(Msg::PracticeTradingDays, &[&c.days]).dimmed());

    if !run.challenges.is_empty() {
        println!();
        for ch in &run.challenges {
            let verdict = if ch.passed {
                t(Msg::PracticePassed).green().bold()
            } else {
                t(Msg::PracticeFailed).red().bold()
            };
            println!(
                "  {} {} — {verdict} ({})",
                t(Msg::PracticeChallenge).bold(),
                ch.title,
                ch.reason
            );
        }
    }
    println!();
    println!("  {}", t(Msg::PracticeBadges).bold().yellow());
    if run.badges.is_empty() {
        println!("  {}", t(Msg::PracticeNoBadges).dimmed());
    }
    for id in &run.badges {
        if let Some(b) = score::badge(id) {
            let new = if run.new_badges.contains(id) {
                " NEW".yellow().bold().to_string()
            } else {
                String::new()
            };
            println!("  {} {}{new}", "★".yellow(), t(b.title));
        }
    }
    if !debrief.is_empty() {
        println!();
        println!("  {}", t(Msg::PracticeDebrief).bold().yellow());
        for line in wrap(debrief, 70) {
            println!("  {line}");
        }
    }
    println!();
    println!(
        "  {} {}  ·  {}",
        "→".dimmed(),
        "stockwise sim card --svg card.svg".cyan(),
        "stockwise sim profile".cyan()
    );
    println!("  {}\n", disclaimer().dimmed());
}

/// Badges a challenge earned as it ended belong to the run's result too.
fn merge_new_badges(run: &mut RunResult, ids: Vec<String>) {
    for id in ids {
        if !run.badges.contains(&id) {
            run.badges.push(id.clone());
        }
        if !run.new_badges.contains(&id) {
            run.new_badges.push(id);
        }
    }
}

fn run_json(run: &RunResult, debrief: &str) -> Value {
    let mut v = serde_json::to_value(run).unwrap_or(Value::Null);
    v["debrief"] = json!(debrief);
    v["disclaimer"] = json!(disclaimer());
    v
}

// ── sim replay end ──

pub fn cmd_replay_end(fmt: Format) -> Result<()> {
    let mut r = load_running_replay()?;
    let today = chrono::Local::now().date_naive();
    let mut profile = Profile::load()?;
    let mut challenge_badges = Vec::new();
    if let Some(ch) = &r.challenge {
        let abandoned = r.day < ch.end_day;
        let outcome = r.finish_challenge(abandoned);
        challenge_badges = profile.record_challenge(&outcome, today);
    }
    let input = r.run_input();
    let card = score::score(&input);
    let mut badges: Vec<String> = score::badges(&card, &r.run_facts(&input))
        .into_iter()
        .map(String::from)
        .collect();
    if card.score.is_none() {
        badges.clear();
    }
    let run = RunResult {
        kind: "replay".into(),
        name: r.scenario.name.clone(),
        title: r.scenario.title.clone(),
        market: r.scenario.market.clone(),
        benchmark_name: r.scenario.benchmark_name.clone(),
        from: r.calendar[0],
        to: r.date(),
        finished: today,
        disciplined: score::disciplined(&input, &card),
        scorecard: card,
        badges,
        new_badges: Vec::new(),
        challenges: r
            .finished_challenges
            .iter()
            .map(ChallengeSummary::from)
            .collect(),
        curve: score::rebased(&input.nav, 60),
        benchmark_curve: score::rebased(&input.benchmark, 60),
    };
    let mut run = if run.scorecard.score.is_some() {
        profile.record_run(run, today)
    } else {
        run
    };
    merge_new_badges(&mut run, challenge_badges);
    profile.save()?;
    remove_replay_files()?;
    let debrief = r.scenario.debrief.clone();
    if fmt == Format::Json {
        return output::print_json(&run_json(&run, &debrief));
    }
    print_scorecard(&r.scenario.title, &run, &debrief);
    Ok(())
}

// ── sim challenge ──

pub fn cmd_challenge_list(fmt: Format) -> Result<()> {
    let all = challenge::built_in();
    if fmt == Format::Json {
        return output::print_json(&all);
    }
    print_header("Challenges");
    for c in &all {
        let modes: Vec<&str> = c
            .modes
            .iter()
            .map(|m| match m {
                Mode::Replay => "replay",
                Mode::Live => "live",
            })
            .collect();
        let length = match c.days {
            0 => "until the replay ends".to_string(),
            n => format!("{n} trading days"),
        };
        println!(
            "  {}  {}",
            pad_end(&c.name, 16).cyan().bold(),
            c.title.bold()
        );
        for line in wrap(&c.description, 64) {
            println!("  {}  {}", " ".repeat(16), line);
        }
        println!(
            "  {}  {}",
            " ".repeat(16),
            format!("{length} · {}", modes.join(", ")).dimmed()
        );
    }
    println!(
        "\n  {} {} (during a replay, or on its own for live paper sessions)",
        "Start:".dimmed(),
        "stockwise sim challenge start steady-hands".cyan()
    );
    println!("  {}\n", disclaimer().dimmed());
    Ok(())
}

pub fn cmd_challenge_start(name: &str, m: Market, fmt: Format) -> Result<()> {
    let def = challenge::load(name)?;
    if let Some(mut r) = Replay::load()? {
        let tape = Tape::load()?;
        r.start_challenge(&tape, def)?;
        r.save()?;
        let c = r.challenge.as_ref().expect("just started");
        if fmt == Format::Json {
            return output::print_json(&challenge_json(c, r.day));
        }
        println!(
            "\n  {} {} — {} trading days from {}.",
            "✓".green(),
            c.def.title.bold(),
            c.end_day - c.start_day,
            c.start_date
        );
        for rule in &c.def.rules {
            println!(
                "    {} {}",
                if rule.is_goal() { "goal" } else { "rule" }.dimmed(),
                rule.label()
            );
        }
        if c.def.monthly_contribution.is_some() {
            println!(
                "  {} Your account now holds one month's contribution; the next arrives on the first trading day of each month.",
                "→".dimmed()
            );
        }
        println!();
        return Ok(());
    }
    if !def.modes.contains(&Mode::Live) {
        bail!(
            "The {} challenge runs over a replay. Start one first: `stockwise sim replay start covid-crash-2020`.",
            def.title
        );
    }
    m.require_bot_support("sim challenge")?;
    if let Some(c) = LiveChallenge::load()? {
        bail!(
            "The {} challenge is already running on live paper sessions. `stockwise sim challenge end` first.",
            c.active.def.title
        );
    }
    let today = crate::clock::session_date(m, chrono::Utc::now());
    let days = def.days;
    let live = LiveChallenge {
        active: Active::new(def, 0, today, days),
        market: m.id().into(),
    };
    live.save()?;
    if fmt == Format::Json {
        return output::print_json(&challenge_json(&live.active, 0));
    }
    println!(
        "\n  {} {} — over your next {days} settled paper sessions ({}).",
        "✓".green(),
        live.active.def.title.bold(),
        m.label()
    );
    for rule in &live.active.def.rules {
        println!(
            "    {} {}",
            if rule.is_goal() { "goal" } else { "rule" }.dimmed(),
            rule.label()
        );
    }
    println!(
        "  {} Each trading day: {} in the morning, {} after the close. Progress: {}.\n",
        "→".dimmed(),
        "sim start".cyan(),
        "sim settle".cyan(),
        "sim challenge status".cyan()
    );
    Ok(())
}

/// Benchmark bars for a live challenge, from a few days before it started.
async fn live_benchmark(live: &LiveChallenge) -> Result<Vec<DayBar>> {
    let m = <Market as clap::ValueEnum>::from_str(&live.market, true).unwrap_or(Market::In);
    let symbol = default_benchmark(m).0;
    let start = crate::clock::at(m, live.active.start_date, chrono::NaiveTime::MIN).timestamp()
        - 10 * 86_400;
    let end = chrono::Utc::now().timestamp() + 86_400;
    let client = YahooClient::new().await?;
    let chart = client.get_chart_between(symbol, start, end, "1d").await?;
    Ok(Tape::day_bars(&backtest::Bars::from_chart(&chart)))
}

fn live_benchmark_name(live: &LiveChallenge) -> &'static str {
    let m = <Market as clap::ValueEnum>::from_str(&live.market, true).unwrap_or(Market::In);
    default_benchmark(m).1
}

pub async fn cmd_challenge_status(fmt: Format) -> Result<()> {
    if let Some(r) = Replay::load()? {
        let Some(c) = &r.challenge else {
            if fmt == Format::Json {
                return output::print_json(&json!({
                    "mode": "replay",
                    "challenge": null,
                    "finished": r.finished_challenges,
                }));
            }
            println!(
                "\n  {} No challenge in this replay. {}\n",
                "→".dimmed(),
                "stockwise sim challenge list".cyan()
            );
            for o in &r.finished_challenges {
                println!("{}", outcome_line(o));
            }
            return Ok(());
        };
        if fmt == Format::Json {
            return output::print_json(&json!({
                "mode": "replay",
                "challenge": challenge_json(c, r.day),
                "finished": r.finished_challenges,
            }));
        }
        let tape = Tape::load()?;
        let lines = status_lines(&r, &tape);
        print_header(&format!("{} — {}", t(Msg::PracticeChallenge), c.def.title));
        for line in lines.iter().take(2) {
            println!("{line}");
        }
        let start = lines
            .iter()
            .position(|l| l.contains(&c.def.title))
            .unwrap_or(0);
        for line in lines.iter().skip(start).take_while(|l| !l.is_empty()) {
            println!("{line}");
        }
        println!("  {}\n", disclaimer().dimmed());
        return Ok(());
    }
    let Some(live) = LiveChallenge::load()? else {
        if fmt == Format::Json {
            return output::print_json(&json!({"challenge": null}));
        }
        println!(
            "\n  {} No challenge running. {}\n",
            "→".dimmed(),
            "stockwise sim challenge list".cyan()
        );
        return Ok(());
    };
    let history = SimHistory::load()?;
    let sessions = live.sessions(&history);
    let bench = if sessions.is_empty() {
        Vec::new()
    } else {
        live_benchmark(&live).await?
    };
    let run = live.evaluate(&sessions, &bench);
    if fmt == Format::Json {
        return output::print_json(&json!({
            "mode": "live",
            "challenge": challenge_json(&live.active, run.sessions),
            "sessions": run.sessions,
            "complete": live.is_complete(run.sessions),
            "return_percent": round(run.facts.return_percent, 2),
            "benchmark_return_percent": round(run.facts.benchmark_return_percent, 2),
            "max_drawdown_percent": round(run.facts.max_drawdown_percent, 2),
            "violations": run.violations,
        }));
    }
    print_header(&format!(
        "{} — {}",
        t(Msg::PracticeChallenge),
        live.active.def.title
    ));
    println!(
        "  {} {} / {} settled sessions since {}",
        "Progress:".dimmed(),
        run.sessions,
        live.active.end_day,
        live.active.start_date
    );
    println!(
        "  {} {}  ·  {} {}  ·  {} {:.1}%",
        t(Msg::PracticeReturn).dimmed(),
        signed(run.facts.return_percent),
        live_benchmark_name(&live),
        signed(run.facts.benchmark_return_percent),
        t(Msg::PracticeMaxDrawdown).dimmed(),
        run.facts.max_drawdown_percent
    );
    for v in &run.violations {
        println!("  {} {} — {} ({})", "!".yellow(), v.rule, v.detail, v.date);
    }
    if live.is_complete(run.sessions) {
        println!(
            "  {} Complete — run {} for the result.",
            "→".bold(),
            "stockwise sim challenge end".cyan()
        );
    }
    println!("  {}\n", disclaimer().dimmed());
    Ok(())
}

pub async fn cmd_challenge_end(fmt: Format) -> Result<()> {
    let today = chrono::Local::now().date_naive();
    if let Some(mut r) = Replay::load()? {
        let Some(c) = &r.challenge else {
            bail!("No challenge is running in this replay");
        };
        let abandoned = r.day < c.end_day;
        let outcome = r.finish_challenge(abandoned);
        r.save()?;
        let mut profile = Profile::load()?;
        let new = profile.record_challenge(&outcome, today);
        profile.save()?;
        if fmt == Format::Json {
            return output::print_json(&json!({"outcome": outcome, "new_badges": new}));
        }
        println!("\n{}", outcome_line(&outcome));
        print_new_badges(&new);
        println!(
            "  {} The replay goes on: {}\n",
            "→".dimmed(),
            "stockwise sim replay status".cyan()
        );
        return Ok(());
    }
    let Some(live) = LiveChallenge::load()? else {
        bail!("No challenge is running. `stockwise sim challenge list` shows them.");
    };
    let history = SimHistory::load()?;
    let sessions = live.sessions(&history);
    let bench = if sessions.is_empty() {
        Vec::new()
    } else {
        live_benchmark(&live).await?
    };
    let lr = live.evaluate(&sessions, &bench);
    let outcome = live.outcome(&lr);
    let mut profile = Profile::load()?;
    let challenge_badges = profile.record_challenge(&outcome, today);
    let card = score::score(&lr.input);
    let facts = score::RunFacts {
        buys: lr.input.buys.len(),
        buys_with_stop: lr.input.buys.iter().filter(|b| b.with_stop).count(),
        hard_break: lr.input.hard_break,
        max_sectors: 0,
        challenge_passed: outcome.passed,
        sip_clean: false,
        full_distance: false,
    };
    let badges: Vec<String> = if card.score.is_some() {
        score::badges(&card, &facts)
            .into_iter()
            .map(String::from)
            .collect()
    } else {
        Vec::new()
    };
    let run = RunResult {
        kind: "live".into(),
        name: "live".into(),
        title: format!("{} (live paper sessions)", live.active.def.title),
        market: live.market.to_lowercase(),
        benchmark_name: live_benchmark_name(&live).into(),
        from: lr.from,
        to: lr.to,
        finished: today,
        disciplined: score::disciplined(&lr.input, &card),
        scorecard: card,
        badges,
        new_badges: Vec::new(),
        challenges: vec![ChallengeSummary::from(&outcome)],
        curve: score::rebased(&lr.input.nav, 60),
        benchmark_curve: score::rebased(&lr.input.benchmark, 60),
    };
    let mut run = if run.scorecard.score.is_some() {
        profile.record_run(run, today)
    } else {
        run
    };
    merge_new_badges(&mut run, challenge_badges);
    profile.save()?;
    LiveChallenge::clear()?;
    if fmt == Format::Json {
        let mut v = run_json(&run, "");
        v["outcome"] = json!(outcome);
        return output::print_json(&v);
    }
    println!("\n{}", outcome_line(&outcome));
    print_scorecard(&run.title, &run, "");
    Ok(())
}

// ── sim profile / sim card ──

pub fn cmd_sim_profile(fmt: Format) -> Result<()> {
    let p = Profile::load()?;
    if fmt == Format::Json {
        let badges: Vec<Value> = score::BADGES
            .iter()
            .map(|b| {
                let earned = p.badges.iter().find(|e| e.id == b.id);
                json!({
                    "id": b.id,
                    "title": t(b.title),
                    "how": t(b.how),
                    "earned": earned.map(|e| e.date),
                    "run": earned.map(|e| e.run.clone()),
                })
            })
            .collect();
        return output::print_json(&json!({
            "badges": badges,
            "streaks": p.streaks,
            "best": p.best,
            "challenges_passed": p.challenges_passed,
            "challenges_failed": p.challenges_failed,
            "runs": p.runs,
            "disclaimer": disclaimer(),
        }));
    }
    print_header("Practice profile");
    if p.runs.is_empty() && p.badges.is_empty() {
        println!(
            "  {} Nothing yet. Play a replay: {}\n",
            "→".dimmed(),
            "stockwise sim replay start covid-crash-2020".cyan()
        );
        return Ok(());
    }
    print_section(t(Msg::PracticeBadges));
    for b in score::BADGES {
        match p.badges.iter().find(|e| e.id == b.id) {
            Some(e) => println!(
                "  {} {}  {}",
                "★".yellow().bold(),
                pad_end(t(b.title), 30).bold(),
                format!("{} · {}", e.date, e.run).dimmed()
            ),
            None => println!(
                "  {} {}  {}",
                "☆".dimmed(),
                pad_end(t(b.title), 30).dimmed(),
                t(b.how).dimmed()
            ),
        }
    }
    print_section(t(Msg::PracticeStreaks));
    print_kv(
        t(Msg::PracticeStreakDisciplined),
        &format!(
            "{} (best {})",
            p.streaks.disciplined.current, p.streaks.disciplined.best
        ),
    );
    print_kv(
        t(Msg::PracticeStreakChallenges),
        &format!(
            "{} (best {}) · {} passed, {} failed",
            p.streaks.challenges.current,
            p.streaks.challenges.best,
            p.challenges_passed,
            p.challenges_failed
        ),
    );
    if !p.best.is_empty() {
        print_section(t(Msg::PracticeBestScores));
        for (name, b) in &p.best {
            print_kv(name, &format!("{} ({}) · {}", b.score, b.grade, b.date));
        }
    }
    if !p.runs.is_empty() {
        print_section("Recent runs");
        for run in p.runs.iter().rev().take(5) {
            let score = run.scorecard.score.map_or("—".to_string(), |s| {
                format!("{s} {}", run.scorecard.grade.clone().unwrap_or_default())
            });
            println!(
                "  {}  {}  {}  {}",
                run.finished.to_string().dimmed(),
                pad_end(&run.title, 34),
                pad_start(&score, 6).bold(),
                format!(
                    "{:+.1}% vs {:+.1}%",
                    run.scorecard.return_percent, run.scorecard.benchmark_return_percent
                )
                .dimmed()
            );
        }
    }
    println!("\n  {}\n", disclaimer().dimmed());
    Ok(())
}

pub fn cmd_sim_card(svg: Option<std::path::PathBuf>, text: bool) -> Result<()> {
    let p = Profile::load()?;
    let Some(run) = p.last_run() else {
        println!(
            "\n  {} No finished run yet. Play one: {}\n",
            "→".dimmed(),
            "stockwise sim replay start covid-crash-2020".cyan()
        );
        return Ok(());
    };
    if let Some(path) = &svg {
        std::fs::write(path, card::svg(run))
            .with_context(|| format!("Cannot write {}", path.display()))?;
    }
    if text || svg.is_none() {
        println!();
        for line in card::text(run) {
            println!("  {line}");
        }
        println!();
    }
    if let Some(path) = &svg {
        println!("  {} Card saved to {}\n", "✓".green(), path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::practice::tape::DayBar;

    const FUTURE: f64 = 4242.42;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Ten days of bars; from day 5 on every price is FUTURE.
    fn bars(base: f64) -> Vec<DayBar> {
        (0..10)
            .map(|i| {
                let p = if i >= 5 { FUTURE } else { base + i as f64 };
                DayBar {
                    date: d("2020-03-02") + chrono::Duration::days(i),
                    open: p,
                    high: p,
                    low: p,
                    close: p,
                }
            })
            .collect()
    }

    fn replay() -> (Replay, Tape) {
        let mut tape = Tape::default();
        tape.insert("AAA.NS", Some("INR".into()), bars(100.0));
        tape.insert("BBB.NS", Some("INR".into()), bars(50.0));
        tape.insert("^NSEI", None, bars(9000.0));
        let s = Scenario {
            name: "leak".into(),
            title: "Leak test".into(),
            market: "in".into(),
            from: d("2020-03-02"),
            to: d("2020-03-11"),
            benchmark: "^NSEI".into(),
            benchmark_name: "NIFTY 50".into(),
            capital: 100_000.0,
            description: String::new(),
            debrief: String::new(),
            universe: vec![
                Member {
                    symbol: "AAA.NS".into(),
                    sector: "IT".into(),
                },
                Member {
                    symbol: "BBB.NS".into(),
                    sector: "Energy".into(),
                },
            ],
        };
        let r = Replay::new(s, &tape, 100_000.0, "auto").unwrap();
        (r, tape)
    }

    fn assert_no_future(text: &str) {
        assert!(!text.contains("4242"), "future price leaked:\n{text}");
        for day in 7..=11 {
            let date = format!("2020-03-{day:02}");
            assert!(!text.contains(&date), "future date {date} leaked:\n{text}");
        }
        assert!(!text.contains("Sat 07 Mar"), "{text}");
    }

    #[test]
    fn no_future_bar_reaches_any_output() {
        let (mut r, tape) = replay();
        r.place(&tape, OrderSide::Buy, "AAA.NS", 100, Some(95.0), None)
            .unwrap();
        // Days 0..4 are the past; stop on day 4 (2020-03-06).
        let mut log = Vec::new();
        for _ in 0..4 {
            log.push(r.step(&tape).unwrap());
        }
        assert_eq!(r.date(), d("2020-03-06"));
        r.place(&tape, OrderSide::Buy, "BBB.NS", 10, None, None)
            .unwrap();

        let json = serde_json::to_string(&status_json(&r, &tape)).unwrap();
        assert_no_future(&json);
        assert_no_future(&status_lines(&r, &tape).join("\n"));
        assert_no_future(&serde_json::to_string(&log).unwrap());
        for events in &log {
            for e in events {
                assert_no_future(&event_line(e, Some("INR")));
            }
        }
        // The score at this point only covers days played.
        let card = r.scorecard();
        assert_eq!(card.days, 4);
        assert_no_future(&serde_json::to_string(&card).unwrap());
        let input = r.run_input();
        assert_eq!(input.nav.len(), 5);
        assert!(input.benchmark.iter().all(|b| *b < 9_100.0));
        // A challenge's view too.
        let mut def = challenge::load("steady-hands").unwrap();
        def.days = 3;
        r.start_challenge(&tape, def).unwrap();
        assert_no_future(&serde_json::to_string(&status_json(&r, &tape)).unwrap());

        // Stepping onto day 5 makes its bar the present, and only then.
        r.step(&tape).unwrap();
        let json = serde_json::to_string(&status_json(&r, &tape)).unwrap();
        assert!(json.contains("4242"));
    }

    #[test]
    fn status_json_shape() {
        let (r, tape) = replay();
        let v = status_json(&r, &tape);
        for key in [
            "scenario",
            "date",
            "day",
            "days",
            "cash",
            "equity",
            "return_percent",
            "benchmark",
            "positions",
            "orders",
            "universe",
            "challenge",
            "disclaimer",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v["day"], 1);
        assert_eq!(v["days"], 10);
        assert_eq!(v["universe"][0]["close"], 100.0);
        assert!(v["universe"][0]["change_percent"].is_null());
    }

    #[test]
    fn custom_scenarios_need_dates_and_symbols() {
        let mut a = ReplayStart {
            scenario: None,
            from: Some(d("2022-01-03")),
            to: Some(d("2022-06-30")),
            symbols: vec!["tcs,INFY".into(), "TCS".into()],
            benchmark: None,
            capital: None,
            costs: "auto".into(),
            fmt: Format::Table,
        };
        let s = custom_scenario(&a, Market::In).unwrap();
        assert_eq!(s.symbols(), ["TCS.NS", "INFY.NS"]);
        assert_eq!(s.benchmark, "^NSEI");
        assert_eq!(custom_scenario(&a, Market::Us).unwrap().benchmark, "^GSPC");
        assert!(custom_scenario(&a, Market::Crypto).is_err());
        // Global exchanges keep their market and benchmark; London's pence
        // prices are refused like the other bots.
        let jp = custom_scenario(&a, Market::Global(market::Exchange::Jp)).unwrap();
        assert_eq!(jp.market, "jp");
        assert_eq!(jp.benchmark, "^N225");
        assert_eq!(scenario_market(&jp), Market::Global(market::Exchange::Jp));
        assert!(custom_scenario(&a, Market::Global(market::Exchange::Uk)).is_err());
        a.to = Some(d("2022-01-05"));
        assert!(custom_scenario(&a, Market::In).is_err());
        a.to = Some(d("2099-01-01"));
        assert!(custom_scenario(&a, Market::In).is_err());
        a.to = Some(d("2022-06-30"));
        a.symbols.clear();
        assert!(custom_scenario(&a, Market::In).is_err());
    }

    #[test]
    fn wrap_breaks_on_spaces() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert!(wrap("", 5).is_empty());
    }
}
