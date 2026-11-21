//! Indian IPO intelligence: live ongoing & upcoming board, the IPO market
//! regime (recency-weighted median listing-day return), consolidated NSE + BSE
//! subscription & price-band demand book, SEBI allotment odds from application
//! counts, valuation against listed peers, and the rule-based verdict of
//! `crate::ipo_verdict` (APPLY, APPLY – SELL ON LISTING, WAIT, AVOID or NO
//! CALL) on the calibrated listing-day model of `crate::ipo_model`.
//!
//! Primary data comes from NSE's public IPO endpoints via [`nse::shared_session`]
//! (`/api/ipo-current-issue`, `/api/all-upcoming-issues?category=ipo`,
//! `/api/public-past-issues`, and `/api/ipo-detail?symbol=X&series=EQ|SME`),
//! enriched with live quotes for post-listing performance and Indian financial
//! news RSS for street consensus / unofficial GMP chatter. Fields NSE does not
//! supply (or all of them, when NSE refuses) come from BSE's public-issue
//! pages, merged field by field with provenance (`crate::ipo_fallback`).

use crate::api::{Quote, YahooClient};
use crate::ipo_fallback::{self as fallback, BseFallback};
use crate::ipo_model::ListingModel;
use crate::ipo_verdict::{
    self as verdict, CategoryBook, CategoryOdds, PeerQuote, RhpFigures, Valuation, Verdict,
};
use crate::output::round;
use crate::provenance::{self, Meta};
use crate::providers::nse;
use crate::providers::session::FetchError;
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
    /// Listing-day open and close (recently listed rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_open: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_close: Option<f64>,
    /// Listing-day close over the issue price, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_return_pct: Option<f64>,
    pub quick_signal: String,
    /// The engine's verdict for open and upcoming issues.
    #[serde(default, skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub verdict: Option<Verdict>,
}

/// The IPO market: how recent mainboard listings did on their first day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpoRegime {
    /// HOT, CONSTRUCTIVE, SELECTIVE, COLD, or INSUFFICIENT DATA.
    pub label: String,
    /// Mainboard listings in the last 180 days with a listing-day close.
    pub sample_size: usize,
    pub positive_count: usize,
    pub hit_rate_pct: f64,
    /// Recency-weighted (half-life 45 days) median listing-day return; the
    /// model's regime input. `None` below 8 listings.
    pub weighted_median_listing_pct: Option<f64>,
    pub median_listing_pct: Option<f64>,
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
#[derive(Debug, Clone, Serialize)]
pub struct IpoBoardView {
    pub as_of: NaiveDate,
    pub filter: BoardFilter,
    pub regime: IpoRegime,
    pub ongoing: Vec<IpoSummaryRow>,
    pub upcoming: Vec<IpoSummaryRow>,
    pub recent: Vec<IpoSummaryRow>,
    pub headlines: Vec<IpoHeadline>,
    pub notes: Vec<String>,
    /// Sources, the fields BSE filled (`field_sources`, keyed `SYMBOL field`),
    /// what no source had, and the key-field coverage of the open and
    /// upcoming rows.
    pub meta: Meta,
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

/// Unofficial press signals extracted from news RSS. The counts say how many
/// third-party headlines read positive or negative about the issue; they are
/// reported data, not a call from StockWise.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreetBuzz {
    pub gmp_inr: Option<f64>,
    pub gmp_pct: Option<f64>,
    /// Headlines the GMP figure was read from. The negative-GMP gate needs
    /// two or more.
    #[serde(default)]
    pub gmp_readings: usize,
    #[serde(alias = "subscribe_mentions")]
    pub positive_headlines: usize,
    #[serde(alias = "avoid_mentions")]
    pub negative_headlines: usize,
    #[serde(alias = "neutral_mentions")]
    pub neutral_headlines: usize,
    /// Plain-news headlines only; headlines that tell readers what to do are
    /// counted above but not repeated here.
    pub headlines: Vec<IpoHeadline>,
    pub caveat: String,
}

/// SEBI allotment odds by application category, and what the `--capital`
/// amount buys. Probability maths: lots of the minimum size available ÷
/// applications (see `crate::ipo_verdict::p_allot`).
#[derive(Debug, Clone, Serialize)]
pub struct AllotmentOptimizer {
    pub buckets: Vec<CategoryOdds>,
    pub capital_input_inr: Option<f64>,
    /// What the `--capital` amount buys, with the odds and expected value.
    pub capital_odds: Vec<String>,
    /// How allotment works under SEBI's rules.
    pub allotment_rules: Vec<String>,
}

/// Complete per-IPO analysis returned by `stockwise ipo <SYMBOL>` and `ipo_analyze`.
#[derive(Debug, Clone, Serialize)]
pub struct IpoAnalysis {
    /// The engine's verdict; printed first.
    pub verdict: Verdict,
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
    pub valuation: Valuation,
    /// The RHP figures file read for the valuation, if any.
    pub rhp_file: Option<String>,
    pub street_buzz: StreetBuzz,
    pub regime: IpoRegime,
    /// Data gaps and file problems worth knowing.
    pub notes: Vec<String>,
    /// Sources, the fields BSE filled, what no source had, and coverage of
    /// the issue's key fields (terms and category book).
    pub meta: Meta,
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

    notice_nse_ipo_failure(&session, [&curr_res, &up_res, &past_res]);
    let mut notes = Vec::new();
    let mut meta = Meta::default();
    let nse_board_failed = curr_res.is_err() || up_res.is_err();
    if curr_res.is_ok() || up_res.is_ok() || past_res.is_ok() {
        meta.add_source(provenance::NSE);
    }
    // When NSE refuses or is down, say so once; BSE fills in below.
    if let (Err(e), Err(_), Err(_)) = (&curr_res, &up_res, &past_res) {
        notes.push(format!(
            "NSE's IPO pages are not answering ({e}); open and upcoming issues come from BSE where it answers."
        ));
        meta.warn(format!("NSE IPO pages unavailable ({e})"));
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

    let (mut ongoing, mut upcoming) = merge_current_and_upcoming(&curr_val, &up_val, today);
    let mut recent = parse_past_issues(&past_val, today, REGIME_PAST_ROWS);

    // BSE fills the terms NSE left empty, and the whole board when NSE's
    // feeds failed.
    let mut bse = BseFallback::default();
    if nse_board_failed
        || ongoing
            .iter()
            .chain(upcoming.iter())
            .any(fallback::row_has_gaps)
    {
        let issues = bse.issues(&mut meta).await;
        fallback::merge_board(
            &mut ongoing,
            &mut upcoming,
            &recent,
            &issues,
            today,
            &mut meta,
        );
        sort_board(&mut ongoing, &mut upcoming);
    }
    ongoing.retain(|r| filter.matches(r.board));
    upcoming.retain(|r| filter.matches(r.board));

    let regime = recent_regime(yahoo, &mut recent, today).await;
    if regime.sample_size > 0 {
        meta.add_source(provenance::YAHOO);
    }

    let model = crate::ipo_model::active();
    let hour = ist_hour();
    // Enrich ongoing issues (up to 6) with the category book from
    // `/api/ipo-detail` (BSE's pages for what NSE lacks) and decide each;
    // upcoming issues are decided on their terms and any RHP figures.
    for row in ongoing.iter_mut().take(6) {
        let detail = if fallback::bse_only(row, &meta) {
            None
        } else {
            session
                .get_json(
                    "/api/ipo-detail",
                    &[
                        ("symbol", row.symbol.as_str()),
                        ("series", row.board.series_code()),
                    ],
                    DETAIL_TTL,
                )
                .await
                .ok()
        };
        if let Some(d) = &detail {
            apply_detail_to_row(row, d, today);
        }
        let (mut sub, mut structure) = match &detail {
            Some(d) => (
                parse_subscription(d),
                parse_issue_info(d, &row.price_band).2,
            ),
            None => (board_subscription(row), IssueStructure::default()),
        };
        let no_book = sub.qib_x.is_none() && sub.nii_x.is_none() && sub.retail_x.is_none();
        if no_book || structure.fresh_pct.is_none() || fallback::row_has_gaps(row) {
            let issues = bse.issues(&mut meta).await;
            if let Some(issue) = fallback::find_issue(&issues, &row.company_name) {
                let (det, book) = bse.pages(&issue.ipo_no, no_book, &mut meta).await;
                let prefix = row.symbol.clone();
                fallback::fill_from_detail(
                    row,
                    &mut structure,
                    &mut sub,
                    det.as_ref(),
                    book.as_ref(),
                    &prefix,
                    &mut meta,
                );
                refresh_row(row, today);
            }
        }
        let ctx = RowContext::load(yahoo, &row.symbol, None).await;
        let (v, _) = decide_row(
            row,
            &structure,
            &sub,
            &StreetBuzz::default(),
            &regime,
            &ctx,
            model.as_ref(),
            today,
            hour,
        );
        row.verdict = Some(v);
    }
    for row in upcoming.iter_mut() {
        let mut structure = IssueStructure::default();
        if fallback::row_has_gaps(row) {
            let issues = bse.issues(&mut meta).await;
            if let Some(issue) = fallback::find_issue(&issues, &row.company_name) {
                let (det, _) = bse.pages(&issue.ipo_no, false, &mut meta).await;
                let prefix = row.symbol.clone();
                fallback::fill_from_detail(
                    row,
                    &mut structure,
                    &mut SubscriptionBreakdown::default(),
                    det.as_ref(),
                    None,
                    &prefix,
                    &mut meta,
                );
                refresh_row(row, today);
            }
        }
        let ctx = RowContext::load(yahoo, &row.symbol, None).await;
        let (v, _) = decide_row(
            row,
            &structure,
            &board_subscription(row),
            &StreetBuzz::default(),
            &regime,
            &ctx,
            model.as_ref(),
            today,
            hour,
        );
        row.verdict = Some(v);
    }

    let recent: Vec<IpoSummaryRow> = recent
        .into_iter()
        .filter(|r| filter.matches(r.board))
        .take(past_limit)
        .collect();

    let headlines = parse_rss_headlines(&rss_xml, 6);
    if !headlines.is_empty() {
        meta.add_source(GOOGLE_NEWS);
    }
    board_coverage(&mut meta, &ongoing, &upcoming);
    if model.is_none() {
        notes.push(
            "No calibrated listing-day model is installed, so mainboard issues get NO CALL; run `stockwise ipo calibrate`."
                .to_string(),
        );
    }

    Ok(IpoBoardView {
        as_of: today,
        filter,
        regime,
        ongoing,
        upcoming,
        recent,
        headlines,
        notes,
        meta,
    })
}

/// Source name of the press headlines.
const GOOGLE_NEWS: &str = "Google News";

/// `meta.missing` and `meta.coverage` over the key fields of the open and
/// upcoming rows (`SYMBOL field`).
pub fn board_coverage(meta: &mut Meta, ongoing: &[IpoSummaryRow], upcoming: &[IpoSummaryRow]) {
    let mut names: Vec<(String, bool)> = Vec::new();
    for row in ongoing.iter().chain(upcoming) {
        for (field, ok) in fallback::field_checks(row, None) {
            names.push((format!("{} {field}", row.symbol), ok));
        }
    }
    let checks: Vec<(&str, bool)> = names.iter().map(|(n, ok)| (n.as_str(), *ok)).collect();
    meta.set_fields(&checks);
    let answered = meta
        .sources
        .iter()
        .any(|s| s == provenance::NSE || s == provenance::BSE);
    if checks.is_empty() && !answered {
        meta.coverage = 0.0;
    }
}

/// What the IPO commands fall back to when NSE's IPO pages fail.
pub const IPO_FALLBACK: &str = "using BSE's public-issue pages instead";

/// Announce the first failure of NSE's IPO pages on stderr, naming BSE as
/// the stand-in. The session announces a refusal only once, so this also
/// keeps the quote path's generic "using Yahoo Finance instead" from being
/// printed for the same refusal. Returns whether a notice was printed.
fn notice_nse_ipo_failure(
    session: &crate::providers::session::Session,
    results: [&Result<Value, FetchError>; 3],
) -> bool {
    results
        .into_iter()
        .find_map(|r| r.as_ref().err())
        .is_some_and(|e| session.notice(e, IPO_FALLBACK))
}

/// Yahoo Finance symbol for an Indian IPO symbol: numeric BSE scrip codes get
/// `.BO`, NSE symbols get `.NS`, and an already-suffixed symbol is kept.
pub fn yahoo_symbol(sym: &str) -> String {
    let upper = sym.trim().to_ascii_uppercase();
    if upper.ends_with(".NS") || upper.ends_with(".BO") {
        upper
    } else if !upper.is_empty() && upper.chars().all(|c| c.is_ascii_digit()) {
        format!("{upper}.BO")
    } else {
        format!("{upper}.NS")
    }
}

/// Past issues read for the regime (the board shows fewer).
const REGIME_PAST_ROWS: usize = 120;

/// Live LTPs, listing-day bars and the regime for `recent`.
async fn recent_regime(
    yahoo: &YahooClient,
    recent: &mut [IpoSummaryRow],
    today: NaiveDate,
) -> IpoRegime {
    let mut quote_syms: Vec<String> = vec!["^NSEI".to_string(), "^INDIAVIX".to_string()];
    for row in recent.iter().take(30) {
        if row.status == IpoStatus::Listed && !row.symbol.is_empty() {
            quote_syms.push(yahoo_symbol(&row.symbol));
        }
    }
    let quote_refs: Vec<&str> = quote_syms.iter().map(String::as_str).collect();
    let (quotes, ()) = tokio::join!(
        async { yahoo.get_quote(&quote_refs).await.unwrap_or_default() },
        fetch_listing_days(yahoo, recent, today),
    );
    apply_quotes_to_recent(recent, &quotes);
    compute_regime(recent, &quotes, today)
}

/// A row's subscription when there is no `ipo-detail` answer.
fn board_subscription(row: &IpoSummaryRow) -> SubscriptionBreakdown {
    SubscriptionBreakdown {
        source_note: "Summary board snapshot".to_string(),
        total_x: row.total_x,
        qib_x: row.qib_x,
        nii_x: row.nii_x,
        retail_x: row.retail_x,
        ..Default::default()
    }
}

/// The RHP figures and live peer quotes for one issue.
#[derive(Debug, Clone, Default)]
pub struct RowContext {
    pub rhp: Option<RhpFigures>,
    pub rhp_file: Option<String>,
    pub peers: Vec<PeerQuote>,
    pub notes: Vec<String>,
}

impl RowContext {
    /// Read `ipo/<SYMBOL>.toml` (or `explicit`) and quote the peers it names.
    pub async fn load(
        yahoo: &YahooClient,
        symbol: &str,
        explicit: Option<&std::path::Path>,
    ) -> Self {
        let mut ctx = RowContext::default();
        let path = match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => rhp_path(symbol),
        };
        let Some(path) = path else {
            return ctx;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<RhpFigures>(&text) {
                Ok(r) => {
                    ctx.rhp_file = Some(path.display().to_string());
                    ctx.rhp = Some(r);
                }
                Err(e) => ctx.notes.push(format!(
                    "Could not read the RHP figures in {}: {e}",
                    path.display()
                )),
            },
            Err(e) if explicit.is_some() => ctx
                .notes
                .push(format!("Could not open {}: {e}", path.display())),
            Err(_) => {}
        }
        if let Some(r) = &ctx.rhp
            && !r.peers.is_empty()
        {
            let syms: Vec<String> = r
                .peers
                .iter()
                .map(|p| {
                    let p = p.trim().to_ascii_uppercase();
                    if p.contains('.') {
                        p
                    } else {
                        format!("{p}.NS")
                    }
                })
                .collect();
            let refs: Vec<&str> = syms.iter().map(String::as_str).collect();
            match yahoo.get_quote(&refs).await {
                Ok(quotes) => ctx.peers = peer_quotes(&quotes),
                Err(e) => ctx.notes.push(format!("Peer quotes unavailable: {e}")),
            }
        }
        ctx
    }
}

/// Valuation of each peer from its live quote.
pub fn peer_quotes(quotes: &[Quote]) -> Vec<PeerQuote> {
    quotes
        .iter()
        .filter_map(|q| {
            Some(PeerQuote {
                symbol: q.symbol.clone()?,
                pe: q.trailing_pe.filter(|v| *v > 0.0),
                pb: q.price_to_book.filter(|v| *v > 0.0),
                ev_sales: q.enterprise_to_revenue.filter(|v| *v > 0.0),
            })
        })
        .collect()
}

/// Where the RHP figures for `symbol` live: `ipo/<SYMBOL>.toml` in the data directory.
pub fn rhp_path(symbol: &str) -> Option<std::path::PathBuf> {
    crate::store::data_dir().ok().map(|d| {
        d.join("ipo")
            .join(format!("{}.toml", symbol.trim().to_ascii_uppercase()))
    })
}

/// Fold one `ipo-detail` answer into a board row (category multiples, lot,
/// band, size, quick signal).
fn apply_detail_to_row(row: &mut IpoSummaryRow, detail: &Value, today: NaiveDate) {
    let sub = parse_subscription(detail);
    if sub.total_x.is_some() {
        row.total_x = sub.total_x;
    }
    row.qib_x = sub.qib_x;
    row.nii_x = sub.nii_x;
    row.retail_x = sub.retail_x;
    let (lot_opt, band_opt, structure) = parse_issue_info(detail, &row.price_band);
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

/// Fetch the analysis of one IPO for a symbol or company-name query.
/// `rhp` overrides where the RHP figures are read from.
pub async fn fetch_analysis(
    yahoo: &YahooClient,
    query: &str,
    forced_series: Option<&str>,
    capital: Option<f64>,
    rhp: Option<&std::path::Path>,
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
    notice_nse_ipo_failure(&session, [&curr_res, &up_res, &past_res]);
    let nse_note = match (&curr_res, &up_res, &past_res) {
        (Err(e), Err(_), Err(_)) => Some(format!("NSE's IPO pages are not answering ({e}).")),
        _ => None,
    };

    let curr_val = curr_res.unwrap_or(Value::Array(vec![]));
    let up_val = up_res.unwrap_or(Value::Array(vec![]));
    let past_val = past_res.unwrap_or(Value::Array(vec![]));

    let (ongoing, upcoming) = merge_current_and_upcoming(&curr_val, &up_val, today);
    let all_past = parse_past_issues(&past_val, today, 400);

    let mut meta = Meta::default();
    match &nse_note {
        None => meta.add_source(provenance::NSE),
        Some(n) => meta.warn(n.clone()),
    }
    let mut bse = BseFallback::default();
    let mut bse_issue: Option<fallback::BseIssue> = None;
    let mut matched = resolve_ipo_target(query, forced_series, &ongoing, &upcoming, &all_past);
    if matched.is_none() {
        // Not on NSE's board (or NSE is down): look on BSE's list.
        let issues = bse.issues(&mut meta).await;
        let (bse_on, bse_up): (Vec<_>, Vec<_>) = issues
            .iter()
            .map(|i| fallback::row_from_issue(i, today))
            .partition(|r| r.status == IpoStatus::Ongoing);
        matched = resolve_ipo_target(query, forced_series, &bse_on, &bse_up, &[]);
        if let Some((_, r)) = &matched {
            bse_issue = fallback::find_issue(&issues, &r.company_name).cloned();
            meta.record(fallback::F_BOARD_ROW, provenance::BSE);
        }
    }
    let from_bse = bse_issue.is_some();
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
                    listing_open: None,
                    listing_close: None,
                    listing_return_pct: None,
                    verdict: None,
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
    for ser in series_try.iter().filter(|_| !from_bse) {
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

    // BSE fills what NSE's answer lacks: terms, dates and the category book.
    let nse_detail = detail_json.is_some();
    let has_book = detail_json.as_ref().is_some_and(|d| {
        let s = parse_subscription(d);
        s.qib_x.is_some() || s.nii_x.is_some() || s.retail_x.is_some()
    });
    let terms = detail_json
        .as_ref()
        .map(|d| parse_issue_info(d, &base_row.price_band));
    let has_split = terms.as_ref().is_some_and(|t| t.2.fresh_pct.is_some());
    let mut with_terms = base_row.clone();
    if let Some((lot, band, st)) = &terms {
        with_terms.lot_size = with_terms.lot_size.or(*lot);
        if with_terms.price_band.cap_price().is_none()
            && let Some(b) = band
        {
            with_terms.price_band = b.clone();
        }
        with_terms.issue_size_cr = with_terms.issue_size_cr.or(st.total_issue_cr);
    }
    let book_expected = base_row.status != IpoStatus::Upcoming;
    if (book_expected && !has_book) || !has_split || fallback::row_has_gaps(&with_terms) {
        if bse_issue.is_none() {
            let issues = bse.issues(&mut meta).await;
            bse_issue = fallback::find_issue(&issues, &base_row.company_name).cloned();
        }
        if let Some(issue) = &bse_issue {
            fallback::fill_from_issue(&mut base_row, issue, "", &mut meta);
            let wants_book = book_expected && !has_book;
            let (det, book) = bse.pages(&issue.ipo_no, wants_book, &mut meta).await;
            fallback::merge_detail_json(
                &mut detail_json,
                &mut base_row,
                det.as_ref(),
                book.as_ref(),
                &mut meta,
            );
        }
    }

    if detail_json.is_none()
        && base_row.open_date.is_none()
        && base_row.price_band.cap_price().is_none()
    {
        bail!(
            "No Indian IPO found matching '{}'.{} Run `stockwise ipo` to see active, upcoming, and recent NSE/BSE IPOs.",
            query.trim(),
            nse_note.map(|n| format!(" {n}")).unwrap_or_default()
        );
    }

    let mut recent_for_regime = parse_past_issues(&past_val, today, REGIME_PAST_ROWS);
    let search_name = clean_company_for_search(&base_row.company_name, &symbol);
    let buzz_query = format!("\"{search_name}\" IPO GMP OR review OR subscription");
    let ns_sym = yahoo_symbol(&symbol);
    let (regime, rss_xml, own_quote, ctx) = tokio::join!(
        recent_regime(yahoo, &mut recent_for_regime, today),
        fetch_india_news_rss(yahoo, &buzz_query),
        async {
            yahoo
                .get_quote(&[ns_sym.as_str()])
                .await
                .unwrap_or_default()
        },
        RowContext::load(yahoo, &symbol, rhp),
    );

    if let Some(q) = own_quote
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

    if regime.sample_size > 0 || base_row.ltp.is_some() {
        meta.add_source(provenance::YAHOO);
    }
    let model = crate::ipo_model::active();
    let mut analysis = build_analysis_from_parts(
        base_row,
        detail_json.as_ref(),
        &rss_xml,
        regime,
        capital,
        &ctx,
        model.as_ref(),
        today,
        ist_hour(),
    );
    if !nse_detail && detail_json.is_some() {
        analysis.notes.push(
            "NSE's issue detail is unavailable; the terms and category book shown come from BSE (no application counts, so allotment odds are a lower bound)."
                .to_string(),
        );
    }
    if !analysis.street_buzz.headlines.is_empty() || analysis.street_buzz.gmp_readings > 0 {
        meta.add_source(GOOGLE_NEWS);
    }
    meta.set_fields(&fallback::analysis_checks(&analysis));
    analysis.meta = meta;
    Ok(analysis)
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

#[allow(clippy::too_many_arguments)]
pub fn build_analysis_from_parts(
    mut row: IpoSummaryRow,
    detail: Option<&Value>,
    rss_xml: &str,
    regime: IpoRegime,
    capital: Option<f64>,
    ctx: &RowContext,
    model: Option<&ListingModel>,
    today: NaiveDate,
    hour_ist: u32,
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
    let (verdict, valuation) = decide_row(
        &row,
        &structure,
        &subscription,
        &street_buzz,
        &regime,
        ctx,
        model,
        today,
        hour_ist,
    );
    let mut notes = ctx.notes.clone();
    if detail.is_none() {
        notes.push(
            "No NSE issue detail (category book, application counts, fresh/OFS split): NSE did not answer or has not published it."
                .to_string(),
        );
    }
    if ctx.rhp.is_none() {
        notes.push(format!(
            "No RHP figures for {}: valuation, profit and promoter holding are missing. `stockwise ipo {} --rhp-template` prints the file to fill in.",
            row.symbol, row.symbol
        ));
    }
    let allotment = build_allotment_optimizer(
        row.board,
        row.lot_size,
        cap_price,
        &subscription,
        capital,
        verdict.outlook.as_ref().map(|o| o.expected_gain_pct),
        verdict::blocked_days(today, row.close_date),
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
        valuation,
        rhp_file: ctx.rhp_file.clone(),
        street_buzz,
        regime,
        notes,
        verdict,
        meta: Meta::default(),
        disclaimer: "Figures from public NSE/BSE filings, the RHP figures you saved, live peer quotes and press reports. The verdict is a rule-based educational signal, not SEBI-registered investment advice; the Red Herring Prospectus (RHP) is the source document.".to_string(),
    }
}

/// The engine's verdict for one row, with the valuation it read.
#[allow(clippy::too_many_arguments)]
pub fn decide_row(
    row: &IpoSummaryRow,
    structure: &IssueStructure,
    sub: &SubscriptionBreakdown,
    buzz: &StreetBuzz,
    regime: &IpoRegime,
    ctx: &RowContext,
    model: Option<&ListingModel>,
    today: NaiveDate,
    hour_ist: u32,
) -> (Verdict, Valuation) {
    let cap = row.issue_price.or_else(|| row.price_band.cap_price());
    let mut i = verdict::Inputs::new(&row.symbol, row.board, row.status, today);
    i.book_final = verdict::book_is_final(row.status, row.close_date, today, hour_ist);
    i.open_date = row.open_date;
    i.close_date = row.close_date;
    i.listing_date = row.listing_date;
    i.cap_price = cap;
    i.lot_size = row.lot_size;
    i.qib_x = sub.qib_x.or(row.qib_x);
    i.nii_x = sub.nii_x.or(row.nii_x);
    i.retail_x = sub.retail_x.or(row.retail_x);
    i.total_x = sub.total_x.or(row.total_x);
    i.qib_portion = if sub.categories.is_empty() {
        None
    } else {
        Some(
            sub.categories
                .iter()
                .any(|c| c.sr_no == "1" && c.offered_shares.unwrap_or(0) > 0),
        )
    };
    if let (Some(lot), Some(c)) = (row.lot_size.filter(|l| *l > 0), cap.filter(|c| *c > 0.0)) {
        let (retail, snii, bnii) = category_books(sub);
        i.odds = verdict::category_odds(row.board, lot, c, &retail, &snii, &bnii);
    }
    i.issue_size_cr = structure.total_issue_cr.or(row.issue_size_cr);
    i.structure_known = structure.fresh_pct.is_some();
    i.valuation = verdict::valuation(
        ctx.rhp.as_ref(),
        cap,
        structure.fresh_issue_cr,
        structure.ofs_cr,
        &ctx.peers,
    );
    i.gmp_pct = buzz.gmp_pct.filter(|_| buzz.gmp_readings >= 2);
    i.regime_pct = regime.weighted_median_listing_pct;
    (verdict::decide(&i, model), i.valuation)
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
    sort_board(&mut ongoing, &mut upcoming);
    (ongoing, upcoming)
}

/// Mainboard first; open issues closing soonest, upcoming opening soonest.
pub fn sort_board(ongoing: &mut [IpoSummaryRow], upcoming: &mut [IpoSummaryRow]) {
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
        listing_open: None,
        listing_close: None,
        listing_return_pct: None,
        verdict: None,
        quick_signal: String::new(),
    };
    row.quick_signal = derive_quick_signal(&row, today);
    Some(row)
}

/// Whether an NSE `securityType` / `series` is an equity issue (mainboard or SME).
fn is_equity_issue(series: &str) -> bool {
    matches!(
        series.trim().to_ascii_uppercase().as_str(),
        "EQUITY" | "EQ" | "SME" | "SM" | "BE"
    )
}

pub fn parse_past_issues(past_val: &Value, today: NaiveDate, limit: usize) -> Vec<IpoSummaryRow> {
    let Some(arr) = past_val.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        if out.len() >= limit {
            break;
        }
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
        // NSE's past-issues list also carries debt, REIT and InvIT issues
        // (`securityType` DEBT, REIT, InvIT); equity is `Equity` or `SME`.
        if !is_equity_issue(&series) {
            continue;
        }
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
            listing_open: None,
            listing_close: None,
            listing_return_pct: None,
            verdict: None,
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
        let key = yahoo_symbol(&row.symbol);
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

/// The listing-day bar: the first session on or after `on_or_after`, as
/// (date, open, close). Dates are in the exchange's time zone.
pub fn listing_bar(
    chart: &crate::api::ChartData,
    on_or_after: NaiveDate,
) -> Option<(NaiveDate, f64, f64)> {
    session_bars(chart)
        .into_iter()
        .find(|(d, _, _)| *d >= on_or_after)
}

/// Every session of a daily chart with both an open and a close.
pub fn session_bars(chart: &crate::api::ChartData) -> Vec<(NaiveDate, f64, f64)> {
    let offset = chart
        .meta
        .as_ref()
        .and_then(|m| m.gmtoffset)
        .unwrap_or(19_800);
    let Some(q) = chart.indicators.quote.first() else {
        return Vec::new();
    };
    let opens = q.open.as_deref().unwrap_or_default();
    let closes = q.close.as_deref().unwrap_or_default();
    chart
        .timestamp
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(i, ts)| {
            let d = chrono::DateTime::from_timestamp(ts + offset, 0)?.date_naive();
            let o = (*opens.get(i)?)?;
            let c = (*closes.get(i)?)?;
            (o > 0.0 && c > 0.0).then_some((d, o, c))
        })
        .collect()
}

/// Unix seconds at 00:00 UTC of `d`.
pub fn day_start(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .map_or(0, |t| t.and_utc().timestamp())
}

/// Fill listing-day open, close and return for listed mainboard rows from
/// Yahoo daily bars (rows listed in the regime window only).
pub async fn fetch_listing_days(yahoo: &YahooClient, rows: &mut [IpoSummaryRow], today: NaiveDate) {
    use futures_util::StreamExt;
    let jobs: Vec<(usize, String, NaiveDate)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.board == BoardKind::Mainboard && !r.symbol.is_empty())
        .filter_map(|(i, r)| {
            let ld = r.listing_date?;
            ((today - ld).num_days() <= crate::ipo_model::REGIME_WINDOW_DAYS && ld <= today)
                .then(|| (i, yahoo_symbol(&r.symbol), ld))
        })
        .collect();
    let bars: Vec<(usize, Option<(NaiveDate, f64, f64)>)> = futures_util::stream::iter(jobs)
        .map(|(i, sym, ld)| async move {
            let start = day_start(ld - chrono::Duration::days(1));
            let end = day_start(ld + chrono::Duration::days(8));
            let bar = yahoo
                .get_chart_between(&sym, start, end, "1d")
                .await
                .ok()
                .and_then(|c| listing_bar(&c, ld));
            (i, bar)
        })
        .buffer_unordered(6)
        .collect()
        .await;
    for (i, bar) in bars {
        let row = &mut rows[i];
        if let Some((_, open, close)) = bar
            && let Some(issue) = row.issue_price.or_else(|| row.price_band.cap_price())
            && issue > 0.0
        {
            row.listing_open = Some(round(open, 2));
            row.listing_close = Some(round(close, 2));
            row.listing_return_pct = Some(round((close / issue - 1.0) * 100.0, 2));
        }
    }
}

/// The IPO market regime from listing-day returns of mainboard issues
/// listed in the last 180 days: recency-weighted median (half-life 45
/// days), at least 8 listings.
pub fn compute_regime(recent: &[IpoSummaryRow], quotes: &[Quote], today: NaiveDate) -> IpoRegime {
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

    let priced: Vec<(&str, NaiveDate, f64)> = recent
        .iter()
        .filter(|r| r.board == BoardKind::Mainboard)
        .filter_map(|r| Some((r.symbol.as_str(), r.listing_date?, r.listing_return_pct?)))
        .filter(|(_, d, _)| {
            *d <= today && (today - *d).num_days() <= crate::ipo_model::REGIME_WINDOW_DAYS
        })
        .collect();
    let history: Vec<(NaiveDate, f64)> = priced.iter().map(|(_, d, r)| (*d, *r)).collect();
    let weighted = crate::ipo_model::regime_through(&history, today).map(|v| round(v, 2));
    let sample_size = priced.len();
    let positive_count = priced.iter().filter(|(_, _, p)| *p > 0.0).count();
    let hit_rate_pct = if sample_size > 0 {
        round(positive_count as f64 / sample_size as f64 * 100.0, 1)
    } else {
        0.0
    };
    let mut rets: Vec<f64> = priced.iter().map(|(_, _, p)| *p).collect();
    let median_listing_pct = median_opt(&mut rets);
    let best_recent = priced
        .iter()
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(s, _, p)| ((*s).to_string(), *p));
    let worst_recent = priced
        .iter()
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(s, _, p)| ((*s).to_string(), *p));
    let label = match weighted {
        None => "INSUFFICIENT DATA",
        Some(w) if w >= 20.0 => "HOT",
        Some(w) if w >= 8.0 => "CONSTRUCTIVE",
        Some(w) if w >= 0.0 => "SELECTIVE",
        Some(_) => "COLD",
    };
    let summary = match weighted {
        Some(w) => format!(
            "Recent mainboard listings closed their first day {w:+.1}% over the issue price \
             (recency-weighted median of {sample_size}); {positive_count} of {sample_size} closed above it."
        ),
        None => format!(
            "Only {sample_size} mainboard listing(s) with a listing-day close in the last 180 \
             days; {} are needed to read the IPO market.",
            crate::ipo_model::REGIME_MIN_SAMPLE
        ),
    };
    IpoRegime {
        label: label.to_string(),
        sample_size,
        positive_count,
        hit_rate_pct,
        weighted_median_listing_pct: weighted,
        median_listing_pct,
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

    let use_active = active_total_bids > 0 || (bid_details.is_empty() && !active_cat.is_empty());
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

/// The retail, sNII and bNII books (offered shares, applications, times
/// subscribed) from the parsed categories. For SME issues the first book is
/// the individual investors' category.
pub fn category_books(sub: &SubscriptionBreakdown) -> (CategoryBook, CategoryBook, CategoryBook) {
    let find = |sr: &str, words: &[&str]| -> CategoryBook {
        let c = sub.categories.iter().find(|c| c.sr_no == sr).or_else(|| {
            sub.categories.iter().find(|c| {
                let lc = c.category.to_ascii_lowercase();
                words.iter().all(|w| lc.contains(w))
            })
        });
        CategoryBook {
            offered_shares: c.and_then(|c| c.offered_shares),
            applications: c.and_then(|c| c.applications),
            times: c.and_then(|c| c.times_subscribed),
        }
    };
    let mut retail = find("3", &["retail"]);
    retail.times = retail.times.or(sub.retail_x);
    let mut snii = find("2.2", &["two lakh", "ten lakh"]);
    snii.times = snii.times.or(sub.snii_x).or(sub.nii_x);
    let mut bnii = find("2.1", &["more than ten lakh"]);
    bnii.times = bnii.times.or(sub.bnii_x).or(sub.nii_x);
    (retail, snii, bnii)
}

/// Allotment odds per category and what `capital_inr` buys. `gain_pct` is
/// the model's expected listing gain (for expected values), `days` how long
/// the money stays blocked.
pub fn build_allotment_optimizer(
    board: BoardKind,
    lot_size: Option<u64>,
    cap_price: Option<f64>,
    sub: &SubscriptionBreakdown,
    capital_inr: Option<f64>,
    gain_pct: Option<f64>,
    days: i64,
) -> AllotmentOptimizer {
    let mut capital_odds = Vec::new();
    let mut allotment_rules = vec![
        "Odds = lots of the minimum application size available in the category ÷ applications \
         in it (NSE application counts). Without application counts, 1 ÷ the share \
         subscription is shown: a lower bound, since many applicants bid more than the minimum."
            .to_string(),
    ];
    let (Some(lot), Some(price)) = (lot_size.filter(|&l| l > 0), cap_price.filter(|&p| p > 0.0))
    else {
        return AllotmentOptimizer {
            buckets: Vec::new(),
            capital_input_inr: capital_inr,
            capital_odds: vec![
                "Lot size and price band are not announced yet; allotment odds appear once they are published.".to_string(),
            ],
            allotment_rules,
        };
    };
    let (retail, snii, bnii) = category_books(sub);
    let buckets = verdict::category_odds(board, lot, price, &retail, &snii, &bnii);
    let odds_text = |o: &CategoryOdds| {
        o.p_allot.map_or("odds unknown".to_string(), |p| {
            format!(
                "{:.2}% odds{}",
                p * 100.0,
                if o.basis == "lower bound" {
                    " (at least)"
                } else {
                    ""
                }
            )
        })
    };
    let ev_text =
        |o: &CategoryOdds| match gain_pct.and_then(|g| verdict::expected_value(o, g, days)) {
            Some(e) => format!(", expected value ₹{:.0}", e.ev_inr),
            None => String::new(),
        };
    match capital_inr {
        Some(cap) => {
            let affordable: Vec<&CategoryOdds> =
                buckets.iter().filter(|o| o.min_amount_inr <= cap).collect();
            if affordable.is_empty() {
                let min = &buckets[0];
                capital_odds.push(format!(
                    "₹{} is below the minimum {} application of ₹{} ({} lots × {} shares).",
                    fmt_inr(cap),
                    min.category,
                    fmt_inr(min.min_amount_inr),
                    min.min_lots,
                    lot
                ));
            }
            for o in affordable {
                capital_odds.push(format!(
                    "One {} application of {} lots (₹{}): {}{}; a win allots {} lots (₹{}).",
                    o.category,
                    o.min_lots,
                    fmt_inr(o.min_amount_inr),
                    odds_text(o),
                    ev_text(o),
                    o.allotted_lots,
                    fmt_inr(o.allotted_amount_inr)
                ));
            }
            let first = &buckets[0];
            if let Some(p) = first.p_allot.filter(|p| *p < 1.0)
                && first.min_amount_inr > 0.0
            {
                let n = ((cap / first.min_amount_inr).floor() as i32).min(6);
                if n >= 2 {
                    let any = (1.0 - (1.0 - p).powi(n)) * 100.0;
                    capital_odds.push(format!(
                        "{n} separate {} applications (each on its own PAN, ₹{} in all): {any:.1}% chance of at least one allotment.",
                        first.category,
                        fmt_inr(n as f64 * first.min_amount_inr)
                    ));
                }
            }
        }
        None => {
            let r = &buckets[0];
            capital_odds.push(format!(
                "One {} application of {} lots = ₹{}: {}{}. Pass `--capital 2.5L` or `--capital 12L` to see what an amount buys.",
                r.category,
                r.min_lots,
                fmt_inr(r.min_amount_inr),
                odds_text(r),
                ev_text(r)
            ));
        }
    }
    if board == BoardKind::Sme {
        allotment_rules.push(
            "After listing, SME shares trade only in full lots, so partial exits are not possible, and the 5% circuit filter can limit liquidity.".to_string(),
        );
    } else {
        allotment_rules.push(
            "Retail and NII applicants are allotted the minimum application size by lottery when the category is oversubscribed; bidding more than the minimum adds no odds. bNII winners receive an sNII-sized packet.".to_string(),
        );
        allotment_rules.push(
            "A retail bid below the final issue price is not allotted; a cut-off bid takes whatever the final price turns out to be.".to_string(),
        );
    }
    allotment_rules.push(
        "QIBs typically place most of their bids on the afternoon of the closing day; the UPI mandate deadline is 5:00 PM IST.".to_string(),
    );
    AllotmentOptimizer {
        buckets,
        capital_input_inr: capital_inr,
        capital_odds,
        allotment_rules,
    }
}

// ── Street Buzz / GMP RSS Parser ──

pub fn parse_street_buzz(rss_xml: &str, cap_price: Option<f64>) -> StreetBuzz {
    let raw = parse_rss_headlines_raw(rss_xml, 8);
    let mut gmp_pcts = Vec::new();
    let mut gmp_inrs = Vec::new();
    let mut positive_headlines = 0usize;
    let mut negative_headlines = 0usize;
    let mut neutral_headlines = 0usize;

    for h in &raw {
        let lc = h.title.to_ascii_lowercase();
        if (lc.contains("subscribe")
            && !lc.contains("should you subscribe")
            && !lc.contains("subscribed"))
            || lc.contains("apply for")
            || lc.contains("thumbs up")
        {
            positive_headlines += 1;
        }
        if lc.contains("avoid") || lc.contains("skip") || lc.contains("stay away") {
            negative_headlines += 1;
        }
        if lc.contains("neutral") || lc.contains("may apply") || lc.contains("high risk") {
            neutral_headlines += 1;
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

    let gmp_readings = gmp_pcts.len().max(gmp_inrs.len());
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
        gmp_readings,
        positive_headlines,
        negative_headlines,
        neutral_headlines,
        headlines: raw.into_iter().filter(|h| !reads_as_call(&h.title)).collect(),
        caveat: "Headlines and GMP come from third-party news and are not StockWise's view. GMP is unregulated grey-market chatter and can vanish before listing. Headlines that tell readers what to do are counted but not shown.".to_string(),
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

/// True when a headline tells the reader what to do with the issue
/// ("Subscribe", "Avoid", "Apply", "Skip"...). Such a headline is a third
/// party's call; StockWise counts it but does not repeat it. Past-tense news
/// such as "subscribed 4.7x" is not a call.
fn reads_as_call(title: &str) -> bool {
    let lc = title.to_lowercase();
    if lc.contains("stay away") || lc.contains("thumbs up") || lc.contains("thumbs down") {
        return true;
    }
    lc.split(|c: char| !c.is_alphanumeric()).any(|w| {
        matches!(
            w,
            "subscribe"
                | "avoid"
                | "apply"
                | "skip"
                | "buy"
                | "sell"
                | "hold"
                | "accumulate"
                | "recommend"
                | "recommends"
        )
    })
}

/// Headlines from an RSS feed, without the ones that read as a call.
pub fn parse_rss_headlines(xml: &str, max_items: usize) -> Vec<IpoHeadline> {
    parse_rss_headlines_raw(xml, max_items * 2)
        .into_iter()
        .filter(|h| !reads_as_call(&h.title))
        .take(max_items)
        .collect()
}

fn parse_rss_headlines_raw(xml: &str, max_items: usize) -> Vec<IpoHeadline> {
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

/// Recompute a row's derived fields after values were filled in: window
/// note, minimum investment and quick signal.
pub fn refresh_row(row: &mut IpoSummaryRow, today: NaiveDate) {
    row.window_note = compute_window_note(row.status, row.open_date, row.close_date, today);
    if let (Some(lot), Some(cap)) = (row.lot_size, row.price_band.cap_price()) {
        let min_lots = if row.board == BoardKind::Sme { 2 } else { 1 };
        row.min_investment_inr = Some(round((lot * min_lots) as f64 * cap, 0));
    }
    row.quick_signal = derive_quick_signal(row, today);
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

fn ist_hour() -> u32 {
    use chrono::Timelike;
    (Utc::now() + chrono::Duration::seconds(19_800)).hour()
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
            listing_open: None,
            listing_close: None,
            listing_return_pct: None,
            verdict: None,
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
            listing_open: None,
            listing_close: None,
            listing_return_pct: None,
            verdict: None,
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
        listing_open: None,
        listing_close: None,
        listing_return_pct: None,
        verdict: None,
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
            listing_open: None,
            listing_close: None,
            listing_return_pct: None,
            verdict: None,
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
            listing_open: None,
            listing_close: None,
            listing_return_pct: None,
            verdict: None,
            quick_signal: "+7.5% ABOVE ISSUE".into(),
        },
    ];

    let regime = IpoRegime {
        label: "CONSTRUCTIVE".into(),
        sample_size: 2,
        positive_count: 2,
        hit_rate_pct: 100.0,
        weighted_median_listing_pct: Some(12.0),
        median_listing_pct: Some(11.8),
        best_recent: Some(("JPL".into(), 16.0)),
        worst_recent: Some(("PARASHLTH".into(), 7.5)),
        nifty_change_pct: Some(0.5),
        india_vix: Some(13.8),
        summary: "Recent mainboard listings closed their first day +12.0% over the issue price (recency-weighted median of 9); 9 of 9 closed above it.".into(),
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
        meta: Meta::from_source(provenance::NSE),
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
            {"srNo": "2.1", "category": "bNII", "noofapplication": "1210"},
            {"srNo": "2.2", "category": "sNII", "noofapplication": "2650"},
            {"srNo": "3", "category": "Retail Individual Investors(RIIs)", "noofapplication": "120400"},
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

    // RHP figures: 18.32 crore shares before the issue, 2.23 crore fresh:
    // ₹1,685 crore after the issue at ₹82, P/E 18.7 on ₹90 crore profit.
    let ctx = RowContext {
        rhp: Some(RhpFigures {
            pre_issue_shares: Some(1.832e8),
            fresh_shares: Some(2.2324e7),
            revenue_cr: vec![610.0, 742.0, 905.0],
            pat_cr: vec![52.0, 71.0, 90.0],
            net_worth_cr: Some(540.0),
            total_debt_cr: Some(210.0),
            cash_cr: Some(35.0),
            promoter_pre_pct: Some(88.0),
            promoter_post_pct: Some(78.6),
            peers: vec!["JSWSTEEL".into(), "TATASTEEL".into(), "JINDALSTEL".into()],
            ..Default::default()
        }),
        rhp_file: Some("ipo/BCSSL.toml".into()),
        peers: ["JSWSTEEL.NS", "TATASTEEL.NS", "JINDALSTEL.NS"]
            .iter()
            .zip([21.0, 19.5, 17.0])
            .map(|(s, pe)| PeerQuote {
                symbol: s.to_string(),
                pe: Some(pe),
                pb: Some(2.6),
                ev_sales: None,
            })
            .collect(),
        notes: Vec::new(),
    };
    let model = crate::ipo_model::testing::model();
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
        &ctx,
        Some(&model),
        today,
        16,
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
    fn allotment_optimizer_uses_application_counts() {
        let a = fixture_analysis("BCSSL", None, Some(45_000.0)).unwrap();
        let retail = &a.allotment.buckets[0];
        // 82,14,097 retail shares = 45,132 lots of 182 for 1,20,400 applications.
        assert_eq!(retail.category, "Retail");
        assert_eq!(retail.applications, Some(120_400));
        assert_eq!(retail.basis, "applications");
        let p = retail.p_allot.unwrap();
        assert!((p - 45_132.0 / 120_400.0).abs() < 1e-9, "{p}");
        // Not 1 ÷ 4.20x = 23.8%: most retail applicants bid one lot.
        assert!(p > 0.3);
        // ₹45,000: three separate one-lot applications.
        assert!(
            a.allotment
                .capital_odds
                .iter()
                .any(|l| l.starts_with("3 separate Retail applications")),
            "{:?}",
            a.allotment.capital_odds
        );
        // Without application counts the odds are a lower bound, 1 ÷ x.
        let sub = SubscriptionBreakdown {
            retail_x: Some(5.0),
            snii_x: Some(10.0),
            bnii_x: Some(10.0),
            total_x: Some(7.0),
            ..Default::default()
        };
        let opt = build_allotment_optimizer(
            BoardKind::Mainboard,
            Some(182),
            Some(82.0),
            &sub,
            None,
            None,
            4,
        );
        assert_eq!(opt.buckets.len(), 3);
        assert_eq!(opt.buckets[0].p_allot, Some(0.2));
        assert_eq!(opt.buckets[0].basis, "lower bound");
        assert_eq!(opt.buckets[2].allotted_lots, opt.buckets[1].min_lots);
    }

    #[test]
    fn fixture_analysis_leads_with_a_verdict() {
        let a = fixture_analysis("BCSSL", None, Some(250_000.0)).unwrap();
        assert_eq!(a.symbol, "BCSSL");
        // Closed book (closes 2 Nov; the fixture's today is 1 Nov, so the
        // book is still building): WAIT with a QIB trigger.
        let v = &a.verdict;
        assert_eq!(v.kind, verdict::Kind::Wait, "{v:?}");
        assert!(
            v.trigger
                .as_deref()
                .unwrap()
                .contains("by 3 PM IST on 02 Nov 2026")
        );
        assert_eq!(a.valuation.market_cap_cr, Some(1685.3));
        assert_eq!(a.valuation.pe, Some(18.73));
        assert_eq!(a.valuation.peer_median_pe, Some(19.5));
        assert_eq!(a.rhp_file.as_deref(), Some("ipo/BCSSL.toml"));
        assert_eq!(a.street_buzz.gmp_pct, Some(14.0));
        assert!(
            a.structure
                .registrar_allotment_url
                .as_deref()
                .unwrap_or("")
                .contains("kfintech")
        );
        // The JSON leads with the verdict object.
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            json.starts_with("{\"verdict\":{\"kind\":\"WAIT\""),
            "{}",
            &json[..80]
        );

        let sme = fixture_analysis("MAXENGS", None, Some(100_000.0)).unwrap();
        assert_eq!(sme.board, BoardKind::Sme);
        assert!(matches!(
            sme.verdict.kind,
            verdict::Kind::NoCall | verdict::Kind::Avoid
        ));
        assert!(
            sme.allotment.capital_odds[0].contains("below the minimum SME individual application")
        );
    }

    #[test]
    fn closing_afternoon_turns_wait_into_a_call() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let board = fixture_board(BoardFilter::All, 10);
        let row = board.ongoing[0].clone();
        let a = fixture_analysis("BCSSL", None, None).unwrap();
        let ctx = RowContext {
            rhp: None,
            ..Default::default()
        };
        let model = crate::ipo_model::testing::model();
        // Before 3 PM on the closing day: still WAIT.
        let (v, _) = decide_row(
            &row,
            &a.structure,
            &a.subscription,
            &a.street_buzz,
            &board.regime,
            &ctx,
            Some(&model),
            today,
            14,
        );
        assert!(
            matches!(v.kind, verdict::Kind::Wait | verdict::Kind::NoCall),
            "{v:?}"
        );
        // After 3 PM the book is final and the gates and model decide.
        let (v, _) = decide_row(
            &row,
            &a.structure,
            &a.subscription,
            &a.street_buzz,
            &board.regime,
            &ctx,
            Some(&model),
            today,
            15,
        );
        assert_ne!(v.kind, verdict::Kind::Wait, "{v:?}");
    }

    /// The live NSE answer (recorded 2026-11-21): consolidated book in
    /// `activeCat`, `Total` row without a serial number, no application counts.
    #[test]
    fn live_nse_ipo_detail_fixture_parses() {
        let detail: Value =
            serde_json::from_str(include_str!("fixtures/nse_ipo_detail_blspoly.json")).unwrap();
        let sub = parse_subscription(&detail);
        assert_eq!(sub.source_note, "Consolidated NSE + BSE live bid book");
        assert_eq!(sub.qib_x, Some(200.25));
        assert_eq!(sub.nii_x, Some(559.2));
        assert_eq!(sub.bnii_x, Some(592.83));
        assert_eq!(sub.snii_x, Some(491.95));
        assert_eq!(sub.retail_x, Some(128.36));
        assert_eq!(sub.total_x, Some(241.22));
        assert_eq!(sub.mf_share_of_qib_pct, Some(0.6));
        let (retail, snii, bnii) = category_books(&sub);
        assert_eq!(retail.offered_shares, Some(5_950_000));
        assert_eq!(snii.offered_shares, Some(850_000));
        assert_eq!(bnii.offered_shares, Some(1_700_000));
        assert_eq!(retail.applications, None);
        let (lot, band, st) = parse_issue_info(&detail, &PriceBand::default());
        assert_eq!(lot, Some(100));
        assert_eq!(band.unwrap().cap_price(), Some(150.0));
        // 1,70,00,000 fresh shares × ₹150 = ₹255 crore, all fresh.
        assert_eq!(st.total_issue_cr, Some(255.0));
        assert_eq!(st.fresh_pct, Some(100.0));
        assert_eq!(st.anchor_shares, Some(5_100_000));
        assert!(
            st.ratios_url
                .as_deref()
                .unwrap()
                .ends_with("RATIOS_BLSPOLY.zip")
        );
        // Without application counts the odds are the 1 ÷ x lower bound.
        let opt = build_allotment_optimizer(
            BoardKind::Mainboard,
            lot,
            Some(150.0),
            &sub,
            None,
            Some(10.0),
            4,
        );
        assert_eq!(opt.buckets[0].basis, "lower bound");
        assert!((opt.buckets[0].p_allot.unwrap() - 1.0 / 128.36).abs() < 1e-9);
    }

    #[tokio::test]
    async fn an_nse_ipo_refusal_names_bse_not_yahoo() {
        use crate::providers::session::{DiskCache, Session};
        let server =
            crate::testutil::MockServer::start(vec![(403, "<HTML>Access Denied</HTML>".into())])
                .await;
        let session = Session::for_tests(nse::site(&server.url), DiskCache::Off);
        let curr = session
            .get_json("/api/ipo-current-issue", &[], Duration::ZERO)
            .await;
        let up = session
            .get_json("/api/all-upcoming-issues", &[], Duration::ZERO)
            .await;
        let past = session
            .get_json("/api/public-past-issues", &[], Duration::ZERO)
            .await;
        let err = curr.as_ref().unwrap_err().clone();
        assert!(notice_nse_ipo_failure(&session, [&curr, &up, &past]));
        // The quote path's generic notice for the same refusal stays quiet.
        assert!(!session.notice(&err, "using Yahoo Finance instead"));
        // Every feed answered: nothing to announce.
        let fresh = Session::for_tests(nse::site(&server.url), DiskCache::Off);
        let ok: Result<Value, FetchError> = Ok(Value::Array(vec![]));
        assert!(!notice_nse_ipo_failure(&fresh, [&ok, &ok, &ok]));
        assert!(IPO_FALLBACK.contains("BSE"));
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
        // The live list (2026-11-21) labels equity `Equity` and `SME`, and
        // also carries debt, REIT and InvIT issues, which are not IPOs here.
        let live = serde_json::json!([
            {"symbol": "NCDX", "company": "Some Finance NCD", "issuePrice": "1000", "listingDate": "20-OCT-2026", "securityType": "DEBT"},
            {"symbol": "REITX", "company": "Some REIT", "issuePrice": "100", "listingDate": "20-OCT-2026", "securityType": "REIT"},
            {"symbol": "INVX", "company": "Some InvIT", "issuePrice": "100", "listingDate": "20-OCT-2026", "securityType": "InvIT"},
            {"symbol": "BLSPOLY", "company": "BLS Polymers Limited", "issuePrice": "   150", "listingDate": "03-NOV-2026", "securityType": "Equity"},
            {"symbol": "VPFL", "company": "Vedant Poly Fabs Limited", "issuePrice": "94", "listingDate": "09-NOV-2026", "securityType": "SME"}
        ]);
        let live_rows = parse_past_issues(&live, today, 2);
        let got: Vec<(&str, BoardKind)> = live_rows
            .iter()
            .map(|r| (r.symbol.as_str(), r.board))
            .collect();
        assert_eq!(
            got,
            [("BLSPOLY", BoardKind::Mainboard), ("VPFL", BoardKind::Sme)]
        );
        assert_eq!(live_rows[0].issue_price, Some(150.0));
        assert!(rows[0].window_note.contains("Allotment/Listing pending"));
        assert_eq!(rows[1].status, IpoStatus::Listed);
        assert_eq!(rows[1].issue_price, Some(1119.0));
        assert_eq!(rows[1].listing_date, NaiveDate::from_ymd_opt(2026, 10, 28));
    }

    fn listed(symbol: &str, ret: f64, days_ago: i64) -> IpoSummaryRow {
        let mut r = fixture_board(BoardFilter::All, 10).recent[0].clone();
        r.symbol = symbol.into();
        r.listing_date =
            Some(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap() - chrono::Duration::days(days_ago));
        r.listing_return_pct = Some(ret);
        // The current price is not the regime's input.
        r.return_from_issue_pct = Some(-50.0);
        r
    }

    #[test]
    fn regime_reads_listing_day_returns_recency_weighted() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
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
        // Five old strong listings, four recent weak ones.
        let mut rows: Vec<IpoSummaryRow> = (0..5)
            .map(|i| listed(&format!("OLD{i}"), 40.0, 120 + i))
            .collect();
        rows.extend((0..4).map(|i| listed(&format!("NEW{i}"), -4.0, 2 + i)));
        let r = compute_regime(&rows, &quotes, today);
        assert_eq!((r.sample_size, r.positive_count), (9, 5));
        assert_eq!(r.weighted_median_listing_pct, Some(-4.0));
        assert_eq!(r.median_listing_pct, Some(40.0));
        assert_eq!(r.label, "COLD");
        assert!(r.best_recent.as_ref().unwrap().0.starts_with("OLD"));
        assert_eq!(r.india_vix, Some(13.8));

        // Fewer than 8 listings: no reading, never a default.
        let few: Vec<IpoSummaryRow> = (0..7)
            .map(|i| listed(&format!("A{i}"), 30.0, 5 + i))
            .collect();
        let r = compute_regime(&few, &[], today);
        assert_eq!(r.label, "INSUFFICIENT DATA");
        assert_eq!(r.weighted_median_listing_pct, None);
        // Listings older than 180 days and SME listings are left out.
        let mut mixed = few.clone();
        mixed.push(listed("ANCIENT", 30.0, 400));
        let mut sme = listed("SMEX", 30.0, 3);
        sme.board = BoardKind::Sme;
        mixed.push(sme);
        assert_eq!(compute_regime(&mixed, &[], today).sample_size, 7);
        let hot: Vec<IpoSummaryRow> = (0..8)
            .map(|i| listed(&format!("H{i}"), 25.0, 5 + i))
            .collect();
        assert_eq!(compute_regime(&hot, &[], today).label, "HOT");
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
    fn headlines_that_tell_readers_what_to_do_are_counted_not_shown() {
        let rss = r#"<rss><channel>
            <item><title>Acme IPO opens; GMP at 14%, brokerages say Subscribe - Mint</title></item>
            <item><title>Acme IPO: avoid, say analysts - Mint</title></item>
            <item><title>Should you apply for the Acme IPO? - ET</title></item>
            <item><title>Acme IPO subscribed 4.7x on day two - ET</title></item>
        </channel></rss>"#;
        let b = parse_street_buzz(rss, Some(100.0));
        assert_eq!(b.positive_headlines, 2); // "Subscribe" and "apply for"
        assert_eq!(b.negative_headlines, 1);
        let shown: Vec<&str> = b.headlines.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(shown, ["Acme IPO subscribed 4.7x on day two"]);
        crate::action::testing::assert_action_well_formed(&serde_json::to_string(&b).unwrap());
        // The board list drops them too.
        let board = parse_rss_headlines(rss, 6);
        assert_eq!(board.len(), 1);
    }

    #[test]
    fn old_street_buzz_field_names_still_load() {
        let old = r#"{"gmp_inr":null,"gmp_pct":null,"subscribe_mentions":2,
            "avoid_mentions":1,"neutral_mentions":0,"headlines":[],"caveat":""}"#;
        let b: StreetBuzz = serde_json::from_str(old).unwrap();
        assert_eq!((b.positive_headlines, b.negative_headlines), (2, 1));
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
    fn day_one_book_waits_for_qibs() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let board = fixture_board(BoardFilter::All, 10);
        let mut row = board.ongoing[0].clone();
        row.close_date = NaiveDate::from_ymd_opt(2026, 11, 2);
        let a = fixture_analysis("BCSSL", None, None).unwrap();
        let early = SubscriptionBreakdown {
            qib_x: Some(0.0),
            nii_x: Some(0.4),
            retail_x: Some(0.9),
            total_x: Some(0.5),
            categories: a.subscription.categories.clone(),
            ..Default::default()
        };
        let ctx = RowContext {
            rhp: Some(RhpFigures {
                pre_issue_shares: Some(1.832e8),
                fresh_shares: Some(2.2324e7),
                revenue_cr: vec![610.0, 742.0, 905.0],
                pat_cr: vec![52.0, 71.0, 90.0],
                net_worth_cr: Some(540.0),
                promoter_post_pct: Some(78.6),
                ..Default::default()
            }),
            peers: vec![PeerQuote {
                symbol: "P.NS".into(),
                pe: Some(19.0),
                pb: None,
                ev_sales: None,
            }],
            ..Default::default()
        };
        let model = crate::ipo_model::testing::model();
        let (v, val) = decide_row(
            &row,
            &a.structure,
            &early,
            &StreetBuzz::default(),
            &board.regime,
            &ctx,
            Some(&model),
            today,
            11,
        );
        assert_eq!(v.kind, verdict::Kind::Wait, "{v:?}");
        assert!(v.qib_threshold_x.unwrap() >= 1.0);
        assert!(v.reasons[0].starts_with("Book still building: QIB 0.00x so far"));
        assert_eq!(val.ofs_pct, Some(0.0));
    }

    /// Every verdict in IPO output is well formed and comes from the engine;
    /// no family-PAN plan or old score wording survives.
    #[test]
    fn ipo_output_verdicts_are_well_formed() {
        let mut text = Vec::new();
        for (symbol, capital) in [
            ("BCSSL", Some(250_000.0)),
            ("BCSSL", Some(45_000.0)),
            ("BCSSL", None),
            ("MAXENGS", Some(100_000.0)),
            ("MAXENGS", Some(600_000.0)),
        ] {
            let a = fixture_analysis(symbol, None, capital).unwrap();
            crate::ipo_verdict::testing::check_verdict(&serde_json::to_value(&a.verdict).unwrap());
            text.push(serde_json::to_string(&a).unwrap());
            text.push(a.verdict.render("  "));
        }
        let board = fixture_board(BoardFilter::All, 10);
        text.push(serde_json::to_string(&board).unwrap());
        let text = text.join("\n");
        for banned in [
            "SUBSCRIBE &",
            "family",
            "Split Strategy",
            "listing_verdict",
            "long_term_verdict",
            "timing_guidance",
            "capital_plan",
            "golden_rules",
            "pillars",
            "overall_score",
        ] {
            assert!(!text.contains(banned), "{banned:?} found in IPO output");
        }
        crate::action::testing::assert_action_well_formed(&text);
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

    #[test]
    fn yahoo_symbol_uses_bo_for_bse_codes_and_parse_sub_handles_zero_bids() {
        assert_eq!(yahoo_symbol("544612"), "544612.BO");
        assert_eq!(yahoo_symbol("BCSSL"), "BCSSL.NS");
        assert_eq!(yahoo_symbol("544612.bo"), "544612.BO");

        let zero_book = serde_json::json!({
            "activeCat": {
                "dataList": [
                    { "srNo": "1", "category": "QIB", "noOfShareOffered": 1_000_000.0, "noOfSharesBid": 0.0, "noOfTotalMeant": 0.0 },
                    { "srNo": "2", "category": "NII", "noOfShareOffered": 500_000.0, "noOfSharesBid": 0.0, "noOfTotalMeant": 0.0 },
                    { "srNo": "3", "category": "Retail", "noOfShareOffered": 1_500_000.0, "noOfSharesBid": 0.0, "noOfTotalMeant": 0.0 }
                ]
            }
        });
        let sub = parse_subscription(&zero_book);
        assert_eq!(sub.qib_x, Some(0.0));
        assert_eq!(sub.nii_x, Some(0.0));
        assert_eq!(sub.retail_x, Some(0.0));
    }

    #[test]
    fn board_filter_retains_sme_before_ongoing_enrichment_cap() {
        let mut board = fixture_board(BoardFilter::All, 10);
        let template = board.ongoing[0].clone();
        let mut ongoing = Vec::new();
        for n in 0..6 {
            let mut r = template.clone();
            r.symbol = format!("MAIN{n}");
            r.board = BoardKind::Mainboard;
            ongoing.push(r);
        }
        for n in 0..2 {
            let mut r = template.clone();
            r.symbol = format!("SME{n}");
            r.board = BoardKind::Sme;
            ongoing.push(r);
        }
        ongoing.retain(|r| BoardFilter::Sme.matches(r.board));
        let enriched: Vec<&str> = ongoing.iter().take(6).map(|r| r.symbol.as_str()).collect();
        assert_eq!(enriched, ["SME0", "SME1"]);
        board.ongoing = ongoing;
        assert_eq!(board.ongoing.len(), 2);
    }
}
