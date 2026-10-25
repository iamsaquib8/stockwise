//! Backtests: one symbol, a portfolio, walk-forward, and the strategy list.

use super::*;
use backtest::costs::{Brokerage, CostModel, CostPreset};
use backtest::engine::{
    self, Asset, Fill, Metrics, Rebalance, SimConfig, SimResult, Trade, Weights,
};
use backtest::plugin;
use backtest::rules::{self, StrategyDef};
use backtest::{Bars, walkforward};
use serde_json::json;

/// Settings shared by every form of `backtest`.
pub struct BacktestOpts {
    /// `-s`: a built-in name, a strategy file name, or `file:PATH`
    pub strategy: String,
    pub params: Vec<(String, f64)>,
    pub costs: CostPreset,
    pub brokerage: Option<Brokerage>,
    pub slippage_bps: Option<f64>,
    pub fill: Fill,
    pub capital: f64,
    /// `--json` (the full result) or `--csv` (the trades).
    pub fmt: Format,
}

impl BacktestOpts {
    fn strategy(&self) -> Result<StrategyDef> {
        rules::load(&self.strategy)?.with_params(&self.params)
    }

    fn cost_model(&self, symbol: &str, currency: Option<&str>) -> Result<CostModel> {
        let mut m = self
            .costs
            .model_for(symbol, currency)
            .map_err(|e| anyhow::anyhow!(e))?;
        if let Some(b) = self.brokerage {
            m.brokerage = b;
        }
        if let Some(s) = self.slippage_bps {
            m.slippage_bps = s;
        }
        Ok(m)
    }

    fn sim_config(&self, def: &StrategyDef, costs: CostModel, symbols: &[String]) -> SimConfig {
        SimConfig {
            stop_loss_pct: def.stop_loss_pct,
            take_profit_pct: def.take_profit_pct,
            periods_per_year: engine::bars_per_year(symbols.iter().map(String::as_str)),
            ..SimConfig::single(self.capital, self.fill, costs)
        }
    }
}

/// Daily bars for `symbol`, with its currency.
async fn fetch_bars(
    client: &impl views::DataSource,
    symbol: &str,
    period: &str,
    market: Market,
) -> Result<(Bars, Option<String>)> {
    let chart = client.chart(symbol, period, "1d").await.with_context(|| {
        format!(
            "No price history for {symbol}{}",
            market::not_found_hint(market)
        )
    })?;
    let currency = chart.meta.as_ref().and_then(|m| m.currency.clone());
    let bars = Bars::from_chart(&chart);
    if bars.is_empty() {
        bail!(
            "No price history for {}{}",
            symbol,
            market::not_found_hint(market)
        );
    }
    Ok((bars, currency))
}

/// Refuse to backtest on fewer bars than the strategy needs to warm up.
fn check_history(bars: &Bars, symbol: &str, period: &str, need: usize) -> Result<()> {
    let need = need.max(50);
    if bars.len() < need {
        bail!(
            "Only {} daily bars for {symbol} over {period}; this strategy needs at least {need}. \
             Use a longer period, e.g. -p 5y",
            bars.len()
        );
    }
    Ok(())
}

/// `-₹3,659.70` rather than `₹-3659.70`.
fn money(v: f64, cur: Option<&str>) -> String {
    if v < 0.0 {
        format!("-{}", format_price(-v, cur))
    } else {
        format_price(v, cur)
    }
}

fn signed_pct(v: f64) -> String {
    let s = format!("{v:+.2}%");
    if v >= 0.0 {
        s.green().to_string()
    } else {
        s.red().to_string()
    }
}

fn opt_signed_pct(v: Option<f64>) -> String {
    v.map_or_else(|| "N/A".dimmed().to_string(), signed_pct)
}

fn ratio(v: Option<f64>) -> String {
    v.map_or_else(|| "N/A".dimmed().to_string(), |x| format!("{x:.2}"))
}

/// Add the fields of `extra` (a struct or a JSON object) to the object `out`.
fn extend(out: &mut serde_json::Value, extra: impl serde::Serialize) -> Result<()> {
    if let (Some(o), serde_json::Value::Object(e)) =
        (out.as_object_mut(), serde_json::to_value(extra)?)
    {
        o.extend(e);
    }
    Ok(())
}

fn equity_json(times: &[i64], equity: &[f64]) -> Vec<serde_json::Value> {
    times
        .iter()
        .zip(equity)
        .map(|(t, v)| json!({ "date": backtest::date(*t), "value": v }))
        .collect()
}

fn print_strategy(def: &StrategyDef) {
    let title = if def.description.is_empty() {
        def.name.clone()
    } else {
        format!("{} — {}", def.name, def.description)
    };
    print_kv("Strategy", &title);
    if def.plugin.is_some() {
        print_kv(
            "Signals",
            &format!("WASM plugin {} (ABI v{})", def.source, plugin::ABI_VERSION),
        );
    }
    let join = |c: &[String]| {
        let c: Vec<String> = c.iter().map(|x| rules::render(x, &def.params)).collect();
        if c.len() == 1 {
            c[0].clone()
        } else {
            c.iter()
                .map(|x| format!("({x})"))
                .collect::<Vec<_>>()
                .join(" and ")
        }
    };
    if def.plugin.is_none() {
        print_kv("Entry", &join(&def.entry));
    }
    let mut exit = if def.exit.is_empty() {
        Vec::new()
    } else {
        vec![join(&def.exit)]
    };
    if let Some(sl) = def.stop_loss_pct {
        exit.push(format!("stop {sl}%"));
    }
    if let Some(tp) = def.take_profit_pct {
        exit.push(format!("target {tp}%"));
    }
    if !exit.is_empty() {
        print_kv("Exit", &exit.join(" · "));
    }
    if !def.params.is_empty() {
        print_kv("Parameters", &rules::format_params(&def.params));
    }
}

fn print_settings(opts: &BacktestOpts, costs: &CostModel, cur: Option<&str>) {
    let csym = market::currency_symbol(cur);
    let fill = match opts.fill {
        Fill::NextOpen => "next bar's open",
        Fill::Close => "signal bar's close (optimistic)",
    };
    print_kv(
        "Fills",
        &format!("{fill} · capital {}", format_price(opts.capital, cur)),
    );
    print_kv("Costs", &costs.describe(csym));
}

/// Equity curve rescaled so the starting capital is 100.
fn print_chart(equity: &[f64], capital: f64, title: &str) {
    let scaled: Vec<f64> = equity.iter().map(|v| v / capital * 100.0).collect();
    let up = scaled.last().is_some_and(|v| *v >= 100.0);
    println!();
    for line in charts::line_chart(&scaled, 55, 8, if up { "green" } else { "red" }, title) {
        println!("{line}");
    }
}

fn print_risk(m: &Metrics) {
    print_kv("CAGR", &opt_signed_pct(m.cagr_pct));
    print_kv(
        "Max Drawdown",
        &format!("-{:.2}%", m.max_drawdown_pct).red().to_string(),
    );
    print_kv(
        "Sharpe / Sortino",
        &format!("{} / {}", ratio(m.sharpe), ratio(m.sortino)),
    );
    print_kv(
        "Exposure",
        &format!("{:.0}% invested on average", m.exposure_pct),
    );
}

fn print_costs(net: &Metrics, gross: &Metrics, capital: f64, cur: Option<&str>) {
    print_kv(
        "Net Return (after costs)",
        &signed_pct(net.total_return_pct).bold().to_string(),
    );
    print_kv(
        "Gross Return (no costs)",
        &signed_pct(gross.total_return_pct),
    );
    print_kv(
        "Costs Paid",
        &format!(
            "{} ({:.2}% of capital)",
            format_price(net.costs, cur),
            net.costs / capital * 100.0
        ),
    );
}

fn print_trade_stats(m: &Metrics, skipped: usize) {
    let wins = m
        .win_rate_pct
        .map_or(String::new(), |w| format!(" ({w:.0}% winners)"));
    print_kv("Trades", &format!("{}{wins}", m.trades));
    if m.trades > 0 {
        print_kv(
            "Avg Win / Avg Loss",
            &format!(
                "{} / {}",
                opt_signed_pct(m.avg_win_pct),
                opt_signed_pct(m.avg_loss_pct)
            ),
        );
        print_kv("Profit Factor", &ratio(m.profit_factor));
    }
    if skipped > 0 {
        print_kv(
            "Skipped Entries",
            &format!("{skipped} (capital below one share)")
                .yellow()
                .to_string(),
        );
    }
}

fn print_trades(trades: &[Trade], cur: Option<&str>, with_symbol: bool) {
    if trades.is_empty() {
        return;
    }
    print_section("Recent Trades");
    let sym_col = if with_symbol { 14 } else { 0 };
    println!(
        "  {:<sym_col$}{:<11} {:<11} {:>6} {:>11} {:>11} {:>9}  {}",
        if with_symbol { "Symbol" } else { "" }.bold(),
        "Entry".bold(),
        "Exit".bold(),
        "Qty".bold(),
        "Entry Px".bold(),
        "Exit Px".bold(),
        "Net".bold(),
        "Reason".bold(),
    );
    for t in trades.iter().rev().take(10) {
        let sym = if with_symbol {
            format!("{:<14}", t.symbol)
        } else {
            String::new()
        };
        let reason = match t.exit_reason {
            engine::ExitReason::Signal => "signal",
            engine::ExitReason::StopLoss => "stop",
            engine::ExitReason::TakeProfit => "target",
            engine::ExitReason::Rebalance => "rebalance",
            engine::ExitReason::EndOfData => "still open",
        };
        // Pad before colouring so ANSI codes don't break the alignment.
        let net = format!("{:>9}", format!("{:+.2}%", t.return_pct));
        let net = if t.net_pnl >= 0.0 {
            net.green()
        } else {
            net.red()
        };
        println!(
            "  {sym}{:<11} {:<11} {:>6} {:>11} {:>11} {}  {}",
            t.entry_date,
            t.exit_date,
            t.qty,
            format_price(t.entry_price, cur),
            format_price(t.exit_price, cur),
            net,
            reason.dimmed()
        );
    }
}

fn footer() {
    println!(
        "\n  {}",
        "Backtests use historical data. Past results ≠ future performance."
            .dimmed()
            .italic()
    );
    println!();
}

fn single(symbol: &str, bars: &Bars, def: &StrategyDef, cfg: &SimConfig) -> Result<SimResult> {
    let signals = def.compile()?.signals(bars)?;
    Ok(engine::simulate(
        &[Asset {
            symbol,
            bars,
            signals: &signals,
        }],
        cfg,
    ))
}

/// Trades as CSV rows (`--csv`): one row per round trip, oldest exit first.
impl CsvRow for engine::Trade {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "entry_date",
        "exit_date",
        "qty",
        "entry_price",
        "exit_price",
        "gross_pnl",
        "costs",
        "net_pnl",
        "pnl_percent",
        "bars_held",
        "exit_reason",
    ];
}

/// `backtest SYMBOL --json`. Headline numbers are after costs;
/// `gross_return_percent` is the same rules without them.
pub(crate) fn single_json(
    symbol: &str,
    currency: Option<&str>,
    period: &str,
    bars: &Bars,
    def: &StrategyDef,
    opts: &BacktestOpts,
    costs: &CostModel,
    net: &SimResult,
    gross: &Metrics,
    buy_hold_pct: f64,
) -> Result<serde_json::Value> {
    let m = net.metrics();
    let mut out = json!({
        "symbol": symbol,
        "currency": currency,
        "strategy": def.name,
        "period": period,
        "interval": "1d",
        "bars": bars.len(),
        "start": backtest::date(bars.time[0]),
        "end": backtest::date(bars.time[bars.len() - 1]),
        "fill": opts.fill,
        "capital": opts.capital,
    });
    extend(&mut out, &m)?;
    extend(
        &mut out,
        json!({
            "gross_return_percent": gross.total_return_pct,
            "buy_hold_return_percent": buy_hold_pct,
            "alpha_percent": m.total_return_pct - buy_hold_pct,
            "skipped_entries": net.skipped_entries,
            "costs": net.costs,
            "cost_model": costs,
            "rules": def,
            "trades": net.trades,
            "equity": equity_json(&net.times, &net.equity),
        }),
    )?;
    Ok(out)
}

// ── backtest SYMBOL ──

/// One symbol backtested with and without costs, next to buy and hold.
pub(crate) struct SingleRun {
    pub(crate) resolved: String,
    pub(crate) currency: Option<String>,
    pub(crate) bars: Bars,
    pub(crate) def: StrategyDef,
    pub(crate) costs: CostModel,
    pub(crate) net: SimResult,
    pub(crate) gross: SimResult,
    pub(crate) buy_hold_pct: f64,
}

impl SingleRun {
    /// `backtest SYMBOL --json`.
    pub(crate) fn json(&self, period: &str, opts: &BacktestOpts) -> Result<serde_json::Value> {
        single_json(
            &self.resolved,
            self.currency.as_deref(),
            period,
            &self.bars,
            &self.def,
            opts,
            &self.costs,
            &self.net,
            &self.gross.metrics(),
            self.buy_hold_pct,
        )
    }
}

/// Backtest `opts.strategy` on `symbol`'s daily bars from `src`.
pub(crate) async fn run_single(
    src: &impl views::DataSource,
    symbol: &str,
    period: &str,
    opts: &BacktestOpts,
    market: Market,
) -> Result<SingleRun> {
    let def = opts.strategy()?;
    let resolved = market::resolve_symbol(symbol, market);
    let (bars, currency) = fetch_bars(src, &resolved, period, market).await?;
    check_history(&bars, &resolved, period, def.compile()?.lookback() + 10)?;

    let costs = opts.cost_model(&resolved, currency.as_deref())?;
    let cfg = opts.sim_config(&def, costs.clone(), std::slice::from_ref(&resolved));
    let net = single(&resolved, &bars, &def, &cfg)?;
    let gross = single(
        &resolved,
        &bars,
        &def,
        &SimConfig {
            costs: CostModel::none(),
            ..cfg.clone()
        },
    )?;
    let buy_hold_pct = backtest::buy_and_hold_pct(&bars).unwrap_or(0.0);
    Ok(SingleRun {
        resolved,
        currency,
        bars,
        def,
        costs,
        net,
        gross,
        buy_hold_pct,
    })
}

pub async fn cmd_backtest(
    symbol: &str,
    period: &str,
    opts: BacktestOpts,
    market: Market,
) -> Result<()> {
    // Strategy errors need no network.
    opts.strategy()?;
    let client = YahooClient::new().await?;
    let run = run_single(&client, symbol, period, &opts, market).await?;
    if opts.fmt == Format::Json {
        return output::print_json(&run.json(period, &opts)?);
    }
    let SingleRun {
        resolved,
        currency,
        bars,
        def,
        costs,
        net,
        gross,
        buy_hold_pct: bh,
    } = run;
    let cur = currency.as_deref();
    let (m, gm) = (net.metrics(), gross.metrics());

    if opts.fmt == Format::Csv {
        return output::print_csv(&net.trades);
    }

    print_header(&format!(
        "Backtest: {} — {} ({}, {} → {})",
        resolved,
        def.name,
        period,
        backtest::date(bars.time[0]),
        backtest::date(bars.time[bars.len() - 1]),
    ));
    print_strategy(&def);
    print_settings(&opts, &costs, cur);
    print_chart(
        &net.equity,
        opts.capital,
        "Equity after costs (start = 100)",
    );

    print_section("Performance");
    print_costs(&m, &gm, opts.capital, cur);
    print_kv("Buy & Hold Return", &signed_pct(bh));
    print_kv(
        "Alpha (net vs B&H)",
        &signed_pct(m.total_return_pct - bh).bold().to_string(),
    );
    print_risk(&m);

    print_section("Trade Statistics");
    print_trade_stats(&m, net.skipped_entries);
    print_trades(&net.trades, cur, false);

    let mut ai = crate::ai::AiClient::connect().await;
    if ai.auto_available().await {
        let data = format!(
            "Strategy: {} ({}), Period: {}, Net return after Indian/US trading costs: {:.2}%, \
             Gross: {:.2}%, Buy&Hold: {:.2}%, Win rate: {}, Trades: {}, Max DD: {:.1}%, Sharpe: {}",
            def.name,
            def.description,
            period,
            m.total_return_pct,
            gm.total_return_pct,
            bh,
            m.win_rate_pct.map_or("N/A".into(), |w| format!("{w:.0}%")),
            m.trades,
            m.max_drawdown_pct,
            m.sharpe.map_or("N/A".into(), |s| format!("{s:.2}"))
        );
        print_section(&format!("AI Interpretation ({})", ai.provider().name()));
        if let Ok(interp) = ai.interpret_backtest(&data).await {
            print_ai_answer(&interp);
        }
    }
    footer();
    Ok(())
}

// ── backtest portfolio ──

pub async fn cmd_backtest_portfolio(
    symbols: &[String],
    weights: Weights,
    rebalance: Rebalance,
    period: &str,
    opts: BacktestOpts,
    market: Market,
) -> Result<()> {
    let def = opts.strategy()?;
    let strategy = def.compile()?;
    let resolved = resolve_symbols(symbols, market);
    for (i, s) in resolved.iter().enumerate() {
        if resolved[..i].contains(s) {
            bail!("{s} is listed twice");
        }
    }
    let initial = weights
        .initial(resolved.len())
        .map_err(|e| anyhow::anyhow!(e))?;
    if weights == Weights::InverseVol && rebalance == Rebalance::None {
        bail!(
            "--weights inverse-vol needs --rebalance monthly or quarterly: weights come from the \
             trailing 63 days at each rebalance"
        );
    }

    let client = YahooClient::new().await?;
    let mut data = Vec::new();
    for s in &resolved {
        let (bars, cur) = fetch_bars(&client, s, period, market).await?;
        check_history(&bars, s, period, strategy.lookback() + 10)?;
        data.push((bars, cur));
    }
    let currency = data[0].1.clone();
    if data.iter().any(|(_, c)| *c != currency) {
        let list: Vec<String> = resolved
            .iter()
            .zip(&data)
            .map(|(s, (_, c))| format!("{s} {}", c.as_deref().unwrap_or("?")))
            .collect();
        bail!(
            "Mixed currencies ({}). A portfolio backtest needs every symbol in one currency",
            list.join(", ")
        );
    }
    let cur = currency.as_deref();
    let signals = resolved
        .iter()
        .zip(&data)
        .map(|(s, (b, _))| {
            strategy
                .signals(b)
                .with_context(|| format!("signals for {s}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let assets: Vec<Asset> = resolved
        .iter()
        .zip(&data)
        .zip(&signals)
        .map(|((s, (bars, _)), signals)| Asset {
            symbol: s,
            bars,
            signals,
        })
        .collect();

    // One cost model for the whole portfolio: every symbol must agree on it.
    let costs = opts.cost_model(&resolved[0], cur)?;
    for s in &resolved[1..] {
        if opts.cost_model(s, cur)? != costs {
            bail!(
                "{} and {s} need different cost models (stocks, crypto and FX are charged \
                 differently). Pass one with --costs, or backtest them separately",
                resolved[0]
            );
        }
    }
    let cfg = SimConfig {
        weights: weights.clone(),
        rebalance,
        ..opts.sim_config(&def, costs.clone(), &resolved)
    };
    let net = engine::simulate(&assets, &cfg);
    let gross = engine::simulate(
        &assets,
        &SimConfig {
            costs: CostModel::none(),
            ..cfg.clone()
        },
    );
    let (m, gm) = (net.metrics(), gross.metrics());
    let bh: f64 = data
        .iter()
        .zip(&initial)
        .map(|((b, _), w)| w * backtest::buy_and_hold_pct(b).unwrap_or(0.0))
        .sum();

    if opts.fmt == Format::Csv {
        return output::print_csv(&net.trades);
    }
    if opts.fmt == Format::Json {
        let mut out = json!({
            "symbols": resolved,
            "currency": currency,
            "strategy": def.name,
            "period": period,
            "interval": "1d",
            "weights": weights,
            "rebalance": rebalance,
            "rebalances": net.rebalances,
            "fill": opts.fill,
            "capital": opts.capital,
        });
        extend(&mut out, &m)?;
        extend(
            &mut out,
            json!({
                "gross_return_percent": gm.total_return_pct,
                "buy_hold_return_percent": bh,
                "alpha_percent": m.total_return_pct - bh,
                "skipped_entries": net.skipped_entries,
                "costs": net.costs,
                "cost_model": costs,
                "rules": def,
                "per_symbol": net.assets,
                "trades": net.trades,
                "equity": equity_json(&net.times, &net.equity),
            }),
        )?;
        return output::print_json(&out);
    }

    print_header(&format!(
        "Portfolio Backtest: {} symbols — {} ({})",
        resolved.len(),
        def.name,
        period
    ));
    print_strategy(&def);
    let w = match &weights {
        Weights::Equal => "equal".to_string(),
        Weights::InverseVol => "inverse volatility (trailing 63 days)".to_string(),
        Weights::Fixed(_) => "fixed".to_string(),
    };
    let reb = match rebalance {
        Rebalance::None => "never".to_string(),
        r => format!("{} ({} times)", value_label(r), net.rebalances),
    };
    print_kv("Weights / Rebalance", &format!("{w} / {reb}"));
    print_settings(&opts, &costs, cur);
    print_chart(
        &net.equity,
        opts.capital,
        "Portfolio equity after costs (start = 100)",
    );

    print_section("Performance");
    print_costs(&m, &gm, opts.capital, cur);
    print_kv("Buy & Hold (same weights)", &signed_pct(bh));
    print_kv(
        "Alpha (net vs B&H)",
        &signed_pct(m.total_return_pct - bh).bold().to_string(),
    );
    print_risk(&m);
    print_trade_stats(&m, net.skipped_entries);

    print_section("Per Symbol");
    println!(
        "  {:<14} {:>7} {:>7} {:>7} {:>14} {:>13} {:>9}",
        "Symbol".bold(),
        "Weight".bold(),
        "Trades".bold(),
        "Win %".bold(),
        "Net P&L".bold(),
        "Contribution".bold(),
        "Held".bold(),
    );
    for a in &net.assets {
        let contrib = format!("{:>13}", format!("{:+.2}%", a.contribution_pct));
        let contrib = if a.contribution_pct >= 0.0 {
            contrib.green()
        } else {
            contrib.red()
        };
        println!(
            "  {:<14} {:>6.0}% {:>7} {:>7} {:>14} {} {:>8.0}%",
            a.symbol,
            a.initial_weight * 100.0,
            a.trades,
            a.win_rate_pct.map_or("—".into(), |w| format!("{w:.0}%")),
            money(a.pnl, cur),
            contrib,
            a.exposure_pct
        );
    }
    print_trades(&net.trades, cur, true);
    footer();
    Ok(())
}

fn value_label(r: Rebalance) -> &'static str {
    match r {
        Rebalance::None => "never",
        Rebalance::Monthly => "monthly",
        Rebalance::Quarterly => "quarterly",
    }
}

// ── backtest walkforward ──

pub async fn cmd_backtest_walkforward(
    symbol: &str,
    train: walkforward::Window,
    test: walkforward::Window,
    period: &str,
    opts: BacktestOpts,
    market: Market,
) -> Result<()> {
    let def = opts.strategy()?;
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let (bars, currency) = fetch_bars(&client, &resolved, period, market).await?;
    let cur = currency.as_deref();
    let costs = opts.cost_model(&resolved, cur)?;
    let cfg = opts.sim_config(&def, costs.clone(), std::slice::from_ref(&resolved));
    // Window lengths count bars: trading days for stocks, every day for crypto.
    let (train, test) = (
        train.bars(cfg.periods_per_year),
        test.bars(cfg.periods_per_year),
    );
    let wf = walkforward::run(&def, &resolved, &bars, train, test, &cfg)?;

    if opts.fmt == Format::Csv {
        return output::print_csv(&wf.trades);
    }
    if opts.fmt == Format::Json {
        let mut out = json!({
            "symbol": resolved,
            "currency": currency,
            "strategy": def.name,
            "period": period,
            "interval": "1d",
            "fill": opts.fill,
            "capital": opts.capital,
            "cost_model": costs,
            "rules": def,
        });
        extend(&mut out, &wf)?;
        extend(
            &mut out,
            json!({ "equity": equity_json(&wf.times, &wf.equity) }),
        )?;
        return output::print_json(&out);
    }

    print_header(&format!(
        "Walk-Forward: {} — {} ({})",
        resolved, def.name, period
    ));
    print_strategy(&def);
    print_kv(
        "Windows",
        &format!(
            "train {} / test {} trading days, rolling; {} parameter set{} per window",
            train,
            test,
            wf.combos,
            if wf.combos == 1 { "" } else { "s" }
        ),
    );
    if def.grid.is_empty() {
        print_kv(
            "Note",
            &"no [grid] in this strategy — nothing to tune, so this checks stability only"
                .yellow()
                .to_string(),
        );
    }
    print_settings(&opts, &costs, cur);
    print_chart(
        &wf.equity,
        opts.capital,
        "Out-of-sample equity after costs (start = 100)",
    );

    print_section("Windows (in-sample → out-of-sample)");
    let tuned: Vec<&String> = def.grid.keys().collect();
    println!(
        "  {:<23} {:<22} {:>10} {:>7} {:>9} {:>8} {:>7}",
        "Test window".bold(),
        "Chosen".bold(),
        "IS /yr".bold(),
        "IS Shp".bold(),
        "OOS".bold(),
        "OOS Shp".bold(),
        "Trades".bold(),
    );
    for f in &wf.folds {
        let chosen: Vec<String> = tuned
            .iter()
            .map(|k| format!("{k}={}", f.params[*k]))
            .collect();
        let chosen = if chosen.is_empty() {
            "—".to_string()
        } else {
            chosen.join(" ")
        };
        let oos = format!("{:>9}", format!("{:+.2}%", f.out_of_sample.return_pct));
        let oos = if f.out_of_sample.return_pct >= 0.0 {
            oos.green()
        } else {
            oos.red()
        };
        println!(
            "  {:<23} {:<22} {:>10} {:>7} {} {:>8} {:>7}",
            format!("{} → {}", f.out_of_sample.start, f.out_of_sample.end),
            chosen,
            f.in_sample
                .annualised_pct
                .map_or("N/A".into(), |v| format!("{v:+.1}%")),
            f.in_sample
                .sharpe
                .map_or("N/A".into(), |v| format!("{v:.2}")),
            oos,
            f.out_of_sample
                .sharpe
                .map_or("N/A".into(), |v| format!("{v:.2}")),
            f.out_of_sample.trades,
        );
    }

    let m = &wf.out_of_sample;
    print_section("Out-of-Sample Result");
    print_kv(
        "Net Return (after costs)",
        &signed_pct(m.total_return_pct).bold().to_string(),
    );
    print_kv(
        "Gross Return (no costs)",
        &signed_pct(wf.out_of_sample_gross_return_pct),
    );
    print_kv(
        "Costs Paid",
        &format!(
            "{} ({:.2}% of capital)",
            format_price(m.costs, cur),
            m.costs / opts.capital * 100.0
        ),
    );
    print_risk(m);
    print_trade_stats(m, 0);

    print_section("Overfitting Check");
    print_kv(
        "In-Sample (avg/yr)",
        &format!(
            "{}  Sharpe {}",
            opt_signed_pct(wf.in_sample_annualised_pct),
            ratio(wf.in_sample_sharpe)
        ),
    );
    print_kv(
        "Out-of-Sample (/yr)",
        &format!(
            "{}  Sharpe {}",
            opt_signed_pct(wf.out_of_sample_annualised_pct),
            ratio(m.sharpe)
        ),
    );
    print_kv(
        "Walk-Forward Efficiency",
        &wf.efficiency
            .map_or("N/A".dimmed().to_string(), |e| format!("{:.0}%", e * 100.0)),
    );
    let verdict = match wf.verdict {
        walkforward::Verdict::Robust => "ROBUST".green().bold(),
        walkforward::Verdict::Degraded => "DEGRADED".yellow().bold(),
        walkforward::Verdict::LikelyOverfit => "LIKELY OVERFIT".red().bold(),
        walkforward::Verdict::NoEdge => "NO EDGE".red().bold(),
        walkforward::Verdict::Inconclusive => "INCONCLUSIVE".dimmed().bold(),
    };
    print_kv("Verdict", &format!("{verdict} — {}", wf.verdict_reason));
    footer();
    Ok(())
}

// ── backtest strategies ──

pub fn cmd_backtest_strategies(files: &[String], json: bool) -> Result<()> {
    if !files.is_empty() {
        let results: Vec<(String, Result<StrategyDef>)> = files
            .iter()
            .map(|f| {
                let path = f.strip_prefix("file:").unwrap_or(f);
                (f.clone(), rules::load_file(std::path::Path::new(path)))
            })
            .collect();
        return report_files(&results, json);
    }

    let builtins = rules::builtins();
    let dir = rules::strategies_dir();
    // A build without the `wasm-plugins` feature lists plugins as skipped
    // rather than as broken files.
    let (files, skipped): (Vec<_>, Vec<_>) = dir
        .as_deref()
        .map(rules::user_strategy_files)
        .unwrap_or_default()
        .into_iter()
        .partition(|p| plugin::SUPPORTED || !rules::is_plugin_file(p));
    let mine: Vec<(String, Result<StrategyDef>)> = files
        .into_iter()
        .map(|p| (p.display().to_string(), rules::load_file(&p)))
        .collect();

    if json {
        let out = json!({
            "builtin": builtins
                .iter()
                .map(|(d, aliases)| json!({ "aliases": aliases, "strategy": d }))
                .collect::<Vec<_>>(),
            "directory": dir,
            "user": mine
                .iter()
                .map(|(path, r)| match r {
                    Ok(d) => json!({ "path": path, "ok": true, "strategy": d }),
                    Err(e) => json!({ "path": path, "ok": false, "error": format!("{e:#}") }),
                })
                .collect::<Vec<_>>(),
            "skipped_plugins": skipped,
        });
        output::print_json(&out)?;
    } else {
        print_header("Backtest Strategies");
        print_section("Built-in");
        for (d, aliases) in &builtins {
            let alias = if aliases.is_empty() {
                String::new()
            } else {
                format!(" (also {})", aliases.join(", "))
            };
            println!("  {:<8} {}{}", d.name.bold(), d.description, alias.dimmed());
        }
        print_section("Yours");
        match &dir {
            Some(d) => println!("  {} {}", "Folder:".dimmed(), d.display()),
            None => println!("  {}", "No home directory; use -s file:PATH".dimmed()),
        }
        if mine.is_empty() && skipped.is_empty() {
            println!(
                "  {}",
                "No strategy files yet. Copy one from examples/strategies/ to get started."
                    .dimmed()
            );
        }
        for (path, r) in &mine {
            print_file_status(path, r, true);
        }
        if !skipped.is_empty() {
            println!(
                "  {}",
                format!(
                    "{} WASM plugin(s) skipped: this build has no plugin support \
                     (cargo install stockwise --features wasm-plugins)",
                    skipped.len()
                )
                .yellow()
            );
        }
        println!(
            "\n  {}",
            "Run one with `stockwise backtest SYMBOL -s NAME`. Format: docs/backtesting.md"
                .dimmed()
        );
        println!();
    }
    let bad = mine.iter().filter(|(_, r)| r.is_err()).count();
    if bad > 0 {
        bail!("{bad} strategy file(s) have errors");
    }
    Ok(())
}

/// One line per strategy file. In the strategies folder, a file named like a
/// built-in (`rsi.toml`) can't be run as `-s rsi`, so say so.
fn print_file_status(path: &str, r: &Result<StrategyDef>, in_folder: bool) {
    let p = std::path::Path::new(path);
    match r {
        Ok(d) => {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
            let shadow = if in_folder && stem.as_deref().is_some_and(rules::is_builtin_name) {
                " — the built-in of this name wins; run it with -s file:PATH"
                    .yellow()
                    .to_string()
            } else {
                String::new()
            };
            let kind = if d.plugin.is_some() {
                format!(" [WASM, ABI v{}]", plugin::ABI_VERSION)
                    .dimmed()
                    .to_string()
            } else {
                String::new()
            };
            println!(
                "  {} {:<16} {}{}{}",
                "✓".green(),
                d.name.bold(),
                d.description,
                kind,
                shadow
            );
        }
        Err(e) => {
            let name = p.file_name().map_or(path.into(), |n| n.to_string_lossy());
            println!("  {} {}", "✗".red(), name.bold());
            println!("      {}", format!("{e:#}").red());
        }
    }
}

fn report_files(results: &[(String, Result<StrategyDef>)], json: bool) -> Result<()> {
    if json {
        let out: Vec<_> = results
            .iter()
            .map(|(path, r)| match r {
                Ok(d) => json!({ "path": path, "ok": true, "strategy": d }),
                Err(e) => json!({ "path": path, "ok": false, "error": format!("{e:#}") }),
            })
            .collect();
        output::print_json(&out)?;
    } else {
        println!();
        for (path, r) in results {
            print_file_status(path, r, false);
        }
        println!();
    }
    let bad = results.iter().filter(|(_, r)| r.is_err()).count();
    if bad > 0 {
        bail!("{bad} strategy file(s) have errors");
    }
    Ok(())
}
