//! IPO verdicts: APPLY, APPLY – SELL ON LISTING, WAIT, AVOID or NO CALL for
//! one Indian IPO, from the subscription book, the calibrated listing-day
//! model (`crate::ipo_model`), valuation against listed peers and the issue
//! structure. LISTED hands an issue that already trades to the stock action
//! engine (`crate::action`).
//!
//! The engine is deterministic: the same inputs always give the same
//! verdict. Every verdict carries up to three reasons, a confidence level,
//! what would invalidate it (or, for WAIT, what would trigger APPLY), a
//! review date, the data coverage it was decided on, the methodology version
//! and the disclaimer. The rules are in docs/methodology.md ("IPO
//! verdicts"); bump [`METHODOLOGY`] whenever one changes.

use crate::ipo::{BoardKind, IpoStatus};
use crate::ipo_model::{Features, ListingModel};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

/// Version of the IPO verdict rules.
pub const METHODOLOGY: &str = "ipo-1";
/// Below this data coverage (percent) there is no call.
pub const MIN_COVERAGE: f64 = 60.0;

pub const DISCLAIMER: &str = "Rule-based educational signal from StockWise's published IPO \
     methodology, not investment advice. StockWise is not a SEBI-registered investment adviser or \
     research analyst. IPOs can list below the issue price and you can lose money; read the RHP \
     and decide for yourself.";

// Thresholds. Keep these in step with docs/methodology.md.
/// APPLY needs the probability of a listing-day close below the issue price under this.
pub const MAX_P_LOSS: f64 = 0.25;
/// APPLY (and not only for listing) needs P/E at most this many times the peer median.
pub const PE_STRETCH_APPLY: f64 = 1.2;
/// AVOID above this many times the peer median P/E.
pub const PE_STRETCH_AVOID: f64 = 1.5;
/// AVOID when the offer for sale is more than this share of the issue and a PE/VC fund sells.
pub const OFS_AVOID_PCT: f64 = 75.0;
/// More offer for sale than this makes the long-term case weak (sell on listing).
pub const OFS_LONG_TERM_PCT: f64 = 50.0;
/// A loss-making company needs revenue growing at least this fast (CAGR %).
pub const LOSS_MAKER_MIN_REVENUE_CAGR: f64 = 25.0;
/// SME issues smaller than this (₹ crore) are a tiny float.
pub const SME_MIN_ISSUE_CR: f64 = 10.0;
/// SME promoters holding less than this after the issue (percent).
pub const SME_MIN_PROMOTER_POST_PCT: f64 = 50.0;
/// Return forgone on money blocked by the UPI mandate, percent a year (a liquid fund).
pub const COST_OF_FUNDS_PCT: f64 = 7.0;
/// Calendar days the money stays blocked after the issue closes (allotment
/// on T+1, refund or listing by T+3, plus a weekend).
pub const BLOCK_DAYS_AFTER_CLOSE: i64 = 4;
/// On the closing day the institutional book counts as final from this hour (IST).
pub const QIB_FINAL_HOUR_IST: u32 = 15;
/// APPLY with high confidence needs P(loss) at or under this.
pub const HIGH_CONF_P_LOSS: f64 = 0.10;
/// Anchor shares: half are locked in for 30 days from allotment, the rest for 90.
pub const ANCHOR_HALF_UNLOCK_DAYS: i64 = 30;
pub const ANCHOR_FULL_UNLOCK_DAYS: i64 = 90;
/// Share of the allotment the listing-day plan books at the expected gain.
pub const BOOK_FRACTION_PCT: f64 = 50.0;

/// What the engine says about an IPO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kind {
    Apply,
    ApplySellOnListing,
    Wait,
    Avoid,
    NoCall,
    /// Already trading: the stock action engine decides.
    Listed,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Apply => "APPLY",
            Kind::ApplySellOnListing => "APPLY – SELL ON LISTING",
            Kind::Wait => "WAIT",
            Kind::Avoid => "AVOID",
            Kind::NoCall => "NO CALL",
            Kind::Listed => "LISTED",
        }
    }

    /// Short form for board columns.
    pub fn short(self) -> &'static str {
        match self {
            Kind::ApplySellOnListing => "APPLY–SELL ON LISTING",
            k => k.label(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_uppercase().replace(['_', '–', '-'], " ");
        let s: Vec<&str> = s.split_whitespace().collect();
        Some(match s.join(" ").as_str() {
            "APPLY" => Kind::Apply,
            "APPLY SELL ON LISTING" => Kind::ApplySellOnListing,
            "WAIT" => Kind::Wait,
            "AVOID" => Kind::Avoid,
            "NO CALL" | "NOCALL" => Kind::NoCall,
            "LISTED" => Kind::Listed,
            _ => return None,
        })
    }
}

pub use crate::action::Confidence;

/// One application category's allotment odds at its minimum size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryOdds {
    /// `Retail`, `sNII`, `bNII` or `SME individual`.
    pub category: String,
    /// Lots in the smallest application of this category.
    pub min_lots: u64,
    /// Money blocked by that application.
    pub min_amount_inr: f64,
    /// What a successful applicant receives (bNII winners get an sNII-sized packet).
    pub allotted_lots: u64,
    pub allotted_amount_inr: f64,
    pub subscription_x: Option<f64>,
    pub shares_offered: Option<u64>,
    pub applications: Option<u64>,
    /// Chance that one minimum-size application is allotted, 0 to 1.
    pub p_allot: Option<f64>,
    /// `applications` (lots available ÷ applications) or `lower bound`
    /// (1 ÷ share subscription, when the application count is unknown).
    pub basis: String,
}

/// The book of one category as parsed from the exchange.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CategoryBook {
    pub offered_shares: Option<u64>,
    pub applications: Option<u64>,
    pub times: Option<f64>,
}

/// Lots in the smallest application above `floor_inr` rupees.
pub fn lots_above(floor_inr: f64, lot_inr: f64) -> u64 {
    if lot_inr <= 0.0 {
        return 1;
    }
    (((floor_inr + 1.0) / lot_inr).ceil() as u64).max(1)
}

/// Chance one minimum application is allotted: lots of the minimum size
/// available ÷ applications. SEBI allots retail one lot, and NII one
/// minimum sNII application, per applicant by lottery when oversubscribed.
/// Without an application count the share subscription gives a lower bound
/// (every application is at least the minimum, so applications ≤ bids ÷
/// minimum, and odds ≥ 1 ÷ subscription).
pub fn p_allot(book: &CategoryBook, packet_shares: u64) -> (Option<f64>, &'static str) {
    if let Some(x) = book.times
        && x <= 1.0
    {
        return (Some(1.0), "undersubscribed");
    }
    if let (Some(off), Some(apps)) = (book.offered_shares, book.applications)
        && apps > 0
        && packet_shares > 0
    {
        let packets = (off / packet_shares) as f64;
        return (
            Some((packets / apps as f64).clamp(0.0, 1.0)),
            "applications",
        );
    }
    match book.times {
        Some(x) if x > 0.0 => (Some((1.0 / x).clamp(0.0, 1.0)), "lower bound"),
        _ => (None, "unknown"),
    }
}

/// Odds for the retail, sNII and bNII minimum applications (mainboard) or
/// the SME individual minimum.
pub fn category_odds(
    board: BoardKind,
    lot: u64,
    cap: f64,
    retail: &CategoryBook,
    snii: &CategoryBook,
    bnii: &CategoryBook,
) -> Vec<CategoryOdds> {
    let lot_inr = lot as f64 * cap;
    let row = |category: &str, min_lots: u64, packet_lots: u64, book: &CategoryBook| {
        let (p, basis) = p_allot(book, packet_lots * lot);
        CategoryOdds {
            category: category.to_string(),
            min_lots,
            min_amount_inr: (min_lots as f64 * lot_inr).round(),
            allotted_lots: packet_lots,
            allotted_amount_inr: (packet_lots as f64 * lot_inr).round(),
            subscription_x: book.times,
            shares_offered: book.offered_shares,
            applications: book.applications,
            p_allot: p,
            basis: basis.to_string(),
        }
    };
    match board {
        BoardKind::Sme => {
            // SME individual applicants bid at least two lots, above ₹2 lakh.
            let min = lots_above(200_000.0, lot_inr).max(2);
            vec![row("SME individual", min, min, retail)]
        }
        BoardKind::Mainboard => {
            let snii_min = lots_above(200_000.0, lot_inr);
            let bnii_min = lots_above(1_000_000.0, lot_inr).max(snii_min + 1);
            vec![
                row("Retail", 1, 1, retail),
                row("sNII", snii_min, snii_min, snii),
                row("bNII", bnii_min, snii_min, bnii),
            ]
        }
    }
}

/// Expected value of one minimum application in a category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryEv {
    pub category: String,
    pub amount_blocked_inr: f64,
    pub p_allot: f64,
    pub expected_listing_gain_pct: f64,
    /// Return forgone on the blocked money.
    pub blocked_cost_inr: f64,
    /// P(allotment) × expected listing gain × allotted amount − blocked cost.
    pub ev_inr: f64,
}

/// Days the money is blocked when applying on `today` for an issue closing on `close`.
pub fn blocked_days(today: NaiveDate, close: Option<NaiveDate>) -> i64 {
    let until_close = close.map_or(0, |c| (c - today).num_days().max(0));
    until_close + BLOCK_DAYS_AFTER_CLOSE
}

pub fn expected_value(o: &CategoryOdds, gain_pct: f64, days: i64) -> Option<CategoryEv> {
    let p = o.p_allot?;
    let cost = o.min_amount_inr * COST_OF_FUNDS_PCT / 100.0 * days as f64 / 365.0;
    let ev = p * gain_pct / 100.0 * o.allotted_amount_inr - cost;
    Some(CategoryEv {
        category: o.category.clone(),
        amount_blocked_inr: o.min_amount_inr,
        p_allot: p,
        expected_listing_gain_pct: gain_pct,
        blocked_cost_inr: round(cost, 2),
        ev_inr: round(ev, 2),
    })
}

// ── Valuation ──

/// Who sells in the offer for sale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfsSeller {
    pub name: String,
    /// `promoter`, `pe`, `vc` or `other`.
    #[serde(default)]
    pub kind: String,
}

impl OfsSeller {
    pub fn is_fund(&self) -> bool {
        matches!(
            self.kind.trim().to_ascii_lowercase().as_str(),
            "pe" | "vc" | "fund" | "private equity" | "venture capital"
        )
    }
}

/// Figures from the RHP ("Capital Structure", "Basis for Offer Price",
/// "Restated Financial Information"), kept by the user in
/// `ipo/<SYMBOL>.toml` in the data directory. Amounts in ₹ crore, oldest
/// year first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RhpFigures {
    pub pre_issue_shares: Option<f64>,
    /// New shares issued; when absent, the fresh issue amount ÷ upper band.
    pub fresh_shares: Option<f64>,
    pub ofs_shares: Option<f64>,
    pub revenue_cr: Vec<f64>,
    pub pat_cr: Vec<f64>,
    pub net_worth_cr: Option<f64>,
    pub total_debt_cr: Option<f64>,
    pub cash_cr: Option<f64>,
    pub promoter_pre_pct: Option<f64>,
    pub promoter_post_pct: Option<f64>,
    pub ofs_sellers: Vec<OfsSeller>,
    /// Listed peers named in the RHP (NSE symbols).
    pub peers: Vec<String>,
}

pub const RHP_TEMPLATE: &str = r#"# RHP figures for one IPO. Save as ipo/<SYMBOL>.toml in the StockWise data
# directory (`stockwise config path` shows it). Amounts in rupees crore, oldest
# financial year first. Leave out what you don't have: missing figures lower
# the verdict's data coverage, they are never guessed.

pre_issue_shares = 0          # "Capital Structure": shares before the issue
# fresh_shares = 0            # new shares; default: fresh issue amount / upper band
# ofs_shares = 0
revenue_cr = []               # revenue from operations, e.g. [812.4, 1034.0, 1290.7]
pat_cr = []                   # restated profit after tax, same years
net_worth_cr = 0.0            # latest year
total_debt_cr = 0.0
cash_cr = 0.0
promoter_pre_pct = 0.0
promoter_post_pct = 0.0
peers = []                    # "Comparison with listed industry peers", NSE symbols

# One entry per selling shareholder; kind = promoter, pe, vc or other.
# [[ofs_sellers]]
# name = "Some Fund LP"
# kind = "pe"
"#;

/// Valuation of a listed peer, from a live quote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerQuote {
    pub symbol: String,
    pub pe: Option<f64>,
    pub pb: Option<f64>,
    pub ev_sales: Option<f64>,
}

/// The issue's valuation at the upper band and against its listed peers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Valuation {
    pub post_issue_shares: Option<f64>,
    /// Post-issue market cap at the upper band, ₹ crore.
    pub market_cap_cr: Option<f64>,
    pub pe: Option<f64>,
    /// Price over post-issue book (net worth plus fresh proceeds).
    pub pb: Option<f64>,
    /// (market cap + debt − cash − fresh proceeds) ÷ latest revenue.
    pub ev_sales: Option<f64>,
    pub roe_pct: Option<f64>,
    pub revenue_cagr_pct: Option<f64>,
    pub pat_cagr_pct: Option<f64>,
    pub debt_to_equity: Option<f64>,
    pub loss_making: Option<bool>,
    pub promoter_post_pct: Option<f64>,
    /// Offer for sale as a share of the issue, percent.
    pub ofs_pct: Option<f64>,
    /// Whether a PE or VC fund sells in the OFS (`None` when the sellers are unknown).
    pub ofs_fund_exit: Option<bool>,
    pub ofs_sellers: Vec<String>,
    pub peer_median_pe: Option<f64>,
    pub peer_median_pb: Option<f64>,
    pub peer_median_ev_sales: Option<f64>,
    pub peers_used: Vec<String>,
    /// P/E ÷ peer median P/E.
    pub pe_vs_peers: Option<f64>,
    /// Where the figures came from.
    pub sources: Vec<String>,
}

/// Compound annual growth of `v` (oldest first), percent; `None` without
/// two positive endpoints.
pub fn cagr_pct(v: &[f64]) -> Option<f64> {
    let (first, last) = (*v.first()?, *v.last()?);
    if v.len() < 2 || first <= 0.0 || last <= 0.0 {
        return None;
    }
    Some(((last / first).powf(1.0 / (v.len() - 1) as f64) - 1.0) * 100.0)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite() && *x > 0.0);
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[m - 1] + v[m]) / 2.0
    } else {
        v[m]
    })
}

/// Valuation at the upper band `cap`. `fresh_cr` and `ofs_cr` come from the
/// exchange's issue size text when the RHP figures lack share counts.
pub fn valuation(
    rhp: Option<&RhpFigures>,
    cap: Option<f64>,
    fresh_cr: Option<f64>,
    ofs_cr: Option<f64>,
    peers: &[PeerQuote],
) -> Valuation {
    let mut v = Valuation {
        ofs_pct: match (fresh_cr, ofs_cr) {
            (Some(f), Some(o)) if f + o > 0.0 => Some(o / (f + o) * 100.0),
            _ => None,
        },
        ..Default::default()
    };
    if let Some(r) = rhp {
        v.sources
            .push("RHP figures (ipo/<SYMBOL>.toml)".to_string());
        let fresh_shares = r
            .fresh_shares
            .or_else(|| Some(fresh_cr? * 1e7 / cap.filter(|c| *c > 0.0)?));
        // Share counts from the RHP beat the rupee split in the issue text.
        if let (Some(f), Some(o), Some(c)) = (fresh_shares, r.ofs_shares, cap)
            && (f + o) > 0.0
        {
            v.ofs_pct = Some(o * c / ((f + o) * c) * 100.0);
        }
        v.post_issue_shares = match (r.pre_issue_shares, fresh_shares) {
            (Some(p), Some(f)) if p > 0.0 => Some(p + f),
            (Some(p), None) if p > 0.0 && fresh_cr == Some(0.0) => Some(p),
            _ => None,
        };
        let fresh_proceeds_cr = fresh_cr.or_else(|| Some(fresh_shares? * cap? / 1e7));
        v.market_cap_cr = match (v.post_issue_shares, cap) {
            (Some(s), Some(c)) => Some(s * c / 1e7),
            _ => None,
        };
        let pat = r.pat_cr.last().copied();
        let rev = r.revenue_cr.last().copied();
        v.loss_making = pat.map(|p| p <= 0.0);
        v.pe = match (v.market_cap_cr, pat) {
            (Some(m), Some(p)) if p > 0.0 => Some(m / p),
            _ => None,
        };
        v.pb = match (v.market_cap_cr, r.net_worth_cr, fresh_proceeds_cr) {
            (Some(m), Some(nw), fp) if nw + fp.unwrap_or(0.0) > 0.0 => {
                Some(m / (nw + fp.unwrap_or(0.0)))
            }
            _ => None,
        };
        v.ev_sales = match (v.market_cap_cr, rev) {
            (Some(m), Some(rv)) if rv > 0.0 => {
                let ev = m + r.total_debt_cr.unwrap_or(0.0)
                    - r.cash_cr.unwrap_or(0.0)
                    - fresh_proceeds_cr.unwrap_or(0.0);
                Some(ev / rv)
            }
            _ => None,
        };
        v.roe_pct = match (pat, r.net_worth_cr) {
            (Some(p), Some(nw)) if nw > 0.0 => Some(p / nw * 100.0),
            _ => None,
        };
        v.debt_to_equity = match (r.total_debt_cr, r.net_worth_cr) {
            (Some(d), Some(nw)) if nw > 0.0 => Some(d / nw),
            _ => None,
        };
        v.revenue_cagr_pct = cagr_pct(&r.revenue_cr);
        v.pat_cagr_pct = cagr_pct(&r.pat_cr);
        v.promoter_post_pct = r.promoter_post_pct;
        v.ofs_sellers = r
            .ofs_sellers
            .iter()
            .map(|s| format!("{} ({})", s.name, s.kind))
            .collect();
        if !r.ofs_sellers.is_empty() {
            v.ofs_fund_exit = Some(r.ofs_sellers.iter().any(OfsSeller::is_fund));
        } else if v.ofs_pct == Some(0.0) {
            v.ofs_fund_exit = Some(false);
        }
    } else if v.ofs_pct == Some(0.0) {
        v.ofs_fund_exit = Some(false);
    }
    if !peers.is_empty() {
        v.sources
            .push("live quotes of listed peers (Yahoo)".to_string());
    }
    v.peer_median_pe = median(peers.iter().filter_map(|p| p.pe).collect());
    v.peer_median_pb = median(peers.iter().filter_map(|p| p.pb).collect());
    v.peer_median_ev_sales = median(peers.iter().filter_map(|p| p.ev_sales).collect());
    v.peers_used = peers
        .iter()
        .filter(|p| p.pe.is_some())
        .map(|p| p.symbol.clone())
        .collect();
    v.pe_vs_peers = match (v.pe, v.peer_median_pe) {
        (Some(a), Some(b)) if b > 0.0 => Some(a / b),
        _ => None,
    };
    for x in [
        &mut v.market_cap_cr,
        &mut v.pe,
        &mut v.pb,
        &mut v.ev_sales,
        &mut v.roe_pct,
        &mut v.revenue_cagr_pct,
        &mut v.pat_cagr_pct,
        &mut v.debt_to_equity,
        &mut v.ofs_pct,
        &mut v.peer_median_pe,
        &mut v.peer_median_pb,
        &mut v.peer_median_ev_sales,
        &mut v.pe_vs_peers,
    ] {
        *x = x.map(|n| round(n, 2));
    }
    v
}

// ── Inputs and verdict ──

/// Everything the rules read about one IPO.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub symbol: String,
    pub board: BoardKind,
    pub status: IpoStatus,
    pub today: NaiveDate,
    /// True once the institutional book is complete: after 3 PM IST on the
    /// closing day, or later.
    pub book_final: bool,
    pub open_date: Option<NaiveDate>,
    pub close_date: Option<NaiveDate>,
    pub listing_date: Option<NaiveDate>,
    pub cap_price: Option<f64>,
    pub lot_size: Option<u64>,
    pub qib_x: Option<f64>,
    pub nii_x: Option<f64>,
    pub retail_x: Option<f64>,
    pub total_x: Option<f64>,
    /// Whether the issue reserves shares for QIBs (SME issues may not).
    pub qib_portion: Option<bool>,
    pub odds: Vec<CategoryOdds>,
    pub issue_size_cr: Option<f64>,
    /// Whether the fresh/OFS split is known.
    pub structure_known: bool,
    pub valuation: Valuation,
    pub gmp_pct: Option<f64>,
    pub regime_pct: Option<f64>,
}

impl Inputs {
    pub fn new(symbol: &str, board: BoardKind, status: IpoStatus, today: NaiveDate) -> Self {
        Self {
            symbol: symbol.to_string(),
            board,
            status,
            today,
            book_final: false,
            open_date: None,
            close_date: None,
            listing_date: None,
            cap_price: None,
            lot_size: None,
            qib_x: None,
            nii_x: None,
            retail_x: None,
            total_x: None,
            qib_portion: None,
            odds: Vec::new(),
            issue_size_cr: None,
            structure_known: false,
            valuation: Valuation::default(),
            gmp_pct: None,
            regime_pct: None,
        }
    }

    fn bidding_open(&self) -> bool {
        !matches!(self.status, IpoStatus::Upcoming)
    }
}

/// Whether the institutional book is final at `hour_ist` on `today`.
pub fn book_is_final(
    status: IpoStatus,
    close: Option<NaiveDate>,
    today: NaiveDate,
    hour_ist: u32,
) -> bool {
    match status {
        IpoStatus::ClosedPendingListing | IpoStatus::Listed => true,
        IpoStatus::Upcoming => false,
        IpoStatus::Ongoing => match close {
            Some(c) => today > c || (today == c && hour_ist >= QIB_FINAL_HOUR_IST),
            None => false,
        },
    }
}

/// The listing-day plan for an allotment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListingPlan {
    /// Book [`BOOK_FRACTION_PCT`] of the shares (all of them for APPLY –
    /// SELL ON LISTING) at or above this price: the model's expected gain.
    pub book_at: f64,
    pub book_fraction_pct: f64,
    /// Exit what is left on a trade below the issue price.
    pub stop: f64,
    pub steps: Vec<String>,
}

/// An anchor lock-in ending: shares that may be sold from that day.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unlock {
    pub date: String,
    /// Share of the anchor book released.
    pub anchor_share_pct: f64,
    pub note: String,
}

/// The model's reading of the listing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListingOutlook {
    pub expected_gain_pct: f64,
    /// Probability of a listing-day close below the issue price, 0 to 1.
    pub p_loss: f64,
    /// 10th and 90th percentile of the listing-day return.
    pub p10_pct: f64,
    pub p90_pct: f64,
    /// Whether the regime input was known or the training mean stood in.
    pub regime_known: bool,
    pub model_fit_date: String,
    pub model_n: usize,
    pub model_r2: f64,
    pub model_resid_sd: f64,
}

/// The engine's answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub kind: Kind,
    pub label: &'static str,
    pub confidence: Confidence,
    /// Up to three reasons, most important first.
    pub reasons: Vec<String>,
    /// WAIT: what turns it into an APPLY (`APPLY if QIB ≥ 12.4x by 3 PM IST on 25 Nov 2026`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// The QIB multiple behind the trigger or the invalidation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qib_threshold_x: Option<f64>,
    /// What would make the verdict wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalidation: Option<String>,
    /// When to look again (`YYYY-MM-DD`).
    pub review_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outlook: Option<ListingOutlook>,
    /// Expected value of one minimum application per category, at the current book.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub expected_value: Vec<CategoryEv>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<ListingPlan>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overhang: Vec<Unlock>,
    /// Gates that fired (AVOID) or would fire once the data is complete.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<String>,
    pub coverage: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    pub methodology: &'static str,
    pub disclaimer: &'static str,
}

impl Verdict {
    /// `IPO VERDICT: APPLY · medium confidence · expected listing +18.2% · P(loss) 12% · review by 26 Nov 2026`.
    pub fn headline(&self) -> String {
        match self.kind {
            Kind::NoCall => {
                let what = if self.missing.is_empty() {
                    self.reasons.first().cloned().unwrap_or_default()
                } else {
                    format!("missing: {}", self.missing.join(", "))
                };
                format!("IPO VERDICT: NO CALL — {what}")
            }
            Kind::Listed => format!(
                "IPO VERDICT: LISTED — {}",
                self.reasons.first().cloned().unwrap_or_default()
            ),
            k => {
                let mut parts = vec![
                    format!("IPO VERDICT: {}", k.label()),
                    format!("{} confidence", self.confidence.label()),
                ];
                if let Some(o) = &self.outlook {
                    parts.push(format!(
                        "expected listing {:+.1}% · P(loss) {:.0}%",
                        o.expected_gain_pct,
                        o.p_loss * 100.0
                    ));
                }
                if let Some(t) = &self.trigger {
                    parts.push(t.clone());
                }
                if let Some(d) = self
                    .review_by
                    .as_deref()
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                {
                    parts.push(format!("review by {}", d.format("%d %b %Y")));
                }
                parts.join(" · ")
            }
        }
    }

    /// Short cell for the board: `APPLY (med)`, `WAIT: QIB ≥ 12.4x`, `NO CALL`.
    pub fn cell(&self) -> String {
        match self.kind {
            Kind::NoCall | Kind::Listed => self.kind.label().to_string(),
            Kind::Wait => match self.qib_threshold_x {
                Some(q) => format!("WAIT: QIB ≥ {q:.1}x"),
                None => "WAIT".to_string(),
            },
            k => format!("{} ({})", k.short(), short_conf(self.confidence)),
        }
    }

    /// The headline, reasons, invalidation, plan, overhang, then the
    /// methodology and disclaimer, one per line with `indent`.
    pub fn render(&self, indent: &str) -> String {
        let mut o = format!("{indent}{}\n", self.headline());
        for r in &self.reasons {
            o.push_str(&format!("{indent}  • {r}\n"));
        }
        if let Some(inv) = &self.invalidation {
            o.push_str(&format!("{indent}  • Invalidated if {inv}\n"));
        }
        o.push_str(&format!(
            "{indent}  Methodology {} · data coverage {:.0}%. {}\n",
            self.methodology, self.coverage, self.disclaimer
        ));
        o
    }
}

fn short_conf(c: Confidence) -> &'static str {
    match c {
        Confidence::High => "high",
        Confidence::Medium => "med",
        Confidence::Low => "low",
    }
}

/// Coverage (percent) and the names of missing fields. Book fields are not
/// expected before bidding opens.
pub fn coverage(i: &Inputs) -> (f64, Vec<String>) {
    let v = &i.valuation;
    let book = i.bidding_open();
    let checks: Vec<(&str, bool, bool)> = match i.board {
        BoardKind::Mainboard => vec![
            ("price band", i.cap_price.is_some(), true),
            ("lot size", i.lot_size.is_some(), true),
            ("fresh/OFS split", i.structure_known, true),
            ("QIB subscription", i.qib_x.is_some(), book),
            ("NII subscription", i.nii_x.is_some(), book),
            ("retail subscription", i.retail_x.is_some(), book),
            ("post-issue shares", v.post_issue_shares.is_some(), true),
            ("profit after tax", v.loss_making.is_some(), true),
            (
                "P/E at upper band",
                v.pe.is_some() || v.loss_making == Some(true),
                true,
            ),
            ("peer median P/E", v.peer_median_pe.is_some(), true),
            ("revenue growth", v.revenue_cagr_pct.is_some(), true),
            (
                "promoter holding after issue",
                v.promoter_post_pct.is_some(),
                true,
            ),
            (
                "OFS sellers",
                v.ofs_fund_exit.is_some(),
                v.ofs_pct.is_none_or(|p| p > 0.0),
            ),
            ("IPO market regime", i.regime_pct.is_some(), true),
        ],
        BoardKind::Sme => vec![
            ("price band", i.cap_price.is_some(), true),
            ("lot size", i.lot_size.is_some(), true),
            ("issue size", i.issue_size_cr.is_some(), true),
            ("QIB portion", i.qib_portion.is_some(), true),
            ("total subscription", i.total_x.is_some(), book),
            (
                "promoter holding after issue",
                v.promoter_post_pct.is_some(),
                true,
            ),
        ],
    };
    let applicable: Vec<_> = checks.iter().filter(|c| c.2).collect();
    let have = applicable.iter().filter(|c| c.1).count();
    let missing = applicable
        .iter()
        .filter(|c| !c.1)
        .map(|c| c.0.to_string())
        .collect();
    let pct = if applicable.is_empty() {
        0.0
    } else {
        (have as f64 / applicable.len() as f64 * 100.0).round()
    };
    (pct, missing)
}

/// Hard gates: each one alone is an AVOID.
pub fn hard_gates(i: &Inputs) -> Vec<String> {
    let v = &i.valuation;
    let mut g = Vec::new();
    if let (Some(r), Some(pe), Some(peer)) = (v.pe_vs_peers, v.pe, v.peer_median_pe)
        && r > PE_STRETCH_AVOID
    {
        g.push(format!(
            "P/E {pe:.1} at the upper band is {r:.2}× the peer median of {peer:.1} (limit {PE_STRETCH_AVOID}×)"
        ));
    }
    if let (Some(ofs), Some(true)) = (v.ofs_pct, v.ofs_fund_exit)
        && ofs > OFS_AVOID_PCT
    {
        g.push(format!(
            "{ofs:.0}% of the issue is an offer for sale with a PE/VC fund selling (limit {OFS_AVOID_PCT:.0}%)"
        ));
    }
    if v.loss_making == Some(true)
        && v.revenue_cagr_pct
            .is_none_or(|c| c < LOSS_MAKER_MIN_REVENUE_CAGR)
    {
        g.push(match v.revenue_cagr_pct {
            Some(c) => format!(
                "Loss-making, and revenue grows only {c:.0}% a year (needs {LOSS_MAKER_MIN_REVENUE_CAGR:.0}% to justify the losses)"
            ),
            None => "Loss-making, with no revenue growth on record to justify it".to_string(),
        });
    }
    if i.book_final
        && i.board == BoardKind::Mainboard
        && let Some(q) = i.qib_x
        && q < 1.0
    {
        g.push(format!("QIB portion only {q:.2}x subscribed at close"));
    }
    if i.book_final
        && let Some(t) = i.total_x
        && t < 1.0
    {
        g.push(format!("Issue only {t:.2}x subscribed at close"));
    }
    if let Some(gmp) = i.gmp_pct
        && gmp < 0.0
    {
        g.push(format!(
            "Grey market premium reported negative ({gmp:+.1}%, unofficial press figure)"
        ));
    }
    if i.board == BoardKind::Sme {
        if i.qib_portion == Some(false) {
            g.push("SME issue with no QIB portion: no institutional price check".to_string());
        }
        if let Some(cr) = i.issue_size_cr
            && cr < SME_MIN_ISSUE_CR
        {
            g.push(format!(
                "Tiny float: ₹{cr:.1} crore issue (SME minimum for a call ₹{SME_MIN_ISSUE_CR:.0} crore)"
            ));
        }
        if let Some(p) = v.promoter_post_pct
            && p < SME_MIN_PROMOTER_POST_PCT
        {
            g.push(format!(
                "Promoters keep only {p:.0}% after the issue (SME minimum {SME_MIN_PROMOTER_POST_PCT:.0}%)"
            ));
        }
    }
    g
}

/// The next weekday after `d`.
fn next_weekday(d: NaiveDate) -> NaiveDate {
    let mut n = d + Duration::days(1);
    while matches!(n.weekday(), Weekday::Sat | Weekday::Sun) {
        n += Duration::days(1);
    }
    n
}

/// `n` weekdays after `d` (exchange holidays not counted).
fn weekdays_after(d: NaiveDate, n: u32) -> NaiveDate {
    (0..n).fold(d, |acc, _| next_weekday(acc))
}

/// Anchor lock-in endings: allotment is one working day after close; half
/// the anchor shares unlock 30 days later, the rest 90 days later.
pub fn anchor_unlocks(close: Option<NaiveDate>, today: NaiveDate) -> Vec<Unlock> {
    let Some(close) = close else {
        return Vec::new();
    };
    let allot = next_weekday(close);
    [
        (ANCHOR_HALF_UNLOCK_DAYS, 50.0),
        (ANCHOR_FULL_UNLOCK_DAYS, 50.0),
    ]
    .into_iter()
    .map(|(days, share)| {
        let date = allot + Duration::days(days);
        let left = (date - today).num_days();
        Unlock {
            date: date.to_string(),
            anchor_share_pct: share,
            note: if left >= 0 {
                format!(
                    "{share:.0}% of anchor shares may be sold from {} ({left} days): supply overhang",
                    date.format("%d %b %Y")
                )
            } else {
                format!(
                    "{share:.0}% of anchor shares unlocked on {}",
                    date.format("%d %b %Y")
                )
            },
        }
    })
    .collect()
}

/// When to look again: the listing date (or close + 3 working days) for
/// APPLY and AVOID; the closing day for WAIT.
fn review_date(i: &Inputs, kind: Kind) -> Option<String> {
    let listing = i
        .listing_date
        .or_else(|| i.close_date.map(|c| weekdays_after(c, 3)));
    let d = match kind {
        Kind::Wait => i.close_date.or(listing),
        _ => listing,
    };
    Some(d.unwrap_or(i.today + Duration::days(7)).to_string())
}

fn no_call(i: &Inputs, reason: String, coverage: f64, missing: Vec<String>) -> Verdict {
    Verdict {
        kind: Kind::NoCall,
        label: Kind::NoCall.label(),
        confidence: Confidence::Low,
        reasons: vec![reason],
        trigger: None,
        qib_threshold_x: None,
        invalidation: None,
        review_by: review_date(i, Kind::Wait),
        outlook: None,
        expected_value: Vec::new(),
        plan: None,
        overhang: Vec::new(),
        gates: Vec::new(),
        coverage,
        missing,
        methodology: METHODOLOGY,
        disclaimer: DISCLAIMER,
    }
}

fn pct(v: f64) -> String {
    format!("{v:+.1}%")
}

/// Decide the verdict. `model` is the listing-day model in use
/// ([`crate::ipo_model::active`]); without one, mainboard issues get NO CALL.
pub fn decide(i: &Inputs, model: Option<&ListingModel>) -> Verdict {
    let (coverage, missing) = coverage(i);
    if i.status == IpoStatus::Listed {
        let mut v = no_call(
            i,
            format!(
                "{} already trades; `stockwise {}` gives the stock's action",
                i.symbol, i.symbol
            ),
            coverage,
            Vec::new(),
        );
        v.kind = Kind::Listed;
        v.label = Kind::Listed.label();
        v.overhang = anchor_unlocks(i.close_date, i.today)
            .into_iter()
            .filter(|u| NaiveDate::parse_from_str(&u.date, "%Y-%m-%d").is_ok_and(|d| d >= i.today))
            .collect();
        return v;
    }
    let gates = hard_gates(i);

    if i.board == BoardKind::Sme {
        if !gates.is_empty() {
            return avoid(i, gates, coverage, missing, None, Vec::new());
        }
        let mut v = no_call(
            i,
            "The listing model is fitted on mainboard issues only; SME issues list under a 90% \
             pre-open price cap and trade in lots, so the model does not apply. No SME red flag \
             fired."
                .to_string(),
            coverage,
            missing,
        );
        v.overhang = anchor_unlocks(i.close_date, i.today);
        return v;
    }

    let Some(model) = model else {
        return no_call(
            i,
            "No calibrated listing-day model is installed (run `stockwise ipo calibrate`)"
                .to_string(),
            coverage,
            missing,
        );
    };

    if coverage < MIN_COVERAGE {
        let mut v = no_call(
            i,
            format!(
                "Data coverage {coverage:.0}% is below {MIN_COVERAGE:.0}%; add the RHP figures \
                 (`stockwise ipo {} --rhp-template`)",
                i.symbol
            ),
            coverage,
            missing,
        );
        v.gates = gates;
        return v;
    }

    let features = Features {
        qib_x: i.qib_x.unwrap_or(0.0),
        nii_x: i.nii_x.unwrap_or(0.0),
        retail_x: i.retail_x.unwrap_or(0.0),
        regime_pct: i.regime_pct,
    };
    let mu = model.predict(&features);
    let p_loss = model.p_loss(mu);
    let outlook = ListingOutlook {
        expected_gain_pct: round(mu, 1),
        p_loss: round(p_loss, 3),
        p10_pct: round(model.quantile(mu, 0.10), 1),
        p90_pct: round(model.quantile(mu, 0.90), 1),
        regime_known: i.regime_pct.is_some(),
        model_fit_date: model.fit_date.clone(),
        model_n: model.n,
        model_r2: model.r2,
        model_resid_sd: model.resid_sd,
    };
    let days = blocked_days(i.today, i.close_date);
    let ev_at = |gain: f64| -> Vec<CategoryEv> {
        i.odds
            .iter()
            .filter_map(|o| expected_value(o, gain, days))
            .collect()
    };
    let evs = ev_at(mu);
    // Before bidding opens the odds are unknown: the EV test waits for them.
    let retail_ev_positive = |gain: f64| ev_at(gain).first().is_none_or(|e| e.ev_inr > 0.0);

    if !gates.is_empty() {
        return avoid(i, gates, coverage, missing, Some(outlook), evs);
    }

    let listing_ok = |m: f64, p: f64| m > 0.0 && p < MAX_P_LOSS && retail_ev_positive(m);
    let listing_strong = listing_ok(mu, p_loss);

    // Long-term case: fairly valued against peers, not a fund exit, profitable.
    let v = &i.valuation;
    let mut long_term_weak: Vec<String> = Vec::new();
    match v.pe_vs_peers {
        Some(r) if r > PE_STRETCH_APPLY => long_term_weak.push(format!(
            "P/E is {r:.2}× the peer median (APPLY needs at most {PE_STRETCH_APPLY}×)"
        )),
        Some(_) => {}
        None => long_term_weak.push("no P/E comparison with listed peers".to_string()),
    }
    if let Some(ofs) = v.ofs_pct
        && ofs > OFS_LONG_TERM_PCT
    {
        long_term_weak.push(format!(
            "{ofs:.0}% of the issue is an offer for sale (money goes to sellers, not the company)"
        ));
    }
    if v.loss_making == Some(true) {
        long_term_weak.push("loss-making".to_string());
    }
    let if_strong = if long_term_weak.is_empty() {
        Kind::Apply
    } else {
        Kind::ApplySellOnListing
    };

    let threshold = model.qib_threshold(&features, listing_ok);
    let valuation_reason = || match (v.pe, v.peer_median_pe, v.pe_vs_peers) {
        (Some(pe), Some(peer), Some(r)) => Some(format!(
            "P/E {pe:.1} at the upper band vs peer median {peer:.1} ({r:.2}×)"
        )),
        _ => None,
    };

    let mut reasons: Vec<String> = Vec::new();
    let kind;
    let mut trigger = None;
    let invalidation;

    let mut threshold_shown = threshold;
    if !i.book_final {
        // Institutions bid on the last afternoon: wait for their book. The
        // trigger never goes below 1x, the AVOID gate at close.
        match threshold {
            Some(q) => {
                let q = q.max(1.0);
                threshold_shown = Some(q);
                kind = Kind::Wait;
                let day = i.close_date.map_or("the closing day".to_string(), |d| {
                    d.format("%d %b %Y").to_string()
                });
                trigger = Some(format!(
                    "{} if QIB ≥ {q:.1}x by 3 PM IST on {day}",
                    if_strong.label()
                ));
                let now = i.qib_x.unwrap_or(0.0);
                reasons.push(if now >= q {
                    format!(
                        "Book still building: QIB {} already clears the {q:.1}x the model needs \
                         (with NII and retail at their current levels); the call waits for the \
                         institutional book to close at 3 PM IST",
                        times(i.qib_x)
                    )
                } else {
                    format!(
                        "Book still building: QIB {} so far; the model needs QIB ≥ {q:.1}x (with \
                         NII and retail at their current levels) for a positive expected listing \
                         with P(loss) under {:.0}%",
                        times(i.qib_x),
                        MAX_P_LOSS * 100.0
                    )
                });
                invalidation = Some(format!("QIB ends below {q:.1}x at close"));
            }
            None => {
                return avoid(
                    i,
                    vec![format!(
                        "No QIB subscription up to 1,000x gives a positive expected listing with \
                         P(loss) under {:.0}% at the current NII and retail demand",
                        MAX_P_LOSS * 100.0
                    )],
                    coverage,
                    missing,
                    Some(outlook),
                    evs,
                );
            }
        }
    } else if !listing_strong {
        let ev_retail = evs.first().map(|e| e.ev_inr);
        return avoid(
            i,
            vec![format!(
                "Final book gives an expected listing of {} with a {:.0}% chance of closing below \
                 the issue price{}",
                pct(mu),
                p_loss * 100.0,
                ev_retail.map_or(String::new(), |e| format!(
                    "; expected value of a retail application ₹{e:.0}"
                ))
            )],
            coverage,
            missing,
            Some(outlook),
            evs,
        );
    } else {
        kind = if_strong;
        reasons.push(format!(
            "Expected listing {} (10th–90th percentile {} to {}), P(loss) {:.0}%, from QIB {}, \
             NII {}, retail {}",
            pct(mu),
            pct(outlook.p10_pct),
            pct(outlook.p90_pct),
            p_loss * 100.0,
            times(i.qib_x),
            times(i.nii_x),
            times(i.retail_x)
        ));
        if let Some(e) = evs.first() {
            reasons.push(format!(
                "{} application: {:.1}% allotment odds, expected value ₹{:.0} on ₹{:.0} blocked",
                e.category,
                e.p_allot * 100.0,
                e.ev_inr,
                e.amount_blocked_inr
            ));
        }
        invalidation = Some("the listing-day price trades below the issue price".to_string());
    }
    match kind {
        Kind::ApplySellOnListing => {
            reasons.push(format!("Listing case only: {}", long_term_weak.join("; ")))
        }
        Kind::Apply => {
            if let Some(r) = valuation_reason() {
                reasons.push(r);
            }
        }
        _ => {
            if let Some(r) = valuation_reason() {
                reasons.push(r);
            } else if !long_term_weak.is_empty() {
                reasons.push(format!(
                    "Long-term case weak: {}",
                    long_term_weak.join("; ")
                ));
            }
        }
    }
    reasons.truncate(3);

    let confidence = if coverage >= 85.0 && i.book_final && p_loss <= HIGH_CONF_P_LOSS {
        Confidence::High
    } else if coverage >= 70.0 && i.regime_pct.is_some() {
        Confidence::Medium
    } else {
        Confidence::Low
    };
    let plan = matches!(kind, Kind::Apply | Kind::ApplySellOnListing)
        .then(|| listing_plan(i, kind, mu))
        .flatten();
    Verdict {
        kind,
        label: kind.label(),
        confidence,
        reasons,
        trigger,
        qib_threshold_x: threshold_shown.map(|q| round(q, 1)),
        invalidation,
        review_by: review_date(i, kind),
        outlook: Some(outlook),
        expected_value: evs,
        plan,
        overhang: anchor_unlocks(i.close_date, i.today),
        gates: Vec::new(),
        coverage,
        missing,
        methodology: METHODOLOGY,
        disclaimer: DISCLAIMER,
    }
}

fn avoid(
    i: &Inputs,
    gates: Vec<String>,
    coverage: f64,
    missing: Vec<String>,
    outlook: Option<ListingOutlook>,
    evs: Vec<CategoryEv>,
) -> Verdict {
    let confidence = if coverage >= 70.0 {
        Confidence::High
    } else {
        Confidence::Medium
    };
    let mut reasons = gates.clone();
    reasons.truncate(3);
    Verdict {
        kind: Kind::Avoid,
        label: Kind::Avoid.label(),
        confidence,
        reasons,
        trigger: None,
        qib_threshold_x: None,
        invalidation: None,
        review_by: review_date(i, Kind::Avoid),
        outlook,
        expected_value: evs,
        plan: None,
        overhang: anchor_unlocks(i.close_date, i.today),
        gates,
        coverage,
        missing,
        methodology: METHODOLOGY,
        disclaimer: DISCLAIMER,
    }
}

fn listing_plan(i: &Inputs, kind: Kind, mu: f64) -> Option<ListingPlan> {
    let issue = i.cap_price?;
    let book_at = round(issue * (1.0 + mu / 100.0), 2);
    let fraction = if kind == Kind::ApplySellOnListing {
        100.0
    } else {
        BOOK_FRACTION_PCT
    };
    let mut steps = vec![
        format!(
            "Listing day: sell {fraction:.0}% of the allotted shares at ₹{book_at:.2} or better \
             (the model's expected gain, {})",
            pct(mu)
        ),
        format!("Exit what is left if it trades below the issue price, ₹{issue:.2}"),
    ];
    steps.push(if kind == Kind::Apply {
        format!(
            "From the day after listing, `stockwise {}` gives the stock's action for the rest",
            i.symbol
        )
    } else {
        "Sell the rest on listing day too: the long-term case is weak".to_string()
    });
    Some(ListingPlan {
        book_at,
        book_fraction_pct: fraction,
        stop: issue,
        steps,
    })
}

fn times(v: Option<f64>) -> String {
    v.map_or("—".to_string(), |x| format!("{x:.2}x"))
}

fn round(v: f64, dp: i32) -> f64 {
    let f = 10f64.powi(dp);
    (v * f).round() / f
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Checks one IPO verdict object: a known kind, one to three reasons,
    /// the disclaimer and methodology; APPLY only at 60%+ coverage with an
    /// outlook; WAIT with a trigger.
    pub fn check_verdict(v: &serde_json::Value) {
        let kind = v["kind"].as_str().expect("verdict.kind");
        let kind = Kind::parse(kind).unwrap_or_else(|| panic!("unknown verdict kind {kind}"));
        let reasons = v["reasons"].as_array().expect("verdict.reasons");
        assert!((1..=3).contains(&reasons.len()), "1 to 3 reasons: {v}");
        assert!(
            v["disclaimer"]
                .as_str()
                .is_some_and(|d| d.contains("not investment advice")),
            "{v}"
        );
        assert_eq!(v["methodology"].as_str(), Some(METHODOLOGY), "{v}");
        match kind {
            Kind::Apply | Kind::ApplySellOnListing => {
                assert!(v["coverage"].as_f64().unwrap() >= MIN_COVERAGE, "{v}");
                assert!(v["outlook"].is_object(), "{v}");
                assert!(v["review_by"].is_string(), "{v}");
            }
            Kind::Wait => assert!(v["trigger"].is_string(), "{v}"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipo_model::testing::model;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn rhp() -> RhpFigures {
        RhpFigures {
            pre_issue_shares: Some(9.0e7),
            fresh_shares: Some(1.0e7),
            ofs_shares: None,
            revenue_cr: vec![800.0, 1000.0, 1250.0],
            pat_cr: vec![60.0, 80.0, 100.0],
            net_worth_cr: Some(600.0),
            total_debt_cr: Some(100.0),
            cash_cr: Some(50.0),
            promoter_pre_pct: Some(80.0),
            promoter_post_pct: Some(72.0),
            ofs_sellers: Vec::new(),
            peers: vec!["PEER1".into(), "PEER2".into(), "PEER3".into()],
        }
    }

    fn peers(pe: f64) -> Vec<PeerQuote> {
        [pe * 0.9, pe, pe * 1.1]
            .iter()
            .enumerate()
            .map(|(n, p)| PeerQuote {
                symbol: format!("PEER{}", n + 1),
                pe: Some(*p),
                pb: Some(4.0),
                ev_sales: Some(2.0),
            })
            .collect()
    }

    /// A clean mainboard issue, final book, strong demand, fair valuation.
    /// Upper band ₹200, 10 crore post-issue shares: market cap ₹2,000 crore,
    /// P/E 20 against a peer median of 20.
    fn strong() -> Inputs {
        let mut i = Inputs::new(
            "GOODIPO",
            BoardKind::Mainboard,
            IpoStatus::Ongoing,
            d("2026-11-25"),
        );
        i.book_final = true;
        i.open_date = Some(d("2026-11-21"));
        i.close_date = Some(d("2026-11-25"));
        i.cap_price = Some(200.0);
        i.lot_size = Some(75);
        i.qib_x = Some(80.0);
        i.nii_x = Some(30.0);
        i.retail_x = Some(8.0);
        i.total_x = Some(35.0);
        i.structure_known = true;
        i.odds = category_odds(
            BoardKind::Mainboard,
            75,
            200.0,
            &CategoryBook {
                offered_shares: Some(3_500_000),
                applications: Some(1_000_000),
                times: Some(8.0),
            },
            &CategoryBook {
                offered_shares: Some(500_000),
                applications: Some(40_000),
                times: Some(25.0),
            },
            &CategoryBook {
                offered_shares: Some(1_000_000),
                applications: Some(20_000),
                times: Some(32.0),
            },
        );
        i.valuation = valuation(
            Some(&rhp()),
            Some(200.0),
            Some(200.0),
            Some(0.0),
            &peers(20.0),
        );
        i.regime_pct = Some(15.0);
        i
    }

    #[test]
    fn valuation_maths_at_the_upper_band() {
        let v = valuation(
            Some(&rhp()),
            Some(200.0),
            Some(200.0),
            Some(0.0),
            &peers(20.0),
        );
        assert_eq!(v.post_issue_shares, Some(1.0e8));
        assert_eq!(v.market_cap_cr, Some(2000.0));
        assert_eq!(v.pe, Some(20.0));
        // Post-issue book: 600 + 200 fresh proceeds.
        assert_eq!(v.pb, Some(2.5));
        // EV: 2000 + 100 debt - 50 cash - 200 fresh cash = 1850, over 1250 revenue.
        assert_eq!(v.ev_sales, Some(1.48));
        assert_eq!(v.roe_pct, Some(16.67));
        assert_eq!(v.revenue_cagr_pct, Some(25.0));
        assert_eq!(v.pat_cagr_pct, Some(29.1));
        assert_eq!(v.debt_to_equity, Some(0.17));
        assert_eq!(v.peer_median_pe, Some(20.0));
        assert_eq!(v.pe_vs_peers, Some(1.0));
        assert_eq!(v.ofs_pct, Some(0.0));
        assert_eq!(v.ofs_fund_exit, Some(false));
        assert_eq!(v.loss_making, Some(false));
        // Fresh shares derived from the amount when the RHP lacks the count.
        let mut r = rhp();
        r.fresh_shares = None;
        let v2 = valuation(Some(&r), Some(200.0), Some(200.0), Some(0.0), &[]);
        assert_eq!(v2.post_issue_shares, Some(1.0e8));
        assert_eq!(v2.peer_median_pe, None);
        assert_eq!(v2.pe_vs_peers, None);
        // No RHP: only the OFS share from the issue text.
        let v3 = valuation(None, Some(200.0), Some(100.0), Some(300.0), &[]);
        assert_eq!(v3.ofs_pct, Some(75.0));
        assert_eq!(v3.market_cap_cr, None);
        assert_eq!(v3.ofs_fund_exit, None);
    }

    #[test]
    fn allotment_odds_are_lots_over_applications() {
        // Lot 75 × ₹200 = ₹15,000. Retail: 35 lakh shares = 46,666 lots for
        // 10 lakh applications: 4.67%, not 1/8x = 12.5%.
        let i = strong();
        let retail = &i.odds[0];
        assert_eq!(retail.category, "Retail");
        assert_eq!(retail.min_amount_inr, 15_000.0);
        assert!((retail.p_allot.unwrap() - 46_666.0 / 1_000_000.0).abs() < 1e-9);
        assert_eq!(retail.basis, "applications");
        // sNII minimum: just above ₹2 lakh = 14 lots (₹2,10,000).
        let snii = &i.odds[1];
        assert_eq!(snii.min_lots, 14);
        assert_eq!(snii.min_amount_inr, 210_000.0);
        // 5 lakh shares / 1,050 shares = 476 packets for 40,000 applications.
        assert!((snii.p_allot.unwrap() - 476.0 / 40_000.0).abs() < 1e-9);
        // bNII: just above ₹10 lakh = 67 lots, winning an sNII-sized packet.
        let bnii = &i.odds[2];
        assert_eq!(bnii.min_lots, 67);
        assert_eq!(bnii.allotted_lots, 14);
        assert!((bnii.p_allot.unwrap() - 952.0 / 20_000.0).abs() < 1e-9);
        // Without applications: the lower bound 1/x.
        let (p, basis) = p_allot(
            &CategoryBook {
                offered_shares: Some(100),
                applications: None,
                times: Some(4.0),
            },
            75,
        );
        assert_eq!((p, basis), (Some(0.25), "lower bound"));
        // Undersubscribed: everyone is allotted.
        let (p, basis) = p_allot(
            &CategoryBook {
                offered_shares: Some(100),
                applications: Some(10),
                times: Some(0.8),
            },
            75,
        );
        assert_eq!((p, basis), (Some(1.0), "undersubscribed"));
        // SME: two lots minimum, above ₹2 lakh.
        let sme = category_odds(
            BoardKind::Sme,
            1200,
            94.0,
            &CategoryBook::default(),
            &CategoryBook::default(),
            &CategoryBook::default(),
        );
        assert_eq!(sme.len(), 1);
        assert_eq!(sme[0].min_lots, 2);
        assert_eq!(sme[0].p_allot, None);
    }

    #[test]
    fn expected_value_is_odds_times_gain_minus_blocked_cost() {
        let o = &strong().odds[0];
        let ev = expected_value(o, 20.0, 4).unwrap();
        // 4.6666% × 20% × ₹15,000 = ₹140.00; cost ₹15,000 × 7% × 4/365 = ₹11.51.
        assert_eq!(ev.blocked_cost_inr, 11.51);
        assert!((ev.ev_inr - (0.046666 * 0.2 * 15_000.0 - 11.5068)).abs() < 0.01);
        assert_eq!(blocked_days(d("2026-11-21"), Some(d("2026-11-25"))), 8);
        assert_eq!(blocked_days(d("2026-11-26"), Some(d("2026-11-25"))), 4);
        // A loss on listing is a negative expected value.
        assert!(expected_value(o, -5.0, 4).unwrap().ev_inr < 0.0);
    }

    #[test]
    fn apply_when_listing_and_valuation_are_both_good() {
        let v = decide(&strong(), Some(&model()));
        assert_eq!(v.kind, Kind::Apply, "{v:?}");
        assert!(v.reasons.len() <= 3);
        let o = v.outlook.as_ref().unwrap();
        assert!(o.expected_gain_pct > 20.0 && o.p_loss < MAX_P_LOSS, "{o:?}");
        assert!(v.reasons[0].starts_with("Expected listing"));
        assert!(v.reasons.iter().any(|r| r.contains("peer median")));
        let plan = v.plan.as_ref().unwrap();
        assert_eq!(plan.stop, 200.0);
        assert_eq!(plan.book_fraction_pct, 50.0);
        assert!(plan.book_at > 200.0);
        assert_eq!(v.overhang.len(), 2);
        // Allotment on the next working day (Thu 26 Nov), unlocks 30 and 90 days later.
        assert_eq!(v.overhang[0].date, "2026-12-26");
        assert_eq!(v.overhang[1].date, "2027-02-24");
        assert_eq!(v.review_by.as_deref(), Some("2026-11-30"));
        assert!(v.headline().starts_with("IPO VERDICT: APPLY · "));
        testing::check_verdict(&serde_json::to_value(&v).unwrap());
    }

    #[test]
    fn stretched_valuation_or_ofs_makes_it_sell_on_listing() {
        let mut i = strong();
        i.valuation = valuation(
            Some(&rhp()),
            Some(200.0),
            Some(200.0),
            Some(0.0),
            &peers(15.0),
        );
        // P/E 20 vs 15 = 1.33×: above 1.2×, below the 1.5× gate.
        let v = decide(&i, Some(&model()));
        assert_eq!(v.kind, Kind::ApplySellOnListing, "{v:?}");
        assert!(v.reasons.iter().any(|r| r.starts_with("Listing case only")));
        assert_eq!(v.plan.as_ref().unwrap().book_fraction_pct, 100.0);
        assert_eq!(v.cell(), "APPLY–SELL ON LISTING (high)");
        crate::action::testing::assert_action_well_formed(&format!(
            "{}\n{}\n{}",
            v.render("  "),
            v.cell(),
            serde_json::json!({ "verdict": v })
        ));

        let mut j = strong();
        j.valuation.ofs_pct = Some(60.0);
        j.valuation.ofs_fund_exit = Some(false);
        assert_eq!(decide(&j, Some(&model())).kind, Kind::ApplySellOnListing);
    }

    #[test]
    fn wait_gives_the_qib_trigger_before_the_book_is_final() {
        let mut i = strong();
        i.book_final = false;
        i.today = d("2026-11-23");
        i.qib_x = Some(0.4);
        i.nii_x = Some(1.0);
        i.retail_x = Some(1.5);
        let v = decide(&i, Some(&model()));
        assert_eq!(v.kind, Kind::Wait, "{v:?}");
        let q = v.qib_threshold_x.unwrap();
        assert!(q > 0.4, "{q}");
        let t = v.trigger.as_deref().unwrap();
        assert!(t.starts_with("APPLY if QIB ≥ "), "{t}");
        assert!(t.ends_with("by 3 PM IST on 25 Nov 2026"), "{t}");
        assert_eq!(v.review_by.as_deref(), Some("2026-11-25"));
        assert!(v.cell().starts_with("WAIT: QIB ≥ "));
        // At the threshold the same issue clears the listing test.
        let mut at = i.clone();
        at.qib_x = Some(q + 0.05);
        at.book_final = true;
        assert_eq!(decide(&at, Some(&model())).kind, Kind::Apply);
        testing::check_verdict(&serde_json::to_value(&v).unwrap());
    }

    #[test]
    fn upcoming_issue_waits_with_book_fields_pending() {
        let mut i = strong();
        i.status = IpoStatus::Upcoming;
        i.book_final = false;
        i.today = d("2026-11-19");
        i.qib_x = None;
        i.nii_x = None;
        i.retail_x = None;
        i.total_x = None;
        // No book yet: no odds, so no expected value either.
        let empty = CategoryBook::default();
        i.odds = category_odds(BoardKind::Mainboard, 75, 200.0, &empty, &empty, &empty);
        assert!(i.odds.iter().all(|o| o.p_allot.is_none()));
        let (cov, missing) = coverage(&i);
        assert_eq!(cov, 100.0, "{missing:?}");
        let v = decide(&i, Some(&model()));
        assert_eq!(v.kind, Kind::Wait, "{v:?}");
        assert!(v.expected_value.is_empty());
        assert!(v.trigger.as_deref().unwrap().starts_with("APPLY if QIB ≥ "));
    }

    #[test]
    fn each_hard_gate_alone_is_an_avoid() {
        let m = model();
        let cases: Vec<(&str, Box<dyn Fn(&mut Inputs)>)> = vec![
            (
                "peer median",
                Box::new(|i| {
                    i.valuation = valuation(
                        Some(&rhp()),
                        Some(200.0),
                        Some(200.0),
                        Some(0.0),
                        &peers(12.0),
                    )
                }),
            ),
            (
                "PE/VC fund",
                Box::new(|i| {
                    i.valuation.ofs_pct = Some(80.0);
                    i.valuation.ofs_fund_exit = Some(true);
                }),
            ),
            (
                "Loss-making",
                Box::new(|i| {
                    i.valuation.loss_making = Some(true);
                    i.valuation.revenue_cagr_pct = Some(10.0);
                }),
            ),
            ("QIB portion only", Box::new(|i| i.qib_x = Some(0.8))),
            ("Issue only", Box::new(|i| i.total_x = Some(0.9))),
            ("Grey market", Box::new(|i| i.gmp_pct = Some(-3.0))),
        ];
        for (needle, change) in cases {
            let mut i = strong();
            change(&mut i);
            let v = decide(&i, Some(&m));
            assert_eq!(v.kind, Kind::Avoid, "{needle}: {v:?}");
            assert!(
                v.reasons[0].contains(needle),
                "{needle} not in {:?}",
                v.reasons
            );
            testing::check_verdict(&serde_json::to_value(&v).unwrap());
        }
        // A loss-maker growing fast enough passes the gate (but not APPLY).
        let mut i = strong();
        i.valuation.loss_making = Some(true);
        i.valuation.revenue_cagr_pct = Some(40.0);
        assert_eq!(decide(&i, Some(&m)).kind, Kind::ApplySellOnListing);
        // OFS above 75% but no fund selling: no gate.
        let mut i = strong();
        i.valuation.ofs_pct = Some(80.0);
        i.valuation.ofs_fund_exit = Some(false);
        assert_eq!(decide(&i, Some(&m)).kind, Kind::ApplySellOnListing);
        // QIB under 1x before the book is final is not a gate.
        let mut i = strong();
        i.book_final = false;
        i.qib_x = Some(0.5);
        assert_eq!(decide(&i, Some(&m)).kind, Kind::Wait);
    }

    #[test]
    fn weak_final_book_is_an_avoid() {
        let mut i = strong();
        i.qib_x = Some(1.5);
        i.nii_x = Some(1.2);
        i.retail_x = Some(1.1);
        i.total_x = Some(1.3);
        i.regime_pct = Some(-5.0);
        let v = decide(&i, Some(&model()));
        assert_eq!(v.kind, Kind::Avoid, "{v:?}");
        assert!(v.reasons[0].starts_with("Final book gives an expected listing"));
        assert!(v.outlook.is_some());
    }

    #[test]
    fn no_call_below_coverage_and_without_a_model() {
        let mut i = strong();
        i.valuation = valuation(None, Some(200.0), Some(200.0), Some(0.0), &[]);
        i.regime_pct = None;
        let v = decide(&i, Some(&model()));
        assert_eq!(v.kind, Kind::NoCall, "{v:?}");
        assert!(v.coverage < MIN_COVERAGE);
        assert!(
            v.missing.contains(&"peer median P/E".to_string()),
            "{:?}",
            v.missing
        );
        assert!(v.headline().starts_with("IPO VERDICT: NO CALL — missing: "));
        // Missing data never stands in as neutral: no outlook either.
        assert!(v.outlook.is_none());

        let v = decide(&strong(), None);
        assert_eq!(v.kind, Kind::NoCall);
        assert!(v.reasons[0].contains("No calibrated listing-day model"));
        testing::check_verdict(&serde_json::to_value(&v).unwrap());
    }

    #[test]
    fn sme_red_flags_avoid_otherwise_no_call() {
        let base = || {
            let mut i = Inputs::new("SMEX", BoardKind::Sme, IpoStatus::Ongoing, d("2026-11-25"));
            i.book_final = true;
            i.close_date = Some(d("2026-11-25"));
            i.cap_price = Some(94.0);
            i.lot_size = Some(1200);
            i.issue_size_cr = Some(40.0);
            i.qib_portion = Some(true);
            i.total_x = Some(50.0);
            i.valuation.promoter_post_pct = Some(70.0);
            i
        };
        let v = decide(&base(), Some(&model()));
        assert_eq!(v.kind, Kind::NoCall);
        assert!(v.reasons[0].contains("mainboard issues only"));
        for (needle, change) in [
            (
                "no QIB portion",
                Box::new(|i: &mut Inputs| i.qib_portion = Some(false)) as Box<dyn Fn(&mut Inputs)>,
            ),
            (
                "Tiny float",
                Box::new(|i: &mut Inputs| i.issue_size_cr = Some(6.0)),
            ),
            (
                "Promoters keep only",
                Box::new(|i: &mut Inputs| i.valuation.promoter_post_pct = Some(40.0)),
            ),
        ] {
            let mut i = base();
            change(&mut i);
            let v = decide(&i, Some(&model()));
            assert_eq!(v.kind, Kind::Avoid, "{needle}");
            assert!(v.reasons[0].contains(needle), "{:?}", v.reasons);
        }
    }

    #[test]
    fn listed_issue_hands_off_to_the_stock_engine() {
        let mut i = strong();
        i.status = IpoStatus::Listed;
        i.today = d("2026-12-01");
        let v = decide(&i, Some(&model()));
        assert_eq!(v.kind, Kind::Listed);
        assert!(v.reasons[0].contains("`stockwise GOODIPO`"));
        // Both anchor unlocks are still ahead.
        assert_eq!(v.overhang.len(), 2);
        assert!(v.headline().starts_with("IPO VERDICT: LISTED — "));
    }

    #[test]
    fn book_is_final_after_three_pm_on_the_closing_day() {
        let close = Some(d("2026-11-25"));
        assert!(!book_is_final(
            IpoStatus::Ongoing,
            close,
            d("2026-11-25"),
            14
        ));
        assert!(book_is_final(
            IpoStatus::Ongoing,
            close,
            d("2026-11-25"),
            15
        ));
        assert!(!book_is_final(
            IpoStatus::Ongoing,
            close,
            d("2026-11-24"),
            18
        ));
        assert!(book_is_final(
            IpoStatus::ClosedPendingListing,
            None,
            d("2026-11-24"),
            9
        ));
        assert!(!book_is_final(
            IpoStatus::Upcoming,
            close,
            d("2026-11-30"),
            9
        ));
    }

    #[test]
    fn kinds_parse_from_labels_and_json() {
        for k in [
            Kind::Apply,
            Kind::ApplySellOnListing,
            Kind::Wait,
            Kind::Avoid,
            Kind::NoCall,
            Kind::Listed,
        ] {
            assert_eq!(Kind::parse(k.label()), Some(k));
            let json = serde_json::to_value(k).unwrap();
            assert_eq!(Kind::parse(json.as_str().unwrap()), Some(k));
        }
        assert_eq!(cagr_pct(&[100.0]), None);
        assert_eq!(cagr_pct(&[-5.0, 10.0]), None);
    }
}
