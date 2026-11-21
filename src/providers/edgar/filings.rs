//! Recent SEC filings of a US company (8-K, 10-Q, 10-K, 6-K, 20-F, SC 13D),
//! from EDGAR's submissions feed, as headlines for the holding monitor and
//! the news feed.

use super::{TICKERS_TTL, TICKERS_URL, explain, find_cik, ticker, user_agent};
use crate::api::YahooClient;
use crate::research::Headline;
use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

const SUBMISSIONS_URL: &str = "https://data.sec.gov/submissions/CIK";
const SUBMISSIONS_TTL: Duration = Duration::from_secs(1800);

/// Forms worth a headline. Form 4 (insider trades) is read by `insider`.
const FORMS: &[&str] = &[
    "8-K", "8-K/A", "10-Q", "10-K", "6-K", "20-F", "SC 13D", "SC 13D/A", "SC 13G",
];

/// What an 8-K item number means.
fn item_title(item: &str) -> Option<&'static str> {
    Some(match item.trim() {
        "1.01" => "Entry into a material agreement",
        "1.02" => "Termination of a material agreement",
        "1.03" => "Bankruptcy or receivership",
        "2.01" => "Completed acquisition or disposition",
        "2.02" => "Results of operations",
        "2.03" => "New financial obligation",
        "2.05" => "Exit or restructuring costs",
        "2.06" => "Material impairment",
        "3.01" => "Delisting notice or listing-rule failure",
        "3.02" => "Unregistered sale of shares",
        "4.01" => "Change of auditor",
        "4.02" => "Earlier financial statements no longer reliable",
        "5.01" => "Change in control",
        "5.02" => "Director or officer change",
        "5.03" => "Bylaw or fiscal-year change",
        "5.07" => "Shareholder vote results",
        "7.01" => "Regulation FD disclosure",
        "8.01" => "Other events",
        _ => return None,
    })
}

/// Filings in a submissions document filed on or after `since` (unix
/// seconds), newest first.
pub fn parse_submissions(json: &Value, cik: u64, since: i64) -> Vec<Headline> {
    let recent = &json["filings"]["recent"];
    let col = |k: &str| recent[k].as_array().cloned().unwrap_or_default();
    let (forms, dates, accessions, docs, items, descs) = (
        col("form"),
        col("filingDate"),
        col("accessionNumber"),
        col("primaryDocument"),
        col("items"),
        col("primaryDocDescription"),
    );
    let mut out = Vec::new();
    for (i, form) in forms.iter().enumerate() {
        let Some(form) = form.as_str() else { continue };
        if !FORMS.contains(&form) {
            continue;
        }
        let Some(date) = dates
            .get(i)
            .and_then(Value::as_str)
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        // Filed by the end of the US business day.
        let ts = date
            .and_hms_opt(21, 0, 0)
            .map_or(0, |d| d.and_utc().timestamp());
        if ts < since {
            continue;
        }
        let what: Vec<&str> = items
            .get(i)
            .and_then(Value::as_str)
            .unwrap_or("")
            .split(',')
            .filter_map(item_title)
            .collect();
        let desc = descs
            .get(i)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty() && !d.eq_ignore_ascii_case(form));
        let title = match (what.is_empty(), desc) {
            (false, _) => format!("{form}: {}", what.join("; ")),
            (true, Some(d)) => format!("{form}: {d}"),
            (true, None) => match form {
                "10-Q" => "10-Q quarterly report".to_string(),
                "10-K" => "10-K annual report".to_string(),
                f => format!("{f} filing"),
            },
        };
        let link = match (
            accessions.get(i).and_then(Value::as_str),
            docs.get(i).and_then(Value::as_str),
        ) {
            (Some(a), Some(d)) if !d.is_empty() => Some(format!(
                "https://www.sec.gov/Archives/edgar/data/{cik}/{}/{d}",
                a.replace('-', "")
            )),
            _ => None,
        };
        out.push(Headline {
            ts,
            title,
            summary: None,
            publisher: "SEC EDGAR".into(),
            link,
            official: true,
            category: Some(form.to_string()),
        });
    }
    out.sort_by_key(|h| std::cmp::Reverse(h.ts));
    out
}

/// Recent filings of a US-listed `symbol` over the last `days` days.
/// `Ok(empty)` for symbols EDGAR doesn't cover.
pub async fn recent(client: &YahooClient, symbol: &str, days: i64) -> Result<Vec<Headline>> {
    let Some(ticker) = ticker(symbol) else {
        return Ok(Vec::new());
    };
    let ua = user_agent();
    let headers = [("User-Agent", ua.as_str()), ("Accept", "application/json")];
    let list = client
        .fetch_with(TICKERS_URL, TICKERS_TTL, &headers)
        .await
        .map_err(|e| explain(e, "EDGAR ticker list"))?;
    let Some(cik) = find_cik(&list, &ticker) else {
        return Ok(Vec::new());
    };
    let url = format!("{SUBMISSIONS_URL}{cik:010}.json");
    let body = client
        .fetch_with(&url, SUBMISSIONS_TTL, &headers)
        .await
        .map_err(|e| explain(e, "EDGAR filings"))?;
    let json: Value = serde_json::from_str(&body)?;
    let since = chrono::Utc::now().timestamp() - days.max(1) * 86_400;
    Ok(parse_submissions(&json, cik, since))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submissions_become_dated_linked_headlines() {
        let json = serde_json::json!({"filings": {"recent": {
            "form": ["4", "8-K", "10-Q", "8-K"],
            "filingDate": ["2026-11-20", "2026-11-19", "2026-10-30", "2025-01-02"],
            "accessionNumber": ["0001-26-1", "0000320193-26-000101", "0000320193-26-000090", "x"],
            "primaryDocument": ["f4.xml", "aapl-8k.htm", "aapl-10q.htm", "old.htm"],
            "items": ["", "2.02,9.01", "", "8.01"],
            "primaryDocDescription": ["4", "8-K", "10-Q", "8-K"]
        }}});
        let since = chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let h = parse_submissions(&json, 320193, since);
        assert_eq!(h.len(), 2, "{h:?}");
        assert_eq!(h[0].title, "8-K: Results of operations");
        assert_eq!(
            h[0].link.as_deref(),
            Some("https://www.sec.gov/Archives/edgar/data/320193/000032019326000101/aapl-8k.htm")
        );
        assert!(h[0].official);
        assert_eq!(h[1].title, "10-Q quarterly report");
    }
}
