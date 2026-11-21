//! `stockwise mcp`: a Model Context Protocol server for AI assistants
//! (Claude Desktop, Claude Code, Cursor, ChatGPT and others), built on the
//! official Rust SDK (`rmcp`).
//!
//! - Tools: the tool layer in `crate::tools`, the same one `stockwise ask`
//!   uses. Every tool carries annotations (read-only, destructive,
//!   idempotent, open-world); the main ones also publish an `outputSchema`
//!   that their `structuredContent` follows, next to the same JSON as text
//!   for older hosts.
//! - Resources: the portfolio, watchlist, alerts and profile as JSON, the
//!   latest logged picks, their track record, the scoring methodology, and a
//!   `stockwise://quote/{symbol}` template. Over stdio a client can
//!   subscribe to the local ones and gets `notifications/resources/updated`
//!   when their files change.
//! - Prompts: a daily briefing, "should I buy X", an exit check across the
//!   holdings, a small-cap short-term scan, a tax-aware rebalance, a weekly
//!   review against the track record, and the earlier portfolio review,
//!   tax-harvest check, option-chain explainer and profile onboarding.
//! - Completion: symbol arguments complete from the holdings, the watchlist
//!   and ticker search.
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
use crate::tools::{ToolContext, client_error};
use crate::views::{self, DataSource};
use anyhow::{Context, Result};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CompleteRequestParams, CompleteResult,
    CompletionInfo, ContentBlock, GetPromptRequestParams, GetPromptResponse, GetPromptResult,
    Implementation, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Reference, Resource,
    ResourceContents, ResourceTemplate, ResourceUpdatedNotificationParam, Role, ServerCapabilities,
    ServerConfig, SubscribeRequestParams, SubscriptionFilter, ToolAnnotations,
    UnsubscribeRequestParams,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Told to the client at `initialize`; hosts show it to the model.
const INSTRUCTIONS: &str = "StockWise gives market data and the user's own portfolio from a \
local command-line tool: quotes, history, technicals, risk, Indian mutual funds, F&O option \
chains, screens, backtests, the portfolio and its analysis and tax, market conditions, ranked \
picks and their track record, calendars and goal calculators. Bare tickers use the user's \
default market (India/NSE unless configured); pass `market` for US (`us`), crypto, fx, \
commodity or another exchange. `quote` and `verdicts` take up to 25 tickers in one call.

Recommendations policy:
1. Actions (BUY, WATCH or AVOID for a stock; ADD, HOLD, TRIM or EXIT for a holding) come only \
from StockWise's deterministic, rule-based engine. Never make one up, never upgrade, soften or \
reverse one, and never turn scores into one yourself. `stock_action` gives the action for one \
stock (held or not), `portfolio_review` for every holding (EXIT, TRIM, ADD, HOLD first); \
`verdict` and `picks` carry the same `action` object. Pass `horizon: short` for weeks-to-months \
rules.
2. Explain an action with the reasons, confidence, invalidation price, review date, size hint \
and tax note the tool returned, quoting them as returned. Never invent price levels: give an \
invalidation, entry, stop or target price only when a tool returned it, exactly as returned.
3. NO CALL: when the action is NO CALL (data coverage below 60%), say there is not enough data \
for a call, name the missing fields, and offer none.
4. Keep the disclaimer: the information is educational, not personalised investment advice; \
StockWise is not a SEBI-registered investment adviser or research analyst; prices can be \
delayed, so verify before acting.

Indian market: bare tickers are NSE (`TCS` is `TCS.NS`; `.BO` for BSE); amounts are in rupees, \
written the Indian way (₹1,25,000; lakh = 1,00,000, crore = 1,00,00,000; `picks` floors are in \
crore). NSE trades 09:15-15:30 IST on weekdays and Yahoo quotes run about 15 minutes late. \
Listed equity held over 12 months is long term: LTCG over ₹1.25 lakh a year is taxed at 12.5%, \
STCG at 20%, and the tax year runs April to March (`tax_estimate`, `tax_advance` for the 15 June, \
September, December and March instalments, `harvest`). For Indian investors there are also \
mutual funds (`mf_search`, `mf_nav`, `mf_sip`, `mf_bleed` for Regular against Direct plans), \
F&O option chains (`options_*`), IPOs, G-sec yields and the bond calculator (`bonds_*`), gold, \
silver and Sovereign Gold Bonds (`commodity`) and USD/INR (`fx_history`).

Scores from `verdict`, `verdicts` and `picks` describe data; the `action` next to them is the \
call. Every `picks` list is logged; when \
presenting picks, cite how earlier lists did (`picks_review`, or the stockwise://track-record \
resource), including when they lagged the index. `fit_check` says whether a stock meets the \
user's own rules; it is not a recommendation.

When the user turned them on, tools also change the local workspace (watchlist, alerts, goals, \
target weights, the investor profile, notes) and, separately, record purchases and sales in the \
portfolio; every change is journalled and `stockwise undo` reverts it, so say what changed and \
give the change id. Ask before changing anything the user did not ask for. These tools never \
place orders.";

/// A resource: (uri, title, description, MIME type).
type ResourceDef = (&'static str, &'static str, &'static str, &'static str);

const JSON: &str = "application/json";

const RESOURCES: &[ResourceDef] = &[
    (
        "stockwise://portfolio",
        "Portfolio",
        "Holdings with lots, cost, live value, P&L and totals (the default portfolio, or the one picked with --portfolio)",
        JSON,
    ),
    (
        "stockwise://watchlist",
        "Watchlist",
        "Watched tickers with live prices",
        JSON,
    ),
    (
        "stockwise://alerts",
        "Alerts",
        "Price and technical alerts with their live status",
        JSON,
    ),
    (
        "stockwise://profile",
        "Investor Profile",
        "Active investor profile mode, risk level, horizon and guardrails",
        JSON,
    ),
    (
        "stockwise://picks/latest",
        "Latest Picks",
        "The most recent logged picks list for each market and horizon (from signal_log.json): rank, score, price when listed",
        JSON,
    ),
    (
        "stockwise://track-record",
        "Picks Track Record",
        "How the long- and short-horizon picks of the last 90 days did since each was first listed, against the market's benchmark",
        JSON,
    ),
    (
        "stockwise://methodology",
        "Scoring Methodology",
        "How picks and verdicts are scored: pillars, weights, floors, coverage and the track record",
        "text/markdown",
    ),
];

/// A quote for any ticker, by URI; `{symbol}` completes like a prompt's.
const QUOTE_TEMPLATE: &str = "stockwise://quote/{symbol}";
const QUOTE_PREFIX: &str = "stockwise://quote/";

/// The scoring methodology, shipped inside the binary.
const METHODOLOGY: &str = include_str!("../docs/methodology.md");

/// Resources whose content lives in local files a client can subscribe to.
const SUBSCRIBABLE: &[&str] = &[
    "stockwise://portfolio",
    "stockwise://watchlist",
    "stockwise://alerts",
    "stockwise://profile",
    "stockwise://picks/latest",
    "stockwise://track-record",
];

/// How often subscribed files are checked for changes.
const POLL_EVERY: Duration = Duration::from_secs(2);

/// The local files behind the subscribable resources.
#[derive(Debug, Clone)]
pub struct Files {
    /// The data directory (watchlist, alerts, the picks log).
    pub data_dir: PathBuf,
    /// The active portfolio's file.
    pub portfolio: PathBuf,
    /// `config.toml` (the investor profile).
    pub config: PathBuf,
}

impl Files {
    /// The user's own data directory, active portfolio and config file.
    pub fn for_user() -> Result<Self> {
        let w = crate::tools::Workspace::for_user("mcp", false)?;
        Ok(Self {
            data_dir: w.dir,
            portfolio: w.portfolio,
            config: w.config,
        })
    }

    /// The files whose change updates `uri`.
    fn behind(&self, uri: &str) -> Vec<PathBuf> {
        match uri {
            "stockwise://portfolio" => vec![self.portfolio.clone()],
            "stockwise://watchlist" => vec![self.data_dir.join("watchlist.json")],
            "stockwise://alerts" => vec![self.data_dir.join("alerts.json")],
            "stockwise://profile" => vec![self.config.clone()],
            "stockwise://picks/latest" | "stockwise://track-record" => {
                vec![self.data_dir.join(PICKS_LOG)]
            }
            _ => Vec::new(),
        }
    }

    /// What the files behind `uri` look like now: modification time and
    /// length of each (`None` while a file is missing).
    fn stamp(&self, uri: &str) -> Vec<Option<(SystemTime, u64)>> {
        self.behind(uri)
            .iter()
            .map(|p| {
                std::fs::metadata(p)
                    .ok()
                    .and_then(|m| Some((m.modified().ok()?, m.len())))
            })
            .collect()
    }
}

const PICKS_LOG: &str = "signal_log.json";

/// One subscribed URI and how its files looked when last checked.
type Stamps = HashMap<String, Vec<Option<(SystemTime, u64)>>>;

/// Legacy `resources/subscribe` state of one stdio session: the subscribed
/// URIs and whether the polling task runs.
#[derive(Default)]
struct Subscriptions {
    uris: Stamps,
    polling: bool,
}

/// The MCP server: tools, resources and prompts over one [`ToolContext`].
pub struct StockwiseMcp<D> {
    ctx: Arc<ToolContext<D>>,
    /// Local files for subscriptions and the picks log; `None` turns
    /// subscriptions off (stateless HTTP cannot push notifications).
    files: Option<Arc<Files>>,
    subs: Arc<Mutex<Subscriptions>>,
    poll_every: Duration,
}

impl<D> Clone for StockwiseMcp<D> {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
            files: self.files.clone(),
            subs: self.subs.clone(),
            poll_every: self.poll_every,
        }
    }
}

impl<D: DataSource + 'static> StockwiseMcp<D> {
    pub fn new(ctx: ToolContext<D>) -> Self {
        Self {
            ctx: Arc::new(ctx),
            files: None,
            subs: Arc::default(),
            poll_every: POLL_EVERY,
        }
    }

    /// Serve resource subscriptions for these files.
    pub fn with_files(mut self, files: Files) -> Self {
        self.files = Some(Arc::new(files));
        self
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
                let h = crate::tools::hints(t);
                let mut tool = rmcp::model::Tool::new(t.name, t.description, Arc::new(schema))
                    .with_title(t.title)
                    .with_annotations(
                        ToolAnnotations::with_title(t.title)
                            .read_only(h.read_only)
                            .destructive(h.destructive)
                            .idempotent(h.idempotent)
                            .open_world(h.open_world),
                    );
                if let Some(Value::Object(out)) = crate::tools::schema::output_schema(t.name) {
                    tool = tool.with_raw_output_schema(Arc::new(out));
                }
                tool
            })
            .collect()
    }

    /// Run a tool. Failures the model can act on (bad arguments, unknown
    /// symbol, Yahoo unreachable) come back as an error result it can read,
    /// not as a protocol error. Results carry the same JSON twice: as
    /// `structuredContent` (following the tool's `outputSchema` when it has
    /// one) and as text, for hosts that ignore structured content.
    pub async fn call(&self, name: &str, args: &Value) -> CallToolResult {
        match self.ctx.call(name, args).await {
            Ok(v) => CallToolResult::structured(v),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(client_error(&e))]),
        }
    }

    async fn read(&self, uri: &str) -> Result<Value> {
        let src = &self.ctx.source;
        let m = self.ctx.market;
        if let Some(sym) = uri.strip_prefix(QUOTE_PREFIX) {
            return self
                .ctx
                .call("quote", &json!({ "symbols": [sym] }))
                .await
                .map(|v| v["data"].clone());
        }
        Ok(match uri {
            "stockwise://portfolio" => serde_json::to_value(views::portfolio(src, m, None).await?)?,
            "stockwise://watchlist" => serde_json::to_value(views::watchlist(src).await?)?,
            "stockwise://alerts" => serde_json::to_value(views::alerts(src).await?)?,
            "stockwise://profile" => {
                serde_json::to_value(crate::tools::current_profile(&self.ctx))?
            }
            "stockwise://picks/latest" => latest_picks(&self.picks_log()),
            "stockwise://track-record" => {
                let mut out = json!({ "market": m.code(), "since_days": 90 });
                for (key, horizon) in [
                    ("long", crate::picks::Horizon::Long),
                    ("short", crate::picks::Horizon::Short),
                ] {
                    out[key] = match self
                        .ctx
                        .call(
                            "picks_review",
                            &json!({ "since": "90d", "horizon": horizon.as_str() }),
                        )
                        .await
                    {
                        Ok(v) => v["data"].clone(),
                        Err(e) => json!({ "error": client_error(&e) }),
                    };
                }
                out
            }
            other => anyhow::bail!("Unknown resource {}", other),
        })
    }

    /// The picks log: from the session's data directory when it has one.
    fn picks_log(&self) -> Vec<crate::picks_log::Entry> {
        match &self.files {
            Some(f) => crate::store::load_from(&f.data_dir.join(PICKS_LOG)).unwrap_or_default(),
            None => crate::picks_log::load().unwrap_or_default(),
        }
    }

    /// Ticker completions for `typed`: holdings first, then the watchlist,
    /// then ticker search when those leave room (and the text is long
    /// enough to search for).
    async fn complete_symbol(&self, typed: &str) -> Vec<String> {
        const MAX: usize = 20;
        let q = typed.trim().to_ascii_uppercase();
        let src = &self.ctx.source;
        let mut local: Vec<String> = src
            .portfolio()
            .map(|p| p.holdings.iter().map(|h| h.symbol.clone()).collect())
            .unwrap_or_default();
        local.extend(src.watchlist().map(|w| w.symbols).unwrap_or_default());
        let mut out: Vec<String> = Vec::new();
        for s in local {
            let up = s.to_ascii_uppercase();
            let bare = up.split('.').next().unwrap_or(&up);
            if (up.starts_with(&q) || bare.starts_with(&q)) && !out.contains(&s) {
                out.push(s);
            }
        }
        if out.len() < MAX && q.chars().count() >= 2 {
            let found = tokio::time::timeout(Duration::from_secs(4), src.search(typed.trim()))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let found: Vec<String> = found.into_iter().filter_map(|r| r.symbol).collect();
            for sym in rank_for_market(found, &q, self.ctx.market) {
                if !out.contains(&sym) {
                    out.push(sym);
                }
            }
        }
        out.truncate(MAX);
        out
    }

    /// Start the polling task of a legacy subscription, once per session.
    fn start_polling(&self, peer: Peer<RoleServer>) {
        let (Some(files), Ok(mut subs)) = (self.files.clone(), self.subs.lock()) else {
            return;
        };
        if subs.polling {
            return;
        }
        subs.polling = true;
        let state = self.subs.clone();
        let every = self.poll_every;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let changed: Vec<String> = match state.lock() {
                    Ok(mut s) => {
                        if s.uris.is_empty() {
                            s.polling = false;
                            return;
                        }
                        changes(&files, &mut s.uris)
                    }
                    Err(_) => return,
                };
                for uri in changed {
                    if peer
                        .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                        .await
                        .is_err()
                    {
                        // The client went away.
                        if let Ok(mut s) = state.lock() {
                            s.polling = false;
                            s.uris.clear();
                        }
                        return;
                    }
                }
            }
        });
    }
}

/// Search results ordered for the user's market: for India, NSE then BSE
/// listings (and nothing else when there are any), for the US, symbols
/// without an exchange suffix; within each group, tickers that start with
/// what was typed come first. Yahoo's search is fuzzy ("INF" finds US
/// funds), so this keeps the user's own market on top.
fn rank_for_market(found: Vec<String>, typed: &str, market: Market) -> Vec<String> {
    let home = |s: &str| match market {
        Market::In => s.ends_with(".NS") || s.ends_with(".BO"),
        Market::Us => !s.contains('.') && !s.contains('=') && !s.starts_with('^'),
        _ => true,
    };
    let any_home = found.iter().any(|s| home(s));
    let mut ranked: Vec<(u8, usize, String)> = found
        .into_iter()
        .enumerate()
        .filter(|(_, s)| !(market == Market::In && any_home && !home(s)))
        .map(|(i, s)| {
            let up = s.to_ascii_uppercase();
            let starts = up.starts_with(typed);
            let group = match (home(&s), starts, up.ends_with(".BO")) {
                (true, true, false) => 0,
                (true, true, true) => 1,
                (true, false, _) => 2,
                (false, true, _) => 3,
                (false, false, _) => 4,
            };
            (group, i, s)
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, _, s)| s).collect()
}

/// The URIs in `stamps` whose files changed since the last look, with the
/// stamps brought up to date.
fn changes(files: &Files, stamps: &mut Stamps) -> Vec<String> {
    let mut out = Vec::new();
    for (uri, old) in stamps.iter_mut() {
        let now = files.stamp(uri);
        if now != *old {
            *old = now;
            out.push(uri.clone());
        }
    }
    out.sort();
    out
}

/// The newest logged list of each market and horizon, newest first.
fn latest_picks(log: &[crate::picks_log::Entry]) -> Value {
    let mut keys: Vec<(&str, &str)> = Vec::new();
    for e in log.iter().rev().filter(|e| e.kind == "pick") {
        if !keys.contains(&(e.market.as_str(), e.horizon.as_str())) {
            keys.push((e.market.as_str(), e.horizon.as_str()));
        }
    }
    let lists: Vec<Value> = keys
        .iter()
        .filter_map(|(market, horizon)| {
            let newest = log
                .iter()
                .rev()
                .find(|e| e.kind == "pick" && e.market == *market && e.horizon == *horizon)?;
            let mut run: Vec<&crate::picks_log::Entry> = log
                .iter()
                .filter(|e| {
                    e.kind == "pick"
                        && e.market == newest.market
                        && e.horizon == newest.horizon
                        && e.universe == newest.universe
                        && e.date == newest.date
                })
                .collect();
            run.sort_by_key(|e| e.rank);
            Some(json!({
                "date": newest.date,
                "market": newest.market,
                "horizon": newest.horizon,
                "universe": newest.universe,
                "methodology": newest.methodology,
                "picks": run.iter().map(|e| json!({
                    "rank": e.rank,
                    "symbol": e.symbol,
                    "score": e.score,
                    "price_when_listed": e.price,
                    "currency": e.currency,
                })).collect::<Vec<_>>(),
            }))
        })
        .collect();
    let note = if lists.is_empty() {
        "No picks logged yet: run the picks tool or `stockwise picks`; every list is logged."
    } else {
        "Logged lists as they were scored on their date; prices are when listed. picks_review shows how they did since."
    };
    json!({ "lists": lists, "note": note })
}

/// A prompt argument: (name, description, required).
type PromptArg = (&'static str, &'static str, bool);

/// The prompts: (name, title, description, arguments).
const PROMPTS: &[(&str, &str, &str, &[PromptArg])] = &[
    (
        "daily_briefing",
        "Daily briefing",
        "Market conditions, the portfolio's day, triggered alerts and watchlist movers in one short briefing",
        &[],
    ),
    (
        "should_i_buy",
        "Should I buy this stock?",
        "The stock's scores, data coverage, risk and fit with your own rules, explained without inventing a call",
        &[
            ("symbol", "Ticker, e.g. TCS, RELIANCE, AAPL", true),
            (
                "amount",
                "Amount you are considering, in the portfolio's currency",
                false,
            ),
        ],
    ),
    (
        "exit_check",
        "Exit check across my holdings",
        "Each holding's scores and coverage, concentration and checkup findings, to see which deserve a closer look",
        &[],
    ),
    (
        "smallcap_short_scan",
        "Small-cap short-term scan",
        "Small caps ranked for weeks-to-months (picks with cap small, horizon short), with their track record and risks",
        &[("market", "in (default) or us", false)],
    ),
    (
        "tax_aware_rebalance",
        "Tax-aware rebalance",
        "Drift from your target weights, with the tax each trim would cost and ways to rebalance with less tax",
        &[],
    ),
    (
        "weekly_review",
        "Weekly review against the track record",
        "How the portfolio and the past picks lists did, against the benchmark, with the changes made this week",
        &[],
    ),
    (
        "mf_review",
        "Review my mutual funds",
        "Regular-plan cost against Direct, overlap with your stocks, and recent NAVs of your funds (India)",
        &[],
    ),
    (
        "advance_tax_check",
        "Advance tax check",
        "Capital gains so far this financial year and the advance-tax instalments due on 15 June, September, December and March (India)",
        &[],
    ),
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
    (
        "onboard_profile",
        "Set up my investor profile",
        "A few plain questions about how you invest, then your profile is saved (mode, risk, horizon, tax)",
        &[],
    ),
];

/// Said at the end of every prompt that discusses stocks.
const POLICY: &str = " Follow StockWise's recommendations policy: actions come only from its \
rule-based engine and you never invent one or any price level; when data coverage is under 60% \
say there is not enough data for a call; end with the disclaimer that this is educational \
information, not personalised investment advice, and prices can be delayed.";

/// The text of a prompt, with its arguments filled in.
pub fn prompt_text(name: &str, args: &Map<String, Value>) -> Result<String> {
    let arg = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    Ok(match name {
        "daily_briefing" => format!(
            "Give me my StockWise daily briefing, short enough to read in two minutes. Call \
             market_conditions and markets for where the market stands, today for my \
             portfolio's day (each holding's move and why, with its action), alerts_list for \
             any alert that triggered, and whats_hot for what is moving in my holdings and the \
             market. Then: the market in two lines (trend, volatility, breadth, valuation as \
             readings), my portfolio's day with the actions that changed, triggered alerts, \
             and anything unusual. Leave out what has no news.{POLICY}"
        ),
        "should_i_buy" => {
            let symbol =
                arg("symbol").ok_or_else(|| anyhow::anyhow!("should_i_buy needs a symbol"))?;
            let amount = arg("amount")
                .map(|a| format!(" with amount {a}"))
                .unwrap_or_default();
            format!(
                "I'm thinking about buying {symbol}. Call stock_action for {symbol}, verdict and \
                 risk for it, and fit_check for {symbol}{amount}. Lead with the action exactly as \
                 stock_action returned it (BUY, WATCH, AVOID or NO CALL), its confidence, its \
                 reasons, the invalidation price and review date, and the size hint. Then walk \
                 me through the pillar scores behind it, the volatility and drawdown, and which \
                 of my own rules it passes or fails. Do not change the action or add levels of \
                 your own.{POLICY}"
            )
        }
        "exit_check" => format!(
            "Run an exit check across my StockWise holdings. Call portfolio_review, then \
             portfolio_checkup. List the holdings in the order portfolio_review gives (EXIT, \
             TRIM, ADD, HOLD, NO CALL), each with its action as returned, confidence, reasons, \
             invalidation price, review date, the shares a TRIM would sell and any tax note \
             (waiting a few days for long-term rates). Add the checkup findings that touch \
             each holding. Do not change any action.{POLICY}"
        ),
        "smallcap_short_scan" => {
            let market = arg("market").unwrap_or("in");
            format!(
                "Scan small caps for the short term. Call picks with market {market}, cap small \
                 and horizon short, then picks_review with market {market} and horizon short. \
                 Show the list with each stock's score, its pillars and data coverage, and say \
                 how earlier short-horizon lists did against the index (sample size and \
                 period too). Explain the risks that come with small caps held for weeks: \
                 volatility, thin trading and gaps. Leave out any stock whose coverage is under \
                 0.6, saying so.{POLICY}"
            )
        }
        "tax_aware_rebalance" => format!(
            "Help me rebalance my StockWise portfolio with tax in mind. Call rebalance for the \
             drift from my target weights, tax_estimate for realized gains and the exemption \
             left, holdings_lots for each lot's holding period, and harvest for losses that \
             could offset gains. For each holding over its target show the trim, the tax it \
             would cost (short- or long-term, by lot), and cheaper ways to get there: using \
             new money instead of selling, waiting for lots about to turn long-term, or \
             pairing with harvestable losses. Present it as options and their costs, and \
             remind me to check the figures against my broker's tax P&L. If I need cash, call \
             rebalance with `raise` for the sells that raise it at the lowest tax.{POLICY}"
        ),
        "weekly_review" => format!(
            "Review my week with StockWise. Call portfolio_summary and portfolio_pme for how the \
             portfolio did against its benchmark, picks_review for the long and the short \
             horizon over the last 90 days, and changes_list for what assistants changed this \
             week. Report the portfolio's return against the benchmark, how past picks lists \
             did (hit rate, share that beat the index, mean and median excess, and how many \
             picks that is: say plainly when the sample is too small to mean much), and the \
             changes made. Be as clear about misses as about hits.{POLICY}"
        ),
        "mf_review" => format!(
            "Review my Indian mutual funds with StockWise. Call portfolio_summary for the funds \
             I hold (MF: holdings), mf_bleed for what Regular plans cost me against the Direct \
             plan of the same scheme over time, portfolio_analyze for overlap between my funds \
             and my stocks and for concentration, and mf_nav for the funds' recent NAVs. Show \
             each fund's value and gain, the yearly and long-run cost of Regular plans (with the \
             exit load and tax a switch could trigger, as caveats), and where my funds hold the \
             same stocks I already own.{POLICY}"
        ),
        "advance_tax_check" => "Check my Indian advance tax with StockWise. Call tax_advance \
             for the instalments (15 June 15%, 15 September 45%, 15 December 75%, 15 March \
             100% of the year's tax), tax_estimate for realized short- and long-term gains this \
             financial year and the ₹1.25 lakh LTCG exemption left, and harvest for losses that \
             could reduce the gains. Show what is due by the next date, what was already \
             covered, and interest under sections 234B and 234C if an instalment is missed, as \
             estimates. Remind me that this covers only the capital gains StockWise has \
             recorded, not salary or other income, and to check with my broker's tax P&L or a \
             tax adviser."
            .to_string(),
        "onboard_profile" => "Help me set up my StockWise investor profile. Call \
             profile_questions and ask me its questions one at a time, in plain words, with the \
             choices; accept my own words and map them to a choice, asking again if unsure. \
             Skip the optional ones if I prefer. Then call profile_onboard with my answers and \
             tell me the mode, risk level and guardrails it set and why, and that `stockwise \
             undo` reverts it. If profile_onboard is not available (workspace changes are off), \
             give me the matching `stockwise profile set` command instead. This sets how \
             StockWise describes things for me; it is not investment advice."
            .to_string(),
        "review_portfolio" => {
            let mut s = String::from(
                "Review my StockWise portfolio. Call portfolio_summary first, then holdings_lots, \
                 and risk for the two or three largest holdings. Cover: total value and P&L, \
                 XIRR if available, the weight of each holding and sector concentration, \
                 holdings that are not priced, and the volatility and drawdown of the biggest \
                 positions. Keep it factual: describe what the numbers show and the risks to be \
                 aware of.",
            );
            if let Some(f) = arg("focus") {
                s.push_str(&format!(" Pay particular attention to: {f}."));
            }
            s.push_str(POLICY);
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

/// Whether argument `arg` of `reference` is a ticker.
fn is_symbol_argument(reference: &Reference, arg: &str) -> bool {
    match reference {
        Reference::Prompt(p) => {
            arg == "symbol"
                && PROMPTS
                    .iter()
                    .any(|(name, _, _, args)| *name == p.name && args.iter().any(|a| a.0 == arg))
        }
        Reference::Resource(r) => r.uri == QUOTE_TEMPLATE && arg == "symbol",
        _ => false,
    }
}

impl<D: DataSource + 'static> ServerHandler for StockwiseMcp<D> {
    fn get_info(&self) -> ServerConfig {
        let caps = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .enable_completions();
        let caps = if self.files.is_some() {
            caps.enable_resources_subscribe().build()
        } else {
            caps.build()
        };
        ServerConfig::new(caps)
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
                .map(|(uri, name, desc, mime)| {
                    Resource::new(*uri, name.to_ascii_lowercase())
                        .with_title(*name)
                        .with_description(*desc)
                        .with_mime_type(*mime)
                })
                .collect(),
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new(QUOTE_TEMPLATE, "quote")
                .with_title("Quote")
                .with_description("A live (often delayed) quote for one ticker")
                .with_mime_type(JSON),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri;
        if uri == "stockwise://methodology" {
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(METHODOLOGY, uri).with_mime_type("text/markdown"),
            ])
            .into());
        }
        let quote = uri
            .strip_prefix(QUOTE_PREFIX)
            .is_some_and(|s| !s.trim().is_empty());
        if !quote && !RESOURCES.iter().any(|r| r.0 == uri) {
            return Err(McpError::resource_not_found(
                format!("Unknown resource {}", uri),
                None,
            ));
        }
        let value = self
            .read(&uri)
            .await
            .map_err(|e| McpError::internal_error(client_error(&e), None))?;
        let text = serde_json::to_string(&value)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(
            ReadResourceResult::new(vec![ResourceContents::text(text, uri).with_mime_type(JSON)])
                .into(),
        )
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        let Some(files) = &self.files else {
            return Err(McpError::invalid_request(
                "Resource subscriptions need the stdio transport",
                None,
            ));
        };
        if !SUBSCRIBABLE.contains(&request.uri.as_str()) {
            return Err(McpError::resource_not_found(
                format!(
                    "{} cannot be subscribed to; these can: {}",
                    request.uri,
                    SUBSCRIBABLE.join(", ")
                ),
                None,
            ));
        }
        let stamp = files.stamp(&request.uri);
        if let Ok(mut s) = self.subs.lock() {
            s.uris.insert(request.uri, stamp);
        }
        self.start_polling(context.peer);
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if let Ok(mut s) = self.subs.lock() {
            s.uris.remove(&request.uri);
        }
        Ok(())
    }

    /// Protocol 2026-07-28 and later: `subscriptions/listen` with the
    /// subscribable resources the client asked for.
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.files.as_ref()?;
        let uris: Vec<String> = requested
            .resource_subscriptions
            .iter()
            .flatten()
            .filter(|u| SUBSCRIBABLE.contains(&u.as_str()))
            .cloned()
            .collect();
        let mut filter = SubscriptionFilter::new();
        filter.resource_subscriptions = Some(uris);
        Some(filter)
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let Some(files) = self.files.clone() else {
            return Ok(());
        };
        let mut stamps: Stamps = context
            .accepted()
            .resource_subscriptions
            .iter()
            .flatten()
            .map(|u| (u.clone(), files.stamp(u)))
            .collect();
        if stamps.is_empty() {
            context.cancelled().await;
            return Ok(());
        }
        loop {
            tokio::select! {
                _ = context.cancelled() => return Ok(()),
                _ = tokio::time::sleep(self.poll_every) => {
                    for uri in changes(&files, &mut stamps) {
                        if context.sink().notify_resource_updated(uri).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        let arg = &request.argument;
        let values = if is_symbol_argument(&request.r#ref, &arg.name) {
            self.complete_symbol(&arg.value).await
        } else if arg.name == "market" {
            ["in", "us"]
                .iter()
                .filter(|m| m.starts_with(arg.value.trim()))
                .map(|m| m.to_string())
                .collect()
        } else {
            Vec::new()
        };
        let info = CompletionInfo::new(values).map_err(|e| McpError::internal_error(e, None))?;
        Ok(CompleteResult::new(info))
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
    access: crate::commands::McpAccess,
    http: Option<&str>,
    allow_remote: bool,
) -> Result<()> {
    let mut ctx = ToolContext::new(views::LiveSource::new(), market);
    ctx.allow_orders = access.orders;
    // STOCKWISE_MCP_TIMEOUT=SECONDS caps every read tool (default: 60 s, 180 s
    // for market scans and portfolio analyses).
    ctx.timeout = std::env::var("STOCKWISE_MCP_TIMEOUT")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(std::time::Duration::from_secs);
    if access.writes || access.portfolio {
        ctx.workspace = Some(crate::tools::Workspace::for_user("mcp", access.portfolio)?);
    }
    let allow_orders = access.orders;
    let mode = match (access.writes || access.portfolio, access.portfolio) {
        (false, _) => "read-only",
        (true, false) => "workspace changes on, undo with `stockwise undo`",
        (true, true) => "workspace and portfolio changes on, undo with `stockwise undo`",
    };
    let server = StockwiseMcp::new(ctx);
    match http {
        None => {
            // Subscriptions push notifications, which only stdio carries here
            // (the HTTP transport answers each request statelessly).
            let server = match Files::for_user() {
                Ok(files) => server.with_files(files),
                Err(e) => {
                    eprintln!("stockwise: resource subscriptions are off: {e:#}");
                    server
                }
            };
            // stdout belongs to the protocol: notes go to stderr.
            eprintln!(
                "StockWise MCP server {} on stdio ({} tools, {}{}). Ctrl+C to stop.",
                env!("CARGO_PKG_VERSION"),
                server.ctx.tools().len(),
                mode,
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
        Some(bind) => serve_http(server, bind, allow_remote, mode).await,
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
    mode: &str,
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
        "StockWise MCP server {} — {}, Streamable HTTP",
        env!("CARGO_PKG_VERSION"),
        mode
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
