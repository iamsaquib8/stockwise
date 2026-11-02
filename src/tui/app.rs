//! TUI state and its update logic, kept free of I/O so it can be tested
//! without a terminal or the network. The runtime (`mod.rs`) turns the
//! [`Effect`]s that `update` returns into fetches and file writes, and feeds
//! the results back in as [`Msg`]s.

use crate::market::Market;
use crate::views::Snapshot;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Duration;

/// Chart periods, selected with the keys 1-6 or `[` / `]`.
pub const CHART_PERIODS: [&str; 6] = ["1d", "5d", "1mo", "6mo", "1y", "5y"];
const DEFAULT_PERIOD: usize = 2;
const MAX_QUERY: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Watchlist,
    Portfolio,
}

/// Closing prices of one symbol over one period.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub symbol: String,
    pub period: &'static str,
    pub closes: Vec<f64>,
    /// Unix seconds of each close.
    pub times: Vec<i64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub symbol: String,
    pub name: String,
    pub exchange: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub input: String,
    pub results: Vec<SearchHit>,
    pub selected: usize,
    /// A search for `input` is in flight.
    pub busy: bool,
    /// `results` belong to the current `input`.
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Help,
    Detail,
    Search(Search),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    #[default]
    Default,
    Symbol,
    ChangePct,
    Value,
    PnlPct,
}

impl SortBy {
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Symbol,
            Self::Symbol => Self::ChangePct,
            Self::ChangePct => Self::Value,
            Self::Value => Self::PnlPct,
            Self::PnlPct => Self::Default,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default order",
            Self::Symbol => "symbol (A–Z)",
            Self::ChangePct => "day change %",
            Self::Value => "holding value",
            Self::PnlPct => "P&L %",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
    /// The refresh timer fired.
    RefreshDue,
    /// Something changed on screen (e.g. a resize); just redraw.
    Redraw,
    /// The terminal can no longer be read; quit.
    InputClosed,
    Snapshot(Result<Snapshot, String>),
    Chart {
        symbol: String,
        period: &'static str,
        result: Result<Series, String>,
    },
    SearchDone {
        query: String,
        result: Result<Vec<SearchHit>, String>,
    },
    /// A watchlist edit finished: the message to show, or an error.
    WatchlistSaved(Result<String, String>),
}

/// Work for the runtime. Nothing here trades: the TUI is read-only apart
/// from editing the local watchlist.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Refresh,
    LoadChart {
        symbol: String,
        period: &'static str,
    },
    Search(String),
    AddToWatchlist(String),
    RemoveFromWatchlist(String),
}

/// Listed on the selected market's exchanges: NSE/BSE for India, a bare
/// ticker for the US.
fn home_listing(symbol: &str, market: Market) -> bool {
    match market {
        Market::In => symbol.ends_with(".NS") || symbol.ends_with(".BO"),
        Market::Us => !symbol.contains('.'),
        Market::Crypto => symbol.contains('-'),
        Market::Fx => symbol.ends_with("=X"),
        Market::Commodity => symbol.ends_with("=F") || symbol.ends_with(".NS"),
        Market::Global(x) => symbol.ends_with(x.info().suffix),
    }
}

pub struct App {
    pub market: Market,
    pub interval: Duration,
    pub snapshot: Option<Snapshot>,
    pub refreshing: bool,
    /// A refresh was asked for while one was running.
    refresh_queued: bool,
    pub last_refresh: Option<chrono::DateTime<chrono::Local>>,
    pub focus: Focus,
    pub watch_sel: usize,
    pub hold_sel: usize,
    pub period: usize,
    pub chart: Option<Series>,
    pub chart_error: Option<String>,
    pub chart_loading: bool,
    pub mode: Mode,
    pub sort: SortBy,
    pub status: Option<Status>,
    pub quit: bool,
}

impl App {
    pub fn new(market: Market, interval: Duration) -> Self {
        Self {
            market,
            interval,
            snapshot: None,
            refreshing: false,
            refresh_queued: false,
            last_refresh: None,
            focus: Focus::Watchlist,
            watch_sel: 0,
            hold_sel: 0,
            period: DEFAULT_PERIOD,
            chart: None,
            chart_error: None,
            chart_loading: false,
            mode: Mode::Normal,
            sort: SortBy::Default,
            status: None,
            quit: false,
        }
    }

    /// The first refresh, issued by the runtime at start-up.
    pub fn start(&mut self) -> Vec<Effect> {
        self.request_refresh()
    }

    pub fn chart_period(&self) -> &'static str {
        CHART_PERIODS[self.period]
    }

    fn list_len(&self, focus: Focus) -> usize {
        self.snapshot.as_ref().map_or(0, |s| match focus {
            Focus::Watchlist => s.watchlist.len(),
            Focus::Portfolio => s.portfolio.holdings.len(),
        })
    }

    /// The symbol the chart follows: the selected row of the focused list,
    /// or the first index while that list is empty.
    pub fn selected_symbol(&self) -> Option<String> {
        let s = self.snapshot.as_ref()?;
        let row = match self.focus {
            Focus::Watchlist => s.watchlist.get(self.watch_sel).map(|r| r.symbol.clone()),
            Focus::Portfolio => s
                .portfolio
                .holdings
                .get(self.hold_sel)
                .map(|r| r.symbol.clone()),
        };
        row.or_else(|| s.indices.first().map(|i| i.symbol.clone()))
    }

    fn request_refresh(&mut self) -> Vec<Effect> {
        if self.refreshing {
            self.refresh_queued = true;
            Vec::new()
        } else {
            self.refreshing = true;
            vec![Effect::Refresh]
        }
    }

    /// Load the chart for the current selection and period, unless it is
    /// already shown (`force` reloads anyway, e.g. on refresh).
    fn chart_effect(&mut self, force: bool) -> Vec<Effect> {
        let Some(symbol) = self.selected_symbol() else {
            return Vec::new();
        };
        let period = self.chart_period();
        let shown = self
            .chart
            .as_ref()
            .is_some_and(|c| c.symbol == symbol && c.period == period);
        if shown && !force {
            // Back on a chart that is already drawn: an answer for the symbol
            // just left is ignored, so nothing is loading any more.
            self.chart_loading = false;
            return Vec::new();
        }
        if !shown {
            self.chart_error = None;
        }
        self.chart_loading = true;
        vec![Effect::LoadChart { symbol, period }]
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
        });
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::RefreshDue => self.request_refresh(),
            Msg::Redraw => Vec::new(),
            Msg::InputClosed => {
                self.quit = true;
                Vec::new()
            }
            Msg::Snapshot(result) => {
                self.refreshing = false;
                let mut effects = Vec::new();
                match result {
                    Ok(snap) => {
                        self.watch_sel = self.watch_sel.min(snap.watchlist.len().saturating_sub(1));
                        self.hold_sel = self
                            .hold_sel
                            .min(snap.portfolio.holdings.len().saturating_sub(1));
                        // Start on the portfolio when there is nothing to watch.
                        if self.snapshot.is_none()
                            && snap.watchlist.is_empty()
                            && !snap.portfolio.holdings.is_empty()
                        {
                            self.focus = Focus::Portfolio;
                        }
                        self.snapshot = Some(snap);
                        self.apply_sort();
                        self.last_refresh = Some(chrono::Local::now());
                        if self.status.as_ref().is_some_and(|s| s.error) {
                            self.status = None;
                        }
                        effects.extend(self.chart_effect(true));
                    }
                    Err(e) => self.error(format!("Refresh failed: {}", e)),
                }
                if std::mem::take(&mut self.refresh_queued) {
                    effects.extend(self.request_refresh());
                }
                effects
            }
            Msg::Chart {
                symbol,
                period,
                result,
            } => {
                // Ignore answers for a selection the user already moved away from.
                if Some(&symbol) != self.selected_symbol().as_ref() || period != self.chart_period()
                {
                    return Vec::new();
                }
                self.chart_loading = false;
                match result {
                    Ok(series) => {
                        self.chart = Some(series);
                        self.chart_error = None;
                    }
                    Err(e) => {
                        // Keep a stale chart of the same symbol rather than blanking it.
                        if self
                            .chart
                            .as_ref()
                            .is_none_or(|c| c.symbol != symbol || c.period != period)
                        {
                            self.chart = None;
                        }
                        self.chart_error = Some(e);
                    }
                }
                Vec::new()
            }
            Msg::SearchDone { query, result } => {
                if let Mode::Search(search) = &mut self.mode
                    && search.input == query
                {
                    search.busy = false;
                    match result {
                        Ok(mut hits) => {
                            // Home-market listings first; Yahoo's order otherwise.
                            hits.sort_by_key(|h| !home_listing(&h.symbol, self.market));
                            search.done = true;
                            search.selected = 0;
                            search.results = hits;
                        }
                        Err(e) => {
                            self.error(format!("Search failed: {}", e));
                        }
                    }
                }
                Vec::new()
            }
            Msg::WatchlistSaved(result) => match result {
                Ok(text) => {
                    self.info(text);
                    self.request_refresh()
                }
                Err(e) => {
                    self.error(e);
                    Vec::new()
                }
            },
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        // Windows also reports key releases.
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return Vec::new();
        }
        match &mut self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                Vec::new()
            }
            Mode::Detail => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('d') | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                    Vec::new()
                }
                KeyCode::Char(c @ '1'..='6') => {
                    self.period = c as usize - '1' as usize;
                    self.chart_effect(false)
                }
                KeyCode::Char('[') => {
                    self.period = (self.period + CHART_PERIODS.len() - 1) % CHART_PERIODS.len();
                    self.chart_effect(false)
                }
                KeyCode::Char(']') => {
                    self.period = (self.period + 1) % CHART_PERIODS.len();
                    self.chart_effect(false)
                }
                _ => Vec::new(),
            },
            Mode::Search(_) => self.on_search_key(key),
            Mode::Normal => self.on_normal_key(key),
        }
    }

    fn on_normal_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if !matches!(key.code, KeyCode::Char('r')) && self.status.as_ref().is_some_and(|s| !s.error)
        {
            self.status = None;
        }
        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                Vec::new()
            }
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.mode = Mode::Help;
                Vec::new()
            }
            KeyCode::Char('d') | KeyCode::Enter => {
                if self.selected_symbol().is_some() {
                    self.mode = Mode::Detail;
                }
                Vec::new()
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.apply_sort();
                self.info(format!("Sorted by {}", self.sort.label()));
                self.chart_effect(false)
            }
            KeyCode::Char('/') | KeyCode::Char('a') => {
                self.mode = Mode::Search(Search::default());
                Vec::new()
            }
            KeyCode::Char('r') => {
                self.info("Refreshing…");
                self.request_refresh()
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Watchlist => Focus::Portfolio,
                    Focus::Portfolio => Focus::Watchlist,
                };
                self.chart_effect(false)
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(isize::MAX),
            KeyCode::Char(c @ '1'..='6') => {
                self.period = c as usize - '1' as usize;
                self.chart_effect(false)
            }
            KeyCode::Char('[') => {
                self.period = (self.period + CHART_PERIODS.len() - 1) % CHART_PERIODS.len();
                self.chart_effect(false)
            }
            KeyCode::Char(']') => {
                self.period = (self.period + 1) % CHART_PERIODS.len();
                self.chart_effect(false)
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if self.focus != Focus::Watchlist {
                    self.info("Select the watchlist (Tab) to remove a symbol");
                    return Vec::new();
                }
                let symbol = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.watchlist.get(self.watch_sel))
                    .map(|r| r.symbol.clone());
                match symbol {
                    Some(s) => vec![Effect::RemoveFromWatchlist(s)],
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn move_selection(&mut self, delta: isize) -> Vec<Effect> {
        let len = self.list_len(self.focus);
        if len == 0 {
            return Vec::new();
        }
        let sel = match self.focus {
            Focus::Watchlist => &mut self.watch_sel,
            Focus::Portfolio => &mut self.hold_sel,
        };
        *sel = sel.saturating_add_signed(delta).min(len - 1);
        self.chart_effect(false)
    }

    fn apply_sort(&mut self) {
        let Some(snap) = &mut self.snapshot else {
            return;
        };
        let cmp_desc = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        match self.sort {
            SortBy::Default => {}
            SortBy::Symbol => {
                snap.watchlist.sort_by(|a, b| a.symbol.cmp(&b.symbol));
                snap.portfolio
                    .holdings
                    .sort_by(|a, b| a.symbol.cmp(&b.symbol));
            }
            SortBy::ChangePct => {
                snap.watchlist
                    .sort_by(|a, b| cmp_desc(a.change_percent, b.change_percent));
                snap.portfolio
                    .holdings
                    .sort_by(|a, b| cmp_desc(a.pnl_percent, b.pnl_percent));
            }
            SortBy::Value => {
                snap.watchlist.sort_by(|a, b| cmp_desc(a.price, b.price));
                snap.portfolio
                    .holdings
                    .sort_by(|a, b| cmp_desc(a.value, b.value));
            }
            SortBy::PnlPct => {
                snap.watchlist
                    .sort_by(|a, b| cmp_desc(a.change_percent, b.change_percent));
                snap.portfolio
                    .holdings
                    .sort_by(|a, b| cmp_desc(a.pnl_percent, b.pnl_percent));
            }
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Mode::Search(search) = &mut self.mode else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                Vec::new()
            }
            KeyCode::Up => {
                search.selected = search.selected.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Down => {
                if search.selected + 1 < search.results.len() {
                    search.selected += 1;
                }
                Vec::new()
            }
            KeyCode::Enter => {
                if search.done && !search.results.is_empty() {
                    let symbol = search.results[search.selected].symbol.clone();
                    self.mode = Mode::Normal;
                    vec![Effect::AddToWatchlist(symbol)]
                } else if !search.input.trim().is_empty() && !search.busy {
                    search.busy = true;
                    vec![Effect::Search(search.input.trim().to_string())]
                } else {
                    Vec::new()
                }
            }
            // Editing the query drops results and any search in flight (its
            // answer will no longer match the input and is ignored).
            KeyCode::Backspace => {
                search.input.pop();
                search.done = false;
                search.busy = false;
                search.results.clear();
                Vec::new()
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if search.input.chars().count() < MAX_QUERY {
                    search.input.push(c);
                    search.done = false;
                    search.busy = false;
                    search.results.clear();
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::views::fixtures::Fixture;

    pub fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    pub fn snapshot() -> Snapshot {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(crate::views::snapshot(&Fixture::default(), Market::In))
            .unwrap()
    }

    pub fn series(symbol: &str, period: &'static str) -> Series {
        Series {
            symbol: symbol.into(),
            period,
            closes: vec![100.0, 104.0, 102.0, 108.0],
            times: vec![1_700_000_000, 1_700_086_400, 1_700_172_800, 1_700_259_200],
            currency: Some("INR".into()),
        }
    }

    /// An app that has received one snapshot and the matching chart.
    pub fn loaded() -> App {
        let mut app = App::new(Market::In, Duration::from_secs(30));
        assert_eq!(app.start(), vec![Effect::Refresh]);
        app.update(Msg::Snapshot(Ok(snapshot())));
        app.update(Msg::Chart {
            symbol: "RELIANCE.NS".into(),
            period: "1mo",
            result: Ok(series("RELIANCE.NS", "1mo")),
        });
        app
    }

    #[test]
    fn first_snapshot_loads_the_selected_chart() {
        let mut app = App::new(Market::In, Duration::from_secs(30));
        app.start();
        assert!(app.refreshing);
        let effects = app.update(Msg::Snapshot(Ok(snapshot())));
        assert_eq!(
            effects,
            vec![Effect::LoadChart {
                symbol: "RELIANCE.NS".into(),
                period: "1mo"
            }]
        );
        assert!(!app.refreshing);
        assert!(app.last_refresh.is_some());
        assert!(app.chart_loading);
    }

    #[test]
    fn refreshes_do_not_overlap() {
        let mut app = App::new(Market::In, Duration::from_secs(30));
        assert_eq!(app.update(Msg::RefreshDue), vec![Effect::Refresh]);
        assert!(app.update(Msg::RefreshDue).is_empty());
        assert!(app.update(key(KeyCode::Char('r'))).is_empty());
        // The queued refresh runs once the first finishes.
        let effects = app.update(Msg::Snapshot(Ok(snapshot())));
        assert!(effects.contains(&Effect::Refresh));
        assert!(app.refreshing);
    }

    #[test]
    fn failed_refresh_keeps_the_last_data() {
        let mut app = loaded();
        app.update(Msg::RefreshDue);
        app.update(Msg::Snapshot(Err("offline".into())));
        assert!(app.snapshot.is_some());
        let status = app.status.clone().unwrap();
        assert!(status.error);
        assert!(status.text.contains("offline"));
        // The next good refresh clears the error.
        app.update(Msg::RefreshDue);
        app.update(Msg::Snapshot(Ok(snapshot())));
        assert!(app.status.is_none());
    }

    #[test]
    fn selection_moves_and_the_chart_follows() {
        let mut app = loaded();
        assert_eq!(app.selected_symbol().as_deref(), Some("RELIANCE.NS"));
        let effects = app.update(key(KeyCode::Down));
        assert_eq!(app.watch_sel, 1);
        assert_eq!(
            effects,
            vec![Effect::LoadChart {
                symbol: "TCS.NS".into(),
                period: "1mo"
            }]
        );
        // Clamped at the end of the list.
        app.update(key(KeyCode::Down));
        assert_eq!(app.watch_sel, 1);
        app.update(key(KeyCode::Char('g')));
        assert_eq!(app.watch_sel, 0);
        // A late chart for the old selection is ignored.
        app.update(key(KeyCode::Char('j')));
        app.update(Msg::Chart {
            symbol: "RELIANCE.NS".into(),
            period: "1mo",
            result: Ok(series("RELIANCE.NS", "1mo")),
        });
        assert!(app.chart_loading);
    }

    #[test]
    fn tab_switches_to_the_portfolio() {
        let mut app = loaded();
        let effects = app.update(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Portfolio);
        assert_eq!(
            effects,
            vec![Effect::LoadChart {
                symbol: "TCS.NS".into(),
                period: "1mo"
            }]
        );
        app.update(key(KeyCode::End));
        assert_eq!(app.hold_sel, 2);
        assert_eq!(app.selected_symbol().as_deref(), Some("DELISTED.NS"));
    }

    #[test]
    fn period_keys() {
        let mut app = loaded();
        let effects = app.update(key(KeyCode::Char('5')));
        assert_eq!(app.chart_period(), "1y");
        assert_eq!(
            effects,
            vec![Effect::LoadChart {
                symbol: "RELIANCE.NS".into(),
                period: "1y"
            }]
        );
        app.update(key(KeyCode::Char(']')));
        assert_eq!(app.chart_period(), "5y");
        app.update(key(KeyCode::Char(']')));
        assert_eq!(app.chart_period(), "1d");
        app.update(key(KeyCode::Char('[')));
        assert_eq!(app.chart_period(), "5y");
        // Once the chart is shown, pressing its period again does nothing.
        app.update(Msg::Chart {
            symbol: "RELIANCE.NS".into(),
            period: "5y",
            result: Ok(series("RELIANCE.NS", "5y")),
        });
        assert!(app.update(key(KeyCode::Char('6'))).is_empty());
    }

    #[test]
    fn chart_errors_are_shown_not_fatal() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('1')));
        app.update(Msg::Chart {
            symbol: "RELIANCE.NS".into(),
            period: "1d",
            result: Err("No chart data returned".into()),
        });
        assert!(app.chart.is_none());
        assert_eq!(app.chart_error.as_deref(), Some("No chart data returned"));
        assert!(!app.chart_loading);
    }

    #[test]
    fn search_then_add_to_watchlist() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('/')));
        assert!(matches!(app.mode, Mode::Search(_)));
        for c in "tata".chars() {
            app.update(key(KeyCode::Char(c)));
        }
        // 'q' types into the query instead of quitting.
        app.update(key(KeyCode::Char('q')));
        app.update(key(KeyCode::Backspace));
        assert!(!app.quit);
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::Search("tata".into())]
        );
        // Enter while searching does not search twice.
        assert!(app.update(key(KeyCode::Enter)).is_empty());
        let hit = |s: &str| SearchHit {
            symbol: s.into(),
            name: s.into(),
            exchange: "NSI".into(),
        };
        // An answer for an older query is ignored.
        app.update(Msg::SearchDone {
            query: "ta".into(),
            result: Ok(vec![hit("TA.NS")]),
        });
        app.update(Msg::SearchDone {
            query: "tata".into(),
            result: Ok(vec![
                hit("TTST.IL"),
                hit("TATAMOTORS.NS"),
                hit("TATASTEEL.NS"),
            ]),
        });
        // NSE listings are ranked above the London one.
        let Mode::Search(s) = &app.mode else {
            panic!("still searching")
        };
        let order: Vec<&str> = s.results.iter().map(|h| h.symbol.as_str()).collect();
        assert_eq!(order, ["TATAMOTORS.NS", "TATASTEEL.NS", "TTST.IL"]);
        app.update(key(KeyCode::Down));
        let effects = app.update(key(KeyCode::Enter));
        assert_eq!(effects, vec![Effect::AddToWatchlist("TATASTEEL.NS".into())]);
        assert_eq!(app.mode, Mode::Normal);
        let effects = app.update(Msg::WatchlistSaved(Ok("Added TATASTEEL.NS".into())));
        assert_eq!(effects, vec![Effect::Refresh]);
        assert_eq!(app.status.as_ref().unwrap().text, "Added TATASTEEL.NS");
    }

    #[test]
    fn editing_during_a_search_allows_a_new_one() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('/')));
        app.update(key(KeyCode::Char('t')));
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::Search("t".into())]
        );
        app.update(key(KeyCode::Char('c')));
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::Search("tc".into())]
        );
        // The late answer for "t" is ignored; the one for "tc" lands.
        app.update(Msg::SearchDone {
            query: "t".into(),
            result: Ok(vec![]),
        });
        let Mode::Search(s) = &app.mode else {
            panic!("still searching")
        };
        assert!(s.busy && !s.done);
    }

    #[test]
    fn returning_to_a_shown_chart_stops_loading() {
        let mut app = loaded();
        app.update(key(KeyCode::Down));
        assert!(app.chart_loading);
        assert!(app.update(key(KeyCode::Up)).is_empty());
        assert!(!app.chart_loading);
    }

    #[test]
    fn escape_cancels_search_and_help() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('a')));
        app.update(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        app.update(key(KeyCode::Char('?')));
        assert_eq!(app.mode, Mode::Help);
        // Any key closes help; q does not quit from help.
        app.update(key(KeyCode::Char('q')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.quit);
    }

    #[test]
    fn remove_only_from_the_watchlist() {
        let mut app = loaded();
        assert_eq!(
            app.update(key(KeyCode::Char('x'))),
            vec![Effect::RemoveFromWatchlist("RELIANCE.NS".into())]
        );
        app.update(key(KeyCode::Tab));
        assert!(app.update(key(KeyCode::Delete)).is_empty());
        assert!(app.status.is_some());
    }

    #[test]
    fn quit_keys() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('q')));
        assert!(app.quit);
        let mut app = loaded();
        app.update(key(KeyCode::Char('/')));
        app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.quit, "Ctrl+C quits even while typing a search");
    }

    #[test]
    fn selection_is_clamped_when_the_watchlist_shrinks() {
        let mut app = loaded();
        app.update(key(KeyCode::Down));
        let mut snap = snapshot();
        snap.watchlist.truncate(1);
        app.update(Msg::RefreshDue);
        app.update(Msg::Snapshot(Ok(snap)));
        assert_eq!(app.watch_sel, 0);
    }

    #[test]
    fn empty_lists_fall_back_to_the_first_index() {
        let mut app = App::new(Market::In, Duration::from_secs(30));
        app.start();
        let mut snap = snapshot();
        snap.watchlist.clear();
        snap.portfolio = Default::default();
        let effects = app.update(Msg::Snapshot(Ok(snap)));
        assert_eq!(
            effects,
            vec![Effect::LoadChart {
                symbol: "^NSEI".into(),
                period: "1mo"
            }]
        );
        assert!(app.update(key(KeyCode::Down)).is_empty());
        assert!(app.update(key(KeyCode::Char('x'))).is_empty());
    }

    #[test]
    fn starts_on_the_portfolio_when_nothing_is_watched() {
        let mut app = App::new(Market::In, Duration::from_secs(30));
        app.start();
        let mut snap = snapshot();
        snap.watchlist.clear();
        app.update(Msg::Snapshot(Ok(snap)));
        assert_eq!(app.focus, Focus::Portfolio);
        assert_eq!(app.selected_symbol().as_deref(), Some("TCS.NS"));
    }

    #[test]
    fn detail_modal_and_sort_cycling() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('d')));
        assert_eq!(app.mode, Mode::Detail);
        // 'q' closes the detail modal without quitting the app.
        app.update(key(KeyCode::Char('q')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.quit);

        // 's' cycles sort order.
        app.update(key(KeyCode::Char('s')));
        assert_eq!(app.sort, SortBy::Symbol);
        app.update(key(KeyCode::Char('s')));
        assert_eq!(app.sort, SortBy::ChangePct);
        app.update(key(KeyCode::Char('s')));
        assert_eq!(app.sort, SortBy::Value);
        app.update(key(KeyCode::Char('s')));
        assert_eq!(app.sort, SortBy::PnlPct);
        app.update(key(KeyCode::Char('s')));
        assert_eq!(app.sort, SortBy::Default);
    }
}
