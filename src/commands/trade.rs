//! Live trading through a broker — Angel One, or the experimental Zerodha
//! Kite, Upstox, Dhan and Fyers integrations — with order previews and
//! confirmation. Every order goes through [`place_live_order`].

use super::*;
use crate::brokers::{
    self, Broker, BrokerKind, EXPERIMENTAL_NOTE, Gtt, GttRequest, OrderKind, OrderLimits,
    OrderRequest, Side,
};

/// Safety switches for commands that place live orders.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrderOpts {
    /// Skip the interactive confirmation (required when stdin is not a terminal).
    pub yes: bool,
    /// Resolve and preview the order, but never send it.
    pub dry_run: bool,
}

/// A `trade` subcommand, as parsed in `main.rs`.
#[derive(Debug, Clone)]
pub enum TradeAction {
    Setup,
    Config {
        /// Move secrets into the OS keyring now.
        keyring: bool,
    },
    Login,
    Holdings,
    Positions,
    Orders,
    /// MARKET, LIMIT, SL or SL-M, depending on which prices are given.
    Order {
        side: Side,
        symbol: String,
        qty: u32,
        limit: Option<f64>,
        trigger: Option<f64>,
        opts: OrderOpts,
    },
    GttCreate {
        side: Side,
        symbol: String,
        qty: u32,
        trigger: f64,
        limit: f64,
        opts: OrderOpts,
    },
    GttList {
        /// Send a notification for GTTs that triggered, were cancelled or
        /// went away since the last look.
        notify: bool,
        format: Format,
    },
    GttCancel {
        id: String,
        opts: OrderOpts,
    },
    /// Read-only verification against the user's own account.
    Selftest {
        redact_amounts: bool,
        format: Format,
    },
    /// Trade book and GTT status into the portfolio.
    Sync {
        /// `YYYY-MM-DD`: ignore older fills (and fetch history where the
        /// broker keeps it).
        since: Option<String>,
        /// Write the portfolio; without it, preview only.
        apply: bool,
        notify: bool,
        format: Format,
    },
}

/// What to send once the preview is confirmed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Intent {
    Order(OrderKind),
    /// Single-leg GTT: a LIMIT order at `limit` once `trigger` trades.
    Gtt {
        trigger: f64,
        limit: f64,
    },
}

/// Inputs to the safety flow besides the broker session, so tests can
/// supply them without a terminal or network.
pub(super) struct Guard<'a> {
    pub limits: OrderLimits,
    /// Last traded price (Yahoo), used for the value estimate and trigger checks.
    pub last_price: Option<f64>,
    /// Ask the user to type `yes`; the argument completes "Type yes to ...".
    pub confirm: &'a dyn Fn(&str) -> Result<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Outcome {
    DryRun,
    Cancelled,
    /// Sent; the broker's order or GTT id when it returned one.
    Sent(Option<String>),
}

/// Checks that need no network: run before logging in.
fn check_args(qty: u32, intent: &Intent) -> Result<()> {
    if qty == 0 {
        bail!("Quantity must be at least 1");
    }
    let finite = |name: &str, v: f64| {
        if v.is_finite() && v > 0.0 {
            brokers::check_paise(name, v)
        } else {
            Err(anyhow::anyhow!("{name} must be a positive number"))
        }
    };
    match intent {
        Intent::Order(kind) => {
            if let Some(p) = kind.price() {
                finite("Limit price", p)?;
            }
            if let Some(t) = kind.trigger() {
                finite("Trigger price", t)?;
            }
        }
        Intent::Gtt { trigger, limit } => {
            finite("Trigger price", *trigger)?;
            finite("Limit price", *limit)?;
        }
    }
    Ok(())
}

fn type_line(intent: &Intent) -> String {
    match *intent {
        Intent::Order(OrderKind::Market) => "MARKET".into(),
        Intent::Order(OrderKind::Limit { price }) => format!("LIMIT @ ₹{:.2}", price),
        Intent::Order(OrderKind::StopLimit { trigger, price }) => format!(
            "SL (stop-loss limit): trigger ₹{:.2}, then LIMIT @ ₹{:.2}",
            trigger, price
        ),
        Intent::Order(OrderKind::StopMarket { trigger }) => format!(
            "SL-M (stop-loss market): trigger ₹{:.2}, then MARKET",
            trigger
        ),
        Intent::Gtt { trigger, limit } => format!(
            "GTT: when the price reaches ₹{:.2}, LIMIT @ ₹{:.2} (valid up to a year)",
            trigger, limit
        ),
    }
}

/// What a live action is, for the opt-in check. `None` = cancelling a GTT.
fn feature(intent: Option<&Intent>) -> &'static str {
    match intent {
        Some(Intent::Order(OrderKind::Market | OrderKind::Limit { .. })) => "Live orders",
        Some(Intent::Order(_)) => "Stop-loss (SL / SL-M) orders",
        Some(Intent::Gtt { .. }) | None => "GTT orders",
    }
}

/// Paths never run against a live account: every order on an experimental
/// broker, and stop-loss / GTT orders on Angel One. Market and limit orders
/// on Angel One keep their 0.3 behaviour.
fn untested(kind: BrokerKind, intent: Option<&Intent>) -> bool {
    kind.experimental() || feature(intent) != "Live orders"
}

fn print_broker_line(kind: BrokerKind, intent: Option<&Intent>) {
    if untested(kind, intent) {
        let note = if kind.experimental() {
            format!("{} — {}", kind.label(), EXPERIMENTAL_NOTE)
        } else {
            format!(
                "{} — {} here are {}",
                kind.label(),
                feature(intent).to_lowercase(),
                EXPERIMENTAL_NOTE
            )
        };
        print_kv("Broker", &note.yellow().to_string());
    } else {
        print_kv("Broker", kind.label());
    }
}

/// Refuse a live (non-dry-run) action on an untested path until the user
/// opts in through the broker's config file.
fn check_orders_enabled(
    kind: BrokerKind,
    intent: Option<&Intent>,
    limits: &OrderLimits,
) -> Result<()> {
    if !untested(kind, intent) || limits.enable_orders {
        return Ok(());
    }
    let file = crate::store::path(kind.config_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| kind.config_file().into());
    bail!(
        "{} through {} are {}. Check this preview against the {} app, then add \
         \"enable_orders\": true to {} to allow them. --dry-run always works.",
        feature(intent),
        kind.label(),
        EXPERIMENTAL_NOTE,
        kind.label(),
        file
    )
}

/// An order or GTT that passed every check that comes before the preview.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Prepared {
    pub instrument: brokers::Instrument,
    /// Estimated value for the preview and the `max_order_value` cap.
    pub est_value: Option<f64>,
}

/// The checks before the preview: arguments, GTT support, the exact
/// instrument, and trigger / limit against the side and the last price.
/// Sends nothing. Shared by [`place_live_order`] and `trade selftest`.
pub(super) async fn prepare_order(
    broker: &dyn Broker,
    side: Side,
    symbol: &str,
    qty: u32,
    intent: Intent,
    last: Option<f64>,
) -> Result<Prepared> {
    check_args(qty, &intent)?;
    let last = usable_price(last);
    if matches!(intent, Intent::Gtt { .. }) && !broker.supports_gtt() {
        bail!("{} GTT orders are not supported yet", broker.kind().label());
    }
    let sym = brokers::nse_symbol(symbol)?;
    let instrument = broker.resolve_instrument(&sym, "NSE").await?;

    // Trigger checks need the market price; run them before the preview so a
    // refused order never looks ready to send.
    let est_value = match intent {
        Intent::Order(k) => {
            brokers::validate_order(side, &k, last)?;
            brokers::estimate_value(&k, qty, last)
        }
        Intent::Gtt { trigger, limit } => {
            brokers::validate_gtt(side, trigger, limit, last)?;
            Some(limit * qty as f64)
        }
    };
    Ok(Prepared {
        instrument,
        est_value,
    })
}

/// Refuse an order whose estimated value is above the broker's
/// `max_order_value`, or unknown while a cap is set.
pub(super) fn check_cap(kind: BrokerKind, cap: Option<f64>, est_value: Option<f64>) -> Result<()> {
    let Some(cap) = cap else {
        return Ok(());
    };
    match est_value {
        Some(v) if v <= cap => Ok(()),
        Some(v) => bail!(
            "Estimated value ₹{:.2} exceeds max_order_value ₹{:.2} in your {} config",
            v,
            cap,
            kind.label()
        ),
        None => bail!(
            "Cannot estimate the order value, so max_order_value (₹{:.2}) cannot be checked. Use a limit order.",
            cap
        ),
    }
}

/// An order sent with `--yes` has no typed confirmation, so the value cap is
/// the only guard left against a wrong quantity. Refuse it unless the
/// broker's config sets `max_order_value`. `--dry-run` places nothing and is
/// exempt.
pub(super) fn require_cap_for_unattended(
    kind: BrokerKind,
    opts: OrderOpts,
    limits: &OrderLimits,
) -> Result<()> {
    if !opts.yes || opts.dry_run || limits.max_order_value.is_some() {
        return Ok(());
    }
    let file = crate::store::path(kind.config_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| kind.config_file().into());
    bail!(
        "--yes skips the typed confirmation, so it needs a cap on the order value. Add \
         \"max_order_value\": 50000 to {file} (your {} config), or leave out --yes and \
         confirm by typing yes. --dry-run always works.",
        kind.label()
    )
}

/// Resolve, validate, preview, cap-check, confirm, and send one live order
/// or GTT. Each guard exists because a typo in a shell command could
/// otherwise spend real money. This is the only caller of
/// [`Broker::place_order`] and [`Broker::gtt_create`].
pub(super) async fn place_live_order(
    broker: &dyn Broker,
    side: Side,
    symbol: &str,
    qty: u32,
    intent: Intent,
    opts: OrderOpts,
    guard: &Guard<'_>,
) -> Result<Outcome> {
    let kind = broker.kind();
    let last = usable_price(guard.last_price);
    let Prepared {
        instrument,
        est_value,
    } = prepare_order(broker, side, symbol, qty, intent, last).await?;

    let title = match intent {
        Intent::Order(_) => format!("{} order — REAL MONEY", side.as_str()),
        Intent::Gtt { .. } => format!("{} GTT — REAL MONEY", side.as_str()),
    };
    print_header(&title);
    print_broker_line(kind, Some(&intent));
    print_kv(
        "Instrument",
        &format!(
            "{} ({}, token {})",
            instrument.tradingsymbol, instrument.exchange, instrument.token
        ),
    );
    print_kv("Side", side.as_str());
    print_kv("Quantity", &qty.to_string());
    print_kv("Type", &type_line(&intent));
    print_kv("Product", "Delivery (CNC)");
    if let Some(l) = last {
        print_kv("Last price", &format!("₹{:.2}", l));
    }
    print_kv(
        "Est. value",
        &est_value.map_or("unknown".to_string(), |v| format!("₹{:.2}", v)),
    );
    println!();

    check_cap(kind, guard.limits.max_order_value, est_value)?;
    if guard.limits.max_order_value.is_none() && !opts.yes {
        println!(
            "  {}",
            "No max_order_value set in your broker config: add one to cap any single order."
                .dimmed()
        );
    }

    if opts.dry_run {
        println!("  {} Dry run — no order sent.\n", "✓".green());
        return Ok(Outcome::DryRun);
    }
    check_orders_enabled(kind, Some(&intent), &guard.limits)?;
    require_cap_for_unattended(kind, opts, &guard.limits)?;
    let what = match intent {
        Intent::Order(_) => "place this order",
        Intent::Gtt { .. } => "create this GTT",
    };
    if !opts.yes && !(guard.confirm)(what)? {
        println!("  Cancelled — no order sent.\n");
        return Ok(Outcome::Cancelled);
    }

    let id = match intent {
        Intent::Order(k) => {
            broker
                .place_order(&OrderRequest {
                    instrument,
                    side,
                    qty,
                    kind: k,
                })
                .await?
                .order_id
        }
        Intent::Gtt { trigger, limit } => Some(
            broker
                .gtt_create(&GttRequest {
                    instrument,
                    side,
                    qty,
                    trigger,
                    limit,
                    // validate_gtt refused a missing last price.
                    last_price: last.unwrap_or(trigger),
                })
                .await?,
        ),
    };

    let noun = match intent {
        Intent::Order(_) => "Order",
        Intent::Gtt { .. } => "GTT",
    };
    match &id {
        Some(id) => println!(
            "  {} {} placed! ID: {}",
            "✓".green().bold(),
            noun,
            id.cyan()
        ),
        None => println!(
            "  {} {} accepted but no id was returned — {}",
            "⚠".yellow().bold(),
            noun,
            "do NOT resubmit; verify with `stockwise trade orders`.".yellow()
        ),
    }
    println!();
    Ok(Outcome::Sent(id))
}

/// Preview and confirm cancelling one GTT, through the same guards as an
/// order (cancelling a stop-loss GTT can leave a position unprotected).
pub(super) async fn cancel_gtt(
    broker: &dyn Broker,
    id: &str,
    opts: OrderOpts,
    guard: &Guard<'_>,
) -> Result<Outcome> {
    let kind = broker.kind();
    if !broker.supports_gtt() {
        bail!("{} GTT orders are not supported yet", kind.label());
    }
    let gtts = broker.gtt_list().await?;
    let Some(g) = gtts.iter().find(|g| g.id == id) else {
        bail!(
            "No {} GTT with id {} — see `stockwise trade gtt list`",
            kind.label(),
            id
        );
    };
    print_header("Cancel GTT");
    print_broker_line(kind, None);
    print_gtt_kv(g);
    println!();
    if opts.dry_run {
        println!("  {} Dry run — nothing cancelled.\n", "✓".green());
        return Ok(Outcome::DryRun);
    }
    check_orders_enabled(kind, None, &guard.limits)?;
    if !opts.yes && !(guard.confirm)("cancel this GTT")? {
        println!("  Kept — nothing cancelled.\n");
        return Ok(Outcome::Cancelled);
    }
    broker.gtt_cancel(id).await?;
    println!("  {} GTT {} cancelled.\n", "✓".green().bold(), id.cyan());
    Ok(Outcome::Sent(Some(id.to_string())))
}

fn print_gtt_kv(g: &Gtt) {
    print_kv("GTT id", &g.id);
    print_kv("Instrument", &g.symbol);
    print_kv("Side", g.side.as_deref().unwrap_or("?"));
    print_kv("Quantity", &g.qty.map_or("?".into(), |q| q.to_string()));
    print_kv("Trigger", &money(g.trigger));
    print_kv("Limit", &money(g.limit));
    print_kv("Status", g.status.as_deref().unwrap_or("?"));
}

fn money(v: Option<f64>) -> String {
    v.map_or("—".to_string(), |p| format!("₹{:.2}", p))
}

/// Ask the user to type `yes`. Refuses (errors) when stdin is not a terminal,
/// so scripts must opt in with `--yes`.
pub(super) fn confirm_live_order(what: &str) -> Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!(
            "Refusing to place a live order without confirmation. Run it in a terminal, or pass --yes."
        );
    }
    print!(
        "  Type {} to {} (anything else cancels): ",
        "yes".bold(),
        what
    );
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim() == "yes")
}

/// A price an order check can rely on: finite and above zero. A halted or
/// broken quote of `0.0` (or NaN) is "no price", so a market order cannot be
/// valued at nothing and slip under `max_order_value`.
pub(super) fn usable_price(p: Option<f64>) -> Option<f64> {
    p.filter(|p| p.is_finite() && *p > 0.0)
}

/// Last NSE price from Yahoo, or None.
pub(super) async fn last_price(symbol: &str) -> Option<f64> {
    let y = YahooClient::new().await.ok()?;
    last_price_with(&y, symbol).await
}

async fn last_price_with(y: &YahooClient, symbol: &str) -> Option<f64> {
    let yahoo_sym = format!("{}.NS", brokers::plain_symbol(symbol));
    let quotes = y.get_quote(&[yahoo_sym.as_str()]).await.ok()?;
    usable_price(quotes.first()?.regular_market_price)
}

/// Log in, look up the price, and run the safety flow for one order or GTT.
async fn run_order(
    kind: BrokerKind,
    side: Side,
    symbol: &str,
    qty: u32,
    intent: Intent,
    opts: OrderOpts,
) -> Result<()> {
    check_args(qty, &intent)?;
    brokers::nse_symbol(symbol)?;
    let limits = brokers::order_limits(kind)?;
    // Before logging in: an unattended order without a cap is refused
    // without touching the broker.
    require_cap_for_unattended(kind, opts, &limits)?;
    let broker = brokers::connect(kind).await?;
    let guard = Guard {
        limits,
        last_price: last_price(symbol).await,
        confirm: &confirm_live_order,
    };
    place_live_order(broker.as_ref(), side, symbol, qty, intent, opts, &guard).await?;
    Ok(())
}

/// Broker orders go to NSE cash equity only; refuse other markets instead of
/// guessing what `-m us` / `-m crypto` was meant to buy.
fn check_market(market: Market) -> Result<()> {
    let (name, flag) = match market {
        Market::In => return Ok(()),
        Market::Us => ("US", "us"),
        Market::Crypto => ("crypto", "crypto"),
        Market::Fx => ("forex", "fx"),
        Market::Commodity => ("commodity", "commodity"),
        Market::Global(x) => (x.info().country, x.info().code),
    };
    bail!(
        "`trade` places NSE equity orders only and can't trade the {name} market. \
         Run it without -m {flag} (or STOCKWISE_MARKET={flag})"
    )
}

pub async fn cmd_trade(kind: BrokerKind, action: TradeAction, market: Market) -> Result<()> {
    if matches!(
        action,
        TradeAction::Order { .. } | TradeAction::GttCreate { .. }
    ) {
        check_market(market)?;
    }
    match action {
        TradeAction::Setup => {
            print_setup(kind);
            Ok(())
        }
        TradeAction::Config { keyring } => cmd_trade_config(kind, keyring),
        TradeAction::Login => brokers::login(kind).await,
        TradeAction::Holdings => {
            let broker = brokers::connect(kind).await?;
            print_holdings(kind, &broker.holdings().await?).await
        }
        TradeAction::Positions => {
            let broker = brokers::connect(kind).await?;
            print_positions(kind, &broker.positions().await?);
            Ok(())
        }
        TradeAction::Orders => {
            let broker = brokers::connect(kind).await?;
            print_orders(kind, &broker.orders().await?);
            Ok(())
        }
        TradeAction::Order {
            side,
            symbol,
            qty,
            limit,
            trigger,
            opts,
        } => {
            let intent = Intent::Order(OrderKind::from_prices(limit, trigger));
            run_order(kind, side, &symbol, qty, intent, opts).await
        }
        TradeAction::GttCreate {
            side,
            symbol,
            qty,
            trigger,
            limit,
            opts,
        } => {
            run_order(
                kind,
                side,
                &symbol,
                qty,
                Intent::Gtt { trigger, limit },
                opts,
            )
            .await
        }
        TradeAction::GttList { notify, format } => {
            super::cmd_trade_gtt_list(kind, notify, format).await
        }
        TradeAction::GttCancel { id, opts } => {
            let limits = brokers::order_limits(kind)?;
            let broker = brokers::connect(kind).await?;
            let guard = Guard {
                limits,
                last_price: None,
                confirm: &confirm_live_order,
            };
            cancel_gtt(broker.as_ref(), &id, opts, &guard).await?;
            Ok(())
        }
        TradeAction::Selftest {
            redact_amounts,
            format,
        } => super::cmd_trade_selftest(kind, redact_amounts, format).await,
        TradeAction::Sync {
            since,
            apply,
            notify,
            format,
        } => super::cmd_trade_sync(kind, since, apply, notify, format).await,
    }
}

fn print_setup(kind: BrokerKind) {
    let path = crate::store::path(kind.config_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| kind.config_file().into());
    let flag = if kind == BrokerKind::Angel {
        String::new()
    } else {
        format!(" --broker {}", kind.id())
    };
    println!();
    print_header(&format!("{} setup", kind.display()));
    if kind.experimental() {
        println!(
            "  {} {} Built from {}'s public API docs.",
            "⚠".yellow(),
            "Experimental: untested against a live account.".yellow(),
            kind.label()
        );
        println!(
            "    Live orders stay off until you add {} to the config file.",
            "\"enable_orders\": true".bold()
        );
        println!();
    }
    println!("  To connect your {} account:", kind.label());
    println!();
    for (i, step) in brokers::setup_steps(kind).iter().enumerate() {
        println!("  {}. {}", i + 1, step);
    }
    println!();
    println!("  Then run:");
    println!("    {} stockwise trade config{}", "$".dimmed(), flag);
    println!("  and fill in the file it creates:");
    println!("    {}", path.cyan());
    if kind != BrokerKind::Angel {
        println!("  and log in (sessions expire daily):");
        println!("    {} stockwise trade login{}", "$".dimmed(), flag);
    }
    println!();
    println!(
        "  {}",
        "Never pass credentials as command-line arguments — they end up in shell history.".dimmed()
    );
    println!(
        "  {}",
        format!(
            "Secrets move into the OS keyring on first use when one is available; otherwise the \
             file stays readable only by you. It is sent nowhere except {}'s API.",
            kind.label()
        )
        .dimmed()
    );
    println!(
        "  {}",
        "Optional: add \"max_order_value\": 50000 to cap the value of any single order.".dimmed()
    );
    println!();
}

fn cmd_trade_config(kind: BrokerKind, keyring: bool) -> Result<()> {
    let flag = if kind == BrokerKind::Angel {
        String::new()
    } else {
        format!(" --broker {}", kind.id())
    };
    let backend = brokers::secrets::backend();
    let status = brokers::config_status(kind, true)?;
    let path = status.path.display().to_string();
    println!();
    if !status.exists {
        brokers::write_template(kind)?;
        println!(
            "  {} Created a {} credentials template (owner-only access).",
            "✓".green(),
            kind.label()
        );
        println!("  Edit it, then run this command again:");
        println!("    {}", path.cyan());
        println!();
        return Ok(());
    }
    if keyring {
        if status.storage == brokers::secrets::Storage::File {
            let storage =
                brokers::secrets::migrate_file(&status.path, kind.secret_keys(), backend)?;
            if let brokers::secrets::Storage::Keyring { .. } = storage {
                println!(
                    "  {} {} secrets are now in the {}.",
                    "✓".green(),
                    kind.label(),
                    backend.name()
                );
            } else {
                println!(
                    "  {} No secrets to move yet — fill in {} first.",
                    "!".yellow(),
                    path
                );
            }
        } else {
            println!(
                "  {} {} secrets are already in the {}.",
                "✓".green(),
                kind.label(),
                backend.name()
            );
        }
    }
    let status = brokers::config_status(kind, false)?;
    if status.configured {
        println!(
            "  {} {} configured for {} — {}",
            "✓".green(),
            kind.display(),
            status.summary.cyan(),
            path
        );
    } else {
        println!(
            "  {} Credentials file still has placeholder or empty values.",
            "⚠".yellow()
        );
        println!("  Edit it:");
        println!("    {}", path.cyan());
    }
    println!(
        "  {} {}",
        "Storage:".dimmed(),
        brokers::secrets::describe(&status.storage, backend)
    );
    if let Some(s) = &status.session {
        println!("  {} {}", "Session:".dimmed(), s);
    } else if kind != BrokerKind::Angel && status.configured {
        println!(
            "  {} not logged in — run `stockwise trade login{}`",
            "Session:".dimmed(),
            flag
        );
    }
    println!();
    Ok(())
}

fn signed_money(v: f64) -> String {
    if v >= 0.0 {
        format!("+₹{:.2}", v).green().to_string()
    } else {
        format!("-₹{:.2}", v.abs()).red().to_string()
    }
}

async fn print_holdings(kind: BrokerKind, holdings: &[brokers::Holding]) -> Result<()> {
    if holdings.is_empty() {
        println!("\n  {}", "No holdings found.".dimmed());
        return Ok(());
    }

    print_header(&format!("{} — Holdings (Live)", kind.display()));
    println!(
        "  {:<14} {:>8} {:>10} {:>10} {:>12} {:>10}",
        "Symbol".bold(),
        "Qty".bold(),
        "Avg Cost".bold(),
        "LTP".bold(),
        "P&L".bold(),
        "P&L %".bold()
    );
    println!("  {}", "─".repeat(68).dimmed());

    let mut total_pnl = 0.0_f64;
    let mut total_value = 0.0_f64;
    let mut total_cost = 0.0_f64;
    let mut unpriced = 0;

    for h in holdings {
        let cost = h.avg_price.map(|a| a * h.qty);
        // P&L from the broker, else from LTP; never from a missing price.
        let pnl = h
            .pnl
            .or_else(|| Some(h.ltp? * h.qty - cost?))
            .filter(|_| h.ltp.is_some());
        let pnl_pct = match (pnl, cost) {
            (Some(p), Some(c)) if c > 0.0 => Some(p / c * 100.0),
            _ => None,
        };
        match (h.ltp, cost) {
            (Some(ltp), Some(c)) => {
                total_value += ltp * h.qty;
                total_cost += c;
                total_pnl += pnl.unwrap_or(ltp * h.qty - c);
            }
            _ => unpriced += 1,
        }
        let pct_str = match pnl_pct {
            Some(p) if p >= 0.0 => format!("+{:.2}%", p).green().to_string(),
            Some(p) => format!("{:.2}%", p).red().to_string(),
            None => "N/A".dimmed().to_string(),
        };
        println!(
            "  {:<14} {:>8} {:>10} {:>10} {:>12} {:>10}",
            h.symbol.cyan(),
            h.qty,
            money(h.avg_price),
            money(h.ltp),
            pnl.map_or("N/A".dimmed().to_string(), signed_money),
            pct_str
        );
    }

    println!("  {}", "─".repeat(68).dimmed());
    println!(
        "  {:<14} {:>8} {:>10} {:>10} {:>12}",
        "TOTAL".bold(),
        "",
        format!("₹{:.0}", total_cost),
        format!("₹{:.0}", total_value),
        signed_money(total_pnl).bold()
    );
    if unpriced > 0 {
        println!(
            "  {}",
            format!("{unpriced} holding(s) without a price are left out of the totals.").dimmed()
        );
    }

    // Also sync to stockwise portfolio.
    let portfolio = Portfolio::load()?;
    let snapshot: Vec<(String, f64, f64)> = holdings
        .iter()
        .filter_map(|h| {
            let avg = h.avg_price?;
            (!h.symbol.is_empty() && h.qty > 0.0).then(|| {
                // Broker symbols are plain (RELIANCE); Yahoo wants RELIANCE.NS.
                let resolved = if h.symbol.contains('.') {
                    h.symbol.clone()
                } else {
                    format!("{}.NS", h.symbol)
                };
                (resolved, h.qty, avg)
            })
        })
        .collect();
    // More shares at the broker may just be a split or bonus not yet
    // applied to the lots; syncing those would count them twice.
    let wanted: Vec<(String, f64)> = snapshot.iter().map(|(s, q, _)| (s.clone(), *q)).collect();
    let blocked = super::portfolio::blocked_by_pending_splits(&portfolio, &wanted).await;
    let source = format!("broker:{}", kind.id());
    // The checks above ran on the portfolio as loaded: sync the file as it is
    // now, under a lock.
    let reduced = Portfolio::update(|p| {
        let mut reduced = Vec::new();
        for (resolved, qty, avg) in &snapshot {
            if blocked.iter().any(|(s, _)| s == resolved) {
                continue;
            }
            // Keep existing lots and their purchase dates; only the quantity
            // follows the broker.
            if let crate::portfolio::SyncChange::Reduced(n) =
                p.sync_quantity(resolved, *qty, *avg, &source)
            {
                reduced.push(format!("{} (-{})", resolved, n));
            }
        }
        Ok(reduced)
    })?;
    println!(
        "\n  {} Holdings synced to StockWise portfolio.",
        "✓".green()
    );
    for (sym, reason) in &blocked {
        println!("  {} {} not synced: {}", "!".yellow(), sym, reason);
    }
    if !reduced.is_empty() {
        println!(
            "  {} Fewer shares than StockWise had: {} — taken from the oldest lots, not \
             recorded as sales. To keep a sale's P&L for `tax`, record it with \
             `stockwise portfolio sell` before syncing.",
            "!".yellow(),
            reduced.join(", ")
        );
    }
    println!();
    Ok(())
}

fn print_positions(kind: BrokerKind, positions: &[brokers::Position]) {
    if positions.is_empty() {
        println!("\n  {}", "No open positions.".dimmed());
        return;
    }

    print_header(&format!("{} — Open Positions", kind.display()));
    println!(
        "  {:<14} {:>8} {:>10} {:>10} {:>12}",
        "Symbol".bold(),
        "Qty".bold(),
        "Avg".bold(),
        "LTP".bold(),
        "P&L".bold()
    );
    println!("  {}", "─".repeat(58).dimmed());

    for p in positions {
        println!(
            "  {:<14} {:>8} {:>10} {:>10} {:>12}",
            p.symbol.cyan(),
            p.qty,
            money(p.avg_price),
            money(p.ltp),
            p.pnl.map_or("N/A".dimmed().to_string(), signed_money)
        );
    }
    println!();
}

fn print_orders(kind: BrokerKind, orders: &[brokers::Order]) {
    if orders.is_empty() {
        println!("\n  {}", "No orders today.".dimmed());
        return;
    }

    print_header(&format!("{} — Order Book", kind.display()));
    println!(
        "  {:<18} {:<14} {:>5} {:>7} {:>8} {:>10} {:>10} {:>12}",
        "Order ID".bold(),
        "Symbol".bold(),
        "Side".bold(),
        "Type".bold(),
        "Qty".bold(),
        "Price".bold(),
        "Trigger".bold(),
        "Status".bold()
    );
    println!("  {}", "─".repeat(90).dimmed());

    for o in orders {
        let side = o.side.as_deref().unwrap_or("?");
        let side_str = match side.to_ascii_uppercase().as_str() {
            "BUY" => side.green().to_string(),
            "SELL" => side.red().to_string(),
            _ => side.to_string(),
        };
        let status = o.status.as_deref().unwrap_or("???");
        let status_str = match status.to_ascii_lowercase().as_str() {
            "complete" | "completed" | "traded" | "filled" => status.green().bold().to_string(),
            "rejected" | "cancelled" | "canceled" => status.red().to_string(),
            "open" | "pending" | "trigger pending" | "transit" => status.yellow().to_string(),
            _ => status.to_string(),
        };
        println!(
            "  {:<18} {:<14} {:>5} {:>7} {:>8} {:>10} {:>10} {:>12}",
            o.id.dimmed(),
            o.symbol.cyan(),
            side_str,
            o.order_type.as_deref().unwrap_or(""),
            o.qty.map_or("?".into(), |q| q.to_string()),
            money(o.price),
            money(o.trigger),
            status_str
        );
        if let Some(msg) = &o.message
            && status.eq_ignore_ascii_case("rejected")
        {
            println!("  {:<18} {}", "", msg.dimmed());
        }
    }
    println!();
}

pub(super) fn print_gtts(kind: BrokerKind, gtts: &[Gtt]) {
    if gtts.is_empty() {
        println!("\n  {}", "No GTT orders.".dimmed());
        return;
    }
    print_header(&format!("{} — GTT orders", kind.display()));
    println!(
        "  {:<14} {:<14} {:>5} {:>8} {:>10} {:>10} {:>12}",
        "GTT id".bold(),
        "Symbol".bold(),
        "Side".bold(),
        "Qty".bold(),
        "Trigger".bold(),
        "Limit".bold(),
        "Status".bold()
    );
    println!("  {}", "─".repeat(80).dimmed());
    for g in gtts {
        println!(
            "  {:<14} {:<14} {:>5} {:>8} {:>10} {:>10} {:>12}",
            g.id.dimmed(),
            g.symbol.cyan(),
            g.side.as_deref().unwrap_or("?"),
            g.qty.map_or("?".into(), |q| q.to_string()),
            money(g.trigger),
            money(g.limit),
            g.status.as_deref().unwrap_or("?")
        );
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::{Holding, Instrument, Order, OrderAck, Position};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// In-memory broker that records what the safety flow sends.
    #[derive(Default)]
    struct FakeBroker {
        experimental: bool,
        placed: Mutex<Vec<OrderRequest>>,
        gtts: Mutex<Vec<GttRequest>>,
        cancelled: Mutex<Vec<String>>,
        resolve_fails: bool,
        no_gtt: bool,
    }

    #[async_trait]
    impl Broker for FakeBroker {
        fn kind(&self) -> BrokerKind {
            if self.experimental {
                BrokerKind::Zerodha
            } else {
                BrokerKind::Angel
            }
        }
        async fn holdings(&self) -> Result<Vec<Holding>> {
            Ok(vec![])
        }
        async fn positions(&self) -> Result<Vec<Position>> {
            Ok(vec![])
        }
        async fn orders(&self) -> Result<Vec<Order>> {
            Ok(vec![])
        }
        async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument> {
            if self.resolve_fails {
                bail!("No exact match for {symbol}");
            }
            Ok(Instrument {
                symbol: symbol.into(),
                tradingsymbol: format!("{symbol}-EQ"),
                token: "1".into(),
                exchange: exchange.into(),
            })
        }
        async fn place_order(&self, order: &OrderRequest) -> Result<OrderAck> {
            self.placed.lock().unwrap().push(order.clone());
            Ok(OrderAck {
                order_id: Some("OID1".into()),
            })
        }
        fn supports_gtt(&self) -> bool {
            !self.no_gtt
        }
        async fn gtt_create(&self, gtt: &GttRequest) -> Result<String> {
            self.gtts.lock().unwrap().push(gtt.clone());
            Ok("G1".into())
        }
        async fn gtt_list(&self) -> Result<Vec<Gtt>> {
            Ok(vec![Gtt {
                id: "G1".into(),
                symbol: "SBIN".into(),
                side: Some("SELL".into()),
                qty: Some(10.0),
                trigger: Some(780.0),
                limit: Some(779.0),
                status: Some("active".into()),
            }])
        }
        async fn gtt_cancel(&self, id: &str) -> Result<()> {
            self.cancelled.lock().unwrap().push(id.into());
            Ok(())
        }
    }

    impl FakeBroker {
        fn sent(&self) -> usize {
            self.placed.lock().unwrap().len()
                + self.gtts.lock().unwrap().len()
                + self.cancelled.lock().unwrap().len()
        }
    }

    fn never_confirm(_: &str) -> Result<bool> {
        panic!("confirmation must not be asked")
    }
    fn say_yes(_: &str) -> Result<bool> {
        Ok(true)
    }
    fn say_no(_: &str) -> Result<bool> {
        Ok(false)
    }

    fn guard<'a>(
        last: Option<f64>,
        cap: Option<f64>,
        confirm: &'a dyn Fn(&str) -> Result<bool>,
    ) -> Guard<'a> {
        Guard {
            limits: OrderLimits {
                max_order_value: cap,
                enable_orders: true,
            },
            last_price: last,
            confirm,
        }
    }

    const LIVE: OrderOpts = OrderOpts {
        yes: false,
        dry_run: false,
    };
    const DRY: OrderOpts = OrderOpts {
        yes: false,
        dry_run: true,
    };
    const YES: OrderOpts = OrderOpts {
        yes: true,
        dry_run: false,
    };

    #[tokio::test]
    async fn dry_run_never_sends_an_order_or_gtt() {
        let b = FakeBroker::default();
        let g = guard(Some(800.0), None, &never_confirm);
        let sl = Intent::Order(OrderKind::StopMarket { trigger: 780.0 });
        let out = place_live_order(&b, Side::Sell, "SBIN", 10, sl, DRY, &g).await;
        assert_eq!(out.unwrap(), Outcome::DryRun);
        let gtt = Intent::Gtt {
            trigger: 780.0,
            limit: 779.0,
        };
        let out = place_live_order(&b, Side::Sell, "SBIN", 10, gtt, DRY, &g).await;
        assert_eq!(out.unwrap(), Outcome::DryRun);
        let out = cancel_gtt(&b, "G1", DRY, &g).await;
        assert_eq!(out.unwrap(), Outcome::DryRun);
        assert_eq!(b.sent(), 0);
    }

    #[tokio::test]
    async fn typed_yes_is_required() {
        let b = FakeBroker::default();
        let no = guard(Some(800.0), None, &say_no);
        let out = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1,
            Intent::Order(OrderKind::Market),
            LIVE,
            &no,
        )
        .await;
        assert_eq!(out.unwrap(), Outcome::Cancelled);
        assert_eq!(b.sent(), 0);

        let yes = guard(Some(800.0), None, &say_yes);
        let out = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1,
            Intent::Order(OrderKind::Market),
            LIVE,
            &yes,
        )
        .await;
        assert_eq!(out.unwrap(), Outcome::Sent(Some("OID1".into())));
        assert_eq!(
            b.placed.lock().unwrap()[0].instrument.tradingsymbol,
            "SBIN-EQ"
        );
    }

    #[tokio::test]
    async fn yes_flag_skips_the_prompt_but_not_the_cap() {
        let b = FakeBroker::default();
        let g = guard(Some(800.0), Some(5000.0), &never_confirm);
        // 10 x 800 = 8000 > 5000.
        let err = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            10,
            Intent::Order(OrderKind::Market),
            YES,
            &g,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("max_order_value"), "{err}");
        // Dry run reports the cap too.
        assert!(
            place_live_order(
                &b,
                Side::Buy,
                "SBIN",
                10,
                Intent::Order(OrderKind::Market),
                DRY,
                &g
            )
            .await
            .is_err()
        );
        let out = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            6,
            Intent::Order(OrderKind::Market),
            YES,
            &g,
        )
        .await;
        assert!(matches!(out.unwrap(), Outcome::Sent(_)));
        assert_eq!(b.sent(), 1);
    }

    #[tokio::test]
    async fn yes_without_a_cap_is_refused_but_dry_run_and_typing_yes_still_work() {
        let b = FakeBroker::default();
        let g = guard(Some(800.0), None, &say_yes);
        let market = Intent::Order(OrderKind::Market);
        // Unattended and uncapped: nothing is sent, and the error names the setting.
        let err = place_live_order(&b, Side::Buy, "SBIN", 1000, market, YES, &g)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("max_order_value") && err.contains("--yes"),
            "{err}"
        );
        assert_eq!(b.sent(), 0);
        // The same refusal comes before any login (`run_order` calls it first).
        assert!(require_cap_for_unattended(BrokerKind::Angel, YES, &g.limits).is_err());
        // --dry-run places nothing, so it is not blocked.
        let out = place_live_order(&b, Side::Buy, "SBIN", 1000, market, DRY, &g)
            .await
            .unwrap();
        assert_eq!(out, Outcome::DryRun);
        let dry_yes = OrderOpts {
            yes: true,
            dry_run: true,
        };
        assert!(require_cap_for_unattended(BrokerKind::Angel, dry_yes, &g.limits).is_ok());
        // Typing yes at the prompt keeps working without a cap.
        let out = place_live_order(&b, Side::Buy, "SBIN", 1, market, LIVE, &g)
            .await
            .unwrap();
        assert!(matches!(out, Outcome::Sent(_)));
        // With a cap, --yes works as before.
        let capped = guard(Some(800.0), Some(5000.0), &never_confirm);
        let out = place_live_order(&b, Side::Buy, "SBIN", 2, market, YES, &capped)
            .await
            .unwrap();
        assert!(matches!(out, Outcome::Sent(_)));
        assert_eq!(b.sent(), 2);
    }

    #[tokio::test]
    async fn a_zero_last_price_is_no_price_and_a_cap_refuses_the_market_order() {
        // Direct: halted, broken and negative prices are "unknown".
        for p in [Some(0.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY), None] {
            assert_eq!(usable_price(p), None, "{p:?}");
        }
        assert_eq!(usable_price(Some(1234.5)), Some(1234.5));

        // Through the safety flow: a guard that carries Some(0.0) still has
        // no estimate, so --yes with a cap sends nothing.
        let b = FakeBroker::default();
        let g = guard(Some(0.0), Some(5000.0), &never_confirm);
        let err = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1000,
            Intent::Order(OrderKind::Market),
            YES,
            &g,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot estimate"), "{err}");
        assert_eq!(b.sent(), 0);

        // End to end with a mock Yahoo quote that reports a halted stock.
        let body = r#"{"quoteResponse":{"result":[
            {"symbol":"SBIN.NS","quoteType":"EQUITY","regularMarketPrice":0.0}
        ],"error":null}}"#;
        let server = crate::testutil::MockServer::start(vec![(200, body.into())]).await;
        let base = server.url.clone();
        let y = YahooClient::for_tests_with_rewrite(reqwest::Client::new(), "c", move |url| {
            let parsed = reqwest::Url::parse(url).unwrap();
            match parsed.query() {
                Some(q) => format!("{}{}?{}", base, parsed.path(), q),
                None => format!("{}{}", base, parsed.path()),
            }
        });
        let last = last_price_with(&y, "SBIN").await;
        assert_eq!(last, None, "a quote of 0.0 is not a price");
        // The quote was fetched and parsed; it was the zero that was refused.
        assert_eq!(server.requests().len(), 1);
        let g = guard(last, Some(5000.0), &never_confirm);
        let err = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1000,
            Intent::Order(OrderKind::Market),
            YES,
            &g,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot estimate"), "{err}");
        assert_eq!(b.sent(), 0);
    }

    #[tokio::test]
    async fn unknown_value_with_a_cap_is_refused() {
        let b = FakeBroker::default();
        let g = guard(None, Some(5000.0), &never_confirm);
        let err = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1,
            Intent::Order(OrderKind::Market),
            YES,
            &g,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Cannot estimate"), "{err}");
        assert_eq!(b.sent(), 0);
    }

    #[tokio::test]
    async fn gtt_value_uses_the_limit_for_the_cap() {
        let b = FakeBroker::default();
        let g = guard(Some(800.0), Some(7000.0), &never_confirm);
        let gtt = Intent::Gtt {
            trigger: 780.0,
            limit: 779.0,
        };
        // 10 x 779 = 7790 > 7000.
        assert!(
            place_live_order(&b, Side::Sell, "SBIN", 10, gtt, YES, &g)
                .await
                .is_err()
        );
        let out = place_live_order(&b, Side::Sell, "SBIN", 8, gtt, YES, &g)
            .await
            .unwrap();
        assert_eq!(out, Outcome::Sent(Some("G1".into())));
        let sent = b.gtts.lock().unwrap()[0].clone();
        assert_eq!((sent.trigger, sent.limit, sent.qty), (780.0, 779.0, 8));
        assert_eq!(sent.last_price, 800.0);
    }

    #[tokio::test]
    async fn wrong_side_trigger_is_refused_before_anything_is_sent() {
        let b = FakeBroker::default();
        let g = guard(Some(800.0), None, &never_confirm);
        // Stop-loss sell with the trigger above the market fires at once.
        let bad = Intent::Order(OrderKind::StopLimit {
            trigger: 810.0,
            price: 805.0,
        });
        assert!(
            place_live_order(&b, Side::Sell, "SBIN", 1, bad, YES, &g)
                .await
                .is_err()
        );
        // Missing price: stop orders are refused rather than sent unchecked.
        let blind = guard(None, None, &never_confirm);
        let sl = Intent::Order(OrderKind::StopMarket { trigger: 780.0 });
        assert!(
            place_live_order(&b, Side::Sell, "SBIN", 1, sl, YES, &blind)
                .await
                .is_err()
        );
        assert_eq!(b.sent(), 0);
    }

    #[tokio::test]
    async fn bad_arguments_and_unresolved_symbols_send_nothing() {
        let b = FakeBroker::default();
        let g = guard(Some(800.0), None, &never_confirm);
        assert!(
            place_live_order(
                &b,
                Side::Buy,
                "SBIN",
                0,
                Intent::Order(OrderKind::Market),
                YES,
                &g
            )
            .await
            .is_err()
        );
        let neg = Intent::Order(OrderKind::Limit { price: -1.0 });
        assert!(
            place_live_order(&b, Side::Buy, "SBIN", 1, neg, YES, &g)
                .await
                .is_err()
        );
        let sub_paise = Intent::Order(OrderKind::Limit { price: 800.123 });
        assert!(
            place_live_order(&b, Side::Buy, "SBIN", 1, sub_paise, YES, &g)
                .await
                .is_err()
        );
        let market = Intent::Order(OrderKind::Market);
        assert!(
            place_live_order(&b, Side::Buy, "SBIN.BO", 1, market, YES, &g)
                .await
                .is_err()
        );
        let unresolved = FakeBroker {
            resolve_fails: true,
            ..Default::default()
        };
        assert!(
            place_live_order(
                &unresolved,
                Side::Buy,
                "SBI",
                1,
                Intent::Order(OrderKind::Market),
                YES,
                &g
            )
            .await
            .is_err()
        );
        assert_eq!(b.sent() + unresolved.sent(), 0);
    }

    #[tokio::test]
    async fn experimental_brokers_need_enable_orders_for_live_orders() {
        let b = FakeBroker {
            experimental: true,
            ..Default::default()
        };
        let mut g = guard(Some(800.0), None, &never_confirm);
        g.limits.enable_orders = false;
        // Dry run works.
        let out = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1,
            Intent::Order(OrderKind::Market),
            DRY,
            &g,
        )
        .await;
        assert_eq!(out.unwrap(), Outcome::DryRun);
        // Live (even with --yes) is refused.
        let err = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1,
            Intent::Order(OrderKind::Market),
            YES,
            &g,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("enable_orders"), "{err}");
        assert!(cancel_gtt(&b, "G1", YES, &g).await.is_err());
        assert_eq!(b.sent(), 0);
    }

    #[tokio::test]
    async fn angel_market_orders_need_no_opt_in_but_stop_loss_and_gtt_do() {
        let b = FakeBroker::default();
        let mut g = guard(Some(800.0), None, &say_yes);
        g.limits.enable_orders = false;
        // 0.3 behaviour: market and limit orders work as before.
        let out = place_live_order(
            &b,
            Side::Buy,
            "SBIN",
            1,
            Intent::Order(OrderKind::Market),
            LIVE,
            &g,
        )
        .await;
        assert!(matches!(out.unwrap(), Outcome::Sent(_)));
        // New, untested paths need enable_orders.
        let sl = Intent::Order(OrderKind::StopMarket { trigger: 780.0 });
        let err = place_live_order(&b, Side::Sell, "SBIN", 1, sl, YES, &g)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Stop-loss"), "{err}");
        let gtt = Intent::Gtt {
            trigger: 780.0,
            limit: 779.0,
        };
        assert!(
            place_live_order(&b, Side::Sell, "SBIN", 1, gtt, YES, &g)
                .await
                .is_err()
        );
        assert!(cancel_gtt(&b, "G1", YES, &g).await.is_err());
        assert_eq!(b.sent(), 1);
    }

    #[tokio::test]
    async fn unsupported_gtt_fails_before_the_preview() {
        let b = FakeBroker {
            no_gtt: true,
            ..Default::default()
        };
        let g = guard(Some(800.0), None, &never_confirm);
        let gtt = Intent::Gtt {
            trigger: 780.0,
            limit: 779.0,
        };
        let err = place_live_order(&b, Side::Sell, "SBIN", 1, gtt, DRY, &g)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not supported"), "{err}");
        assert!(cancel_gtt(&b, "G1", DRY, &g).await.is_err());
        assert_eq!(b.sent(), 0);
    }

    #[tokio::test]
    async fn cancel_gtt_checks_the_id_and_confirms() {
        let b = FakeBroker::default();
        let g = guard(None, None, &say_no);
        assert_eq!(
            cancel_gtt(&b, "G1", LIVE, &g).await.unwrap(),
            Outcome::Cancelled
        );
        assert!(cancel_gtt(&b, "NOPE", YES, &g).await.is_err());
        assert_eq!(b.sent(), 0);
        assert!(matches!(
            cancel_gtt(&b, "G1", YES, &g).await.unwrap(),
            Outcome::Sent(_)
        ));
        assert_eq!(b.cancelled.lock().unwrap().as_slice(), ["G1"]);
    }

    #[test]
    fn orders_refuse_non_nse_markets() {
        assert!(check_market(Market::In).is_ok());
        for m in [Market::Us, Market::Crypto, Market::Fx] {
            let err = check_market(m).unwrap_err().to_string();
            assert!(err.contains("NSE equity orders only"), "{err}");
        }
    }

    #[test]
    fn type_line_keeps_the_0_3_wording_for_market_and_limit() {
        assert_eq!(type_line(&Intent::Order(OrderKind::Market)), "MARKET");
        assert_eq!(
            type_line(&Intent::Order(OrderKind::Limit { price: 1300.0 })),
            "LIMIT @ ₹1300.00"
        );
    }
}
