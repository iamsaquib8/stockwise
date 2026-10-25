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
    assert_eq!(personal["annotations"]["openWorldHint"], false);
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
            "stockwise://alerts"
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
            "review_portfolio",
            "tax_harvest_check",
            "explain_option_chain"
        ]
    );
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
