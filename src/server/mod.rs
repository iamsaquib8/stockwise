//! `stockwise serve`: a read-only JSON API over the same views the CLI
//! prints, a web dashboard on top of it (`web.rs`), and Prometheus metrics.
//! There are no trading or broker endpoints, and broker credentials are
//! never read.
//!
//! Listens on loopback by default. A non-loopback bind needs
//! `--allow-remote` and a bearer token in `STOCKWISE_API_TOKEN`.

mod metrics;
mod web;

use crate::market::{self, Market};
use crate::views::{self, DataSource};
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::{MatchedPath, Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use clap::ValueEnum;
use colored::Colorize;
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

pub use metrics::Metrics;

/// Most symbols one `/api/quote` request may ask for.
const MAX_SYMBOLS: usize = 50;
/// Shortest accepted `STOCKWISE_API_TOKEN`.
const MIN_TOKEN_LEN: usize = 16;
/// How long Ctrl+C waits for requests in flight.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

pub struct AppState<D> {
    pub source: D,
    /// Market for bare tickers when a request has no `market=` parameter.
    pub market: Market,
    /// Bearer token required on every route except `/healthz`.
    pub token: Option<String>,
    pub metrics: Metrics,
}

type Shared<D> = State<Arc<AppState<D>>>;

/// JSON error body: `{"error": "..."}`.
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    /// Market data could not be fetched.
    Data(anyhow::Error),
    /// A local data file could not be read. Details (with file paths) go to
    /// the server log only.
    Local(anyhow::Error),
}

impl ApiError {
    fn from_data(e: anyhow::Error) -> Self {
        if let Some(nf) = e.downcast_ref::<views::NotFound>() {
            Self::NotFound(nf.to_string())
        } else if e.downcast_ref::<views::LocalData>().is_some() {
            Self::Local(e)
        } else {
            Self::Data(e)
        }
    }
}

/// Drop the Yahoo session crumb from error text before it leaves the process.
pub(crate) fn redact(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(i) = rest.find("crumb=") {
        out.push_str(&rest[..i + "crumb=".len()]);
        out.push_str("REDACTED");
        rest = &rest[i + "crumb=".len()..];
        let end = rest.find(['&', ')', ' ']).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Data(e) => {
                let msg = redact(&format!("{:#}", e));
                eprintln!("  {} {}", "error:".red(), msg);
                (StatusCode::BAD_GATEWAY, msg)
            }
            Self::Local(e) => {
                eprintln!("  {} {:#}", "error:".red(), e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not read local data (see the server log)".into(),
                )
            }
        };
        (status, axum::Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

type ApiResult = std::result::Result<Response, ApiError>;

fn json<T: serde::Serialize>(v: T) -> ApiResult {
    Ok(axum::Json(v).into_response())
}

#[derive(Deserialize)]
struct MarketsParams {
    market: Option<String>,
    /// Add the crypto section (`markets --crypto`).
    #[serde(default)]
    crypto: bool,
    /// Add the currency section (`markets --fx`).
    #[serde(default)]
    fx: bool,
    /// Add each exchange's main index (`markets --global`).
    #[serde(default)]
    global: bool,
}

impl<D> AppState<D> {
    fn market_for(&self, param: Option<&str>) -> std::result::Result<Market, ApiError> {
        match param {
            None | Some("") => Ok(self.market),
            Some(m) => Market::from_str(m, true).map_err(|_| {
                ApiError::BadRequest(format!("Unknown market '{}' (use in, us, crypto or fx)", m))
            }),
        }
    }
}

/// A ticker as typed by a user: short, and only characters Yahoo symbols use.
fn check_symbol(s: &str) -> std::result::Result<(), ApiError> {
    let ok = !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".^=-&_".contains(c));
    if ok {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!("Invalid symbol '{}'", s)))
    }
}

#[derive(Deserialize)]
struct QuoteParams {
    symbols: Option<String>,
    market: Option<String>,
}

/// `GET /api/quote?symbols=RELIANCE,TCS` — the `quote --json` array.
/// Symbols Yahoo doesn't know are listed in the `X-Stockwise-Missing` header;
/// 404 if none are known.
async fn quote<D: DataSource>(State(st): Shared<D>, Query(p): Query<QuoteParams>) -> ApiResult {
    let market = st.market_for(p.market.as_deref())?;
    let raw: Vec<&str> = p
        .symbols
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if raw.is_empty() {
        return Err(ApiError::BadRequest(
            "symbols is required, e.g. /api/quote?symbols=RELIANCE,TCS".into(),
        ));
    }
    if raw.len() > MAX_SYMBOLS {
        return Err(ApiError::BadRequest(format!(
            "At most {} symbols per request",
            MAX_SYMBOLS
        )));
    }
    for s in &raw {
        check_symbol(s)?;
    }
    let resolved: Vec<String> = raw
        .iter()
        .map(|s| market::resolve_symbol(s, market))
        .collect();
    let set = views::quotes(&st.source, &resolved)
        .await
        .map_err(ApiError::from_data)?;
    if set.quotes.is_empty() {
        return Err(ApiError::NotFound(format!(
            "No quote for {}",
            set.missing.join(", ")
        )));
    }
    let mut resp = axum::Json(&set.quotes).into_response();
    if !set.missing.is_empty()
        && let Ok(v) = HeaderValue::from_str(&set.missing.join(","))
    {
        resp.headers_mut().insert("x-stockwise-missing", v);
    }
    Ok(resp)
}

#[derive(Deserialize)]
struct HistoryParams {
    period: Option<String>,
    market: Option<String>,
}

/// `GET /api/history/{symbol}?period=1mo` — the `history --json` object.
async fn history<D: DataSource>(
    State(st): Shared<D>,
    Path(symbol): Path<String>,
    Query(p): Query<HistoryParams>,
) -> ApiResult {
    let market = st.market_for(p.market.as_deref())?;
    check_symbol(&symbol)?;
    let period = p.period.as_deref().unwrap_or("1mo");
    views::check_period(period).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let resolved = market::resolve_symbol(&symbol, market);
    let hist = views::history(&st.source, &resolved, period, market)
        .await
        .map_err(ApiError::from_data)?;
    json(hist)
}

/// `GET /api/markets?crypto=true&fx=true&global=true` — the `markets --json` array.
async fn markets<D: DataSource>(State(st): Shared<D>, Query(p): Query<MarketsParams>) -> ApiResult {
    let market = st.market_for(p.market.as_deref())?;
    json(
        views::markets(&st.source, market, p.crypto, p.fx, p.global)
            .await
            .map_err(ApiError::from_data)?,
    )
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
}

/// `GET /api/search?q=tata motors` — the `search --json` array.
async fn search<D: DataSource>(State(st): Shared<D>, Query(p): Query<SearchParams>) -> ApiResult {
    let q = p.q.as_deref().unwrap_or("").trim();
    if q.is_empty() || q.len() > 100 {
        return Err(ApiError::BadRequest(
            "q is required (1-100 characters), e.g. /api/search?q=tata".into(),
        ));
    }
    json(st.source.search(q).await.map_err(ApiError::from_data)?)
}

#[derive(Deserialize)]
struct PortfolioParams {
    market: Option<String>,
    /// Currency for the totals (`portfolio --base`): inr or usd.
    base: Option<String>,
}

/// `GET /api/portfolio?base=usd` — the `portfolio --json` object.
async fn portfolio<D: DataSource>(
    State(st): Shared<D>,
    Query(p): Query<PortfolioParams>,
) -> ApiResult {
    let market = st.market_for(p.market.as_deref())?;
    let base = match p.base.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(b) if b.eq_ignore_ascii_case("inr") || b.eq_ignore_ascii_case("usd") => {
            Some(b.to_uppercase())
        }
        Some(b) => {
            return Err(ApiError::BadRequest(format!(
                "Unknown base currency '{}' (use inr or usd)",
                b
            )));
        }
    };
    json(
        views::portfolio(&st.source, market, base.as_deref())
            .await
            .map_err(ApiError::from_data)?,
    )
}

/// `GET /api/watchlist` — watched symbols with live prices.
async fn watchlist<D: DataSource>(State(st): Shared<D>) -> ApiResult {
    json(
        views::watchlist(&st.source)
            .await
            .map_err(ApiError::from_data)?,
    )
}

/// `GET /api/alerts` — every alert, with the live price and whether it fired.
async fn alerts<D: DataSource>(State(st): Shared<D>) -> ApiResult {
    json(
        views::alerts(&st.source)
            .await
            .map_err(ApiError::from_data)?,
    )
}

async fn healthz() -> Response {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .into_response()
}

async fn prometheus<D: DataSource>(State(st): Shared<D>) -> Response {
    let daemon = st.source.daemon_status();
    let body = st.metrics.render(&daemon, chrono::Utc::now().timestamp());
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

async fn not_found() -> Response {
    ApiError::NotFound("No such endpoint — see docs/server.md".into()).into_response()
}

/// Compare without leaking the matching prefix length through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `req` carries `Authorization: Bearer <token>`.
fn has_bearer(req: &Request, token: &str) -> bool {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        axum::Json(serde_json::json!({ "error": "Missing or invalid bearer token" })),
    )
        .into_response()
}

async fn require_token<D: DataSource>(State(st): Shared<D>, req: Request, next: Next) -> Response {
    if let Some(token) = &st.token
        && !has_bearer(&req, token)
    {
        return unauthorized();
    }
    next.run(req).await
}

/// `router` with every request needing the bearer token, when there is one
/// (`stockwise mcp --http`).
pub fn with_bearer_token(router: Router, token: Option<String>) -> Router {
    let Some(token) = token else {
        return router;
    };
    let token = Arc::new(token);
    router.layer(middleware::from_fn(move |req: Request, next: Next| {
        let token = token.clone();
        async move {
            if has_bearer(&req, &token) {
                next.run(req).await
            } else {
                unauthorized()
            }
        }
    }))
}

/// Without a token the API trusts the network position, so it only answers
/// requests addressed to this machine by a loopback name. That stops a web
/// page from reading it through DNS rebinding (evil.example resolving to
/// 127.0.0.1): the browser would still send `Host: evil.example`.
fn loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        // [::1]:8787
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(name, _port)| name),
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name.parse::<IpAddr>().is_ok_and(is_loopback)
}

async fn check_host<D: DataSource>(State(st): Shared<D>, req: Request, next: Next) -> Response {
    if st.token.is_none() {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !loopback_host(host) {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "error": "Host not allowed: without STOCKWISE_API_TOKEN the API only answers to localhost"
                })),
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// Method label for metrics: anything unusual is folded into OTHER so
/// clients can't grow the label set without bound.
fn method_label(m: &axum::http::Method) -> &'static str {
    use axum::http::Method;
    match *m {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

/// Count and time every request; mark responses private, uncacheable and
/// not embeddable.
async fn track<D: DataSource>(State(st): Shared<D>, req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".into());
    let method = method_label(req.method());
    let started = Instant::now();
    let mut resp = next.run(req).await;
    st.metrics
        .observe(method, &route, resp.status().as_u16(), started.elapsed());
    let headers = resp.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    web::secure(headers);
    resp
}

pub fn router<D: DataSource + 'static>(state: Arc<AppState<D>>) -> Router {
    let protected = Router::new()
        .route("/api/quote", get(quote::<D>))
        .route("/api/history/{symbol}", get(history::<D>))
        .route("/api/markets", get(markets::<D>))
        .route("/api/search", get(search::<D>))
        .route("/api/portfolio", get(portfolio::<D>))
        .route("/api/watchlist", get(watchlist::<D>))
        .route("/api/alerts", get(alerts::<D>))
        .route("/metrics", get(prometheus::<D>))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_token::<D>,
        ))
        .route_layer(middleware::from_fn(web::same_origin));
    Router::new()
        .route("/healthz", get(healthz))
        .merge(web::routes())
        .merge(protected)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            check_host::<D>,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), track::<D>))
        .with_state(state)
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// Refuse to expose the API beyond this machine unless asked to, and then
/// only behind a token.
pub fn check_bind(addrs: &[SocketAddr], allow_remote: bool, token: Option<&str>) -> Result<()> {
    if addrs.is_empty() {
        bail!("--bind did not resolve to any address");
    }
    if let Some(t) = token
        && t.len() < MIN_TOKEN_LEN
    {
        bail!(
            "STOCKWISE_API_TOKEN is too short — use at least {} random characters (e.g. `openssl rand -hex 32`)",
            MIN_TOKEN_LEN
        );
    }
    let Some(remote) = addrs.iter().find(|a| !is_loopback(a.ip())) else {
        return Ok(());
    };
    if !allow_remote {
        bail!(
            "Refusing to listen on {}: other machines could read your portfolio.\n  \
             Keep the default 127.0.0.1:8787, or pass --allow-remote and set STOCKWISE_API_TOKEN \
             to require a bearer token.",
            remote
        );
    }
    if token.is_none() {
        bail!(
            "--allow-remote needs STOCKWISE_API_TOKEN: every request must then send \
             `Authorization: Bearer <token>`"
        );
    }
    Ok(())
}

/// Where a browser on this machine finds the dashboard: a wildcard bind
/// (0.0.0.0, [::]) is reached through loopback.
fn dashboard_url(local: SocketAddr) -> String {
    let ip = match local.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}/", SocketAddr::new(ip, local.port()))
}

/// Open `url` in the default browser. A failure is reported, not fatal: the
/// URL is in the startup log.
fn open_browser(url: &str) {
    use crate::i18n::{Msg, tf};
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        Command::new("xdg-open")
    };
    cmd.arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let url = url.to_string();
    let warn = move |why: String| {
        eprintln!("  {}", tf(Msg::ServeOpenFailed, &[&why, &url]).yellow());
    };
    match cmd.spawn() {
        // Reap the launcher without holding up the server.
        Ok(mut child) => {
            std::thread::spawn(move || match child.wait() {
                Ok(status) if !status.success() => warn(status.to_string()),
                Err(e) => warn(e.to_string()),
                Ok(_) => {}
            });
        }
        Err(e) => warn(e.to_string()),
    }
}

pub async fn run(bind: &str, allow_remote: bool, market: Market, open: bool) -> Result<()> {
    let token = std::env::var("STOCKWISE_API_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(bind)
        .await
        .with_context(|| {
            format!(
                "Cannot resolve --bind {} (use HOST:PORT, e.g. 127.0.0.1:8787)",
                bind
            )
        })?
        .collect();
    check_bind(&addrs, allow_remote, token.as_deref())?;
    let listener = tokio::net::TcpListener::bind(&addrs[..])
        .await
        .with_context(|| format!("Cannot listen on {}", bind))?;
    let local = listener.local_addr()?;
    let url = dashboard_url(local);

    crate::display::print_header("StockWise dashboard and API — read-only");
    crate::display::print_kv(
        crate::i18n::t(crate::i18n::Msg::ServeDashboard),
        &url.cyan().to_string(),
    );
    crate::display::print_kv(
        "Listening on",
        &format!("http://{}", local).cyan().to_string(),
    );
    crate::display::print_kv(
        "Market",
        match market {
            Market::In => "in (bare tickers get .NS; override with ?market=us)",
            Market::Us => "us (override with ?market=in)",
            Market::Crypto => "crypto (bare tickers are coins; override with ?market=in)",
            Market::Fx => "fx (bare tickers are currency pairs; override with ?market=in)",
            Market::Commodity => {
                "commodity (gold/silver → COMEX futures, other bare tickers → NSE)"
            }
            Market::Global(_) => {
                "a global exchange (bare tickers get its suffix; override with ?market=in)"
            }
        },
    );
    crate::display::print_kv(
        "Auth",
        if token.is_some() {
            "bearer token required (STOCKWISE_API_TOKEN)"
        } else {
            "none — loopback only"
        },
    );
    println!(
        "\n  {}\n  {}\n  {}\n",
        "/api/quote?symbols=  /api/history/{symbol}?period=  /api/markets  /api/search?q=".dimmed(),
        "/api/portfolio  /api/watchlist  /api/alerts  /healthz  /metrics".dimmed(),
        "No trading endpoints. Press Ctrl+C to stop.".dimmed()
    );
    if open {
        open_browser(&url);
    }

    let state = Arc::new(AppState {
        source: views::LiveSource::new(),
        market,
        token,
        metrics: Metrics::new(),
    });
    // Ctrl+C stops accepting connections and lets requests in flight finish,
    // for at most SHUTDOWN_GRACE (a request can be inside Yahoo retries).
    let (stopping_tx, stopping) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, router(state)).with_graceful_shutdown(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stopping_tx.send(());
    });
    let grace = async {
        match stopping.await {
            Ok(()) => tokio::time::sleep(SHUTDOWN_GRACE).await,
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        result = std::future::IntoFuture::into_future(server) => result.context("Server error")?,
        _ = grace => eprintln!(
            "  Requests still running after {}s; stopping anyway.",
            SHUTDOWN_GRACE.as_secs()
        ),
    }
    println!("\n  Stopped.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_status::DaemonStatus;
    use crate::views::fixtures::Fixture;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app_with(fixture: Fixture, token: Option<&str>) -> Router {
        router(Arc::new(AppState {
            source: fixture,
            market: Market::In,
            token: token.map(str::to_string),
            metrics: Metrics::new(),
        }))
    }

    fn app() -> Router {
        app_with(Fixture::default(), None)
    }

    async fn get_with(app: &Router, uri: &str, auth: Option<&str>) -> (StatusCode, Response) {
        let mut req = Request::builder()
            .uri(uri)
            .header(header::HOST, "127.0.0.1:8787");
        if let Some(a) = auth {
            req = req.header(header::AUTHORIZATION, a);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        (resp.status(), resp)
    }

    async fn body(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let (status, resp) = get_with(app, uri, None).await;
        (status, serde_json::from_str(&body(resp).await).unwrap())
    }

    #[tokio::test]
    async fn healthz_is_ok() {
        let (status, v) = get_json(&app(), "/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn quote_resolves_bare_tickers_and_reports_missing() {
        let app = app();
        let (status, resp) = get_with(&app, "/api/quote?symbols=tcs,%20NOPE", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.headers()["x-stockwise-missing"], "NOPE.NS");
        assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
        let v: serde_json::Value = serde_json::from_str(&body(resp).await).unwrap();
        // Same objects as `quote --json`: Yahoo's camelCase fields.
        assert_eq!(v[0]["symbol"], "TCS.NS");
        assert_eq!(v[0]["regularMarketPrice"], 4100.0);
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn quote_errors() {
        let app = app();
        assert_eq!(
            get_json(&app, "/api/quote").await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            get_json(&app, "/api/quote?symbols=a%20b").await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            get_json(&app, "/api/quote?symbols=TCS&market=xx").await.0,
            StatusCode::BAD_REQUEST
        );
        let many = vec!["A"; MAX_SYMBOLS + 1].join(",");
        assert_eq!(
            get_json(&app, &format!("/api/quote?symbols={many}"))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let (status, v) = get_json(&app, "/api/quote?symbols=NOPE").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(v["error"], "No quote for NOPE.NS");
    }

    #[tokio::test]
    async fn quote_market_param_overrides_default() {
        let (status, v) = get_json(&app(), "/api/quote?symbols=^GSPC&market=us").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v[0]["symbol"], "^GSPC");
    }

    #[tokio::test]
    async fn history_matches_cli_json() {
        let (status, v) = get_json(&app(), "/api/history/TCS?period=1mo").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["symbol"], "TCS.NS");
        assert_eq!(v["interval"], "1d");
        assert_eq!(v["period"], "1mo");
        assert_eq!(v["bars"].as_array().unwrap().len(), 5);
        assert_eq!(v["bars"][4]["close"], 107.5);
        assert!(v["bars"][0].get("ts").is_none());
    }

    #[tokio::test]
    async fn history_errors() {
        let app = app();
        assert_eq!(
            get_json(&app, "/api/history/TCS?period=7x").await.0,
            StatusCode::BAD_REQUEST
        );
        // Unknown symbol: Yahoo answers 404, which surfaces as a data error.
        let (status, v) = get_json(&app, "/api/history/NOPE").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(v["error"].as_str().unwrap().contains("404"));
    }

    #[tokio::test]
    async fn markets_search_portfolio_watchlist_alerts() {
        let app = app();
        let (s, v) = get_json(&app, "/api/markets").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v[0]["name"], "NIFTY 50");
        assert_eq!(v[0]["region"], "Indian Markets");

        let (s, v) = get_json(&app, "/api/search?q=reliance").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v[0]["symbol"], "RELIANCE.NS");
        assert_eq!(
            get_json(&app, "/api/search?q=").await.0,
            StatusCode::BAD_REQUEST
        );

        // Same shapes as `portfolio --json`, `watch --json`, `alert check --json`.
        let (s, v) = get_json(&app, "/api/portfolio").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["totals"]["pnl"], 5500.0);
        assert_eq!(v["holdings"][0]["weight_percent"], 84.54);
        assert_eq!(v["unpriced"][0], "DELISTED.NS");

        let (s, v) = get_json(&app, "/api/watchlist").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v[0]["symbol"], "RELIANCE.NS");
        assert_eq!(v[0]["price"], 1300.0);

        let (s, v) = get_json(&app, "/api/alerts").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v[0]["status"], "triggered");
        assert_eq!(v[1]["status"], "waiting");
        assert_eq!(v[2]["status"], "no_price");
        assert!(v[2]["price"].is_null());
    }

    #[tokio::test]
    async fn upstream_failure_is_502_without_the_crumb() {
        let f = Fixture::default();
        *f.fail.lock().unwrap() = Some(
            "error sending request for url (https://x.yahoo.com/q?symbols=A&crumb=s3cr3t)".into(),
        );
        let (status, v) = get_json(&app_with(f, None), "/api/markets").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        let msg = v["error"].as_str().unwrap();
        assert!(!msg.contains("s3cr3t"), "{msg}");
        assert!(msg.contains("crumb=REDACTED"), "{msg}");
    }

    #[tokio::test]
    async fn local_file_errors_are_500_without_paths() {
        struct BrokenFiles(Fixture);
        impl DataSource for BrokenFiles {
            async fn quotes(&self, s: &[&str]) -> anyhow::Result<Vec<crate::api::Quote>> {
                self.0.quotes(s).await
            }
            async fn chart(
                &self,
                s: &str,
                r: &str,
                i: &str,
            ) -> anyhow::Result<crate::api::ChartData> {
                self.0.chart(s, r, i).await
            }
            async fn search(&self, q: &str) -> anyhow::Result<Vec<crate::api::SearchResult>> {
                self.0.search(q).await
            }
            fn alerts(&self) -> anyhow::Result<crate::alerts::AlertStore> {
                anyhow::bail!("/Users/me/stockwise/alerts.json is not valid JSON")
            }
        }
        let app = router(Arc::new(AppState {
            source: BrokenFiles(Fixture::default()),
            market: Market::In,
            token: None,
            metrics: Metrics::new(),
        }));
        let (status, v) = get_json(&app, "/api/alerts").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!v["error"].as_str().unwrap().contains("/Users"), "{v}");
    }

    #[tokio::test]
    async fn foreign_host_names_are_refused_without_a_token() {
        let app = app();
        let get = |host: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .uri("/api/portfolio")
                        .header(header::HOST, host)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            }
        };
        // DNS rebinding: a page on evil.example resolved to 127.0.0.1.
        assert_eq!(get("evil.example:8787").await, StatusCode::FORBIDDEN);
        assert_eq!(get("192.168.1.5:8787").await, StatusCode::FORBIDDEN);
        for ok in [
            "localhost:8787",
            "127.0.0.1:8787",
            "[::1]:8787",
            "app.localhost",
        ] {
            assert_eq!(get(ok).await, StatusCode::OK, "{ok}");
        }
        // With a token any Host is fine (remote use).
        let token = "0123456789abcdef0123";
        let app = app_with(Fixture::default(), Some(token));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/portfolio")
                    .header(header::HOST, "my-server.lan:8787")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn unusual_methods_share_one_metrics_label() {
        use axum::http::Method;
        assert_eq!(method_label(&Method::GET), "GET");
        assert_eq!(
            method_label(&Method::from_bytes(b"A0001").unwrap()),
            "OTHER"
        );
    }

    #[tokio::test]
    async fn unknown_routes_and_methods() {
        let app = app();
        assert_eq!(get_json(&app, "/api/trade").await.0, StatusCode::NOT_FOUND);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/portfolio")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn token_guards_everything_but_healthz() {
        let token = "0123456789abcdef0123";
        let app = app_with(Fixture::default(), Some(token));
        for uri in ["/api/portfolio", "/metrics", "/api/quote?symbols=TCS"] {
            let (status, resp) = get_with(&app, uri, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(resp.headers()[header::WWW_AUTHENTICATE], "Bearer");
            let (status, _) = get_with(&app, uri, Some("Bearer wrong")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
            let (status, _) = get_with(&app, uri, Some(&format!("Bearer {token}"))).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
        }
        assert_eq!(get_with(&app, "/healthz", None).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_count_routes_and_report_the_daemon() {
        let f = Fixture {
            daemon: vec![DaemonStatus {
                market: "IN".into(),
                updated_at: chrono::Utc::now().timestamp(),
                open_positions: 3,
                day_pnl: 420.5,
                kill_switch: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let app = app_with(f, None);
        get_json(&app, "/api/history/TCS").await;
        get_json(&app, "/api/history/INFY").await;
        get_json(&app, "/nope").await;
        let (status, resp) = get_with(&app, "/metrics", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            resp.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/plain; version=0.0.4")
        );
        let text = body(resp).await;
        assert!(
            text.contains(
                "stockwise_http_requests_total{method=\"GET\",route=\"/api/history/{symbol}\",status=\"200\"} 2"
            ),
            "{text}"
        );
        assert!(text.contains(
            "stockwise_http_requests_total{method=\"GET\",route=\"unmatched\",status=\"404\"} 1"
        ));
        assert!(text.contains("stockwise_daemon_up{market=\"IN\"} 1"));
        assert!(text.contains("stockwise_daemon_up{market=\"US\"} 0"));
        assert!(text.contains("stockwise_daemon_open_positions{market=\"IN\"} 3"));
        assert!(text.contains("stockwise_daemon_day_pnl{market=\"IN\"} 420.5"));
        assert!(text.contains("stockwise_daemon_kill_switch{market=\"IN\"} 0"));
        assert!(text.contains("stockwise_upstream_errors_total "));
        assert!(text.contains("stockwise_cache_hits_total{layer=\"memory\"} "));
    }

    #[test]
    fn bind_rules() {
        let local: SocketAddr = "127.0.0.1:8787".parse().unwrap();
        let local6: SocketAddr = "[::1]:8787".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:8787".parse().unwrap();
        let any: SocketAddr = "0.0.0.0:8787".parse().unwrap();
        let lan: SocketAddr = "192.168.1.5:8787".parse().unwrap();
        let token = "0123456789abcdef";

        assert!(check_bind(&[local], false, None).is_ok());
        assert!(check_bind(&[local6, mapped], false, None).is_ok());
        assert!(check_bind(&[], false, None).is_err());
        for addr in [any, lan] {
            let err = check_bind(&[addr], false, Some(token))
                .unwrap_err()
                .to_string();
            assert!(err.contains("--allow-remote"), "{err}");
            let err = check_bind(&[addr], true, None).unwrap_err().to_string();
            assert!(err.contains("STOCKWISE_API_TOKEN"), "{err}");
            assert!(check_bind(&[addr], true, Some(token)).is_ok());
        }
        // A localhost name that also resolves to a LAN address is remote.
        assert!(check_bind(&[local, lan], false, None).is_err());
        assert!(check_bind(&[local], false, Some("short")).is_err());
    }

    #[test]
    fn redact_strips_every_crumb() {
        assert_eq!(
            redact("url (https://a?x=1&crumb=abc) and crumb=def&y=2"),
            "url (https://a?x=1&crumb=REDACTED) and crumb=REDACTED&y=2"
        );
        assert_eq!(redact("no secrets"), "no secrets");
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    /// GET `uri` with extra request headers.
    async fn get_headers(app: &Router, uri: &str, headers: &[(&str, &str)]) -> Response {
        let mut req = Request::builder()
            .uri(uri)
            .header(header::HOST, "127.0.0.1:8787");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn assert_hardened(resp: &Response, what: &str) {
        let h = resp.headers();
        assert_eq!(h[header::CONTENT_SECURITY_POLICY], web::CSP, "{what}");
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY", "{what}");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{what}");
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer", "{what}");
        assert_eq!(h["cross-origin-resource-policy"], "same-origin", "{what}");
        assert_eq!(h["cross-origin-opener-policy"], "same-origin", "{what}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{what}");
        assert!(
            !h.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            "{what}"
        );
    }

    #[tokio::test]
    async fn dashboard_is_html_with_a_strict_csp() {
        let (status, resp) = get_with(&app(), "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_hardened(&resp, "/");
        let html = body(resp).await;
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains(r#"<script src="/assets/app.js" defer></script>"#));
        assert!(html.contains(r#"<link rel="stylesheet" href="/assets/app.css">"#));
    }

    #[tokio::test]
    async fn dashboard_assets_have_their_content_types() {
        let app = app();
        for (uri, ty) in [
            ("/assets/app.js", "text/javascript; charset=utf-8"),
            ("/assets/app.css", "text/css; charset=utf-8"),
            ("/assets/favicon.svg", "image/svg+xml"),
        ] {
            let (status, resp) = get_with(&app, uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert_eq!(resp.headers()[header::CONTENT_TYPE], ty, "{uri}");
            assert_hardened(&resp, uri);
            assert!(!body(resp).await.is_empty(), "{uri}");
        }
        assert_eq!(
            get_with(&app, "/assets/nope.js", None).await.0,
            StatusCode::NOT_FOUND
        );
        // HEAD works for every asset (link checkers, `curl -I`).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/")
                    .header(header::HOST, "localhost:8787")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_responses_carry_the_same_hardening() {
        let (status, resp) = get_with(&app(), "/api/portfolio", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_hardened(&resp, "/api/portfolio");
        let (status, resp) = get_with(&app(), "/nope", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_hardened(&resp, "/nope");
    }

    #[tokio::test]
    async fn dashboard_page_needs_no_token_but_its_data_does() {
        let token = "0123456789abcdef0123";
        let app = app_with(Fixture::default(), Some(token));
        for uri in ["/", "/assets/app.js", "/assets/app.css"] {
            assert_eq!(get_with(&app, uri, None).await.0, StatusCode::OK, "{uri}");
        }
        assert_eq!(
            get_with(&app, "/api/watchlist", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        // Without a token the page, too, only answers to localhost names.
        let resp = app_with(Fixture::default(), None)
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "evil.example:8787")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn data_is_only_for_same_origin_pages() {
        let app = app();
        for site in ["cross-site", "same-site"] {
            for uri in ["/api/portfolio", "/api/quote?symbols=TCS", "/metrics"] {
                let resp = get_headers(
                    &app,
                    uri,
                    &[
                        ("sec-fetch-site", site),
                        ("origin", "http://localhost:3000"),
                    ],
                )
                .await;
                assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{site} {uri}");
                assert_hardened(&resp, uri);
            }
        }
        // The dashboard's own fetches, typed URLs and non-browser clients.
        for headers in [
            &[("sec-fetch-site", "same-origin")][..],
            &[("sec-fetch-site", "none")][..],
            &[][..],
        ] {
            let resp = get_headers(&app, "/api/portfolio", headers).await;
            assert_eq!(resp.status(), StatusCode::OK, "{headers:?}");
        }
        // A link from another site may open the page; it holds no data.
        let resp = get_headers(
            &app,
            "/",
            &[
                ("sec-fetch-site", "cross-site"),
                ("sec-fetch-mode", "navigate"),
            ],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        // /healthz stays open to monitors in browsers too.
        let resp = get_headers(&app, "/healthz", &[("sec-fetch-site", "cross-site")]).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_cors_preflight_or_allow_origin() {
        let app = app();
        let resp = get_headers(
            &app,
            "/api/watchlist",
            &[("origin", "https://evil.example")],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            !resp
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/watchlist")
                    .header(header::HOST, "localhost:8787")
                    .header("origin", "https://evil.example")
                    .header("access-control-request-method", "GET")
                    .header("access-control-request-headers", "authorization")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        for h in [
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            header::ACCESS_CONTROL_ALLOW_METHODS,
        ] {
            assert!(!resp.headers().contains_key(&h), "{h}");
        }
    }

    #[tokio::test]
    async fn dashboard_routes_have_fixed_metric_labels() {
        let app = app();
        get_with(&app, "/", None).await;
        get_with(&app, "/assets/app.js", None).await;
        let text = body(get_with(&app, "/metrics", None).await.1).await;
        assert!(
            text.contains(
                "stockwise_http_requests_total{method=\"GET\",route=\"/\",status=\"200\"} 1"
            ),
            "{text}"
        );
        assert!(text.contains("route=\"/assets/app.js\",status=\"200\"} 1"));
    }

    #[test]
    fn dashboard_url_reaches_wildcard_binds_through_loopback() {
        let url = |s: &str| dashboard_url(s.parse().unwrap());
        assert_eq!(url("127.0.0.1:8787"), "http://127.0.0.1:8787/");
        assert_eq!(url("0.0.0.0:9000"), "http://127.0.0.1:9000/");
        assert_eq!(url("[::]:8787"), "http://[::1]:8787/");
        assert_eq!(url("[::1]:8787"), "http://[::1]:8787/");
        assert_eq!(url("192.168.1.5:8787"), "http://192.168.1.5:8787/");
    }
}
