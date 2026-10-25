//! `stockwise mcp`: a Model Context Protocol server for AI assistants
//! (Claude Desktop, Claude Code, Cursor, ChatGPT and others), built on the
//! official Rust SDK (`rmcp`).
//!
//! - Tools: the read-only tool layer in `crate::tools`, the same one
//!   `stockwise ask` uses.
//! - Resources: the portfolio, watchlist and alerts as JSON.
//! - Prompts: portfolio review, a tax-harvest check before 31 March, and an
//!   option-chain explainer.
//!
//! Nothing here places orders. `--allow-orders` adds an `order_preview` tool
//! that estimates an order and returns the `stockwise trade` command for the
//! user to run themselves: live placement stays human-only, behind the
//! preview and typed confirmation of `place_live_order`.
//!
//! Transports: stdio (the default; stdout carries only JSON-RPC) and
//! Streamable HTTP (`--http`), loopback by default and behind
//! `STOCKWISE_API_TOKEN` otherwise, like `stockwise serve`.

use crate::market::Market;
use crate::tools::ToolContext;
use crate::views::{self, DataSource};
use anyhow::{Context, Result};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, Implementation, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerConfig, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value};
use std::sync::Arc;

/// Told to the client at `initialize`; hosts show it to the model.
const INSTRUCTIONS: &str = "StockWise gives read-only market data and the user's own \
portfolio from a local command-line tool: quotes, history, technicals, risk, Indian mutual \
funds, F&O option chains, screens, backtests, the portfolio and its tax, calendars and goal \
calculators. Bare tickers use the user's default market (India/NSE unless configured); pass \
`market` for US (`us`), crypto, fx, commodity or another exchange. Present results as \
educational information, not personalised advice or buy/sell recommendations, and say that \
prices can be delayed. These tools never place orders.";

const RESOURCES: &[(&str, &str, &str)] = &[
    (
        "stockwise://portfolio",
        "Portfolio",
        "Holdings with lots, cost, live value, P&L and totals (the default portfolio, or the one picked with --portfolio)",
    ),
    (
        "stockwise://watchlist",
        "Watchlist",
        "Watched tickers with live prices",
    ),
    (
        "stockwise://alerts",
        "Alerts",
        "Price and technical alerts with their live status",
    ),
];

/// The MCP server: tools, resources and prompts over one [`ToolContext`].
pub struct StockwiseMcp<D> {
    ctx: Arc<ToolContext<D>>,
}

impl<D> Clone for StockwiseMcp<D> {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
        }
    }
}

impl<D: DataSource + 'static> StockwiseMcp<D> {
    pub fn new(ctx: ToolContext<D>) -> Self {
        Self { ctx: Arc::new(ctx) }
    }

    fn tool_list(&self) -> Vec<rmcp::model::Tool> {
        self.ctx
            .tools()
            .into_iter()
            .map(|t| {
                let schema = match t.input_schema() {
                    Value::Object(m) => m,
                    _ => Map::new(),
                };
                rmcp::model::Tool::new(t.name, t.description, Arc::new(schema))
                    .with_title(t.title)
                    .with_annotations(
                        ToolAnnotations::with_title(t.title)
                            .read_only(true)
                            .destructive(false)
                            .idempotent(true)
                            // Market data comes from outside; the portfolio does not.
                            .open_world(!t.personal),
                    )
            })
            .collect()
    }

    /// Run a tool. Failures the model can act on (bad arguments, unknown
    /// symbol, Yahoo unreachable) come back as an error result it can read,
    /// not as a protocol error.
    pub async fn call(&self, name: &str, args: &Value) -> CallToolResult {
        match self.ctx.call(name, args).await {
            Ok(v) => CallToolResult::structured(v),
            Err(e) => {
                let msg = crate::server::redact(&format!("{:#}", e));
                CallToolResult::error(vec![ContentBlock::text(msg)])
            }
        }
    }

    async fn read(&self, uri: &str) -> Result<Value> {
        let src = &self.ctx.source;
        Ok(match uri {
            "stockwise://portfolio" => {
                serde_json::to_value(views::portfolio(src, self.ctx.market, None).await?)?
            }
            "stockwise://watchlist" => serde_json::to_value(views::watchlist(src).await?)?,
            "stockwise://alerts" => serde_json::to_value(views::alerts(src).await?)?,
            other => anyhow::bail!("Unknown resource {}", other),
        })
    }
}

/// The prompts: (name, title, description, arguments as (name, description, required)).
const PROMPTS: &[(&str, &str, &str, &[(&str, &str, bool)])] = &[
    (
        "review_portfolio",
        "Review my portfolio",
        "Walk through the portfolio's value, P&L, concentration and risk",
        &[(
            "focus",
            "Anything to look at closely (a holding, a sector, risk)",
            false,
        )],
    ),
    (
        "tax_harvest_check",
        "Tax-harvest check before 31 March",
        "Unrealized gains and losses by lot against India's LTCG exemption and set-off rules",
        &[],
    ),
    (
        "explain_option_chain",
        "Explain this option chain",
        "Open interest, PCR, max pain, IV and greeks for an underlying, in plain English",
        &[
            (
                "symbol",
                "Underlying, e.g. NIFTY, BANKNIFTY, RELIANCE",
                true,
            ),
            ("expiry", "Expiry date YYYY-MM-DD (default nearest)", false),
        ],
    ),
];

/// The text of a prompt, with its arguments filled in.
pub fn prompt_text(name: &str, args: &Map<String, Value>) -> Result<String> {
    let arg = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    Ok(match name {
        "review_portfolio" => {
            let mut s = String::from(
                "Review my StockWise portfolio. Call portfolio_summary first, then holdings_lots, \
                 and risk for the two or three largest holdings. Cover: total value and P&L, \
                 XIRR if available, the weight of each holding and sector concentration, \
                 holdings that are not priced, and the volatility and drawdown of the biggest \
                 positions. Keep it factual and educational: describe what the numbers show and \
                 the risks to be aware of, without telling me what to buy or sell.",
            );
            if let Some(f) = arg("focus") {
                s.push_str(&format!(" Pay particular attention to: {f}."));
            }
            s
        }
        "tax_harvest_check" => "It's tax-planning time before 31 March. Using StockWise, call \
             tax_estimate and holdings_lots. Show: realized short- and long-term gains so far \
             this tax year; unrealized gains and losses lot by lot, marking which lots are long \
             term (listed equity held over 12 months); how much of India's ₹1.25 lakh LTCG \
             exemption is left; and which unrealized losses could be set off against gains \
             under the set-off rules (STCL against STCG or LTCG, LTCL only against LTCG). \
             Explain the options and their caveats (STT and costs, the reset cost basis if \
             shares are bought back) as education, not as instructions, and remind me to check \
             the figures against my broker's tax P&L."
            .to_string(),
        "explain_option_chain" => {
            let symbol = arg("symbol")
                .ok_or_else(|| anyhow::anyhow!("explain_option_chain needs a symbol"))?;
            let expiry = arg("expiry")
                .map(|e| format!(" for the {e} expiry"))
                .unwrap_or_default();
            format!(
                "Explain the option chain of {symbol}{expiry} in plain English for someone \
                 learning options. Call options_chain, options_pcr and options_maxpain. \
                 Describe where open interest is concentrated on the call and put side and what \
                 traders often read into it, the put/call ratio, the max-pain strike and its \
                 distance from the spot, how implied volatility differs across strikes, and \
                 what delta and theta mean for an at-the-money contract. Keep it educational \
                 and do not suggest trades."
            )
        }
        other => anyhow::bail!("Unknown prompt {}", other),
    })
}

impl<D: DataSource + 'static> ServerHandler for StockwiseMcp<D> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(
            Implementation::new("stockwise", env!("CARGO_PKG_VERSION")).with_title("StockWise"),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tool_list()))
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.tool_list().into_iter().find(|t| t.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if self.ctx.find(&request.name).is_none() {
            return Err(McpError::invalid_params(
                format!("Unknown tool '{}'", request.name),
                None,
            ));
        }
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        Ok(self.call(&request.name, &args).await.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(
            RESOURCES
                .iter()
                .map(|(uri, name, desc)| {
                    Resource::new(*uri, name.to_ascii_lowercase())
                        .with_title(*name)
                        .with_description(*desc)
                        .with_mime_type("application/json")
                })
                .collect(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if !RESOURCES.iter().any(|(uri, _, _)| *uri == request.uri) {
            return Err(McpError::resource_not_found(
                format!("Unknown resource {}", request.uri),
                None,
            ));
        }
        let value = self.read(&request.uri).await.map_err(|e| {
            McpError::internal_error(crate::server::redact(&format!("{:#}", e)), None)
        })?;
        let text = serde_json::to_string(&value)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, request.uri).with_mime_type("application/json"),
        ])
        .into())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(
            PROMPTS
                .iter()
                .map(|(name, title, desc, args)| {
                    let args: Vec<PromptArgument> = args
                        .iter()
                        .map(|(n, d, r)| {
                            PromptArgument::new(*n)
                                .with_description(*d)
                                .with_required(*r)
                        })
                        .collect();
                    Prompt::new(*name, Some(*desc), (!args.is_empty()).then_some(args))
                        .with_title(*title)
                })
                .collect(),
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let Some((_, _, desc, _)) = PROMPTS.iter().find(|p| p.0 == request.name) else {
            return Err(McpError::invalid_params(
                format!("Unknown prompt {}", request.name),
                None,
            ));
        };
        let args = request.arguments.unwrap_or_default();
        let text = prompt_text(&request.name, &args)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(
            GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
                .with_description(*desc)
                .into(),
        )
    }
}

/// `stockwise mcp`: serve over stdio until the client closes it, or over
/// Streamable HTTP when `http` names an address.
pub async fn run(
    market: Market,
    allow_orders: bool,
    http: Option<&str>,
    allow_remote: bool,
) -> Result<()> {
    let mut ctx = ToolContext::new(views::LiveSource::new(), market);
    ctx.allow_orders = allow_orders;
    let server = StockwiseMcp::new(ctx);
    match http {
        None => {
            // stdout belongs to the protocol: notes go to stderr.
            eprintln!(
                "StockWise MCP server {} on stdio ({} tools, read-only{}). Ctrl+C to stop.",
                env!("CARGO_PKG_VERSION"),
                server.ctx.tools().len(),
                if allow_orders {
                    "; order previews only"
                } else {
                    ""
                }
            );
            let running = server
                .serve(rmcp::transport::stdio())
                .await
                .context("MCP handshake failed")?;
            running.waiting().await.context("MCP server stopped")?;
            Ok(())
        }
        Some(bind) => serve_http(server, bind, allow_remote).await,
    }
}

/// Streamable HTTP settings. Stateless JSON responses: every tool is a
/// plain request and answer. Without a token only loopback `Host` names
/// (DNS rebinding) and loopback browser origins are accepted.
fn http_config(token: bool) -> rmcp::transport::StreamableHttpServerConfig {
    let config = rmcp::transport::StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true);
    if token {
        // A bearer token guards every request, so any host name may reach it.
        config.disable_allowed_hosts()
    } else {
        config.with_allowed_origins(["http://localhost:*", "http://127.0.0.1:*", "http://[::1]:*"])
    }
}

/// The HTTP app: the MCP endpoint at `/mcp`, behind the bearer token when
/// there is one.
fn http_app<D: DataSource + 'static>(
    server: StockwiseMcp<D>,
    token: Option<String>,
    config: rmcp::transport::StreamableHttpServerConfig,
) -> axum::Router {
    use rmcp::transport::StreamableHttpService;
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    crate::server::with_bearer_token(axum::Router::new().route_service("/mcp", service), token)
}

/// Streamable HTTP at `/mcp`, with the bind checked like `serve`: loopback
/// unless `--allow-remote`, and then only with `STOCKWISE_API_TOKEN`.
async fn serve_http<D: DataSource + 'static>(
    server: StockwiseMcp<D>,
    bind: &str,
    allow_remote: bool,
) -> Result<()> {
    let token = std::env::var("STOCKWISE_API_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(bind)
        .await
        .with_context(|| {
            format!(
                "Cannot resolve --http {} (use HOST:PORT, e.g. 127.0.0.1:8788)",
                bind
            )
        })?
        .collect();
    crate::server::check_bind(&addrs, allow_remote, token.as_deref())?;
    let listener = tokio::net::TcpListener::bind(&addrs[..])
        .await
        .with_context(|| format!("Cannot listen on {}", bind))?;
    let local = listener.local_addr()?;

    let config = http_config(token.is_some());
    let cancel = config.cancellation_token.clone();
    let auth = if token.is_some() {
        "bearer token required (STOCKWISE_API_TOKEN)"
    } else {
        "none — loopback only"
    };
    let app = http_app(server, token, config);

    eprintln!(
        "StockWise MCP server {} — read-only, Streamable HTTP",
        env!("CARGO_PKG_VERSION")
    );
    eprintln!("  Endpoint: http://{}/mcp", local);
    eprintln!("  Auth:     {}", auth);
    eprintln!("  Press Ctrl+C to stop.");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancel.cancel();
        })
        .await
        .context("MCP server error")
}

#[cfg(test)]
mod tests;
