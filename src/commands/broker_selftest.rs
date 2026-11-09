//! `trade selftest`: a strictly read-only check of a broker integration
//! against the user's own account, printed as a redacted report to paste
//! into a GitHub issue (see docs/brokers.md, "Help verify a broker").
//!
//! The session is wrapped in [`ReadOnly`], so no order, GTT or cancel can
//! reach the broker whatever this file does — and this file never calls
//! those methods. The dry-run order goes through the same checks as a real
//! one ([`prepare_order`], [`check_cap`]) and stops before anything is sent.

use super::trade::{Intent, check_cap, prepare_order};
use super::*;
use crate::brokers::readonly::ReadOnly;
use crate::brokers::{self, Broker, BrokerKind, Endpoint, OrderKind, Side, redact_id};

/// Liquid NSE symbols every broker should resolve exactly. NIFTYBEES is
/// the Nifty 50 ETF.
const SYMBOLS: [&str; 3] = ["RELIANCE", "TCS", "NIFTYBEES"];

/// Pause between broker calls, to stay well under per-second rate limits.
const PAUSE: std::time::Duration = std::time::Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl Status {
    fn mark(self) -> &'static str {
        match self {
            Status::Pass => "✅ pass",
            Status::Warn => "⚠️ warn",
            Status::Fail => "❌ fail",
            Status::Skip => "➖ skip",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

/// A response-shape check: field names only, never values.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ShapeLine {
    pub endpoint: Endpoint,
    pub status: Status,
    pub rows: usize,
    pub missing: Vec<String>,
    pub unknown: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct Summary {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub skip: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Report {
    pub stockwise: &'static str,
    pub os: String,
    pub date: String,
    pub market: String,
    pub broker: &'static str,
    pub broker_name: &'static str,
    pub experimental: bool,
    /// Last two characters of the account id.
    pub account: Option<String>,
    pub secrets: String,
    /// `"enable_orders": true` in the broker's config file.
    pub orders_enabled: bool,
    pub max_order_value_set: bool,
    pub amounts_redacted: bool,
    /// Always true: the session was read-only for the whole run.
    pub read_only: bool,
    pub checks: Vec<Check>,
    pub shapes: Vec<ShapeLine>,
    pub summary: Summary,
}

/// Removes personal data from everything the report prints.
pub(super) struct Redactor {
    /// Account and app ids to cut down wherever they appear.
    ids: Vec<String>,
    amounts: bool,
}

impl Redactor {
    pub fn new(ids: Vec<String>, amounts: bool) -> Self {
        let mut ids: Vec<String> = ids
            .into_iter()
            .map(|i| i.trim().to_string())
            .filter(|i| i.chars().count() >= 3)
            .collect();
        // Longest first, so an id that contains another is cut whole.
        ids.sort_by_key(|i| std::cmp::Reverse(i.len()));
        Self { ids, amounts }
    }

    fn add_id(&mut self, id: &str) {
        let id = id.trim();
        if id.chars().count() >= 3 && !self.ids.iter().any(|i| i == id) {
            self.ids.push(id.to_string());
            self.ids.sort_by_key(|i| std::cmp::Reverse(i.len()));
        }
    }

    /// Scrub text built from broker data: known ids, e-mail addresses and
    /// long tokens. URLs keep their path but lose the query string.
    pub fn light(&self, s: &str) -> String {
        self.scrub(s, false)
    }

    /// Scrub free text such as error messages: everything [`Redactor::light`]
    /// does, plus runs of 6+ digits (order, account and phone numbers).
    pub fn text(&self, s: &str) -> String {
        self.scrub(s, true)
    }

    fn scrub(&self, s: &str, digits: bool) -> String {
        let mut s = s.to_string();
        for id in &self.ids {
            for variant in [id.clone(), id.to_uppercase(), id.to_lowercase()] {
                s = s.replace(&variant, &redact_id(id));
            }
        }
        let mut out = String::with_capacity(s.len());
        let mut token = String::new();
        let flush = |token: &mut String, out: &mut String| {
            if !token.is_empty() {
                out.push_str(&scrub_token(token, digits));
                token.clear();
            }
        };
        for c in s.chars() {
            if c.is_ascii_alphanumeric() || "_-.=+/@:%?&".contains(c) {
                token.push(c);
            } else {
                flush(&mut token, &mut out);
                out.push(c);
            }
        }
        flush(&mut token, &mut out);
        out
    }

    /// A rupee amount, or `₹•••` with `--redact-amounts`.
    pub fn money(&self, v: Option<f64>) -> String {
        match v {
            _ if self.amounts => "₹•••".into(),
            Some(v) => format!("₹{v:.2}"),
            None => "unknown".into(),
        }
    }
}

fn scrub_token(token: &str, digit_runs: bool) -> String {
    if token.starts_with("http://") || token.starts_with("https://") {
        return token.split('?').next().unwrap_or("").to_string();
    }
    if token.contains('@') && token.contains('.') {
        return "[email]".into();
    }
    if token.chars().count() >= 24 {
        return "[redacted]".into();
    }
    if !digit_runs {
        return token.to_string();
    }
    // Runs of 6 or more digits.
    let mut out = String::new();
    let mut digits = String::new();
    for c in token.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if digits.len() >= 6 {
            out.push_str("•••");
        } else {
            out.push_str(&digits);
        }
        digits.clear();
        if c != '\0' {
            out.push(c);
        }
    }
    out
}

/// What the checks need besides the session, so tests can supply it.
pub(super) struct Context {
    pub status: brokers::ConfigStatus,
    pub limits: brokers::OrderLimits,
    /// Last RELIANCE price (Yahoo), for the dry-run order.
    pub last_price: Option<f64>,
    pub today: chrono::NaiveDate,
    pub market: String,
    pub pause: std::time::Duration,
}

struct Run {
    checks: Vec<Check>,
    red: Redactor,
}

impl Run {
    fn push(&mut self, name: &'static str, status: Status, detail: impl Into<String>) {
        let detail = self.red.light(&detail.into());
        self.checks.push(Check {
            name,
            status,
            detail,
        });
    }

    fn fail(&mut self, name: &'static str, err: &anyhow::Error) {
        let detail = self.red.text(&format!("{err:#}"));
        self.push(name, Status::Fail, detail);
    }
}

fn counts<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let mut tally: Vec<(String, usize)> = Vec::new();
    for s in items {
        match tally.iter_mut().find(|(k, _)| k == s) {
            Some((_, n)) => *n += 1,
            None => tally.push((s.to_string(), 1)),
        }
    }
    if tally.is_empty() {
        return "—".into();
    }
    tally
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where the broker's secrets live, without paths or hints.
fn storage_name(storage: &brokers::secrets::Storage) -> &'static str {
    match storage {
        brokers::secrets::Storage::Keyring { .. } => "OS keyring",
        brokers::secrets::Storage::File => "owner-only file",
    }
}

/// Run every check against `broker` (already read-only) and build the
/// report. `broker` is `None` when the session could not be opened.
pub(super) async fn run_checks(
    kind: BrokerKind,
    broker: Result<&ReadOnly, &anyhow::Error>,
    ctx: Context,
    redact_amounts: bool,
) -> Report {
    let mut run = Run {
        checks: Vec::new(),
        red: Redactor::new(ctx.status.identifiers.clone(), redact_amounts),
    };
    let storage = storage_name(&ctx.status.storage);
    run.push(
        "Config",
        Status::Pass,
        format!("credentials found · secrets in the {storage}"),
    );

    let mut account = None;
    let mut shapes = Vec::new();
    let pause = ctx.pause;
    let broker = match broker {
        Ok(b) => {
            let detail = match (kind, ctx.status.session_date) {
                (BrokerKind::Angel, _) => "logged in with PIN + TOTP".to_string(),
                (_, Some(d)) if d == ctx.today => format!("today's session ({d})"),
                (_, Some(d)) => format!(
                    "session from {d} — sessions expire daily; run `trade login` if reads fail"
                ),
                (_, None) => "session loaded".to_string(),
            };
            let stale = kind != BrokerKind::Angel && ctx.status.session_date != Some(ctx.today);
            let status = if stale { Status::Warn } else { Status::Pass };
            run.push("Session", status, detail);
            b
        }
        Err(e) => {
            run.fail("Session", e);
            for name in [
                "Profile",
                "Holdings",
                "Positions",
                "Order book",
                "Trade book",
                "GTT list",
                "Instruments",
                "Dry-run order",
            ] {
                run.push(name, Status::Skip, "no session");
            }
            return finish(kind, ctx, run, account, shapes, redact_amounts);
        }
    };

    match broker.profile().await {
        Ok(p) => {
            if let Some(id) = &p.user_id {
                run.red.add_id(id);
                account = Some(redact_id(id));
            }
            let list = |v: &[String]| {
                if v.is_empty() {
                    "—".to_string()
                } else {
                    v.join(", ")
                }
            };
            let status = if p.user_id.is_some() {
                Status::Pass
            } else {
                Status::Warn
            };
            run.push(
                "Profile",
                status,
                format!(
                    "account {} · exchanges {} · products {}",
                    account.as_deref().unwrap_or("not returned"),
                    list(&p.exchanges),
                    list(&p.products)
                ),
            );
        }
        Err(e) => run.fail("Profile", &e),
    }
    tokio::time::sleep(pause).await;

    match broker.holdings().await {
        Ok(h) => {
            let n = h.len();
            let qty = h.iter().filter(|x| x.qty > 0.0).count();
            let avg = h.iter().filter(|x| x.avg_price.is_some()).count();
            let ltp = h.iter().filter(|x| x.ltp.is_some()).count();
            let status = if qty < n || avg < n {
                Status::Warn
            } else {
                Status::Pass
            };
            run.push(
                "Holdings",
                status,
                format!(
                    "{n} rows · with quantity {qty} · with avg price {avg} · with last price {ltp}"
                ),
            );
        }
        Err(e) => run.fail("Holdings", &e),
    }
    tokio::time::sleep(pause).await;

    match broker.positions().await {
        Ok(p) => run.push(
            "Positions",
            Status::Pass,
            format!(
                "{} rows · products: {}",
                p.len(),
                counts(p.iter().map(|x| x.product.as_deref().unwrap_or("?")))
            ),
        ),
        Err(e) => run.fail("Positions", &e),
    }
    tokio::time::sleep(pause).await;

    match broker.orders().await {
        Ok(o) => run.push(
            "Order book",
            Status::Pass,
            format!(
                "{} orders today · statuses: {}",
                o.len(),
                counts(o.iter().map(|x| x.status.as_deref().unwrap_or("?")))
            ),
        ),
        Err(e) => run.fail("Order book", &e),
    }
    tokio::time::sleep(pause).await;

    match broker.trades(None).await {
        Ok(t) => {
            let delivery = t.iter().filter(|x| x.is_delivery_equity()).count();
            let unpriced = t
                .iter()
                .filter(|x| x.price.is_none() || x.side.is_none())
                .count();
            let status = if unpriced > 0 {
                Status::Warn
            } else {
                Status::Pass
            };
            run.push(
                "Trade book",
                status,
                format!(
                    "{} fills today · delivery equity {} · other {} · missing price or side {}",
                    t.len(),
                    delivery,
                    t.len() - delivery,
                    unpriced
                ),
            );
        }
        Err(e) => run.fail("Trade book", &e),
    }
    tokio::time::sleep(pause).await;

    if broker.supports_gtt() {
        match broker.gtt_list().await {
            Ok(g) => run.push(
                "GTT list",
                Status::Pass,
                format!(
                    "{} GTTs · statuses: {}",
                    g.len(),
                    counts(g.iter().map(|x| x.status.as_deref().unwrap_or("?")))
                ),
            ),
            Err(e) => run.fail("GTT list", &e),
        }
        tokio::time::sleep(pause).await;
    } else {
        run.push(
            "GTT list",
            Status::Skip,
            format!("GTT orders are not supported for {}", kind.label()),
        );
    }

    let mut resolved = Vec::new();
    let mut failed = Vec::new();
    for sym in SYMBOLS {
        match broker.resolve_instrument(sym, "NSE").await {
            Ok(i) => resolved.push(format!("{sym} → {} (token {})", i.tradingsymbol, i.token)),
            Err(e) => failed.push(format!("{sym}: {}", run.red.text(&format!("{e:#}")))),
        }
        tokio::time::sleep(pause).await;
    }
    if failed.is_empty() {
        run.push("Instruments", Status::Pass, resolved.join("; "));
    } else {
        run.push(
            "Instruments",
            Status::Fail,
            format!("{}; resolved: {}", failed.join("; "), resolved.join("; ")),
        );
    }

    dry_run(&mut run, broker, &ctx).await;

    for ep in Endpoint::ALL {
        if ep == Endpoint::Gtts && !broker.supports_gtt() {
            continue;
        }
        let Some(shape) = broker.shape(ep) else {
            continue;
        };
        let line = match broker.rows(ep).await {
            Ok(rows) => {
                let r = brokers::check_shape(&rows, &shape);
                let status = match () {
                    _ if r.rows == 0 => Status::Skip,
                    _ if !r.missing.is_empty() || r.not_objects > 0 => Status::Warn,
                    _ => Status::Pass,
                };
                ShapeLine {
                    endpoint: ep,
                    status,
                    rows: r.rows,
                    missing: r.missing,
                    unknown: r.unknown,
                    error: None,
                }
            }
            Err(e) => ShapeLine {
                endpoint: ep,
                status: Status::Fail,
                rows: 0,
                missing: Vec::new(),
                unknown: Vec::new(),
                error: Some(run.red.text(&format!("{e:#}"))),
            },
        };
        shapes.push(line);
        tokio::time::sleep(pause).await;
    }

    finish(kind, ctx, run, account, shapes, redact_amounts)
}

/// Run the order checks on a one-share LIMIT buy 5% below the market (or
/// MARKET without a price) and stop before anything is sent.
async fn dry_run(run: &mut Run, broker: &ReadOnly, ctx: &Context) {
    let symbol = SYMBOLS[0];
    let kind = match ctx.last_price {
        Some(last) => OrderKind::Limit {
            // A 0.05 tick, 5% under the last price.
            price: ((last * 0.95 * 20.0).floor() / 20.0 * 100.0).round() / 100.0,
        },
        None => OrderKind::Market,
    };
    let intent = Intent::Order(kind);
    let prepared = prepare_order(broker, Side::Buy, symbol, 1, intent, ctx.last_price).await;
    match prepared {
        Ok(p) => {
            // The cap's own message carries amounts; the report only says
            // whether it would pass.
            let cap = match check_cap(broker.kind(), ctx.limits.max_order_value, p.est_value) {
                Ok(()) if ctx.limits.max_order_value.is_some() => "within max_order_value",
                Ok(()) => "no max_order_value set",
                Err(_) if p.est_value.is_some() => "would be refused: over max_order_value",
                Err(_) => "would be refused: max_order_value can't be checked without a price",
            };
            let order = match kind {
                OrderKind::Limit { price } => format!("LIMIT {}", run.red.money(Some(price))),
                _ => "MARKET (no last price from Yahoo)".to_string(),
            };
            let detail = format!(
                "BUY 1 × {} ({}, token {}) {order} · est. {} · {cap} · not sent",
                p.instrument.tradingsymbol,
                p.instrument.exchange,
                p.instrument.token,
                run.red.money(p.est_value),
            );
            run.push("Dry-run order", Status::Pass, detail);
        }
        Err(e) => run.fail("Dry-run order", &e),
    }
}

fn finish(
    kind: BrokerKind,
    ctx: Context,
    run: Run,
    account: Option<String>,
    shapes: Vec<ShapeLine>,
    redact_amounts: bool,
) -> Report {
    let mut summary = Summary::default();
    for s in run
        .checks
        .iter()
        .map(|c| c.status)
        .chain(shapes.iter().map(|s| s.status))
    {
        match s {
            Status::Pass => summary.pass += 1,
            Status::Warn => summary.warn += 1,
            Status::Fail => summary.fail += 1,
            Status::Skip => summary.skip += 1,
        }
    }
    Report {
        stockwise: env!("CARGO_PKG_VERSION"),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        date: ctx.today.to_string(),
        market: ctx.market,
        broker: kind.id(),
        broker_name: kind.label(),
        experimental: kind.experimental(),
        account,
        secrets: storage_name(&ctx.status.storage).to_string(),
        orders_enabled: ctx.limits.enable_orders,
        max_order_value_set: ctx.limits.max_order_value.is_some(),
        amounts_redacted: redact_amounts,
        read_only: true,
        checks: run.checks,
        shapes,
        summary,
    }
}

/// Markdown table cell: no pipes or newlines.
fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// The report as Markdown for a GitHub issue.
pub(super) fn render_markdown(r: &Report) -> String {
    let mut out = String::new();
    let label = if r.experimental {
        format!("{} (experimental)", r.broker_name)
    } else {
        r.broker_name.to_string()
    };
    out.push_str(&format!("### StockWise broker self-test — {label}\n\n"));
    out.push_str("| | |\n|---|---|\n");
    out.push_str(&format!("| StockWise | {} |\n", r.stockwise));
    out.push_str(&format!("| OS | {} |\n", r.os));
    out.push_str(&format!(
        "| Date (IST) | {} · NSE session: {} |\n",
        r.date, r.market
    ));
    out.push_str(&format!(
        "| Account | {} |\n",
        r.account.as_deref().unwrap_or("—")
    ));
    out.push_str(&format!("| Secrets | {} |\n", cell(&r.secrets)));
    let live = if !r.experimental {
        "market/limit orders on; stop-loss/GTT need `enable_orders`"
    } else if r.orders_enabled {
        "`enable_orders` is on"
    } else {
        "off (`enable_orders` not set)"
    };
    out.push_str(&format!("| Live orders | {live} |\n"));
    out.push_str(&format!(
        "| max_order_value | {} |\n",
        if r.max_order_value_set {
            "set"
        } else {
            "not set"
        }
    ));
    out.push('\n');
    out.push_str("| Check | Result | Details |\n|---|---|---|\n");
    for c in &r.checks {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            c.name,
            c.status.mark(),
            cell(&c.detail)
        ));
    }
    if !r.shapes.is_empty() {
        out.push_str("\n**Response shapes** (field names only)\n\n");
        out.push_str("| Endpoint | Result | Rows | Missing fields | New fields |\n");
        out.push_str("|---|---|---|---|---|\n");
        let list = |v: &[String]| {
            if v.is_empty() {
                "—".to_string()
            } else {
                v.iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        for s in &r.shapes {
            let missing = match &s.error {
                Some(e) => cell(e),
                None => list(&s.missing),
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                s.endpoint.label(),
                s.status.mark(),
                s.rows,
                missing,
                list(&s.unknown)
            ));
        }
    }
    out.push_str(&format!(
        "\n{} passed · {} warnings · {} failed · {} skipped\n\n",
        r.summary.pass, r.summary.warn, r.summary.fail, r.summary.skip
    ));
    out.push_str(
        "_Read-only run: StockWise sent no order, GTT or cancel request. No credentials \
         are included; the account id is cut to its last 2 characters",
    );
    out.push_str(if r.amounts_redacted {
        "; amounts are hidden._\n"
    } else {
        "._\n"
    });
    out
}

pub async fn cmd_trade_selftest(
    kind: BrokerKind,
    redact_amounts: bool,
    format: Format,
) -> Result<()> {
    let status = brokers::config_status(kind, false)?;
    if !status.configured {
        let flag = if kind == BrokerKind::Angel {
            String::new()
        } else {
            format!(" --broker {}", kind.id())
        };
        bail!(
            "{} is not configured, so there is nothing to test. Run `stockwise trade setup{flag}` \
             and `stockwise trade config{flag}` first.",
            kind.label()
        );
    }
    let limits = brokers::order_limits(kind).unwrap_or_default();
    if format.is_table() {
        eprintln!(
            "  Running read-only checks against {} — no order, GTT or cancel is sent…",
            kind.display()
        );
    }
    let session = brokers::connect(kind).await.map(ReadOnly::new);
    let ctx = Context {
        status,
        limits,
        last_price: super::trade::last_price(SYMBOLS[0]).await,
        today: brokers::ist_now().date_naive(),
        market: crate::clock::market_phase(Market::In, chrono::Utc::now())
            .to_string()
            .to_lowercase(),
        pause: PAUSE,
    };
    let report = run_checks(kind, session.as_ref(), ctx, redact_amounts).await;
    if format == Format::Json {
        output::print_json(&report)?;
    } else {
        println!();
        println!(
            "<!-- Paste everything below into a broker test report: \
             https://github.com/iamsaquib8/stockwise/issues/new?template=broker_test_report.yml -->"
        );
        println!();
        print!("{}", render_markdown(&report));
        println!();
    }
    if report.summary.fail > 0 {
        bail!(
            "{} self-test check(s) failed — see the report above",
            report.summary.fail
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::{
        Gtt, GttRequest, Holding, Instrument, Order, OrderAck, OrderRequest, Position, Profile,
        Trade,
    };
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    /// A broker account with a little of everything, recording any
    /// order-changing call that reaches it.
    #[derive(Clone, Default)]
    struct Account {
        writes: Arc<Mutex<Vec<&'static str>>>,
        fail_holdings: bool,
    }

    #[async_trait]
    impl Broker for Account {
        fn kind(&self) -> BrokerKind {
            BrokerKind::Zerodha
        }
        async fn holdings(&self) -> Result<Vec<Holding>> {
            if self.fail_holdings {
                bail!("Kite holdings failed: invalid session for XX0042, order 151220000000000");
            }
            Ok(vec![Holding {
                symbol: "INFY".into(),
                qty: 10.0,
                avg_price: Some(1400.0),
                ltp: Some(1500.0),
                ..Default::default()
            }])
        }
        async fn positions(&self) -> Result<Vec<Position>> {
            Ok(vec![])
        }
        async fn orders(&self) -> Result<Vec<Order>> {
            Ok(vec![Order {
                id: "151220000000000".into(),
                status: Some("COMPLETE".into()),
                ..Default::default()
            }])
        }
        async fn profile(&self) -> Result<Profile> {
            Ok(Profile {
                user_id: Some("XX0042".into()),
                exchanges: vec!["NSE".into()],
                products: vec!["CNC".into()],
            })
        }
        async fn trades(&self, _since: Option<chrono::NaiveDate>) -> Result<Vec<Trade>> {
            Ok(vec![])
        }
        async fn rows(&self, endpoint: Endpoint) -> Result<Vec<serde_json::Value>> {
            Ok(match endpoint {
                Endpoint::Holdings => vec![serde_json::json!({
                    "tradingsymbol": "INFY", "quantity": 10, "average_price": 1400,
                    "last_price": 1500, "brand_new_field": 1
                })],
                Endpoint::Profile => vec![serde_json::json!({"user_id": "XX0042"})],
                _ => vec![],
            })
        }
        fn shape(&self, endpoint: Endpoint) -> Option<brokers::Shape> {
            (endpoint == Endpoint::Holdings || endpoint == Endpoint::Profile).then_some(
                brokers::Shape {
                    required: "tradingsymbol|user_id quantity|user_id",
                    documented: "average_price last_price",
                },
            )
        }
        async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument> {
            Ok(Instrument {
                symbol: symbol.into(),
                tradingsymbol: symbol.into(),
                token: "2885".into(),
                exchange: exchange.into(),
            })
        }
        async fn place_order(&self, _o: &OrderRequest) -> Result<OrderAck> {
            self.writes.lock().unwrap().push("place_order");
            Ok(OrderAck::default())
        }
        fn supports_gtt(&self) -> bool {
            true
        }
        async fn gtt_create(&self, _g: &GttRequest) -> Result<String> {
            self.writes.lock().unwrap().push("gtt_create");
            Ok("1".into())
        }
        async fn gtt_list(&self) -> Result<Vec<Gtt>> {
            Ok(vec![])
        }
        async fn gtt_cancel(&self, _id: &str) -> Result<()> {
            self.writes.lock().unwrap().push("gtt_cancel");
            Ok(())
        }
    }

    fn ctx(last: Option<f64>, cap: Option<f64>) -> Context {
        Context {
            status: brokers::ConfigStatus {
                path: "zerodha_config.json".into(),
                exists: true,
                configured: true,
                summary: "app kiteapikey".into(),
                storage: brokers::secrets::Storage::File,
                session: None,
                session_date: chrono::NaiveDate::from_ymd_opt(2026, 10, 25),
                identifiers: vec!["kiteapikey".into(), "XX0042".into()],
            },
            limits: brokers::OrderLimits {
                max_order_value: cap,
                enable_orders: false,
            },
            last_price: last,
            today: chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(),
            market: "closed".into(),
            pause: std::time::Duration::ZERO,
        }
    }

    async fn report(account: &Account, c: Context, redact: bool) -> Report {
        let ro = ReadOnly::new(Box::new(account.clone()));
        run_checks(BrokerKind::Zerodha, Ok(&ro), c, redact).await
    }

    #[test]
    fn ids_are_cut_to_two_characters() {
        assert_eq!(redact_id("XX0042"), "••42");
        assert_eq!(redact_id("A1"), "••");
    }

    #[test]
    fn free_text_loses_ids_emails_tokens_and_long_numbers() {
        let r = Redactor::new(vec!["XX0042".into(), "kiteapikey".into()], false);
        let s = r.text(
            "user XX0042 (xx0042) app kiteapikey mail a.b@example.com order 151220000000000 \
             token eyJhbGciOiJIUzI1NiJ9abcdefghijkl HTTP 403 https://api.kite.trade/x?api_key=k",
        );
        for leaked in [
            "XX0042",
            "xx0042",
            "kiteapikey",
            "a.b@example.com",
            "151220000000000",
            "eyJhbG",
            "api_key=k",
        ] {
            assert!(!s.contains(leaked), "{leaked} leaked: {s}");
        }
        assert!(s.contains("••42") && s.contains("HTTP 403"), "{s}");
        assert!(s.contains("https://api.kite.trade/x"), "{s}");
    }

    #[test]
    fn amounts_can_be_hidden() {
        assert_eq!(Redactor::new(vec![], false).money(Some(12.5)), "₹12.50");
        assert_eq!(Redactor::new(vec![], true).money(Some(12.5)), "₹•••");
    }

    #[tokio::test]
    async fn a_full_run_sends_nothing_and_reports_every_check() {
        let account = Account::default();
        let r = report(&account, ctx(Some(1300.0), Some(5000.0)), false).await;
        assert!(account.writes.lock().unwrap().is_empty());
        let names: Vec<&str> = r.checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "Config",
                "Session",
                "Profile",
                "Holdings",
                "Positions",
                "Order book",
                "Trade book",
                "GTT list",
                "Instruments",
                "Dry-run order"
            ]
        );
        assert_eq!(r.summary.fail, 0, "{:#?}", r.checks);
        assert_eq!(r.account.as_deref(), Some("••42"));
        let dry = r.checks.iter().find(|c| c.name == "Dry-run order").unwrap();
        // 5% under 1300 on a 0.05 tick; within the cap; never sent.
        assert!(dry.detail.contains("LIMIT ₹1235.00"), "{}", dry.detail);
        assert!(
            dry.detail.contains("within max_order_value"),
            "{}",
            dry.detail
        );
        assert!(dry.detail.contains("not sent"));
        // The shape check names the new field but no value.
        let holdings = r
            .shapes
            .iter()
            .find(|s| s.endpoint == Endpoint::Holdings)
            .unwrap();
        assert_eq!(holdings.unknown, ["brand_new_field"]);
        assert_eq!(holdings.status, Status::Pass);
        assert!(r.read_only);
    }

    #[tokio::test]
    async fn markdown_report_has_no_personal_data() {
        let account = Account {
            fail_holdings: true,
            ..Default::default()
        };
        let r = report(&account, ctx(Some(1300.0), None), true).await;
        let md = render_markdown(&r);
        for leaked in ["XX0042", "kiteapikey", "151220000000000", "1235", "1300"] {
            assert!(!md.contains(leaked), "{leaked} leaked:\n{md}");
        }
        assert!(md.contains("❌ fail") && md.contains("••42"), "{md}");
        assert!(md.contains("amounts are hidden"));
        assert_eq!(r.summary.fail, 1);
        // JSON carries the same redacted text.
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("XX0042") && !json.contains("151220000000000"));
    }

    #[tokio::test]
    async fn a_failed_login_skips_the_network_checks() {
        let err = anyhow::anyhow!("Zerodha rejected the saved session (HTTP 403) for XX0042");
        let r = run_checks(BrokerKind::Zerodha, Err(&err), ctx(None, None), false).await;
        assert_eq!(r.checks[1].status, Status::Fail);
        assert!(!r.checks[1].detail.contains("XX0042"));
        assert!(r.checks[2..].iter().all(|c| c.status == Status::Skip));
    }

    #[tokio::test]
    async fn stale_session_warns_and_missing_price_uses_market() {
        let mut c = ctx(None, Some(5000.0));
        c.status.session_date = chrono::NaiveDate::from_ymd_opt(2026, 10, 24);
        let r = report(&Account::default(), c, false).await;
        assert_eq!(r.checks[1].status, Status::Warn);
        let dry = r.checks.iter().find(|c| c.name == "Dry-run order").unwrap();
        assert!(dry.detail.contains("MARKET"), "{}", dry.detail);
        // No price with a cap: the real flow would refuse, and the report says so.
        assert!(dry.detail.contains("would be refused"), "{}", dry.detail);
    }

    #[tokio::test]
    async fn an_order_over_the_cap_is_reported_without_the_cap_amount() {
        let r = report(&Account::default(), ctx(Some(1300.0), Some(1000.0)), false).await;
        let dry = r.checks.iter().find(|c| c.name == "Dry-run order").unwrap();
        assert!(
            dry.detail.contains("over max_order_value"),
            "{}",
            dry.detail
        );
        assert!(!dry.detail.contains("1000"), "{}", dry.detail);
    }

    #[tokio::test]
    async fn place_live_order_on_a_read_only_session_sends_nothing() {
        use super::super::trade::{Guard, OrderOpts, place_live_order};
        let account = Account::default();
        let ro = ReadOnly::new(Box::new(account.clone()));
        let yes = |_: &str| -> Result<bool> { Ok(true) };
        let guard = Guard {
            limits: brokers::OrderLimits {
                // --yes needs a cap; the read-only wrapper must still refuse.
                max_order_value: Some(50_000.0),
                enable_orders: true,
            },
            last_price: Some(1300.0),
            confirm: &yes,
        };
        let opts = OrderOpts {
            yes: true,
            dry_run: false,
        };
        let err = place_live_order(
            &ro,
            Side::Buy,
            "RELIANCE",
            1,
            Intent::Order(OrderKind::Market),
            opts,
            &guard,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("read-only"), "{err}");
        assert!(account.writes.lock().unwrap().is_empty());
    }
}
