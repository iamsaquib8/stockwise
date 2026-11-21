//! `stockwise ipo`: the live Indian IPO board with a verdict per issue, the
//! analysis of one issue (verdict first), `ipo review` (the verdicts' track
//! record) and `ipo calibrate` (refit the listing-day model). The data and
//! the rules live in `crate::ipo`, `crate::ipo_verdict` and
//! `crate::ipo_model`; this file only fetches, logs and prints.

use super::*;
use crate::ipo::{
    self, BoardFilter, BoardKind, IpoAnalysis, IpoBoardView, IpoStatus, IpoSummaryRow,
};
use crate::ipo_verdict::{self as iv, Kind as VKind, Verdict};
use colored::ColoredString;

#[allow(clippy::too_many_arguments)]
pub async fn cmd_ipo(
    query: Option<&str>,
    board: BoardFilter,
    capital: Option<f64>,
    recent: usize,
    rhp: Option<&std::path::Path>,
    rhp_template: bool,
    fmt: Format,
) -> Result<()> {
    if rhp_template {
        let sym = query.unwrap_or("SYMBOL").trim().to_ascii_uppercase();
        if let Some(p) = ipo::rhp_path(&sym) {
            eprintln!("Save as {}", p.display());
        }
        print!("{}", iv::RHP_TEMPLATE);
        return Ok(());
    }
    let client = YahooClient::new().await?;
    match query {
        None => {
            let view = ipo::fetch_board(&client, board, recent).await?;
            log_board(&view);
            if fmt == Format::Json {
                return output::print_json(&view);
            }
            print_ipo_board(&view);
        }
        Some(q) => {
            let series = match board {
                BoardFilter::Mainboard => Some("EQ"),
                BoardFilter::Sme => Some("SME"),
                BoardFilter::All => None,
            };
            let analysis = ipo::fetch_analysis(&client, q, series, capital, rhp).await?;
            log_analysis(&analysis);
            let stock_action = if analysis.status == IpoStatus::Listed {
                listed_action(&client, &analysis.symbol).await
            } else {
                None
            };
            if fmt == Format::Json {
                let mut v = serde_json::to_value(&analysis)?;
                if let Some(a) = stock_action {
                    v["stock_action"] = serde_json::Value::String(a);
                }
                return output::print_json(&v);
            }
            print_ipo_analysis(&analysis, stock_action.as_deref());
        }
    }
    Ok(())
}

/// The stock action engine's line for an issue that already trades.
async fn listed_action(client: &YahooClient, symbol: &str) -> Option<String> {
    let sym = format!("{symbol}.NS");
    let q = client
        .get_quote(&[sym.as_str()])
        .await
        .ok()?
        .into_iter()
        .next()?;
    let notes = crate::commands::load_notes();
    let profile = crate::config::effective_profile();
    Some(crate::commands::action_line(
        &q,
        None,
        None,
        None,
        &notes,
        &profile,
        chrono::Local::now().date_naive(),
    ))
}

fn ist_today() -> chrono::NaiveDate {
    (chrono::Utc::now() + chrono::Duration::seconds(19_800)).date_naive()
}

/// Append the board's verdicts to the IPO track record (best effort).
fn log_board(v: &IpoBoardView) {
    let today = ist_today();
    let entries: Vec<_> = v
        .ongoing
        .iter()
        .chain(&v.upcoming)
        .filter_map(|r| {
            crate::ipo_log::entry_for(
                today,
                &r.symbol,
                r.board,
                r.issue_price.or_else(|| r.price_band.cap_price()),
                r.close_date,
                r.listing_date,
                r.qib_x,
                r.verdict.as_ref()?,
            )
        })
        .collect();
    record(entries);
}

pub(crate) fn log_analysis(a: &IpoAnalysis) {
    let entry = crate::ipo_log::entry_for(
        ist_today(),
        &a.symbol,
        a.board,
        a.issue_price.or_else(|| a.price_band.cap_price()),
        a.close_date,
        a.listing_date,
        a.subscription.qib_x,
        &a.verdict,
    );
    record(entry.into_iter().collect());
}

fn record(entries: Vec<crate::ipo_log::Entry>) {
    if entries.is_empty() {
        return;
    }
    if let Err(e) = crate::ipo_log::record(entries) {
        eprintln!("  ⚠ Could not log the IPO verdicts (ipo_signal_log.json): {e:#}");
    }
}

/// `4.20x`, or a dash when the number is unknown.
fn times(v: Option<f64>) -> String {
    v.map_or("—".to_string(), |x| format!("{x:.2}x"))
}

/// Colours a verdict: green APPLY, yellow WAIT and SELL ON LISTING, red
/// AVOID, dim NO CALL.
fn verdict_tone(v: &Verdict, text: &str) -> ColoredString {
    match v.kind {
        VKind::Apply => text.green().bold(),
        VKind::ApplySellOnListing | VKind::Wait => text.yellow().bold(),
        VKind::Avoid => text.red().bold(),
        VKind::NoCall | VKind::Listed => text.normal().bold(),
    }
}

/// Colours a descriptive label: red for weak readings, yellow for mixed or
/// pending ones, green for the rest.
fn tone(text: &str) -> ColoredString {
    let t = text.to_ascii_uppercase();
    if t.contains("WEAK") || t.contains("COLD") || t.contains("BELOW") {
        text.red().bold()
    } else if t.contains("BOOK")
        || t.contains("MIXED")
        || t.contains("SELECTIVE")
        || t.contains("INSUFFICIENT")
        || t.contains("SME")
        || t.contains("UPCOMING")
        || t.contains("MODERATE")
        || t.contains("AWAITING")
    {
        text.yellow().bold()
    } else {
        text.green().bold()
    }
}

fn board_badge(board: BoardKind, is_bse_sme: bool) -> ColoredString {
    let label = format!("[{}]", board.label(is_bse_sme));
    match board {
        BoardKind::Mainboard => label.cyan(),
        BoardKind::Sme => label.magenta(),
    }
}

fn terms_line(r: &IpoSummaryRow) -> String {
    let mut terms = vec![r.price_band.display_short()];
    if let Some(lot) = r.lot_size {
        terms.push(format!("lot {lot}"));
    }
    if let Some(min) = r.min_investment_inr {
        terms.push(format!("min ₹{}", ipo::fmt_inr(min)));
    }
    if let Some(cr) = r.issue_size_cr {
        terms.push(format!("size ₹{} Cr", ipo::fmt_inr(cr)));
    }
    terms.join(" · ")
}

fn print_board_row(r: &IpoSummaryRow) {
    let verdict = r
        .verdict
        .as_ref()
        .map(|v| format!("  {}", verdict_tone(v, &v.cell())))
        .unwrap_or_default();
    println!(
        "  {} {}  {}{}",
        r.symbol.bold(),
        board_badge(r.board, r.is_bse_sme),
        r.company_name,
        verdict
    );
    println!("      {}  ·  {}", terms_line(r), r.window_note.dimmed());
    if r.status == IpoStatus::Ongoing && r.total_x.is_some() {
        println!(
            "      QIB {}  NII {}  Retail {}  Total {}   {}",
            times(r.qib_x),
            times(r.nii_x),
            times(r.retail_x),
            times(r.total_x).bold(),
            tone(&r.quick_signal)
        );
    }
    if let Some(v) = &r.verdict
        && let Some(first) = v.reasons.first()
    {
        println!("      {}", first.dimmed());
    }
}

fn print_recent_row(r: &IpoSummaryRow) {
    let issue = r
        .issue_price
        .or_else(|| r.price_band.cap_price())
        .map_or("TBA".to_string(), |p| format!("₹{}", ipo::fmt_price(p)));
    let now = r
        .ltp
        .map_or("—".to_string(), |p| format!("₹{}", ipo::fmt_price(p)));
    let colour = |v: Option<f64>| match v {
        Some(p) if p >= 0.0 => format!("{p:+.1}%").green().to_string(),
        Some(p) => format!("{p:+.1}%").red().to_string(),
        None => "—".dimmed().to_string(),
    };
    println!(
        "  {} {} {} {} {} {} {}",
        pad_end(&r.symbol, 12).bold(),
        pad_end(&format!("{}", board_badge(r.board, r.is_bse_sme)), 14),
        pad_end(&r.window_note, 24).dimmed(),
        pad_start(&issue, 9),
        pad_start(&colour(r.listing_return_pct), 9),
        pad_start(&now, 9),
        pad_start(&colour(r.return_from_issue_pct), 8)
    );
}

pub fn print_ipo_board(v: &IpoBoardView) {
    print_header(&format!(
        "Indian IPO Board — {}",
        v.as_of.format("%d %b %Y")
    ));

    let r = &v.regime;
    println!("  IPO market:  {}  {}", tone(&r.label), r.summary.dimmed());
    let mut ctx = Vec::new();
    if let Some(n) = r.nifty_change_pct {
        ctx.push(format!("NIFTY 50 {n:+.2}% today"));
    }
    if let Some(vix) = r.india_vix {
        ctx.push(format!("India VIX {vix:.1}"));
    }
    if !ctx.is_empty() {
        println!("               {}", ctx.join(" · ").dimmed());
    }

    print_section(&format!("Open for bidding ({})", v.ongoing.len()));
    if v.ongoing.is_empty() {
        println!("  {}", "No IPO is open for bidding right now.".dimmed());
    }
    for r in &v.ongoing {
        print_board_row(r);
    }

    print_section(&format!("Upcoming ({})", v.upcoming.len()));
    if v.upcoming.is_empty() {
        println!("  {}", "No IPO has announced its dates yet.".dimmed());
    }
    for r in &v.upcoming {
        print_board_row(r);
    }

    if !v.recent.is_empty() {
        print_section("Recently closed & listed");
        println!(
            "  {} {} {} {} {} {} {}",
            pad_end("Symbol", 12).dimmed(),
            pad_end("Board", 14).dimmed(),
            pad_end("Status", 24).dimmed(),
            pad_start("Issue", 9).dimmed(),
            pad_start("Day 1", 9).dimmed(),
            pad_start("Now", 9).dimmed(),
            pad_start("Return", 8).dimmed()
        );
        for r in &v.recent {
            print_recent_row(r);
        }
    }

    if !v.headlines.is_empty() {
        print_section("In the news");
        for h in &v.headlines {
            println!("  • {}  {}", h.title, format!("({})", h.source).dimmed());
        }
    }
    for n in &v.notes {
        println!("\n  {} {}", "ⓘ".dimmed(), n.dimmed());
    }
    let methodology = v
        .ongoing
        .iter()
        .chain(&v.upcoming)
        .find_map(|r| r.verdict.as_ref());
    if let Some(m) = methodology {
        println!(
            "\n  {} Verdicts: IPO methodology {} (docs/methodology.md). {}",
            "ⓘ".dimmed(),
            m.methodology,
            m.disclaimer.dimmed()
        );
    }

    println!(
        "\n  {} Verdict, valuation, allotment odds and listing plan: '{}'\n",
        "💡".yellow(),
        "stockwise ipo <SYMBOL> [--capital 2.5L]".cyan()
    );
}

fn money(v: f64) -> String {
    format!("₹{}", ipo::fmt_inr(v))
}

fn opt(v: Option<f64>, f: impl Fn(f64) -> String) -> String {
    v.map_or("—".to_string(), f)
}

pub fn print_ipo_analysis(a: &IpoAnalysis, stock_action: Option<&str>) {
    let border = "═".repeat(64);
    let subline = "─".repeat(64);
    let v = &a.verdict;

    // The verdict comes first.
    println!("\n{}", verdict_tone(v, &v.headline()));
    for r in &v.reasons {
        println!("  • {r}");
    }
    if let Some(inv) = &v.invalidation {
        println!("  • Invalidated if {inv}");
    }
    if !v.missing.is_empty() && v.kind != VKind::NoCall {
        println!(
            "  {} missing: {}",
            "ⓘ".dimmed(),
            v.missing.join(", ").dimmed()
        );
    }
    if !v.gates.is_empty() && v.kind == VKind::NoCall {
        for g in &v.gates {
            println!("  {} {}", "▼".red(), g);
        }
    }
    if let Some(sa) = stock_action {
        println!("  Stock action now: {}", sa.bold());
    }
    println!(
        "  {}",
        format!(
            "IPO methodology {} · data coverage {:.0}%",
            v.methodology, v.coverage
        )
        .dimmed()
    );

    println!("\n{}", border.cyan());
    println!(
        "  {} {} ({})",
        a.symbol.bold(),
        board_badge(a.board, a.is_bse_sme),
        a.company_name
    );
    let mut meta = vec![a.status.label().to_string()];
    if !a.window_note.is_empty() {
        meta.push(a.window_note.clone());
    }
    if let Some(ind) = &a.industry {
        meta.push(ind.clone());
    }
    println!("  {}", meta.join(" · ").dimmed());
    println!("{}", border.cyan());

    // ── Listing model ──
    if let Some(o) = &v.outlook {
        println!("\n  {}", "LISTING-DAY MODEL".bold().yellow());
        println!("  {}", subline.dimmed());
        print_kv(
            "Expected listing gain",
            &format!(
                "{:+.1}%  (10th–90th percentile {:+.1}% to {:+.1}%)",
                o.expected_gain_pct, o.p10_pct, o.p90_pct
            ),
        );
        print_kv("P(close below issue)", &format!("{:.0}%", o.p_loss * 100.0));
        if let Some(q) = v.qib_threshold_x {
            print_kv("QIB needed", &format!("≥ {q:.1}x"));
        }
        print_kv(
            "Model",
            &format!(
                "fitted {} on {} mainboard issues · R² {:.2} · residual sd {:.1} pts{}",
                o.model_fit_date,
                o.model_n,
                o.model_r2,
                o.model_resid_sd,
                if o.regime_known {
                    ""
                } else {
                    " · regime unknown, training mean used"
                }
            )
            .dimmed()
            .to_string(),
        );
    }
    if !v.expected_value.is_empty() {
        println!(
            "\n  {}",
            "EXPECTED VALUE PER MINIMUM APPLICATION".bold().yellow()
        );
        println!("  {}", subline.dimmed());
        for e in &v.expected_value {
            let ev = format!("₹{:.0}", e.ev_inr);
            println!(
                "  {} {:>12} blocked  odds {:>6.2}%  cost ₹{:<6.0} EV {}",
                pad_end(&e.category, 16),
                money(e.amount_blocked_inr),
                e.p_allot * 100.0,
                e.blocked_cost_inr,
                if e.ev_inr >= 0.0 {
                    ev.green()
                } else {
                    ev.red()
                }
            );
        }
    }
    if let Some(p) = &v.plan {
        println!("\n  {}", "LISTING-DAY PLAN".bold().yellow());
        println!("  {}", subline.dimmed());
        for s in &p.steps {
            println!("  {} {s}", "→".cyan());
        }
    }
    if !v.overhang.is_empty() {
        println!("\n  {}", "ANCHOR LOCK-IN".bold().yellow());
        println!("  {}", subline.dimmed());
        for u in &v.overhang {
            println!("  {} {}", "•".dimmed(), u.note);
        }
    }

    // ── Terms ──
    println!("\n  {}", "ISSUE TERMS".bold().yellow());
    println!("  {}", subline.dimmed());
    print_kv("Price band", &a.price_band.display_short());
    if let Some(lot) = a.lot_size {
        print_kv("Lot size", &format!("{lot} shares"));
    }
    if let Some(min) = a.min_investment_inr {
        print_kv("Minimum application", &money(min));
    }
    if let Some(cr) = a.structure.total_issue_cr {
        print_kv("Issue size", &format!("₹{} Cr", ipo::fmt_inr(cr)));
    }
    print_kv("Structure", &a.structure.structure_tag);
    if let Some(n) = a.structure.anchor_shares {
        print_kv(
            "Anchor reservation",
            &format!("{} shares", ipo::fmt_int_indian(n)),
        );
    }
    if !a.structure.brlms.is_empty() {
        print_kv("Lead managers", &a.structure.brlms.join(", "));
    }
    if let Some(reg) = &a.structure.registrar_name {
        print_kv("Registrar", reg);
    }
    if let Some(ltp) = a.ltp {
        let ret = a
            .return_from_issue_pct
            .map_or(String::new(), |p| format!("  ({p:+.1}% vs issue price)"));
        print_kv("Trading at", &format!("₹{}{}", ipo::fmt_price(ltp), ret));
    }

    // ── Valuation ──
    let val = &a.valuation;
    println!("\n  {}", "VALUATION AT THE UPPER BAND".bold().yellow());
    println!("  {}", subline.dimmed());
    print_kv(
        "Market cap after issue",
        &opt(val.market_cap_cr, |m| format!("₹{} Cr", ipo::fmt_inr(m))),
    );
    print_kv(
        "P/E · P/B · EV/Sales",
        &format!(
            "{} · {} · {}",
            opt(val.pe, |x| format!("{x:.1}")),
            opt(val.pb, |x| format!("{x:.1}")),
            opt(val.ev_sales, |x| format!("{x:.1}"))
        ),
    );
    print_kv(
        "Peer median",
        &format!(
            "{} · {} · {}{}",
            opt(val.peer_median_pe, |x| format!("{x:.1}")),
            opt(val.peer_median_pb, |x| format!("{x:.1}")),
            opt(val.peer_median_ev_sales, |x| format!("{x:.1}")),
            if val.peers_used.is_empty() {
                String::new()
            } else {
                format!("  ({})", val.peers_used.join(", "))
            }
        ),
    );
    print_kv(
        "ROE · D/E",
        &format!(
            "{} · {}",
            opt(val.roe_pct, |x| format!("{x:.1}%")),
            opt(val.debt_to_equity, |x| format!("{x:.2}"))
        ),
    );
    print_kv(
        "Revenue · PAT growth",
        &format!(
            "{} · {} a year",
            opt(val.revenue_cagr_pct, |x| format!("{x:+.1}%")),
            opt(val.pat_cagr_pct, |x| format!("{x:+.1}%"))
        ),
    );
    print_kv(
        "Promoters after issue",
        &opt(val.promoter_post_pct, |x| format!("{x:.1}%")),
    );
    print_kv(
        "Offer for sale",
        &format!(
            "{}{}",
            opt(val.ofs_pct, |x| format!("{x:.0}% of the issue")),
            match val.ofs_fund_exit {
                Some(true) => " · PE/VC fund selling",
                Some(false) => "",
                None => " · sellers unknown",
            }
        ),
    );
    if let Some(f) = &a.rhp_file {
        print_kv("RHP figures", &f.dimmed().to_string());
    }

    // ── Subscription ──
    let s = &a.subscription;
    println!(
        "\n  {}  {}",
        "SUBSCRIPTION".bold().yellow(),
        format!("({})", s.source_note).dimmed()
    );
    println!("  {}", subline.dimmed());
    print_kv("Total", &times(s.total_x));
    if s.qib_x.is_some() {
        let mut extra = Vec::new();
        if let Some(p) = s.mf_share_of_qib_pct {
            extra.push(format!("Mutual funds {p:.0}% of QIB bids"));
        }
        if let Some(n) = s.fii_bids.filter(|&n| n > 0) {
            extra.push(format!("FII {} sh", ipo::fmt_int_indian(n)));
        }
        print_kv(
            "QIB (institutions)",
            &format!("{}  {}", times(s.qib_x), extra.join(" · ").dimmed()),
        );
    }
    print_kv(
        "NII (HNI)",
        &format!(
            "{}  {}",
            times(s.nii_x),
            format!("bNII {} · sNII {}", times(s.bnii_x), times(s.snii_x)).dimmed()
        ),
    );
    print_kv(
        "Retail (RII)",
        &format!(
            "{}  {}",
            times(s.retail_x),
            s.retail_cutoff_pct
                .map_or(String::new(), |p| format!("{p:.0}% of bids at Cut-Off"))
                .dimmed()
        ),
    );
    if let Some(x) = s.employee_x {
        print_kv("Employees", &times(Some(x)));
    }
    if let Some(x) = s.shareholder_x {
        print_kv("Shareholders", &times(Some(x)));
    }

    if !a.demand_ladder.is_empty() {
        println!(
            "\n  {}",
            "DEMAND BY PRICE (cumulative bids)".bold().yellow()
        );
        println!("  {}", subline.dimmed());
        for step in &a.demand_ladder {
            let filled = ((step.pct_of_total_bids / 100.0) * 24.0).round() as usize;
            let bar = format!(
                "{}{}",
                "█".repeat(filled.min(24)),
                "░".repeat(24 - filled.min(24))
            );
            println!(
                "  {} {} {:>14}  {:>5.1}%",
                pad_end(&step.label, 9),
                bar.cyan(),
                ipo::fmt_int_indian(step.cumulative_shares),
                step.pct_of_total_bids
            );
        }
    }

    // ── Allotment ──
    println!("\n  {}", "ALLOTMENT ODDS (SEBI RULES)".bold().yellow());
    println!("  {}", subline.dimmed());
    for b in &a.allotment.buckets {
        let odds = b.p_allot.map_or("—".to_string(), |p| {
            format!(
                "{:.2}%{}",
                p * 100.0,
                if b.basis == "lower bound" { "+" } else { "" }
            )
        });
        let apps = b.applications.map_or(String::new(), |n| {
            format!("{} apps", ipo::fmt_int_indian(n))
        });
        println!(
            "  {} {:>12}  {:>3} lots  {:>8}  odds {:<8} {}",
            pad_end(&b.category, 16),
            money(b.min_amount_inr),
            b.min_lots,
            times(b.subscription_x),
            odds.bold(),
            apps.dimmed()
        );
    }
    for line in &a.allotment.capital_odds {
        println!("  {} {}", "→".cyan(), line);
    }
    for rule in &a.allotment.allotment_rules {
        println!("  {} {}", "•".dimmed(), rule.dimmed());
    }

    // ── Regime ──
    println!("\n  {}", "IPO MARKET".bold().yellow());
    println!("  {}", subline.dimmed());
    println!("  {}  {}", tone(&a.regime.label), a.regime.summary.dimmed());

    // ── Street ──
    let b = &a.street_buzz;
    if !b.headlines.is_empty() || b.gmp_pct.is_some() {
        println!("\n  {}", "STREET BUZZ (unofficial)".bold().yellow());
        println!("  {}", subline.dimmed());
        if let Some(p) = b.gmp_pct {
            let inr = b.gmp_inr.map_or(String::new(), |r| format!("₹{r:.0} · "));
            println!(
                "  Reported GMP: {}{:+.1}% (from {} headline(s); not a model input)",
                inr, p, b.gmp_readings
            );
        }
        for h in &b.headlines {
            println!("  • {}  {}", h.title, format!("({})", h.source).dimmed());
        }
        println!("  {}", b.caveat.dimmed().italic());
    }

    // ── Documents ──
    let st = &a.structure;
    let links: Vec<(&str, &String)> = [
        ("Prospectus (RHP)", st.rhp_url.as_ref()),
        ("Anchor allocation", st.anchor_report_url.as_ref()),
        ("Basis of issue price", st.ratios_url.as_ref()),
        ("Allotment status", st.registrar_allotment_url.as_ref()),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|v| (k, v)))
    .collect();
    if !links.is_empty() {
        println!("\n  {}", "DOCUMENTS".bold().yellow());
        println!("  {}", subline.dimmed());
        for (k, url) in links {
            print_kv(k, url);
        }
    }
    for n in &a.notes {
        println!("\n  {} {}", "ⓘ".dimmed(), n.dimmed());
    }

    println!("\n  {}", v.disclaimer.dimmed().italic());
    println!("  {}\n", a.disclaimer.dimmed().italic());
}

// ── ipo review ──

/// Each logged issue's final verdict against its listing day and three months.
pub(crate) async fn ipo_review_report(
    client: &YahooClient,
    since_days: u32,
) -> Result<crate::ipo_log::Review> {
    use futures_util::StreamExt;
    let today = ist_today();
    let since = today
        .checked_sub_days(chrono::Days::new(since_days as u64))
        .ok_or_else(|| anyhow::anyhow!("--since {since_days} days reaches before any date"))?;
    let log = crate::ipo_log::load()
        .context("Could not read the IPO track record (ipo_signal_log.json)")?;
    let finals = crate::ipo_log::final_verdicts(&log, since);
    if finals.is_empty() {
        bail!(
            "No IPO verdicts logged in the last {since_days} days. Every `stockwise ipo` run logs \
             its verdicts; review them after the issues list."
        );
    }
    let earliest = finals
        .iter()
        .filter_map(|e| e.close_date.or(Some(e.date)))
        .min()
        .unwrap_or(since);
    let end = ipo::day_start(today + chrono::Duration::days(1));
    let nifty = client
        .get_chart_between(
            "^NSEI",
            ipo::day_start(earliest - chrono::Duration::days(10)),
            end,
            "1d",
        )
        .await
        .map(|c| ipo::session_bars(&c))
        .unwrap_or_default();
    let jobs: Vec<(String, Option<chrono::NaiveDate>, chrono::NaiveDate)> = finals
        .iter()
        .map(|e| (e.symbol.clone(), e.close_date, e.date))
        .collect();
    let nifty_ref = &nifty;
    let outcomes: std::collections::HashMap<String, crate::ipo_log::Outcome> =
        futures_util::stream::iter(jobs)
            .map(|(sym, close, logged)| async move {
                let from = close.unwrap_or(logged);
                let bars = client
                    .get_chart_between(
                        &format!("{sym}.NS"),
                        ipo::day_start(from - chrono::Duration::days(1)),
                        end,
                        "1d",
                    )
                    .await
                    .map(|c| ipo::session_bars(&c))
                    .unwrap_or_default();
                let o = crate::ipo_log::outcome_from_bars(&bars, nifty_ref, close);
                (sym, o)
            })
            .buffer_unordered(6)
            .collect()
            .await;
    Ok(crate::ipo_log::review(&finals, &outcomes, since))
}

pub async fn cmd_ipo_review(since_days: u32, fmt: Format) -> Result<()> {
    let client = YahooClient::new().await?;
    let r = ipo_review_report(&client, since_days).await?;
    match fmt {
        Format::Csv => return output::print_csv(&r.rows),
        Format::Json => return output::print_json(&r),
        _ => {}
    }
    print_header(&format!(
        "IPO Verdict Track Record — since {}",
        r.since.format("%d %b %Y")
    ));
    println!(
        "  {} each issue's last verdict before close; listing-day close and ~3 months (63 sessions) vs the issue price, against {}",
        "ⓘ".dimmed(),
        r.benchmark
    );
    let pct = |v: Option<f64>| v.map_or("–".to_string(), |v| format!("{v:+.1}%"));
    let share = |v: Option<f64>| v.map_or("–".to_string(), |v| format!("{:.0}%", v * 100.0));
    println!(
        "\n  {} {:>6} {:>6} {:>8} {:>10} {:>10} {:>10} {:>6} {:>10} {:>10}",
        pad_end("Verdict", 24).bold(),
        "Issues".bold(),
        "Listed".bold(),
        "Hit rate".bold(),
        "Expected".bold(),
        "Day 1".bold(),
        "Median".bold(),
        "3m n".bold(),
        "3m".bold(),
        "vs NIFTY".bold()
    );
    println!("  {}", "─".repeat(108).dimmed());
    for b in &r.buckets {
        let label = VKind::parse(&b.verdict).map_or(b.verdict.as_str(), |k| k.label());
        println!(
            "  {} {:>6} {:>6} {:>8} {:>10} {:>10} {:>10} {:>6} {:>10} {:>10}",
            pad_end(label, 24),
            b.issues,
            b.listed,
            share(b.hit_rate),
            pct(b.mean_expected_pct),
            pct(b.mean_listing_pct),
            pct(b.median_listing_pct),
            b.with_3m,
            pct(b.mean_3m_pct),
            pct(b.mean_excess_3m_pct)
        );
    }
    println!();
    println!(
        "  {:<14} {:<24} {:>11} {:>9} {:>9} {:>9} {:>9}",
        "Symbol".bold(),
        "Verdict".bold(),
        "Listed".bold(),
        "Expected".bold(),
        "Day 1".bold(),
        "3m".bold(),
        "vs NIFTY".bold()
    );
    for row in &r.rows {
        let label = VKind::parse(&row.verdict).map_or(row.verdict.as_str(), |k| k.label());
        println!(
            "  {:<14} {:<24} {:>11} {:>9} {:>9} {:>9} {:>9}",
            row.symbol.cyan(),
            label,
            row.listing_date
                .map_or("pending".to_string(), |d| d.format("%d %b %Y").to_string()),
            pct(row.expected_gain_pct),
            pct(row.listing_return_pct),
            pct(row.return_3m_pct),
            pct(row.excess_3m_pct)
        );
    }
    if !r.pending.is_empty() {
        println!(
            "\n  {} not listed yet or no prices: {}",
            "ⓘ".dimmed(),
            r.pending.join(", ")
        );
    }
    println!(
        "  {} methodology: {}\n",
        "ⓘ".dimmed(),
        r.methodologies.join(", ")
    );
    Ok(())
}

// ── ipo calibrate ──

pub struct CalibrateOptions {
    pub csv: Option<std::path::PathBuf>,
    pub since: chrono::NaiveDate,
    pub save: bool,
    pub print_const: bool,
}

/// Build the calibration rows from NSE (past issues, then each issue's
/// final subscription) and Yahoo (listing-day open and close).
async fn calibration_rows_live(
    client: &YahooClient,
    since: chrono::NaiveDate,
) -> Result<(Vec<crate::ipo_model::Sample>, Vec<String>)> {
    let session = crate::providers::nse::shared_session();
    let past = session
        .get_json(
            "/api/public-past-issues",
            &[],
            std::time::Duration::from_secs(3600),
        )
        .await
        .map_err(|e| anyhow::anyhow!("NSE's past-issues page is not answering: {e}"))?;
    let today = ist_today();
    let rows: Vec<IpoSummaryRow> = ipo::parse_past_issues(&past, today, 5000)
        .into_iter()
        .filter(|r| {
            r.board == BoardKind::Mainboard
                && r.listing_date.is_some_and(|d| d >= since && d < today)
                && r.issue_price.is_some()
        })
        .collect();
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for (n, r) in rows.iter().enumerate() {
        eprint!("\r  {}/{} {}          ", n + 1, rows.len(), r.symbol);
        let detail = session
            .get_json(
                "/api/ipo-detail",
                &[("symbol", r.symbol.as_str()), ("series", "EQ")],
                std::time::Duration::from_secs(86_400),
            )
            .await;
        let Ok(detail) = detail else {
            skipped.push(format!("{}: no NSE issue detail", r.symbol));
            continue;
        };
        let sub = ipo::parse_subscription(&detail);
        let (Some(q), Some(nii), Some(ret)) = (sub.qib_x, sub.nii_x, sub.retail_x) else {
            skipped.push(format!("{}: no category subscription", r.symbol));
            continue;
        };
        out.push(crate::ipo_model::Sample {
            symbol: r.symbol.clone(),
            listing_date: r.listing_date.unwrap_or(today),
            issue_price: r.issue_price.unwrap_or(0.0),
            qib_x: q,
            nii_x: nii,
            retail_x: ret,
            listing_open: 0.0,
            listing_close: 0.0,
        });
    }
    eprintln!();
    fill_listing_prices(client, &mut out, &mut skipped).await;
    Ok((out, skipped))
}

/// Listing-day open and close from Yahoo for rows that lack them.
async fn fill_listing_prices(
    client: &YahooClient,
    rows: &mut [crate::ipo_model::Sample],
    skipped: &mut Vec<String>,
) {
    use futures_util::StreamExt;
    let jobs: Vec<(usize, String, chrono::NaiveDate)> = rows
        .iter()
        .enumerate()
        .filter(|(_, s)| s.listing_open <= 0.0 || s.listing_close <= 0.0)
        .map(|(i, s)| (i, format!("{}.NS", s.symbol), s.listing_date))
        .collect();
    let bars: Vec<(usize, Option<(chrono::NaiveDate, f64, f64)>)> =
        futures_util::stream::iter(jobs)
            .map(|(i, sym, ld)| async move {
                let bar = client
                    .get_chart_between(
                        &sym,
                        ipo::day_start(ld - chrono::Duration::days(1)),
                        ipo::day_start(ld + chrono::Duration::days(8)),
                        "1d",
                    )
                    .await
                    .ok()
                    .and_then(|c| ipo::listing_bar(&c, ld));
                (i, bar)
            })
            .buffer_unordered(4)
            .collect()
            .await;
    for (i, bar) in bars {
        match bar {
            // A first bar days after the listing date means the symbol
            // changed or Yahoo lacks the day: leave it out.
            Some((d, o, c)) if (d - rows[i].listing_date).num_days() <= 3 => {
                rows[i].listing_open = o;
                rows[i].listing_close = c;
            }
            _ => skipped.push(format!("{}: no listing-day bar on Yahoo", rows[i].symbol)),
        }
    }
}

pub async fn cmd_ipo_calibrate(opts: CalibrateOptions, fmt: Format) -> Result<()> {
    let client = YahooClient::new().await?;
    let (mut rows, mut skipped) = match &opts.csv {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("Could not read {}", path.display()))?;
            let (rows, errors) = crate::ipo_model::samples_from_csv(&text);
            if rows.is_empty() {
                bail!(
                    "No usable rows in {}: {}",
                    path.display(),
                    errors.join("; ")
                );
            }
            (rows, errors)
        }
        None => calibration_rows_live(&client, opts.since).await?,
    };
    if opts.csv.is_some() {
        fill_listing_prices(&client, &mut rows, &mut skipped).await;
    }
    rows.retain(|s| s.listing_date >= opts.since);
    let today = ist_today();
    let model = crate::ipo_model::fit(&rows, true, today).map_err(|e| anyhow::anyhow!("{e}"))?;
    let ols = crate::ipo_model::fit(&rows, false, today).ok();
    let dataset = crate::ipo_model::samples_to_csv(&rows);
    if let Ok(dir) = crate::store::data_dir() {
        let path = dir.join("ipo_calibration.csv");
        if std::fs::write(&path, &dataset).is_ok() {
            eprintln!("  dataset: {}", path.display());
        }
    }
    if opts.save {
        crate::store::save(crate::ipo_model::USER_FILE, &Some(model.clone()), false)?;
        eprintln!(
            "  saved {} in the data directory; verdicts use it from now on",
            crate::ipo_model::USER_FILE
        );
    }
    let calibration = crate::ipo_model::loss_calibration(&model, &rows);
    if fmt == Format::Json {
        return output::print_json(&serde_json::json!({
            "model": model,
            "ols": ols,
            "p_loss_calibration": calibration
                .iter()
                .map(|(band, n, share)| serde_json::json!({"predicted": band, "issues": n, "closed_below_issue": share}))
                .collect::<Vec<_>>(),
            "skipped": skipped,
        }));
    }
    if opts.print_const {
        println!("{}", crate::ipo_model::as_const(&model));
        return Ok(());
    }
    print_header("IPO listing-day model fit");
    print_kv("Issues", &model.n.to_string());
    print_kv(
        "Listings",
        &format!("{} to {}", model.sample_from, model.sample_to),
    );
    print_kv("Method", "Huber robust regression (k = 1.345)");
    print_kv(
        "Coefficients",
        &format!(
            "{:+.2} {:+.2}·ln(1+QIB) {:+.2}·ln(1+NII) {:+.2}·ln(1+retail) {:+.3}·regime",
            model.intercept, model.b_qib, model.b_nii, model.b_retail, model.b_regime
        ),
    );
    print_kv("R²", &format!("{:.3}", model.r2));
    print_kv("Residual sd", &format!("{:.1} pts", model.resid_sd));
    if let Some(o) = &ols {
        print_kv(
            "OLS for comparison",
            &format!(
                "{:+.2} {:+.2} {:+.2} {:+.2} {:+.3} · R² {:.3}",
                o.intercept, o.b_qib, o.b_nii, o.b_retail, o.b_regime, o.r2
            ),
        );
    }
    println!("\n  P(loss) check (in sample): predicted band → share that closed below issue");
    for (band, n, share) in &calibration {
        println!(
            "    {:<8} {:>4} issues  {}",
            band,
            n,
            share.map_or("–".to_string(), |s| format!("{:.0}%", s * 100.0))
        );
    }
    if !skipped.is_empty() {
        println!(
            "\n  {} left out: {}",
            skipped.len(),
            skipped.join("; ").dimmed()
        );
    }
    println!();
    Ok(())
}
