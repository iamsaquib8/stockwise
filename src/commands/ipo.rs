//! `stockwise ipo`: the live Indian IPO board, and a deep analysis with
//! demand scores and SEBI allotment odds for one issue. The data and the
//! scoring live in `crate::ipo`; this file only prints.

use super::*;
use crate::ipo::{
    self, BoardFilter, BoardKind, IpoAnalysis, IpoBoardView, IpoStatus, IpoSummaryRow,
};
use colored::ColoredString;

pub async fn cmd_ipo(
    query: Option<&str>,
    board: BoardFilter,
    capital: Option<f64>,
    recent: usize,
    fmt: Format,
) -> Result<()> {
    let client = YahooClient::new().await?;
    match query {
        None => {
            let view = ipo::fetch_board(&client, board, recent).await?;
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
            let analysis = ipo::fetch_analysis(&client, q, series, capital).await?;
            if fmt == Format::Json {
                return output::print_json(&analysis);
            }
            print_ipo_analysis(&analysis);
        }
    }
    Ok(())
}

/// `4.20x`, or a dash when the number is unknown.
fn times(v: Option<f64>) -> String {
    v.map_or("—".to_string(), |x| format!("{x:.2}x"))
}

/// Colours a label by what it describes: red for weak or negative readings,
/// yellow for mixed, pending or in-between ones, green for the rest. The colour
/// describes the data; it is not an instruction.
fn tone(text: &str) -> ColoredString {
    let t = text.to_ascii_uppercase();
    if t.contains("WEAK")
        || t.contains("COLD")
        || t.contains("BELOW")
        || t.contains("TEPID")
        || t.contains("NEGATIVE")
        || t.contains("EXIT")
        || t.contains("HEAVY")
    {
        text.red().bold()
    } else if t.contains("BOOK")
        || t.contains("MIXED")
        || t.contains("SELECTIVE")
        || t.contains("NEUTRAL")
        || t.contains("SME")
        || t.contains("UPCOMING")
        || t.contains("MODERATE")
        || t.contains("PENDING")
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
    println!(
        "  {} {}  {}",
        r.symbol.bold(),
        board_badge(r.board, r.is_bse_sme),
        r.company_name
    );
    println!("      {}  ·  {}", terms_line(r), r.window_note.dimmed());
    if r.status == IpoStatus::Ongoing && r.total_x.is_some() {
        println!(
            "      QIB {}  NII {}  Retail {}  Total {}   → {}",
            times(r.qib_x),
            times(r.nii_x),
            times(r.retail_x),
            times(r.total_x).bold(),
            tone(&r.quick_signal)
        );
    } else if r.status == IpoStatus::Ongoing {
        println!("      → {}", tone(&r.quick_signal));
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
    let ret = match r.return_from_issue_pct {
        Some(p) if p >= 0.0 => format!("{p:+.1}%").green().to_string(),
        Some(p) => format!("{p:+.1}%").red().to_string(),
        None => "—".dimmed().to_string(),
    };
    println!(
        "  {} {} {} {} {} {}",
        pad_end(&r.symbol, 12).bold(),
        pad_end(&format!("{}", board_badge(r.board, r.is_bse_sme)), 14),
        pad_end(&r.window_note, 24).dimmed(),
        pad_start(&issue, 9),
        pad_start(&now, 9),
        pad_start(&ret, 8)
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
            "  {} {} {} {} {} {}",
            pad_end("Symbol", 12).dimmed(),
            pad_end("Board", 14).dimmed(),
            pad_end("Status", 24).dimmed(),
            pad_start("Issue", 9).dimmed(),
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

    println!(
        "\n  {} Subscription detail, allotment odds and demand scores: '{}'\n",
        "💡".yellow(),
        "stockwise ipo <SYMBOL> [--capital 2.5L]".cyan()
    );
}

pub fn print_ipo_analysis(a: &IpoAnalysis) {
    let border = "═".repeat(64);
    let subline = "─".repeat(64);

    println!("\n{}", border.cyan());
    println!(
        "  IPO ANALYSIS: {} {} ({})",
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

    // ── Scores ──
    let v = &a.verdict;
    println!("\n  LISTING DEMAND:        {}", tone(&v.listing_demand));
    println!("  LONG-TERM PROFILE:     {}", tone(&v.long_term_profile));
    println!(
        "  IPO Score:             {:.0}/100  {}  (Data: {})",
        v.overall_score,
        rating_bar(v.overall_score, 100.0),
        v.confidence
    );
    println!("  Timeline:              {}", v.timeline.italic());

    // ── Terms ──
    println!("\n  {}", "ISSUE TERMS".bold().yellow());
    println!("  {}", subline.dimmed());
    print_kv("Price band", &a.price_band.display_short());
    if let Some(lot) = a.lot_size {
        print_kv("Lot size", &format!("{lot} shares"));
    }
    if let Some(min) = a.min_investment_inr {
        print_kv("Minimum application", &format!("₹{}", ipo::fmt_inr(min)));
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

    // ── Pillars ──
    println!("\n  {}", "6-PILLAR IPO SCORING".bold().yellow());
    println!("  {}", subline.dimmed());
    for p in &v.pillars {
        println!(
            "  {} {:>3}%  {}  {:>3.0}  {}",
            pad_end(&p.name, 31),
            p.weight_pct,
            rating_bar(p.score, 100.0),
            p.score,
            tone(&p.status)
        );
        println!("      {}", p.summary.dimmed());
    }

    if !v.bull_points.is_empty() {
        println!("\n  {}", "POSITIVE FACTORS".bold().green());
        for b in &v.bull_points {
            println!("  {} {}", "▲".green(), b);
        }
    }
    if !v.risk_flags.is_empty() {
        println!("\n  {}", "RED FLAGS".bold().red());
        for r in &v.risk_flags {
            println!("  {} {}", "▼".red(), r);
        }
    }

    // ── Allotment ──
    println!("\n  {}", "ALLOTMENT ODDS (SEBI RULES)".bold().yellow());
    println!("  {}", subline.dimmed());
    for b in &a.allotment.buckets {
        let odds = b
            .estimated_win_prob_pct
            .map_or("—".to_string(), |p| format!("{p:.1}%"));
        println!(
            "  {} ₹{:>10}  {:>3} lots  {:>8}  odds {}",
            pad_end(&b.bucket, 30),
            ipo::fmt_inr(b.amount_inr),
            b.lots_required,
            times(b.subscription_x),
            odds.bold()
        );
    }
    for line in &a.allotment.capital_odds {
        println!("  {} {}", "→".cyan(), line);
    }
    for rule in &a.allotment.allotment_rules {
        println!("  {} {}", "•".dimmed(), rule.dimmed());
    }

    // ── Street ──
    let b = &a.street_buzz;
    if !b.headlines.is_empty() {
        println!("\n  {}", "STREET BUZZ (unofficial)".bold().yellow());
        println!("  {}", subline.dimmed());
        if let Some(p) = b.gmp_pct {
            let inr = b.gmp_inr.map_or(String::new(), |r| format!("₹{r:.0} · "));
            println!("  Reported GMP: {}{:+.1}%", inr, p);
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

    println!("\n  {}\n", a.disclaimer.dimmed().italic());
}
