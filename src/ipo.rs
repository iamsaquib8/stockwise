//! Indian IPO intelligence: live ongoing & upcoming board, post-listing regime
//! tracker, consolidated NSE + BSE subscription & price-band demand book,
//! SEBI allotment odds, and a 6-pillar score that describes listing-day demand
//! and the long-term profile of an issue. Nothing here tells the reader to
//! apply for or avoid an issue.
//!
//! Primary data comes from NSE's public IPO endpoints via [`nse::shared_session`]
//! (`/api/ipo-current-issue`, `/api/all-upcoming-issues?category=ipo`,
//! `/api/public-past-issues`, and `/api/ipo-detail?symbol=X&series=EQ|SME`),
//! enriched with live quotes for post-listing performance and Indian financial
//! news RSS for street consensus / unofficial GMP chatter.

use crate::api::{Quote, YahooClient};
use crate::output::round;
use crate::providers::nse;
use crate::providers::session::{FetchError, Session};
use anyhow::{Result, bail};
use chrono::{Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

const BOARD_TTL: Duration = Duration::from_secs(120);
const DETAIL_TTL: Duration = Duration::from_secs(90);
const PAST_TTL: Duration = Duration::from_secs(600);

/// Which board(s) to include on `stockwise ipo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BoardFilter {
    #[default]
    All,
    Mainboard,
    Sme,
}

impl BoardFilter {
    pub fn matches(self, kind: BoardKind) -> bool {
        match self {
            Self::All => true,
            Self::Mainboard => kind == BoardKind::Mainboard,
            Self::Sme => kind == BoardKind::Sme,
        }
    }

    pub fn from_name(s: Option<&str>) -> Self {
        match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("mainboard" | "main" | "eq") => Self::Mainboard,
            Some("sme" | "emerge") => Self::Sme,
            _ => Self::All,
        }
    }
}

/// Mainboard (`EQ`) vs SME (`SME` — NSE Emerge or BSE SME).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoardKind {
    Mainboard,
    Sme,
}

impl BoardKind {
    pub fn from_series(series: &str) -> Self {
        if series.trim().eq_ignore_ascii_case("SME") || series.trim().eq_ignore_ascii_case("SM") {
            Self::Sme
        } else {
            Self::Mainboard
        }
    }

    pub fn series_code(self) -> &'static str {
        match self {
            Self::Mainboard => "EQ",
            Self::Sme => "SME",
        }
    }

    pub fn label(self, is_bse_sme: bool) -> &'static str {
        match (self, is_bse_sme) {
            (Self::Mainboard, _) => "MAINBOARD",
            (Self::Sme, true) => "BSE SME",
            (Self::Sme, false) => "NSE SME",
        }
    }
}

/// Lifecycle stage of an Indian IPO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpoStatus {
    Ongoing,
    Upcoming,
    ClosedPendingListing,
    Listed,
}

impl IpoStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ongoing => "OPEN",
            Self::Upcoming => "UPCOMING",
            Self::ClosedPendingListing => "ALLOTMENT / LISTING PENDING",
            Self::Listed => "LISTED",
        }
    }
}

/// Floor and cap price of a book-built or fixed-price IPO.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PriceBand {
    pub low: Option<f64>,
    pub high: Option<f64>,
    pub raw: String,
}

impl PriceBand {
    pub fn cap_price(&self) -> Option<f64> {
        self.high.or(self.low)
    }

    pub fn display_short(&self) -> String {
        match (self.low, self.high) {
            (Some(lo), Some(hi)) if (hi - lo).abs() > 0.005 => {
                format!("₹{}–₹{}", fmt_price(lo), fmt_price(hi))
            }
            (Some(p), _) | (_, Some(p)) => format!("₹{}", fmt_price(p)),
            (None, None) if !self.raw.trim().is_empty() && self.raw.trim() != "-" => {
                self.raw.trim().to_string()
            }
            (None, None) => "TBA".to_string(),
        }
    }
}

/// One row on the `stockwise ipo` board (ongoing, upcoming, or recent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoSummaryRow {
    pub symbol: String,
    pub company_name: String,
    pub board: BoardKind,
    pub is_bse_sme: bool,
    pub status: IpoStatus,
    pub open_date: Option<NaiveDate>,
    pub close_date: Option<NaiveDate>,
    pub listing_date: Option<NaiveDate>,
    pub window_note: String,
    pub price_band: PriceBand,
    pub issue_price: Option<f64>,
    pub lot_size: Option<u64>,
    pub min_investment_inr: Option<f64>,
    pub issue_size_cr: Option<f64>,
    pub qib_x: Option<f64>,
    pub nii_x: Option<f64>,
    pub retail_x: Option<f64>,
    pub total_x: Option<f64>,
    pub ltp: Option<f64>,
    pub return_from_issue_pct: Option<f64>,
    pub quick_signal: String,
}

/// IPO market sentiment & post-listing performance regime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoRegime {
    pub label: String,
    pub sample_size: usize,
    pub positive_count: usize,
    pub hit_rate_pct: f64,
    pub avg_return_pct: f64,
    pub median_return_pct: f64,
    pub best_recent: Option<(String, f64)>,
    pub worst_recent: Option<(String, f64)>,
    pub nifty_change_pct: Option<f64>,
    pub india_vix: Option<f64>,
    pub summary: String,
}

/// News headline around Indian IPOs or a specific issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoHeadline {
    pub title: String,
    pub source: String,
}

/// Full response of `stockwise ipo` / `ipo_board`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoBoardView {
    pub as_of: NaiveDate,
    pub filter: BoardFilter,
    pub regime: IpoRegime,
    pub ongoing: Vec<IpoSummaryRow>,
    pub upcoming: Vec<IpoSummaryRow>,
    pub recent: Vec<IpoSummaryRow>,
    pub headlines: Vec<IpoHeadline>,
    pub notes: Vec<String>,
}

/// Subscription row for one investor category (`QIB`, `Mutual funds`, `bNII`, `sNII`, `RII`, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySubscription {
    pub sr_no: String,
    pub category: String,
    pub offered_shares: Option<u64>,
    pub bid_shares: Option<u64>,
    pub times_subscribed: Option<f64>,
    pub applications: Option<u64>,
}

/// Consolidated category-level subscription summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubscriptionBreakdown {
    pub source_note: String,
    pub total_offered: Option<u64>,
    pub total_bids: Option<u64>,
    pub total_x: Option<f64>,
    pub qib_x: Option<f64>,
    pub fii_bids: Option<u64>,
    pub mf_bids: Option<u64>,
    pub dfi_bids: Option<u64>,
    pub mf_share_of_qib_pct: Option<f64>,
    pub nii_x: Option<f64>,
    pub bnii_x: Option<f64>,
    pub snii_x: Option<f64>,
    pub retail_x: Option<f64>,
    pub retail_cutoff_bids: Option<u64>,
    pub retail_price_bids: Option<u64>,
    pub retail_cutoff_pct: Option<f64>,
    pub employee_x: Option<f64>,
    pub shareholder_x: Option<f64>,
    pub categories: Vec<CategorySubscription>,
}

/// Cumulative bid demand at a single price point in the book-building band.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceDemandStep {
    pub label: String,
    pub price: Option<f64>,
    pub cumulative_shares: u64,
    pub pct_of_total_bids: f64,
}

/// Parsed Fresh Issue vs Offer For Sale (OFS) & reservation split.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueStructure {
    pub raw_issue_size: String,
    pub total_issue_cr: Option<f64>,
    pub fresh_issue_cr: Option<f64>,
    pub ofs_cr: Option<f64>,
    pub fresh_pct: Option<f64>,
    pub anchor_shares: Option<u64>,
    pub market_maker_shares: Option<u64>,
    pub discount: Option<String>,
    pub issue_type: Option<String>,
    pub face_value: Option<String>,
    pub brlms: Vec<String>,
    pub sponsor_bank: Option<String>,
    pub upi_cutoff: Option<String>,
    pub registrar_name: Option<String>,
    pub registrar_allotment_url: Option<String>,
    pub rhp_url: Option<String>,
    pub anchor_report_url: Option<String>,
    pub ratios_url: Option<String>,
    pub structure_tag: String,
}

/// Unofficial press / street consensus signals extracted from news RSS.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreetBuzz {
    pub gmp_inr: Option<f64>,
    pub gmp_pct: Option<f64>,
    pub subscribe_mentions: usize,
    pub avoid_mentions: usize,
    pub neutral_mentions: usize,
    pub headlines: Vec<IpoHeadline>,
    pub caveat: String,
}

/// SEBI allotment odds for one application tier (`Retail`, `sNII`, `bNII`, or `SME Individual`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllotmentBucketOdds {
    pub bucket: String,
    pub lots_required: u64,
    pub shares_required: u64,
    pub amount_inr: f64,
    pub subscription_x: Option<f64>,
    pub allotment_packet_lots: u64,
    pub allotment_packet_inr: f64,
    pub estimated_win_prob_pct: Option<f64>,
    pub rule_note: String,
}

/// SEBI allotment odds by application tier, and the odds for the amount given
/// with `--capital`. Probability maths, not a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllotmentOptimizer {
    pub buckets: Vec<AllotmentBucketOdds>,
    pub capital_input_inr: Option<f64>,
    /// What the `--capital` amount buys and the odds at that size.
    pub capital_odds: Vec<String>,
    /// How allotment works under SEBI's rules.
    pub allotment_rules: Vec<String>,
}

/// One scored pillar of the 6-pillar Indian IPO Verdict Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoPillarScore {
    pub name: String,
    pub weight_pct: u8,
    pub score: f64,
    pub status: String,
    pub summary: String,
}

/// Score of one IPO: the demand seen so far, the long-term profile of the
/// issue, six pillar scores and the facts behind them. Not a recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoDualVerdict {
    pub overall_score: f64,
    /// Describes bids so far ("Strong demand", "Weak demand", ...).
    pub listing_demand: String,
    /// Describes institutional demand and issue structure.
    pub long_term_profile: String,
    /// How complete the data behind the score is.
    pub confidence: String,
    /// Dates and what is known at this stage of the issue.
    pub timeline: String,
    pub pillars: Vec<IpoPillarScore>,
    pub bull_points: Vec<String>,
    pub risk_flags: Vec<String>,
}

/// Complete per-IPO analysis returned by `stockwise ipo <SYMBOL>` and `ipo_analyze`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoAnalysis {
    pub symbol: String,
    pub company_name: String,
    pub industry: Option<String>,
    pub isin: Option<String>,
    pub board: BoardKind,
    pub is_bse_sme: bool,
    pub status: IpoStatus,
    pub open_date: Option<NaiveDate>,
    pub close_date: Option<NaiveDate>,
    pub listing_date: Option<NaiveDate>,
    pub window_note: String,
    pub price_band: PriceBand,
    pub issue_price: Option<f64>,
    pub lot_size: Option<u64>,
    pub min_investment_inr: Option<f64>,
    pub ltp: Option<f64>,
    pub return_from_issue_pct: Option<f64>,
    pub structure: IssueStructure,
    pub subscription: SubscriptionBreakdown,
    pub demand_ladder: Vec<PriceDemandStep>,
    pub allotment: AllotmentOptimizer,
    pub street_buzz: StreetBuzz,
    pub regime: IpoRegime,
    pub verdict: IpoDualVerdict,
    pub disclaimer: String,
}

// ── Live Fetching ──

/// Fetch the live Indian IPO board (ongoing, upcoming, and recent listings).
pub async fn fetch_board(
    yahoo: &YahooClient,
    filter: BoardFilter,
    past_limit: usize,
) -> Result<IpoBoardView> {
    let session = nse::shared_session();
    let today = ist_today();
    let year = today.year();
    let news_query = format!("India IPO GMP subscription NSE BSE {year}");

    let (curr_res, up_res, past_res, rss_xml) = tokio::join!(
        session.get_json("/api/ipo-current-issue", &[], BOARD_TTL),
        session.get_json(
            "/api/all-upcoming-issues",
            &[("category", "ipo")],
            BOARD_TTL
        ),
        session.get_json("/api/public-past-issues", &[], PAST_TTL),
        fetch_india_news_rss(yahoo, &news_query),
    );

    let mut notes = Vec::new();
    // NSE's IPO pages are the only source for the board. When it refuses or is
    // down, say so once and still show the press headlines.
    if let (Err(e), Err(_), Err(_)) = (&curr_res, &up_res, &past_res) {
        notes.push(format!(
            "NSE's IPO pages are not answering ({e}); showing news headlines only. Try again later."
        ));
    }
    let feed = |res: Result<Value, FetchError>, what: &str, notes: &mut Vec<String>| match res {
        Ok(v) => v,
        Err(e) => {
            if notes.is_empty() {
                notes.push(format!("{what} IPO feed unavailable: {e}"));
            }
            Value::Array(vec![])
        }
    };
    let curr_val = feed(curr_res, "Ongoing", &mut notes);
    let up_val = feed(up_res, "Upcoming", &mut notes);
    let past_val = feed(past_res, "Past", &mut notes);

    let (mut ongoing, upcoming) = merge_current_and_upcoming(&curr_val, &up_val, today);

    // Enrich ongoing Mainboard/top issues (up to 6) with consolidated QIB/NII/Retail breakdown
    // from `/api/ipo-detail` so the board table shows exact category multiples.
    enrich_ongoing_rows(&session, &mut ongoing, today).await;

    let mut recent = parse_past_issues(&past_val, today, past_limit.max(10));

    // Fetch live quotes for recently listed Mainboard/NSE issues + NIFTY 50 & INDIA VIX in one batch.
    let mut quote_syms: Vec<String> = vec!["^NSEI".to_string(), "^INDIAVIX".to_string()];
    for row in &recent {
        if row.status == IpoStatus::Listed && !row.symbol.is_empty() {
            quote_syms.push(format!("{}.NS", row.symbol));
        }
    }
    let quote_refs: Vec<&str> = quote_syms.iter().map(String::as_str).collect();
    let quotes = yahoo.get_quote(&quote_refs).await.unwrap_or_default();
    apply_quotes_to_recent(&mut recent, &quotes);
    let regime = compute_regime(&recent, &quotes);

    let ongoing: Vec<IpoSummaryRow> = ongoing
        .into_iter()
        .filter(|r| filter.matches(r.board))
        .collect();
    let upcoming: Vec<IpoSummaryRow> = upcoming
        .into_iter()
        .filter(|r| filter.matches(r.board))
        .collect();
    let recent: Vec<IpoSummaryRow> = recent
        .into_iter()
        .filter(|r| filter.matches(r.board))
        .take(past_limit)
        .collect();

    let headlines = parse_rss_headlines(&rss_xml, 6);

    Ok(IpoBoardView {
        as_of: today,
        filter,
        regime,
        ongoing,
        upcoming,
        recent,
        headlines,
        notes,
    })
}

/// Fetch deep 6-pillar IPO analysis for a symbol or company-name query.
pub async fn fetch_analysis(
    yahoo: &YahooClient,
    query: &str,
    forced_series: Option<&str>,
    capital: Option<f64>,
) -> Result<IpoAnalysis> {
    let session = nse::shared_session();
    let today = ist_today();

    let (curr_res, up_res, past_res) = tokio::join!(
        session.get_json("/api/ipo-current-issue", &[], BOARD_TTL),
        session.get_json(
            "/api/all-upcoming-issues",
            &[("category", "ipo")],
            BOARD_TTL
        ),
        session.get_json("/api/public-past-issues", &[], PAST_TTL),
    );

    let curr_val = curr_res.unwrap_or(Value::Array(vec![]));
    let up_val = up_res.unwrap_or(Value::Array(vec![]));
    let past_val = past_res.unwrap_or(Value::Array(vec![]));

    let (ongoing, upcoming) = merge_current_and_upcoming(&curr_val, &up_val, today);
    let all_past = parse_past_issues(&past_val, today, 200);

    let matched = resolve_ipo_target(query, forced_series, &ongoing, &upcoming, &all_past);
    let (symbol, mut base_row) = match matched {
        Some(pair) => pair,
        None => {
            let sym = query
                .trim()
                .trim_end_matches(".NS")
                .trim_end_matches(".BO")
                .to_ascii_uppercase();
            let board = forced_series
                .map(BoardKind::from_series)
                .unwrap_or(BoardKind::Mainboard);
            (
                sym.clone(),
                IpoSummaryRow {
                    symbol: sym.clone(),
                    company_name: sym,
                    board,
                    is_bse_sme: false,
                    status: IpoStatus::Ongoing,
                    open_date: None,
                    close_date: None,
                    listing_date: None,
                    window_note: String::new(),
                    price_band: PriceBand::default(),
                    issue_price: None,
                    lot_size: None,
                    min_investment_inr: None,
                    issue_size_cr: None,
                    qib_x: None,
                    nii_x: None,
                    retail_x: None,
                    total_x: None,
                    ltp: None,
                    return_from_issue_pct: None,
                    quick_signal: String::new(),
                },
            )
        }
    };

    // Fetch `/api/ipo-detail`; if series wasn't forced and EQ fails/is empty, try SME too.
    let series_try = forced_series
        .map(|s| vec![s.to_ascii_uppercase()])
        .unwrap_or_else(|| {
            let primary = base_row.board.series_code().to_string();
            let alt = if primary == "EQ" { "SME" } else { "EQ" }.to_string();
            vec![primary, alt]
        });

    let mut detail_json: Option<Value> = None;
    for ser in &series_try {
        if let Ok(v) = session
            .get_json(
                "/api/ipo-detail",
                &[("symbol", symbol.as_str()), ("series", ser.as_str())],
                DETAIL_TTL,
            )
            .await
            && (v.get("issueInfo").is_some() || v.get("bidDetails").is_some())
        {
            base_row.board = BoardKind::from_series(ser);
            detail_json = Some(v);
            break;
        }
    }

    if detail_json.is_none()
        && base_row.open_date.is_none()
        && base_row.price_band.cap_price().is_none()
    {
        bail!(
            "No Indian IPO found matching '{}'. Run `stockwise ipo` to see active, upcoming, and recent NSE/BSE IPOs.",
            query.trim()
        );
    }

    // Fetch recent regime quotes + target live quote (if listed) + news buzz concurrently.
    let mut recent_for_regime = parse_past_issues(&past_val, today, 14);
    let mut quote_syms: Vec<String> = vec!["^NSEI".to_string(), "^INDIAVIX".to_string()];
    for r in &recent_for_regime {
        if r.status == IpoStatus::Listed && !r.symbol.is_empty() {
            quote_syms.push(format!("{}.NS", r.symbol));
        }
    }
    let ns_sym = format!("{symbol}.NS");
    if !quote_syms.contains(&ns_sym) {
        quote_syms.push(ns_sym.clone());
    }

    let search_name = clean_company_for_search(&base_row.company_name, &symbol);
    let buzz_query = format!("\"{search_name}\" IPO GMP OR subscribe OR avoid");

    let quote_refs: Vec<&str> = quote_syms.iter().map(String::as_str).collect();
    let (quotes_res, rss_xml) = tokio::join!(
        yahoo.get_quote(&quote_refs),
        fetch_india_news_rss(yahoo, &buzz_query),
    );
    let quotes = quotes_res.unwrap_or_default();
    apply_quotes_to_recent(&mut recent_for_regime, &quotes);
    let regime = compute_regime(&recent_for_regime, &quotes);

    if let Some(q) = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some(ns_sym.as_str()))
        && let Some(price) = q.regular_market_price
        && price > 0.0
    {
        base_row.ltp = Some(round(price, 2));
        base_row.status = IpoStatus::Listed;
        if let Some(iss) = base_row
            .issue_price
            .or_else(|| base_row.price_band.cap_price())
            && iss > 0.0
        {
            base_row.return_from_issue_pct = Some(round((price / iss - 1.0) * 100.0, 2));
        }
    }

    Ok(build_analysis_from_parts(
        base_row,
        detail_json.as_ref(),
        &rss_xml,
        regime,
        capital,
        today,
    ))
}

async fn enrich_ongoing_rows(session: &Session, ongoing: &mut [IpoSummaryRow], today: NaiveDate) {
    // Politely enrich up to 6 ongoing issues so the board shows exact QIB / NII / Retail x.
    for row in ongoing.iter_mut().take(6) {
        let ser = row.board.series_code();
        if let Ok(detail) = session
            .get_json(
                "/api/ipo-detail",
                &[("symbol", row.symbol.as_str()), ("series", ser)],
                DETAIL_TTL,
            )
            .await
        {
            let sub = parse_subscription(&detail);
            if sub.total_x.is_some() {
                row.total_x = sub.total_x;
            }
            row.qib_x = sub.qib_x;
            row.nii_x = sub.nii_x;
            row.retail_x = sub.retail_x;

            let (lot_opt, band_opt, structure) = parse_issue_info(&detail, &row.price_band);
            if row.lot_size.is_none() {
                row.lot_size = lot_opt;
            }
            if row.price_band.cap_price().is_none()
                && let Some(b) = band_opt
            {
                row.price_band = b;
            }
            if let (Some(lot), Some(cap)) = (row.lot_size, row.price_band.cap_price()) {
                let min_lots = if row.board == BoardKind::Sme { 2 } else { 1 };
                row.min_investment_inr = Some(round((lot * min_lots) as f64 * cap, 0));
            }
            if structure.total_issue_cr.is_some() {
                row.issue_size_cr = structure.total_issue_cr;
            }
            row.quick_signal = derive_quick_signal(row, today);
        }
    }
}

async fn fetch_india_news_rss(yahoo: &YahooClient, query: &str) -> String {
    let url = crate::api::build_url(
        "https://news.google.com/rss/search",
        None,
        &[
            ("q", query),
            ("hl", "en-IN"),
            ("gl", "IN"),
            ("ceid", "IN:en"),
        ],
    );
    yahoo.raw_get_text(&url).await.unwrap_or_default()
}

// ── Assembly & Core Domain Logic ──

pub fn build_analysis_from_parts(
    mut row: IpoSummaryRow,
    detail: Option<&Value>,
    rss_xml: &str,
    regime: IpoRegime,
    capital: Option<f64>,
    today: NaiveDate,
) -> IpoAnalysis {
    let mut industry = None;
    let mut isin = None;

    let (subscription, demand_ladder, structure) = if let Some(d) = detail {
        if let Some(meta) = d.get("metaInfo") {
            industry = meta
                .get("industry")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string);
            isin = meta
                .get("isin")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string);
            if row.listing_date.is_none()
                && let Some(ld) = meta.get("listingDate").and_then(Value::as_str)
            {
                row.listing_date = parse_nse_date(ld);
            }
            if let Some(cname) = meta.get("companyName").and_then(Value::as_str)
                && !cname.trim().is_empty()
                && row.company_name == row.symbol
            {
                row.company_name = cname.trim().to_string();
            }
        }
        let sub = parse_subscription(d);
        let ladder = parse_demand_ladder(d);
        let (lot_opt, band_opt, mut st) = parse_issue_info(d, &row.price_band);
        if let Some(b) = band_opt
            && row.price_band.cap_price().is_none()
        {
            row.price_band = b;
        }
        if row.lot_size.is_none() {
            row.lot_size = lot_opt;
        }
        if st.total_issue_cr.is_none() {
            st.total_issue_cr = row.issue_size_cr;
        }
        if let Some(data_list) = d
            .get("issueInfo")
            .and_then(|i| i.get("dataList"))
            .and_then(Value::as_array)
        {
            if let Some(first_title) = data_list
                .first()
                .and_then(|x| x.get("title"))
                .and_then(Value::as_str)
                && !first_title.trim().is_empty()
                && row.company_name == row.symbol
            {
                row.company_name = first_title.trim().to_string();
            }
            for item in data_list {
                if item.get("title").and_then(Value::as_str) == Some("Issue Period")
                    && let Some(val) = item.get("value").and_then(Value::as_str)
                    && (row.open_date.is_none() || row.close_date.is_none())
                {
                    let (o, c) = parse_issue_period(val);
                    row.open_date = row.open_date.or(o);
                    row.close_date = row.close_date.or(c);
                }
            }
        }
        (sub, ladder, st)
    } else {
        (
            SubscriptionBreakdown {
                source_note: "Summary board snapshot".to_string(),
                total_x: row.total_x,
                qib_x: row.qib_x,
                nii_x: row.nii_x,
                retail_x: row.retail_x,
                ..Default::default()
            },
            Vec::new(),
            IssueStructure {
                total_issue_cr: row.issue_size_cr,
                structure_tag: "Pending RHP / Issue Details".to_string(),
                ..Default::default()
            },
        )
    };

    if let (Some(lot), Some(cap)) = (row.lot_size, row.price_band.cap_price()) {
        let min_lots = if row.board == BoardKind::Sme { 2 } else { 1 };
        row.min_investment_inr = Some(round((lot * min_lots) as f64 * cap, 0));
    }
    row.window_note = compute_window_note(row.status, row.open_date, row.close_date, today);

    let cap_price = row.issue_price.or_else(|| row.price_band.cap_price());
    let street_buzz = parse_street_buzz(rss_xml, cap_price);
    let allotment = build_allotment_optimizer(
        row.board,
        row.status,
        row.lot_size,
        cap_price,
        &subscription,
        capital,
    );
    let verdict = score_ipo_verdict(
        &row,
        &structure,
        &subscription,
        &demand_ladder,
        &street_buzz,
        &regime,
        today,
    );

    IpoAnalysis {
        symbol: row.symbol,
        company_name: row.company_name,
        industry,
        isin,
        board: row.board,
        is_bse_sme: row.is_bse_sme,
        status: row.status,
        open_date: row.open_date,
        close_date: row.close_date,
        listing_date: row.listing_date,
        window_note: row.window_note,
        price_band: row.price_band,
        issue_price: row.issue_price,
        lot_size: row.lot_size,
        min_investment_inr: row.min_investment_inr,
        ltp: row.ltp,
        return_from_issue_pct: row.return_from_issue_pct,
        structure,
        subscription,
        demand_ladder,
        allotment,
        street_buzz,
        regime,
        verdict,
        disclaimer: "Educational IPO analysis from public NSE/BSE filings and press reports — not SEBI-registered investment advice. The Red Herring Prospectus (RHP) is the source document.".to_string(),
    }
}

// ── NSE Parsers ──

pub fn merge_current_and_upcoming(
    curr_val: &Value,
    up_val: &Value,
    today: NaiveDate,
) -> (Vec<IpoSummaryRow>, Vec<IpoSummaryRow>) {
    let mut ongoing_map: HashMap<String, IpoSummaryRow> = HashMap::new();
    let mut ongoing_order: Vec<String> = Vec::new();
    let mut upcoming: Vec<IpoSummaryRow> = Vec::new();

    if let Some(arr) = curr_val.as_array() {
        for item in arr {
            if let Some(row) = parse_board_item(item, IpoStatus::Ongoing, today) {
                let key = format!("{}:{}", row.symbol, row.board.series_code());
                if !ongoing_map.contains_key(&key) {
                    ongoing_order.push(key.clone());
                }
                ongoing_map.insert(key, row);
            }
        }
    }

    if let Some(arr) = up_val.as_array() {
        for item in arr {
            let status_str = str_field(item, &["status"]).unwrap_or_default();
            let is_active = status_str.eq_ignore_ascii_case("active");
            let status = if is_active {
                IpoStatus::Ongoing
            } else {
                IpoStatus::Upcoming
            };
            if let Some(row) = parse_board_item(item, status, today) {
                let key = format!("{}:{}", row.symbol, row.board.series_code());
                if is_active {
                    if let Some(existing) = ongoing_map.get_mut(&key) {
                        if existing.lot_size.is_none() {
                            existing.lot_size = row.lot_size;
                        }
                        if existing.price_band.cap_price().is_none() {
                            existing.price_band = row.price_band;
                        }
                        if existing.issue_size_cr.is_none() {
                            existing.issue_size_cr = row.issue_size_cr;
                        }
                        if let (Some(lot), Some(cap)) =
                            (existing.lot_size, existing.price_band.cap_price())
                        {
                            let min_lots = if existing.board == BoardKind::Sme {
                                2
                            } else {
                                1
                            };
                            existing.min_investment_inr =
                                Some(round((lot * min_lots) as f64 * cap, 0));
                        }
                    } else {
                        ongoing_order.push(key.clone());
                        ongoing_map.insert(key, row);
                    }
                } else if !ongoing_map.contains_key(&key) {
                    upcoming.push(row);
                }
            }
        }
    }

    let mut ongoing: Vec<IpoSummaryRow> = ongoing_order
        .into_iter()
        .filter_map(|k| ongoing_map.remove(&k))
        .collect();

    // Mainboard first, then closing soonest.
    ongoing.sort_by(|a, b| {
        (a.board == BoardKind::Sme)
            .cmp(&(b.board == BoardKind::Sme))
            .then_with(|| a.close_date.cmp(&b.close_date))
            .then_with(|| {
                b.total_x
                    .unwrap_or(0.0)
                    .partial_cmp(&a.total_x.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    upcoming.sort_by(|a, b| {
        (a.board == BoardKind::Sme)
            .cmp(&(b.board == BoardKind::Sme))
            .then_with(|| a.open_date.cmp(&b.open_date))
    });

    (ongoing, upcoming)
}

fn parse_board_item(
    item: &Value,
    default_status: IpoStatus,
    today: NaiveDate,
) -> Option<IpoSummaryRow> {
    let symbol = str_field(item, &["symbol"])?.trim().to_ascii_uppercase();
    if symbol.is_empty() {
        return None;
    }
    let company_name =
        str_field(item, &["companyName", "company"]).unwrap_or_else(|| symbol.clone());
    let series = str_field(item, &["series", "securityType"]).unwrap_or_else(|| "EQ".to_string());
    let board = BoardKind::from_series(&series);
    let is_bse_sme = str_field(item, &["isBse"]).as_deref() == Some("1");

    let open_date =
        str_field(item, &["issueStartDate", "ipoStartDate"]).and_then(|s| parse_nse_date(&s));
    let close_date =
        str_field(item, &["issueEndDate", "ipoEndDate"]).and_then(|s| parse_nse_date(&s));

    let band_raw = str_field(item, &["priceBand", "issuePrice", "priceRange"]).unwrap_or_default();
    let price_band = parse_price_band(&band_raw);
    let lot_size = num_field(item, &["lotSize"])
        .map(|n| n.round() as u64)
        .filter(|&n| n > 0);

    // `issueSize` in NSE current/upcoming JSON is usually number of shares offered!
    let shares_offered = num_field(item, &["noOfSharesOffered", "issueSize"]);
    let issue_size_cr = match (shares_offered, price_band.cap_price()) {
        (Some(sh), Some(cap)) if sh > 10_000.0 => Some(round(sh * cap / 1e7, 2)),
        _ => None,
    };

    let min_investment_inr = match (lot_size, price_band.cap_price()) {
        (Some(lot), Some(cap)) => {
            let min_lots = if board == BoardKind::Sme { 2 } else { 1 };
            Some(round((lot * min_lots) as f64 * cap, 0))
        }
        _ => None,
    };

    let total_x = num_field(item, &["noOfTime"]).map(|x| round(x, 2));
    let window_note = compute_window_note(default_status, open_date, close_date, today);

    let mut row = IpoSummaryRow {
        symbol,
        company_name,
        board,
        is_bse_sme,
        status: default_status,
        open_date,
        close_date,
        listing_date: None,
        window_note,
        price_band,
        issue_price: None,
        lot_size,
        min_investment_inr,
        issue_size_cr,
        qib_x: None,
        nii_x: None,
        retail_x: None,
        total_x,
        ltp: None,
        return_from_issue_pct: None,
        quick_signal: String::new(),
    };
    row.quick_signal = derive_quick_signal(&row, today);
    Some(row)
}

pub fn parse_past_issues(past_val: &Value, today: NaiveDate, limit: usize) -> Vec<IpoSummaryRow> {
    let Some(arr) = past_val.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr.iter().take(limit) {
        let Some(symbol) = str_field(item, &["symbol"]).map(|s| s.trim().to_ascii_uppercase())
        else {
            continue;
        };
        if symbol.is_empty() {
            continue;
        }
        let company_name =
            str_field(item, &["company", "companyName"]).unwrap_or_else(|| symbol.clone());
        let series =
            str_field(item, &["securityType", "series"]).unwrap_or_else(|| "EQ".to_string());
        let board = BoardKind::from_series(&series);
        let open_date = str_field(item, &["ipoStartDate"]).and_then(|s| parse_nse_date(&s));
        let close_date = str_field(item, &["ipoEndDate"]).and_then(|s| parse_nse_date(&s));
        let listing_date = str_field(item, &["listingDate"]).and_then(|s| parse_nse_date(&s));
        let price_band = parse_price_band(&str_field(item, &["priceRange"]).unwrap_or_default());
        let issue_price = num_field(item, &["issuePrice"]).filter(|&p| p > 0.0);

        let status = if listing_date.is_some() || issue_price.is_some() {
            IpoStatus::Listed
        } else {
            IpoStatus::ClosedPendingListing
        };
        let window_note = if let Some(ld) = listing_date {
            format!("Listed {}", ld.format("%d %b %Y"))
        } else if let Some(cd) = close_date {
            let days = (today - cd).num_days();
            if (0..=7).contains(&days) {
                format!("Closed {} · Allotment/Listing pending", cd.format("%d %b"))
            } else {
                format!("Closed {}", cd.format("%d %b %Y"))
            }
        } else {
            status.label().to_string()
        };

        out.push(IpoSummaryRow {
            symbol,
            company_name,
            board,
            is_bse_sme: false,
            status,
            open_date,
            close_date,
            listing_date,
            window_note,
            price_band,
            issue_price,
            lot_size: None,
            min_investment_inr: None,
            issue_size_cr: None,
            qib_x: None,
            nii_x: None,
            retail_x: None,
            total_x: None,
            ltp: None,
            return_from_issue_pct: None,
            quick_signal: if status == IpoStatus::Listed {
                "LISTED".to_string()
            } else {
                "AWAITING LISTING".to_string()
            },
        });
    }
    out
}

fn apply_quotes_to_recent(recent: &mut [IpoSummaryRow], quotes: &[Quote]) {
    let map: HashMap<&str, &Quote> = quotes
        .iter()
        .filter_map(|q| q.symbol.as_deref().map(|s| (s, q)))
        .collect();
    for row in recent.iter_mut() {
        let key = format!("{}.NS", row.symbol);
        if let Some(q) = map.get(key.as_str())
            && let Some(price) = q.regular_market_price
            && price > 0.0
        {
            row.ltp = Some(round(price, 2));
            row.status = IpoStatus::Listed;
            if let Some(iss) = row.issue_price.or_else(|| row.price_band.cap_price())
                && iss > 0.0
            {
                let ret = round((price / iss - 1.0) * 100.0, 2);
                row.return_from_issue_pct = Some(ret);
                row.quick_signal = if ret >= 0.0 {
                    format!("+{ret:.1}% ABOVE ISSUE")
                } else {
                    format!("{ret:.1}% BELOW ISSUE")
                };
            }
        }
    }
}

pub fn compute_regime(recent: &[IpoSummaryRow], quotes: &[Quote]) -> IpoRegime {
    let nifty_change_pct = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some("^NSEI"))
        .and_then(|q| q.regular_market_change_percent)
        .map(|v| round(v, 2));
    let india_vix = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some("^INDIAVIX"))
        .and_then(|q| q.regular_market_price)
        .map(|v| round(v, 2));

    let priced: Vec<(&str, f64)> = recent
        .iter()
        .filter_map(|r| r.return_from_issue_pct.map(|p| (r.symbol.as_str(), p)))
        .collect();

    if priced.is_empty() {
        return IpoRegime {
            label: "NEUTRAL".to_string(),
            sample_size: 0,
            positive_count: 0,
            hit_rate_pct: 50.0,
            avg_return_pct: 0.0,
            median_return_pct: 0.0,
            best_recent: None,
            worst_recent: None,
            nifty_change_pct,
            india_vix,
            summary: "Insufficient recent post-listing price quotes to score listing sentiment."
                .to_string(),
        };
    }

    let sample_size = priced.len();
    let positive_count = priced.iter().filter(|(_, p)| *p > 0.0).count();
    let hit_rate_pct = round(positive_count as f64 / sample_size as f64 * 100.0, 1);
    let avg_return_pct = round(
        priced.iter().map(|(_, p)| *p).sum::<f64>() / sample_size as f64,
        2,
    );
    let mut sorted_rets: Vec<f64> = priced.iter().map(|(_, p)| *p).collect();
    sorted_rets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted_rets.len() / 2;
    let median_return_pct = if sorted_rets.len().is_multiple_of(2) {
        round((sorted_rets[mid - 1] + sorted_rets[mid]) / 2.0, 2)
    } else {
        round(sorted_rets[mid], 2)
    };

    let best_recent = priced
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(s, p)| ((*s).to_string(), *p));
    let worst_recent = priced
        .iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(s, p)| ((*s).to_string(), *p));

    let vix_high = india_vix.is_some_and(|v| v >= 20.0);
    let label = if hit_rate_pct >= 70.0 && avg_return_pct >= 12.0 && !vix_high {
        "HOT"
    } else if hit_rate_pct >= 55.0 && avg_return_pct >= 3.0 {
        "CONSTRUCTIVE"
    } else if hit_rate_pct >= 40.0 || avg_return_pct >= -2.0 {
        "SELECTIVE"
    } else {
        "COLD"
    };

    let summary = format!(
        "{positive_count}/{sample_size} recent NSE listings trading above issue price ({hit_rate_pct:.0}% hit-rate, avg {avg_return_pct:+.1}%, median {median_return_pct:+.1}%)."
    );

    IpoRegime {
        label: label.to_string(),
        sample_size,
        positive_count,
        hit_rate_pct,
        avg_return_pct,
        median_return_pct,
        best_recent,
        worst_recent,
        nifty_change_pct,
        india_vix,
        summary,
    }
}

/// Parse consolidated subscription (`activeCat` + `bidDetails` + `demandGraphALL`).
pub fn parse_subscription(detail: &Value) -> SubscriptionBreakdown {
    let bid_details = detail
        .get("bidDetails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let active_cat = detail
        .get("activeCat")
        .and_then(|a| a.get("dataList"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Check if `activeCat` has real consolidated NSE+BSE bid numbers.
    let active_total_bids: u64 = active_cat
        .iter()
        .filter(|x| {
            let sr = str_field(x, &["srNo"]).unwrap_or_default();
            matches!(sr.as_str(), "1" | "2" | "3" | "4" | "5")
        })
        .filter_map(|x| num_field(x, &["noOfSharesBid", "noOfshareBid"]))
        .map(|n| n.round() as u64)
        .sum();

    // Map application counts and Cut-Off/Price sub-rows from `bidDetails`.
    let mut apps_by_sr: HashMap<String, u64> = HashMap::new();
    let mut bids_by_sr_fallback: HashMap<String, u64> = HashMap::new();
    for item in &bid_details {
        let sr = str_field(item, &["srNo"]).unwrap_or_else(|| "total".to_string());
        if let Some(app) = num_field(item, &["noofapplication", "noOfApplication"])
            && app > 0.0
        {
            apps_by_sr.insert(sr.clone(), app.round() as u64);
        }
        if let Some(b) = num_field(item, &["noOfsharesBid", "noOfshareBid", "noOfSharesBid"]) {
            bids_by_sr_fallback.insert(sr, b.round() as u64);
        }
    }

    let use_active = active_total_bids > 0;
    let rows_src = if use_active {
        &active_cat
    } else {
        &bid_details
    };
    let mut categories = Vec::new();

    for item in rows_src {
        let cat = str_field(item, &["category"]).unwrap_or_default();
        if cat.is_empty() || cat.eq_ignore_ascii_case("category") {
            continue;
        }
        let sr_no = str_field(item, &["srNo"]).unwrap_or_else(|| "Total".to_string());
        let offered = num_field(item, &["noOfShareOffered", "noOfSharesOffered"])
            .map(|n| n.round() as u64)
            .filter(|&n| n > 0);
        let bids = num_field(item, &["noOfSharesBid", "noOfsharesBid", "noOfshareBid"])
            .map(|n| n.round() as u64);
        let times =
            num_field(item, &["noOfTotalMeant", "noOfTime"]).or_else(|| match (bids, offered) {
                (Some(b), Some(o)) if o > 0 => Some(b as f64 / o as f64),
                _ => None,
            });
        let applications = apps_by_sr.get(&sr_no).copied();

        categories.push(CategorySubscription {
            sr_no,
            category: cat,
            offered_shares: offered,
            bid_shares: bids,
            times_subscribed: times.map(|t| round(t, 2)),
            applications,
        });
    }

    let mut out = SubscriptionBreakdown {
        source_note: if use_active {
            "Consolidated NSE + BSE live bid book".to_string()
        } else {
            "NSE live bid book".to_string()
        },
        ..Default::default()
    };

    let mut qib_bids_total: Option<u64> = None;

    for c in &categories {
        let cat_lc = c.category.to_ascii_lowercase();
        match c.sr_no.as_str() {
            "1" => {
                out.qib_x = c.times_subscribed;
                qib_bids_total = c.bid_shares;
            }
            "1(a)" => out.fii_bids = c.bid_shares,
            "1(b)" => out.dfi_bids = c.bid_shares,
            "1(c)" => out.mf_bids = c.bid_shares,
            "2" => out.nii_x = c.times_subscribed,
            "2.1" => out.bnii_x = c.times_subscribed,
            "2.2" => out.snii_x = c.times_subscribed,
            "3" => out.retail_x = c.times_subscribed,
            "3(a)" => out.retail_cutoff_bids = c.bid_shares,
            "3(b)" => out.retail_price_bids = c.bid_shares,
            _ => {
                if cat_lc.contains("employee") && c.times_subscribed.is_some() {
                    out.employee_x = c.times_subscribed;
                } else if cat_lc.contains("shareholder") && c.times_subscribed.is_some() {
                    out.shareholder_x = c.times_subscribed;
                } else if cat_lc == "total" || c.sr_no == "Total" {
                    out.total_offered = c.offered_shares;
                    out.total_bids = c.bid_shares;
                    out.total_x = c.times_subscribed;
                }
            }
        }
    }

    // Fill retail Cut-Off / Price-bid breakdown from `bidDetails` when `activeCat` omits 3(a)/3(b).
    if out.retail_cutoff_bids.is_none() {
        out.retail_cutoff_bids = bids_by_sr_fallback.get("3(a)").copied();
    }
    if out.retail_price_bids.is_none() {
        out.retail_price_bids = bids_by_sr_fallback.get("3(b)").copied();
    }
    if let (Some(co), Some(pr)) = (out.retail_cutoff_bids, out.retail_price_bids)
        && co + pr > 0
    {
        out.retail_cutoff_pct = Some(round(co as f64 / (co + pr) as f64 * 100.0, 1));
    }

    if let (Some(mf), Some(qib)) = (out.mf_bids, qib_bids_total)
        && qib > 0
    {
        out.mf_share_of_qib_pct = Some(round(mf as f64 / qib as f64 * 100.0, 1));
    }

    // Prefer `demandGraphALL` total multiple if `activeCat` lacked a Total row.
    if out.total_x.is_none()
        && let Some(dg) = detail
            .get("demandGraphALL")
            .or_else(|| detail.get("demandGraph"))
    {
        out.total_x = num_field(dg, &["noOfTimesIssueSubscribed"]).map(|x| round(x, 2));
        if out.total_bids.is_none() {
            out.total_bids = num_field(dg, &["TOTAL_BIDS"]).map(|n| n.round() as u64);
        }
        if out.total_offered.is_none() {
            out.total_offered = num_field(dg, &["totalIssueSize"]).map(|n| n.round() as u64);
        }
    }

    // Compute Total from top-level category sums if still absent.
    if out.total_x.is_none() {
        let sum_off: u64 = categories
            .iter()
            .filter(|c| matches!(c.sr_no.as_str(), "1" | "2" | "3" | "4" | "5"))
            .filter_map(|c| c.offered_shares)
            .sum();
        let sum_bid: u64 = categories
            .iter()
            .filter(|c| matches!(c.sr_no.as_str(), "1" | "2" | "3" | "4" | "5"))
            .filter_map(|c| c.bid_shares)
            .sum();
        if sum_off > 0 {
            out.total_offered = Some(sum_off);
            out.total_bids = Some(sum_bid);
            out.total_x = Some(round(sum_bid as f64 / sum_off as f64, 2));
        }
    }

    out.categories = categories;
    out
}

/// Parse cumulative price-band demand ladder (`demandGraphALL` preferred over `demandGraph`).
pub fn parse_demand_ladder(detail: &Value) -> Vec<PriceDemandStep> {
    let Some(dg) = detail
        .get("demandGraphALL")
        .filter(|v| v.get("plotData").is_some())
        .or_else(|| detail.get("demandGraph"))
    else {
        return Vec::new();
    };
    let Some(plot) = dg.get("plotData").and_then(Value::as_object) else {
        return Vec::new();
    };
    let total_bids = num_field(dg, &["TOTAL_BIDS"])
        .map(|n| n.round() as u64)
        .unwrap_or(0);

    let mut numeric_steps: Vec<PriceDemandStep> = Vec::new();
    let mut cutoff_step: Option<PriceDemandStep> = None;

    for (k, v) in plot {
        let Some(qty_str) = v.as_str() else { continue };
        let Some(qty) = parse_indian_number(qty_str).map(|n| n.round() as u64) else {
            continue;
        };
        let pct = if total_bids > 0 {
            round(qty as f64 / total_bids as f64 * 100.0, 1)
        } else {
            0.0
        };
        if k.to_ascii_lowercase().contains("cut") {
            cutoff_step = Some(PriceDemandStep {
                label: "Cut-Off".to_string(),
                price: None,
                cumulative_shares: qty,
                pct_of_total_bids: pct,
            });
        } else if let Ok(price) = k.trim().parse::<f64>() {
            numeric_steps.push(PriceDemandStep {
                label: format!("₹{}", fmt_price(price)),
                price: Some(price),
                cumulative_shares: qty,
                pct_of_total_bids: pct,
            });
        }
    }

    // Sort highest price (cap) down to floor price, with Cut-Off first.
    numeric_steps.sort_by(|a, b| {
        b.price
            .unwrap_or(0.0)
            .partial_cmp(&a.price.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some(co) = cutoff_step {
        numeric_steps.insert(0, co);
    }
    numeric_steps
}

/// Parse `issueInfo.dataList` for Fresh Issue vs OFS, BRLMs, Registrar, Lot Size, and RHP links.
pub fn parse_issue_info(
    detail: &Value,
    existing_band: &PriceBand,
) -> (Option<u64>, Option<PriceBand>, IssueStructure) {
    let mut st = IssueStructure::default();
    let mut lot_size: Option<u64> = None;
    let mut band_opt: Option<PriceBand> = None;

    let Some(list) = detail
        .get("issueInfo")
        .and_then(|i| i.get("dataList"))
        .and_then(Value::as_array)
    else {
        st.structure_tag = "Standard Issue".to_string();
        return (None, None, st);
    };

    for entry in list {
        let title = entry
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let val = entry
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .trim();
        if title.is_empty() || val.is_empty() {
            continue;
        }
        let t_lc = title.to_ascii_lowercase();
        if t_lc == "issue size" {
            st.raw_issue_size = val.to_string();
        } else if t_lc == "issue type" {
            st.issue_type = Some(val.to_string());
        } else if t_lc == "price range" || t_lc == "issue price" {
            let pb = parse_price_band(val);
            if pb.cap_price().is_some() {
                band_opt = Some(pb);
            }
        } else if t_lc == "bid lot" || t_lc == "lot size" || t_lc == "minimum order quantity" {
            if lot_size.is_none() {
                lot_size = first_u64_in_text(val);
            }
        } else if t_lc == "discount" && !val.eq_ignore_ascii_case("na") && val != "-" {
            st.discount = Some(val.to_string());
        } else if t_lc == "face value" {
            st.face_value = Some(val.to_string());
        } else if t_lc.contains("book running lead manager") || t_lc.contains("lead manager") {
            st.brlms = val
                .split([';', '\n'])
                .flat_map(|s| s.split(" and "))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        } else if t_lc == "sponsor bank" {
            st.sponsor_bank = Some(val.to_string());
        } else if t_lc.contains("upi mandate") {
            st.upi_cutoff = Some(val.split('.').next().unwrap_or(val).trim().to_string());
        } else if t_lc == "name of the registrar" || t_lc == "registrar" {
            st.registrar_allotment_url = registrar_portal_url(val).map(str::to_string);
            st.registrar_name = Some(val.to_string());
        } else if t_lc.contains("red herring prospectus") || t_lc == "prospectus" {
            st.rhp_url = extract_url(val);
        } else if t_lc.contains("anchor allocation") {
            st.anchor_report_url = extract_url(val);
        } else if t_lc.contains("ratios / basis") {
            st.ratios_url = extract_url(val);
        }
    }

    let cap = band_opt
        .as_ref()
        .and_then(PriceBand::cap_price)
        .or_else(|| existing_band.cap_price());
    populate_fresh_vs_ofs(&mut st, cap);

    (lot_size, band_opt, st)
}

fn populate_fresh_vs_ofs(st: &mut IssueStructure, cap_price: Option<f64>) {
    let raw = &st.raw_issue_size;
    if raw.is_empty() {
        st.structure_tag = "Issue terms pending".to_string();
        return;
    }
    let lc = raw.to_ascii_lowercase();
    let has_fresh = lc.contains("fresh issue");
    let has_ofs = lc.contains("offer for sale") || lc.contains("ofs");

    st.anchor_shares = extract_shares_near(&lc, "anchor");
    st.market_maker_shares = extract_shares_near(&lc, "market maker");

    // Extract rupee amounts or share counts for Fresh Issue, OFS, and Total.
    if has_fresh && !has_ofs {
        let cr = extract_amount_cr(raw, cap_price);
        st.fresh_issue_cr = cr;
        st.ofs_cr = Some(0.0);
        st.total_issue_cr = cr;
        st.fresh_pct = Some(100.0);
        st.structure_tag = "100% Fresh Issue (Growth Capital)".to_string();
    } else if has_ofs && !has_fresh {
        let cr = extract_amount_cr(raw, cap_price);
        st.fresh_issue_cr = Some(0.0);
        st.ofs_cr = cr;
        st.total_issue_cr = cr;
        st.fresh_pct = Some(0.0);
        st.structure_tag = "100% OFS (Promoter / PE Exit)".to_string();
    } else if has_fresh && has_ofs {
        // Split around "offer for sale" to parse fresh segment and OFS segment separately.
        let ofs_idx = lc
            .find("offer for sale")
            .or_else(|| lc.find("ofs"))
            .unwrap_or(0);
        let fresh_idx = lc.find("fresh issue").unwrap_or(0);
        let (fresh_part, ofs_part) = if fresh_idx <= ofs_idx {
            (&raw[fresh_idx..ofs_idx], &raw[ofs_idx..])
        } else {
            (&raw[fresh_idx..], &raw[ofs_idx..fresh_idx])
        };
        let f_cr = extract_amount_cr(fresh_part, cap_price);
        let o_cr = extract_amount_cr(ofs_part, cap_price);
        st.fresh_issue_cr = f_cr;
        st.ofs_cr = o_cr;
        match (f_cr, o_cr) {
            (Some(f), Some(o)) if f + o > 0.0 => {
                let tot = round(f + o, 2);
                let fp = round(f / tot * 100.0, 1);
                st.total_issue_cr = Some(tot);
                st.fresh_pct = Some(fp);
                st.structure_tag = format!("Mixed ({fp:.0}% Fresh / {:.0}% OFS)", 100.0 - fp);
            }
            _ => {
                st.total_issue_cr = extract_amount_cr(raw, cap_price);
                st.structure_tag = "Mixed (Fresh Issue + OFS)".to_string();
            }
        }
    } else {
        st.total_issue_cr = extract_amount_cr(raw, cap_price);
        st.structure_tag = st
            .issue_type
            .clone()
            .unwrap_or_else(|| "Book Built Issue".to_string());
    }
}

// ── SEBI Allotment Odds & Capital Optimizer ──

pub fn build_allotment_optimizer(
    board: BoardKind,
    status: IpoStatus,
    lot_size: Option<u64>,
    cap_price: Option<f64>,
    sub: &SubscriptionBreakdown,
    capital_inr: Option<f64>,
) -> AllotmentOptimizer {
    let mut buckets = Vec::new();
    let mut capital_odds = Vec::new();
    let mut allotment_rules = Vec::new();

    let (Some(lot), Some(price)) = (lot_size.filter(|&l| l > 0), cap_price.filter(|&p| p > 0.0))
    else {
        return AllotmentOptimizer {
            buckets,
            capital_input_inr: capital_inr,
            capital_odds: vec![
                "Lot size and price band are not announced yet; allotment odds appear once they are published.".to_string(),
            ],
            allotment_rules: vec![
                "QIB bids typically arrive late on the closing day, so the QIB multiple changes most then.".to_string(),
            ],
        };
    };

    let one_lot_inr = lot as f64 * price;

    if board == BoardKind::Sme {
        let ind_lots = if one_lot_inr * 2.0 >= 100_000.0 {
            2
        } else {
            (200_000.0 / one_lot_inr).ceil().max(2.0) as u64
        };
        let ind_amt = ind_lots as f64 * one_lot_inr;
        let nii_x = sub.nii_x.or(sub.retail_x).or(sub.total_x);
        let win_prob = nii_x.map(|x| {
            if x <= 1.0 {
                100.0
            } else {
                round((1.0 / x * 100.0).min(100.0), 1)
            }
        });
        buckets.push(AllotmentBucketOdds {
            bucket: "SME Individual (Min 2 Lots)".to_string(),
            lots_required: ind_lots,
            shares_required: ind_lots * lot,
            amount_inr: round(ind_amt, 0),
            subscription_x: nii_x,
            allotment_packet_lots: ind_lots,
            allotment_packet_inr: round(ind_amt, 0),
            estimated_win_prob_pct: win_prob,
            rule_note: "SEBI SME framework requires multi-lot ₹2L+ minimum ticket; post-listing shares trade strictly in full lots.".to_string(),
        });

        if let Some(cap) = capital_inr {
            if cap < ind_amt {
                capital_odds.push(format!(
                    "₹{} is below this SME IPO's minimum application of ₹{} ({} lots × {} shares).",
                    fmt_inr(cap),
                    fmt_inr(ind_amt),
                    ind_lots,
                    lot
                ));
            } else {
                let pans = (cap / ind_amt).floor() as u64;
                capital_odds.push(format!(
                    "₹{} covers up to {} applications of the minimum size ({} lots, ₹{} each); every application needs its own PAN.",
                    fmt_inr(cap),
                    pans.max(1),
                    ind_lots,
                    fmt_inr(ind_amt)
                ));
            }
        }
        allotment_rules.push(
            "After listing, SME shares trade only in full lots, so partial exits are not possible, and the 5% circuit filter can limit liquidity.".to_string(),
        );
        return AllotmentOptimizer {
            buckets,
            capital_input_inr: capital_inr,
            capital_odds,
            allotment_rules,
        };
    }

    // Mainboard SEBI ICDR 3-bucket rules:
    // 1. Retail (RII): 1 lot (~₹15k), max ₹2,00,000.
    let rii_prob = sub.retail_x.map(|x| {
        if x <= 1.0 {
            100.0
        } else {
            round((1.0 / x * 100.0).min(100.0), 1)
        }
    });
    buckets.push(AllotmentBucketOdds {
        bucket: "Retail (RII ≤ ₹2L)".to_string(),
        lots_required: 1,
        shares_required: lot,
        amount_inr: round(one_lot_inr, 0),
        subscription_x: sub.retail_x,
        allotment_packet_lots: 1,
        allotment_packet_inr: round(one_lot_inr, 0),
        estimated_win_prob_pct: rii_prob,
        rule_note:
            "SEBI 1-lot lottery when RII > 1x; bidding >1 lot on one PAN gives 0% extra odds."
                .to_string(),
    });

    // 2. Small HNI (sNII): ₹2,00,001 to ₹10,00,000 (1/3rd of NII quota).
    let snii_lots = ((200_001.0 / one_lot_inr).ceil() as u64).max(2);
    let snii_amt = snii_lots as f64 * one_lot_inr;
    let snii_mult = sub.snii_x.or(sub.nii_x);
    let snii_prob = snii_mult.map(|x| {
        if x <= 1.0 {
            100.0
        } else {
            round((1.0 / x * 100.0).min(100.0), 1)
        }
    });
    buckets.push(AllotmentBucketOdds {
        bucket: "Small HNI (sNII ₹2L–₹10L)".to_string(),
        lots_required: snii_lots,
        shares_required: snii_lots * lot,
        amount_inr: round(snii_amt, 0),
        subscription_x: snii_mult,
        allotment_packet_lots: snii_lots,
        allotment_packet_inr: round(snii_amt, 0),
        estimated_win_prob_pct: snii_prob,
        rule_note: "Allotted in ₹2L+ packets by lottery when sNII > 1x; bidding above minimum ₹2L in sNII wastes blocked funds.".to_string(),
    });

    // 3. Big HNI (bNII): > ₹10,00,000 (2/3rds of NII quota), allotted in sNII-sized ₹2L packets!
    let bnii_lots = ((1_000_001.0 / one_lot_inr).ceil() as u64).max(snii_lots + 1);
    let bnii_amt = bnii_lots as f64 * one_lot_inr;
    let packets_per_bnii_app = (bnii_lots as f64 / snii_lots as f64).max(1.0);
    let bnii_mult = sub.bnii_x.or(sub.nii_x);
    let bnii_prob = bnii_mult.map(|x| {
        if x <= 0.0 {
            100.0
        } else {
            // Each bNII bid is ~5x the ₹2L allotment packet, so share-subscription `x`
            // corresponds to `x / packets_per_bnii_app` applications per available ₹2L packet!
            round((packets_per_bnii_app / x * 100.0).min(100.0), 1)
        }
    });
    buckets.push(AllotmentBucketOdds {
        bucket: "Big HNI (bNII > ₹10L)".to_string(),
        lots_required: bnii_lots,
        shares_required: bnii_lots * lot,
        amount_inr: round(bnii_amt, 0),
        subscription_x: bnii_mult,
        allotment_packet_lots: snii_lots,
        allotment_packet_inr: round(snii_amt, 0),
        estimated_win_prob_pct: bnii_prob,
        rule_note: "Bidding ₹10L+ enters the 2/3rd NII pool where winners receive ₹2L+ sNII packets (~5x higher per-PAN packet odds per share multiple).".to_string(),
    });

    // Capital deployment plan (either tailored to `--capital` or standard multi-PAN comparison).
    let max_retail_lots = (200_000.0 / one_lot_inr).floor().max(1.0) as u64;
    if let Some(cap) = capital_inr {
        if cap < one_lot_inr {
            capital_odds.push(format!(
                "₹{} is below 1 Retail lot (₹{} for {} shares).",
                fmt_inr(cap),
                fmt_inr(one_lot_inr),
                lot
            ));
        } else if sub.retail_x.unwrap_or(1.5) <= 1.0 && status == IpoStatus::Ongoing {
            let affordable_lots = ((cap / one_lot_inr).floor() as u64).min(max_retail_lots);
            capital_odds.push(format!(
                "Retail is at {:.2}x. At or below 1.0x every valid retail bid is allotted in full; above 1.0x allotment is by lottery and extra lots in one application add no odds. ₹{} buys up to {} lots within the ₹2L retail limit.",
                sub.retail_x.unwrap_or(0.0),
                fmt_inr(affordable_lots as f64 * one_lot_inr),
                affordable_lots
            ));
        } else {
            let affordable_apps = (cap / one_lot_inr).floor() as u32;
            if let Some(p_pct) = rii_prob {
                let p = (p_pct / 100.0).clamp(0.0, 1.0);
                let n = affordable_apps.min(6);
                let multi_pan_prob = round((1.0 - (1.0 - p).powi(n as i32)) * 100.0, 1);
                capital_odds.push(format!(
                    "Odds for {} separate applications of 1 lot (₹{} in total): ~{multi_pan_prob:.1}% chance of at least one allotment, against {p_pct:.1}% for one application (each needs its own PAN).",
                    n,
                    fmt_inr(n as f64 * one_lot_inr)
                ));
            }
            if cap >= bnii_amt {
                capital_odds.push(format!(
                    "One bNII application of {} lots (₹{}) has ~{}% odds of an allotment of a ₹{} packet.",
                    bnii_lots,
                    fmt_inr(bnii_amt),
                    bnii_prob.map_or("N/A".to_string(), |v| format!("{v:.1}")),
                    fmt_inr(snii_amt)
                ));
            } else if cap >= snii_amt {
                capital_odds.push(format!(
                    "One sNII application of {} lots (₹{}) has ~{}% odds of a ₹{} packet.",
                    snii_lots,
                    fmt_inr(snii_amt),
                    snii_prob.map_or("N/A".to_string(), |v| format!("{v:.1}")),
                    fmt_inr(snii_amt)
                ));
            }
        }
    } else if sub.retail_x.unwrap_or(0.0) > 1.0 {
        if let Some(p_pct) = rii_prob {
            let p = (p_pct / 100.0).clamp(0.0, 1.0);
            let three_pan = round((1.0 - (1.0 - p).powi(3)) * 100.0, 1);
            capital_odds.push(format!(
                "Retail is oversubscribed ({:.2}x): one application of 1 lot (₹{}) has {p_pct:.1}% odds; 3 separate applications would have {three_pan:.1}% odds of at least one allotment.",
                sub.retail_x.unwrap_or(0.0),
                fmt_inr(one_lot_inr)
            ));
        }
    } else {
        capital_odds.push(format!(
            "1 Retail lot = {} shares (₹{}). Pass `--capital 2.5L` or `--capital 12L` to see the odds at that amount for retail, sNII and bNII applications.",
            lot,
            fmt_inr(one_lot_inr)
        ));
    }

    allotment_rules.push(
        "A retail bid below the final issue price is not allotted; a cut-off bid takes whatever the final price turns out to be.".to_string(),
    );
    allotment_rules.push(
        "QIBs typically place most of their bids on the afternoon of the closing day; the UPI mandate deadline for retail is 5:00 PM IST.".to_string(),
    );

    AllotmentOptimizer {
        buckets,
        capital_input_inr: capital_inr,
        capital_odds,
        allotment_rules,
    }
}

// ── 6-Pillar Dual Verdict Engine ──

pub fn score_ipo_verdict(
    row: &IpoSummaryRow,
    st: &IssueStructure,
    sub: &SubscriptionBreakdown,
    ladder: &[PriceDemandStep],
    buzz: &StreetBuzz,
    regime: &IpoRegime,
    today: NaiveDate,
) -> IpoDualVerdict {
    let mut bull_points = Vec::new();
    let mut risk_flags = Vec::new();

    let is_upcoming = row.status == IpoStatus::Upcoming;
    let is_final_day_or_closed = match row.close_date {
        Some(cd) => today >= cd || row.status != IpoStatus::Ongoing,
        None => row.status != IpoStatus::Ongoing,
    };

    // 1. Pillar 1: Institutional & Smart-Money Conviction (30%)
    let (p1_score, p1_status, p1_sum) = if is_upcoming {
        (
            50.0,
            "PENDING".to_string(),
            "Bidding has not opened yet; the anchor book and QIB demand are not available."
                .to_string(),
        )
    } else if row.board == BoardKind::Sme && sub.qib_x.unwrap_or(0.0) == 0.0 {
        let anchor_bonus = if st.anchor_shares.unwrap_or(0) > 0 {
            12.0
        } else {
            0.0
        };
        let tot = sub.total_x.unwrap_or(0.0);
        let base: f64 = if tot >= 25.0 {
            62.0
        } else if tot >= 5.0 {
            52.0
        } else {
            38.0
        };
        let sc = (base + anchor_bonus).clamp(20.0, 78.0);
        (
            sc,
            if sc >= 60.0 { "MODERATE" } else { "LIMITED" }.to_string(),
            format!(
                "SME issue with no active QIB book reported{}{}.",
                if st.anchor_shares.unwrap_or(0) > 0 {
                    " (Anchor allocation present)"
                } else {
                    " (no Anchor reservation detected)"
                },
                sub.total_x
                    .map_or(String::new(), |t| format!(" · overall {t:.2}x"))
            ),
        )
    } else {
        let qib = sub.qib_x.unwrap_or(0.0);
        let mut sc: f64 = if qib >= 50.0 {
            95.0
        } else if qib >= 20.0 {
            88.0
        } else if qib >= 10.0 {
            80.0
        } else if qib >= 5.0 {
            72.0
        } else if qib >= 2.0 {
            62.0
        } else if qib >= 1.0 {
            52.0
        } else if !is_final_day_or_closed {
            // Day 1 / Day 2: QIBs routinely wait until Day 3 afternoon.
            48.0
        } else {
            22.0
        };

        if let Some(mf_pct) = sub.mf_share_of_qib_pct {
            if mf_pct >= 30.0 && qib >= 1.5 {
                sc += 6.0;
                bull_points.push(format!(
                    "Domestic Mutual Funds account for {mf_pct:.0}% of QIB demand."
                ));
            } else if mf_pct == 0.0 && qib >= 2.0 && is_final_day_or_closed {
                sc -= 4.0;
                risk_flags.push(
                    "Zero domestic Mutual Fund participation in the QIB book (demand is purely FII/Other QIBs).".to_string(),
                );
            }
        }
        if st.anchor_shares.unwrap_or(0) > 0 {
            sc += 4.0;
            bull_points.push(format!(
                "Backed by pre-IPO Anchor book reservation ({} shares).",
                fmt_int_indian(st.anchor_shares.unwrap_or(0))
            ));
        }
        if qib >= 10.0 {
            bull_points.push(format!("QIB book subscribed {qib:.2}x."));
        } else if qib < 1.0 && is_final_day_or_closed {
            risk_flags.push(format!(
                "QIB book subscribed only {qib:.2}x on the final or closed book."
            ));
        }
        let sc = sc.clamp(10.0, 99.0);
        let status_lbl = if sc >= 72.0 {
            "STRONG"
        } else if sc >= 48.0 {
            "MODERATE"
        } else {
            "WEAK"
        };
        let summary = if !is_final_day_or_closed && qib < 1.0 {
            format!(
                "QIB at {qib:.2}x so far (early bidding day — QIBs typically bid on the afternoon of the closing day)."
            )
        } else {
            format!(
                "QIB subscribed {qib:.2}x{}",
                sub.mf_share_of_qib_pct.map_or(String::new(), |p| format!(
                    " (Mutual Funds: {p:.0}% of QIB)"
                ))
            )
        };
        (sc, status_lbl.to_string(), summary)
    };

    // 2. Pillar 2: Issue Structure — Fresh Issue vs OFS (20%)
    let (p2_score, p2_status, p2_sum) = match st.fresh_pct {
        Some(fp) if fp >= 85.0 => {
            bull_points.push(format!(
                "{fp:.0}% fresh issue: proceeds go to the company rather than to selling shareholders."
            ));
            (
                86.0,
                "CLEAN".to_string(),
                format!("{} ({})", st.structure_tag, fmt_cr_opt(st.total_issue_cr)),
            )
        }
        Some(fp) if fp >= 50.0 => (
            70.0,
            "BALANCED".to_string(),
            format!("{} ({})", st.structure_tag, fmt_cr_opt(st.total_issue_cr)),
        ),
        Some(fp) if fp >= 20.0 => {
            risk_flags.push(format!(
                "OFS-heavy structure ({:.0}% Offer For Sale) — majority of proceeds go to selling shareholders.",
                100.0 - fp
            ));
            (
                45.0,
                "OFS HEAVY".to_string(),
                format!("{} ({})", st.structure_tag, fmt_cr_opt(st.total_issue_cr)),
            )
        }
        Some(fp) => {
            risk_flags.push(format!(
                "{:.0}% Offer For Sale (OFS) — company receives minimal/zero fresh growth capital from this IPO.",
                100.0 - fp
            ));
            (
                28.0,
                "EXIT ISSUE".to_string(),
                format!("{} ({})", st.structure_tag, fmt_cr_opt(st.total_issue_cr)),
            )
        }
        None => (
            55.0,
            "NEUTRAL".to_string(),
            format!("{} ({})", st.structure_tag, fmt_cr_opt(st.total_issue_cr)),
        ),
    };

    // 3. Pillar 3: HNI & Retail Demand Quality (15%)
    let (p3_score, p3_status, p3_sum) = if is_upcoming {
        (
            50.0,
            "PENDING".to_string(),
            "Retail & HNI bid books open on issue start date.".to_string(),
        )
    } else {
        let nii = sub.nii_x.unwrap_or(0.0);
        let rii = sub.retail_x.unwrap_or(0.0);
        let combo = nii * 0.55 + rii * 0.45;
        let mut sc: f64 = if combo >= 30.0 {
            92.0
        } else if combo >= 10.0 {
            82.0
        } else if combo >= 3.0 {
            70.0
        } else if combo >= 1.0 {
            58.0
        } else {
            30.0
        };
        if let Some(co_pct) = sub.retail_cutoff_pct
            && co_pct >= 80.0
            && rii >= 1.0
        {
            sc += 4.0;
        }
        // Check if cumulative bids at cap price are >= 95% of total bids in the demand ladder.
        let cap_concentration = ladder
            .iter()
            .filter(|s| s.price.is_some())
            .max_by(|a, b| {
                a.price
                    .unwrap_or(0.0)
                    .partial_cmp(&b.price.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|s| s.pct_of_total_bids);
        if let Some(cap_pct) = cap_concentration
            && cap_pct >= 95.0
            && sub.total_x.unwrap_or(0.0) >= 1.0
        {
            sc += 3.0;
            bull_points.push(format!(
                "{cap_pct:.1}% of total NSE+BSE bids are committed at the upper price cap."
            ));
        }
        if rii < 1.0 && nii < 1.0 && is_final_day_or_closed {
            risk_flags.push(format!(
                "Undersubscribed in both NII ({nii:.2}x) and Retail ({rii:.2}x)."
            ));
        }
        let sc = sc.clamp(15.0, 98.0);
        (
            sc,
            if sc >= 70.0 {
                "STRONG"
            } else if sc >= 50.0 {
                "MODERATE"
            } else {
                "TEPID"
            }
            .to_string(),
            format!(
                "NII {nii:.2}x (bNII {}, sNII {}) · Retail {rii:.2}x{}",
                sub.bnii_x.map_or("—".to_string(), |v| format!("{v:.2}x")),
                sub.snii_x.map_or("—".to_string(), |v| format!("{v:.2}x")),
                sub.retail_cutoff_pct
                    .map_or(String::new(), |p| format!(" ({p:.0}% at Cut-Off)"))
            ),
        )
    };

    // 4. Pillar 4: Board Quality, Liquidity & BRLM Pedigree (15%)
    let tier1_brlm = has_tier1_brlm(&st.brlms);
    let (p4_score, p4_status, p4_sum) = match (row.board, tier1_brlm) {
        (BoardKind::Mainboard, true) => {
            bull_points.push(format!(
                "Mainboard listing led by a Tier-1 lead manager ({}).",
                st.brlms.first().cloned().unwrap_or_default()
            ));
            (
                88.0,
                "INSTITUTIONAL".to_string(),
                format!(
                    "NSE/BSE Mainboard · Tier-1 Lead Manager ({})",
                    st.brlms.join(", ")
                ),
            )
        }
        (BoardKind::Mainboard, false) => {
            let issue_small = st.total_issue_cr.is_some_and(|c| c < 250.0);
            if issue_small {
                risk_flags.push(format!(
                    "Small Mainboard issue ({}) without a Tier-1 lead manager; post-listing price swings can be large.",
                    fmt_cr_opt(st.total_issue_cr)
                ));
            }
            (
                if issue_small { 58.0 } else { 70.0 },
                "MAINBOARD".to_string(),
                format!(
                    "Mainboard listing · BRLM: {}",
                    if st.brlms.is_empty() {
                        "Not listed".to_string()
                    } else {
                        st.brlms.join(", ")
                    }
                ),
            )
        }
        (BoardKind::Sme, _) => {
            risk_flags.push(
                "SME Board Issue: ₹2L+ minimum commitment, lot-only trading post-listing, and high illiquidity / circuit-lock risk.".to_string(),
            );
            (
                34.0,
                "SME HIGH-RISK".to_string(),
                format!(
                    "{} · Lot-locked liquidity · BRLM: {}",
                    row.board.label(row.is_bse_sme),
                    if st.brlms.is_empty() {
                        "SME Merchant Banker".to_string()
                    } else {
                        st.brlms.join(", ")
                    }
                ),
            )
        }
    };

    // 5. Pillar 5: Street Consensus & Unofficial GMP (10% — reweighted if absent)
    let has_buzz = buzz.gmp_pct.is_some() || buzz.subscribe_mentions > 0 || buzz.avoid_mentions > 0;
    let (p5_score, p5_status, p5_sum) = if !has_buzz {
        (
            50.0,
            "NO SIGNAL".to_string(),
            "No verified broker ratings or GMP figures detected in recent headlines (weight redistributed).".to_string(),
        )
    } else {
        let mut sc: f64 = 55.0;
        if let Some(gp) = buzz.gmp_pct {
            if gp >= 25.0 {
                sc += 30.0;
                bull_points.push(format!(
                    "Press reports cite an unofficial grey market premium of ~{gp:+.1}%."
                ));
            } else if gp >= 10.0 {
                sc += 18.0;
                bull_points.push(format!(
                    "Financial press reports an unofficial GMP of ~{gp:+.1}%."
                ));
            } else if gp > 0.0 {
                sc += 6.0;
            } else if gp < 0.0 {
                sc -= 22.0;
                risk_flags.push(format!(
                    "Negative unofficial GMP ({gp:+.1}%) reported in press."
                ));
            }
        }
        if buzz.subscribe_mentions > buzz.avoid_mentions {
            sc += ((buzz.subscribe_mentions - buzz.avoid_mentions) as f64 * 5.0).min(15.0);
        } else if buzz.avoid_mentions > buzz.subscribe_mentions {
            sc -= ((buzz.avoid_mentions - buzz.subscribe_mentions) as f64 * 8.0).min(24.0);
            risk_flags.push(format!(
                "Press headlines lean cautious ({} mention 'avoid' vs {} 'subscribe').",
                buzz.avoid_mentions, buzz.subscribe_mentions
            ));
        }
        let sc = sc.clamp(15.0, 95.0);
        (
            sc,
            if sc >= 70.0 {
                "POSITIVE"
            } else if sc >= 48.0 {
                "MIXED"
            } else {
                "NEGATIVE"
            }
            .to_string(),
            format!(
                "GMP: {} · Press mentions: {} Subscribe / {} Avoid (unofficial)",
                buzz.gmp_pct.map_or("Not reported".to_string(), |p| {
                    match buzz.gmp_inr {
                        Some(r) => format!("₹{r:.0} ({p:+.1}%)"),
                        None => format!("{p:+.1}%"),
                    }
                }),
                buzz.subscribe_mentions,
                buzz.avoid_mentions
            ),
        )
    };

    // 6. Pillar 6: IPO Market Regime & Macro Backdrop (10%)
    let mut p6_score: f64 = match regime.label.as_str() {
        "HOT" => 88.0,
        "CONSTRUCTIVE" => 72.0,
        "SELECTIVE" => 55.0,
        "COLD" => 30.0,
        _ => 55.0,
    };
    if let Some(vix) = regime.india_vix {
        if vix >= 22.0 {
            p6_score -= 12.0;
            risk_flags.push(format!(
                "Elevated India VIX ({vix:.1}) increases listing-day market volatility risk."
            ));
        } else if vix < 14.5 {
            p6_score += 4.0;
        }
    }
    let p6_score = p6_score.clamp(15.0, 95.0);
    let p6_status = regime.label.clone();
    let p6_sum = regime.summary.clone();

    // Compute overall weighted score (redistributing pending/missing weights cleanly).
    let raw_weights: [(f64, f64, bool); 6] = [
        (p1_score, 30.0, !is_upcoming),
        (p2_score, 20.0, true),
        (p3_score, 15.0, !is_upcoming),
        (p4_score, 15.0, true),
        (p5_score, 10.0, has_buzz),
        (p6_score, 10.0, regime.sample_size > 0),
    ];
    let active_weight_sum: f64 = raw_weights
        .iter()
        .filter(|(_, _, active)| *active)
        .map(|(_, w, _)| *w)
        .sum();
    let overall_score = if active_weight_sum > 0.0 {
        let weighted: f64 = raw_weights
            .iter()
            .filter(|(_, _, active)| *active)
            .map(|(s, w, _)| s * w)
            .sum();
        round(weighted / active_weight_sum, 1)
    } else {
        50.0
    };

    // Separate Listing-Gain Verdict vs Long-Term Hold Verdict!
    let listing_score = if is_upcoming {
        overall_score
    } else {
        round(
            p1_score * 0.38
                + p3_score * 0.24
                + (if has_buzz { p5_score } else { overall_score }) * 0.18
                + p6_score * 0.12
                + p4_score * 0.08,
            1,
        )
    };
    let long_term_score = round(
        p2_score * 0.35 + p4_score * 0.30 + p1_score * 0.25 + p6_score * 0.10,
        1,
    );

    let listing_demand = match row.status {
        IpoStatus::Upcoming => "Upcoming: book not open yet".to_string(),
        IpoStatus::Listed => match row.return_from_issue_pct {
            Some(r) => format!(
                "Listed ({r:+.1}% vs issue price); see `stockwise verdict {}`",
                row.symbol
            ),
            None => format!("Already listed; see `stockwise verdict {}`", row.symbol),
        },
        IpoStatus::ClosedPendingListing => {
            if listing_score >= 75.0 {
                "Strong demand at close".to_string()
            } else if listing_score >= 55.0 {
                "Moderate demand at close".to_string()
            } else {
                "Weak demand at close".to_string()
            }
        }
        IpoStatus::Ongoing => {
            if !is_final_day_or_closed && sub.qib_x.unwrap_or(0.0) < 2.0 {
                "Book still building: QIB bids usually arrive on the last day".to_string()
            } else if row.board == BoardKind::Sme && listing_score >= 68.0 {
                "Strong demand (SME: lot-locked liquidity)".to_string()
            } else if listing_score >= 78.0 {
                "Strong demand".to_string()
            } else if listing_score >= 64.0 {
                "Moderate demand".to_string()
            } else if listing_score >= 50.0 {
                "Mixed demand".to_string()
            } else {
                "Weak demand".to_string()
            }
        }
    };

    let long_term_profile = if row.board == BoardKind::Sme {
        if long_term_score >= 65.0 {
            "Higher-risk SME profile; no earnings history yet".to_string()
        } else {
            "Weak profile: SME illiquidity".to_string()
        }
    } else if long_term_score >= 76.0 {
        "Strong institutional demand and issue structure".to_string()
    } else if long_term_score >= 60.0 {
        "Mixed profile".to_string()
    } else if st.fresh_pct.is_some_and(|f| f < 25.0) {
        "Weak profile: mostly offer for sale".to_string()
    } else {
        "Weak institutional demand and issue structure".to_string()
    };

    let confidence = if active_weight_sum >= 85.0 && is_final_day_or_closed {
        "HIGH (Full bid book & issue structure)"
    } else if active_weight_sum >= 70.0 {
        "MEDIUM (Live book still building)"
    } else {
        "PRELIMINARY (Pre-open / partial filings)"
    }
    .to_string();

    let timeline = match row.status {
        IpoStatus::Upcoming => format!(
            "Opens {}. The anchor allocation report is published the evening before opening.",
            row.open_date
                .map_or("soon".to_string(), |d| d.format("%d %b %Y").to_string())
        ),
        IpoStatus::Ongoing => {
            if is_final_day_or_closed {
                "Closing day: the UPI mandate deadline is 5:00 PM IST, and QIB bids typically arrive between 1:30 PM and 3:15 PM IST.".to_string()
            } else {
                format!(
                    "Issue closes {}. QIBs typically bid on the final afternoon.",
                    row.close_date
                        .map_or("soon".to_string(), |d| d.format("%d %b %Y").to_string())
                )
            }
        }
        IpoStatus::ClosedPendingListing => format!(
            "Bidding closed. Allotment status is published on the registrar portal{}.",
            st.registrar_name
                .as_deref()
                .map_or(String::new(), |r| format!(" ({r})"))
        ),
        IpoStatus::Listed => format!(
            "Trading in the secondary market; `stockwise verdict {}` shows its 6-pillar score.",
            row.symbol
        ),
    };

    if bull_points.is_empty() {
        bull_points.push("Exchange book-built offering with ASBA/UPI bidding.".to_string());
    }
    if risk_flags.is_empty() {
        risk_flags.push(
            "Valuation multiples and contingent liabilities in the RHP are not part of this score."
                .to_string(),
        );
    }

    let pillars = vec![
        IpoPillarScore {
            name: "Smart Money & QIB Book".to_string(),
            weight_pct: 30,
            score: round(p1_score, 1),
            status: p1_status,
            summary: p1_sum,
        },
        IpoPillarScore {
            name: "Issue Structure (Fresh vs OFS)".to_string(),
            weight_pct: 20,
            score: round(p2_score, 1),
            status: p2_status,
            summary: p2_sum,
        },
        IpoPillarScore {
            name: "HNI & Retail Bid Quality".to_string(),
            weight_pct: 15,
            score: round(p3_score, 1),
            status: p3_status,
            summary: p3_sum,
        },
        IpoPillarScore {
            name: "Board, Liquidity & BRLM".to_string(),
            weight_pct: 15,
            score: round(p4_score, 1),
            status: p4_status,
            summary: p4_sum,
        },
        IpoPillarScore {
            name: "Street Consensus & GMP".to_string(),
            weight_pct: 10,
            score: round(p5_score, 1),
            status: p5_status,
            summary: p5_sum,
        },
        IpoPillarScore {
            name: "IPO Regime & Macro".to_string(),
            weight_pct: 10,
            score: round(p6_score, 1),
            status: p6_status,
            summary: p6_sum,
        },
    ];

    IpoDualVerdict {
        overall_score,
        listing_demand,
        long_term_profile,
        confidence,
        timeline,
        pillars,
        bull_points,
        risk_flags,
    }
}

// ── Street Buzz / GMP RSS Parser ──

pub fn parse_street_buzz(rss_xml: &str, cap_price: Option<f64>) -> StreetBuzz {
    let headlines = parse_rss_headlines(rss_xml, 8);
    let mut gmp_pcts = Vec::new();
    let mut gmp_inrs = Vec::new();
    let mut subscribe_mentions = 0usize;
    let mut avoid_mentions = 0usize;
    let mut neutral_mentions = 0usize;

    for h in &headlines {
        let lc = h.title.to_ascii_lowercase();
        if (lc.contains("subscribe")
            && !lc.contains("should you subscribe")
            && !lc.contains("subscribed"))
            || lc.contains("apply for")
            || lc.contains("thumbs up")
        {
            subscribe_mentions += 1;
        }
        if lc.contains("avoid") || lc.contains("skip") || lc.contains("stay away") {
            avoid_mentions += 1;
        }
        if lc.contains("neutral") || lc.contains("may apply") || lc.contains("high risk") {
            neutral_mentions += 1;
        }
        if lc.contains("gmp") || lc.contains("grey market") {
            let (inr_opt, pct_opt) = extract_gmp_from_headline(&h.title, cap_price);
            if let Some(r) = inr_opt {
                gmp_inrs.push(r);
            }
            if let Some(p) = pct_opt {
                gmp_pcts.push(p);
            }
        }
    }

    let gmp_pct = median_opt(&mut gmp_pcts);
    let gmp_inr = median_opt(&mut gmp_inrs).or_else(|| match (gmp_pct, cap_price) {
        (Some(p), Some(cap)) => Some(round(cap * p / 100.0, 1)),
        _ => None,
    });
    let gmp_pct = gmp_pct.or_else(|| match (gmp_inr, cap_price) {
        (Some(r), Some(cap)) if cap > 0.0 => Some(round(r / cap * 100.0, 1)),
        _ => None,
    });

    StreetBuzz {
        gmp_inr,
        gmp_pct,
        subscribe_mentions,
        avoid_mentions,
        neutral_mentions,
        headlines,
        caveat: "GMP is unregulated, unofficial grey-market chatter extracted from financial news headlines and can vanish before listing.".to_string(),
    }
}

fn extract_gmp_from_headline(title: &str, cap_price: Option<f64>) -> (Option<f64>, Option<f64>) {
    let lc = title.to_ascii_lowercase();
    let Some(gmp_pos) = lc.find("gmp").or_else(|| lc.find("grey market")) else {
        return (None, None);
    };
    // Scan a window around "gmp" for `X%` or `Rs X` / `₹X`.
    let start = gmp_pos.saturating_sub(20);
    let end = (gmp_pos + 55).min(title.len());
    let start = title.floor_char_boundary(start);
    let end = title.ceil_char_boundary(end);
    let window = &title[start..end];
    let win_lc = window.to_ascii_lowercase();
    let sign = if win_lc.contains("discount")
        || win_lc.contains("minus")
        || win_lc.contains("negative")
        || has_negative_number(&win_lc)
    {
        -1.0
    } else {
        1.0
    };

    let mut pct_val: Option<f64> = None;
    let mut inr_val: Option<f64> = None;

    // Look for `<num>%`
    for (idx, _) in window.match_indices('%') {
        if let Some(n) = number_before(&window[..idx])
            && n <= 300.0
        {
            pct_val = Some(round(n * sign, 1));
            break;
        }
    }

    // Look for `₹<num>` or `rs <num>` or `rs.<num>`
    for marker in ["₹", "rs.", "rs "] {
        if let Some(pos) = win_lc.find(marker) {
            let after = &window[pos + marker.len()..];
            if let Some(n) = first_f64_in_text(after)
                && cap_price.is_none_or(|c| n <= c * 2.5)
            {
                inr_val = Some(round(n * sign, 1));
                break;
            }
        }
    }

    (inr_val, pct_val)
}

/// A minus sign right before a figure (`-8%`, `- ₹20`, `−₹5`), not a hyphen
/// between words (`high-demand`).
fn has_negative_number(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.iter().enumerate().any(|(i, c)| {
        if !matches!(c, '-' | '−' | '–') {
            return false;
        }
        // The sign must not be glued to a word on its left.
        if i > 0 && chars[i - 1].is_alphanumeric() {
            return false;
        }
        chars[i + 1..]
            .iter()
            .skip_while(|n| n.is_whitespace() || matches!(n, '₹'))
            .take(1)
            .any(char::is_ascii_digit)
    })
}

pub fn parse_rss_headlines(xml: &str, max_items: usize) -> Vec<IpoHeadline> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(item_idx) = rest.find("<item>") {
        let after = &rest[item_idx + 6..];
        let Some(end_idx) = after.find("</item>") else {
            break;
        };
        let block = &after[..end_idx];
        rest = &after[end_idx + 7..];

        let Some(raw_title) = xml_tag(block, "title") else {
            continue;
        };
        let decoded = decode_xml_entities(&raw_title);
        let (title, source_from_title) = match decoded.rsplit_once(" - ") {
            Some((t, s)) if !t.trim().is_empty() && s.len() <= 45 => {
                (t.trim().to_string(), Some(s.trim().to_string()))
            }
            _ => (decoded.trim().to_string(), None),
        };
        let source = xml_tag(block, "source")
            .map(|s| decode_xml_entities(&s))
            .or(source_from_title)
            .unwrap_or_else(|| "News".to_string());

        if !title.is_empty()
            && !out
                .iter()
                .any(|x: &IpoHeadline| x.title.eq_ignore_ascii_case(&title))
        {
            out.push(IpoHeadline { title, source });
            if out.len() >= max_items {
                break;
            }
        }
    }
    out
}

// ── Target Resolution & Text Utilities ──

fn resolve_ipo_target(
    query: &str,
    forced_series: Option<&str>,
    ongoing: &[IpoSummaryRow],
    upcoming: &[IpoSummaryRow],
    past: &[IpoSummaryRow],
) -> Option<(String, IpoSummaryRow)> {
    let q = query
        .trim()
        .trim_end_matches(".NS")
        .trim_end_matches(".BO")
        .trim();
    let q_up = q.to_ascii_uppercase();
    let q_lc = q.to_ascii_lowercase();
    let series_ok = |r: &IpoSummaryRow| {
        forced_series.is_none_or(|s| r.board.series_code().eq_ignore_ascii_case(s))
    };

    let pools = [ongoing, upcoming, past];
    // 1. Exact symbol match.
    for pool in pools {
        if let Some(r) = pool.iter().find(|r| r.symbol == q_up && series_ok(r)) {
            return Some((r.symbol.clone(), r.clone()));
        }
    }
    // 2. Prefix / substring match on symbol or company name.
    for pool in pools {
        if let Some(r) = pool.iter().find(|r| {
            series_ok(r)
                && (r.company_name.to_ascii_lowercase().contains(&q_lc)
                    || r.symbol.to_ascii_lowercase().contains(&q_lc))
        }) {
            return Some((r.symbol.clone(), r.clone()));
        }
    }
    None
}

fn derive_quick_signal(row: &IpoSummaryRow, today: NaiveDate) -> String {
    match row.status {
        IpoStatus::Upcoming => "UPCOMING".to_string(),
        IpoStatus::ClosedPendingListing => "AWAITING LISTING".to_string(),
        IpoStatus::Listed => "LISTED".to_string(),
        IpoStatus::Ongoing => {
            let final_day = row.close_date.is_some_and(|cd| today >= cd);
            let qib = row.qib_x.unwrap_or(0.0);
            let tot = row.total_x.unwrap_or(0.0);
            if row.board == BoardKind::Sme {
                if tot >= 15.0 {
                    "SME · STRONG DEMAND".to_string()
                } else if tot >= 2.0 {
                    "SME · MODERATE DEMAND".to_string()
                } else {
                    "SME · WEAK DEMAND".to_string()
                }
            } else if qib >= 10.0 || (qib >= 3.0 && tot >= 8.0) {
                "STRONG · QIB BACKED".to_string()
            } else if qib >= 1.5 && tot >= 2.0 {
                "MODERATE DEMAND".to_string()
            } else if !final_day {
                "BOOK BUILDING".to_string()
            } else if tot >= 1.0 {
                "MIXED DEMAND".to_string()
            } else {
                "WEAK DEMAND".to_string()
            }
        }
    }
}

fn compute_window_note(
    status: IpoStatus,
    open: Option<NaiveDate>,
    close: Option<NaiveDate>,
    today: NaiveDate,
) -> String {
    match status {
        IpoStatus::Ongoing => match (open, close) {
            (_, Some(cd)) if cd == today => "CLOSES TODAY (UPI 5 PM IST)".to_string(),
            (_, Some(cd)) if cd > today => {
                let left = (cd - today).num_days();
                format!("Closes {} ({}d left)", cd.format("%d %b"), left)
            }
            (Some(od), Some(cd)) => format!("{} – {}", od.format("%d %b"), cd.format("%d %b")),
            _ => "Open now".to_string(),
        },
        IpoStatus::Upcoming => match (open, close) {
            (Some(od), Some(cd)) => {
                let in_days = (od - today).num_days();
                if in_days == 0 {
                    format!("Opens today · closes {}", cd.format("%d %b"))
                } else if in_days > 0 {
                    format!(
                        "{} – {} (in {}d)",
                        od.format("%d %b"),
                        cd.format("%d %b"),
                        in_days
                    )
                } else {
                    format!("{} – {}", od.format("%d %b"), cd.format("%d %b"))
                }
            }
            (Some(od), None) => format!("Opens {}", od.format("%d %b %Y")),
            _ => "Dates TBA".to_string(),
        },
        IpoStatus::ClosedPendingListing => close.map_or("Allotment pending".to_string(), |cd| {
            format!("Closed {} · Allotment pending", cd.format("%d %b"))
        }),
        IpoStatus::Listed => "Listed".to_string(),
    }
}

pub fn parse_price_band(raw: &str) -> PriceBand {
    let trimmed = raw.trim();
    let nums = extract_all_numbers(trimmed);
    let (low, high) = match nums.as_slice() {
        [] => (None, None),
        [single] if *single > 0.0 => (Some(*single), Some(*single)),
        [a, b, ..] if *a > 0.0 && *b > 0.0 => (Some(a.min(*b)), Some(a.max(*b))),
        _ => (None, None),
    };
    PriceBand {
        low,
        high,
        raw: trimmed.to_string(),
    }
}

pub fn parse_nse_date(s: &str) -> Option<NaiveDate> {
    let cleaned = s.trim().trim_matches('"').trim();
    if cleaned.is_empty() || cleaned == "-" {
        return None;
    }
    // Common NSE formats: `29-Oct-2026`, `29-OCT-2026`, `2026-10-28`
    if let Ok(d) = NaiveDate::parse_from_str(cleaned, "%Y-%m-%d") {
        return Some(d);
    }
    let parts: Vec<&str> = cleaned
        .split(['-', ' ', '/'])
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() == 3 {
        let day: u32 = parts[0].parse().ok()?;
        let month = match parts[1].to_ascii_lowercase().get(..3)? {
            "jan" => 1,
            "feb" => 2,
            "mar" => 3,
            "apr" => 4,
            "may" => 5,
            "jun" => 6,
            "jul" => 7,
            "aug" => 8,
            "sep" => 9,
            "oct" => 10,
            "nov" => 11,
            "dec" => 12,
            _ => return None,
        };
        let year: i32 = parts[2].parse().ok()?;
        return NaiveDate::from_ymd_opt(year, month, day);
    }
    None
}

fn parse_issue_period(s: &str) -> (Option<NaiveDate>, Option<NaiveDate>) {
    if let Some((a, b)) = s.split_once(" to ") {
        (parse_nse_date(a), parse_nse_date(b))
    } else {
        (None, None)
    }
}

fn extract_amount_cr(text: &str, cap_price: Option<f64>) -> Option<f64> {
    let lc = text.to_ascii_lowercase();
    // Look for `<number> crore` or `<number> cr` or `<number> million` or `<number> lakh`.
    for (unit, factor_to_cr) in [
        ("crores", 1.0),
        ("crore", 1.0),
        (" cr", 1.0),
        ("million", 0.1),
        ("mn", 0.1),
        ("lakhs", 0.01),
        ("lakh", 0.01),
    ] {
        if let Some(pos) = lc.find(unit)
            && let Some(num) = number_before(&text[..pos])
            && num > 0.0
        {
            return Some(round(num * factor_to_cr, 2));
        }
    }
    // Otherwise look for `<number> equity shares` × cap_price!
    if let Some(cap) = cap_price
        && let Some(pos) = lc.find("equity share")
        && let Some(shares) = number_before(&text[..pos])
        && shares >= 10_000.0
    {
        return Some(round(shares * cap / 1e7, 2));
    }
    None
}

fn extract_shares_near(lc_text: &str, keyword: &str) -> Option<u64> {
    let idx = lc_text.find(keyword)?;
    let after = &lc_text[idx..];
    let share_idx = after.find("equity share").or_else(|| after.find("share"))?;
    let n = number_before(&after[..share_idx])?;
    if n >= 100.0 {
        Some(n.round() as u64)
    } else {
        None
    }
}

fn registrar_portal_url(registrar: &str) -> Option<&'static str> {
    let lc = registrar.to_ascii_lowercase();
    if lc.contains("kfin") {
        Some("https://kosmic.kfintech.com/ipostatus/")
    } else if lc.contains("link intime") || lc.contains("mufg intime") {
        Some("https://in.mpms.mufg.com/Initial_Offer/public-issues.html")
    } else if lc.contains("bigshare") {
        Some("https://ipo.bigshareonline.com/IPO_Status.html")
    } else if lc.contains("cameo") {
        Some("https://ipo.cameoindia.com/")
    } else if lc.contains("skyline") {
        Some("https://www.skylinerta.com/ipo.php")
    } else if lc.contains("purva") {
        Some("https://www.purvashare.com/investor-service/ipo-query")
    } else if lc.contains("maashitla") {
        Some("https://maashitla.com/allotment-status/public-issues")
    } else if lc.contains("mas services") {
        Some("https://www.masserv.com/opt.asp")
    } else {
        None
    }
}

fn has_tier1_brlm(brlms: &[String]) -> bool {
    const TIER1: &[&str] = &[
        "kotak",
        "icici securities",
        "axis capital",
        "jm financial",
        "iifl",
        "motilal oswal",
        "sbi capital",
        "citigroup",
        "morgan stanley",
        "goldman sachs",
        "j.p. morgan",
        "jp morgan",
        "bofa",
        "merrill",
        "nomura",
        "jefferies",
        "avendus",
        "dam capital",
        "nuvama",
        "edelweiss",
        "hdfc bank",
        "ambit",
        "hsbc",
        "clsa",
    ];
    brlms.iter().any(|b| {
        let lc = b.to_ascii_lowercase();
        TIER1.iter().any(|t| lc.contains(t))
    })
}

fn extract_url(s: &str) -> Option<String> {
    if let Some(idx) = s.find("https://").or_else(|| s.find("http://")) {
        let rest = &s[idx..];
        let end = rest.find([' ', '"', '\'', '>', ')']).unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    None
}

fn clean_company_for_search(name: &str, symbol: &str) -> String {
    let mut s = name.trim().to_string();
    for suffix in [
        " Limited", " Ltd.", " Ltd", " Private", " Pvt.", " Pvt", " (India)",
    ] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.trim().to_string();
        }
    }
    if s.is_empty() { symbol.to_string() } else { s }
}

fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn num_field(v: &Value, keys: &[&str]) -> Option<f64> {
    for k in keys {
        match v.get(*k) {
            Some(Value::Number(n)) => {
                if let Some(f) = n.as_f64()
                    && f.is_finite()
                {
                    return Some(f);
                }
            }
            Some(Value::String(s)) => {
                if let Some(f) = parse_indian_number(s) {
                    return Some(f);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_indian_number(s: &str) -> Option<f64> {
    let cleaned: String = s
        .trim()
        .chars()
        .filter(|c| *c != ',' && *c != ' ')
        .collect();
    if cleaned.is_empty() || cleaned == "-" {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn extract_all_numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() || (ch == '.' && !buf.is_empty() && !buf.contains('.')) {
            buf.push(ch);
        } else if ch == ',' && !buf.is_empty() {
            continue;
        } else if !buf.is_empty() {
            if let Ok(v) = buf.trim_end_matches('.').parse::<f64>() {
                out.push(v);
            }
            buf.clear();
        }
    }
    if !buf.is_empty()
        && let Ok(v) = buf.trim_end_matches('.').parse::<f64>()
    {
        out.push(v);
    }
    out
}

fn first_u64_in_text(s: &str) -> Option<u64> {
    extract_all_numbers(s)
        .into_iter()
        .find(|&n| n >= 1.0)
        .map(|n| n.round() as u64)
}

fn first_f64_in_text(s: &str) -> Option<f64> {
    extract_all_numbers(s).into_iter().next()
}

fn number_before(s: &str) -> Option<f64> {
    extract_all_numbers(s).into_iter().last()
}

fn median_opt(vals: &mut [f64]) -> Option<f64> {
    if vals.is_empty() {
        return None;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = vals.len() / 2;
    if vals.len().is_multiple_of(2) {
        Some(round((vals[mid - 1] + vals[mid]) / 2.0, 1))
    } else {
        Some(round(vals[mid], 1))
    }
}

fn xml_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = block.find(&open)?;
    let gt = block[start..].find('>')?;
    let content_start = start + gt + 1;
    let end = block[content_start..].find(&close)?;
    let raw = block[content_start..content_start + end].trim();
    let stripped = raw
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(raw);
    Some(stripped.trim().to_string())
}

fn decode_xml_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn ist_today() -> NaiveDate {
    (Utc::now() + chrono::Duration::seconds(19_800)).date_naive()
}

pub fn fmt_price(p: f64) -> String {
    if (p.fract()).abs() < 0.005 {
        format!("{:.0}", p)
    } else {
        format!("{:.2}", p)
    }
}

pub fn fmt_int_indian(n: u64) -> String {
    let s = n.to_string();
    if s.len() <= 3 {
        return s;
    }
    let (head, tail) = s.split_at(s.len() - 3);
    let mut groups = Vec::new();
    let mut rem = head;
    while rem.len() > 2 {
        let (h, t) = rem.split_at(rem.len() - 2);
        groups.push(t);
        rem = h;
    }
    if !rem.is_empty() {
        groups.push(rem);
    }
    groups.reverse();
    format!("{},{tail}", groups.join(","))
}

pub fn fmt_inr(amount: f64) -> String {
    fmt_int_indian(amount.round().max(0.0) as u64)
}

fn fmt_cr_opt(cr: Option<f64>) -> String {
    match cr {
        Some(v) if v > 0.0 => format!("₹{v:.0} Cr"),
        _ => "Size TBA".to_string(),
    }
}

// ── Deterministic offline fixtures (unit tests) ──

#[cfg(test)]
pub fn fixture_board(filter: BoardFilter, past_limit: usize) -> IpoBoardView {
    let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let all_ongoing = vec![
        IpoSummaryRow {
            symbol: "BCSSL".into(),
            company_name: "Bombay Coated and Special Steels Limited".into(),
            board: BoardKind::Mainboard,
            is_bse_sme: false,
            status: IpoStatus::Ongoing,
            open_date: NaiveDate::from_ymd_opt(2026, 10, 29),
            close_date: NaiveDate::from_ymd_opt(2026, 11, 2),
            listing_date: None,
            window_note: "Closes 02 Nov (1d left)".into(),
            price_band: parse_price_band("Rs.78 to Rs.82"),
            issue_price: None,
            lot_size: Some(182),
            min_investment_inr: Some(14_924.0),
            issue_size_cr: Some(183.06),
            qib_x: Some(6.56),
            nii_x: Some(3.84),
            retail_x: Some(4.20),
            total_x: Some(4.78),
            ltp: None,
            return_from_issue_pct: None,
            quick_signal: "STRONG · QIB BACKED".into(),
        },
        IpoSummaryRow {
            symbol: "MAXENGS".into(),
            company_name: "Maxwell Engineering Solutions Limited".into(),
            board: BoardKind::Sme,
            is_bse_sme: true,
            status: IpoStatus::Ongoing,
            open_date: NaiveDate::from_ymd_opt(2026, 10, 30),
            close_date: NaiveDate::from_ymd_opt(2026, 11, 3),
            listing_date: None,
            window_note: "Closes 03 Nov (2d left)".into(),
            price_band: parse_price_band("Rs.150 to Rs.158"),
            issue_price: None,
            lot_size: Some(800),
            min_investment_inr: Some(252_800.0),
            issue_size_cr: Some(57.64),
            qib_x: Some(0.0),
            nii_x: Some(4.17),
            retail_x: Some(5.10),
            total_x: Some(4.17),
            ltp: None,
            return_from_issue_pct: None,
            quick_signal: "SME · ACTIVE".into(),
        },
    ];
    let all_upcoming = vec![IpoSummaryRow {
        symbol: "VPFL".into(),
        company_name: "Vedant Poly Fabs Limited".into(),
        board: BoardKind::Sme,
        is_bse_sme: false,
        status: IpoStatus::Upcoming,
        open_date: NaiveDate::from_ymd_opt(2026, 11, 2),
        close_date: NaiveDate::from_ymd_opt(2026, 11, 4),
        listing_date: None,
        window_note: "02 Nov – 04 Nov (in 1d)".into(),
        price_band: parse_price_band("Rs.88 to Rs.94"),
        issue_price: None,
        lot_size: Some(1200),
        min_investment_inr: Some(225_600.0),
        issue_size_cr: Some(56.68),
        qib_x: None,
        nii_x: None,
        retail_x: None,
        total_x: None,
        ltp: None,
        return_from_issue_pct: None,
        quick_signal: "UPCOMING".into(),
    }];
    let all_recent = vec![
        IpoSummaryRow {
            symbol: "JPL".into(),
            company_name: "Jio Platforms Limited".into(),
            board: BoardKind::Mainboard,
            is_bse_sme: false,
            status: IpoStatus::Listed,
            open_date: NaiveDate::from_ymd_opt(2026, 10, 21),
            close_date: NaiveDate::from_ymd_opt(2026, 10, 23),
            listing_date: NaiveDate::from_ymd_opt(2026, 10, 28),
            window_note: "Listed 28 Oct 2026".into(),
            price_band: parse_price_band("Rs.1065 to Rs.1119"),
            issue_price: Some(1119.0),
            lot_size: Some(13),
            min_investment_inr: Some(14_547.0),
            issue_size_cr: Some(15_000.0),
            qib_x: Some(38.4),
            nii_x: Some(19.2),
            retail_x: Some(7.5),
            total_x: Some(21.6),
            ltp: Some(1298.0),
            return_from_issue_pct: Some(16.0),
            quick_signal: "+16.0% WINNER".into(),
        },
        IpoSummaryRow {
            symbol: "PARASHLTH".into(),
            company_name: "Paras Healthcare Limited".into(),
            board: BoardKind::Mainboard,
            is_bse_sme: false,
            status: IpoStatus::Listed,
            open_date: NaiveDate::from_ymd_opt(2026, 10, 21),
            close_date: NaiveDate::from_ymd_opt(2026, 10, 23),
            listing_date: NaiveDate::from_ymd_opt(2026, 10, 28),
            window_note: "Listed 28 Oct 2026".into(),
            price_band: parse_price_band("Rs.660 to Rs.693"),
            issue_price: Some(693.0),
            lot_size: Some(21),
            min_investment_inr: Some(14_553.0),
            issue_size_cr: Some(900.0),
            qib_x: Some(14.1),
            nii_x: Some(8.5),
            retail_x: Some(4.1),
            total_x: Some(8.9),
            ltp: Some(745.0),
            return_from_issue_pct: Some(7.5),
            quick_signal: "+7.5% ABOVE ISSUE".into(),
        },
    ];

    let regime = IpoRegime {
        label: "CONSTRUCTIVE".into(),
        sample_size: 2,
        positive_count: 2,
        hit_rate_pct: 100.0,
        avg_return_pct: 11.75,
        median_return_pct: 11.75,
        best_recent: Some(("JPL".into(), 16.0)),
        worst_recent: Some(("PARASHLTH".into(), 7.5)),
        nifty_change_pct: Some(0.5),
        india_vix: Some(13.8),
        summary: "2/2 recent NSE listings trading above issue price (100% hit-rate, avg +11.8%, median +11.8%).".into(),
    };

    IpoBoardView {
        as_of: today,
        filter,
        regime,
        ongoing: all_ongoing
            .into_iter()
            .filter(|r| filter.matches(r.board))
            .collect(),
        upcoming: all_upcoming
            .into_iter()
            .filter(|r| filter.matches(r.board))
            .collect(),
        recent: all_recent
            .into_iter()
            .filter(|r| filter.matches(r.board))
            .take(past_limit)
            .collect(),
        headlines: vec![IpoHeadline {
            title: "Bombay Coated IPO subscribed over 4.7x; QIBs lead demand, GMP at 14%".into(),
            source: "Economic Times".into(),
        }],
        notes: Vec::new(),
    }
}

#[cfg(test)]
pub fn fixture_analysis(
    query: &str,
    forced_series: Option<&str>,
    capital: Option<f64>,
) -> Result<IpoAnalysis> {
    let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let board = fixture_board(BoardFilter::All, 10);
    let Some((_, row)) = resolve_ipo_target(
        query,
        forced_series,
        &board.ongoing,
        &board.upcoming,
        &board.recent,
    ) else {
        bail!("No Indian IPO found matching '{}'", query.trim());
    };

    let detail = serde_json::json!({
        "companyName": row.company_name,
        "metaInfo": {
            "industry": "Iron & Steel Products",
            "isin": "INE0BCSSL010"
        },
        "activeCat": {
            "dataList": [
                {"srNo": "1", "category": "Qualified Institutional Buyers(QIBs)", "noOfShareOffered": "4798588", "noOfSharesBid": "31455424", "noOfTotalMeant": "6.56"},
                {"srNo": "1(a)", "category": "Foreign Institutional Investors(FIIs)", "noOfShareOffered": "", "noOfSharesBid": "11000000", "noOfTotalMeant": ""},
                {"srNo": "1(c)", "category": "Mutual funds", "noOfShareOffered": "", "noOfSharesBid": "15455424", "noOfTotalMeant": ""},
                {"srNo": "2", "category": "Non Institutional Investors", "noOfShareOffered": "3520327", "noOfSharesBid": "13518055", "noOfTotalMeant": "3.84"},
                {"srNo": "2.1", "category": "Non Institutional Investors(Bid amount of more than Ten Lakh Rupees)", "noOfShareOffered": "2346885", "noOfSharesBid": "9622228", "noOfTotalMeant": "4.10"},
                {"srNo": "2.2", "category": "Non Institutional Investors(Bid amount of more than Two Lakh Rupees upto Ten Lakh Rupees)", "noOfShareOffered": "1173442", "noOfSharesBid": "3895827", "noOfTotalMeant": "3.32"},
                {"srNo": "3", "category": "Retail Individual Investors(RIIs)", "noOfShareOffered": "8214097", "noOfSharesBid": "34499207", "noOfTotalMeant": "4.20"},
                {"srNo": "Total", "category": "Total", "noOfShareOffered": "16533012", "noOfSharesBid": "79472686", "noOfTotalMeant": "4.81"}
            ]
        },
        "bidDetails": [
            {"srNo": "3(a)", "category": "Cut Off", "noOfsharesBid": "30100000"},
            {"srNo": "3(b)", "category": "Price bids", "noOfsharesBid": "4399207"}
        ],
        "demandGraphALL": {
            "TOTAL_BIDS": "79472686",
            "totalIssueSize": "16533012",
            "noOfTimesIssueSubscribed": "4.81",
            "plotData": {
                "Cut-Off": "3,01,00,000",
                "82": "7,85,00,000",
                "81": "7,90,00,000",
                "78": "7,94,72,686"
            }
        },
        "issueInfo": {
            "dataList": [
                {"title": "Issue Size", "value": "Initial Public Offer comprising of Fresh Issue aggregating upto Rs. 1,830.57 million (including Anchor reservation portion of 65,97,500 Equity Shares)"},
                {"title": "Issue Type", "value": "Book Building"},
                {"title": "Price Range", "value": "Rs. 78/- to Rs. 82/- per Equity Share"},
                {"title": "Bid Lot", "value": "182 Equity Shares and in multiples thereof"},
                {"title": "Book Running Lead Managers", "value": "Kotak Mahindra Capital Company Limited"},
                {"title": "Name of the Registrar", "value": "KFin Technologies Limited"},
                {"title": "Red Herring Prospectus", "value": "https://nsearchives.nseindia.com/content/ipo/RHP_BCSSL.zip"},
                {"title": "Anchor Allocation Report", "value": "https://nsearchives.nseindia.com/content/ipo/ANCHOR_BCSSL.zip"}
            ]
        }
    });
    let rss = r#"<rss><channel>
        <item><title>Bombay Coated IPO opens; GMP at 14%, brokerages say Subscribe - Economic Times</title></item>
        <item><title>Bombay Coated IPO receives Subscribe call from analysts on strong capex - Livemint</title></item>
    </channel></rss>"#;

    Ok(build_analysis_from_parts(
        row,
        if query.trim().eq_ignore_ascii_case("MAXENGS") {
            None
        } else {
            Some(&detail)
        },
        rss,
        board.regime,
        capital,
        today,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_band_parses_nse_formats() {
        let b = parse_price_band("Rs. 78/- to Rs. 82/- per Equity Share");
        assert_eq!(b.low, Some(78.0));
        assert_eq!(b.high, Some(82.0));
        assert_eq!(b.display_short(), "₹78–₹82");

        let single = parse_price_band("  1119");
        assert_eq!(single.cap_price(), Some(1119.0));
        assert_eq!(single.display_short(), "₹1119");

        let tba = parse_price_band("-");
        assert_eq!(tba.cap_price(), None);
        assert_eq!(tba.display_short(), "TBA");
    }

    #[test]
    fn fresh_vs_ofs_and_share_based_issue_size_parse_cleanly() {
        let mut fresh = IssueStructure {
            raw_issue_size: "Initial Public Offer comprising of Fresh Issue aggregating upto Rs. 1,830.57 million (including Anchor reservation portion of 65,97,500 Equity Shares)".into(),
            ..Default::default()
        };
        populate_fresh_vs_ofs(&mut fresh, Some(82.0));
        assert_eq!(fresh.fresh_pct, Some(100.0));
        assert_eq!(fresh.total_issue_cr, Some(183.06));
        assert_eq!(fresh.anchor_shares, Some(6_597_500));

        let mut mixed = IssueStructure {
            raw_issue_size: "Fresh Issue of Rs. 600 crore and Offer for Sale of Rs. 400 crore"
                .into(),
            ..Default::default()
        };
        populate_fresh_vs_ofs(&mut mixed, Some(500.0));
        assert_eq!(mixed.total_issue_cr, Some(1000.0));
        assert_eq!(mixed.fresh_pct, Some(60.0));
        assert!(mixed.structure_tag.contains("60% Fresh"));

        let mut sme_shares = IssueStructure {
            raw_issue_size: "Initial Public Offer comprising of Fresh issue of up to 36,48,000 equity shares (including Market Maker portion of 2,22,400 Equity Shares)".into(),
            ..Default::default()
        };
        populate_fresh_vs_ofs(&mut sme_shares, Some(158.0));
        // 36,48,000 * 158 = ₹57.64 Cr
        assert_eq!(sme_shares.total_issue_cr, Some(57.64));
        assert_eq!(sme_shares.market_maker_shares, Some(222_400));
    }

    #[test]
    fn sebi_allotment_optimizer_models_retail_snii_bnii_and_multi_pan() {
        let sub = SubscriptionBreakdown {
            retail_x: Some(5.0),
            snii_x: Some(10.0),
            bnii_x: Some(10.0),
            total_x: Some(7.0),
            ..Default::default()
        };
        // 1 lot = 182 * 82 = ₹14,924
        let opt = build_allotment_optimizer(
            BoardKind::Mainboard,
            IpoStatus::Ongoing,
            Some(182),
            Some(82.0),
            &sub,
            Some(45_000.0),
        );
        assert_eq!(opt.buckets.len(), 3);
        // Retail 5.0x -> 20% per PAN
        assert_eq!(opt.buckets[0].estimated_win_prob_pct, Some(20.0));
        // sNII 10.0x -> 10% per application
        assert_eq!(opt.buckets[1].estimated_win_prob_pct, Some(10.0));
        // bNII 10.0x with ~68/14 = 4.86 packets -> ~48.6% packet odds!
        assert!(opt.buckets[2].estimated_win_prob_pct.unwrap() > 45.0);
        // ₹45,000 -> 3 applications of 1 lot = 1 - 0.8^3 = 48.8%
        assert!(
            opt.capital_odds.iter().any(|p| p.contains("48.8%")),
            "{:?}",
            opt.capital_odds
        );
    }

    #[test]
    fn fixture_analysis_scores_six_pillars_and_dual_verdict() {
        let a = fixture_analysis("BCSSL", None, Some(250_000.0)).unwrap();
        assert_eq!(a.symbol, "BCSSL");
        assert_eq!(a.verdict.pillars.len(), 6);
        assert!(
            a.verdict.overall_score >= 70.0,
            "score={}",
            a.verdict.overall_score
        );
        assert!(
            a.verdict.listing_demand.contains("demand"),
            "{}",
            a.verdict.listing_demand
        );
        assert_eq!(
            a.verdict.long_term_profile,
            "Strong institutional demand and issue structure"
        );
        assert_eq!(a.street_buzz.gmp_pct, Some(14.0));
        assert!(
            a.structure
                .registrar_allotment_url
                .as_deref()
                .unwrap_or("")
                .contains("kfintech")
        );

        let sme = fixture_analysis("MAXENGS", None, Some(100_000.0)).unwrap();
        assert_eq!(sme.board, BoardKind::Sme);
        assert!(sme.verdict.risk_flags.iter().any(|f| f.contains("SME")));
        assert!(sme.allotment.capital_odds[0].contains("below this SME IPO's minimum"));
    }

    #[test]
    fn indian_number_formatting_groups_lakhs_and_crores() {
        assert_eq!(fmt_int_indian(999), "999");
        assert_eq!(fmt_int_indian(14_924), "14,924");
        assert_eq!(fmt_int_indian(252_800), "2,52,800");
        assert_eq!(fmt_int_indian(10_000_000), "1,00,00,000");
    }

    #[test]
    fn board_merges_feeds_and_puts_mainboard_first() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let current = serde_json::json!([
            {"symbol": "MAXENGS", "companyName": "Maxwell Engineering Solutions Limited", "series": "SME",
             "isBse": "1", "status": "Active", "issueStartDate": "30-Oct-2026",
             "issueEndDate": "03-Nov-2026", "noOfTime": "4.17"},
            {"symbol": "BCSSL", "companyName": "Bombay Coated and Special Steels Limited", "series": "EQ",
             "status": "Active", "issueStartDate": "29-Oct-2026", "issueEndDate": "02-Nov-2026",
             "issuePrice": "Rs.78 to Rs.82", "noOfTime": "1.122227940075287"}
        ]);
        let upcoming = serde_json::json!([
            {"symbol": "BCSSL", "series": "EQ", "status": "Active", "issueStartDate": "29-Oct-2026",
             "issueEndDate": "02-Nov-2026", "issuePrice": "Rs.78 to Rs.82"},
            {"symbol": "VPFL", "companyName": "Vedant Poly Fabs Limited", "series": "SME",
             "status": "Forthcoming", "issueStartDate": "02-Nov-2026", "issueEndDate": "04-Nov-2026",
             "lotSize": "1200", "priceBand": "Rs.88 to Rs.94", "issueSize": "6030000"}
        ]);
        let (ongoing, soon) = merge_current_and_upcoming(&current, &upcoming, today);
        // The Active row in both feeds is listed once, mainboard before SME.
        let names: Vec<&str> = ongoing.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(names, ["BCSSL", "MAXENGS"]);
        assert_eq!(ongoing[0].total_x, Some(1.12));
        assert!(ongoing[1].is_bse_sme);
        assert_eq!(ongoing[1].board.label(ongoing[1].is_bse_sme), "BSE SME");

        assert_eq!(soon.len(), 1);
        let v = &soon[0];
        assert_eq!(v.status, IpoStatus::Upcoming);
        assert_eq!(v.lot_size, Some(1200));
        // SME: two lots minimum, at the top of the band.
        assert_eq!(v.min_investment_inr, Some(225_600.0));
        assert_eq!(v.issue_size_cr, Some(56.68));
        assert!(v.window_note.contains("in 1d"), "{}", v.window_note);
    }

    #[test]
    fn past_issues_separate_listed_from_awaiting_listing() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let past = serde_json::json!([
            {"symbol": "BLSPOLY", "company": "BLS Polymers Limited", "ipoStartDate": "27-OCT-2026",
             "ipoEndDate": "29-OCT-2026", "issuePrice": "-", "listingDate": "-",
             "priceRange": "Rs.142 to Rs.150", "securityType": "EQ"},
            {"symbol": "JPL", "company": "Jio Platforms Limited", "ipoStartDate": "21-OCT-2026",
             "ipoEndDate": "23-OCT-2026", "issuePrice": "  1119", "listingDate": "28-OCT-2026",
             "priceRange": "Rs.1065 to Rs.1119", "securityType": "EQ"}
        ]);
        let rows = parse_past_issues(&past, today, 10);
        assert_eq!(rows[0].status, IpoStatus::ClosedPendingListing);
        assert!(rows[0].window_note.contains("Allotment/Listing pending"));
        assert_eq!(rows[1].status, IpoStatus::Listed);
        assert_eq!(rows[1].issue_price, Some(1119.0));
        assert_eq!(rows[1].listing_date, NaiveDate::from_ymd_opt(2026, 10, 28));
    }

    fn listed(symbol: &str, ret: f64) -> IpoSummaryRow {
        let mut r = fixture_board(BoardFilter::All, 10).recent[0].clone();
        r.symbol = symbol.into();
        r.return_from_issue_pct = Some(ret);
        r
    }

    #[test]
    fn regime_reads_the_hit_rate_of_recent_listings() {
        let rows = [listed("A", 20.0), listed("B", 10.0), listed("C", -5.0)];
        let quotes = vec![
            Quote {
                symbol: Some("^NSEI".into()),
                regular_market_change_percent: Some(0.5),
                ..Default::default()
            },
            Quote {
                symbol: Some("^INDIAVIX".into()),
                regular_market_price: Some(13.8),
                ..Default::default()
            },
        ];
        let r = compute_regime(&rows, &quotes);
        assert_eq!((r.sample_size, r.positive_count), (3, 2));
        assert_eq!(r.hit_rate_pct, 66.7);
        assert_eq!(r.median_return_pct, 10.0);
        assert_eq!(r.label, "CONSTRUCTIVE");
        assert_eq!(r.best_recent.as_ref().unwrap().0, "A");
        assert_eq!(r.worst_recent.as_ref().unwrap().0, "C");
        assert_eq!(r.india_vix, Some(13.8));

        // Losing listings read as cold; no prices read as neutral, never as hot.
        let cold = compute_regime(&[listed("X", -12.0), listed("Y", -8.0)], &[]);
        assert_eq!(cold.label, "COLD");
        assert_eq!(compute_regime(&[], &[]).label, "NEUTRAL");
    }

    #[test]
    fn gmp_sign_comes_from_a_figure_not_a_hyphen() {
        let rss = |title: &str| {
            format!("<rss><channel><item><title>{title} - Mint</title></item></channel></rss>")
        };
        let up = parse_street_buzz(
            &rss("Acme IPO: GMP at 14% - high-demand issue"),
            Some(100.0),
        );
        assert_eq!(up.gmp_pct, Some(14.0));
        let down = parse_street_buzz(
            &rss("Acme IPO GMP falls to -8%, subscribe with caution"),
            Some(100.0),
        );
        assert_eq!(down.gmp_pct, Some(-8.0));
        let rupees = parse_street_buzz(&rss("Acme IPO GMP is Rs 20 today"), Some(100.0));
        assert_eq!(rupees.gmp_inr, Some(20.0));
        assert_eq!(rupees.gmp_pct, Some(20.0));
        // No GMP headline: no figure, never a zero.
        let none = parse_street_buzz(&rss("Acme IPO opens on Monday"), Some(100.0));
        assert_eq!((none.gmp_pct, none.gmp_inr), (None, None));
    }

    #[test]
    fn resolve_matches_symbol_then_company_name_and_respects_the_board() {
        let b = fixture_board(BoardFilter::All, 10);
        let find = |q: &str, series: Option<&str>| {
            resolve_ipo_target(q, series, &b.ongoing, &b.upcoming, &b.recent).map(|(s, _)| s)
        };
        assert_eq!(find("bcssl", None).as_deref(), Some("BCSSL"));
        assert_eq!(find("bombay coated", None).as_deref(), Some("BCSSL"));
        assert_eq!(find("MAXENGS.NS", None).as_deref(), Some("MAXENGS"));
        assert_eq!(find("BCSSL", Some("SME")), None);
        assert_eq!(find("nothing like this", None), None);
    }

    #[test]
    fn verdict_waits_for_qibs_on_day_one_and_flags_ofs_exits() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let mut row = fixture_board(BoardFilter::All, 10).ongoing[0].clone();
        row.close_date = NaiveDate::from_ymd_opt(2026, 11, 2);
        let early = SubscriptionBreakdown {
            qib_x: Some(0.0),
            nii_x: Some(0.4),
            retail_x: Some(0.9),
            total_x: Some(0.5),
            ..Default::default()
        };
        let ofs = IssueStructure {
            fresh_pct: Some(0.0),
            structure_tag: "100% OFS (Promoter / PE Exit)".into(),
            ..Default::default()
        };
        let regime = fixture_board(BoardFilter::All, 10).regime;
        let v = score_ipo_verdict(
            &row,
            &ofs,
            &early,
            &[],
            &StreetBuzz::default(),
            &regime,
            today,
        );
        assert!(
            v.listing_demand.starts_with("Book still building"),
            "{}",
            v.listing_demand
        );
        assert!(v.risk_flags.iter().any(|f| f.contains("Offer For Sale")));
        assert_eq!(v.long_term_profile, "Weak profile: mostly offer for sale");
        assert_eq!(v.pillars.len(), 6);
        assert_eq!(
            v.pillars.iter().map(|p| p.weight_pct as u32).sum::<u32>(),
            100
        );
    }

    /// Every user-visible string of the IPO analysis and board is a description
    /// of data: no apply/avoid call, no family-PAN plan, no advice wording.
    #[test]
    fn ipo_output_has_no_apply_or_avoid_call() {
        let mut text = Vec::new();
        for (symbol, capital) in [
            ("BCSSL", Some(250_000.0)),
            ("BCSSL", Some(45_000.0)),
            ("BCSSL", None),
            ("MAXENGS", Some(100_000.0)),
            ("MAXENGS", Some(600_000.0)),
        ] {
            let a = fixture_analysis(symbol, None, capital).unwrap();
            text.push(serde_json::to_string(&a).unwrap());
        }
        let board = fixture_board(BoardFilter::All, 10);
        text.push(serde_json::to_string(&board).unwrap());
        let text = text.join("\n");
        for banned in [
            "APPLY",
            "AVOID",
            "SUBSCRIBE &",
            "family",
            "Split Strategy",
            "listing_verdict",
            "long_term_verdict",
            "timing_guidance",
            "capital_plan",
            "golden_rules",
        ] {
            assert!(!text.contains(banned), "{banned:?} found in IPO output");
        }
        crate::verdict::testing::assert_neutral(&text);
    }

    #[test]
    fn board_signals_describe_demand() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let mut row = fixture_board(BoardFilter::All, 10).ongoing[0].clone();
        row.status = IpoStatus::Ongoing;
        row.board = BoardKind::Mainboard;
        row.close_date = NaiveDate::from_ymd_opt(2026, 10, 30);
        row.qib_x = Some(0.2);
        row.total_x = Some(0.4);
        assert_eq!(derive_quick_signal(&row, today), "WEAK DEMAND");
        row.total_x = Some(1.4);
        assert_eq!(derive_quick_signal(&row, today), "MIXED DEMAND");
        row.qib_x = Some(12.0);
        assert_eq!(derive_quick_signal(&row, today), "STRONG · QIB BACKED");
        row.close_date = NaiveDate::from_ymd_opt(2026, 11, 5);
        row.qib_x = Some(0.0);
        assert_eq!(derive_quick_signal(&row, today), "BOOK BUILDING");
    }
}
