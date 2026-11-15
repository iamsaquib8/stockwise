//! The TUI's portfolio analysis screen (key `p`): holdings with weight,
//! P&L, today's move, tax term and observations on the left; summary,
//! allocation, risk and observations on the right. Like `ui.rs`, a pure
//! function of [`App`].

use super::app::App;
use super::ui::{Theme, pane, placeholder, popup, short};
use crate::display::format_price;
use crate::portfolio_analysis::{Analysis, Group, Level, Position};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};

fn signed_style(v: f64, theme: &Theme) -> Style {
    if v > 0.0 {
        theme.up
    } else if v < 0.0 {
        theme.down
    } else {
        Style::default()
    }
}

fn pct_span(v: Option<f64>, theme: &Theme) -> Span<'static> {
    match v {
        Some(v) => Span::styled(format!("{:+.1}%", v), signed_style(v, theme)),
        None => Span::styled("–", theme.dim),
    }
}

/// A bar of `width` cells for `pct` of 100.
fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!(
        "{}{}",
        "█".repeat(filled.min(width)),
        "░".repeat(width - filled.min(width))
    )
}

/// The tax term in a few characters, as the terminal dashboard writes it.
pub fn term(p: &Position) -> String {
    crate::commands::term_cell(p)
}

/// `●2` when a holding has a notice among its observations, `○1` for
/// information only, blank without any.
fn obs_span(a: &Analysis, symbol: &str, theme: &Theme) -> Span<'static> {
    let mine: Vec<&Level> = a
        .observations
        .iter()
        .filter(|o| o.symbol.as_deref() == Some(symbol))
        .map(|o| &o.level)
        .collect();
    match mine.len() {
        0 => Span::raw(""),
        n if mine.contains(&&Level::Notice) => Span::styled(format!("●{}", n), theme.warn),
        n => Span::styled(format!("○{}", n), theme.dim),
    }
}

/// The analysis screen's body.
pub fn render_body(f: &mut Frame, area: Rect, app: &App, theme: &Theme, full: bool) {
    let Some(report) = &app.analysis else {
        let block = pane(" Portfolio analysis ", true, theme);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = vec![];
        if let Some(e) = &app.analysis_error {
            lines.push(Line::styled(format!(" Analysis failed: {}", e), theme.down));
            lines.push(Line::styled(" r tries again · p goes back", theme.dim));
        } else {
            lines.push(Line::styled(
                " Analysing the portfolio: prices, a year of closes, sectors…",
                theme.dim,
            ));
        }
        placeholder(f, inner, lines);
        return;
    };
    let a = &report.analysis;
    if a.positions.is_empty() {
        let block = pane(" Portfolio analysis ", true, theme);
        let inner = block.inner(area);
        f.render_widget(block, area);
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
    if full {
        let side_w = (area.width * 2 / 5).clamp(36, 60);
        let [left, right] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(side_w)]).areas(area);
        render_holdings(f, left, app, a, theme, true);
        // Allocation: two headings and three rows each, plus the border.
        let [summary, alloc, risk, obs] = Layout::vertical([
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Min(3),
        ])
        .areas(right);
        render_summary(f, summary, report, theme);
        render_allocation(f, alloc, a, theme);
        render_risk(f, risk, a, theme);
        render_observations(f, obs, a, theme);
    } else {
        let [summary, holdings, obs] = Layout::vertical([
            Constraint::Length(6),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .areas(area);
        render_summary(f, summary, report, theme);
        render_holdings(f, holdings, app, a, theme, false);
        let notices = a
            .observations
            .iter()
            .filter(|o| o.level == Level::Notice)
            .count();
        f.render_widget(
            Line::from(vec![
                Span::styled(format!(" {} observations", a.observations.len()), theme.dim),
                Span::styled(format!(" · {} notices", notices), theme.warn),
                Span::styled(" · Enter: details", theme.dim),
            ]),
            obs,
        );
    }
}

fn render_holdings(f: &mut Frame, area: Rect, app: &App, a: &Analysis, theme: &Theme, wide: bool) {
    let mut title = vec![
        Span::raw(" Holdings "),
        Span::styled(format!("· {} ", a.positions.len()), theme.dim),
    ];
    // The last re-run failed: what is shown is the run before it.
    if app.analysis_error.is_some() {
        title.push(Span::styled(
            "· last run failed, figures are older ",
            theme.warn,
        ));
    }
    let block = pane(Line::from(title), true, theme);
    let wide = wide && area.width >= 70;
    let rows: Vec<Row> = a
        .positions
        .iter()
        .map(|p| {
            let weight = p.weight_pct.unwrap_or(0.0);
            let mut cells = vec![
                Cell::from(short(&p.symbol).to_string()),
                Cell::from(Line::from(format!("{:.1}%", weight)).right_aligned()),
            ];
            if wide {
                cells.push(Cell::from(Span::styled(bar(weight, 8), theme.accent)));
            }
            cells.push(Cell::from(
                Line::from(pct_span(p.pnl_pct, theme)).right_aligned(),
            ));
            if wide {
                cells.push(Cell::from(
                    Line::from(pct_span(p.day_change_pct, theme)).right_aligned(),
                ));
            }
            cells.push(Cell::from(Line::from(term(p)).right_aligned()));
            if wide {
                cells.push(Cell::from(
                    Line::from(p.score.map_or("–".to_string(), |s| format!("{:.0}", s)))
                        .right_aligned(),
                ));
            }
            cells.push(Cell::from(obs_span(a, &p.symbol, theme)));
            let row = Row::new(cells);
            if p.value_base.is_none() {
                row.style(theme.warn)
            } else {
                row
            }
        })
        .collect();
    let (widths, header): (Vec<Constraint>, Vec<&str>) = if wide {
        (
            vec![
                Constraint::Min(10),
                Constraint::Length(6),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(5),
                Constraint::Length(3),
            ],
            vec!["Symbol", "Wt", "", "P&L", "Today", "Term", "Score", ""],
        )
    } else {
        (
            vec![
                Constraint::Min(8),
                Constraint::Length(6),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(3),
            ],
            vec!["Symbol", "Wt", "P&L", "Term", ""],
        )
    };
    let header = Row::new(
        header
            .into_iter()
            .map(|h| Cell::from(Span::styled(h, theme.dim))),
    );
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme.selected);
    let mut state = TableState::default().with_selected(Some(app.analysis_sel));
    f.render_stateful_widget(table, area, &mut state);
}

fn render_summary(
    f: &mut Frame,
    area: Rect,
    report: &crate::commands::AnalyzeReport,
    theme: &Theme,
) {
    let a = &report.analysis;
    let cur = Some(a.base_currency.as_str());
    let block = pane(" Summary ", false, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Value ", theme.dim),
            Span::styled(format_price(a.value, cur), theme.title),
            Span::styled("  P&L ", theme.dim),
            Span::styled(format!("{:+.1}%", a.pnl_pct), signed_style(a.pnl, theme)),
        ]),
        Line::from(vec![
            Span::styled(" Today ", theme.dim),
            match a.day_change_pct {
                Some(p) => Span::styled(
                    format!("{} ({:+.2}%)", format_price(a.day_change, cur), p),
                    signed_style(p, theme),
                ),
                None => Span::styled("–", theme.dim),
            },
        ]),
        Line::from(vec![
            Span::styled(" Short-term ", theme.dim),
            Span::styled(
                format_price(a.terms.short_term_gain, cur),
                signed_style(a.terms.short_term_gain, theme),
            ),
            Span::styled("  Long ", theme.dim),
            Span::styled(
                format_price(a.terms.long_term_gain, cur),
                signed_style(a.terms.long_term_gain, theme),
            ),
        ]),
    ];
    let mut last = vec![
        Span::styled(" Effective holdings ", theme.dim),
        Span::raw(format!("{:.1}", a.concentration.effective_holdings)),
    ];
    if let Some(t) = &report.tax_if_sold_today {
        last.push(Span::styled("  Tax if sold ", theme.dim));
        last.push(Span::raw(format_price(t.total, Some(&t.currency))));
    }
    lines.push(Line::from(last));
    f.render_widget(Paragraph::new(lines), inner);
}

fn group_lines<'a>(
    title: &'a str,
    groups: &[Group],
    take: usize,
    theme: &Theme,
    w: usize,
) -> Vec<Line<'a>> {
    let mut out = vec![Line::styled(format!(" {}", title), theme.dim)];
    let name_w = w.saturating_sub(22).clamp(8, 22);
    for g in groups.iter().take(take) {
        let name: String = if g.name.chars().count() > name_w {
            g.name.chars().take(name_w - 1).chain(['…']).collect()
        } else {
            g.name.clone()
        };
        out.push(Line::from(vec![
            Span::raw(format!(" {:<width$} ", name, width = name_w)),
            Span::styled(bar(g.weight_pct, 10), theme.accent),
            Span::raw(format!(" {:>5.1}%", g.weight_pct)),
        ]));
    }
    out
}

fn render_allocation(f: &mut Frame, area: Rect, a: &Analysis, theme: &Theme) {
    let block = pane(" Allocation ", false, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let mut lines = group_lines("By market", &a.by_market, 3, theme, w);
    lines.extend(group_lines("By sector", &a.by_sector, 3, theme, w));
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_risk(f: &mut Frame, area: Rect, a: &Analysis, theme: &Theme) {
    let block = pane(" Risk and concentration ", false, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let r = &a.risk;
    let c = &a.concentration;
    let opt =
        |v: Option<f64>, unit: &str| v.map_or("–".to_string(), |v| format!("{:.1}{}", v, unit));
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!(" {:<22}", k), theme.dim),
            Span::raw(v),
        ])
    };
    let lines = vec![
        kv("Yearly volatility", opt(r.volatility_pct, "%")),
        kv("Largest fall, last year", opt(r.max_drawdown_pct, "%")),
        kv(
            &format!("Beta vs {}", r.benchmark),
            r.beta.map_or("–".to_string(), |b| format!("{:.2}", b)),
        ),
        kv(
            "Largest holding",
            format!(
                "{} {:.1}%",
                c.largest.as_deref().map(short).unwrap_or("–"),
                c.largest_pct
            ),
        ),
        kv("Top five", format!("{:.1}%", c.top5_pct)),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_observations(f: &mut Frame, area: Rect, a: &Analysis, theme: &Theme) {
    let block = pane(
        Line::from(vec![
            Span::raw(" Observations "),
            Span::styled(format!("· {} ", a.observations.len()), theme.dim),
        ]),
        false,
        theme,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    if a.observations.is_empty() {
        placeholder(f, inner, vec![Line::styled(" Nothing to note.", theme.dim)]);
        return;
    }
    let lines: Vec<Line> = a
        .observations
        .iter()
        .map(|o| {
            let mark = match o.level {
                Level::Notice => Span::styled(" ● ", theme.warn),
                Level::Info => Span::styled(" ○ ", theme.dim),
            };
            let mut spans = vec![mark];
            if let Some(s) = &o.symbol {
                spans.push(Span::styled(format!("{} ", short(s)), theme.accent));
            }
            spans.push(Span::raw(o.text.clone()));
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// The selected holding in detail.
pub fn render_detail(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let (Some(report), Some(p)) = (&app.analysis, app.selected_position()) else {
        return;
    };
    let a = &report.analysis;
    let title = format!(" {} · ↑↓ scroll ", short(&p.symbol));
    let inner = popup(f, area, 72, 22, &title, theme);
    let cur = p.currency.as_deref();
    let base = Some(a.base_currency.as_str());
    let opt =
        |v: Option<f64>, unit: &str| v.map_or("–".to_string(), |v| format!("{:.1}{}", v, unit));
    let kv = |k: &str, v: Span<'static>| {
        Line::from(vec![Span::styled(format!(" {:<24}", k), theme.dim), v])
    };
    let mut lines = vec![];
    if let Some(name) = &p.name {
        lines.push(Line::styled(format!(" {}", name), theme.title));
    }
    lines.push(kv(
        "Market",
        Span::raw(format!(
            "{} · {} · {} cap · {}",
            p.market,
            p.currency.as_deref().unwrap_or("?"),
            p.cap_tier,
            p.sector.as_deref().unwrap_or("sector unknown")
        )),
    ));
    lines.push(kv(
        "Shares, cost, value",
        Span::raw(format!(
            "{} · {} · {}",
            p.shares,
            format_price(p.cost, cur),
            p.value.map_or("–".to_string(), |v| format_price(v, cur))
        )),
    ));
    lines.push(kv(
        "Weight, in base",
        Span::raw(format!(
            "{} · {}",
            opt(p.weight_pct, "%"),
            p.value_base
                .map_or("–".to_string(), |v| format_price(v, base))
        )),
    ));
    lines.push(kv(
        "P&L",
        match (p.pnl, p.pnl_pct) {
            (Some(g), Some(pc)) => Span::styled(
                format!("{} ({:+.1}%)", format_price(g, cur), pc),
                signed_style(g, theme),
            ),
            _ => Span::styled("–", theme.dim),
        },
    ));
    lines.push(kv(
        "Today, XIRR",
        Span::raw(format!(
            "{} · {}",
            p.day_change_pct
                .map_or("–".to_string(), |v| format!("{:+.2}%", v)),
            p.xirr_pct
                .map_or("–".to_string(), |v| format!("{:.1}% a year", v))
        )),
    ));
    lines.push(kv(
        "Gain short / long term",
        Span::raw(format!(
            "{} / {}",
            p.short_term_gain
                .map_or("–".to_string(), |v| format_price(v, cur)),
            p.long_term_gain
                .map_or("–".to_string(), |v| format_price(v, cur))
        )),
    ));
    if let Some(n) = &p.next_long_term {
        lines.push(kv(
            "Next lot long-term",
            Span::raw(format!(
                "{} share(s) on {} (in {} days)",
                n.qty,
                n.date.format("%d %b %Y"),
                n.days
            )),
        ));
    }
    lines.push(kv(
        "Held since",
        Span::raw(
            p.days_held
                .map_or("–".to_string(), |d| format!("{} days", d)),
        ),
    ));
    lines.push(kv(
        "vs 200-day, 52-week range",
        Span::raw(format!(
            "{} · {}",
            p.vs_200_day_pct
                .map_or("–".to_string(), |v| format!("{:+.1}%", v)),
            p.range_position_pct
                .map_or("–".to_string(), |v| format!("{:.0}% of the way up", v))
        )),
    ));
    lines.push(kv(
        "Volatility, fall, beta",
        Span::raw(format!(
            "{} · {} below year high · {}",
            opt(p.volatility_pct, "%"),
            opt(p.from_high_pct, "%"),
            p.beta
                .map_or("beta –".to_string(), |b| format!("beta {:.2}", b))
        )),
    ));
    lines.push(kv(
        "Long-term score",
        Span::raw(match (p.score, p.score_coverage) {
            (Some(s), Some(c)) => format!(
                "{:.0} of 100 ({:.0}% of inputs){}",
                s,
                c * 100.0,
                p.risk_tier
                    .as_deref()
                    .map_or(String::new(), |r| format!(" · risk {}", r))
            ),
            _ => "–".to_string(),
        }),
    ));
    let mine: Vec<_> = a
        .observations
        .iter()
        .filter(|o| o.symbol.as_deref() == Some(&p.symbol))
        .collect();
    if !mine.is_empty() {
        lines.push(Line::styled(" Observations", theme.dim));
        for o in mine {
            let mark = match o.level {
                Level::Notice => Span::styled(" ● ", theme.warn),
                Level::Info => Span::styled(" ○ ", theme.dim),
            };
            lines.push(Line::from(vec![mark, Span::raw(o.text.clone())]));
        }
    }
    lines.push(Line::styled(
        " Readings of the data, not a recommendation · Esc closes",
        theme.dim,
    ));
    // Rows the wrapped text takes, so scrolling stops at its last line.
    let width = inner.width.max(1) as usize;
    let rows: usize = lines.iter().map(|l| l.width().max(1).div_ceil(width)).sum();
    let max_scroll = rows.saturating_sub(inner.height as usize) as u16;
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .scroll((app.detail_scroll.min(max_scroll), 0)),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::super::app::tests::{key, loaded};
    use super::super::app::{Effect, Mode, Msg, Screen};
    use super::super::ui::{Theme, render, to_ansi};
    use crate::commands::{AnalyzeReport, TaxIfSold};
    use crate::market::CapTier;
    use crate::portfolio_analysis::{self as pa, LotInput, PositionInput};
    use chrono::NaiveDate;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn holding(sym: &str, shares: f64, cost: f64, price: f64, bought: &str) -> PositionInput {
        PositionInput {
            symbol: sym.into(),
            name: Some(format!("{} Ltd", sym)),
            market: "India".into(),
            currency: Some("INR".into()),
            cap_tier: Some(CapTier::Large),
            sector: Some("Technology".into()),
            shares,
            cost: shares * cost,
            value: Some(shares * price),
            fx_to_base: Some(1.0),
            price: Some(price),
            day_change_pct: Some(0.8),
            lots: vec![LotInput {
                qty: shares,
                price: cost,
                date: Some(day(bought)),
                estimated: false,
            }],
            tax_country: Some(crate::tax::Country::India),
            score: Some((62.0, 0.8)),
            ..Default::default()
        }
    }

    pub fn report() -> Box<AnalyzeReport> {
        let ctx = pa::Context {
            today: day("2026-11-15"),
            base_currency: "INR".into(),
            max_stock_weight: Some(20.0),
            max_smallcap_weight: Some(25.0),
            benchmark_closes: Vec::new(),
            benchmark_name: "NIFTY 50".into(),
        };
        let analysis = pa::analyze(
            &[
                holding("TCS.NS", 10.0, 3200.0, 2400.0, "2024-03-10"),
                holding("RELIANCE.NS", 30.0, 1250.0, 1340.0, "2025-12-10"),
                holding("INFY.NS", 20.0, 1400.0, 1500.0, "2023-01-02"),
            ],
            &ctx,
        );
        Box::new(AnalyzeReport {
            analysis,
            tax_if_sold_today: Some(TaxIfSold {
                country: "India",
                currency: "INR".into(),
                short_term_tax: 540.0,
                long_term_tax: 0.0,
                cess: 21.6,
                total: 561.6,
                exemption_used: 2000.0,
            }),
            disclaimer: crate::verdict::DISCLAIMER,
        })
    }

    fn draw(app: &super::App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, app, &Theme::new(false))).unwrap();
        to_ansi(term.backend().buffer(), false)
    }

    #[test]
    fn p_opens_the_analysis_and_asks_for_it_once() {
        let mut app = loaded();
        assert_eq!(
            app.update(key(KeyCode::Char('p'))),
            vec![Effect::LoadAnalysis]
        );
        assert_eq!(app.screen, Screen::Analysis);
        // Still loading: going back and forth asks no second time.
        app.update(key(KeyCode::Char('p')));
        assert_eq!(app.screen, Screen::Overview);
        assert!(app.update(key(KeyCode::Char('p'))).is_empty());
        let loading = draw(&app, 100, 30);
        assert!(loading.contains("Analysing the portfolio"), "{loading}");

        app.update(Msg::Analysis(Ok(report())));
        assert!(!app.analysis_loading);
        // Largest first: RELIANCE ₹40,200, INFY ₹30,000, TCS ₹24,000.
        assert_eq!(app.selected_position().unwrap().symbol, "RELIANCE.NS");
        app.update(key(KeyCode::Down));
        assert_eq!(app.selected_position().unwrap().symbol, "INFY.NS");
        app.update(key(KeyCode::Char('G')));
        assert_eq!(app.selected_position().unwrap().symbol, "TCS.NS");
        app.update(key(KeyCode::Down));
        assert_eq!(app.analysis_sel, 2, "stays on the last row");
        // r refreshes the overview and re-analyses.
        let effects = app.update(key(KeyCode::Char('r')));
        assert!(effects.contains(&Effect::LoadAnalysis), "{effects:?}");
        app.update(Msg::Analysis(Err("offline".into())));
        assert_eq!(app.analysis_error.as_deref(), Some("offline"));
        assert!(
            app.analysis.is_some(),
            "a failed re-run keeps the last analysis"
        );
        let stale = draw(&app, 120, 40);
        assert!(stale.contains("last run failed"), "{stale}");
        assert!(stale.contains("Analysis failed (offline)"), "{stale}");
        // Esc returns to the overview.
        app.update(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Overview);
    }

    #[test]
    fn the_screen_fits_every_size_and_says_what_it_shows() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('p')));
        app.update(Msg::Analysis(Ok(report())));
        for (w, h) in [
            (160, 48),
            (120, 40),
            (100, 30),
            (80, 24),
            (60, 20),
            (40, 12),
        ] {
            let screen = draw(&app, w, h);
            assert_eq!(screen.lines().count(), h as usize, "{w}x{h}");
            assert!(screen.contains("RELIANCE"), "{w}x{h}\n{screen}");
        }
        let wide = draw(&app, 120, 40);
        for part in [
            "Holdings",
            "Summary",
            "Allocation",
            "By market",
            "Risk and concentration",
            "Observations",
            "Tax if sold",
            "re-analyse",
        ] {
            assert!(wide.contains(part), "{part} missing\n{wide}");
        }
        // TCS is 25% below cost: a notice on its row.
        let tcs = wide.lines().find(|l| l.contains("TCS")).unwrap();
        assert!(tcs.contains('●'), "{tcs}");
        let compact = draw(&app, 60, 20);
        assert!(compact.contains("observations"), "{compact}");
        crate::verdict::testing::assert_neutral(&wide);
    }

    #[test]
    fn enter_shows_the_selected_holding_in_detail() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('p')));
        app.update(Msg::Analysis(Ok(report())));
        app.update(key(KeyCode::Char('G')));
        app.update(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Detail);
        let screen = draw(&app, 120, 40);
        for part in [
            "TCS.NS Ltd",
            "Market",
            "Gain short / long term",
            "Long-term score",
            "below cost",
            "not a recommendation",
        ] {
            assert!(screen.contains(part), "{part} missing\n{screen}");
        }
        crate::verdict::testing::assert_neutral(&screen);
        app.update(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.screen, Screen::Analysis);
    }

    #[test]
    fn detail_scrolls_at_the_smallest_size() {
        let mut app = loaded();
        app.update(key(KeyCode::Char('p')));
        app.update(Msg::Analysis(Ok(report())));
        app.update(key(KeyCode::Enter));
        let first = draw(&app, 40, 12);
        assert!(!first.contains("Readings of the data"), "{first}");
        for _ in 0..40 {
            app.update(key(KeyCode::Down));
        }
        let later = draw(&app, 40, 12);
        assert!(later.contains("Esc closes"), "{later}");
        // Reopening starts from the top again.
        app.update(key(KeyCode::Esc));
        app.update(key(KeyCode::Enter));
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn allocation_lists_three_markets_and_three_sectors() {
        let mut r = report();
        let groups = |names: [&str; 3]| -> Vec<crate::portfolio_analysis::Group> {
            names
                .iter()
                .map(|n| crate::portfolio_analysis::Group {
                    name: n.to_string(),
                    holdings: 1,
                    value_base: 1.0,
                    weight_pct: 33.3,
                    pnl_base: 0.0,
                    local_value: None,
                    local_currency: None,
                })
                .collect()
        };
        r.analysis.by_market = groups(["India", "US", "United Kingdom"]);
        r.analysis.by_sector = groups(["Energy", "Technology", "Utilities"]);
        let mut app = loaded();
        app.update(key(KeyCode::Char('p')));
        app.update(Msg::Analysis(Ok(r)));
        let screen = draw(&app, 120, 40);
        assert!(
            screen.contains("United Kingdom") && screen.contains("Utilities"),
            "{screen}"
        );
    }
}
