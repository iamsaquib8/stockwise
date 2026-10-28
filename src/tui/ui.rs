//! Drawing the TUI. Pure functions of [`App`]: nothing here fetches data or
//! changes state, so a frame can be rendered into a test buffer.

use super::app::{App, CHART_PERIODS, Focus, Mode, Series};
use crate::alerts::{Alert, AlertCondition};
use crate::display::format_price;
use crate::views::{AlertCheckRow, IndexRow};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Axis, Block, Cell, Chart, Clear, Dataset, GraphType, Paragraph, Row, Table, TableState, Wrap,
};

/// Below this the full layout (with the chart) doesn't fit.
pub const FULL_SIZE: (u16, u16) = (80, 24);
/// Below this nothing useful fits.
pub const MIN_SIZE: (u16, u16) = (40, 12);

/// Styles for one frame. With `NO_COLOR` set, only bold/reverse are used and
/// direction is carried by ▲/▼ and +/− signs.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub up: Style,
    pub down: Style,
    pub accent: Style,
    pub dim: Style,
    pub warn: Style,
    pub title: Style,
    pub selected: Style,
    pub focus_border: Style,
    pub border: Style,
}

impl Theme {
    pub fn new(color: bool) -> Self {
        let plain = Style::default();
        let bold = plain.add_modifier(Modifier::BOLD);
        if color {
            Self {
                up: plain.fg(Color::Green),
                down: plain.fg(Color::Red),
                accent: plain.fg(Color::Cyan),
                dim: plain.fg(Color::DarkGray),
                warn: bold.fg(Color::Yellow),
                title: bold.fg(Color::Cyan),
                selected: plain.add_modifier(Modifier::REVERSED),
                focus_border: plain.fg(Color::Cyan),
                border: plain.fg(Color::DarkGray),
            }
        } else {
            Self {
                up: plain,
                down: plain,
                accent: plain,
                dim: plain,
                warn: bold,
                title: bold,
                selected: plain.add_modifier(Modifier::REVERSED),
                focus_border: bold,
                border: plain,
            }
        }
    }

    /// Colour unless `NO_COLOR` is set (see [`no_color`]).
    pub fn from_env() -> Self {
        Self::new(!no_color())
    }

    fn signed(&self, v: f64) -> Style {
        if v >= 0.0 { self.up } else { self.down }
    }
}

/// True when `NO_COLOR` is set to a non-empty value (no-color.org).
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// `RELIANCE.NS` → `RELIANCE`: the NSE suffix is implied in the TUI.
fn short(symbol: &str) -> &str {
    symbol.strip_suffix(".NS").unwrap_or(symbol)
}

fn pct_text(pct: Option<f64>) -> String {
    match pct {
        Some(p) if p >= 0.0 => format!("▲{:.2}%", p),
        Some(p) => format!("▼{:.2}%", p.abs()),
        None => "—".into(),
    }
}

fn pct_span(pct: Option<f64>, theme: &Theme) -> Span<'static> {
    let style = pct.map_or(theme.dim, |p| theme.signed(p));
    Span::styled(pct_text(pct), style)
}

fn price_text(v: Option<f64>, currency: Option<&str>) -> String {
    v.map_or_else(|| "N/A".into(), |v| format_price(v, currency))
}

fn signed_money(v: f64, currency: Option<&str>) -> String {
    let sign = if v >= 0.0 { "+" } else { "-" };
    format!("{}{}", sign, format_price(v.abs(), currency))
}

pub fn render(f: &mut Frame, app: &App, theme: &Theme) {
    let area = f.area();
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        render_too_small(f, area, theme);
        return;
    }
    let full = area.width >= FULL_SIZE.0 && area.height >= FULL_SIZE.1;
    let [header, indices, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(f, header, app, theme);
    render_indices(f, indices, app, theme);
    if full {
        let [top, bottom] =
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);
        let watch_w = (area.width * 2 / 5).clamp(34, 56);
        let [watch, chart] =
            Layout::horizontal([Constraint::Length(watch_w), Constraint::Min(0)]).areas(top);
        let [port, alerts] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(bottom);
        render_watchlist(f, watch, app, theme);
        render_chart(f, chart, app, theme);
        render_portfolio(f, port, app, theme);
        render_alerts(f, alerts, app, theme);
    } else {
        // Compact: no chart, the two lists stacked, alerts as one line.
        let [watch, port, alerts] = Layout::vertical([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Length(1),
        ])
        .areas(body);
        render_watchlist(f, watch, app, theme);
        render_portfolio(f, port, app, theme);
        render_alert_summary(f, alerts, app, theme);
    }
    render_footer(f, footer, app, theme, full);

    match &app.mode {
        Mode::Help => render_help(f, area, app, theme),
        Mode::Search(_) => render_search(f, area, app, theme),
        Mode::Normal => {}
    }
}

fn render_too_small(f: &mut Frame, area: Rect, theme: &Theme) {
    let text = Text::from(vec![
        Line::styled("Terminal too small", theme.title),
        Line::from(format!(
            "need {}×{}, have {}×{}",
            MIN_SIZE.0, MIN_SIZE.1, area.width, area.height
        )),
        Line::styled("q quits", theme.dim),
    ]);
    let y = area.height.saturating_sub(3) / 2;
    let rect = Rect::new(area.x, area.y + y, area.width, area.height.min(3));
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        rect,
    );
}

fn render_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let market = app.market.label();
    let mut left = vec![
        Span::styled(" StockWise ", theme.title),
        Span::raw(market),
        Span::styled(" · ", theme.dim),
    ];
    match app.last_refresh {
        Some(t) => left.push(Span::styled(
            format!("updated {}", t.format("%H:%M:%S")),
            theme.dim,
        )),
        None => left.push(Span::styled("loading…", theme.dim)),
    }
    if app.refreshing && app.last_refresh.is_some() {
        left.push(Span::styled(" ⟳", theme.accent));
    }
    let right = Line::from(vec![
        Span::styled("read-only", theme.dim),
        Span::styled(" · ", theme.dim),
        Span::raw("? help "),
    ])
    .alignment(Alignment::Right);
    f.render_widget(Line::from(left), area);
    if area.width >= 60 {
        f.render_widget(right, area);
    }
}

fn index_spans(rows: &[IndexRow], with_price: bool, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  │ ", theme.dim));
        }
        spans.push(Span::styled(r.name.clone(), theme.accent));
        spans.push(Span::raw(" "));
        if with_price {
            spans.push(Span::raw(
                r.price
                    .map_or_else(|| "N/A".into(), |p| format!("{:.0}", p)),
            ));
            spans.push(Span::raw(" "));
        }
        spans.push(pct_span(r.change_percent, theme));
    }
    spans
}

fn render_indices(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(snap) = &app.snapshot else {
        return;
    };
    let full = Line::from(index_spans(&snap.indices, true, theme));
    let line = if full.width() <= area.width as usize {
        full
    } else {
        Line::from(index_spans(&snap.indices, false, theme))
    };
    f.render_widget(line, area);
}

fn pane<'a>(title: impl Into<Line<'a>>, focused: bool, theme: &Theme) -> Block<'a> {
    Block::bordered()
        .title(title)
        .title_style(if focused {
            theme.title
        } else {
            Style::default()
        })
        .border_style(if focused {
            theme.focus_border
        } else {
            theme.border
        })
}

fn placeholder(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn render_watchlist(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.focus == Focus::Watchlist;
    let rows = app.snapshot.as_ref().map(|s| &s.watchlist);
    let title = format!(" Watchlist ({}) ", rows.map_or(0, |r| r.len()));
    let block = pane(title, focused, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(rows) = rows else {
        placeholder(f, inner, vec![Line::styled(" Loading…", theme.dim)]);
        return;
    };
    if rows.is_empty() {
        placeholder(
            f,
            inner,
            vec![
                Line::styled(" Nothing watched yet.", theme.dim),
                Line::from(" Press / to search and add a symbol."),
            ],
        );
        return;
    }
    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(short(&r.symbol).to_string()),
                Cell::from(Line::from(price_text(r.price, r.currency.as_deref())).right_aligned()),
                Cell::from(Line::from(pct_span(r.change_percent, theme)).right_aligned()),
            ])
        })
        .collect();
    let table = Table::new(
        table_rows,
        [
            Constraint::Min(8),
            Constraint::Length(11),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("Symbol"),
            Cell::from(Line::from("Price").right_aligned()),
            Cell::from(Line::from("Day").right_aligned()),
        ])
        .style(theme.dim),
    )
    .row_highlight_style(if focused {
        theme.selected
    } else {
        Style::default()
    });
    let mut state = TableState::default().with_selected(Some(app.watch_sel));
    f.render_stateful_widget(table, inner, &mut state);
}

fn render_portfolio(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.focus == Focus::Portfolio;
    let view = app.snapshot.as_ref().map(|s| &s.portfolio);
    let mut title = vec![Span::raw(" Portfolio ")];
    if let Some(n) = view.map(|v| v.unpriced.len()).filter(|n| *n > 0) {
        title.push(Span::styled(format!("· {} unpriced ", n), theme.warn));
    }
    let block = pane(Line::from(title), focused, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(view) = view else {
        placeholder(f, inner, vec![Line::styled(" Loading…", theme.dim)]);
        return;
    };
    if view.holdings.is_empty() {
        placeholder(
            f,
            inner,
            vec![
                Line::styled(" No holdings yet.", theme.dim),
                Line::from(" Add one: stockwise portfolio add TCS 10 3500"),
            ],
        );
        return;
    }
    let totals = &view.totals;
    let cur = totals.currency.as_deref();
    let summary = vec![
        Span::raw(" Value "),
        Span::styled(format_price(totals.value, cur), theme.title),
        Span::raw("  P&L "),
        Span::styled(
            format!(
                "{} ({:+.2}%)",
                signed_money(totals.pnl, cur),
                totals.pnl_percent
            ),
            theme.signed(totals.pnl),
        ),
    ];
    let [summary_area, table_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    f.render_widget(Line::from(summary), summary_area);

    let wide = inner.width >= 60;
    let rows: Vec<Row> = view
        .holdings
        .iter()
        .map(|h| {
            let cur = h.currency.as_deref();
            // A fund's code says nothing: show its scheme name.
            let label = match (&h.name, crate::offmarket::is_fund(&h.symbol)) {
                (Some(name), true) => name.clone(),
                _ => short(&h.symbol).to_string(),
            };
            let mut cells = vec![
                Cell::from(label),
                Cell::from(Line::from(format!("{}", h.shares)).right_aligned()),
            ];
            if wide {
                cells.push(Cell::from(
                    Line::from(format_price(h.avg_cost, cur)).right_aligned(),
                ));
            }
            // ≈: an SGB valued at its gram of gold (NSE had no price).
            let estimated = h.price_source == Some(crate::offmarket::PriceSource::GoldEstimate);
            cells.push(Cell::from(
                Line::from(format!(
                    "{}{}",
                    if estimated { "≈" } else { "" },
                    price_text(h.price, cur)
                ))
                .right_aligned(),
            ));
            if wide {
                let pnl = h.pnl.map_or(Span::styled("—", theme.dim), |p| {
                    Span::styled(signed_money(p, cur), theme.signed(p))
                });
                cells.push(Cell::from(Line::from(pnl).right_aligned()));
            }
            let pct = h.pnl_percent.map_or(Span::styled("—", theme.dim), |p| {
                Span::styled(format!("{:+.2}%", p), theme.signed(p))
            });
            cells.push(Cell::from(Line::from(pct).right_aligned()));
            let row = Row::new(cells);
            if h.price.is_none() {
                row.style(theme.warn)
            } else {
                row
            }
        })
        .collect();
    let (widths, header): (Vec<Constraint>, Vec<&str>) = if wide {
        (
            vec![
                Constraint::Min(8),
                Constraint::Length(6),
                Constraint::Length(11),
                Constraint::Length(11),
                Constraint::Length(12),
                Constraint::Length(8),
            ],
            vec!["Symbol", "Qty", "Avg", "LTP", "P&L", "P&L%"],
        )
    } else {
        (
            vec![
                Constraint::Min(8),
                Constraint::Length(6),
                Constraint::Length(11),
                Constraint::Length(8),
            ],
            vec!["Symbol", "Qty", "LTP", "P&L%"],
        )
    };
    let header = Row::new(
        header
            .into_iter()
            .enumerate()
            .map(|(i, h)| {
                if i == 0 {
                    Cell::from(h)
                } else {
                    Cell::from(Line::from(h).right_aligned())
                }
            })
            .collect::<Vec<_>>(),
    )
    .style(theme.dim);
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(if focused {
            theme.selected
        } else {
            Style::default()
        });
    let mut state = TableState::default().with_selected(Some(app.hold_sel));
    f.render_stateful_widget(table, table_area, &mut state);
}

/// Paise don't matter at a glance once a price is in the thousands.
fn compact(v: f64) -> String {
    if v.abs() >= 1000.0 {
        format!("{:.0}", v)
    } else {
        format!("{:.2}", v)
    }
}

/// `! TCS ≥ 4000   4100`: the condition, then the live price.
fn triggered(a: &AlertCheckRow) -> bool {
    a.status == "triggered"
}

/// `≥ 4000` for price levels, `RSI14≤30`, `↑SMA50`, ... for the rest.
fn alert_label(a: &AlertCheckRow) -> String {
    match AlertCondition::parse(&a.condition) {
        Some(AlertCondition::Above) | None => format!("≥ {}", compact(a.target)),
        Some(AlertCondition::Below) => format!("≤ {}", compact(a.target)),
        Some(condition) => Alert {
            symbol: a.symbol.clone(),
            condition,
            target: a.target,
            period: a.period,
            created_at: String::new(),
        }
        .short_label(),
    }
}

fn alert_row(a: &AlertCheckRow, theme: &Theme) -> Row<'static> {
    let (mark, style) = if triggered(a) {
        ("! ", theme.warn)
    } else {
        ("  ", Style::default())
    };
    let now = a.price.map_or_else(|| "—".into(), compact);
    Row::new(vec![
        Cell::from(Span::styled(
            format!("{}{} {}", mark, short(&a.symbol), alert_label(a)),
            style,
        )),
        Cell::from(Line::styled(now, theme.dim).right_aligned()),
    ])
}

fn render_alerts(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let alerts = app.snapshot.as_ref().map(|s| &s.alerts);
    let fired = alerts.map_or(0, |a| a.iter().filter(|a| triggered(a)).count());
    let title = match alerts {
        Some(a) if !a.is_empty() => format!(" Alerts ({} of {} triggered) ", fired, a.len()),
        _ => " Alerts ".to_string(),
    };
    let block =
        pane(title, false, theme).border_style(if fired > 0 { theme.warn } else { theme.border });
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(alerts) = alerts else {
        return;
    };
    if alerts.is_empty() {
        placeholder(
            f,
            inner,
            vec![
                Line::styled(" No alerts set.", theme.dim),
                Line::from(" stockwise alert add TCS above 4200"),
            ],
        );
        return;
    }
    // Triggered first, then the rest in their `alert list` order.
    let mut sorted: Vec<&AlertCheckRow> = alerts.iter().collect();
    sorted.sort_by_key(|a| !triggered(a));
    let rows: Vec<Row> = sorted.into_iter().map(|a| alert_row(a, theme)).collect();
    let table = Table::new(rows, [Constraint::Min(10), Constraint::Length(8)]).header(
        Row::new(vec![
            Cell::from("  Alert"),
            Cell::from(Line::from("Now").right_aligned()),
        ])
        .style(theme.dim),
    );
    f.render_widget(table, inner);
}

fn render_alert_summary(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(alerts) = app.snapshot.as_ref().map(|s| &s.alerts) else {
        return;
    };
    let fired: Vec<&AlertCheckRow> = alerts.iter().filter(|a| triggered(a)).collect();
    let line = if fired.is_empty() {
        Line::styled(
            format!(" Alerts: none triggered ({} set)", alerts.len()),
            theme.dim,
        )
    } else {
        let names: Vec<&str> = fired.iter().map(|a| short(&a.symbol)).collect();
        Line::styled(
            format!(" ! Alerts triggered: {}", names.join(", ")),
            theme.warn,
        )
    };
    f.render_widget(line, area);
}

fn period_tabs(app: &App, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, p) in CHART_PERIODS.iter().enumerate() {
        let style = if i == app.period {
            theme.title.add_modifier(Modifier::REVERSED)
        } else {
            theme.dim
        };
        spans.push(Span::styled(format!("{}:{}", i + 1, p), style));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

fn time_label(ts: i64, period: &str) -> String {
    let Some(t) = chrono::DateTime::from_timestamp(ts, 0) else {
        return String::new();
    };
    let t = t.with_timezone(&chrono::Local);
    match period {
        "1d" => t.format("%H:%M").to_string(),
        "5d" => t.format("%d %b %H:%M").to_string(),
        "5y" => t.format("%b %Y").to_string(),
        _ => t.format("%d %b %y").to_string(),
    }
}

fn render_chart(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let symbol = app.selected_symbol();
    let shown: Option<&Series> = app
        .chart
        .as_ref()
        .filter(|c| Some(&c.symbol) == symbol.as_ref() && c.period == app.chart_period());
    let mut title = vec![Span::raw(" ")];
    title.push(Span::styled(
        symbol.as_deref().map(short).unwrap_or("Chart").to_string(),
        theme.title,
    ));
    title.push(Span::styled(
        format!(" · {} ", app.chart_period()),
        theme.dim,
    ));
    if let Some(s) = shown
        && let (Some(first), Some(last)) = (s.closes.first(), s.closes.last())
        && *first > 0.0
    {
        title.push(pct_span(Some((last - first) / first * 100.0), theme));
        title.push(Span::raw(" "));
    }
    let block = Block::bordered()
        .title(Line::from(title))
        .title_bottom(period_tabs(app, theme))
        .border_style(theme.border);

    let Some(series) = shown.filter(|s| s.closes.len() >= 2) else {
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = if let Some(e) = &app.chart_error {
            Line::styled(format!(" No chart: {}", e), theme.warn)
        } else if app.chart_loading || app.snapshot.is_none() {
            Line::styled(" Loading chart…", theme.dim)
        } else if shown.is_some() {
            Line::styled(" Not enough data for this period.", theme.dim)
        } else {
            Line::styled(" Select a symbol to chart it.", theme.dim)
        };
        placeholder(f, inner, vec![msg]);
        return;
    };

    let data: Vec<(f64, f64)> = series
        .closes
        .iter()
        .enumerate()
        .map(|(i, c)| (i as f64, *c))
        .collect();
    let min = series.closes.iter().copied().fold(f64::INFINITY, f64::min);
    let max = series
        .closes
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let pad = ((max - min) * 0.05).max(max.abs() * 0.001).max(0.01);
    let rising = series.closes.last() >= series.closes.first();
    let cur = series.currency.as_deref();
    let first_t = series.times.first().map(|t| time_label(*t, series.period));
    let last_t = series.times.last().map(|t| time_label(*t, series.period));
    let chart = Chart::new(vec![
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(if rising { theme.up } else { theme.down })
            .data(&data),
    ])
    .block(block)
    .x_axis(
        Axis::default()
            .bounds([0.0, (data.len() - 1) as f64])
            .style(theme.dim)
            .labels([first_t.unwrap_or_default(), last_t.unwrap_or_default()]),
    )
    .y_axis(
        Axis::default()
            .bounds([min - pad, max + pad])
            .style(theme.dim)
            .labels([format_price(min, cur), format_price(max, cur)]),
    );
    f.render_widget(chart, area);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App, theme: &Theme, full: bool) {
    if let Some(status) = &app.status {
        let style = if status.error {
            theme.down.add_modifier(Modifier::BOLD)
        } else {
            theme.accent
        };
        f.render_widget(Line::styled(format!(" {}", status.text), style), area);
        return;
    }
    let keys: &[(&str, &str)] = if full {
        &[
            ("↑↓", "select"),
            ("Tab", "pane"),
            ("1-6", "period"),
            ("/", "add"),
            ("x", "remove"),
            ("r", "refresh"),
            ("?", "help"),
            ("q", "quit"),
        ]
    } else {
        &[
            ("↑↓", "select"),
            ("Tab", "pane"),
            ("/", "add"),
            ("?", "help"),
            ("q", "quit"),
        ]
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(*k, theme.accent));
        spans.push(Span::styled(format!(" {}  ", label), theme.dim));
    }
    f.render_widget(Line::from(spans), area);
}

/// A centered popup no larger than `w`×`h`, cleared underneath.
fn popup(f: &mut Frame, area: Rect, w: u16, h: u16, title: &str, theme: &Theme) -> Rect {
    let rect = area.centered(
        Constraint::Length(w.min(area.width)),
        Constraint::Length(h.min(area.height)),
    );
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(Span::styled(title, theme.title))
        .border_style(theme.focus_border);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

const HELP: &[(&str, &str)] = &[
    ("↑ ↓  j k", "move the selection; the chart follows it"),
    ("g  G", "first / last row"),
    ("Tab", "switch between watchlist and portfolio"),
    ("1-6  [ ]", "chart period: 1d 5d 1mo 6mo 1y 5y"),
    ("/  a", "search a company and add it to the watchlist"),
    ("x  Del", "remove the selected watchlist symbol"),
    ("r", "refresh now"),
    ("?", "show or hide this help"),
    ("q  Ctrl+C", "quit"),
];

fn render_help(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let inner = popup(f, area, 66, HELP.len() as u16 + 7, " Keys ", theme);
    let mut lines: Vec<Line> = HELP
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!(" {:<11}", k), theme.accent),
                Span::raw(*d),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!(
            " Prices refresh every {}s (quotes are cached for 30s).",
            app.interval.as_secs()
        ),
        theme.dim,
    ));
    lines.push(Line::styled(
        " Read-only: this screen never places orders. Live trading",
        theme.warn,
    ));
    lines.push(Line::styled(
        " is only in `stockwise trade`, with a preview and confirmation.",
        theme.warn,
    ));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_search(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Mode::Search(search) = &app.mode else {
        return;
    };
    let inner = popup(f, area, 64, 16, " Add to watchlist ", theme);
    let [input, hint, results] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .areas(inner);
    f.render_widget(
        Line::from(vec![
            Span::styled(" Search: ", theme.accent),
            Span::raw(search.input.clone()),
            Span::styled("▏", theme.accent),
        ]),
        input,
    );
    let hint_text = if search.busy {
        " Searching…"
    } else if search.done && search.results.is_empty() {
        " No matches. Try another name or ticker."
    } else if search.done {
        " ↑↓ choose · Enter add · Esc cancel"
    } else {
        " Type a company name or ticker, Enter to search, Esc to cancel"
    };
    f.render_widget(
        Paragraph::new(Line::styled(hint_text, theme.dim)).wrap(Wrap { trim: false }),
        hint,
    );
    if search.results.is_empty() {
        return;
    }
    let rows: Vec<Row> = search
        .results
        .iter()
        .map(|h| {
            Row::new(vec![
                Cell::from(h.symbol.clone()),
                Cell::from(h.name.clone()),
                Cell::from(Span::styled(h.exchange.clone(), theme.dim)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Min(10),
            Constraint::Length(6),
        ],
    )
    .row_highlight_style(theme.selected);
    let mut state = TableState::default().with_selected(Some(search.selected));
    f.render_stateful_widget(table, results, &mut state);
}

fn sgr_color(c: Color) -> u8 {
    match c {
        Color::Black => 30,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Blue => 34,
        Color::Magenta => 35,
        Color::Cyan => 36,
        Color::Gray => 37,
        Color::DarkGray => 90,
        Color::LightRed => 91,
        Color::LightGreen => 92,
        Color::LightYellow => 93,
        Color::LightBlue => 94,
        Color::LightMagenta => 95,
        Color::LightCyan => 96,
        Color::White => 97,
        _ => 39,
    }
}

/// A rendered buffer as lines of text with basic ANSI colours (for
/// `tui --snapshot` and `assets/ansi2svg.py`). Without `color`, plain text.
pub fn to_ansi(buf: &Buffer, color: bool) -> String {
    let mut out = String::new();
    let area = buf.area;
    for y in area.top()..area.bottom() {
        let mut current: Option<(Color, Modifier)> = None;
        let mut skip = 0usize;
        let mut line = String::new();
        for x in area.left()..area.right() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buf[(x, y)];
            let style = (cell.fg, cell.modifier);
            if color && current != Some(style) {
                let mut codes = vec!["0".to_string()];
                if cell.modifier.contains(Modifier::BOLD) {
                    codes.push("1".into());
                }
                if cell.modifier.contains(Modifier::DIM) {
                    codes.push("2".into());
                }
                if cell.modifier.contains(Modifier::REVERSED) {
                    codes.push("7".into());
                }
                if cell.fg != Color::Reset {
                    codes.push(sgr_color(cell.fg).to_string());
                }
                line.push_str(&format!("\x1b[{}m", codes.join(";")));
                current = Some(style);
            }
            let sym = cell.symbol();
            line.push_str(sym);
            skip = Span::raw(sym).width().saturating_sub(1);
        }
        if color {
            line.push_str("\x1b[0m");
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::app::tests::{key, loaded};
    use super::super::app::{Msg, Search};
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn draw(app: &App, w: u16, h: u16, theme: Theme) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, app, &theme)).unwrap();
        term.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        to_ansi(buf, false)
    }

    #[test]
    fn full_frame_at_80x24() {
        let app = loaded();
        let screen = text(&draw(&app, 80, 24, Theme::new(true)));
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines.len(), 24);
        assert!(
            lines[0].starts_with(" StockWise India · updated "),
            "{}",
            lines[0]
        );
        assert!(lines[0].ends_with("read-only · ? help"), "{}", lines[0]);
        // Indices fit on one line without prices at 80 columns.
        assert!(lines[1].contains("NIFTY 50 ▲0.50%"), "{}", lines[1]);
        assert!(lines[1].contains("BANK NIFTY ▼0.30%"), "{}", lines[1]);
        for want in [
            "Watchlist (2)",
            "RELIANCE",
            "₹1300.00",
            "RELIANCE · 1mo ▲8.00%",
            "1:1d",
            "3:1mo",
            "Portfolio",
            "Portfolio · 1 unpriced",
            "Value ₹48500.00  P&L +₹5500.00 (+12.79%)",
            "DELISTED",
            "N/A",
            "Alerts (1 of 3 triggered)",
            "! TCS ≥ 4000",
            "4100",
            "DELISTED ≤ 1000000",
            "q quit",
        ] {
            assert!(screen.contains(want), "missing {want:?} in\n{screen}");
        }
        // The chart is drawn with braille dots.
        assert!(
            screen
                .chars()
                .any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
        );
    }

    #[test]
    fn wide_frame_shows_prices_and_more_columns() {
        let app = loaded();
        let screen = text(&draw(&app, 140, 40, Theme::new(true)));
        assert!(screen.contains("NIFTY 50 24800 ▲0.50%"), "{screen}");
        assert!(screen.contains("Avg"));
        assert!(screen.contains("+₹6000.00"), "TCS P&L column\n{screen}");
    }

    #[test]
    fn compact_frame_drops_the_chart() {
        let app = loaded();
        let screen = text(&draw(&app, 60, 20, Theme::new(true)));
        assert!(screen.contains("Watchlist (2)"));
        assert!(screen.contains("Portfolio"));
        assert!(!screen.contains("1:1d"), "{screen}");
        assert!(screen.contains("! Alerts triggered: TCS"), "{screen}");
    }

    #[test]
    fn tiny_terminals_get_a_message_not_a_panic() {
        let app = loaded();
        for (w, h) in [(39, 24), (80, 11), (10, 3), (1, 1)] {
            let screen = text(&draw(&app, w, h, Theme::new(true)));
            if w >= 18 && h >= 3 {
                assert!(screen.contains("Terminal too small"), "{w}x{h}\n{screen}");
            }
        }
    }

    #[test]
    fn every_size_renders() {
        let mut app = loaded();
        for mode in [0, 1, 2] {
            match mode {
                1 => {
                    app.update(key(KeyCode::Char('?')));
                }
                2 => {
                    app.update(key(KeyCode::Esc));
                    app.update(key(KeyCode::Char('/')));
                }
                _ => {}
            }
            for w in [40, 41, 60, 79, 80, 81, 120, 200] {
                for h in [12, 13, 23, 24, 25, 50] {
                    draw(&app, w, h, Theme::new(true));
                }
            }
        }
    }

    #[test]
    fn loading_state_before_first_snapshot() {
        let app = App::new(
            crate::market::Market::In,
            std::time::Duration::from_secs(30),
        );
        let screen = text(&draw(&app, 80, 24, Theme::new(true)));
        assert!(screen.contains("loading…"));
        assert!(screen.contains("Loading…"));
    }

    #[test]
    fn empty_states_explain_what_to_do() {
        let mut app = loaded();
        let snap = app.snapshot.as_mut().unwrap();
        snap.watchlist.clear();
        snap.portfolio = Default::default();
        snap.alerts.clear();
        let screen = text(&draw(&app, 100, 30, Theme::new(true)));
        assert!(screen.contains("Press / to search and add a symbol."));
        assert!(screen.contains("stockwise portfolio add TCS 10 3500"));
        assert!(screen.contains("stockwise alert add TCS above 4200"));
    }

    #[test]
    fn help_says_read_only() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('?')));
        let screen = text(&draw(&app, 80, 24, Theme::new(true)));
        assert!(
            screen.contains("Read-only: this screen never places orders."),
            "{screen}"
        );
        assert!(screen.contains("chart period: 1d 5d 1mo 6mo 1y 5y"));
    }

    #[test]
    fn search_popup_lists_results() {
        let mut app = loaded();
        app.mode = Mode::Search(Search {
            input: "tata".into(),
            results: vec![super::super::app::SearchHit {
                symbol: "TATAMOTORS.NS".into(),
                name: "Tata Motors Limited".into(),
                exchange: "NSI".into(),
            }],
            selected: 0,
            busy: false,
            done: true,
        });
        let screen = text(&draw(&app, 80, 24, Theme::new(true)));
        assert!(screen.contains("Search: tata"));
        assert!(screen.contains("TATAMOTORS.NS"));
        assert!(screen.contains("Enter add"));
    }

    #[test]
    fn status_line_replaces_key_hints() {
        let mut app = loaded();
        app.update(Msg::RefreshDue);
        app.update(Msg::Snapshot(Err("Cannot reach Yahoo Finance".into())));
        let screen = text(&draw(&app, 80, 24, Theme::new(true)));
        assert!(screen.contains("Refresh failed: Cannot reach Yahoo Finance"));
        assert!(!screen.contains("q quit"));
    }

    #[test]
    fn no_color_theme_uses_no_colours() {
        let app = loaded();
        let buf = draw(&app, 100, 30, Theme::new(false));
        assert!(
            buf.content
                .iter()
                .all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
        );
        // Direction is still visible without colour.
        assert!(text(&buf).contains("▲"));
        let coloured = draw(&app, 100, 30, Theme::new(true));
        assert!(coloured.content.iter().any(|c| c.fg == Color::Green));
    }

    #[test]
    fn ansi_output_has_codes_only_with_color() {
        let app = loaded();
        let buf = draw(&app, 80, 24, Theme::new(true));
        let ansi = to_ansi(&buf, true);
        assert!(ansi.contains("\x1b[0;32m"), "green somewhere");
        assert_eq!(ansi.lines().count(), 24);
        assert!(!to_ansi(&buf, false).contains('\x1b'));
    }
}
