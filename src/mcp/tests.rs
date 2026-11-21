//! MCP server tests: the JSON-RPC protocol over an in-memory stdio pipe and
//! over Streamable HTTP, against fixture data (no network).

use super::*;
use crate::views::fixtures::Fixture;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt as _;

fn server(allow_orders: bool) -> StockwiseMcp<Fixture> {
    let mut ctx = ToolContext::new(Fixture::default(), Market::In);
    ctx.allow_orders = allow_orders;
    StockwiseMcp::new(ctx)
}

/// A JSON-RPC client on one end of a pipe, the server on the other — what
/// a host does with `stockwise mcp` over stdio.
struct Client {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    next_id: u64,
}

impl Client {
    async fn start(server: StockwiseMcp<Fixture>) -> Self {
        let (ours, theirs) = tokio::io::duplex(1 << 20);
        let (r, w) = tokio::io::split(theirs);
        tokio::spawn(async move {
            if let Ok(running) = server.serve((r, w)).await {
                let _ = running.waiting().await;
            }
        });
        let (r, w) = tokio::io::split(ours);
        let mut c = Client {
            writer: w,
            lines: BufReader::new(r).lines(),
            next_id: 0,
        };
        let init = c
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0"},
                }),
            )
            .await;
        assert_eq!(init["result"]["serverInfo"]["name"], "stockwise", "{init}");
        c.notify("notifications/initialized").await;
        c
    }

    async fn send(&mut self, msg: Value) {
        let mut line = msg.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
    }

    async fn notify(&mut self, method: &str) {
        self.send(json!({"jsonrpc": "2.0", "method": method})).await;
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line =
                tokio::time::timeout(std::time::Duration::from_secs(10), self.lines.next_line())
                    .await
                    .expect("server answered in time")
                    .unwrap()
                    .expect("server kept the pipe open");
            let v: Value = serde_json::from_str(&line).expect("only JSON-RPC on stdout");
            if v["id"] == id {
                return v;
            }
        }
    }
}

#[tokio::test]
async fn initialize_advertises_tools_resources_and_prompts() {
    let mut c = Client::start(server(false)).await;
    let tools = c.request("tools/list", json!({})).await;
    let list = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(list.len(), crate::tools::TOOLS.len());
    let quote = list.iter().find(|t| t["name"] == "quote").unwrap();
    assert_eq!(quote["inputSchema"]["required"], json!(["symbols"]));
    assert_eq!(quote["annotations"]["readOnlyHint"], true);
    assert_eq!(quote["annotations"]["destructiveHint"], false);
    let personal = list
        .iter()
        .find(|t| t["name"] == "portfolio_summary")
        .unwrap();
    // It fetches live prices, so it reaches outside the machine.
    assert_eq!(personal["annotations"]["openWorldHint"], true);
    let lots = list.iter().find(|t| t["name"] == "holdings_lots").unwrap();
    assert_eq!(lots["annotations"]["openWorldHint"], false);
    let checkup = list
        .iter()
        .find(|t| t["name"] == "portfolio_checkup")
        .unwrap();
    assert_eq!(checkup["annotations"]["openWorldHint"], true);
    assert!(list.iter().all(|t| t["name"] != "order_preview"));

    let ping = c.request("ping", json!({})).await;
    assert!(ping["result"].is_object(), "{ping}");
}

#[tokio::test]
async fn tools_call_returns_structured_results_with_the_note() {
    let mut c = Client::start(server(false)).await;
    let r = c
        .request(
            "tools/call",
            json!({"name": "quote", "arguments": {"symbols": ["TCS"]}}),
        )
        .await;
    let result = &r["result"];
    assert_eq!(result["isError"], false, "{r}");
    assert_eq!(
        result["structuredContent"]["data"]["quotes"][0]["symbol"],
        "TCS.NS"
    );
    assert_eq!(result["structuredContent"]["note"], crate::tools::NOTE);
    // The same JSON as text, for clients that ignore structured content.
    let text = result["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed, result["structuredContent"]);

    // Bad arguments are a tool error the model can read, not a protocol error.
    let r = c
        .request(
            "tools/call",
            json!({"name": "goal_sip", "arguments": {"target": 100000}}),
        )
        .await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Missing required argument 'years'")
    );

    // An unknown tool is a protocol error.
    let r = c
        .request(
            "tools/call",
            json!({"name": "place_order", "arguments": {}}),
        )
        .await;
    assert!(r["error"].is_object(), "{r}");
}

#[tokio::test]
async fn resources_are_the_portfolio_watchlist_and_alerts() {
    let mut c = Client::start(server(false)).await;
    let r = c.request("resources/list", json!({})).await;
    let uris: Vec<&str> = r["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "stockwise://portfolio",
            "stockwise://watchlist",
            "stockwise://alerts",
            "stockwise://profile",
            "stockwise://picks/latest",
            "stockwise://track-record",
            "stockwise://methodology",
        ]
    );
    let r = c
        .request("resources/read", json!({"uri": "stockwise://portfolio"}))
        .await;
    let content = &r["result"]["contents"][0];
    assert_eq!(content["mimeType"], "application/json");
    let p: Value = serde_json::from_str(content["text"].as_str().unwrap()).unwrap();
    assert_eq!(p["unpriced"], json!(["DELISTED.NS"]));
    let r = c
        .request("resources/read", json!({"uri": "stockwise://profile"}))
        .await;
    let content = &r["result"]["contents"][0];
    assert_eq!(content["mimeType"], "application/json");
    let prof: Value = serde_json::from_str(content["text"].as_str().unwrap()).unwrap();
    assert!(prof["mode"].is_string());
    let r = c
        .request("resources/read", json!({"uri": "stockwise://secrets"}))
        .await;
    assert!(r["error"].is_object(), "{r}");
}

#[tokio::test]
async fn prompts_fill_in_their_arguments() {
    let mut c = Client::start(server(false)).await;
    let r = c.request("prompts/list", json!({})).await;
    let names: Vec<&str> = r["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "daily_briefing",
            "should_i_buy",
            "exit_check",
            "smallcap_short_scan",
            "tax_aware_rebalance",
            "weekly_review",
            "mf_review",
            "advance_tax_check",
            "review_portfolio",
            "tax_harvest_check",
            "explain_option_chain",
            "onboard_profile"
        ]
    );
    let onboard = prompt_text("onboard_profile", &Map::new()).unwrap();
    assert!(onboard.contains("profile_questions") && onboard.contains("profile_onboard"));
    assert!(onboard.contains("one at a time"));
    let r = c
        .request(
            "prompts/get",
            json!({"name": "explain_option_chain", "arguments": {"symbol": "BANKNIFTY", "expiry": "2026-11-27"}}),
        )
        .await;
    let text = r["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(
        text.contains("BANKNIFTY for the 2026-11-27 expiry"),
        "{text}"
    );
    assert!(text.contains("options_chain"));
    let r = c
        .request("prompts/get", json!({"name": "explain_option_chain"}))
        .await;
    assert!(r["error"].is_object(), "a symbol is required: {r}");
    for name in ["review_portfolio", "tax_harvest_check"] {
        let t = prompt_text(name, &Map::new()).unwrap();
        assert!(t.contains("portfolio") || t.contains("tax_estimate"));
        assert!(!t.to_lowercase().contains("you should buy"));
    }
    assert!(
        prompt_text("tax_harvest_check", &Map::new())
            .unwrap()
            .contains("1.25 lakh")
    );
}

#[tokio::test]
async fn order_previews_only_with_allow_orders() {
    let mut c = Client::start(server(true)).await;
    let r = c.request("tools/list", json!({})).await;
    let names: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"order_preview"));
    let r = c
        .request(
            "tools/call",
            json!({"name": "order_preview", "arguments": {"side": "sell", "symbol": "INFY", "quantity": 3, "broker": "angel"}}),
        )
        .await;
    let d = &r["result"]["structuredContent"]["data"];
    assert_eq!(d["command"], "stockwise trade sell INFY 3 --broker angel");
    assert_eq!(d["estimated_value_inr"], 4500.0);
}

// ── Streamable HTTP ──

const TOKEN: &str = "0123456789abcdef0123";

fn initialize_request(host: &str, token: Option<&str>, origin: Option<&str>) -> Request<Body> {
    let mut req = Request::post("/mcp")
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    if let Some(o) = origin {
        req = req.header("origin", o);
    }
    req.body(Body::from(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            },
        })
        .to_string(),
    ))
    .unwrap()
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&bytes)))
}

#[tokio::test]
async fn http_answers_loopback_hosts_without_a_token() {
    let app = http_app(server(false), None, http_config(false));
    let resp = app
        .clone()
        .oneshot(initialize_request("127.0.0.1:8788", None, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["result"]["serverInfo"]["name"], "stockwise", "{v}");

    // DNS rebinding: a page on evil.example resolving to 127.0.0.1.
    let resp = app
        .clone()
        .oneshot(initialize_request("evil.example:8788", None, None))
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "{}", resp.status());
    // A web page on another origin, even addressed to localhost.
    let resp = app
        .oneshot(initialize_request(
            "127.0.0.1:8788",
            None,
            Some("https://evil.example"),
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "{}", resp.status());
}

#[tokio::test]
async fn http_with_a_token_needs_the_bearer_header() {
    let app = http_app(server(false), Some(TOKEN.into()), http_config(true));
    let resp = app
        .clone()
        .oneshot(initialize_request("mcp.example.net", None, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = app
        .clone()
        .oneshot(initialize_request(
            "mcp.example.net",
            Some("wrong-token-0000000"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = app
        .oneshot(initialize_request("mcp.example.net", Some(TOKEN), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_json(resp).await["result"]["serverInfo"]["name"],
        "stockwise"
    );
}

#[tokio::test]
async fn http_calls_a_tool_statelessly() {
    let app = http_app(server(false), None, http_config(false));
    let req = Request::post("/mcp")
        .header("host", "localhost:8788")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {"name": "goal_education", "arguments": {"cost": 1000000, "years": 5}},
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(
        v["result"]["structuredContent"]["data"]["goal"], "education",
        "{v}"
    );
}

/// Market data from the fixture; the portfolio file is broken.
struct BrokenPortfolio(Fixture);

impl DataSource for BrokenPortfolio {
    async fn quotes(&self, symbols: &[&str]) -> anyhow::Result<Vec<crate::api::Quote>> {
        self.0.quotes(symbols).await
    }
    async fn chart(
        &self,
        symbol: &str,
        range: &str,
        interval: &str,
    ) -> anyhow::Result<crate::api::ChartData> {
        self.0.chart(symbol, range, interval).await
    }
    async fn search(&self, query: &str) -> anyhow::Result<Vec<crate::api::SearchResult>> {
        self.0.search(query).await
    }
    fn portfolio(&self) -> anyhow::Result<crate::portfolio::Portfolio> {
        anyhow::bail!("/home/asha/.local/share/stockwise/portfolio.json is not valid JSON")
    }
}

#[tokio::test]
async fn local_file_errors_keep_paths_out_of_tool_results() {
    let server = StockwiseMcp::new(ToolContext::new(
        BrokenPortfolio(Fixture::default()),
        Market::In,
    ));
    let r = server.call("portfolio_summary", &json!({})).await;
    assert_eq!(r.is_error, Some(true));
    let text = serde_json::to_string(&r.content).unwrap();
    assert!(
        text.contains("Could not read the local StockWise data"),
        "{text}"
    );
    assert!(!text.contains("/home/asha"), "{text}");
    // Market data errors still say what went wrong.
    let r = server.call("quote", &json!({"symbols": ["NOPE"]})).await;
    assert!(
        serde_json::to_string(&r.content)
            .unwrap()
            .contains("No quote for NOPE.NS")
    );
}

// ── 0.9: annotations, output schemas, completion, subscriptions, prompts ──

/// A scratch directory with a watchlist, alerts, a config file and a
/// portfolio file, for subscriptions and the picks log.
fn scratch(name: &str) -> Files {
    let dir = std::env::temp_dir().join(format!(
        "stockwise-mcp-{}-{}-{}",
        name,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["watchlist.json", "alerts.json", "portfolio.json"] {
        std::fs::write(dir.join(f), "{}").unwrap();
    }
    std::fs::write(dir.join("config.toml"), "").unwrap();
    Files {
        portfolio: dir.join("portfolio.json"),
        config: dir.join("config.toml"),
        data_dir: dir,
    }
}

fn writer_server(files: &Files) -> StockwiseMcp<Fixture> {
    let mut ctx = ToolContext::new(Fixture::default(), Market::In);
    ctx.workspace = Some(crate::tools::Workspace {
        dir: files.data_dir.clone(),
        config: files.config.clone(),
        portfolio: files.portfolio.clone(),
        source: "test",
        allow_portfolio: true,
    });
    StockwiseMcp::new(ctx).with_files(files.clone())
}

#[tokio::test]
async fn every_tool_has_annotations_and_the_main_ones_an_output_schema() {
    let files = scratch("annotations");
    let mut c = Client::start(writer_server(&files)).await;
    let r = c.request("tools/list", json!({})).await;
    let list = r["result"]["tools"].as_array().unwrap();
    let by_name = |n: &str| list.iter().find(|t| t["name"] == n).unwrap();
    for t in list {
        let a = &t["annotations"];
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(a[hint].is_boolean(), "{} lacks {hint}: {a}", t["name"]);
        }
        let name = t["name"].as_str().unwrap();
        let write = crate::tools::access(name) != crate::tools::Access::Read;
        let logs = crate::tools::LOGGING_READS.contains(&name);
        assert_eq!(a["readOnlyHint"], !write && !logs, "{name}");
        if write {
            assert_eq!(a["openWorldHint"], false, "{}", t["name"]);
        } else {
            assert_eq!(a["destructiveHint"], false, "{}", t["name"]);
            assert_eq!(a["idempotentHint"], true, "{}", t["name"]);
        }
    }
    for (name, destructive, idempotent) in [
        ("watchlist_add", false, true),
        ("watchlist_remove", true, true),
        ("alert_add", false, false),
        ("alert_remove", true, false),
        ("goal_save", true, true),
        ("profile_set", true, true),
        ("portfolio_add", false, false),
        ("portfolio_sell", true, false),
    ] {
        let a = &by_name(name)["annotations"];
        assert_eq!(a["destructiveHint"], destructive, "{name}");
        assert_eq!(a["idempotentHint"], idempotent, "{name}");
    }
    for name in [
        "quote",
        "history",
        "verdict",
        "verdicts",
        "picks",
        "picks_review",
        "portfolio_summary",
        "portfolio_analyze",
        "portfolio_checkup",
        "rebalance",
        "tax_estimate",
        "market_conditions",
    ] {
        let s = &by_name(name)["outputSchema"];
        assert_eq!(s["type"], "object", "{name}: {s}");
        assert_eq!(s["required"], json!(["data", "note"]), "{name}");
        assert!(s["properties"]["data"]["required"].is_array(), "{name}");
    }
    assert!(by_name("goal_sip").get("outputSchema").is_none());
}

/// Check one tool result (the envelope) against the tool's output schema.
fn assert_follows_schema(tool: &str, result: &Value) {
    let schema = crate::tools::schema::output_schema(tool).expect("schema");
    if let Err(e) = crate::tools::schema::validate(&schema, result) {
        panic!("{tool}: {e}\n{result}");
    }
}

fn wrapped(data: Value) -> Value {
    crate::tools::envelope(data, crate::tools::MAX_RESULT_BYTES)
}

#[tokio::test]
async fn structured_results_follow_their_output_schemas() {
    // Tools that run on fixture data, through the protocol.
    let mut c = Client::start(server(false)).await;
    for (name, args) in [
        ("quote", json!({"symbols": ["TCS", "INFY", "NOPE"]})),
        ("history", json!({"symbol": "TCS", "period": "1mo"})),
        ("portfolio_summary", json!({})),
        ("portfolio_analyze", json!({"quick": true})),
        ("tax_estimate", json!({})),
    ] {
        let r = c
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let result = &r["result"];
        assert_eq!(result["isError"], false, "{name}: {r}");
        assert_follows_schema(name, &result["structuredContent"]);
    }

    // Tools that need the network, from the same Rust values their handlers
    // serialize.
    let mut research = crate::research::Research::default();
    research.symbol = "TCS.NS".into();
    research.quote.regular_market_price = Some(100.0);
    research.quote.fifty_day_average = Some(95.0);
    research.quote.two_hundred_day_average = Some(90.0);
    let cards = crate::insights::generate_rich_insights(&research);
    let v = crate::verdict::generate_verdict(&research, &cards);
    assert_follows_schema("verdict", &wrapped(serde_json::to_value(&v).unwrap()));

    let pick = crate::picks::Pick {
        symbol: "TCS.NS".into(),
        name: Some("TCS".into()),
        price: Some(4100.0),
        change_percent: Some(1.2),
        currency: Some("INR".into()),
        sector: Some("Technology".into()),
        market_cap: Some(1.5e13),
        score: 71.0,
        value: Some(40.0),
        quality: Some(90.0),
        growth: None,
        trend: Some(60.0),
        risk: Some(f64::NAN),
        coverage: 0.8,
        flags: Vec::new(),
        quoted_at: None,
        action: None,
    };
    let mut flagged = pick.clone();
    flagged.flags = vec!["P/E below 3".into()];
    let run = crate::picks::Run {
        picks: vec![pick],
        universe: 50,
        scanned: 40,
        below_floor: 10,
        enriched: 30,
        flagged: vec![flagged],
        thin_data: 2,
        floors: None,
        horizon: crate::picks::Horizon::Short,
        methodology: crate::picks::METHODOLOGY,
        meta: crate::provenance::Meta::default(),
    };
    assert_follows_schema("picks", &wrapped(crate::tools::extra::picks_value(&run)));

    let day = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
    let entry = crate::picks_log::Entry {
        date: day("2026-09-01"),
        kind: "pick".into(),
        horizon: "long".into(),
        market: "in".into(),
        universe: "India".into(),
        symbol: "TCS.NS".into(),
        rank: 1,
        score: 71.0,
        price: 3500.0,
        currency: Some("INR".into()),
        benchmark_price: Some(100.0),
        methodology: crate::picks::METHODOLOGY.into(),
    };
    let prices = std::collections::HashMap::from([("TCS.NS".to_string(), 4100.0)]);
    let review = crate::picks_log::review(
        &[entry],
        "in",
        "long",
        day("2026-08-01"),
        day("2026-11-21"),
        &prices,
        &std::collections::HashMap::new(),
        Some(crate::picks_log::Benchmark {
            name: "NIFTY 50",
            now: Some(104.0),
        }),
    );
    let mut rv = serde_json::to_value(&review).unwrap();
    rv["market"] = json!("in");
    rv["horizon"] = json!("long");
    assert_follows_schema("picks_review", &wrapped(rv));

    let mut plan = crate::rebalance::RebalancePlan::empty("INR");
    plan.total_value = 100_000.0;
    plan.items.push(crate::rebalance::RebalanceItem {
        symbol: "TCS.NS".into(),
        current_value: 60_000.0,
        current_pct: 60.0,
        target_pct: 50.0,
        projected_value: 50_000.0,
        projected_pct: 50.0,
        price: 4100.0,
        diff_value: -10_000.0,
        diff_shares: -2.4,
        action_type: crate::rebalance::RebalanceActionType::Sell,
        action_display: "Sell 2".into(),
        is_fund: false,
        sector: Some("Technology".into()),
        held: true,
        ..Default::default()
    });
    assert_follows_schema("rebalance", &wrapped(serde_json::to_value(&plan).unwrap()));

    let checkup = crate::checkup::evaluate(&crate::checkup::CheckupInput::default(), Vec::new());
    assert_follows_schema(
        "portfolio_checkup",
        &wrapped(serde_json::to_value(&checkup).unwrap()),
    );

    let quotes: Vec<crate::api::Quote> = ["TCS.NS", "INFY.NS"]
        .iter()
        .map(|s| crate::views::fixtures::quote(s, Some(100.0), 1.0, "INR"))
        .collect();
    let closes: Vec<f64> = (0..300).map(|i| 100.0 + i as f64 * 0.1).collect();
    let report = crate::conditions::Report {
        market: "India".into(),
        index: "NIFTY 50".into(),
        trend: crate::conditions::trend(&closes),
        volatility: crate::conditions::volatility("India VIX", &closes),
        breadth: crate::conditions::breadth(&quotes),
        valuation: crate::conditions::valuation(&quotes, Some(6.5)),
        scanned: quotes.len(),
        as_of: None,
        history: None,
    };
    assert_follows_schema(
        "market_conditions",
        &wrapped(serde_json::to_value(&report).unwrap()),
    );

    let rows = json!({
        "verdicts": [{
            "symbol": "TCS.NS", "name": "TCS", "price": 100.0, "currency": "INR",
            "overall_score": null, "data_coverage": 40.0, "low_coverage": true,
            "pillars": [{"name": "Valuation", "score": null, "band": "no_data"}],
            "warnings": [],
        }],
        "failed": [{"symbol": "NOPE.NS", "error": "No quote"}],
        "disclaimer": crate::verdict::DISCLAIMER,
    });
    assert_follows_schema("verdicts", &wrapped(rows));

    // The validator does catch a wrong shape.
    let schema = crate::tools::schema::output_schema("quote").unwrap();
    assert!(crate::tools::schema::validate(&schema, &wrapped(json!({"quotes": "TCS"}))).is_err());
    assert!(crate::tools::schema::validate(&schema, &json!({"data": {}})).is_err());
}

#[tokio::test]
async fn symbol_arguments_complete_from_holdings_watchlist_and_search() {
    let mut c = Client::start(server(false)).await;
    let r = c
        .request(
            "completion/complete",
            json!({
                "ref": {"type": "ref/prompt", "name": "should_i_buy"},
                "argument": {"name": "symbol", "value": "t"},
            }),
        )
        .await;
    let values = r["result"]["completion"]["values"].as_array().unwrap();
    // TCS is held (and watched): offered once, holdings first.
    assert_eq!(values[0], "TCS.NS", "{r}");
    assert_eq!(values.iter().filter(|v| *v == "TCS.NS").count(), 1);

    // Longer text also searches; the fixture's search knows RELIANCE.
    let r = c
        .request(
            "completion/complete",
            json!({
                "ref": {"type": "ref/resource", "uri": "stockwise://quote/{symbol}"},
                "argument": {"name": "symbol", "value": "REL"},
            }),
        )
        .await;
    let values = r["result"]["completion"]["values"].as_array().unwrap();
    assert!(values.iter().any(|v| v == "RELIANCE.NS"), "{r}");

    // Other arguments: nothing to offer.
    let r = c
        .request(
            "completion/complete",
            json!({
                "ref": {"type": "ref/prompt", "name": "review_portfolio"},
                "argument": {"name": "focus", "value": "ri"},
            }),
        )
        .await;
    assert_eq!(r["result"]["completion"]["values"], json!([]), "{r}");

    let init_caps = c.request("tools/list", json!({})).await;
    assert!(init_caps["result"].is_object());
}

#[tokio::test]
async fn new_resources_read_the_picks_log_methodology_and_quote_template() {
    let files = scratch("resources");
    let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
    let entry = |sym: &str, rank: usize, horizon: &str, date| crate::picks_log::Entry {
        date,
        kind: "pick".into(),
        horizon: horizon.into(),
        market: "in".into(),
        universe: "India".into(),
        symbol: sym.into(),
        rank,
        score: 80.0 - rank as f64,
        price: 100.0,
        currency: Some("INR".into()),
        benchmark_price: None,
        methodology: "2".into(),
    };
    let log = vec![
        entry("OLD.NS", 1, "long", day.pred_opt().unwrap()),
        entry("INFY.NS", 2, "long", day),
        entry("TCS.NS", 1, "long", day),
        entry("SMALL.NS", 1, "short", day),
    ];
    std::fs::write(
        files.data_dir.join("signal_log.json"),
        serde_json::to_string(&log).unwrap(),
    )
    .unwrap();
    let mut c = Client::start(server(false).with_files(files)).await;
    let r = c
        .request("resources/read", json!({"uri": "stockwise://picks/latest"}))
        .await;
    let v: Value =
        serde_json::from_str(r["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    let lists = v["lists"].as_array().unwrap();
    assert_eq!(lists.len(), 2, "{v}");
    let long = lists.iter().find(|l| l["horizon"] == "long").unwrap();
    let syms: Vec<&str> = long["picks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(syms, ["TCS.NS", "INFY.NS"], "newest list only, by rank");

    let r = c
        .request("resources/read", json!({"uri": "stockwise://methodology"}))
        .await;
    let content = &r["result"]["contents"][0];
    assert_eq!(content["mimeType"], "text/markdown");
    assert!(content["text"].as_str().unwrap().len() > 200);

    let r = c.request("resources/templates/list", json!({})).await;
    assert_eq!(
        r["result"]["resourceTemplates"][0]["uriTemplate"],
        "stockwise://quote/{symbol}"
    );
    let r = c
        .request("resources/read", json!({"uri": "stockwise://quote/TCS"}))
        .await;
    let q: Value =
        serde_json::from_str(r["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(q["quotes"][0]["symbol"], "TCS.NS", "{r}");
}

#[tokio::test]
async fn subscribed_resources_notify_when_their_files_change() {
    let files = scratch("subscribe");
    let watchlist = files.data_dir.join("watchlist.json");
    let mut server = server(false).with_files(files);
    server.poll_every = std::time::Duration::from_millis(50);
    let mut c = Client::start(server).await;
    let init = c.request("tools/list", json!({})).await;
    assert!(init["result"].is_object());

    let r = c
        .request(
            "resources/subscribe",
            json!({"uri": "stockwise://watchlist"}),
        )
        .await;
    assert!(r["result"].is_object(), "{r}");
    let r = c
        .request(
            "resources/subscribe",
            json!({"uri": "stockwise://methodology"}),
        )
        .await;
    assert!(r["error"].is_object(), "not a local file: {r}");

    // Different length, so the change shows even where mtime is coarse.
    std::fs::write(&watchlist, r#"{"symbols":["TCS.NS"]}"#).unwrap();
    let note = loop {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), c.lines.next_line())
            .await
            .expect("a notification in time")
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["method"] == "notifications/resources/updated" {
            break v;
        }
    };
    assert_eq!(note["params"]["uri"], "stockwise://watchlist");

    // After unsubscribing, changes are quiet.
    let r = c
        .request(
            "resources/unsubscribe",
            json!({"uri": "stockwise://watchlist"}),
        )
        .await;
    assert!(r["result"].is_object(), "{r}");
    std::fs::write(&watchlist, r#"{"symbols":["TCS.NS","INFY.NS"]}"#).unwrap();
    let quiet =
        tokio::time::timeout(std::time::Duration::from_millis(400), c.lines.next_line()).await;
    assert!(
        quiet.is_err(),
        "no notification after unsubscribe: {quiet:?}"
    );
}

#[tokio::test]
async fn subscriptions_are_off_without_local_files() {
    let mut c = Client::start(server(false)).await;
    let r = c
        .request(
            "resources/subscribe",
            json!({"uri": "stockwise://watchlist"}),
        )
        .await;
    assert!(r["error"].is_object(), "{r}");
}

#[test]
fn instructions_state_the_recommendations_policy() {
    assert!(INSTRUCTIONS.contains("deterministic, rule-based engine"));
    assert!(INSTRUCTIONS.contains("Never invent"));
    assert!(INSTRUCTIONS.contains("NO CALL"));
    assert!(INSTRUCTIONS.contains("below 60%"));
    assert!(INSTRUCTIONS.contains("not personalised investment advice"));
    assert!(!INSTRUCTIONS.contains("TODO(0.9 workstream C)"));
    assert!(INSTRUCTIONS.contains("stock_action") && INSTRUCTIONS.contains("portfolio_review"));
    for (name, _, _, args) in PROMPTS {
        for (arg, _, required) in *args {
            assert!(!arg.is_empty() && (*required || !arg.is_empty()), "{name}");
        }
    }
}

#[test]
fn new_prompts_render_with_the_policy() {
    let mut a = Map::new();
    a.insert("symbol".into(), json!("TCS"));
    a.insert("amount".into(), json!("50000"));
    let buy = prompt_text("should_i_buy", &a).unwrap();
    assert!(
        buy.contains("stock_action for TCS") && buy.contains("fit_check for TCS with amount 50000")
    );
    assert!(!buy.contains("TODO"), "{buy}");
    let exit = prompt_text("exit_check", &Map::new()).unwrap();
    assert!(
        exit.contains("portfolio_review") && !exit.contains("TODO"),
        "{exit}"
    );
    assert!(prompt_text("should_i_buy", &Map::new()).is_err());

    let scan = prompt_text("smallcap_short_scan", &Map::new()).unwrap();
    assert!(scan.contains("cap small") && scan.contains("horizon short"));
    assert!(scan.contains("picks_review"));

    for name in [
        "daily_briefing",
        "should_i_buy",
        "exit_check",
        "smallcap_short_scan",
        "tax_aware_rebalance",
        "weekly_review",
    ] {
        let t = prompt_text(name, &a).unwrap();
        assert!(t.contains("not enough data for a call"), "{name}");
        assert!(t.contains("not personalised investment advice"), "{name}");
        assert!(!t.to_lowercase().contains("you should buy"), "{name}");
    }
    assert!(
        prompt_text("exit_check", &a)
            .unwrap()
            .contains("portfolio_review")
    );
    assert!(
        prompt_text("weekly_review", &a)
            .unwrap()
            .contains("picks_review")
    );
    assert!(
        prompt_text("tax_aware_rebalance", &a)
            .unwrap()
            .contains("holdings_lots")
    );
}

#[test]
fn search_completions_put_the_users_market_first() {
    let found = vec![
        "VGT".to_string(),
        "INFY".to_string(),
        "INFY.BO".to_string(),
        "NAUKRI.NS".to_string(),
        "INFY.NS".to_string(),
    ];
    assert_eq!(
        rank_for_market(found.clone(), "INF", Market::In),
        ["INFY.NS", "INFY.BO", "NAUKRI.NS"],
        "NSE before BSE, prefix matches first, foreign listings dropped"
    );
    assert_eq!(
        rank_for_market(found.clone(), "INF", Market::Us)[..2],
        ["INFY".to_string(), "VGT".to_string()]
    );
    // No Indian listing at all: keep what search found.
    assert_eq!(
        rank_for_market(vec!["VGT".into()], "VG", Market::In),
        ["VGT"]
    );
}

#[test]
fn instructions_cover_the_indian_market() {
    for needle in [
        "TCS.NS",
        "lakh",
        "crore",
        "IST",
        "1.25 lakh",
        "April to March",
        "bonds_",
        "commodity",
        "fx_history",
        "mf_bleed",
    ] {
        assert!(INSTRUCTIONS.contains(needle), "{needle}");
    }
    let t = prompt_text("advance_tax_check", &Map::new()).unwrap();
    assert!(t.contains("tax_advance") && t.contains("15 March"));
    let t = prompt_text("mf_review", &Map::new()).unwrap();
    assert!(t.contains("mf_bleed") && t.contains("not enough data for a call"));
}
