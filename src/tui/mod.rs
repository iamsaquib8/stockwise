//! `stockwise tui`: a full-screen, read-only dashboard.
//!
//! [`app`] holds the state and the key handling, [`ui`] draws it, and this
//! module runs the loop: terminal events and fetch results go in as
//! messages, and the effects `App::update` asks for become fetches on the
//! shared [`LiveSource`] (one Yahoo client, so one rate limiter and cache).

mod app;
mod ui;

use crate::market::Market;
use crate::views::{self, DataSource, LiveSource};
use crate::watchlist::Watchlist;
use anyhow::{Context, Result, bail};
use app::{App, Effect, Msg, SearchHit, Series};
use ratatui::crossterm::event::{self, Event};
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

pub struct Options {
    pub interval: Duration,
    /// Print one frame of this size and exit instead of running.
    pub snapshot: Option<(u16, u16)>,
}

/// Parse `COLSxROWS`, e.g. `100x30`.
pub fn parse_size(s: &str) -> Result<(u16, u16)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .context("Size must look like 100x30")?;
    let w: u16 = w.trim().parse().context("Bad column count")?;
    let h: u16 = h.trim().parse().context("Bad row count")?;
    if !(20..=500).contains(&w) || !(5..=200).contains(&h) {
        bail!("Size must be between 20x5 and 500x200");
    }
    Ok((w, h))
}

/// Restores the terminal however the loop ends (error, `?`, or normally).
/// Panics are covered by the hook `ratatui::try_init` installs.
struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub async fn run(market: Market, opts: Options) -> Result<()> {
    let source = Arc::new(LiveSource::new());
    if let Some((w, h)) = opts.snapshot {
        return print_frame(&source, market, opts.interval, w, h).await;
    }
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        bail!(
            "`stockwise tui` needs an interactive terminal. For a one-shot view use \
             `stockwise dashboard`; for scripts, the `--json` commands or `stockwise serve`."
        );
    }

    // Live updates: the ticker keeps held and watched quotes warm and tells
    // the loop when they move, without blocking rendering.
    let bus = crate::worker::bus();
    let _ticker = crate::worker::spawn_ticker(market, bus.clone());
    let _tasks = crate::worker::spawn_task_watcher(bus.clone());
    let mut live = bus.subscribe();

    let theme = ui::Theme::from_env();
    let mut app = App::new(market, opts.interval);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Msg>();

    // Before init, so a half-finished init (raw mode on, no alternate
    // screen) is undone too.
    let _restore = RestoreGuard;
    let mut terminal = ratatui::try_init().context("Cannot start the terminal UI")?;
    let stop = Arc::new(AtomicBool::new(false));
    let input = spawn_input(tx.clone(), stop.clone());

    for effect in app.start() {
        spawn_effect(effect, &source, market, &tx);
    }
    let mut refresh =
        tokio::time::interval_at(tokio::time::Instant::now() + app.interval, app.interval);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Redraw every second so "updated" and spinners stay current.
    let mut clock = tokio::time::interval(Duration::from_secs(1));

    let mut terminate = Box::pin(terminate_signal());
    let result: Result<()> = async {
        loop {
            terminal.draw(|f| ui::render(f, &app, &theme))?;
            let msg = tokio::select! {
                Some(msg) = rx.recv() => msg,
                _ = refresh.tick() => Msg::RefreshDue,
                Ok(event) = live.recv() => Msg::Live(event),
                _ = clock.tick() => continue,
                // Signals from outside (raw mode turns the keyboard's Ctrl+C into a key event).
                _ = &mut terminate => break,
            };
            for effect in app.update(msg) {
                spawn_effect(effect, &source, market, &tx);
            }
            if app.quit {
                break;
            }
        }
        Ok(())
    }
    .await;

    stop.store(true, Ordering::Relaxed);
    let _ = input.join();
    result
}

/// SIGINT, SIGTERM or SIGHUP (Ctrl+C only on Windows): leave the loop so
/// the terminal is restored.
async fn terminate_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Terminal events on a plain thread (crossterm's reader blocks), forwarded
/// to the async loop. Polls so it notices `stop` within 200 ms. If the
/// terminal can't be read any more, the app quits rather than ignore keys.
fn spawn_input(tx: UnboundedSender<Msg>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let msg = match event::poll(Duration::from_millis(200)) {
                Ok(false) => continue,
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) => Msg::Key(key),
                    Ok(Event::Resize(..)) => Msg::Redraw,
                    Ok(_) => continue,
                    Err(_) => Msg::InputClosed,
                },
                Err(_) => Msg::InputClosed,
            };
            let closed = matches!(msg, Msg::InputClosed);
            if tx.send(msg).is_err() || closed {
                break;
            }
        }
    })
}

fn chart_series(
    hist: views::History,
    symbol: &str,
    period: &'static str,
) -> std::result::Result<Series, String> {
    let (times, closes): (Vec<i64>, Vec<f64>) = hist
        .bars
        .iter()
        .filter_map(|b| b.close.map(|c| (b.ts, c)))
        .unzip();
    if closes.is_empty() {
        return Err(format!("no prices for {} over {}", symbol, period));
    }
    Ok(Series {
        symbol: symbol.to_string(),
        period,
        closes,
        times,
        currency: hist.currency,
    })
}

async fn load_chart(
    src: &impl DataSource,
    symbol: &str,
    period: &'static str,
) -> std::result::Result<Series, String> {
    let (range, interval) = views::history_range(period);
    let chart = src
        .chart(symbol, range, interval)
        .await
        .map_err(|e| format!("{:#}", e))?;
    chart_series(
        views::history_from_chart(chart, symbol, period, interval),
        symbol,
        period,
    )
}

fn edit_watchlist(symbol: &str, add: bool) -> std::result::Result<String, String> {
    let run = || -> Result<String> {
        let mut wl = Watchlist::load()?;
        let changed = if add {
            wl.add(symbol)
        } else {
            wl.remove(symbol)
        };
        if changed {
            wl.save()?;
        }
        Ok(match (add, changed) {
            (true, true) => format!("Added {} to the watchlist", symbol),
            (true, false) => format!("{} is already in the watchlist", symbol),
            (false, true) => format!("Removed {} — press / to add it back", symbol),
            (false, false) => format!("{} was not in the watchlist", symbol),
        })
    };
    run().map_err(|e| format!("Could not update the watchlist: {:#}", e))
}

fn spawn_effect(
    effect: Effect,
    source: &Arc<LiveSource>,
    market: Market,
    tx: &UnboundedSender<Msg>,
) {
    let src = source.clone();
    let tx = tx.clone();
    match effect {
        Effect::Refresh => {
            tokio::spawn(async move {
                let result = views::snapshot(&*src, market)
                    .await
                    .map_err(|e| format!("{:#}", e));
                let _ = tx.send(Msg::Snapshot(result));
            });
        }
        Effect::LoadChart { symbol, period } => {
            tokio::spawn(async move {
                let result = load_chart(&*src, &symbol, period).await;
                let _ = tx.send(Msg::Chart {
                    symbol,
                    period,
                    result,
                });
            });
        }
        Effect::Search(query) => {
            tokio::spawn(async move {
                let result = src
                    .search(&query)
                    .await
                    .map(|found| {
                        found
                            .into_iter()
                            .filter_map(|r| {
                                Some(SearchHit {
                                    symbol: r.symbol?,
                                    name: r.long_name.or(r.short_name).unwrap_or_default(),
                                    exchange: r.exchange.unwrap_or_default(),
                                })
                            })
                            .collect()
                    })
                    .map_err(|e| format!("{:#}", e));
                let _ = tx.send(Msg::SearchDone { query, result });
            });
        }
        // Small local file writes: done inline.
        Effect::AddToWatchlist(symbol) => {
            let _ = tx.send(Msg::WatchlistSaved(edit_watchlist(&symbol, true)));
        }
        Effect::RemoveFromWatchlist(symbol) => {
            let _ = tx.send(Msg::WatchlistSaved(edit_watchlist(&symbol, false)));
        }
    }
}

/// `tui --snapshot COLSxROWS`: fetch once, draw one frame, print it.
async fn print_frame(
    source: &LiveSource,
    market: Market,
    interval: Duration,
    w: u16,
    h: u16,
) -> Result<()> {
    let mut app = App::new(market, interval);
    app.start();
    let snap = views::snapshot(source, market).await?;
    app.update(Msg::Snapshot(Ok(snap)));
    if let Some(symbol) = app.selected_symbol() {
        let period = app.chart_period();
        let result = load_chart(source, &symbol, period).await;
        app.update(Msg::Chart {
            symbol,
            period,
            result,
        });
    }
    let theme = ui::Theme::from_env();
    let color = !ui::no_color()
        && (std::io::stdout().is_terminal() || std::env::var_os("CLICOLOR_FORCE").is_some());
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h))?;
    term.draw(|f| ui::render(f, &app, &theme))?;
    print!("{}", ui::to_ansi(term.backend().buffer(), color));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::fixtures::Fixture;

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("100x30").unwrap(), (100, 30));
        assert_eq!(parse_size("80X24").unwrap(), (80, 24));
        assert!(parse_size("80").is_err());
        assert!(parse_size("5x5").is_err());
        assert!(parse_size("axb").is_err());
    }

    #[tokio::test]
    async fn charts_load_from_the_data_source() {
        let f = Fixture::default();
        let s = load_chart(&f, "TCS.NS", "1mo").await.unwrap();
        assert_eq!(s.closes, vec![100.0, 102.0, 101.0, 105.0, 107.5]);
        assert_eq!(s.times.len(), 5);
        assert_eq!(s.currency.as_deref(), Some("INR"));
        assert!(load_chart(&f, "NOPE.NS", "1mo").await.is_err());
    }

    #[test]
    fn bars_without_a_close_are_skipped() {
        let mut hist = views::history_from_chart(
            crate::views::fixtures::chart(&[1.0, 2.0, 3.0], 0, 60, "USD"),
            "X",
            "1d",
            "5m",
        );
        hist.bars[1].close = None;
        let s = chart_series(hist.clone(), "X", "1d").unwrap();
        assert_eq!(s.closes, vec![1.0, 3.0]);
        assert_eq!(s.times, vec![0, 120]);
        hist.bars.iter_mut().for_each(|b| b.close = None);
        assert!(chart_series(hist, "X", "1d").is_err());
    }
}
