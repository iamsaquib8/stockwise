//! Insider trades for the `insider` command, from the regulator's own
//! filings where there is one:
//!
//! - NSE's PIT disclosures (`/api/corporates-pit`) for Indian stocks: the
//!   trades promoters, directors and key managerial staff report under
//!   SEBI's Prohibition of Insider Trading regulations;
//! - SEC EDGAR Form 4 filings for US stocks: the latest filings listed in
//!   the company's `submissions` document, each read from its XML;
//! - Yahoo Finance `insiderTransactions` as the fallback, labelled as such.
//!
//! NSE and SEC refuse scripted requests at times (HTTP 403): the command
//! says so and falls back to Yahoo. The parsers are pure and tested on
//! fixtures.

use crate::api::YahooClient;
use crate::providers::edgar;
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use serde_json::Value;
use std::time::Duration;

/// Where a trade was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    NsePit,
    SecForm4,
    Yahoo,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::NsePit => "NSE PIT disclosures (SEBI insider-trading filings)",
            Source::SecForm4 => "SEC EDGAR Form 4 filings",
            Source::Yahoo => {
                "Yahoo Finance insiderTransactions (fallback, not the regulator's filing)"
            }
        }
    }
}

/// Which way the trade moved the insider's holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Acquired,
    Disposed,
    Other,
}

/// One reported trade.
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub date: NaiveDate,
    pub name: String,
    /// Promoter, director, officer title, ...
    pub role: String,
    /// What the filing calls it: "Buy", "Sell (open market)", "Award", ...
    pub action: String,
    pub side: Side,
    /// Whether it was an ordinary trade on the exchange (not a grant, gift,
    /// option exercise, pledge or off-market transfer).
    pub open_market: bool,
    pub shares: Option<f64>,
    pub price: Option<f64>,
    pub value: Option<f64>,
    pub source: Source,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "-" && !s.eq_ignore_ascii_case("nil"))
}

/// NSE numbers come as `"1,234.5"`, `"-"` or plain numbers.
fn nse_num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::String(s) => s.replace(',', "").trim().parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}

/// Price per share from a value and a share count, when both are positive.
fn per_share(value: Option<f64>, shares: Option<f64>) -> Option<f64> {
    match (value, shares) {
        (Some(v), Some(s)) if v > 0.0 && s > 0.0 => Some(v / s),
        _ => None,
    }
}

/// Newest first, then by name.
fn sort_newest_first(trades: &mut [Trade]) {
    trades.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.name.cmp(&b.name)));
}

// ── NSE PIT ──

/// Every trade in an NSE `corporates-pit` answer, newest first. The trade
/// date is the end of the acquisition period (`acqtoDt`), else its start,
/// else the day NSE published it.
pub fn parse_nse_pit(v: &Value) -> Vec<Trade> {
    let Some(rows) = v.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<Trade> = rows
        .iter()
        .filter_map(|t| {
            let date = ["acqtoDt", "acqfromDt", "date", "intimDt"]
                .iter()
                .find_map(|k| text(t, k).and_then(|d| crate::providers::nse::parse_date(&d)))?;
            let kind = text(t, "tdpTransactionType").unwrap_or_default();
            let mode = text(t, "acqMode").unwrap_or_default();
            let kl = kind.to_lowercase();
            let side = if kl.contains("buy") || kl.contains("acq") {
                Side::Acquired
            } else if kl.contains("sell") || kl.contains("sale") || kl.contains("disp") {
                Side::Disposed
            } else {
                Side::Other
            };
            let ml = mode.to_lowercase();
            let open_market = ml.contains("market") && !ml.contains("off market");
            let action = match (kind.is_empty(), mode.is_empty()) {
                (false, false) => format!("{kind} ({mode})"),
                (false, true) => kind,
                (true, false) => mode,
                (true, true) => "—".to_string(),
            };
            let shares = nse_num(t, "secAcq");
            let value = nse_num(t, "secVal");
            Some(Trade {
                date,
                name: text(t, "acqName").unwrap_or_else(|| "—".into()),
                role: text(t, "personCategory").unwrap_or_default(),
                action,
                side,
                open_market,
                shares,
                price: per_share(value, shares),
                value,
                source: Source::NsePit,
            })
        })
        .collect();
    sort_newest_first(&mut out);
    out
}

/// NSE's PIT disclosures for an Indian stock (`TCS.NS`, `TCS.BO` or
/// `TCS`).
pub async fn nse_trades(symbol: &str) -> Result<Vec<Trade>> {
    let base = symbol
        .trim()
        .to_ascii_uppercase()
        .trim_end_matches(".NS")
        .trim_end_matches(".BO")
        .to_string();
    let session = crate::providers::nse::shared_session();
    let v = session
        .get_json(
            "/api/corporates-pit",
            &[("index", "equities"), ("symbol", &base)],
            Duration::from_secs(3600),
        )
        .await
        .map_err(|e| anyhow!("NSE insider disclosures unavailable: {e}"))?;
    Ok(parse_nse_pit(&v))
}

// ── SEC Form 4 ──

/// A Form 4 filing listed in a company's EDGAR `submissions` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form4Ref {
    pub accession: String,
    /// The XML file inside the filing (the listed document without its
    /// `xslF345X05/` style-sheet folder).
    pub document: String,
    pub filed: NaiveDate,
}

impl Form4Ref {
    /// Where the filing's XML lives.
    pub fn url(&self, cik: u64) -> String {
        format!(
            "https://www.sec.gov/Archives/edgar/data/{}/{}/{}",
            cik,
            self.accession.replace('-', ""),
            self.document
        )
    }
}

/// The newest `limit` Form 4 filings in a `submissions` document
/// (`filings.recent`, whose fields are parallel arrays), newest first.
pub fn form4_filings(v: &Value, limit: usize) -> Vec<Form4Ref> {
    let Some(recent) = v.pointer("/filings/recent") else {
        return Vec::new();
    };
    let col = |k: &str| -> Vec<&str> {
        recent
            .get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().unwrap_or("")).collect())
            .unwrap_or_default()
    };
    let (forms, accs, docs, dates) = (
        col("form"),
        col("accessionNumber"),
        col("primaryDocument"),
        col("filingDate"),
    );
    let mut out: Vec<Form4Ref> = forms
        .iter()
        .enumerate()
        .filter(|(_, f)| **f == "4")
        .filter_map(|(i, _)| {
            let doc = docs.get(i)?;
            let document = doc.rsplit('/').next()?.to_string();
            if !document.to_ascii_lowercase().ends_with(".xml") {
                return None;
            }
            Some(Form4Ref {
                accession: accs.get(i)?.to_string(),
                document,
                filed: NaiveDate::parse_from_str(dates.get(i)?, "%Y-%m-%d").ok()?,
            })
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.filed));
    out.truncate(limit);
    out
}

/// The text inside the first `<tag>` (with or without attributes) in `s`.
fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let mut from = 0;
    loop {
        let at = from + s[from..].find(&open)?;
        let rest = &s[at + open.len()..];
        // `<value>` must not match `<valueX>`.
        match rest.chars().next() {
            Some('>') | Some(' ') | Some('\n') | Some('\t') | Some('\r') => {}
            Some('/') => return Some(""),
            _ => {
                from = at + open.len();
                continue;
            }
        }
        let body_start = at + open.len() + rest.find('>')? + 1;
        if s[..body_start].ends_with("/>") {
            return Some("");
        }
        let close = format!("</{name}>");
        let end = body_start + s[body_start..].find(&close)?;
        return Some(&s[body_start..end]);
    }
}

/// Every `<tag>…</tag>` block in `s`.
fn blocks<'a>(s: &'a str, name: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(a) = rest.find(&open) {
        let body = &rest[a + open.len()..];
        let Some(b) = body.find(&close) else { break };
        out.push(&body[..b]);
        rest = &body[b + close.len()..];
    }
    out
}

fn unescape(s: &str) -> String {
    s.trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A Form 4 field that wraps its content in `<value>` (most do), or the
/// field's own text.
fn field(s: &str, name: &str) -> Option<String> {
    let inner = tag(s, name)?;
    let v = tag(inner, "value").unwrap_or(inner);
    let v = unescape(v);
    (!v.is_empty() && !v.starts_with('<')).then_some(v)
}

fn flag(s: &str, name: &str) -> bool {
    field(s, name).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// What a Form 4 transaction code means.
fn form4_action(code: &str, acquired: bool) -> (String, bool) {
    let (label, open_market) = match code {
        "P" => ("Buy (open market)", true),
        "S" => ("Sell (open market)", true),
        "A" => ("Award / grant", false),
        "M" => ("Option exercise", false),
        "X" => ("Option exercise", false),
        "F" => ("Tax withholding", false),
        "G" => ("Gift", false),
        "D" => ("Disposed to issuer", false),
        "C" => ("Conversion", false),
        "J" => ("Other", false),
        _ => ("", false),
    };
    if label.is_empty() {
        let dir = if acquired { "acquired" } else { "disposed" };
        (format!("Code {code} ({dir})"), false)
    } else {
        (label.to_string(), open_market)
    }
}

/// The non-derivative transactions (shares, not options) of one Form 4
/// XML document.
pub fn parse_form4(xml: &str) -> Vec<Trade> {
    let owners = blocks(xml, "reportingOwner");
    let name = owners
        .iter()
        .filter_map(|o| field(o, "rptOwnerName"))
        .collect::<Vec<_>>()
        .join(" / ");
    let role = owners
        .first()
        .map(|o| {
            let mut roles = Vec::new();
            if flag(o, "isDirector") {
                roles.push("Director".to_string());
            }
            if flag(o, "isOfficer") {
                roles.push(field(o, "officerTitle").unwrap_or_else(|| "Officer".into()));
            }
            if flag(o, "isTenPercentOwner") {
                roles.push("10% owner".to_string());
            }
            if flag(o, "isOther") {
                roles.push(field(o, "otherText").unwrap_or_else(|| "Other".into()));
            }
            roles.join(", ")
        })
        .unwrap_or_default();
    let mut out: Vec<Trade> = blocks(xml, "nonDerivativeTransaction")
        .into_iter()
        .filter_map(|t| {
            let date = field(t, "transactionDate")
                .and_then(|d| NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok())?;
            let code = tag(t, "transactionCoding")
                .and_then(|c| field(c, "transactionCode"))
                .unwrap_or_default();
            let acquired = field(t, "transactionAcquiredDisposedCode").as_deref() == Some("A");
            let (action, open_market) = form4_action(&code, acquired);
            let shares = field(t, "transactionShares").and_then(|v| v.parse::<f64>().ok());
            let price = field(t, "transactionPricePerShare")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|p| *p > 0.0);
            let value = match (shares, price) {
                (Some(s), Some(p)) => Some(s * p),
                _ => None,
            };
            Some(Trade {
                date,
                name: if name.is_empty() {
                    "—".into()
                } else {
                    name.clone()
                },
                role: role.clone(),
                action,
                side: if acquired {
                    Side::Acquired
                } else {
                    Side::Disposed
                },
                open_market,
                shares,
                price,
                value,
                source: Source::SecForm4,
            })
        })
        .collect();
    sort_newest_first(&mut out);
    out
}

/// Form 4 filings read per run.
const FORM4_FILINGS: usize = 12;

/// The share trades in a US stock's latest Form 4 filings. `Ok(None)` for
/// a symbol SEC doesn't list.
pub async fn sec_trades(client: &YahooClient, symbol: &str) -> Result<Option<Vec<Trade>>> {
    let Some(cik) = edgar::cik(client, symbol).await? else {
        return Ok(None);
    };
    let url = format!("https://data.sec.gov/submissions/CIK{cik:010}.json");
    let body = edgar::get(
        client,
        &url,
        Duration::from_secs(6 * 3600),
        "EDGAR filings list",
    )
    .await?;
    let v: Value =
        serde_json::from_str(&body).map_err(|_| anyhow!("EDGAR filings list is not JSON"))?;
    let mut trades = Vec::new();
    for f in form4_filings(&v, FORM4_FILINGS) {
        // A filing that fails to load is skipped; the rest still count.
        if let Ok(xml) = edgar::get(
            client,
            &f.url(cik),
            Duration::from_secs(24 * 3600),
            "Form 4",
        )
        .await
        {
            trades.extend(parse_form4(&xml));
        }
    }
    sort_newest_first(&mut trades);
    Ok(Some(trades))
}

// ── Yahoo ──

/// Every transaction in a Yahoo `insiderTransactions` module (the
/// quoteSummary result object), newest first.
pub fn parse_yahoo(summary: &Value) -> Vec<Trade> {
    let Some(rows) = summary
        .pointer("/insiderTransactions/transactions")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut out: Vec<Trade> = rows
        .iter()
        .filter_map(|t| {
            let ts = crate::research::raw(t, "startDate")? as i64;
            let date = chrono::DateTime::from_timestamp(ts, 0)?.date_naive();
            let txt = text(t, "transactionText").unwrap_or_default();
            let tl = txt.to_lowercase();
            let (side, open_market) = if tl.contains("purchase") || tl.starts_with("buy") {
                (Side::Acquired, true)
            } else if tl.contains("sale") {
                (Side::Disposed, true)
            } else {
                (Side::Other, false)
            };
            let shares = crate::research::raw(t, "shares");
            let value = crate::research::raw(t, "value").filter(|v| *v > 0.0);
            Some(Trade {
                date,
                name: text(t, "filerName").unwrap_or_else(|| "—".into()),
                role: text(t, "filerRelation").unwrap_or_default(),
                action: if txt.is_empty() { "—".into() } else { txt },
                side,
                open_market,
                shares,
                price: per_share(value, shares),
                value,
                source: Source::Yahoo,
            })
        })
        .collect();
    sort_newest_first(&mut out);
    out
}

pub async fn yahoo_trades(client: &YahooClient, symbol: &str) -> Result<Vec<Trade>> {
    let v = client
        .quote_summary(symbol, &["insiderTransactions"])
        .await?;
    Ok(parse_yahoo(&v))
}

/// Open-market buys and sells: (count, value) each.
pub fn open_market_totals(trades: &[Trade]) -> ((usize, f64), (usize, f64)) {
    let mut buys = (0, 0.0);
    let mut sells = (0, 0.0);
    for t in trades.iter().filter(|t| t.open_market) {
        let slot = match t.side {
            Side::Acquired => &mut buys,
            Side::Disposed => &mut sells,
            Side::Other => continue,
        };
        slot.0 += 1;
        slot.1 += t.value.unwrap_or(0.0);
    }
    (buys, sells)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn nse_pit_fixture() {
        let v: Value =
            serde_json::from_str(include_str!("fixtures/nse_corporates_pit.json")).unwrap();
        let t = parse_nse_pit(&v);
        assert_eq!(t.len(), 4, "{t:#?}");
        // Newest first, by the trade date (acqtoDt), not the filing date.
        assert_eq!(t[0].date, d("2026-11-14"));
        assert_eq!(t[0].name, "Asha Promoter Holdings LLP");
        assert_eq!(t[0].role, "Promoter Group");
        assert_eq!(t[0].side, Side::Acquired);
        assert!(t[0].open_market);
        assert_eq!(t[0].shares, Some(25_000.0));
        assert_eq!(t[0].value, Some(3_750_000.0));
        assert_eq!(t[0].price, Some(150.0));
        assert_eq!(t[0].action, "Buy (Market Purchase)");
        // A pledge is listed but is neither a buy nor a sell.
        let pledge = t.iter().find(|x| x.action.contains("Pledge")).unwrap();
        assert_eq!(pledge.side, Side::Other);
        assert!(!pledge.open_market);
        // An off-market transfer is not an open-market trade.
        let off = t.iter().find(|x| x.action.contains("Off Market")).unwrap();
        assert_eq!(off.side, Side::Disposed);
        assert!(!off.open_market);
        let ((nb, vb), (ns, vs)) = open_market_totals(&t);
        assert_eq!((nb, ns), (1, 1));
        assert_eq!(vb, 3_750_000.0);
        assert_eq!(vs, 1_200_000.0);
        assert!(t.iter().all(|x| x.source == Source::NsePit));
    }

    #[test]
    fn nse_pit_without_data_is_empty() {
        assert!(parse_nse_pit(&serde_json::json!({})).is_empty());
        assert!(parse_nse_pit(&serde_json::json!({"data": []})).is_empty());
    }

    #[test]
    fn edgar_submissions_fixture_lists_form4_xml() {
        let v: Value =
            serde_json::from_str(include_str!("fixtures/edgar_submissions.json")).unwrap();
        let f = form4_filings(&v, 10);
        assert_eq!(f.len(), 2, "{f:#?}");
        assert_eq!(f[0].filed, d("2026-11-18"));
        assert_eq!(f[0].document, "form4.xml");
        assert_eq!(
            f[0].url(123456),
            "https://www.sec.gov/Archives/edgar/data/123456/000012345626000123/form4.xml"
        );
        assert_eq!(form4_filings(&v, 1).len(), 1);
    }

    #[test]
    fn edgar_form4_fixture() {
        let t = parse_form4(include_str!("fixtures/edgar_form4.xml"));
        assert_eq!(t.len(), 3, "{t:#?}");
        assert!(
            t.iter()
                .all(|x| x.name == "DOE JANE" && x.source == Source::SecForm4)
        );
        assert_eq!(t[0].role, "Director, Chief Financial Officer");
        let sale = t.iter().find(|x| x.action == "Sell (open market)").unwrap();
        assert_eq!(sale.date, d("2026-11-14"));
        assert_eq!(sale.side, Side::Disposed);
        assert!(sale.open_market);
        assert_eq!(sale.shares, Some(1_200.0));
        assert_eq!(sale.price, Some(210.5));
        assert_eq!(sale.value, Some(252_600.0));
        // A grant has no price and is not an open-market trade.
        let award = t.iter().find(|x| x.action == "Award / grant").unwrap();
        assert_eq!(award.side, Side::Acquired);
        assert_eq!(award.price, None);
        assert!(!award.open_market);
        let tax = t.iter().find(|x| x.action == "Tax withholding").unwrap();
        assert!(!tax.open_market);
        let ((nb, _), (ns, vs)) = open_market_totals(&t);
        assert_eq!((nb, ns), (0, 1));
        assert_eq!(vs, 252_600.0);
    }

    #[test]
    fn form4_tags_ignore_lookalikes_and_footnotes() {
        let xml = "<a><valueX>9</valueX><transactionPricePerShare><footnoteId id=\"F1\"/></transactionPricePerShare></a>";
        assert_eq!(field(xml, "transactionPricePerShare"), None);
        assert_eq!(tag("<value>1</value>", "value"), Some("1"));
        assert_eq!(
            tag("<valueX>2</valueX><value>3</value>", "value"),
            Some("3")
        );
        assert_eq!(unescape(" AT&amp;T "), "AT&T");
    }

    #[test]
    fn yahoo_insider_transactions() {
        let v = serde_json::json!({"insiderTransactions": {"transactions": [
            {"filerName": "ROE RICHARD", "filerRelation": "Officer", "transactionText": "Sale at price 100.00 per share.",
             "startDate": {"raw": 1_763_337_600_i64}, "shares": {"raw": 10.0}, "value": {"raw": 1000.0}},
            {"filerName": "ROE RICHARD", "filerRelation": "Officer", "transactionText": "Stock Award(Grant)",
             "startDate": {"raw": 1_763_424_000_i64}, "shares": {"raw": 5.0}}
        ]}});
        let t = parse_yahoo(&v);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].action, "Stock Award(Grant)");
        assert_eq!(t[0].side, Side::Other);
        assert_eq!(t[1].side, Side::Disposed);
        assert_eq!(t[1].price, Some(100.0));
        assert!(t.iter().all(|x| x.source == Source::Yahoo));
    }
}
