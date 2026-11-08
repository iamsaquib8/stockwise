//! Indian government bond data from public web pages:
//!
//! * benchmark yields per tenor (T-bills, G-secs, SDLs) from CCIL's
//!   "Tenorwise Indicative Yields" page, with the RBI home page's benchmark
//!   G-sec yields as the fallback;
//! * the list of outstanding Government of India securities (ISIN, coupon,
//!   issue and maturity dates) from RBI, which `bonds calc` uses to look up
//!   a security by name.
//!
//! Neither site has an API; both pages are plain HTML, fetched politely
//! (one request, cached for hours) and parsed here. Prices of individual
//! G-secs and T-bills are not published by either page, and NSE's live debt
//! quotes have no documented endpoint, so StockWise does not show them.
//! CCIL and RBI data are for personal, non-commercial use.

use crate::pagecache;
use anyhow::{Result, bail};
use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use std::time::Duration;

pub const CCIL_URL: &str = "https://www.ccilindia.com/tenorwise-indicative-yields";
pub const RBI_HOME_URL: &str = "https://www.rbi.org.in/";
pub const RBI_GSEC_LIST_URL: &str = "https://www.rbi.org.in/scripts/bs_viewcontent.aspx?Id=1956";

/// Yields change once a day; six hours keeps requests rare.
const YIELDS_MAX_AGE: Duration = Duration::from_secs(6 * 3600);
/// The outstanding list changes with auctions, about weekly.
const LIST_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

// ── HTML tables ──

/// Text of every table row in `html`: one `Vec` of cell texts per `<tr>`,
/// tags stripped, entities decoded, whitespace collapsed. Comments are
/// removed first (RBI leaves stale dates in them). Rows of nested tables
/// come out on their own; callers filter rows by shape.
pub fn table_rows(html: &str) -> Vec<Vec<String>> {
    let html = strip_comments(html);
    let lower = html.to_ascii_lowercase();
    let mut rows = Vec::new();
    let mut start = 0;
    while let Some(end) = lower[start..].find("</tr>").map(|i| start + i) {
        // The row is the last `<tr` before its end (handles nesting).
        if let Some(open) = lower[start..end].rfind("<tr").map(|i| start + i) {
            rows.push(cells(&html[open..end], &lower[open..end]));
        }
        start = end + 5;
    }
    rows
}

fn cells(row: &str, lower: &str) -> Vec<String> {
    let mut starts: Vec<usize> = Vec::new();
    for tag in ["<td", "<th"] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(tag) {
            let at = from + i;
            if lower[at + 3..].starts_with([' ', '>', '\t', '\n', '\r']) {
                starts.push(at);
            }
            from = at + 3;
        }
    }
    starts.sort_unstable();
    starts
        .iter()
        .enumerate()
        .map(|(n, &at)| {
            let body_start = lower[at..].find('>').map_or(row.len(), |i| at + i + 1);
            let end = starts.get(n + 1).copied().unwrap_or(row.len());
            text(&row[body_start.min(end)..end])
        })
        .collect()
}

fn strip_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Visible text of an HTML fragment.
pub fn text(fragment: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in fragment.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&#8377;", "₹")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace('\u{200B}', "");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `1,01,908.561` → 101908.561 (Indian or Western grouping).
fn number(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    cleaned.parse().ok()
}

// ── Benchmark yields ──

/// What a yield row is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Tbill,
    Gsec,
    /// State development loan.
    Sdl,
}

/// One benchmark yield (`bonds yields --json`; field order = key order).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct YieldRow {
    pub date: Option<NaiveDate>,
    /// CCIL's bucket (`91D`, `9Y-10Y`, SDL `10Y`), or the maturity year for
    /// RBI's benchmark list.
    pub tenor: String,
    pub kind: Kind,
    /// `6.94% GS 2036`, `91 DTB (14/01/2027)`.
    pub security: String,
    /// Yield to maturity in percent; None when the page shows none.
    pub ytm: Option<f64>,
    pub source: &'static str,
}

impl crate::output::CsvRow for YieldRow {
    const COLUMNS: &'static [&'static str] =
        &["date", "tenor", "kind", "security", "ytm", "source"];
}

/// Rows of CCIL's tenor-wise table: Date | Tenor Bucket | Security | YTM (%).
pub fn parse_ccil(html: &str) -> Vec<YieldRow> {
    let Some(at) = html.find("id=\"dtTable\"") else {
        return Vec::new();
    };
    table_rows(&html[at..])
        .into_iter()
        .filter(|r| r.len() == 4)
        .filter_map(|r| {
            let date = NaiveDate::parse_from_str(r[0].get(..10)?, "%Y-%m-%d").ok()?;
            let tenor = r[1].clone();
            let kind = if tenor.ends_with('D') {
                Kind::Tbill
            } else if r[2].contains("SGS") || !tenor.contains('-') {
                Kind::Sdl
            } else {
                Kind::Gsec
            };
            Some(YieldRow {
                date: Some(date),
                tenor,
                kind,
                security: r[2].clone(),
                ytm: number(&r[3]),
                source: "CCIL",
            })
        })
        .collect()
}

/// The "Government Securities Market" block of the RBI home page: five
/// benchmark G-secs and the last T-bill auction cut-offs (`-` when none).
pub fn parse_rbi_home(html: &str) -> Vec<YieldRow> {
    let Some(at) = html.find("Government Securities Market") else {
        return Vec::new();
    };
    let block = &html[at..];
    let end = block.find("</table>").unwrap_or(block.len());
    let date = rbi_as_on(&block[end..]);
    table_rows(&block[..end])
        .into_iter()
        .filter(|r| r.len() == 2)
        .filter_map(|r| {
            let name = r[0].clone();
            let (tenor, kind) = if let Some(days) = name.strip_suffix(" day T-bills") {
                (format!("{days}D"), Kind::Tbill)
            } else {
                let year = name.rsplit(' ').next()?.to_string();
                year.parse::<i32>().ok()?;
                (year, Kind::Gsec)
            };
            let value = r[1].trim_start_matches(':').trim();
            let ytm = value
                .split('%')
                .next()
                .and_then(|v| v.trim().parse::<f64>().ok());
            Some(YieldRow {
                date,
                tenor,
                kind,
                security: name,
                ytm,
                source: "RBI",
            })
        })
        .collect()
}

/// "as on October 22, 2026" after the RBI table (comments already removed
/// by the caller's text; strip them again to be safe).
fn rbi_as_on(after_table: &str) -> Option<NaiveDate> {
    let clean = text(&strip_comments(after_table));
    let at = clean.find("as on ")?;
    let words: Vec<&str> = clean[at + 6..].split_whitespace().take(3).collect();
    NaiveDate::parse_from_str(&words.join(" "), "%B %d, %Y").ok()
}

/// The India 10-year benchmark: CCIL's `9Y-10Y` bucket, or the RBI
/// benchmark maturing closest to ten years after its date.
pub fn ten_year(rows: &[YieldRow]) -> Option<&YieldRow> {
    if let Some(row) = rows.iter().find(|r| r.tenor == "9Y-10Y" && r.ytm.is_some()) {
        return Some(row);
    }
    rows.iter()
        .filter(|r| r.kind == Kind::Gsec && r.source == "RBI" && r.ytm.is_some())
        .filter_map(|r| {
            let year: i32 = r.tenor.parse().ok()?;
            let target = r.date.map_or(year, |d| d.year() + 10);
            Some(((year - target).abs(), r))
        })
        .min_by_key(|(gap, _)| *gap)
        .filter(|(gap, _)| *gap <= 2)
        .map(|(_, r)| r)
}

/// The 364-day T-bill yield from the cached CCIL page, in percent, with no
/// request: `None` when the page was never fetched or has no such row.
pub fn cached_tbill_364() -> Option<f64> {
    let page = pagecache::read(
        "ccil_tenorwise.html",
        Duration::from_secs(14 * 86_400),
        |t| !parse_ccil(t).is_empty(),
    )?;
    parse_ccil(&page.text)
        .into_iter()
        .find(|r| r.kind == Kind::Tbill && r.tenor.starts_with("364"))
        .and_then(|r| r.ytm)
        .filter(|y| (0.0..=50.0).contains(y))
}

/// Benchmark yields and where they came from.
#[derive(Debug, Clone)]
pub struct YieldSet {
    pub rows: Vec<YieldRow>,
    pub url: &'static str,
    pub stale: bool,
}

/// Yields from CCIL, falling back to the RBI home page. Errors only when
/// neither page (nor a cached copy) has any.
pub async fn yields(timeout: Duration) -> Result<YieldSet> {
    let client = pagecache::client(timeout)?;
    let ccil = pagecache::fetch(
        &client,
        "ccil_tenorwise.html",
        CCIL_URL,
        "CCIL",
        YIELDS_MAX_AGE,
        |t| !parse_ccil(t).is_empty(),
    )
    .await;
    let ccil_err = match ccil {
        Ok(page) => {
            let rows = parse_ccil(&page.text);
            if !rows.is_empty() {
                return Ok(YieldSet {
                    rows,
                    url: CCIL_URL,
                    stale: page.stale,
                });
            }
            anyhow::anyhow!("CCIL's yield table had no rows")
        }
        Err(e) => e,
    };
    let rbi = pagecache::fetch(
        &client,
        "rbi_home.html",
        RBI_HOME_URL,
        "RBI",
        YIELDS_MAX_AGE,
        |t| !parse_rbi_home(t).is_empty(),
    )
    .await;
    match rbi {
        Ok(page) => {
            let rows = parse_rbi_home(&page.text);
            if rows.is_empty() {
                bail!("{ccil_err:#}; the RBI page had no G-sec yields either");
            }
            Ok(YieldSet {
                rows,
                url: RBI_HOME_URL,
                stale: page.stale,
            })
        }
        Err(e) => bail!("{ccil_err:#}; RBI: {e:#}"),
    }
}

// ── Outstanding securities ──

/// One Government of India security from RBI's outstanding list.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Security {
    pub isin: String,
    /// RBI's nomenclature: `7.18% GS 2033`, `FRB 2033`, `7.10% GOI SGrB 2028`.
    pub name: String,
    /// Fixed coupon in percent; None for floating-rate bonds.
    pub coupon: Option<f64>,
    pub issue_date: NaiveDate,
    pub maturity: NaiveDate,
    /// Outstanding stock in ₹ crore.
    pub outstanding_cr: Option<f64>,
}

impl crate::output::CsvRow for Security {
    const COLUMNS: &'static [&'static str] = &[
        "isin",
        "name",
        "coupon",
        "issue_date",
        "maturity",
        "outstanding_cr",
    ];
}

impl Security {
    /// The NSE symbol style for a plain G-sec (`7.18% GS 2033` → `718GS2033`,
    /// `7.30% GS 2053` → `73GS2053`); None for FRBs and green bonds.
    pub fn nse_symbol(&self) -> Option<String> {
        let coupon = self.coupon?;
        if !self.name.contains("% GS ") {
            return None;
        }
        // Decimal point and trailing zeros of the fraction dropped:
        // 7.30 → 73, 7.18 → 718, 10.00 → 10.
        let text = format!("{coupon:.2}");
        let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
        Some(format!(
            "{}{}GS{}",
            int,
            frac.trim_end_matches('0'),
            self.maturity.year()
        ))
    }
}

/// Rows of RBI's "List of Government of India Securities outstanding":
/// Sl. No. | ISIN | Nomenclature | Date of issue | Date of maturity | Stock.
pub fn parse_rbi_list(html: &str) -> Vec<Security> {
    let date = |s: &str| NaiveDate::parse_from_str(s.trim(), "%d-%b-%Y").ok();
    table_rows(html)
        .into_iter()
        .filter(|r| r.len() == 6 && r[1].starts_with("IN") && r[1].len() == 12)
        .filter_map(|r| {
            let name = r[2].clone();
            let coupon = name
                .split_once('%')
                .and_then(|(c, _)| c.trim().parse::<f64>().ok());
            Some(Security {
                isin: r[1].clone(),
                coupon,
                issue_date: date(&r[3])?,
                maturity: date(&r[4])?,
                outstanding_cr: number(&r[5]),
                name,
            })
        })
        .collect()
}

/// RBI's outstanding list, cached for a day.
pub async fn securities(timeout: Duration) -> Result<(Vec<Security>, bool)> {
    let client = pagecache::client(timeout)?;
    let page = pagecache::fetch(
        &client,
        "rbi_gsec_list.html",
        RBI_GSEC_LIST_URL,
        "RBI",
        LIST_MAX_AGE,
        |t| !parse_rbi_list(t).is_empty(),
    )
    .await?;
    let list = parse_rbi_list(&page.text);
    if list.is_empty() {
        bail!("RBI's list of outstanding G-secs had no rows ({RBI_GSEC_LIST_URL})");
    }
    Ok((list, page.stale))
}

/// Coupon (percent) and maturity year from a G-sec name as people type it:
/// NSE style `718GS2033` / `73GS2053` (decimal point and trailing zero
/// dropped), or `7.18% GS 2033`, `7.18 GS 2033`.
pub fn parse_name(query: &str) -> Option<(f64, i32)> {
    let s: String = query
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '%')
        .collect::<String>()
        .to_ascii_uppercase();
    let (coupon, rest) = s.split_once("GS")?;
    let year: i32 = rest.get(..4)?.parse().ok()?;
    let coupon = if coupon.contains('.') {
        coupon.parse::<f64>().ok()?
    } else {
        if coupon.is_empty() || !coupon.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        // 10.18% is "1018" and 10% is "10": a leading 1 followed by 0–2 is
        // 10–12%. Other coupons have one integer digit (7.3% is "73").
        let two = coupon.len() >= 2
            && coupon.starts_with('1')
            && matches!(coupon.as_bytes()[1], b'0'..=b'2');
        let int_digits = if two { 2 } else { 1 };
        let (int, frac) = coupon.split_at(int_digits.min(coupon.len()));
        format!("{int}.{frac}0").parse::<f64>().ok()?
    };
    (0.0..=20.0).contains(&coupon).then_some((coupon, year))
}

/// Find a security by ISIN, NSE symbol or name. Errors say why nothing (or
/// more than one thing) matched.
pub fn lookup<'a>(list: &'a [Security], query: &str) -> Result<&'a Security> {
    let q = query.trim();
    if let Some(s) = list.iter().find(|s| s.isin.eq_ignore_ascii_case(q)) {
        return Ok(s);
    }
    let Some((coupon, year)) = parse_name(q) else {
        bail!(
            "'{q}' is not a G-sec name: use an NSE symbol (718GS2033), a name \
             (\"7.18% GS 2033\") or an ISIN (IN0020230085)"
        );
    };
    let hits: Vec<&Security> = list
        .iter()
        .filter(|s| {
            s.maturity.year() == year && s.coupon.is_some_and(|c| (c - coupon).abs() < 0.001)
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => bail!(
            "No outstanding G-sec {coupon:.2}% maturing in {year} in RBI's list — check \
             `stockwise bonds list {year}`, or pass --coupon and --maturity"
        ),
        many => bail!(
            "{} securities match {coupon:.2}% {year}: {} — use the ISIN",
            many.len(),
            many.iter()
                .map(|s| format!("{} ({})", s.name, s.isin))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CCIL: &str = include_str!("fixtures/ccil_tenorwise.html");
    const RBI_HOME: &str = include_str!("fixtures/rbi_home_gsec.html");
    const RBI_LIST: &str = include_str!("fixtures/rbi_gsec_list.html");

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn html_rows_cells_and_entities() {
        let html = "<table><tr><th>A</th><td align=x> 1,01,908.561 </td></tr>\
                    <!-- <tr><td>hidden</td></tr> -->\
                    <tr><td>&nbsp;&#8377; Crore&amp;co</td><td></td></tr></table>";
        let rows = table_rows(html);
        assert_eq!(rows, [vec!["A", "1,01,908.561"], vec!["₹ Crore&co", ""]]);
        assert_eq!(number("1,01,908.561"), Some(101_908.561));
        assert_eq!(number("-"), None);
    }

    #[test]
    fn ccil_tenorwise_yields() {
        let rows = parse_ccil(CCIL);
        assert_eq!(rows.len(), 11);
        let first = &rows[0];
        assert_eq!(first.date, Some(date(2026, 10, 23)));
        assert_eq!(first.tenor, "91D");
        assert_eq!(first.kind, Kind::Tbill);
        assert_eq!(first.security, "91 DTB (14/01/2027)");
        assert_eq!(first.ytm, Some(5.7281));
        let ten = ten_year(&rows).unwrap();
        assert_eq!(ten.security, "6.94% GS 2036");
        assert_eq!(ten.ytm, Some(7.4713));
        assert_eq!(ten.kind, Kind::Gsec);
        assert!(
            rows.iter()
                .any(|r| r.kind == Kind::Sdl && r.tenor == "10Y" && r.security.contains("SGS"))
        );
        assert!(parse_ccil("<html>maintenance</html>").is_empty());
    }

    #[test]
    fn rbi_home_benchmarks_and_missing_cutoffs() {
        let rows = parse_rbi_home(RBI_HOME);
        assert_eq!(rows.len(), 8);
        // The stale date in the HTML comment is ignored.
        assert!(rows.iter().all(|r| r.date == Some(date(2026, 10, 22))));
        let ten = ten_year(&rows).unwrap();
        assert_eq!(ten.security, "6.94% GS 2036");
        assert_eq!(ten.ytm, Some(7.46));
        let tbill = rows.iter().find(|r| r.tenor == "91D").unwrap();
        assert_eq!(tbill.kind, Kind::Tbill);
        // `-%*` is no cut-off, not 0%.
        assert_eq!(tbill.ytm, None);
    }

    #[test]
    fn rbi_outstanding_list() {
        let list = parse_rbi_list(RBI_LIST);
        assert!(list.len() >= 6, "{}", list.len());
        let s = list.iter().find(|s| s.isin == "IN0020230085").unwrap();
        assert_eq!(s.name, "7.18% GS 2033");
        assert_eq!(s.coupon, Some(7.18));
        assert_eq!(s.maturity, date(2033, 8, 14));
        assert_eq!(s.issue_date, date(2023, 8, 14));
        assert_eq!(s.outstanding_cr, Some(201_000.0));
        assert_eq!(s.nse_symbol().as_deref(), Some("718GS2033"));
        // Round coupons round-trip through the NSE spelling.
        for coupon in [7.3, 10.0, 8.0, 10.18] {
            let mut t = s.clone();
            t.coupon = Some(coupon);
            t.name = format!("{coupon:.2}% GS 2033");
            let symbol = t.nse_symbol().unwrap();
            assert_eq!(parse_name(&symbol), Some((coupon, 2033)), "{symbol}");
        }
        let frb = list.iter().find(|s| s.name.starts_with("FRB")).unwrap();
        assert_eq!(frb.coupon, None);
        assert_eq!(frb.nse_symbol(), None);
        // Indian digit grouping.
        let big = list.iter().find(|s| s.isin == "IN0020170026").unwrap();
        assert_eq!(big.outstanding_cr, Some(101_908.561));
    }

    #[test]
    fn names_as_people_type_them() {
        assert_eq!(parse_name("718GS2033"), Some((7.18, 2033)));
        assert_eq!(parse_name("73GS2053"), Some((7.3, 2053)));
        assert_eq!(parse_name("74GS2035"), Some((7.4, 2035)));
        assert_eq!(parse_name("1018GS2026"), Some((10.18, 2026)));
        assert_eq!(parse_name("10GS2030"), Some((10.0, 2030)));
        assert_eq!(parse_name("8GS2030"), Some((8.0, 2030)));
        assert_eq!(parse_name("668GS2035A"), Some((6.68, 2035)));
        assert_eq!(parse_name("7.18% GS 2033"), Some((7.18, 2033)));
        assert_eq!(parse_name("6.94 gs 2036"), Some((6.94, 2036)));
        assert_eq!(parse_name("RELIANCE"), None);
        assert_eq!(parse_name("GS2033"), None);
    }

    #[test]
    fn lookup_by_isin_symbol_or_name() {
        let list = parse_rbi_list(RBI_LIST);
        assert_eq!(lookup(&list, "718GS2033").unwrap().isin, "IN0020230085");
        assert_eq!(lookup(&list, "6.94% GS 2036").unwrap().isin, "IN0020260025");
        assert_eq!(lookup(&list, "in0020230085").unwrap().name, "7.18% GS 2033");
        let err = lookup(&list, "999GS2040").unwrap_err().to_string();
        assert!(err.contains("bonds list 2040"), "{err}");
        assert!(lookup(&list, "TCS").is_err());
    }
}
