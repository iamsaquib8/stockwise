//! `calendar` — upcoming results, dividends and splits for holdings, the
//! watchlist or named symbols. Sources and caching live in `crate::calendar`.

use super::*;
use crate::calendar::{Calendar, CalendarRow};
use chrono::Utc;

/// Which saved symbols `calendar` looks up when none are named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarScope {
    Portfolio,
    Watchlist,
    Both,
}

/// The symbols to look up: those named, else holdings and/or watchlist.
fn calendar_symbols(
    symbols: &[String],
    scope: CalendarScope,
    market: Market,
) -> Result<Vec<String>> {
    if !symbols.is_empty() {
        let mut out = resolve_symbols(symbols, market);
        out.dedup();
        return Ok(out);
    }
    let mut out: Vec<String> = Vec::new();
    if scope != CalendarScope::Watchlist {
        out.extend(Portfolio::load()?.holdings.into_iter().map(|h| h.symbol));
    }
    if scope != CalendarScope::Portfolio {
        out.extend(Watchlist::load()?.symbols);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.clone()));
    Ok(out)
}

/// Events in the next `days` days for already-resolved `symbols`, soonest
/// first (as rows and as events), and notes on sources that could not
/// answer (for stderr).
pub(crate) async fn calendar_rows(
    client: &YahooClient,
    symbols: &[String],
    days: u32,
) -> (Vec<CalendarRow>, Vec<crate::calendar::Event>, Vec<String>) {
    let mut cal = Calendar::new(client);
    let pending = cal.nse_to_fetch(symbols);
    if pending >= 3 {
        eprintln!(
            "  {} Reading NSE filings for {} stocks (about {} s; cached for a day)…",
            "→".dimmed(),
            pending,
            pending * 2
        );
    }
    let mut events = Vec::new();
    for symbol in symbols {
        events.extend(cal.events_for(symbol).await);
    }
    cal.save();

    let now = Utc::now();
    let today_for = |s: &str| crate::calendar::exchange_today(s, now);
    let upcoming = crate::calendar::upcoming(events, days, today_for);
    let rows = crate::calendar::rows(&upcoming, today_for);

    // Only data on stdout with --json/--csv: notes go to stderr.
    let mut notes = Vec::new();
    if let Some(why) = cal.nse_note()
        && crate::calendar::has_indian(symbols)
    {
        notes.push(why.to_string());
    }
    if !cal.yahoo_missing.is_empty() {
        notes.push(format!(
            "Yahoo Finance had no calendar for {}",
            cal.yahoo_missing.join(", ")
        ));
    }
    (rows, upcoming, notes)
}

/// `calendar`.
pub async fn cmd_calendar(
    symbols: &[String],
    days: u32,
    scope: CalendarScope,
    market: Market,
    fmt: Format,
) -> Result<()> {
    let symbols = calendar_symbols(symbols, scope, market)?;
    if symbols.is_empty() {
        return match fmt {
            Format::Json => output::print_json(&Vec::<CalendarRow>::new()),
            Format::Csv => output::print_csv(&Vec::<CalendarRow>::new()),
            Format::Table => {
                println!("\n  {}\n", t(Msg::CalNoSymbols).dimmed());
                Ok(())
            }
        };
    }

    let client = YahooClient::new().await?;
    let (rows, upcoming, notes) = calendar_rows(&client, &symbols, days).await;
    match fmt {
        Format::Json | Format::Csv => {
            for note in &notes {
                eprintln!("  ! {note}");
            }
            return if fmt == Format::Json {
                output::print_json(&rows)
            } else {
                output::print_csv(&rows)
            };
        }
        Format::Table => {}
    }

    print_header(&tf(Msg::CalendarTitle, &[&days]));
    if rows.is_empty() {
        println!(
            "  {}",
            tf(Msg::CalNoEvents, &[&days, &symbols.len()]).dimmed()
        );
    } else {
        print_calendar_table(&rows, &upcoming);
    }
    println!();
    for note in &notes {
        println!("  {} {}", "!".yellow(), note);
    }
    println!("  {}", t(Msg::CalSources).dimmed());
    if crate::calendar::has_indian(&symbols) {
        println!("  {}", t(Msg::CalSourcesNse).dimmed());
    }
    println!();
    Ok(())
}

fn when_label(row: &CalendarRow) -> String {
    match row.days_until {
        0 => t(Msg::CalToday).to_string(),
        1 => t(Msg::CalTomorrow).to_string(),
        d => tf(Msg::CalInDays, &[&d]),
    }
}

fn print_calendar_table(rows: &[CalendarRow], events: &[crate::calendar::Event]) {
    let width = |f: &dyn Fn(&CalendarRow) -> String, header: &str| {
        rows.iter()
            .map(|r| display_width(&f(r)))
            .chain([display_width(header)])
            .max()
            .unwrap_or(0)
    };
    let (h_date, h_when, h_sym, h_event, h_src, h_details) = (
        t(Msg::CalDate),
        t(Msg::CalWhen),
        t(Msg::Symbol),
        t(Msg::CalEvent),
        t(Msg::CalSource),
        t(Msg::CalDetails),
    );
    let w_when = width(&when_label, h_when);
    let w_sym = width(&|r: &CalendarRow| r.symbol.clone(), h_sym);
    let w_event = width(&|r: &CalendarRow| r.event.label().to_string(), h_event);
    println!(
        "  {}  {}  {}  {}  {}  {}",
        pad_end(h_date, 10).bold(),
        pad_end(h_when, w_when).bold(),
        pad_end(h_sym, w_sym).bold(),
        pad_end(h_event, w_event).bold(),
        pad_end(h_src, 6).bold(),
        h_details.bold()
    );
    for (row, event) in rows.iter().zip(events) {
        let when = pad_end(&when_label(row), w_when);
        let when = match row.when {
            "today" => when.red().bold().to_string(),
            "tomorrow" | "this_week" => when.yellow().to_string(),
            _ => when.dimmed().to_string(),
        };
        let mut detail = event.detail.clone();
        if row.estimate {
            let est = format!("({})", t(Msg::CalEstimated));
            detail = if detail.is_empty() {
                est
            } else {
                format!("{detail} {est}")
            };
        }
        println!(
            "  {}  {}  {}  {}  {}  {}",
            row.date,
            when,
            pad_end(&row.symbol, w_sym).cyan(),
            pad_end(row.event.label(), w_event),
            pad_end(&row.source.to_string(), 6).dimmed(),
            truncate(&detail, 70)
        );
    }
    let soon = rows.iter().filter(|r| r.days_until < 7).count();
    if soon > 0 {
        println!("\n  {}", tf(Msg::CalThisWeek, &[&soon]).yellow());
    }
}

/// At most `max` characters, with an ellipsis when cut.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_details_are_cut_with_an_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }
}
