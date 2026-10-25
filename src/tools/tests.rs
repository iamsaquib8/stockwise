//! Tool layer tests: schemas, argument checks, size limits, and handlers
//! against fixtures (no network).

use super::*;
use crate::alerts::AlertStore;
use crate::api::{ChartData, Quote, SearchResult};
use crate::portfolio::{Lot, Portfolio};
use crate::views::fixtures::{Fixture, chart, quote};
use crate::watchlist::Watchlist;

/// The fixture market, with a year of daily bars for every known symbol
/// (enough for indicators, risk and backtests).
#[derive(Default)]
struct LongCharts(Fixture);

impl DataSource for LongCharts {
    async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        self.0.quotes(symbols).await
    }
    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        // Unknown symbols fail like Yahoo's 404.
        self.0.chart(symbol, range, interval).await?;
        let closes: Vec<f64> = (0..300)
            .map(|i| 100.0 + (i as f64 / 9.0).sin() * 8.0 + i as f64 * 0.05)
            .collect();
        Ok(chart(&closes, 1_700_000_000, 86_400, "INR"))
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.0.search(query).await
    }
    fn portfolio(&self) -> Result<Portfolio> {
        self.0.portfolio()
    }
    fn watchlist(&self) -> Result<Watchlist> {
        self.0.watchlist()
    }
    fn alerts(&self) -> Result<AlertStore> {
        self.0.alerts()
    }
}

fn ctx() -> ToolContext<LongCharts> {
    ToolContext::new(LongCharts::default(), Market::In)
}

async fn call(name: &str, args: Value) -> Result<Value> {
    ctx().call(name, &args).await
}

async fn data(name: &str, args: Value) -> Value {
    let v = call(name, args)
        .await
        .unwrap_or_else(|e| panic!("{name}: {e:#}"));
    assert_eq!(v["note"], NOTE, "{name} has no note");
    v["data"].clone()
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

// ── schemas ──

#[test]
fn tool_names_are_unique_and_valid() {
    let mut names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
    names.push(ORDER_PREVIEW.name);
    for n in &names {
        assert!(
            !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "bad tool name {n}"
        );
    }
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "duplicate tool names");
    // The tools the roadmap promises.
    for wanted in [
        "quote",
        "history",
        "technical",
        "risk",
        "search",
        "markets",
        "mf_search",
        "mf_nav",
        "mf_sip",
        "options_chain",
        "options_pcr",
        "options_maxpain",
        "options_payoff",
        "screen",
        "backtest",
        "portfolio_summary",
        "holdings_lots",
        "tax_estimate",
        "calendar",
        "goal_sip",
        "goal_retire",
        "goal_education",
        "alerts_list",
        "watchlist",
    ] {
        assert!(names.contains(&wanted), "missing tool {wanted}");
    }
}

#[test]
fn schemas_are_closed_objects_with_described_params() {
    for t in TOOLS.iter().chain([&ORDER_PREVIEW]) {
        assert!(t.description.len() > 40, "{} needs a description", t.name);
        assert!(!t.title.is_empty());
        let s = t.input_schema();
        assert_eq!(s["type"], "object", "{}", t.name);
        assert_eq!(s["additionalProperties"], false, "{}", t.name);
        let props = s["properties"].as_object().unwrap();
        assert_eq!(props.len(), t.params.len(), "{}: duplicate params", t.name);
        for (name, p) in props {
            let desc = p["description"].as_str().unwrap_or("");
            assert!(desc.len() > 5, "{}.{} needs a description", t.name, name);
            let ty = p["type"].as_str().unwrap();
            assert!(
                ["string", "number", "integer", "boolean", "array"].contains(&ty),
                "{}.{}: {ty}",
                t.name,
                name
            );
            if let Some(values) = p.get("enum") {
                assert!(!values.as_array().unwrap().is_empty());
            }
            if let (Some(lo), Some(hi)) = (p["minimum"].as_f64(), p["maximum"].as_f64()) {
                assert!(lo < hi, "{}.{} range", t.name, name);
            }
        }
        for r in s["required"].as_array().unwrap() {
            assert!(props.contains_key(r.as_str().unwrap()), "{}: {r}", t.name);
        }
    }
}

#[test]
fn market_parameter_lists_every_market() {
    let codes = market_codes();
    for m in ["in", "us", "crypto", "fx", "commodity", "uk", "jp"] {
        assert!(codes.iter().any(|c| c == m), "{m} missing from {codes:?}");
    }
    let quote = TOOLS.iter().find(|t| t.name == "quote").unwrap();
    assert_eq!(
        quote.input_schema()["properties"]["market"]["enum"]
            .as_array()
            .unwrap()
            .len(),
        codes.len()
    );
}

#[test]
fn backtest_description_names_every_builtin_strategy() {
    let t = TOOLS.iter().find(|t| t.name == "backtest").unwrap();
    let desc = t.params.iter().find(|p| p.name == "strategy").unwrap();
    for name in crate::backtest::rules::builtin_names() {
        assert!(
            desc.description.contains(name),
            "strategy {name} not listed"
        );
    }
}

#[test]
fn order_preview_is_offered_only_when_allowed() {
    let mut c = ctx();
    assert!(c.find("order_preview").is_none());
    assert_eq!(c.tools().len(), TOOLS.len());
    c.allow_orders = true;
    assert!(c.find("order_preview").is_some());
    // Nothing that sounds like placing or cancelling an order.
    for t in c.tools() {
        for word in ["place", "cancel", "execute", "submit"] {
            assert!(!t.name.contains(word), "{}", t.name);
        }
    }
}

// ── arguments ──

fn tool(name: &str) -> &'static Tool {
    TOOLS.iter().find(|t| t.name == name).unwrap()
}

#[test]
fn args_reject_unknown_and_missing_arguments() {
    let e = Args::parse(tool("quote"), &json!({"symbol": ["TCS"]})).unwrap_err();
    assert!(e.to_string().contains("Unknown argument 'symbol'"), "{e}");
    assert!(e.to_string().contains("symbols"), "{e}");
    let e = Args::parse(tool("quote"), &json!({})).unwrap_err();
    assert!(
        e.to_string()
            .contains("Missing required argument 'symbols'")
    );
    let e = Args::parse(tool("watchlist"), &json!({"x": 1})).unwrap_err();
    assert!(e.to_string().contains("accepted: none"), "{e}");
    assert!(Args::parse(tool("watchlist"), &Value::Null).is_ok());
    assert!(Args::parse(tool("quote"), &json!([1])).is_err());
}

#[test]
fn args_accept_what_small_models_send() {
    // Arguments as a JSON string, numbers as strings, a list as "A, B".
    let a = Args::parse(
        tool("goal_sip"),
        &json!(r#"{"target": "1,00,00,000", "years": "15"}"#),
    )
    .unwrap();
    assert_eq!(a.f64("target"), Some(10_000_000.0));
    assert_eq!(a.i64("years"), Some(15));
    let a = Args::parse(tool("quote"), &json!({"symbols": "TCS, INFY"})).unwrap();
    assert_eq!(a.list("symbols"), ["TCS", "INFY"]);
    let a = Args::parse(
        tool("screen"),
        &json!({"category": "Growth", "market": "US"}),
    )
    .unwrap();
    assert_eq!(a.str("category"), Some("growth"));
    assert_eq!(a.market(Market::In), Market::Us);
    let a = Args::parse(tool("mf_nav"), &json!({"scheme": 122639})).unwrap();
    assert_eq!(a.str("scheme"), Some("122639"));
}

#[test]
fn args_check_ranges_types_and_enums() {
    let bad = [
        ("goal_sip", json!({"target": 1e6, "years": 0})),
        ("goal_sip", json!({"target": 1e6, "years": 2.5})),
        ("goal_sip", json!({"target": "lots", "years": 5})),
        ("history", json!({"symbol": "TCS", "limit": 5000})),
        ("history", json!({"symbol": "TCS", "period": "7x"})),
        ("history", json!({"symbol": ""})),
        ("quote", json!({"symbols": []})),
        (
            "quote",
            json!({"symbols": (0..25).map(|i| format!("S{i}")).collect::<Vec<_>>()}),
        ),
        ("quote", json!({"symbols": ["TCS"], "market": "mars"})),
        ("markets", json!({"crypto": "maybe"})),
    ];
    for (name, args) in bad {
        assert!(
            Args::parse(tool(name), &args).is_err(),
            "{name} accepted {args}"
        );
    }
}

// ── envelope and size ──

#[test]
fn envelope_adds_the_note_and_cuts_long_lists() {
    let small = envelope(json!({"a": [1, 2, 3]}), 10_000);
    assert_eq!(small["data"]["a"].as_array().unwrap().len(), 3);
    assert_eq!(small["note"], NOTE);
    assert!(small.get("truncated").is_none());

    let rows: Vec<Value> = (0..2000).map(|i| json!({"i": i, "x": "row"})).collect();
    let big = envelope(json!({"rows": rows, "keep": [1, 2]}), 4_000);
    assert!(serde_json::to_string(&big).unwrap().len() <= 4_000);
    assert!(big["truncated"].is_string());
    let kept = big["data"]["rows"].as_array().unwrap();
    assert!(!kept.is_empty() && kept.len() < 2000);
    // The first rows are kept, in order.
    assert_eq!(kept[0]["i"], 0);
    assert_eq!(big["data"]["keep"], json!([1, 2]));
}

// ── handlers ──

#[tokio::test]
async fn every_tool_has_a_handler() {
    let mut c = ctx();
    c.allow_orders = true;
    for t in c.tools() {
        // Missing arguments fail before the handler; a known tool with no
        // handler would say so instead.
        let err = c
            .call(t.name, &json!({}))
            .await
            .err()
            .map(|e| e.to_string());
        if let Some(e) = err {
            assert!(!e.contains("has no handler"), "{}: {e}", t.name);
        }
    }
    let e = c.call("place_order", &json!({})).await.unwrap_err();
    assert!(e.to_string().contains("Unknown tool 'place_order'"), "{e}");
}

#[tokio::test]
async fn quote_resolves_bare_tickers_and_lists_missing() {
    let d = data("quote", json!({"symbols": ["TCS", "NOPE"]})).await;
    assert_eq!(d["quotes"][0]["symbol"], "TCS.NS");
    assert_eq!(d["quotes"][0]["price"], 4100.0);
    assert_eq!(d["missing"], json!(["NOPE.NS"]));
    let e = call("quote", json!({"symbols": ["NOPE"]}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("No quote for NOPE.NS"), "{e}");
}

#[tokio::test]
async fn history_keeps_the_latest_bars_and_summarizes() {
    let d = data(
        "history",
        json!({"symbol": "TCS", "period": "1y", "limit": 5}),
    )
    .await;
    assert_eq!(d["bar_count"], 300);
    assert_eq!(d["bars"].as_array().unwrap().len(), 5);
    assert_eq!(d["bars_shown"], 5);
    assert!(d["change_percent"].is_number());
    assert!(d["period_high"].as_f64().unwrap() >= d["period_low"].as_f64().unwrap());
}

#[tokio::test]
async fn technical_and_risk_match_their_json_commands() {
    let t = data("technical", json!({"symbol": "INFY"})).await;
    assert_eq!(t["symbol"], "INFY.NS");
    for k in ["rsi_14", "macd", "bollinger", "sma_200", "bias"] {
        assert!(t.get(k).is_some(), "technical lacks {k}");
    }
    let r = data("risk", json!({"symbol": "INFY", "period": "2y"})).await;
    for k in [
        "volatility_percent",
        "sharpe",
        "max_drawdown",
        "var_95_percent",
    ] {
        assert!(r.get(k).is_some(), "risk lacks {k}");
    }
    let e = call("risk", json!({"symbol": "NOPE"})).await.unwrap_err();
    assert!(
        format!("{e:#}").contains("No price history for NOPE.NS"),
        "{e:#}"
    );
}

#[tokio::test]
async fn markets_watchlist_alerts_and_search() {
    let m = data("markets", json!({})).await;
    assert_eq!(m["indices"][0]["symbol"], "^NSEI");
    let w = data("watchlist", json!({})).await;
    assert_eq!(w["watchlist"].as_array().unwrap().len(), 2);
    let a = data("alerts_list", json!({})).await;
    let status: Vec<&str> = a["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap())
        .collect();
    assert_eq!(status, ["triggered", "waiting", "no_price"]);
    let s = data("search", json!({"query": "tcs", "limit": 1})).await;
    assert_eq!(s["results"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn portfolio_tools_never_value_missing_prices() {
    let p = data("portfolio_summary", json!({})).await;
    assert_eq!(keys(&p), ["holdings", "totals", "unpriced", "unconverted"]);
    assert_eq!(p["unpriced"], json!(["DELISTED.NS"]));
    assert_eq!(p["totals"]["value"], 41000.0 + 7500.0);

    let lots = data("holdings_lots", json!({"symbol": "TCS"})).await;
    assert_eq!(lots["total_lots"], 1);
    assert_eq!(lots["lots"][0]["symbol"], "TCS.NS");
    assert_eq!(lots["lots"][0]["term"], "short");
    assert_eq!(lots["long_term_after_months"], 12);
    let e = call("holdings_lots", json!({"symbol": "WIPRO"}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not in the portfolio"), "{e}");

    let tax = data("tax_estimate", json!({})).await;
    assert_eq!(tax["country"], "India");
    assert!(tax["if_sold_today"]["short_term_gains"].as_f64().unwrap() > 0.0);
    assert_eq!(tax["unpriced"], json!(["DELISTED.NS"]));
    assert!(tax["ltcg_exemption_left"].is_number());
}

#[tokio::test]
async fn lots_are_long_term_after_twelve_months() {
    let mut f = Fixture::default();
    f.portfolio = Portfolio::default();
    f.portfolio
        .add_lot("TCS.NS", Lot::new(4.0, 3000.0, "2023-01-02", "manual"));
    let c = ToolContext::new(LongCharts(f), Market::In);
    let v = c.call("holdings_lots", &json!({})).await.unwrap();
    assert_eq!(v["data"]["lots"][0]["term"], "long");
    assert!(v["data"]["lots"][0]["days_held"].as_i64().unwrap() > 365);
}

#[tokio::test]
async fn goals_are_pure_and_rounded() {
    let sip = data("goal_sip", json!({"target": 10_000_000, "years": 15})).await;
    assert_eq!(sip["goal"], "sip");
    assert_eq!(sip["currency"], "INR");
    let monthly = sip["monthly_sip"].as_f64().unwrap();
    assert!((19_000.0..21_000.0).contains(&monthly), "{monthly}");
    assert_eq!(sip["schedule"].as_array().unwrap().len(), 15);

    let r = data(
        "goal_retire",
        json!({"monthly_expense": 80000, "years_to_retire": 20, "years_in_retirement": 25, "market": "us"}),
    )
    .await;
    assert_eq!(r["goal"], "retire");
    assert_eq!(r["currency"], "USD");
    assert!(r["corpus"].as_f64().unwrap() > r["monthly_expense_today"].as_f64().unwrap());

    let e = data("goal_education", json!({"cost": 2_500_000, "years": 10})).await;
    assert!(e["future_cost"].as_f64().unwrap() > 2_500_000.0);
}

#[tokio::test]
async fn screen_runs_on_the_quotes_of_the_universe() {
    let d = data("screen", json!({"category": "momentum", "limit": 2})).await;
    assert_eq!(d["universe"], "India");
    let stocks = d["stocks"].as_array().unwrap();
    assert_eq!(stocks.len(), 2);
    // Biggest gainers first: TCS +1.5%, RELIANCE +0.8%.
    assert_eq!(stocks[0]["symbol"], "TCS.NS");
    assert_eq!(stocks[1]["symbol"], "RELIANCE.NS");
    let e = call("screen", json!({"category": "growth", "market": "crypto"}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("stock markets only"), "{e}");
}

#[tokio::test]
async fn backtest_uses_builtins_only_and_drops_the_equity_curve() {
    let d = data(
        "backtest",
        json!({"symbol": "TCS", "strategy": "sma", "period": "1y"}),
    )
    .await;
    assert_eq!(d["symbol"], "TCS.NS");
    assert!(d.get("equity").is_none());
    assert!(d["total_return_percent"].is_number());
    assert!(d["trades"].as_array().unwrap().len() <= 10);
    let e = call(
        "backtest",
        json!({"symbol": "TCS", "strategy": "file:/etc/passwd"}),
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("Built-in strategies"), "{e}");
}

#[tokio::test]
async fn order_preview_sends_nothing_and_returns_the_command() {
    let mut c = ctx();
    c.allow_orders = true;
    let v = c
        .call(
            "order_preview",
            &json!({"side": "buy", "symbol": "TCS", "quantity": 2, "limit": 4000, "broker": "zerodha"}),
        )
        .await
        .unwrap();
    let d = &v["data"];
    assert!(d["status"].as_str().unwrap().contains("nothing was sent"));
    assert_eq!(d["estimated_value_inr"], 8000.0);
    assert_eq!(
        d["command"],
        "stockwise trade buy TCS 2 --broker zerodha --limit 4000"
    );
    assert!(
        d["dry_run_command"]
            .as_str()
            .unwrap()
            .ends_with("--dry-run")
    );
    // A buy stop below the market is refused, as `trade` would.
    let e = c
        .call(
            "order_preview",
            &json!({"side": "buy", "symbol": "TCS", "quantity": 1, "trigger": 10, "broker": "angel"}),
        )
        .await;
    assert!(e.is_err());
}

#[tokio::test]
async fn failures_are_errors_the_model_can_read() {
    let f = Fixture::default();
    *f.fail.lock().unwrap() = Some("Yahoo Finance is unreachable".into());
    let c = ToolContext::new(f, Market::In);
    let e = c
        .call("quote", &json!({"symbols": ["TCS"]}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("unreachable"), "{e}");
    let _ = quote("X", None, 0.0, "INR");
}
