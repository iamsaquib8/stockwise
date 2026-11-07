//! `trade sync`: executed trades from a broker's trade book into the
//! lot-based portfolio, with GTT status changes; and `trade gtt list`
//! with the changes since the last look.
//!
//! Both only read from the broker — the session is wrapped in
//! [`ReadOnly`]. `trade sync` previews by default and writes the portfolio
//! only with `--apply`; re-running it never counts a fill twice (see
//! [`Portfolio::apply_broker_fills`]).

use super::*;
use crate::brokers::gtt_watch::{self, GttChange};
use crate::brokers::readonly::ReadOnly;
use crate::brokers::{self, Broker, BrokerKind, Gtt, Side, Trade};
use crate::portfolio::{BrokerFill, FillEffect, FillSummary, OrderFill};

/// A trade-book row `trade sync` leaves out, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct SkippedFill {
    pub order_id: String,
    pub symbol: String,
    pub reason: String,
}

/// The portfolio symbol for a broker's plain NSE/BSE symbol. Shares are
/// fungible across NSE and BSE, so a fill joins the holding the portfolio
/// already has (`.NS`, else `.BO`); a new holding takes the fill's own
/// exchange (`.BO` for BSE, so a BSE-only scrip gets a ticker that prices).
fn portfolio_symbol(portfolio: &Portfolio, plain: &str, exchange: Option<&str>) -> String {
    let ns = format!("{plain}.NS");
    let bo = format!("{plain}.BO");
    if portfolio.holding(&ns).is_some() {
        ns
    } else if portfolio.holding(&bo).is_some() || exchange == Some("BSE") {
        bo
    } else {
        ns
    }
}

/// The trading day a fill with a time but no date belongs to (Angel One's
/// trade book): today once the session has begun, else the previous NSE
/// trading day — on weekends, holidays and before 09:00 IST the book still
/// shows the last session's fills.
pub(super) fn session_day(now: chrono::DateTime<chrono::FixedOffset>) -> chrono::NaiveDate {
    let mut day = now.date_naive();
    let open = chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("valid time");
    if now.time() >= open && crate::clock::is_trading_day(Market::In, day) {
        return day;
    }
    // Long holiday stretches are under two weeks.
    for _ in 0..14 {
        let Some(prev) = day.pred_opt() else { break };
        day = prev;
        if crate::clock::is_trading_day(Market::In, day) {
            break;
        }
    }
    day
}

/// Turn trade-book rows into portfolio fills. Only delivery (CNC) fills
/// in NSE/BSE cash equity with a side, quantity and price are kept; fills
/// before `since` are dropped silently. A fill without a date (Angel One
/// gives a time only) is dated `undated` (see [`session_day`]). All fills
/// of one stock in a batch map to the same portfolio symbol.
pub(super) fn to_fills(
    kind: BrokerKind,
    trades: &[Trade],
    since: Option<chrono::NaiveDate>,
    undated: chrono::NaiveDate,
    portfolio: &Portfolio,
) -> (Vec<BrokerFill>, Vec<SkippedFill>) {
    let mut fills = Vec::new();
    let mut skipped = Vec::new();
    let mut symbols: Vec<(String, String)> = Vec::new();
    for t in trades {
        let date = t.date.unwrap_or(undated);
        if since.is_some_and(|s| date < s) {
            continue;
        }
        let skip = |reason: String| SkippedFill {
            order_id: t.order_id.clone(),
            symbol: t.symbol.clone(),
            reason,
        };
        if !t.is_delivery_equity() {
            skipped.push(skip(format!(
                "not a delivery equity fill ({} on {})",
                t.product.as_deref().unwrap_or("no product"),
                t.exchange.as_deref().unwrap_or("unknown exchange")
            )));
            continue;
        }
        let (Some(side), Some(price)) = (t.side, t.price) else {
            skipped.push(skip("no side or price in the trade book".into()));
            continue;
        };
        if !(t.qty.is_finite() && t.qty > 0.0) || t.symbol.is_empty() {
            skipped.push(skip("no symbol or quantity in the trade book".into()));
            continue;
        }
        let fill_id = t.trade_id.clone().unwrap_or_else(|| {
            format!(
                "{}/{}@{}@{}",
                t.order_id,
                t.qty,
                price,
                t.time.as_deref().unwrap_or("")
            )
        });
        let symbol = match symbols.iter().find(|(plain, _)| *plain == t.symbol) {
            Some((_, s)) => s.clone(),
            None => {
                let s = portfolio_symbol(portfolio, &t.symbol, t.exchange.as_deref());
                symbols.push((t.symbol.clone(), s.clone()));
                s
            }
        };
        fills.push(BrokerFill {
            broker: kind.id().to_string(),
            order_id: t.order_id.clone(),
            fill_id,
            symbol,
            sell: side == Side::Sell,
            qty: t.qty,
            price,
            date: date.to_string(),
            time: t.time.clone(),
        });
    }
    (fills, skipped)
}

/// Leave out the sells of symbols whose lots still wait for a split or
/// bonus: their quantity and cost are in pre-split units, so a sale would
/// be matched and priced wrongly. Buys stay (a lot bought before the split
/// is adjusted by `portfolio actions --apply` like any other).
pub(super) fn hold_back_sells(
    fills: Vec<BrokerFill>,
    blocked: &[(String, String)],
) -> (Vec<BrokerFill>, Vec<SkippedFill>) {
    let mut kept = Vec::new();
    let mut held = Vec::new();
    for f in fills {
        match blocked
            .iter()
            .find(|(sym, _)| sym.eq_ignore_ascii_case(&f.symbol))
        {
            Some((_, reason)) if f.sell => held.push(SkippedFill {
                order_id: f.order_id.clone(),
                symbol: f.symbol.clone(),
                reason: reason.clone(),
            }),
            _ => kept.push(f),
        }
    }
    (kept, held)
}

/// Symbols with sells in `fills` whose lots have a split or bonus not yet
/// applied (checked on Yahoo). `None` when that could not be checked.
async fn pending_split_sells(
    portfolio: &Portfolio,
    fills: &[BrokerFill],
) -> Option<Vec<(String, String)>> {
    let mut symbols: Vec<&str> = Vec::new();
    for f in fills.iter().filter(|f| f.sell) {
        if portfolio.holding(&f.symbol).is_some() && !symbols.contains(&f.symbol.as_str()) {
            symbols.push(&f.symbol);
        }
    }
    if symbols.is_empty() {
        return Some(Vec::new());
    }
    let client = YahooClient::new().await.ok()?;
    let mut blocked = Vec::new();
    for sym in symbols {
        let splits = super::portfolio::pending_splits(&client, portfolio, sym)
            .await
            .ok()?;
        if !splits.is_empty() {
            let list: Vec<String> = splits
                .iter()
                .map(|s| format!("{}:{} on {}", s.numerator, s.denominator, s.date))
                .collect();
            blocked.push((
                sym.to_string(),
                format!(
                    "split/bonus {} not applied to your lots yet — run `stockwise portfolio \
                     actions {sym} --apply`, then sync again",
                    list.join(", ")
                ),
            ));
        }
    }
    Some(blocked)
}

/// An order still working at the broker (open, or partly filled).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct OpenOrder {
    pub order_id: String,
    pub symbol: String,
    pub side: Option<String>,
    pub status: Option<String>,
    pub qty: Option<f64>,
    pub filled_qty: Option<f64>,
}

fn is_final(status: Option<&str>) -> bool {
    matches!(
        status.unwrap_or("").to_ascii_lowercase().as_str(),
        "complete"
            | "completed"
            | "traded"
            | "filled"
            | "rejected"
            | "cancelled"
            | "canceled"
            | "expired"
    )
}

pub(super) fn open_orders(orders: &[brokers::Order]) -> Vec<OpenOrder> {
    orders
        .iter()
        .filter(|o| !is_final(o.status.as_deref()))
        .map(|o| OpenOrder {
            order_id: o.id.clone(),
            symbol: o.symbol.clone(),
            side: o.side.clone(),
            status: o.status.clone(),
            qty: o.qty,
            filled_qty: o.filled_qty,
        })
        .collect()
}

/// Portfolio quantity after the sync next to what the broker's holdings
/// list shows, for each symbol the sync touched.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct QtyCheck {
    pub symbol: String,
    pub portfolio_qty: f64,
    /// `None` when the broker's holdings list has no such row (or could
    /// not be read).
    pub broker_qty: Option<f64>,
}

fn qty_checks(
    after: &Portfolio,
    summary: &FillSummary,
    holdings: Option<&[brokers::Holding]>,
) -> Vec<QtyCheck> {
    let mut out: Vec<QtyCheck> = Vec::new();
    for o in &summary.orders {
        if out.iter().any(|q| q.symbol == o.symbol) {
            continue;
        }
        let plain = o.symbol.split('.').next().unwrap_or(&o.symbol);
        out.push(QtyCheck {
            symbol: o.symbol.clone(),
            portfolio_qty: after.holding(&o.symbol).map_or(0.0, |h| h.shares),
            broker_qty: holdings.and_then(|hs| {
                hs.iter()
                    .find(|h| h.symbol.eq_ignore_ascii_case(plain))
                    .map(|h| h.qty)
            }),
        });
    }
    out
}

#[derive(Debug, Clone, Serialize)]
struct SyncReport<'a> {
    broker: &'static str,
    experimental: bool,
    portfolio: &'a str,
    since: Option<String>,
    applied: bool,
    orders: &'a [OrderFill],
    already_synced: usize,
    skipped: &'a [SkippedFill],
    open_orders: &'a [OpenOrder],
    quantity_check: &'a [QtyCheck],
    gtt_changes: &'a [GttChange],
    notes: &'a [String],
}

fn effect_text(o: &OrderFill) -> String {
    match &o.effect {
        FillEffect::Lot => "new lot".into(),
        FillEffect::Sold { sales } => {
            let gain: f64 = sales.iter().map(|s| s.gain()).sum();
            format!(
                "sold FIFO from {} lot(s), realized {}",
                sales.len(),
                if gain >= 0.0 {
                    format!("+₹{gain:.2}")
                } else {
                    format!("-₹{:.2}", gain.abs())
                }
            )
        }
        FillEffect::Unmatched { held } => format!(
            "not applied: only {held} held on {} — add the missing lots with `portfolio add --date`",
            o.date
        ),
    }
}

/// `trade sync`.
pub async fn cmd_trade_sync(
    kind: BrokerKind,
    since: Option<String>,
    apply: bool,
    notify: bool,
    format: Format,
) -> Result<()> {
    let since = since
        .as_deref()
        .map(crate::portfolio::parse_date)
        .transpose()?;
    let broker = ReadOnly::new(brokers::connect(kind).await?);
    let trades = broker.trades(since).await?;
    let mut notes = Vec::new();
    let orders = match broker.orders().await {
        Ok(o) => o,
        Err(e) => {
            notes.push(format!("Order book not read: {e:#}"));
            Vec::new()
        }
    };
    let holdings = match broker.holdings().await {
        Ok(h) => Some(h),
        Err(e) => {
            notes.push(format!(
                "Holdings not read, so quantities are not compared: {e:#}"
            ));
            None
        }
    };
    let gtts: Option<Vec<Gtt>> = if broker.supports_gtt() {
        match broker.gtt_list().await {
            Ok(g) => Some(g),
            Err(e) => {
                notes.push(format!("GTT list not read: {e:#}"));
                None
            }
        }
    } else {
        None
    };
    let seen = gtt_watch::load(kind).unwrap_or_default();
    let gtt_changes = gtts
        .as_deref()
        .map(|g| gtt_watch::changes(&seen, g))
        .unwrap_or_default();

    let portfolio = Portfolio::load()?;
    let now = brokers::ist_now();
    let today = now.date_naive();
    let (fills, mut skipped) = to_fills(kind, &trades, since, session_day(now), &portfolio);
    let fills = match pending_split_sells(&portfolio, &fills).await {
        Some(blocked) => {
            let (kept, held) = hold_back_sells(fills, &blocked);
            skipped.extend(held);
            kept
        }
        None => {
            notes.push(
                "Could not check for splits or bonuses not yet applied to your lots (no \
                 connection to Yahoo); check `stockwise portfolio actions` before --apply."
                    .into(),
            );
            fills
        }
    };
    let mut after = portfolio.clone();
    let summary = after.apply_broker_fills(&fills);
    let open = open_orders(&orders);
    let checks = qty_checks(&after, &summary, holdings.as_deref());
    let changed = summary
        .orders
        .iter()
        .any(|o| !matches!(o.effect, FillEffect::Unmatched { .. }));
    if since.is_some_and(|s| s < today) && kind != BrokerKind::Dhan {
        notes.push(format!(
            "{} returns today's trades only; earlier fills can't be synced from its API — \
             import a tradebook with `stockwise import` instead.",
            kind.label()
        ));
    }

    if apply && changed {
        // The fills were worked out on the portfolio loaded before the
        // network calls: apply them to the file as it is now, under a lock.
        // Fills already synced are skipped, so this is safe to repeat.
        Portfolio::update(|p| {
            p.apply_broker_fills(&fills);
            Ok(())
        })?;
    }
    let name = crate::portfolio::active().unwrap_or("default");

    if format == Format::Json {
        output::print_json(&SyncReport {
            broker: kind.id(),
            experimental: kind.experimental(),
            portfolio: name,
            since: since.map(|d| d.to_string()),
            applied: apply && changed,
            orders: &summary.orders,
            already_synced: summary.already_synced,
            skipped: &skipped,
            open_orders: &open,
            quantity_check: &checks,
            gtt_changes: &gtt_changes,
            notes: &notes,
        })?;
    } else {
        print_sync(
            kind,
            name,
            apply,
            &summary,
            &skipped,
            &open,
            &checks,
            &gtt_changes,
            &notes,
        );
    }

    let mut delivered = true;
    if notify {
        if apply {
            let mut lines: Vec<String> = gtt_changes
                .iter()
                .filter(|c| c.notable())
                .map(GttChange::describe)
                .collect();
            let synced = summary
                .orders
                .iter()
                .filter(|o| !matches!(o.effect, FillEffect::Unmatched { .. }))
                .count();
            if synced > 0 {
                lines.push(format!("{synced} order(s) synced into portfolio {name}"));
            }
            delivered = send(kind, lines, format).await;
        } else if format.is_table() {
            println!(
                "  {}",
                "Notifications are sent with --apply (a preview changes nothing).".dimmed()
            );
        }
    }
    // Remember the GTTs only once their changes were reported, so a
    // notification that could not be sent is retried on the next run.
    if apply
        && delivered
        && let Some(g) = &gtts
    {
        gtt_watch::save(kind, g)?;
    }
    Ok(())
}

/// Send `lines` as one notification, saying so (or why not) in table mode.
/// False only when channels are configured and every one failed.
async fn send(kind: BrokerKind, lines: Vec<String>, format: Format) -> bool {
    if lines.is_empty() {
        return true;
    }
    let notifier = crate::notify::Notifier::from_env();
    if !notifier.is_enabled() {
        if format.is_table() {
            println!(
                "  {} No notification channel is configured — see `stockwise notify status`.",
                "!".yellow()
            );
        }
        return true;
    }
    let msg = crate::notify::Message::new(
        format!("StockWise: {} update", kind.label()),
        lines.join("\n"),
    );
    let delivered = notifier.send_logged(&msg).await;
    if format.is_table() {
        if delivered {
            println!("  {} Notification sent.", "✓".green());
        } else {
            println!(
                "  {} Notification failed; the GTT changes will be reported again next time.",
                "!".yellow()
            );
        }
    }
    delivered
}

#[allow(clippy::too_many_arguments)]
fn print_sync(
    kind: BrokerKind,
    portfolio: &str,
    apply: bool,
    summary: &FillSummary,
    skipped: &[SkippedFill],
    open: &[OpenOrder],
    checks: &[QtyCheck],
    gtt_changes: &[GttChange],
    notes: &[String],
) {
    let mode = if apply { "applied" } else { "preview" };
    print_header(&format!("{} — trade sync ({mode})", kind.display()));
    if summary.orders.is_empty() {
        println!("  {}", "No new delivery trades to sync.".dimmed());
    } else {
        println!(
            "  {:<10} {:<20} {:<4} {:<14} {:>8} {:>11}  {}",
            "Date".bold(),
            "Order".bold(),
            "Side".bold(),
            "Symbol".bold(),
            "Qty".bold(),
            "Price".bold(),
            "Effect".bold()
        );
        println!("  {}", "─".repeat(96).dimmed());
        for o in &summary.orders {
            let side = match o.side {
                "BUY" => o.side.green().to_string(),
                _ => o.side.red().to_string(),
            };
            let effect = effect_text(o);
            let effect = match o.effect {
                FillEffect::Unmatched { .. } => effect.yellow().to_string(),
                _ => effect,
            };
            let parts = if o.fills > 1 {
                format!(" ({} fills)", o.fills)
            } else {
                String::new()
            };
            println!(
                "  {:<10} {:<20} {:<4} {:<14} {:>8} {:>11}  {}{}",
                o.date,
                o.order_id.dimmed(),
                side,
                o.symbol.cyan(),
                o.qty,
                format!("₹{:.2}", o.price),
                effect,
                parts.dimmed()
            );
        }
    }
    println!();
    if summary.already_synced > 0 {
        println!(
            "  {} {} fill(s) were synced before and are left alone.",
            "·".dimmed(),
            summary.already_synced
        );
    }
    if !skipped.is_empty() {
        println!(
            "  {} {} trade(s) skipped (only delivery equity becomes lots):",
            "·".dimmed(),
            skipped.len()
        );
        for s in skipped {
            println!(
                "      {} {} — {}",
                s.symbol.cyan(),
                s.order_id.dimmed(),
                s.reason
            );
        }
    }
    for o in open {
        let filled = match (o.filled_qty, o.qty) {
            (Some(f), Some(q)) if f > 0.0 => format!(", {f} of {q} filled"),
            _ => String::new(),
        };
        println!(
            "  {} Open: {} {} {} ({}{}) — its fills sync once they show in the trade book.",
            "·".dimmed(),
            o.side.as_deref().unwrap_or("?"),
            o.symbol.cyan(),
            o.order_id.dimmed(),
            o.status.as_deref().unwrap_or("?"),
            filled
        );
    }
    for q in checks {
        match q.broker_qty {
            Some(b) if (b - q.portfolio_qty).abs() > 1e-9 => println!(
                "  {} {}: portfolio would hold {}, {} lists {} — today's buys reach the holdings \
                 list after settlement (T+1); a gap that lasts means lots are missing or doubled \
                 (`stockwise portfolio lots {}`).",
                "!".yellow(),
                q.symbol,
                q.portfolio_qty,
                kind.label(),
                b,
                q.symbol
            ),
            _ => {}
        }
    }
    if !gtt_changes.is_empty() {
        println!("  GTT changes since the last check:");
        for c in gtt_changes {
            println!("    {} {}", "·".dimmed(), c.describe());
        }
    }
    for n in notes {
        println!("  {} {}", "!".yellow(), n);
    }
    let applicable = summary
        .orders
        .iter()
        .filter(|o| !matches!(o.effect, FillEffect::Unmatched { .. }))
        .count();
    if apply {
        if applicable > 0 {
            println!(
                "\n  {} {} order(s) written to portfolio {}.",
                "✓".green(),
                applicable,
                portfolio.cyan()
            );
        }
    } else if applicable > 0 {
        println!(
            "\n  Preview only — run again with {} to write {} order(s) into portfolio {}.",
            "--apply".bold(),
            applicable,
            portfolio.cyan()
        );
    }
    println!();
}

/// `trade gtt list`: the GTTs, and what changed since the last look.
pub async fn cmd_trade_gtt_list(kind: BrokerKind, notify: bool, format: Format) -> Result<()> {
    let broker = ReadOnly::new(brokers::connect(kind).await?);
    let gtts = broker.gtt_list().await?;
    let seen = gtt_watch::load(kind).unwrap_or_default();
    let changes = gtt_watch::changes(&seen, &gtts);

    if format == Format::Json {
        #[derive(Serialize)]
        struct Out<'a> {
            broker: &'static str,
            experimental: bool,
            gtts: &'a [Gtt],
            changes: &'a [GttChange],
            /// Nothing to compare with yet.
            first_check: bool,
            last_checked: Option<&'a str>,
        }
        output::print_json(&Out {
            broker: kind.id(),
            experimental: kind.experimental(),
            gtts: &gtts,
            changes: &changes,
            first_check: seen.checked_at.is_none(),
            last_checked: seen.checked_at.as_deref(),
        })?;
    } else {
        super::trade::print_gtts(kind, &gtts);
        match &seen.checked_at {
            None => println!(
                "  {}",
                "First check — status changes show from the next `trade gtt list`.".dimmed()
            ),
            Some(_) if changes.is_empty() => {
                println!("  {}", "No changes since the last check.".dimmed())
            }
            Some(at) => {
                let at = chrono::DateTime::parse_from_rfc3339(at)
                    .map(|t| t.format("%Y-%m-%d %H:%M IST").to_string())
                    .unwrap_or_else(|_| at.clone());
                println!("  Changes since {at}:");
                for c in &changes {
                    println!("    {} {}", "·".dimmed(), c.describe());
                }
                println!();
            }
        }
    }
    let delivered = if notify {
        let lines = changes
            .iter()
            .filter(|c| c.notable())
            .map(GttChange::describe)
            .collect();
        send(kind, lines, format).await
    } else {
        true
    };
    // A notification that could not be sent is retried on the next look.
    if delivered {
        gtt_watch::save(kind, &gtts)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn trade(order: &str, id: Option<&str>, side: Side, qty: f64, price: f64) -> Trade {
        Trade {
            trade_id: id.map(String::from),
            order_id: order.into(),
            symbol: "INFY".into(),
            exchange: Some("NSE".into()),
            product: Some("CNC".into()),
            side: Some(side),
            qty,
            price: Some(price),
            date: Some(d("2026-10-20")),
            time: Some("09:30:00".into()),
        }
    }

    #[test]
    fn only_delivery_equity_fills_become_portfolio_fills() {
        let p = Portfolio::default();
        let mut mis = trade("O2", Some("T2"), Side::Buy, 1.0, 10.0);
        mis.product = Some("MIS".into());
        let mut fno = trade("O3", Some("T3"), Side::Buy, 75.0, 10.0);
        fno.exchange = Some("NFO".into());
        let mut unpriced = trade("O4", Some("T4"), Side::Buy, 1.0, 10.0);
        unpriced.price = None;
        let trades = vec![
            trade("O1", Some("T1"), Side::Buy, 5.0, 1500.0),
            mis,
            fno,
            unpriced,
        ];
        let (fills, skipped) = to_fills(BrokerKind::Zerodha, &trades, None, d("2026-10-25"), &p);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].symbol, "INFY.NS");
        assert_eq!(fills[0].fill_id, "T1");
        assert_eq!(fills[0].broker, "zerodha");
        assert_eq!(skipped.len(), 3);
        assert!(skipped[0].reason.contains("MIS"), "{:?}", skipped[0]);
    }

    #[test]
    fn fills_without_ids_or_dates_get_stable_stand_ins() {
        let p = Portfolio::default();
        let mut t = trade("O1", None, Side::Sell, 5.0, 1500.0);
        t.date = None;
        let (a, _) = to_fills(BrokerKind::Angel, &[t.clone()], None, d("2026-10-25"), &p);
        let (b, _) = to_fills(BrokerKind::Angel, &[t], None, d("2026-10-25"), &p);
        assert_eq!(a[0].date, "2026-10-25");
        assert!(a[0].sell);
        assert_eq!(a[0].fill_id, b[0].fill_id);
    }

    fn at(s: &str) -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn time_only_fills_belong_to_the_last_session() {
        // During or after Wednesday's session: today.
        assert_eq!(
            session_day(at("2026-10-21T15:00:00+05:30")),
            d("2026-10-21")
        );
        // Before Wednesday's open: Tuesday is Dussehra, so Monday.
        assert_eq!(
            session_day(at("2026-10-21T08:00:00+05:30")),
            d("2026-10-19")
        );
        // Sunday: Friday.
        assert_eq!(
            session_day(at("2026-10-25T12:00:00+05:30")),
            d("2026-10-23")
        );
    }

    #[test]
    fn sells_wait_for_pending_splits_but_buys_do_not() {
        let p = Portfolio::default();
        let trades = vec![
            trade("B1", Some("T1"), Side::Buy, 5.0, 1500.0),
            trade("S1", Some("T2"), Side::Sell, 5.0, 760.0),
        ];
        let (fills, _) = to_fills(BrokerKind::Zerodha, &trades, None, d("2026-10-25"), &p);
        let blocked = vec![("INFY.NS".to_string(), "split 2:1 not applied".to_string())];
        let (kept, held) = hold_back_sells(fills, &blocked);
        assert_eq!(kept.len(), 1);
        assert!(!kept[0].sell);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].order_id, "S1");
        assert!(held[0].reason.contains("split"));
    }

    #[test]
    fn bse_fills_open_bse_holdings_unless_the_stock_is_held_already() {
        let mut p = Portfolio::default();
        let mut bse = trade("O1", Some("T1"), Side::Buy, 5.0, 100.0);
        bse.exchange = Some("BSE".into());
        // A later NSE sell of the same stock in the batch joins the same holding.
        let nse_sell = trade("O2", Some("T2"), Side::Sell, 5.0, 101.0);
        let (fills, _) = to_fills(
            BrokerKind::Zerodha,
            &[bse.clone(), nse_sell],
            None,
            d("2026-10-25"),
            &p,
        );
        assert_eq!(fills[0].symbol, "INFY.BO");
        assert_eq!(fills[1].symbol, "INFY.BO");
        p.add_lot(
            "INFY.NS",
            crate::portfolio::Lot::new(1.0, 1.0, "2024-01-01", "manual"),
        );
        let (fills, _) = to_fills(BrokerKind::Zerodha, &[bse], None, d("2026-10-25"), &p);
        assert_eq!(fills[0].symbol, "INFY.NS");
    }

    #[test]
    fn since_drops_older_fills_and_bse_fills_join_existing_holdings() {
        let mut p = Portfolio::default();
        p.add_lot(
            "INFY.BO",
            crate::portfolio::Lot::new(1.0, 1.0, "2024-01-01", "manual"),
        );
        let t = trade("O1", Some("T1"), Side::Buy, 5.0, 1500.0);
        let (fills, _) = to_fills(
            BrokerKind::Dhan,
            std::slice::from_ref(&t),
            Some(d("2026-10-21")),
            d("2026-10-25"),
            &p,
        );
        assert!(fills.is_empty());
        let (fills, _) = to_fills(BrokerKind::Dhan, &[t], None, d("2026-10-25"), &p);
        assert_eq!(fills[0].symbol, "INFY.BO");
    }

    #[test]
    fn sync_of_a_fixture_trade_book_is_idempotent_with_partial_fills() {
        let mut p = Portfolio::default();
        p.add_lot(
            "IOC.NS",
            crate::portfolio::Lot::new(3.0, 100.0, "2025-01-01", "manual"),
        );
        // Kite's fixture: INFY bought in two parts, IOC sold, MIS and F&O fills.
        let trades = vec![
            trade("200000000000000", Some("10000000"), Side::Buy, 4.0, 1500.0),
            trade("200000000000000", Some("10000001"), Side::Buy, 6.0, 1510.0),
            Trade {
                symbol: "IOC".into(),
                ..trade("200000000000001", Some("10000002"), Side::Sell, 1.0, 109.4)
            },
        ];
        let (fills, _) = to_fills(BrokerKind::Zerodha, &trades, None, d("2026-10-25"), &p);
        let first = p.apply_broker_fills(&fills);
        assert_eq!(first.orders.len(), 2);
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 10.0);
        assert_eq!(p.holding("IOC.NS").unwrap().shares, 2.0);
        let again = p.apply_broker_fills(&fills);
        assert!(again.orders.is_empty());
        assert_eq!(again.already_synced, 3);
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 10.0);
        assert_eq!(p.realized.len(), 1);
    }

    #[test]
    fn open_orders_exclude_final_states() {
        let o = |status: &str| brokers::Order {
            id: status.into(),
            status: Some(status.into()),
            ..Default::default()
        };
        let open = open_orders(&[
            o("COMPLETE"),
            o("REJECTED"),
            o("OPEN"),
            o("TRIGGER PENDING"),
            o("cancelled"),
            o("traded"),
        ]);
        let ids: Vec<&str> = open.iter().map(|o| o.order_id.as_str()).collect();
        assert_eq!(ids, ["OPEN", "TRIGGER PENDING"]);
    }

    #[test]
    fn quantity_check_compares_with_broker_holdings() {
        let mut p = Portfolio::default();
        let fills = to_fills(
            BrokerKind::Zerodha,
            &[trade("O1", Some("T1"), Side::Buy, 5.0, 1500.0)],
            None,
            d("2026-10-25"),
            &p,
        )
        .0;
        let summary = p.apply_broker_fills(&fills);
        let held = [brokers::Holding {
            symbol: "INFY".into(),
            qty: 5.0,
            ..Default::default()
        }];
        let checks = qty_checks(&p, &summary, Some(&held));
        assert_eq!(checks[0].portfolio_qty, 5.0);
        assert_eq!(checks[0].broker_qty, Some(5.0));
        assert_eq!(qty_checks(&p, &summary, None)[0].broker_qty, None);
    }
}
