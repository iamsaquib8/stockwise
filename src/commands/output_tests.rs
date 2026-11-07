//! Shape tests for the pure builders behind `--json` and `--csv`. The key
//! lists here are the documented shapes in docs/commands.md: a failing test
//! means the docs (and anyone's scripts) need to change too.

use super::backtests::{BacktestOpts, single_json};
use super::fundamentals::{CompareRow, dividend_json};
use super::holdings::{alert_check_rows, tax_report, watch_rows};
use super::markets::{MoverRow, mover_rows};
use super::overview::QuoteRow;
use super::portfolio::{Valuation, portfolio_report};
use super::risk::{correlation_csv, correlation_matrix, return_row, risk_json};
use super::sim::sim_history_json;
use super::technicals::technical_json;
use super::*;
use crate::api::Quote;
use crate::output::{missing_columns, to_csv};
use crate::portfolio::{Lot, Portfolio};
use crate::tax::{self, Country};

fn quote(symbol: &str, price: Option<f64>, change_pct: Option<f64>) -> Quote {
    Quote {
        symbol: Some(symbol.into()),
        regular_market_price: price,
        regular_market_change_percent: change_pct,
        currency: Some("INR".into()),
        ..Default::default()
    }
}

/// A portfolio with one lot per (symbol, shares, price, date).
fn lots(entries: &[(&str, f64, f64, &str)]) -> Portfolio {
    let mut p = Portfolio::default();
    for (symbol, qty, price, date) in entries {
        p.add_lot(symbol, Lot::new(*qty, *price, *date, "test"));
    }
    p
}

fn day(s: &str) -> chrono::NaiveDate {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn valued(p: &Portfolio, quotes: Vec<Quote>) -> Valuation {
    let symbols: Vec<String> = p.holdings.iter().map(|h| h.symbol.clone()).collect();
    Valuation::new(quotes, &symbols, Market::In, None)
}

fn keys(v: &serde_json::Value) -> Vec<&str> {
    v.as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect()
}

fn assert_columns<R: CsvRow>(row: &R) {
    let (missing, extra) = missing_columns(row);
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{}: columns not in the row {:?}, fields not in the columns {:?}",
        std::any::type_name::<R>(),
        missing,
        extra
    );
}

/// A rising-then-falling series long enough for every indicator.
fn closes(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 100.0 + (i as f64 / 7.0).sin() * 10.0 + i as f64 * 0.1)
        .collect()
}

#[test]
fn portfolio_totals_skip_holdings_without_a_price() {
    let p = lots(&[
        ("TCS.NS", 10.0, 3000.0, "2025-01-02"),
        ("GONE.NS", 5.0, 100.0, "2025-01-02"),
    ]);
    let val = valued(&p, vec![quote("TCS.NS", Some(3300.0), Some(1.0))]);
    let report = portfolio_report(&p, &val, day("2026-01-02"));
    let rows = &report.holdings;
    let totals = &report.totals;

    assert_eq!(rows.len(), 2, "unpriced holdings stay in the rows");
    assert_eq!(rows[0].value, Some(33000.0));
    assert_eq!(rows[0].pnl, Some(3000.0));
    assert_eq!(rows[0].pnl_percent, Some(10.0));
    assert_eq!(rows[0].weight_percent, Some(100.0));
    let xirr = rows[0].xirr_percent.unwrap();
    assert!((xirr - 10.0).abs() < 0.1, "one year at +10%: {xirr}");
    assert_eq!(rows[1].price, None);
    assert_eq!(rows[1].value, None, "a missing price is never valued at 0");
    assert_eq!(totals.cost_basis, 30000.0);
    assert_eq!(totals.value, 33000.0);
    assert_eq!(totals.pnl_percent, 10.0);
    assert_eq!(totals.currency.as_deref(), Some("INR"));
    assert_eq!(report.unpriced, ["GONE.NS"]);

    let row = serde_json::to_value(&rows[0]).unwrap();
    assert_eq!(
        keys(&row),
        [
            "symbol",
            "shares",
            "avg_cost",
            "cost_basis",
            "price",
            "value",
            "pnl",
            "pnl_percent",
            "weight_percent",
            "xirr_percent",
            "currency",
            "name",
            "price_source",
            "nav_date"
        ]
    );
    assert_eq!(row["price_source"], "quote");
    assert!(serde_json::to_value(&rows[1]).unwrap()["price_source"].is_null());
    assert_eq!(
        keys(&serde_json::to_value(totals).unwrap()),
        ["cost_basis", "value", "pnl", "pnl_percent", "currency"]
    );
    assert_eq!(
        keys(&serde_json::to_value(&report).unwrap()),
        ["holdings", "totals", "unpriced", "unconverted", "notes"]
    );
    assert_columns(&rows[0]);
}

#[test]
fn portfolio_csv_leaves_missing_prices_empty() {
    let p = lots(&[("GONE.NS", 5.0, 100.0, "2025-01-02")]);
    let report = portfolio_report(&p, &valued(&p, Vec::new()), day("2026-01-02"));
    assert_eq!(
        to_csv(&report.holdings).unwrap().lines().nth(1),
        Some("GONE.NS,5.0,100.0,500.0,,,,,,,INR,,,")
    );
}

#[test]
fn watch_rows_follow_the_watchlist_order() {
    let rows = watch_rows(
        &["INFY.NS".into(), "GONE.NS".into(), "TCS.NS".into()],
        &[
            quote("TCS.NS", Some(1.0), Some(0.5)),
            quote("INFY.NS", Some(2.0), None),
        ],
    );
    let syms: Vec<&str> = rows.iter().map(|r| r.symbol.as_str()).collect();
    assert_eq!(syms, ["INFY.NS", "GONE.NS", "TCS.NS"]);
    assert_eq!(rows[1].price, None);
    assert_columns(&rows[0]);
}

#[test]
fn alert_check_never_triggers_without_a_price() {
    let mut store = AlertStore::default();
    store.add("TCS.NS", AlertCondition::Above, 3000.0);
    store.add("INFY.NS", AlertCondition::Below, 1000.0);
    store.add("GONE.NS", AlertCondition::Below, 1e9);
    store.add("TCS.NS", AlertCondition::RsiBelow, 30.0);
    let data: std::collections::HashMap<String, crate::alerts::MarketData> = [
        quote("TCS.NS", Some(3100.0), None),
        quote("INFY.NS", Some(1500.0), None),
    ]
    .iter()
    .map(|q| {
        let d = crate::alerts::MarketData {
            price: q.regular_market_price,
            currency: q.currency.clone(),
            ..Default::default()
        };
        (q.symbol.clone().unwrap(), d)
    })
    .collect();
    let rows = alert_check_rows(&store.alerts, &data);
    let status: Vec<&str> = rows.iter().map(|r| r.status).collect();
    // A technical alert without bars has no data rather than a reading.
    assert_eq!(status, ["triggered", "waiting", "no_price", "no_data"]);
    assert_eq!(rows[2].index, 2);
    assert_eq!(rows[0].value, Some(3100.0));
    assert_eq!(rows[3].period, Some(14));
    assert_eq!(rows[3].description, "RSI(14) below 30");
    assert_columns(&rows[0]);
}

#[test]
fn tax_report_splits_terms_and_skips_unpriced() {
    let p = lots(&[
        ("OLD.NS", 10.0, 100.0, "2024-01-01"),
        ("NEW.NS", 10.0, 100.0, "2026-05-01"),
        ("GONE.NS", 10.0, 100.0, "2026-05-01"),
    ]);
    let quotes = vec![
        quote("OLD.NS", Some(150.0), None),
        quote("NEW.NS", Some(80.0), None),
    ];
    let r = tax_report(Country::India, &p, &valued(&p, quotes), day("2026-06-01"));
    assert_eq!(r.unpriced, ["GONE.NS"]);
    assert_eq!(r.holdings[0].long_term_gain, 500.0);
    assert_eq!(r.holdings[0].short_term_gain, 0.0);
    assert_eq!(r.holdings[1].short_term_gain, -200.0);
    assert_eq!(r.unrealized.lt_gain, 500.0);
    assert_eq!(r.unrealized.st_loss, 200.0);
    assert!(r.sales.is_empty());
    // ₹500 LTCG is under the ₹1.25L exemption.
    assert_eq!(tax::estimate(Country::India, r.unrealized).total(), 0.0);

    assert_eq!(
        keys(&serde_json::to_value(&r.holdings[0]).unwrap()),
        [
            "symbol",
            "cost",
            "value",
            "short_term_gain",
            "long_term_gain",
            "estimated_dates"
        ]
    );
    assert_columns(&r.holdings[0]);
}

#[test]
fn tax_in_reports_intraday_apart_from_111a_and_112a() {
    use crate::portfolio::{ImportOptions, ImportRow};
    let row = |date: &str, sell: bool, qty: f64, price: f64| ImportRow {
        symbol: "TCS.NS".into(),
        qty,
        price,
        date: Some(date.into()),
        sell,
        units_as_of: None,
    };
    // The issue's tradebook: 100 held since 2023, 50 bought and sold on 2025-06-02.
    let mut p = Portfolio::default();
    p.apply_import(
        &[
            row("2023-01-10", false, 100.0, 3000.0),
            row("2025-06-02", false, 50.0, 4000.0),
            row("2025-06-02", true, 50.0, 4010.0),
        ],
        ImportOptions {
            source: "import:kite",
            replace: false,
            tradebook: true,
            splits: &[],
            account: None,
        },
    );
    let quotes = vec![quote("TCS.NS", Some(4000.0), None)];
    let r = tax_report(Country::India, &p, &valued(&p, quotes), day("2025-07-01"));
    // The ₹500 is speculative income: not a 111A/112A gain.
    assert!(r.sales.is_empty());
    assert_eq!(r.realized.st_gain + r.realized.lt_gain, 0.0);
    assert_eq!(r.intraday.len(), 1);
    assert_eq!(r.intraday_gain, 500.0);
    assert_eq!(r.intraday[0].term, Some("intraday"));
    // The 2023 lot is untouched: 100 shares, long-term.
    assert_eq!(r.holdings[0].long_term_gain, 100_000.0);

    // `tax advance` leaves it out too.
    let s = super::planning::advance_tax_schedule(&p, Some("2025-26"), &[], &Default::default())
        .unwrap();
    assert_eq!((s.total_stcg, s.total_ltcg), (0.0, 0.0));
}

/// The issue's repro for each ETF: 1000 units bought 2026-05-04 at 60, 600 sold
/// 2026-09-10 at 75 (₹9,000 gain), 400 still held and priced at 80.
fn etf_portfolio(symbol: &str) -> Portfolio {
    let mut p = Portfolio::default();
    p.add_lot(symbol, Lot::new(1000.0, 60.0, "2026-05-04", "manual"));
    p.sell_fifo(symbol, 600.0, 75.0, "2026-09-10", "manual")
        .unwrap();
    p
}

#[test]
fn tax_in_leaves_gold_and_liquid_etfs_out_of_111a_and_112a() {
    for symbol in ["GOLDBEES.NS", "SILVERBEES.NS", "LIQUIDBEES.NS", "MON100.NS"] {
        let p = etf_portfolio(symbol);
        let quotes = vec![quote(symbol, Some(80.0), None)];
        let r = tax_report(Country::India, &p, &valued(&p, quotes), day("2026-11-01"));
        assert!(r.sales.is_empty(), "{symbol}");
        assert!(r.holdings.is_empty(), "{symbol}");
        assert_eq!(r.realized.st_gain + r.realized.lt_gain, 0.0, "{symbol}");
        assert_eq!(r.other_rules.len(), 1, "{symbol}");
        assert_eq!(r.other_rules[0].symbol, symbol);
        assert!(r.other_rules[0].reason.contains("ETF"), "{symbol}");
    }
    // An equity ETF is still a listed share: ₹9,000 short-term at 20%.
    let p = etf_portfolio("NIFTYBEES.NS");
    let quotes = vec![quote("NIFTYBEES.NS", Some(80.0), None)];
    let r = tax_report(Country::India, &p, &valued(&p, quotes), day("2026-11-01"));
    assert!(r.other_rules.is_empty());
    assert_eq!(r.realized.st_gain, 9_000.0);
    assert_eq!(tax::estimate(Country::India, r.realized).st_tax, 1_800.0);
}

#[test]
fn harvest_does_not_offer_non_equity_etfs_for_the_exemption() {
    // Held since 2024: a long-term gain a listed share could harvest tax-free.
    let mut p = Portfolio::default();
    for sym in ["GOLDBEES.NS", "NIFTYBEES.NS", "LIQUIDBEES.NS"] {
        p.add_lot(sym, Lot::new(100.0, 50.0, "2024-01-10", "manual"));
    }
    let quotes = vec![
        quote("GOLDBEES.NS", Some(80.0), None),
        quote("NIFTYBEES.NS", Some(80.0), None),
        quote("LIQUIDBEES.NS", Some(40.0), None),
    ];
    let h =
        super::holdings::compute_harvest(&p, &valued(&p, quotes), Market::In, day("2026-11-01"));
    let names = |rows: &[super::holdings::HarvestRow]| {
        rows.iter().map(|r| r.symbol.clone()).collect::<Vec<_>>()
    };
    assert_eq!(names(&h.gain_harvest_candidates), ["NIFTYBEES.NS"]);
    // The liquid ETF's loss is not a 111A/112A loss either.
    assert!(h.loss_candidates.is_empty());
    let mut left_out: Vec<&str> = h.other_rules.iter().map(|o| o.symbol.as_str()).collect();
    left_out.sort();
    assert_eq!(left_out, ["GOLDBEES.NS", "LIQUIDBEES.NS"]);
}

#[test]
fn tax_export_and_advance_tax_skip_non_equity_etfs() {
    let mut p = etf_portfolio("GOLDBEES.NS");
    p.add_lot(
        "NIFTYBEES.NS",
        Lot::new(100.0, 60.0, "2026-05-04", "manual"),
    );
    p.sell_fifo("NIFTYBEES.NS", 100.0, 75.0, "2026-09-10", "manual")
        .unwrap();
    let schedule = crate::tax_export::india_schedule(
        &p.realized,
        day("2026-04-01"),
        |_| None,
        &Default::default(),
    );
    assert_eq!(schedule.sales.len(), 1);
    assert_eq!(schedule.sales[0].symbol, "NIFTYBEES.NS");
    assert_eq!(schedule.other_rules, ["GOLDBEES.NS"]);
    assert_eq!(schedule.summary.stcg_gains, 1_500.0);
    assert!(schedule.notes.iter().any(|n| n.contains("GOLDBEES.NS")));
    // Only the NIFTYBEES gain is in the advance-tax base.
    let adv = super::planning::advance_tax_schedule(&p, Some("2026-27"), &[], &Default::default())
        .unwrap();
    assert_eq!((adv.total_stcg, adv.total_ltcg), (1_500.0, 0.0));
}

#[test]
fn movers_leave_out_quotes_without_a_change() {
    let quotes = [
        quote("A", Some(1.0), Some(3.0)),
        quote("B", Some(1.0), None),
        quote("C", Some(1.0), Some(-2.0)),
        quote("D", None, Some(9.0)),
        quote("E", Some(1.0), Some(0.5)),
    ];
    let (gainers, losers) = mover_rows(&quotes, 2);
    let g: Vec<&str> = gainers.iter().map(|r| r.symbol.as_str()).collect();
    let l: Vec<&str> = losers.iter().map(|r| r.symbol.as_str()).collect();
    assert_eq!(g, ["A", "E"]);
    assert_eq!(l, ["C", "E"]);
    assert_eq!((gainers[0].side, gainers[0].rank), ("gainer", 1));
    assert_eq!((losers[0].side, losers[0].rank), ("loser", 1));
    assert_columns::<MoverRow>(&gainers[0]);
}

#[test]
fn return_row_needs_two_closes() {
    let d = |s: &str, p: f64| (day(s), p);
    assert!(return_row("5d", "1 Week", &[d("2026-01-01", 100.0)]).is_none());
    let r = return_row(
        "1mo",
        "1 Month",
        &[
            d("2026-01-01", 100.0),
            d("2026-01-15", 105.0),
            d("2026-02-01", 110.0),
        ],
    )
    .unwrap();
    assert_eq!(r.return_percent, 10.0);
    assert_eq!(r.growth_of_100000, 110000.0);
    // A month is too short to annualize.
    assert_eq!(r.annualized_percent, None);
    assert_columns(&r);
}

#[test]
fn annualized_return_uses_calendar_time_for_stocks_and_crypto() {
    // 366 daily crypto closes (one per calendar day) from 100 to 200: the
    // span is 365 days, so the annualized return is the return, +100%.
    let crypto: Vec<(chrono::NaiveDate, f64)> = (0..366)
        .map(|i| {
            let t = i as f64 / 365.0;
            (
                day("2025-01-01") + chrono::Duration::days(i),
                100.0 * 2.0_f64.powf(t),
            )
        })
        .collect();
    let r = return_row("1y", "1 Year", &crypto).unwrap();
    assert!((r.annualized_percent.unwrap() - 100.0).abs() < 0.5, "{r:?}");
    // 252 stock sessions over the same 365 days give the same answer: the
    // number of bars no longer matters.
    let stock: Vec<(chrono::NaiveDate, f64)> = (0..253)
        .map(|i| {
            let t = i as f64 / 252.0;
            let days = (i as f64 / 252.0 * 365.0).round() as i64;
            (
                day("2025-01-01") + chrono::Duration::days(days),
                100.0 * 2.0_f64.powf(t),
            )
        })
        .collect();
    let r = return_row("1y", "1 Year", &stock).unwrap();
    assert!((r.annualized_percent.unwrap() - 100.0).abs() < 0.5, "{r:?}");
    // +300% over about five years (1,826 days): CAGR = 4^(1/5) - 1 = 31.95%.
    let five = [(day("2021-01-01"), 100.0), (day("2026-01-01"), 400.0)];
    let r = return_row("5y", "5 Years", &five).unwrap();
    assert!((r.annualized_percent.unwrap() - 31.95).abs() < 0.1, "{r:?}");
}

#[test]
fn correlation_matrix_has_a_unit_diagonal_and_nulls_for_gaps() {
    let start = day("2026-01-01");
    let dated = |v: Vec<f64>| -> Vec<(chrono::NaiveDate, f64)> {
        v.into_iter()
            .enumerate()
            .map(|(i, x)| (start + chrono::Duration::days(i as i64), x))
            .collect()
    };
    let a = closes(60);
    let series = vec![
        ("A".to_string(), dated(a.clone())),
        ("B".to_string(), dated(a.iter().map(|x| x * 2.0).collect())),
        ("EMPTY".to_string(), Vec::new()),
    ];
    let m = correlation_matrix(&series);
    assert_eq!(m[0][0], Some(1.0));
    assert_eq!(m[0][1], Some(1.0));
    assert_eq!(m[0][2], None);
    assert_eq!(
        correlation_csv(&["A", "B", "EMPTY"], &m),
        "symbol,A,B,EMPTY\nA,1.0,1.0,\nB,1.0,1.0,\nEMPTY,,,1.0\n"
    );
}

#[test]
fn technical_json_has_the_documented_keys() {
    let c = closes(250);
    let v = technical_json("TCS.NS", Some("INR"), "1y", "1d", &c, &c, &c, &[1000; 250]);
    assert_eq!(
        keys(&v),
        [
            "symbol",
            "currency",
            "period",
            "interval",
            "bars",
            "price",
            "sma_10",
            "sma_20",
            "sma_50",
            "sma_100",
            "sma_200",
            "ema_12",
            "ema_26",
            "rsi_14",
            "rsi_signal",
            "macd",
            "bollinger",
            "atr_14",
            "atr_percent",
            "vwap",
            "bias"
        ]
    );
    assert_eq!(keys(&v["macd"]), ["line", "signal", "histogram", "trend"]);
    assert_eq!(
        keys(&v["bollinger"]),
        ["upper", "middle", "lower", "position"]
    );
    assert_eq!(keys(&v["bias"]), ["verdict", "bullish", "bearish"]);
    assert!(v["sma_200"].is_number());

    // Too few bars for the long averages: null, not 0.
    let short = closes(40);
    let v = technical_json("X", None, "3mo", "1d", &short, &short, &short, &[1; 40]);
    assert!(v["sma_200"].is_null());
}

#[test]
fn risk_json_has_the_documented_keys() {
    let v = risk_json("TCS.NS", Some("INR"), "1y", &closes(250));
    assert_eq!(
        keys(&v),
        [
            "symbol",
            "currency",
            "period",
            "interval",
            "bars",
            "price",
            "return_percent",
            "volatility_percent",
            "risk_level",
            "sharpe",
            "sortino",
            "calmar",
            "max_drawdown",
            "var_95_percent",
            "var_99_percent"
        ]
    );
    assert_eq!(
        keys(&v["max_drawdown"]),
        ["percent", "peak_price", "trough_price", "trading_days"]
    );
}

#[test]
fn dividend_json_without_a_dividend_has_no_projections() {
    let v = dividend_json("X.NS", Some("INR"), 100.0, 0.0);
    assert_eq!(v["pays_dividend"], false);
    assert_eq!(v["income"].as_array().unwrap().len(), 0);

    let v = dividend_json("X.NS", Some("INR"), 100.0, 0.05);
    assert_eq!(
        keys(&v),
        [
            "symbol",
            "currency",
            "price",
            "pays_dividend",
            "dividend_yield_percent",
            "annual_dividend_per_share",
            "income",
            "drip"
        ]
    );
    assert_eq!(v["dividend_yield_percent"], 5.0);
    assert_eq!(v["income"][0]["annual_income"], 5000.0);
    let years: Vec<i64> = v["drip"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["year"].as_i64().unwrap())
        .collect();
    assert_eq!(years, [1, 5, 10]);
}

#[test]
fn backtest_json_and_trade_rows() {
    use crate::backtest::costs::{CostModel, CostPreset};
    use crate::backtest::engine::{self, Fill, SimConfig};
    use crate::backtest::{Bars, rules};

    let closes: Vec<f64> = (0..120)
        .map(|i| 100.0 + (i as f64 * 0.2).sin() * 10.0)
        .collect();
    let bars = Bars::from_closes(&closes);
    let def = rules::load("rsi").unwrap();
    let signals = def.compile().unwrap().signals(&bars).unwrap();
    let costs = CostModel::in_delivery();
    let cfg = SimConfig::single(100_000.0, Fill::NextOpen, costs.clone());
    let asset = engine::Asset {
        symbol: "X.NS",
        bars: &bars,
        signals: &signals,
    };
    let net = engine::simulate(std::slice::from_ref(&asset), &cfg);
    let gross = engine::simulate(
        &[asset],
        &SimConfig {
            costs: CostModel::none(),
            ..cfg
        },
    )
    .metrics();
    assert!(!net.trades.is_empty());
    assert_columns(&net.trades[0]);

    let opts = BacktestOpts {
        strategy: "rsi".into(),
        params: vec![],
        costs: CostPreset::Auto,
        brokerage: None,
        slippage_bps: None,
        fill: Fill::NextOpen,
        capital: 100_000.0,
        fmt: Format::Json,
    };
    let v = single_json(
        "X.NS",
        Some("INR"),
        "2y",
        &bars,
        &def,
        &opts,
        &costs,
        &net,
        &gross,
        5.0,
    )
    .unwrap();
    assert_eq!(
        keys(&v),
        [
            "symbol",
            "currency",
            "strategy",
            "period",
            "interval",
            "bars",
            "start",
            "end",
            "fill",
            "capital",
            "start_value",
            "end_value",
            "total_return_percent",
            "cagr_percent",
            "max_drawdown_percent",
            "sharpe",
            "sortino",
            "exposure_percent",
            "trade_count",
            "win_rate_percent",
            "avg_win_percent",
            "avg_loss_percent",
            "profit_factor",
            "costs_paid",
            "gross_return_percent",
            "buy_hold_return_percent",
            "alpha_percent",
            "skipped_entries",
            "costs",
            "cost_model",
            "rules",
            "trades",
            "equity"
        ]
    );
    assert_eq!(v["strategy"], "rsi");
    assert_eq!(v["fill"], "next-open");
    assert_eq!(v["trade_count"], net.trades.len());
    assert_eq!(v["trades"][0]["exit_reason"], "signal");
    assert_eq!(v["equity"][0]["date"], "2024-01-01");
}

#[test]
fn empty_sim_history_is_not_an_error() {
    let history = SimHistory::default();
    let v = sim_history_json(&history, Market::In, &history.stats("IN"));
    assert_eq!(keys(&v), ["stats", "confidence", "sessions"]);
    assert_eq!(v["stats"]["days"], 0);
    assert!(v["confidence"].is_null());
    assert_eq!(v["sessions"].as_array().unwrap().len(), 0);
}

#[test]
fn quote_and_compare_rows_match_their_columns() {
    let q = quote("TCS.NS", Some(1.0), Some(1.0));
    assert_columns(&QuoteRow::from(&q));
    assert_columns(&CompareRow::from(&q));
}

#[test]
fn inline_rows_match_their_columns() {
    assert_columns(&crate::views::IndexRow {
        region: "Indian Markets".into(),
        symbol: "^NSEI".into(),
        name: "NIFTY 50".into(),
        price: None,
        change: None,
        change_percent: None,
        currency: None,
    });
    assert_columns(&super::overview::SearchRow {
        symbol: None,
        name: None,
        quote_type: None,
        exchange: None,
    });
    assert_columns(&super::markets::SectorRow {
        sector: "IT".into(),
        symbol: "^CNXIT".into(),
        price: None,
        change_percent: None,
        currency: None,
    });
    assert_columns(&super::markets::ScreenRow {
        symbol: "TCS.NS".into(),
        name: None,
        price: None,
        change_percent: None,
        pe: None,
        market_cap: None,
        dividend_yield: None,
        currency: None,
    });
    assert_columns(&super::holdings::AlertRow {
        index: 0,
        symbol: "TCS.NS".into(),
        condition: "above".into(),
        target: 1.0,
        period: None,
        description: "above 1.00".into(),
        created_at: "2026-01-01 09:15".into(),
    });
    assert_columns(&crate::views::Bar {
        time: None,
        open: None,
        high: None,
        low: None,
        close: None,
        volume: None,
        ts: 0,
    });
}

#[test]
fn benchmark_json_and_csv_shapes() {
    use super::planning::{BenchmarkRow, Flagged, benchmark_json};
    use crate::benchmark::{HoldingDays, analyze};
    let row = BenchmarkRow {
        symbol: "TCS.NS".into(),
        sector: "Technology".into(),
        avg_weight: 100.0,
        end_weight: 100.0,
        return_pct: 2.0,
        contribution: 2.0,
        active_contribution: 1.0,
        days_held: 2,
        days_unpriced: 0,
        flag: None,
    };
    assert_columns(&row);
    let calendar: Vec<chrono::NaiveDate> = (0..3)
        .map(|i| day("2026-01-01") + chrono::Days::new(i))
        .collect();
    let holdings = vec![HoldingDays {
        symbol: "TCS.NS".into(),
        sector: "Technology".into(),
        qty: vec![10.0; 3],
        price: vec![Some(100.0), Some(101.0), Some(102.0)],
    }];
    let a = analyze(&calendar, &[Some(1.0), Some(1.0), Some(1.01)], &holdings).unwrap();
    let flagged = [Flagged {
        symbol: "X.NS".into(),
        reason: "no price history — left out".into(),
    }];
    let v = benchmark_json(
        Some(&a),
        ("^NSEI", "NIFTY 50", Some("INR")),
        "INR",
        "1y",
        &[row],
        &flagged,
    );
    assert_eq!(
        keys(&v),
        [
            "benchmark",
            "base_currency",
            "period",
            "start",
            "end",
            "trading_days",
            "portfolio_return",
            "benchmark_return",
            "excess_return",
            "portfolio_annualized",
            "benchmark_annualized",
            "metrics",
            "holdings",
            "sectors",
            "flagged",
            "series"
        ]
    );
    assert_eq!(
        keys(&v["benchmark"]),
        ["symbol", "name", "currency", "converted_to_base"]
    );
    assert_eq!(
        keys(&v["metrics"]),
        [
            "observations",
            "alpha",
            "beta",
            "tracking_error",
            "information_ratio",
            "correlation"
        ]
    );
    assert_eq!(
        keys(&v["sectors"][0]),
        [
            "sector",
            "holdings",
            "avg_weight",
            "end_weight",
            "return_pct",
            "contribution",
            "active_contribution"
        ]
    );
    assert_eq!(keys(&v["series"][0]), ["date", "portfolio", "benchmark"]);
    assert_eq!(v["portfolio_return"], 2.0);
    assert_eq!(v["benchmark_return"], 1.0);
    assert_eq!(keys(&v["flagged"][0]), ["symbol", "reason"]);
    // Nothing held: the same keys with nulls and empty lists.
    let empty = benchmark_json(None, ("^NSEI", "NIFTY 50", None), "INR", "1y", &[], &[]);
    assert_eq!(keys(&empty), keys(&v));
    assert!(empty["portfolio_return"].is_null());
}

#[test]
fn goal_and_tax_export_json_shapes() {
    let plan = serde_json::to_value(crate::goals::plan_sip(1e7, 2, 12.0, 10.0, 0.0)).unwrap();
    assert_eq!(
        keys(&plan),
        [
            "target",
            "years",
            "annual_return_pct",
            "step_up_pct",
            "savings",
            "savings_future_value",
            "monthly_sip",
            "final_monthly_sip",
            "total_invested",
            "gains",
            "lump_sum_today",
            "schedule"
        ]
    );
    assert_eq!(
        keys(&plan["schedule"][0]),
        ["year", "monthly_sip", "invested", "value"]
    );
    let retire = serde_json::to_value(crate::goals::plan_retirement(
        80_000.0, 20, 30, 6.0, 12.0, 7.0, 0.0, 0.0,
    ))
    .unwrap();
    assert_eq!(
        keys(&retire),
        [
            "monthly_expense_today",
            "years_to_retire",
            "years_in_retirement",
            "inflation_pct",
            "return_pre_pct",
            "return_post_pct",
            "monthly_expense_at_retirement",
            "corpus",
            "sip"
        ]
    );
    let edu =
        serde_json::to_value(crate::goals::plan_education(25e5, 10, 8.0, 12.0, 0.0, 0.0)).unwrap();
    assert_eq!(
        keys(&edu),
        ["cost_today", "years", "inflation_pct", "future_cost", "sip"]
    );

    let sale = crate::portfolio::Sale {
        symbol: "TCS.NS".into(),
        qty: 1.0,
        buy_date: "2024-01-10".into(),
        buy_price: 3500.0,
        sell_date: "2025-06-02".into(),
        sell_price: 4100.0,
        buy_date_estimated: false,
        source: "manual".into(),
        order_id: None,
        intraday: false,
        account: None,
    };
    let india = serde_json::to_value(crate::tax_export::india_schedule(
        std::slice::from_ref(&sale),
        day("2025-04-01"),
        |_| None,
        &Default::default(),
    ))
    .unwrap();
    let schedule_keys = [
        "country",
        "tax_year",
        "period_start",
        "period_end",
        "currency",
        "disclaimer",
        "sales",
        "summary",
        "other_currency",
        "virtual_digital_assets",
        "intraday_sales",
        "intraday_gain",
        "other_rules",
        "notes",
    ];
    assert_eq!(keys(&india), schedule_keys);
    assert_eq!(
        keys(&india["sales"][0]),
        [
            "section",
            "term",
            "isin",
            "scrip",
            "symbol",
            "quantity",
            "sale_date",
            "sale_price",
            "sale_value",
            "purchase_date",
            "purchase_price",
            "cost",
            "holding_days",
            "gain",
            "grandfathered",
            "purchase_date_estimated"
        ]
    );
    let us_sale = crate::portfolio::Sale {
        symbol: "AAPL".into(),
        ..sale
    };
    let us = serde_json::to_value(crate::tax_export::us_schedule(
        &[us_sale],
        day("2025-01-01"),
    ))
    .unwrap();
    let us_keys: Vec<&str> = schedule_keys
        .iter()
        .copied()
        .filter(|k| {
            ![
                "virtual_digital_assets",
                "intraday_sales",
                "intraday_gain",
                "other_rules",
            ]
            .contains(k)
        })
        .collect();
    assert_eq!(keys(&us), us_keys);
    assert_eq!(
        keys(&us["sales"][0]),
        [
            "part",
            "description",
            "symbol",
            "quantity",
            "date_acquired",
            "date_sold",
            "proceeds",
            "cost_basis",
            "gain_or_loss",
            "date_acquired_estimated"
        ]
    );
}

/// One portfolio with a listed share, an equity fund, a liquid fund, crypto,
/// an SGB and a gold ETF: every India tax command must put the same two
/// sales in Sec. 111A/112A.
#[test]
fn india_tax_commands_classify_the_same_portfolio_alike() {
    use crate::offmarket::{Mark, PriceSource};
    // (symbol, AMFI category, gain on the sale, quote used for the open lots)
    let rows: [(&str, Option<&str>, f64); 6] = [
        ("TCS.NS", None, 1_000.0),
        ("MF:100001", Some("Equity Scheme - Flexi Cap Fund"), 3_000.0),
        ("MF:100002", Some("Debt Scheme - Liquid Fund"), 2_000.0),
        ("BTC-INR", None, 4_000.0),
        ("SGBNOV26.NS", None, 5_000.0),
        ("GOLDBEES.NS", None, 6_000.0),
    ];
    // 20 units bought at 100, 10 sold at 100 + gain/10, 10 left at 100 and priced at 90.
    let mut p = Portfolio::default();
    for (symbol, _, gain) in rows {
        p.add_lot(symbol, Lot::new(20.0, 100.0, "2026-05-04", "manual"));
        p.sell_fifo(symbol, 10.0, 100.0 + gain / 10.0, "2026-09-10", "manual")
            .unwrap();
    }
    let quotes: Vec<Quote> = rows
        .iter()
        .map(|(s, _, _)| quote(s, Some(90.0), None))
        .collect();
    let mut val = valued(&p, quotes);
    for (symbol, category, _) in rows {
        val.off.marks.push(Mark {
            symbol: symbol.into(),
            name: None,
            source: PriceSource::AmfiNav,
            as_of: None,
            category: category.map(String::from),
        });
    }
    let today = day("2026-11-01");

    // `tax in`: ₹1,000 + ₹3,000 short-term gain.
    let report = tax_report(Country::India, &p, &val, today);
    assert_eq!(report.realized.st_gain, 4_000.0);
    assert_eq!(report.sales.len(), 2);
    let left_out: Vec<&str> = report
        .other_rules
        .iter()
        .map(|o| o.symbol.as_str())
        .collect();
    for symbol in ["MF:100002", "BTC-INR", "SGBNOV26.NS", "GOLDBEES.NS"] {
        assert!(left_out.contains(&symbol), "tax in leaves out {symbol}");
    }

    // `tax export in`.
    let schedule =
        crate::tax_export::india_schedule(&p.realized, day("2026-04-01"), |_| None, &val.off);
    let mut exported: Vec<&str> = schedule.sales.iter().map(|r| r.symbol.as_str()).collect();
    exported.sort();
    assert_eq!(exported, ["MF:100001", "TCS.NS"]);
    assert_eq!(schedule.summary.stcg_gains, 4_000.0);
    assert_eq!(schedule.virtual_digital_assets, ["BTC-INR"]);

    // `tax advance`.
    let s = super::planning::advance_tax_schedule(&p, Some("2026-27"), &[], &val.off).unwrap();
    assert_eq!((s.total_stcg, s.total_ltcg), (4_000.0, 0.0));

    // `harvest`: only the open lots of the same two symbols are candidates.
    let h = super::holdings::compute_harvest(&p, &val, Market::In, today);
    let mut candidates: Vec<&str> = h
        .loss_candidates
        .iter()
        .map(|c| c.symbol.as_str())
        .collect();
    candidates.sort();
    assert_eq!(candidates, ["MF:100001", "TCS.NS"]);
    let skipped: Vec<&str> = h.other_rules.iter().map(|o| o.symbol.as_str()).collect();
    assert!(skipped.contains(&"BTC-INR"), "harvest leaves out crypto");
}

#[tokio::test]
async fn harvest_counts_the_sale_of_a_fund_no_longer_held() {
    use crate::views::fixtures::Fixture;
    // PPFAS Flexi Cap (equity fund, AMFI 122639): 100 units bought at 100 in
    // January 2024, all sold at 180 on 2026-06-10, so it is no longer held.
    // Long term: gain 100 × (180 - 100) = ₹8,000 of the ₹1,25,000 exemption.
    let mut p = Portfolio::default();
    p.add_lot("TCS.NS", Lot::new(10.0, 3500.0, "2024-01-10", "manual"));
    p.add_lot("MF:122639", Lot::new(100.0, 100.0, "2024-01-10", "manual"));
    p.sell_fifo("MF:122639", 100.0, 180.0, "2026-06-10", "manual")
        .unwrap();
    assert!(p.holding("MF:122639").is_none());
    let mut src = Fixture::default();
    src.navall = Some(crate::offmarket::tests::navall());

    let (h, _) = super::holdings::harvest_summary(&src, &p, Market::In, day("2026-11-01"))
        .await
        .unwrap();
    assert_eq!(h.ltcg_exemption_limit, 125_000.0);
    assert_eq!(h.ltcg_exemption_remaining, 117_000.0);

    // Valuing only what is held (the old `harvest`) loses the mark, so the
    // sale drops out and the whole exemption looks unused.
    let held = valued(&p, vec![quote("TCS.NS", Some(4100.0), None)]);
    let old = super::holdings::compute_harvest(&p, &held, Market::In, day("2026-11-01"));
    assert_eq!(old.ltcg_exemption_remaining, 125_000.0);
}

#[test]
fn tax_advance_for_an_old_year_uses_that_years_rate() {
    // Bought 2023-05-01 at 100, sold 2023-08-10 at 200: STCG ₹1,00,000 before
    // the July 2024 change, so 15% + 4% cess = ₹15,600 (not ₹20,800).
    let mut p = Portfolio::default();
    p.add_lot("TCS.NS", Lot::new(1000.0, 100.0, "2023-05-01", "manual"));
    p.sell_fifo("TCS.NS", 1000.0, 200.0, "2023-08-10", "manual")
        .unwrap();
    let off = crate::offmarket::OffMarket::default();
    let old = super::planning::advance_tax_schedule(&p, Some("2023-24"), &[], &off).unwrap();
    assert_eq!(old.total_stcg, 100_000.0);
    assert!((old.total_tax_estimated - 15_600.0).abs() < 1e-6);
}

/// Hand-calculated `harvest` cases (India, today is 2026-11-01).
mod harvest_math {
    use super::*;
    use crate::commands::holdings::compute_harvest;

    fn harvest(p: &Portfolio, quotes: Vec<Quote>) -> crate::commands::holdings::HarvestSummary {
        compute_harvest(p, &valued(p, quotes), Market::In, day("2026-11-01"))
    }

    #[test]
    fn a_losing_lot_behind_older_gains_is_not_offered_as_a_loss() {
        // TCS: 100 @ 500 (2020) and 100 @ 1,500 (2026), price 1,200. Selling 100
        // (FIFO) books +70,000 LTCG; selling 200 books +70,000 - 30,000 = +40,000.
        let p = lots(&[
            ("TCS.NS", 100.0, 500.0, "2020-01-01"),
            ("TCS.NS", 100.0, 1500.0, "2026-05-04"),
        ]);
        let h = harvest(&p, vec![quote("TCS.NS", Some(1200.0), None)]);
        assert!(h.loss_candidates.is_empty());
        assert_eq!(h.estimated_loss_tax_saved, 0.0);
        let b = &h.fifo_blocked[0];
        assert_eq!(b.symbol, "TCS.NS");
        assert_eq!(b.shares_to_sell, 200.0);
        assert_eq!(b.net_result, 40_000.0);
        assert_eq!(b.lot_loss, 30_000.0);
    }

    #[test]
    fn a_loss_behind_a_bigger_loss_is_sold_through_fifo() {
        // 100 @ 2,000 then 100 @ 1,000 (both this year), price 1,500: the first
        // lot loses 50,000, the second gains 50,000. Selling 100 books -50,000.
        let p = lots(&[
            ("A.NS", 100.0, 2000.0, "2026-04-10"),
            ("A.NS", 100.0, 1000.0, "2026-05-04"),
        ]);
        let h = harvest(&p, vec![quote("A.NS", Some(1500.0), None)]);
        let c = &h.loss_candidates[0];
        assert_eq!((c.shares, c.st_pnl, c.pnl), (100.0, -50_000.0, -50_000.0));
        assert!(h.fifo_blocked.is_empty());
    }

    #[test]
    fn tax_saved_comes_from_the_estimate_with_this_years_gains() {
        // STCG 60,000 already realized this year; a 50,000 short-term loss
        // harvested. Tax goes 12,000 + 480 cess = 12,480 -> 2,000 + 80 = 2,080.
        let mut p = lots(&[("LOSER.NS", 100.0, 2000.0, "2026-05-04")]);
        p.add_lot("WINNER.NS", Lot::new(100.0, 1000.0, "2026-05-04", "manual"));
        p.sell_fifo("WINNER.NS", 100.0, 1600.0, "2026-09-10", "manual")
            .unwrap();
        let h = harvest(&p, vec![quote("LOSER.NS", Some(1500.0), None)]);
        assert_eq!(h.total_st_loss_harvestable, 50_000.0);
        assert_eq!(h.estimated_loss_tax_saved, 10_400.0);
        assert_eq!(h.loss_candidates[0].estimated_tax_saved, 10_400.0);
    }

    #[test]
    fn a_loss_with_nothing_to_offset_saves_nothing() {
        let p = lots(&[("LOSER.NS", 100.0, 2000.0, "2026-05-04")]);
        let h = harvest(&p, vec![quote("LOSER.NS", Some(1500.0), None)]);
        assert_eq!(h.loss_candidates.len(), 1);
        assert_eq!(h.total_st_loss_harvestable, 50_000.0);
        assert_eq!(h.estimated_loss_tax_saved, 0.0);
    }

    #[test]
    fn candidates_share_the_remaining_exemption() {
        // Three long-term winners with 1,00,000 gain each; ₹1.25 lakh is exempt.
        // The first takes 1,00,000 (saves 12.5% + 4% cess = 13,000), the second
        // the last 25,000 (3,250), the third nothing.
        let p = lots(&[
            ("A.NS", 100.0, 1000.0, "2024-01-10"),
            ("B.NS", 100.0, 1000.0, "2024-01-10"),
            ("C.NS", 100.0, 1000.0, "2024-01-10"),
        ]);
        let quotes = ["A.NS", "B.NS", "C.NS"]
            .map(|s| quote(s, Some(2000.0), None))
            .to_vec();
        let h = harvest(&p, quotes);
        assert_eq!(h.gain_harvest_candidates.len(), 2);
        let saved: Vec<f64> = h
            .gain_harvest_candidates
            .iter()
            .map(|c| c.estimated_tax_saved)
            .collect();
        assert_eq!(saved, [13_000.0, 3_250.0]);
        assert_eq!(h.ltcg_exemption_remaining, 125_000.0);
    }

    #[test]
    fn us_holdings_are_not_counted_as_rupees() {
        let mut p = Portfolio::default();
        p.add_lot("AAPL", Lot::new(10.0, 200.0, "2025-01-02", "manual"));
        p.add_lot("OLD.NS", Lot::new(10.0, 100.0, "2025-01-02", "manual"));
        let mut aapl = quote("AAPL", Some(140.0), None);
        aapl.currency = Some("USD".into());
        let h = harvest(&p, vec![aapl, quote("OLD.NS", Some(90.0), None)]);
        assert_eq!(h.other_currency, ["AAPL"]);
        let names: Vec<&str> = h
            .loss_candidates
            .iter()
            .map(|c| c.symbol.as_str())
            .collect();
        assert_eq!(names, ["OLD.NS"]);
        // Only the rupee holding's loss: 10 x 10 = 100.
        assert_eq!(
            h.total_lt_loss_harvestable + h.total_st_loss_harvestable,
            100.0
        );
    }
}
