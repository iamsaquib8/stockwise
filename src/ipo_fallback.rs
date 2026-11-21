//! Fallback IPO data for when NSE does not answer, or answers with gaps.
//!
//! NSE's IPO pages (`crate::ipo`) are the primary source. When they refuse
//! the request or leave fields empty, StockWise asks BSE's public-issue
//! pages, the same JSON that bseindia.com/publicissue loads:
//!
//! - `GetPublicIssue_par_updated/w?flag=1`: open and forthcoming issues
//!   (`Table` rows: `Scrip_Name`, `Scrip_cd`, `IPO_NO`, `Start_Dt`, `End_Dt`,
//!   `Price_Band`, `IR_flag`, `Status` `L`/`F`).
//! - `ipo_details_ng/w?stripono=N`: the issue terms as label / value rows
//!   (`TableRows`: `Label`, `Label_flg`, `Value`).
//! - `Pubissues_BBS_CumultveCatdem_ng/w?IPO_NO=N`: the consolidated
//!   (BSE + NSE) category demand (`Table`: `CATEGORYNAME`, `BID_FOR` shares
//!   offered, `QUANTITY` shares bid, `PERC` times subscribed).
//!
//! The parsers turn BSE's answers into the NSE shapes `crate::ipo` already
//! reads (`issueInfo.dataList`, `activeCat.dataList`), so one set of rules
//! reads both. Values are merged field by field: an NSE value is kept, a
//! missing one is filled from BSE, and each filled field is recorded in the
//! answer's `meta` (`field_sources`). A field neither source has stays
//! empty; the verdict's data coverage drops and, below 60%, it is NO CALL.
//!
//! No other source is used. The IPO aggregators StockWise looked at either
//! disallow automated access in robots.txt or forbid it in their terms.

use crate::ipo::{
    BoardKind, IpoStatus, IpoSummaryRow, IssueStructure, PriceBand, SubscriptionBreakdown,
};
use crate::output::round;
use crate::provenance::Meta;
use crate::providers::bse;
use crate::providers::session::{FetchError, Session};
use chrono::NaiveDate;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// How long BSE's list and an issue's pages stay cached.
const LIST_TTL: Duration = Duration::from_secs(120);
const DETAIL_TTL: Duration = Duration::from_secs(90);
/// Most issues whose BSE detail pages one run reads.
const DETAIL_BUDGET: usize = 9;

/// BSE's list of open and forthcoming public issues.
pub const BSE_BOARD_PATH: &str = "/GetPublicIssue_par_updated/w";
/// BSE's terms of one issue, by IPO number.
pub const BSE_DETAIL_PATH: &str = "/ipo_details_ng/w";
/// BSE's consolidated category demand of one issue, by IPO number.
pub const BSE_DEMAND_PATH: &str = "/Pubissues_BBS_CumultveCatdem_ng/w";

/// Field names as they appear in `meta.field_sources` and `meta.missing`.
pub const F_PRICE_BAND: &str = "price band";
pub const F_LOT: &str = "lot size";
pub const F_DATES: &str = "issue dates";
pub const F_SIZE: &str = "issue size";
pub const F_STRUCTURE: &str = "fresh/OFS split";
pub const F_SUBSCRIPTION: &str = "category subscription";
pub const F_BOARD_ROW: &str = "board entry";

/// One open or forthcoming issue on BSE's public-issue list.
#[derive(Debug, Clone, PartialEq)]
pub struct BseIssue {
    /// BSE's IPO number: the key for the detail and demand pages.
    pub ipo_no: String,
    /// BSE scrip code (assigned before listing; may be empty).
    pub scrip_code: String,
    pub company_name: String,
    pub status: IpoStatus,
    pub open_date: Option<NaiveDate>,
    pub close_date: Option<NaiveDate>,
    pub price_band: PriceBand,
    /// `None` when the list does not say (the detail page does).
    pub board: Option<BoardKind>,
}

impl BseIssue {
    /// The symbol a board row shows when NSE does not list the issue.
    pub fn symbol(&self) -> String {
        if self.scrip_code.trim().is_empty() {
            format!("BSE-IPO-{}", self.ipo_no.trim())
        } else {
            self.scrip_code.trim().to_string()
        }
    }
}

/// The rows of a BSE answer: `Table`, `table` or a bare array.
fn rows(v: &Value) -> Vec<&Value> {
    let arr = v
        .get("Table")
        .or_else(|| v.get("table"))
        .and_then(Value::as_array)
        .or_else(|| v.as_array());
    arr.map(|a| a.iter().collect()).unwrap_or_default()
}

fn text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::String(s) if !s.trim().is_empty() && s.trim() != "-" => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn number(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        Value::String(s) => {
            let cleaned: String = s.chars().filter(|c| *c != ',' && *c != ' ').collect();
            cleaned.parse::<f64>().ok().filter(|f| f.is_finite())
        }
        _ => None,
    })
}

/// A BSE date: `2026-11-19T00:00:00`, `19/11/2026`, `19 Nov 2026` or
/// `19-Nov-2026`.
pub fn bse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    let date_part = s.split('T').next().unwrap_or(s).trim();
    if let Ok(d) = NaiveDate::parse_from_str(date_part, "%d/%m/%Y") {
        return Some(d);
    }
    crate::ipo::parse_nse_date(date_part)
}

/// The board kind a BSE platform or issue-type string names.
fn board_from(s: &str) -> Option<BoardKind> {
    let lc = s.to_ascii_lowercase();
    if lc.contains("sme") {
        Some(BoardKind::Sme)
    } else if lc.contains("main") || lc.trim() == "eq" || lc.contains("equity") {
        Some(BoardKind::Mainboard)
    } else {
        None
    }
}

/// Open (`L`) and forthcoming (`F`) equity IPOs from BSE's public-issue list.
/// FPOs, OFS, debt, InvITs and REITs are left out.
pub fn parse_bse_board(v: &Value) -> Vec<BseIssue> {
    let mut out = Vec::new();
    for r in rows(v) {
        let Some(name) = text(
            r,
            &["Scrip_Name", "Scrip_name", "ScripName", "Company_Name"],
        ) else {
            continue;
        };
        let Some(ipo_no) = text(r, &["IPO_NO", "IPONo", "Ipo_No"]) else {
            continue;
        };
        let kind = text(r, &["IR_flag", "IR_FLAG", "Ir_Flag"]).unwrap_or_default();
        if !kind.is_empty() && !kind.eq_ignore_ascii_case("IPO") {
            continue;
        }
        let status = match text(r, &["Status", "STATUS"]).as_deref() {
            Some(s) if s.eq_ignore_ascii_case("L") => IpoStatus::Ongoing,
            Some(s) if s.eq_ignore_ascii_case("F") => IpoStatus::Upcoming,
            _ => continue,
        };
        let board = ["Exchange_Platform", "EXCHANGE_PLATFORM", "Platform", "TYPE"]
            .iter()
            .find_map(|k| text(r, &[k]).and_then(|s| board_from(&s)))
            .or_else(|| {
                // BSE SME issues carry the platform in the name column at times.
                name.to_ascii_lowercase()
                    .contains(" - sme")
                    .then_some(BoardKind::Sme)
            });
        out.push(BseIssue {
            ipo_no,
            scrip_code: text(r, &["Scrip_cd", "Scrip_Cd", "SCRIP_CD"]).unwrap_or_default(),
            company_name: name,
            status,
            open_date: text(r, &["Start_Dt", "START_DT"]).and_then(|s| bse_date(&s)),
            close_date: text(r, &["End_Dt", "END_DT"]).and_then(|s| bse_date(&s)),
            price_band: crate::ipo::parse_price_band(
                &text(r, &["Price_Band", "PRICE_BAND", "Price_band"]).unwrap_or_default(),
            ),
            board,
        });
    }
    out
}

/// What BSE's detail page adds to an issue, in NSE's shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BseDetail {
    pub board: Option<BoardKind>,
    pub open_date: Option<NaiveDate>,
    pub close_date: Option<NaiveDate>,
    /// `{"issueInfo": {"dataList": [{title, value}]}}` for
    /// [`crate::ipo::parse_issue_info`].
    pub issue_info: Value,
}

/// BSE's issue terms (`TableRows` label / value pairs), with the labels
/// renamed to the titles NSE's `issueInfo` uses.
pub fn parse_bse_detail(v: &Value) -> BseDetail {
    let list = v
        .get("TableRows")
        .or_else(|| v.get("tableRows"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = BseDetail::default();
    let mut data = Vec::new();
    let mut issue_size: Option<String> = None;
    let mut parts: Vec<String> = Vec::new();
    for row in &list {
        let label = text(row, &["Label", "label"]).unwrap_or_default();
        let flag = text(row, &["Label_flg", "label_flg"]).unwrap_or_default();
        let value = text(row, &["Value", "value"]).unwrap_or_default();
        let value = strip_tags(&value);
        if value.is_empty() {
            continue;
        }
        let lc = label.to_ascii_lowercase();
        if flag.eq_ignore_ascii_case("exchange_platform") || lc.contains("exchange platform") {
            out.board = board_from(&value);
            continue;
        }
        let title = if lc.contains("cut off") || lc.contains("cut-off") || lc.contains("premium") {
            None
        } else if lc.contains("price band")
            || lc.contains("price range")
            || lc.contains("issue price")
            || lc.contains("offer price")
        {
            Some("Price Range")
        } else if lc.contains("minimum bid")
            || lc.contains("market lot")
            || lc.contains("lot size")
            || lc.contains("bid lot")
        {
            Some("Bid Lot")
        } else if lc.contains("fresh issue") || lc.contains("offer for sale") || lc == "ofs" {
            parts.push(format!("{label}: {value}"));
            None
        } else if lc.contains("issue size") || lc == "total issue" {
            issue_size = Some(value.clone());
            None
        } else if lc.contains("issue period") || lc.contains("bid period") {
            let (o, c) = issue_period(&value);
            out.open_date = o;
            out.close_date = c;
            None
        } else if lc.contains("issue type") || lc == "type of issue" {
            Some("Issue Type")
        } else if lc.contains("face value") {
            Some("Face Value")
        } else if lc.contains("lead manager") {
            Some("Book Running Lead Manager")
        } else if lc.contains("registrar") {
            Some("Name of the Registrar")
        } else {
            None
        };
        if let Some(t) = title {
            data.push(json!({"title": t, "value": value}));
        }
    }
    // The fresh / OFS rows tell the split when the size row does not.
    let size = match issue_size {
        Some(s) => {
            let lc = s.to_ascii_lowercase();
            if parts.is_empty() || lc.contains("fresh") || lc.contains("offer for sale") {
                Some(s)
            } else {
                Some(format!("{s}; {}", parts.join("; ")))
            }
        }
        None if !parts.is_empty() => Some(parts.join("; ")),
        None => None,
    };
    if let Some(s) = size {
        data.push(json!({"title": "Issue Size", "value": s}));
    }
    if !data.is_empty() {
        out.issue_info = json!({"issueInfo": {"dataList": data}});
    }
    out
}

/// `19 Nov 2026 to 23 Nov 2026`, `19-Nov-2026 - 23-Nov-2026` or
/// `19/11/2026 - 23/11/2026`.
fn issue_period(s: &str) -> (Option<NaiveDate>, Option<NaiveDate>) {
    let lc = s.to_ascii_lowercase();
    for sep in [" to ", " - ", " – "] {
        if let Some(i) = lc.find(sep) {
            return (bse_date(&s[..i]), bse_date(&s[i + sep.len()..]));
        }
    }
    (None, None)
}

/// Drop HTML tags BSE leaves in label values.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&#8377;", "₹")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// NSE's serial number for a BSE category name, so `parse_subscription`
/// and `category_books` read both alike. `None`: a row to leave out
/// (anchor investors, market maker).
fn category_sr(name: &str) -> Option<&'static str> {
    let lc = name.to_ascii_lowercase();
    if lc.contains("anchor") || lc.contains("market maker") {
        return None;
    }
    if lc.trim() == "total" {
        return Some("Total");
    }
    if lc.contains("foreign") || lc.contains("fii") || lc.contains("fpi") {
        return Some("1(a)");
    }
    if lc.contains("domestic financial") || lc.contains("dfi") || lc.contains("bank") {
        return Some("1(b)");
    }
    if lc.contains("mutual fund") {
        return Some("1(c)");
    }
    if lc.contains("qualified institutional") || lc.contains("qib") {
        return Some("1");
    }
    let snii = lc.contains("snii")
        || lc.contains("two lakh")
        || lc.contains("2 lakh")
        || lc.contains("up to ten")
        || lc.contains("upto ten")
        || lc.contains("up to 10")
        || lc.contains("upto 10");
    if snii {
        return Some("2.2");
    }
    if lc.contains("bnii") || lc.contains("ten lakh") || lc.contains("10 lakh") {
        return Some("2.1");
    }
    if lc.contains("non institutional") || lc.contains("non-institutional") || lc.contains("nii") {
        return Some("2");
    }
    if lc.contains("retail") || lc.contains("individual investor") || lc.contains("rii") {
        return Some("3");
    }
    if lc.contains("employee") {
        return Some("4");
    }
    if lc.contains("shareholder") {
        return Some("5");
    }
    Some("Other")
}

/// BSE's consolidated category demand as NSE's
/// `{"activeCat": {"dataList": [...]}}` for `crate::ipo::parse_subscription`.
/// `None` when BSE has no rows yet.
pub fn bse_demand_as_nse(v: &Value) -> Option<Value> {
    let mut list = Vec::new();
    for r in rows(v) {
        let Some(name) = text(r, &["CATEGORYNAME", "CategoryName", "Category"]) else {
            continue;
        };
        let Some(sr) = category_sr(&name) else {
            continue;
        };
        let offered = number(r, &["BID_FOR", "SHARES_OFFERED", "NoOfSharesOffered"]);
        let bid = number(r, &["QUANTITY", "Quantity", "NoOfSharesBid"]);
        let times = number(r, &["PERC", "NoOfTimes"]);
        if offered.is_none() && bid.is_none() && times.is_none() {
            continue;
        }
        let mut row = json!({"srNo": sr, "category": name.trim()});
        if let Some(o) = offered {
            row["noOfShareOffered"] = json!(o.round());
        }
        if let Some(b) = bid {
            row["noOfSharesBid"] = json!(b.round());
        }
        if let Some(t) = times {
            row["noOfTotalMeant"] = json!(t);
        }
        list.push(row);
    }
    (!list.is_empty()).then(|| json!({"activeCat": {"dataList": list}}))
}

/// A company name reduced for matching across exchanges: lower case,
/// letters and digits only, without `limited` / `ltd`.
pub fn name_key(name: &str) -> String {
    let lc = name.to_ascii_lowercase();
    let words: Vec<String> = lc
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty() && !matches!(*w, "limited" | "ltd" | "the" | "sme" | "ipo"))
        .map(str::to_string)
        .collect();
    words.concat()
}

/// Whether two company names are the same issuer.
pub fn same_company(a: &str, b: &str) -> bool {
    let (ka, kb) = (name_key(a), name_key(b));
    if ka.is_empty() || kb.is_empty() {
        return false;
    }
    if ka == kb {
        return true;
    }
    let (short, long) = if ka.len() <= kb.len() {
        (&ka, &kb)
    } else {
        (&kb, &ka)
    };
    short.len() >= 10 && long.starts_with(short.as_str())
}

/// The BSE issue for an NSE row, by company name.
pub fn find_issue<'a>(issues: &'a [BseIssue], company: &str) -> Option<&'a BseIssue> {
    issues
        .iter()
        .find(|i| same_company(&i.company_name, company))
}

/// A board row for an issue only BSE lists.
pub fn row_from_issue(issue: &BseIssue, today: NaiveDate) -> IpoSummaryRow {
    let board = issue.board.unwrap_or(BoardKind::Mainboard);
    let mut row = IpoSummaryRow {
        symbol: issue.symbol(),
        company_name: issue.company_name.clone(),
        board,
        is_bse_sme: board == BoardKind::Sme,
        status: issue.status,
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
    };
    let mut scratch = Meta::default();
    fill_from_issue(&mut row, issue, "", &mut scratch);
    crate::ipo::refresh_row(&mut row, today);
    row
}

/// The `field_sources` key: the field alone for one issue, or prefixed with
/// the symbol on the board.
fn key(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{prefix} {field}")
    }
}

/// Fill the fields of `row` that NSE left empty from BSE's list entry.
/// Returns whether anything was filled.
pub fn fill_from_issue(
    row: &mut IpoSummaryRow,
    issue: &BseIssue,
    prefix: &str,
    meta: &mut Meta,
) -> bool {
    let mut filled = false;
    if row.price_band.cap_price().is_none() && issue.price_band.cap_price().is_some() {
        row.price_band = issue.price_band.clone();
        meta.record(&key(prefix, F_PRICE_BAND), crate::provenance::BSE);
        filled = true;
    }
    if (row.open_date.is_none() && issue.open_date.is_some())
        || (row.close_date.is_none() && issue.close_date.is_some())
    {
        row.open_date = row.open_date.or(issue.open_date);
        row.close_date = row.close_date.or(issue.close_date);
        meta.record(&key(prefix, F_DATES), crate::provenance::BSE);
        filled = true;
    }
    filled
}

/// Fill the fields NSE left empty from BSE's detail and demand pages:
/// board, dates, band, lot, size, fresh/OFS split and the category book.
/// Returns the merged issue structure and subscription.
pub fn fill_from_detail(
    row: &mut IpoSummaryRow,
    structure: &mut IssueStructure,
    sub: &mut SubscriptionBreakdown,
    detail: Option<&BseDetail>,
    demand: Option<&Value>,
    prefix: &str,
    meta: &mut Meta,
) {
    let bse = crate::provenance::BSE;
    if let Some(d) = detail {
        if let Some(b) = d.board
            && b != row.board
            && meta.field_sources.contains_key(&key(prefix, F_BOARD_ROW))
        {
            // Only a BSE-only row takes BSE's board; NSE's series wins otherwise.
            row.board = b;
            row.is_bse_sme = b == BoardKind::Sme;
        }
        if (row.open_date.is_none() && d.open_date.is_some())
            || (row.close_date.is_none() && d.close_date.is_some())
        {
            row.open_date = row.open_date.or(d.open_date);
            row.close_date = row.close_date.or(d.close_date);
            meta.record(&key(prefix, F_DATES), bse);
        }
        let (lot, band, st) = crate::ipo::parse_issue_info(&d.issue_info, &row.price_band);
        if row.price_band.cap_price().is_none()
            && let Some(b) = band
        {
            row.price_band = b;
            meta.record(&key(prefix, F_PRICE_BAND), bse);
        }
        if row.lot_size.is_none() && lot.is_some() {
            row.lot_size = lot;
            meta.record(&key(prefix, F_LOT), bse);
        }
        if structure.total_issue_cr.is_none() && st.total_issue_cr.is_some() {
            structure.total_issue_cr = st.total_issue_cr;
            structure.raw_issue_size = st.raw_issue_size.clone();
            meta.record(&key(prefix, F_SIZE), bse);
        }
        if structure.fresh_pct.is_none() && st.fresh_pct.is_some() {
            structure.fresh_issue_cr = st.fresh_issue_cr;
            structure.ofs_cr = st.ofs_cr;
            structure.fresh_pct = st.fresh_pct;
            structure.structure_tag = st.structure_tag.clone();
            meta.record(&key(prefix, F_STRUCTURE), bse);
        }
        if structure.issue_type.is_none() {
            structure.issue_type = st.issue_type;
        }
        if structure.face_value.is_none() {
            structure.face_value = st.face_value;
        }
        if structure.brlms.is_empty() {
            structure.brlms = st.brlms;
        }
        if structure.registrar_name.is_none() {
            structure.registrar_allotment_url = st.registrar_allotment_url;
            structure.registrar_name = st.registrar_name;
        }
        if structure.structure_tag.is_empty() {
            structure.structure_tag = st.structure_tag;
        }
        if row.issue_size_cr.is_none() {
            row.issue_size_cr = structure.total_issue_cr;
        }
    }
    if let Some(dm) = demand
        && sub.qib_x.is_none()
        && sub.nii_x.is_none()
        && sub.retail_x.is_none()
    {
        let parsed = crate::ipo::parse_subscription(dm);
        if parsed.qib_x.is_some() || parsed.retail_x.is_some() || parsed.total_x.is_some() {
            let mut parsed = parsed;
            parsed.source_note = "Consolidated BSE + NSE bid book (BSE)".to_string();
            *sub = parsed;
            row.qib_x = sub.qib_x;
            row.nii_x = sub.nii_x;
            row.retail_x = sub.retail_x;
            if sub.total_x.is_some() {
                row.total_x = sub.total_x.map(|x| round(x, 2));
            }
            meta.record(&key(prefix, F_SUBSCRIPTION), bse);
        }
    }
}

/// BSE, asked only when needed: the list once per run, then the pages of
/// the issues NSE left incomplete. After a refusal or an outage it is not
/// asked again in the same run.
pub struct BseFallback {
    session: Arc<Session>,
    issues: Option<Vec<BseIssue>>,
    budget: usize,
    down: bool,
}

impl Default for BseFallback {
    fn default() -> Self {
        Self::new(bse::shared_session())
    }
}

impl BseFallback {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            issues: None,
            budget: DETAIL_BUDGET,
            down: false,
        }
    }

    fn failed(&mut self, what: &str, e: &FetchError, meta: &mut Meta) {
        if matches!(e, FetchError::Blocked(_) | FetchError::Unavailable(_)) {
            self.down = true;
        }
        meta.warn(format!("BSE fallback: {what} unavailable ({e})"));
    }

    /// BSE's open and forthcoming IPOs (empty when BSE does not answer).
    pub async fn issues(&mut self, meta: &mut Meta) -> Vec<BseIssue> {
        if self.issues.is_none() {
            let list = if self.down {
                Vec::new()
            } else {
                match self
                    .session
                    .get_json(BSE_BOARD_PATH, &[("flag", "1")], LIST_TTL)
                    .await
                {
                    Ok(v) => parse_bse_board(&v),
                    Err(e) => {
                        self.failed("IPO list", &e, meta);
                        Vec::new()
                    }
                }
            };
            self.issues = Some(list);
        }
        self.issues.clone().unwrap_or_default()
    }

    /// One issue's terms and, when `with_book`, its category book in NSE's
    /// shape.
    pub async fn pages(
        &mut self,
        ipo_no: &str,
        with_book: bool,
        meta: &mut Meta,
    ) -> (Option<BseDetail>, Option<Value>) {
        if self.down || self.budget == 0 {
            return (None, None);
        }
        self.budget -= 1;
        let detail = match self
            .session
            .get_json(BSE_DETAIL_PATH, &[("stripono", ipo_no)], DETAIL_TTL)
            .await
        {
            Ok(v) => Some(parse_bse_detail(&v)),
            Err(e) => {
                self.failed("issue terms", &e, meta);
                None
            }
        };
        let book = if with_book && !self.down {
            match self
                .session
                .get_json(BSE_DEMAND_PATH, &[("IPO_NO", ipo_no)], DETAIL_TTL)
                .await
            {
                Ok(v) => bse_demand_as_nse(&v),
                Err(e) => {
                    self.failed("category book", &e, meta);
                    None
                }
            }
        } else {
            None
        };
        (detail, book)
    }
}

/// Fold BSE's list into NSE's board: fill the empty fields of the rows NSE
/// has, and add the open or forthcoming issues only BSE lists (not ones NSE
/// shows as closed in `recent`, and not ones whose bidding already ended).
pub fn merge_board(
    ongoing: &mut Vec<IpoSummaryRow>,
    upcoming: &mut Vec<IpoSummaryRow>,
    recent: &[IpoSummaryRow],
    issues: &[BseIssue],
    today: NaiveDate,
    meta: &mut Meta,
) {
    let mut used = vec![false; issues.len()];
    for row in ongoing.iter_mut().chain(upcoming.iter_mut()) {
        if let Some(pos) = issues
            .iter()
            .position(|i| same_company(&i.company_name, &row.company_name))
        {
            used[pos] = true;
            let prefix = row.symbol.clone();
            if fill_from_issue(row, &issues[pos], &prefix, meta) {
                crate::ipo::refresh_row(row, today);
            }
        }
    }
    for (issue, used) in issues.iter().zip(used) {
        if used
            || issue.close_date.is_some_and(|c| c < today)
            || recent
                .iter()
                .any(|r| same_company(&r.company_name, &issue.company_name))
        {
            continue;
        }
        let row = row_from_issue(issue, today);
        meta.record(&key(&row.symbol, F_BOARD_ROW), crate::provenance::BSE);
        match row.status {
            IpoStatus::Ongoing => ongoing.push(row),
            _ => upcoming.push(row),
        }
    }
}

/// Whether a row came from BSE alone (NSE does not list it).
pub fn bse_only(row: &IpoSummaryRow, meta: &Meta) -> bool {
    meta.field_sources
        .contains_key(&key(&row.symbol, F_BOARD_ROW))
}

/// Fold BSE's pages into NSE's `ipo-detail` answer (or start one when NSE
/// gave none), field by field: a BSE term goes in only where NSE's is
/// missing, and BSE's category book only when NSE has none. Each field BSE
/// supplied is recorded in `meta`. Also fills the row's dates (and, for a
/// row only BSE lists, its board).
pub fn merge_detail_json(
    detail: &mut Option<Value>,
    row: &mut IpoSummaryRow,
    bse: Option<&BseDetail>,
    book: Option<&Value>,
    meta: &mut Meta,
) {
    let src = crate::provenance::BSE;
    let before = detail
        .as_ref()
        .map(|d| crate::ipo::parse_issue_info(d, &row.price_band));
    let before_sub = detail.as_ref().map(crate::ipo::parse_subscription);
    let had_band =
        row.price_band.cap_price().is_some() || before.as_ref().is_some_and(|b| b.1.is_some());
    let had_lot = row.lot_size.is_some() || before.as_ref().is_some_and(|b| b.0.is_some());
    let had_size = row.issue_size_cr.is_some()
        || before
            .as_ref()
            .is_some_and(|b| b.2.total_issue_cr.is_some());
    let had_split = before.as_ref().is_some_and(|b| b.2.fresh_pct.is_some());
    let had_book = before_sub
        .as_ref()
        .is_some_and(|s| s.qib_x.is_some() || s.nii_x.is_some() || s.retail_x.is_some());

    let mut d = detail.take().unwrap_or_else(|| json!({}));
    if let Some(b) = bse {
        if let Some(board) = b.board
            && meta.field_sources.contains_key(F_BOARD_ROW)
        {
            row.board = board;
            row.is_bse_sme = board == BoardKind::Sme;
        }
        if (row.open_date.is_none() && b.open_date.is_some())
            || (row.close_date.is_none() && b.close_date.is_some())
        {
            row.open_date = row.open_date.or(b.open_date);
            row.close_date = row.close_date.or(b.close_date);
            meta.record(F_DATES, src);
        }
        let nse_titles: Vec<String> = d
            .pointer("/issueInfo/dataList")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(|e| e.get("title").and_then(Value::as_str))
                    .map(|t| t.trim().to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let wanted = |title: &str| match title {
            "Price Range" => !had_band,
            "Bid Lot" => !had_lot,
            "Issue Size" => !had_size || !had_split,
            other => !nse_titles.iter().any(|t| t == &other.to_ascii_lowercase()),
        };
        let extra: Vec<Value> = b
            .issue_info
            .pointer("/issueInfo/dataList")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter(|e| e.get("title").and_then(Value::as_str).is_some_and(wanted))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !extra.is_empty() {
            if d.pointer("/issueInfo/dataList")
                .and_then(Value::as_array)
                .is_none()
            {
                d["issueInfo"] = json!({"dataList": []});
            }
            if let Some(list) = d
                .pointer_mut("/issueInfo/dataList")
                .and_then(Value::as_array_mut)
            {
                list.extend(extra);
            }
        }
    }
    if !had_book && let Some(cat) = book.and_then(|b| b.get("activeCat")) {
        d["activeCat"] = cat.clone();
    }

    let (lot, band, st) = crate::ipo::parse_issue_info(&d, &row.price_band);
    if !had_band && band.is_some() {
        meta.record(F_PRICE_BAND, src);
    }
    if !had_lot && lot.is_some() {
        meta.record(F_LOT, src);
    }
    if !had_size && st.total_issue_cr.is_some() {
        meta.record(F_SIZE, src);
    }
    if !had_split && st.fresh_pct.is_some() {
        meta.record(F_STRUCTURE, src);
    }
    let sub = crate::ipo::parse_subscription(&d);
    if !had_book && (sub.qib_x.is_some() || sub.nii_x.is_some() || sub.retail_x.is_some()) {
        meta.record(F_SUBSCRIPTION, src);
    }
    let empty = d.as_object().is_some_and(serde_json::Map::is_empty);
    *detail = (!empty).then_some(d);
}

/// The key-field checks of one issue's analysis.
pub fn analysis_checks(a: &crate::ipo::IpoAnalysis) -> Vec<(&'static str, bool)> {
    let mut checks = vec![
        (F_PRICE_BAND, a.price_band.cap_price().is_some()),
        (F_LOT, a.lot_size.is_some()),
        (F_DATES, a.open_date.is_some() && a.close_date.is_some()),
        (F_SIZE, a.structure.total_issue_cr.is_some()),
    ];
    if a.board == BoardKind::Mainboard {
        checks.push((F_STRUCTURE, a.structure.fresh_pct.is_some()));
    }
    if matches!(
        a.status,
        IpoStatus::Ongoing | IpoStatus::ClosedPendingListing
    ) {
        let s = &a.subscription;
        checks.push((
            F_SUBSCRIPTION,
            s.qib_x.is_some() || s.nii_x.is_some() || s.retail_x.is_some(),
        ));
    }
    checks
}

/// Whether an open or upcoming row still lacks a term BSE could supply.
pub fn row_has_gaps(row: &IpoSummaryRow) -> bool {
    row.price_band.cap_price().is_none()
        || row.lot_size.is_none()
        || row.open_date.is_none()
        || row.close_date.is_none()
        || row.issue_size_cr.is_none()
}

/// The key-field checks behind a board's or an issue's `meta.coverage`.
pub fn field_checks(
    row: &IpoSummaryRow,
    structure: Option<&IssueStructure>,
) -> Vec<(&'static str, bool)> {
    let mut checks = vec![
        (F_PRICE_BAND, row.price_band.cap_price().is_some()),
        (F_LOT, row.lot_size.is_some()),
        (F_DATES, row.open_date.is_some() && row.close_date.is_some()),
        (
            F_SIZE,
            row.issue_size_cr.is_some() || structure.is_some_and(|s| s.total_issue_cr.is_some()),
        ),
    ];
    if let Some(s) = structure
        && row.board == BoardKind::Mainboard
    {
        checks.push((F_STRUCTURE, s.fresh_pct.is_some()));
    }
    if row.status == IpoStatus::Ongoing {
        checks.push((
            F_SUBSCRIPTION,
            row.qib_x.is_some() || row.retail_x.is_some() || row.total_x.is_some(),
        ));
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/src/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn d_(y: i32, m: u32, day: u32) -> NaiveDate {
        d(y, m, day)
    }

    #[test]
    fn bse_board_keeps_open_and_forthcoming_equity_ipos() {
        let issues = parse_bse_board(&fixture("bse_ipo_board.json"));
        let names: Vec<&str> = issues.iter().map(|i| i.company_name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Mahanadi Coalfields Limited",
                "Shree Siddhivinayak Greentech Industries Limited",
                "Northwind Foods Limited"
            ]
        );
        let mcl = &issues[0];
        assert_eq!(mcl.ipo_no, "2001");
        assert_eq!(mcl.status, IpoStatus::Ongoing);
        assert_eq!(mcl.open_date, Some(d(2026, 11, 19)));
        assert_eq!(mcl.close_date, Some(d(2026, 11, 23)));
        assert_eq!(mcl.price_band.low, Some(129.0));
        assert_eq!(mcl.price_band.high, Some(136.0));
        assert_eq!(mcl.symbol(), "544612");
        let sme = &issues[1];
        assert_eq!(sme.status, IpoStatus::Upcoming);
        assert_eq!(sme.board, Some(BoardKind::Sme));
        assert_eq!(sme.open_date, Some(d(2026, 11, 25)));
        // No scrip code yet: the symbol names BSE's IPO number.
        assert_eq!(issues[2].symbol(), "BSE-IPO-2004");
    }

    #[test]
    fn bse_detail_reads_terms_into_nse_titles() {
        let det = parse_bse_detail(&fixture("bse_ipo_detail.json"));
        assert_eq!(det.board, Some(BoardKind::Mainboard));
        assert_eq!(det.open_date, Some(d(2026, 11, 19)));
        assert_eq!(det.close_date, Some(d(2026, 11, 23)));
        let (lot, band, st) = crate::ipo::parse_issue_info(&det.issue_info, &PriceBand::default());
        assert_eq!(lot, Some(110));
        assert_eq!(band.unwrap().cap_price(), Some(136.0));
        // 66,18,38,000 OFS shares × ₹136 = ₹9,001.0 crore, no fresh issue.
        assert_eq!(st.fresh_pct, Some(0.0));
        assert_eq!(st.total_issue_cr, Some(9001.0));
        assert_eq!(
            st.registrar_name.as_deref(),
            Some("KFin Technologies Limited")
        );
        assert_eq!(st.brlms.len(), 2);
    }

    #[test]
    fn bse_detail_joins_separate_fresh_and_ofs_rows() {
        let v = json!({"TableRows": [
            {"Label": "Price Band", "Value": "Rs. 200 - Rs. 210"},
            {"Label": "Fresh Issue Size", "Value": "Rs. 150.00 Crore"},
            {"Label": "Offer for Sale", "Value": "Rs. 50.00 Crore"},
            {"Label": "Cut off Amount (&#8377;)", "Value": "210"}
        ]});
        let det = parse_bse_detail(&v);
        let (_, band, st) = crate::ipo::parse_issue_info(&det.issue_info, &PriceBand::default());
        assert_eq!(band.unwrap().cap_price(), Some(210.0));
        assert_eq!(st.fresh_issue_cr, Some(150.0));
        assert_eq!(st.ofs_cr, Some(50.0));
        assert_eq!(st.fresh_pct, Some(75.0));
    }

    #[test]
    fn bse_demand_maps_categories_to_nse_serials() {
        let nse = bse_demand_as_nse(&fixture("bse_ipo_demand.json")).unwrap();
        let sub = crate::ipo::parse_subscription(&nse);
        assert_eq!(sub.qib_x, Some(1.37));
        assert_eq!(sub.nii_x, Some(0.18));
        assert_eq!(sub.bnii_x, Some(0.15));
        assert_eq!(sub.snii_x, Some(0.24));
        assert_eq!(sub.retail_x, Some(0.24));
        assert_eq!(sub.employee_x, Some(0.61));
        assert_eq!(sub.total_x, Some(0.48));
        // Anchor investors are not part of the public book.
        assert!(
            sub.categories
                .iter()
                .all(|c| !c.category.contains("Anchor"))
        );
        let (retail, snii, bnii) = crate::ipo::category_books(&sub);
        assert_eq!(retail.offered_shares, Some(231_665_000));
        assert_eq!(snii.offered_shares, Some(33_095_000));
        assert_eq!(bnii.offered_shares, Some(66_190_000));
        assert!(bse_demand_as_nse(&json!({"Table": []})).is_none());
    }

    #[test]
    fn names_match_across_exchanges() {
        assert!(same_company(
            "Mahanadi Coalfields Limited",
            "MAHANADI COALFIELDS LTD"
        ));
        assert!(same_company(
            "Shree Siddhivinayak Greentech Industries Limited",
            "Shree Siddhivinayak Greentech Industries Ltd - SME"
        ));
        assert!(!same_company("Tata Capital Limited", "Tata Steel Limited"));
        assert!(!same_company("ABC Ltd", "ABC Technologies Ltd"));
    }

    fn nse_row(symbol: &str, company: &str) -> IpoSummaryRow {
        let mut row = row_from_issue(
            &BseIssue {
                ipo_no: "0".into(),
                scrip_code: symbol.into(),
                company_name: company.into(),
                status: IpoStatus::Ongoing,
                open_date: None,
                close_date: None,
                price_band: PriceBand::default(),
                board: Some(BoardKind::Mainboard),
            },
            d(2026, 11, 21),
        );
        row.symbol = symbol.into();
        row
    }

    #[test]
    fn merge_keeps_nse_values_and_records_what_bse_filled() {
        let issues = parse_bse_board(&fixture("bse_ipo_board.json"));
        let mut row = nse_row("MAHACOAL", "Mahanadi Coalfields Limited");
        // NSE gave the band but not the dates.
        row.price_band = crate::ipo::parse_price_band("130-135");
        let mut meta = Meta::from_source(crate::provenance::NSE);
        let issue = find_issue(&issues, &row.company_name).unwrap();
        assert!(fill_from_issue(&mut row, issue, "MAHACOAL", &mut meta));
        assert_eq!(row.price_band.high, Some(135.0), "NSE's band is kept");
        assert_eq!(row.close_date, Some(d(2026, 11, 23)));
        assert_eq!(
            meta.field_sources
                .get("MAHACOAL issue dates")
                .map(String::as_str),
            Some("BSE")
        );
        assert!(!meta.field_sources.contains_key("MAHACOAL price band"));

        let det = parse_bse_detail(&fixture("bse_ipo_detail.json"));
        let demand = bse_demand_as_nse(&fixture("bse_ipo_demand.json"));
        let mut st = IssueStructure::default();
        let mut sub = SubscriptionBreakdown::default();
        fill_from_detail(
            &mut row,
            &mut st,
            &mut sub,
            Some(&det),
            demand.as_ref(),
            "MAHACOAL",
            &mut meta,
        );
        assert_eq!(row.lot_size, Some(110));
        assert_eq!(row.qib_x, Some(1.37));
        assert_eq!(st.fresh_pct, Some(0.0));
        assert_eq!(meta.sources, ["NSE", "BSE"]);
        for f in [F_LOT, F_SIZE, F_STRUCTURE, F_SUBSCRIPTION] {
            assert_eq!(
                meta.field_sources
                    .get(&format!("MAHACOAL {f}"))
                    .map(String::as_str),
                Some("BSE"),
                "{f}"
            );
        }
        assert!(field_checks(&row, Some(&st)).iter().all(|(_, ok)| *ok));
    }

    #[test]
    fn nse_subscription_is_not_replaced_by_bse() {
        let mut row = nse_row("MAHACOAL", "Mahanadi Coalfields Limited");
        let mut sub = SubscriptionBreakdown {
            qib_x: Some(2.0),
            ..Default::default()
        };
        let demand = bse_demand_as_nse(&fixture("bse_ipo_demand.json"));
        let mut meta = Meta::default();
        fill_from_detail(
            &mut row,
            &mut IssueStructure::default(),
            &mut sub,
            None,
            demand.as_ref(),
            "",
            &mut meta,
        );
        assert_eq!(sub.qib_x, Some(2.0));
        assert!(meta.field_sources.is_empty());
    }

    #[test]
    fn detail_merge_adds_only_what_nse_lacks() {
        // NSE gave the band and lot, but its issue size has no fresh/OFS
        // wording and there is no category book yet.
        let mut detail = Some(json!({"issueInfo": {"dataList": [
            {"title": "Price Range", "value": "Rs 130 to Rs 135"},
            {"title": "Bid Lot", "value": "100 Equity Shares"},
            {"title": "Issue Size", "value": "Rs 9,001 crore"}
        ]}}));
        let mut row = nse_row("MAHACOAL", "Mahanadi Coalfields Limited");
        let bse = parse_bse_detail(&fixture("bse_ipo_detail.json"));
        let book = bse_demand_as_nse(&fixture("bse_ipo_demand.json"));
        let mut meta = Meta::from_source(crate::provenance::NSE);
        merge_detail_json(&mut detail, &mut row, Some(&bse), book.as_ref(), &mut meta);
        let d = detail.unwrap();
        let (lot, band, st) = crate::ipo::parse_issue_info(&d, &PriceBand::default());
        assert_eq!(lot, Some(100), "NSE's lot is kept");
        assert_eq!(band.unwrap().cap_price(), Some(135.0), "NSE's band is kept");
        assert_eq!(st.fresh_pct, Some(0.0), "BSE supplied the split");
        assert_eq!(crate::ipo::parse_subscription(&d).qib_x, Some(1.37));
        assert_eq!(row.open_date, Some(d_(2026, 11, 19)));
        let filled: Vec<&str> = meta.field_sources.keys().map(String::as_str).collect();
        assert_eq!(filled, [F_SUBSCRIPTION, F_STRUCTURE, F_DATES]);
    }

    #[test]
    fn detail_merge_builds_an_answer_when_nse_gave_none() {
        let mut detail = None;
        let mut row = nse_row("544612", "Mahanadi Coalfields Limited");
        let mut meta = Meta::default();
        meta.record(F_BOARD_ROW, crate::provenance::BSE);
        let bse = parse_bse_detail(&fixture("bse_ipo_detail.json"));
        merge_detail_json(&mut detail, &mut row, Some(&bse), None, &mut meta);
        assert!(detail.is_some());
        for f in [F_PRICE_BAND, F_LOT, F_SIZE, F_STRUCTURE, F_DATES] {
            assert_eq!(
                meta.field_sources.get(f).map(String::as_str),
                Some("BSE"),
                "{f}"
            );
        }
        assert!(!meta.field_sources.contains_key(F_SUBSCRIPTION));
        // Nothing from either source: no detail at all.
        let mut none = None;
        merge_detail_json(&mut none, &mut row, None, None, &mut Meta::default());
        assert!(none.is_none());
    }

    fn read(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/src/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn fallback_reads_bse_list_terms_and_book() {
        use crate::providers::session::DiskCache;
        let server = crate::testutil::MockServer::start_routed(vec![
            ("GetPublicIssue_par_updated", read("bse_ipo_board.json")),
            ("ipo_details_ng", read("bse_ipo_detail.json")),
            (
                "Pubissues_BBS_CumultveCatdem_ng",
                read("bse_ipo_demand.json"),
            ),
        ])
        .await;
        let session = Arc::new(Session::for_tests(bse::site(&server.url), DiskCache::Off));
        let mut fb = BseFallback::new(session);
        let mut meta = Meta::default();
        let issues = fb.issues(&mut meta).await;
        assert_eq!(issues.len(), 3);
        let (det, book) = fb.pages(&issues[0].ipo_no, true, &mut meta).await;
        assert_eq!(det.unwrap().board, Some(BoardKind::Mainboard));
        assert!(book.is_some());
        assert!(meta.warnings.is_empty(), "{:?}", meta.warnings);
        let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
        assert!(
            targets[0].ends_with("/GetPublicIssue_par_updated/w?flag=1"),
            "{targets:?}"
        );
        assert!(targets[1].contains("stripono=2001"), "{targets:?}");
        assert!(targets[2].contains("IPO_NO=2001"), "{targets:?}");
        // The list is read once per run.
        fb.issues(&mut meta).await;
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn a_bse_refusal_is_a_warning_and_stops_further_asks() {
        use crate::providers::session::DiskCache;
        let server =
            crate::testutil::MockServer::start(vec![(403, "<HTML>Access Denied</HTML>".into())])
                .await;
        let session = Arc::new(Session::for_tests(bse::site(&server.url), DiskCache::Off));
        let mut fb = BseFallback::new(session);
        let mut meta = Meta::default();
        assert!(fb.issues(&mut meta).await.is_empty());
        assert_eq!(fb.pages("2001", true, &mut meta).await, (None, None));
        assert_eq!(server.requests().len(), 1, "no ask after the refusal");
        assert_eq!(meta.warnings.len(), 1);
        assert!(meta.warnings[0].contains("BSE"), "{:?}", meta.warnings);
        assert!(meta.sources.is_empty());
    }

    #[test]
    fn a_row_bse_cannot_complete_keeps_its_gaps() {
        let issues = parse_bse_board(&fixture("bse_ipo_board.json"));
        let row = row_from_issue(&issues[2], d(2026, 11, 21));
        assert_eq!(row.status, IpoStatus::Upcoming);
        assert!(row.price_band.cap_price().is_none());
        let checks = field_checks(&row, None);
        let mut meta = Meta::default();
        meta.set_fields(&checks);
        assert!(meta.missing.contains(&F_PRICE_BAND.to_string()));
        assert!(meta.missing.contains(&F_LOT.to_string()));
        assert!(meta.coverage < 60.0);
    }
}
