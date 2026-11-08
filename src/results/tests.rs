use super::*;
use crate::output;
use serde_json::json;
use std::sync::Mutex;

/// Parse a `YYYY-MM-DD` date.
fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// One Yahoo timeseries result for `kind`+`field`.
fn series(kind: &str, field: &str, rows: &[(String, f64)], ccy: &str) -> Value {
    let t = format!("{kind}{field}");
    let items: Vec<Value> = rows
        .iter()
        .map(
            |(date, v)| json!({"asOfDate": date, "currencyCode": ccy, "reportedValue": {"raw": v}}),
        )
        .collect();
    json!({"meta": {"type": [t.clone()]}, t: items})
}

const QUARTER_ENDS: [&str; 10] = [
    "2024-06-30",
    "2024-09-30",
    "2024-12-31",
    "2025-03-31",
    "2025-06-30",
    "2025-09-30",
    "2025-12-31",
    "2026-03-31",
    "2026-06-30",
    "2026-09-30",
];

/// Revenue of the i-th test quarter.
fn rev(i: usize) -> f64 {
    1000.0 + 50.0 * i as f64
}

/// A non-bank with ten quarters: revenue 1000, 1050, …, 1450.
fn nonbank_series() -> Vec<Value> {
    let rows = |f: &dyn Fn(usize) -> f64| -> Vec<(String, f64)> {
        QUARTER_ENDS
            .iter()
            .enumerate()
            .map(|(i, e)| (e.to_string(), f(i)))
            .collect()
    };
    vec![
        series("quarterly", "TotalRevenue", &rows(&rev), "INR"),
        series(
            "quarterly",
            "OperatingIncome",
            &rows(&|i| rev(i) * 0.2),
            "INR",
        ),
        series(
            "quarterly",
            "ReconciledDepreciation",
            &rows(&|_| 20.0),
            "INR",
        ),
        series("quarterly", "OtherIncomeExpense", &rows(&|_| 15.0), "INR"),
        series("quarterly", "InterestExpense", &rows(&|_| 5.0), "INR"),
        series("quarterly", "PretaxIncome", &rows(&|i| rev(i) * 0.2), "INR"),
        series(
            "quarterly",
            "TaxProvision",
            &rows(&|i| rev(i) * 0.05),
            "INR",
        ),
        series("quarterly", "NetIncome", &rows(&|i| rev(i) * 0.15), "INR"),
        series("quarterly", "DilutedEPS", &rows(&|i| rev(i) / 100.0), "INR"),
    ]
}

/// Ten parsed quarters of a non-bank.
fn nonbank() -> Vec<ResultPeriod> {
    parse_statements(&nonbank_series()).quarterly
}

#[test]
fn statements_parse_ten_quarters_with_the_statement_currency() {
    let st = parse_statements(&nonbank_series());
    assert_eq!(st.quarterly.len(), 10);
    assert_eq!(st.currency.as_deref(), Some("INR"));
    let last = st.quarterly.last().unwrap();
    assert_eq!(last.end, d("2026-09-30"));
    // Operating profit = operating income + depreciation.
    assert_eq!(last.operating_profit, Some(1450.0 * 0.2 + 20.0));
    assert_eq!(last.other_income, Some(15.0));
    assert_eq!(last.pbt, Some(1450.0 * 0.2));
}

#[test]
fn a_stub_period_with_only_balance_sheet_data_is_dropped() {
    let rows = vec![("2026-09-30".to_string(), 5.0)];
    let st = parse_statements(&[series("quarterly", "TotalAssets", &rows, "USD")]);
    assert!(st.quarterly.is_empty());
}

#[test]
fn growth_and_margin_changes_use_the_matching_quarters() {
    let q = nonbank();
    let views = quarter_views(&q, 8, true);
    assert_eq!(views.len(), 8);
    let last = views.last().unwrap();
    assert_eq!(last.quarter, "Q2 FY27");
    // 1450 against 1250 a year ago and 1400 last quarter.
    assert_eq!(last.yoy.revenue, Some(16.0));
    assert_eq!(last.qoq.revenue, Some(3.57));
    // OPM: (0.2 * rev + 20) / rev.
    let opm = |i: usize| (rev(i) * 0.2 + 20.0) / rev(i);
    assert_eq!(last.opm_pct, Some((opm(9) * 10000.0).round() / 100.0));
    assert_eq!(
        last.opm_bps_yoy,
        Some(((opm(9) - opm(5)) * 10000.0).round())
    );
    assert_eq!(last.tax_pct, Some(25.0));
    // The oldest quarter has no year-ago column to compare with.
    assert_eq!(views[0].yoy.revenue, None);
}

#[test]
fn ttm_sums_four_consecutive_quarters() {
    let q = nonbank();
    let t = ttm(&q).unwrap();
    assert_eq!(t.through, d("2026-09-30"));
    assert_eq!(t.revenue, Some(1300.0 + 1350.0 + 1400.0 + 1450.0));
    assert_eq!(t.eps, Some(13.0 + 13.5 + 14.0 + 14.5));
    assert_eq!(t.tax_pct, Some(25.0));
}

#[test]
fn ttm_needs_four_consecutive_quarters() {
    let mut q = nonbank();
    q.remove(8); // a missing quarter inside the last four
    assert_eq!(ttm(&q), None);
    assert_eq!(ttm(&nonbank()[..3]), None);
}

#[test]
fn growth_from_a_loss_has_no_percentage() {
    assert_eq!(growth(Some(10.0), Some(-5.0)), None);
    assert_eq!(growth(Some(10.0), Some(0.0)), None);
    assert_eq!(
        growth(Some(110.0), Some(100.0)).map(|g| (g * 100.0).round()),
        Some(10.0)
    );
}

#[test]
fn surprises_are_beat_in_line_or_miss() {
    assert_eq!(outcome(0.05), "beat");
    assert_eq!(outcome(0.02), "in-line");
    assert_eq!(outcome(-0.019), "in-line");
    assert_eq!(outcome(-0.05), "miss");
}

#[test]
fn surprise_lands_on_the_quarter_with_a_nearby_end_date() {
    let mut q = nonbank();
    attach_surprises(
        &mut q,
        &[research::Surprise {
            quarter: d("2026-09-28"),
            actual: 14.5,
            estimate: 13.9,
            surprise_pct: 0.043,
        }],
    );
    let s = q.last().unwrap().surprise.as_ref().unwrap();
    assert_eq!(s.outcome, "beat");
    assert!(q[..9].iter().all(|p| p.surprise.is_none()));
}

/// Daily bars with the given closes from `start`.
fn flat_series(closes: &[f64], start: &str) -> Series {
    let mut s = Series::default();
    let mut day = d(start);
    for c in closes {
        s.dates.push(day);
        s.open.push(*c);
        s.high.push(*c);
        s.low.push(*c);
        s.close.push(*c);
        s.volume.push(1000.0);
        day = day.succ_opt().unwrap();
    }
    s
}

#[test]
fn reaction_measures_one_and_five_sessions_against_the_index() {
    let prices = flat_series(
        &[
            100.0, 100.0, 100.0, 100.0, 100.0, 110.0, 111.0, 112.0, 113.0, 114.0, 115.0,
        ],
        "2026-10-01",
    );
    let bench = flat_series(
        &[
            1000.0, 1000.0, 1000.0, 1000.0, 1000.0, 1010.0, 1010.0, 1010.0, 1010.0, 1010.0, 1010.0,
        ],
        "2026-10-01",
    );
    // Announced 2026-10-06, 11:00 IST (before the close).
    let ts = d("2026-10-06")
        .and_hms_opt(5, 30, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    let r = reaction_at(ts, &prices, &bench, true).unwrap();
    assert_eq!(r.session, d("2026-10-06"));
    assert!((r.one_day - 0.10).abs() < 1e-9);
    assert!((r.one_day_excess.unwrap() - 0.09).abs() < 1e-9);
    assert!((r.five_day.unwrap() - 0.14).abs() < 1e-9);
    assert!((r.five_day_excess.unwrap() - 0.13).abs() < 1e-9);
}

#[test]
fn five_day_reaction_is_absent_when_prices_stop_early() {
    let prices = flat_series(&[100.0, 100.0, 110.0, 111.0], "2026-10-05");
    let ts = d("2026-10-07")
        .and_hms_opt(5, 30, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    let r = reaction_at(ts, &prices, &Series::default(), true).unwrap();
    assert!(r.five_day.is_none() && r.one_day_excess.is_none());
}

// ── Annual ──

/// Six fiscal years growing 10% a year.
fn annual() -> Vec<ResultPeriod> {
    (0..6)
        .map(|i| {
            let f = 1.1f64.powi(i);
            ResultPeriod {
                end: d(&format!("{}-03-31", 2021 + i)),
                revenue: Some(1000.0 * f),
                pat: Some(100.0 * f),
                eps: Some(10.0 * f),
                ocf: Some(120.0 * f),
                capex: Some(-40.0 * f),
                equity: Some(500.0),
                ebit: Some(150.0),
                total_assets: Some(1000.0),
                current_liabilities: Some(250.0),
                ..Default::default()
            }
        })
        .collect()
}

#[test]
fn annual_view_derives_cash_conversion_and_returns() {
    let mut a = annual();
    for p in &mut a {
        p.finalize();
    }
    let v = annual_views(&a, 5, true);
    assert_eq!(v.len(), 5);
    let last = v.last().unwrap();
    assert_eq!(last.fiscal_year, "FY26");
    assert_eq!(last.cfo_to_pat_pct, Some(120.0));
    // FCF = OCF - |capex| when Yahoo gives none.
    assert!((last.period.fcf.unwrap() - 80.0 * 1.1f64.powi(5)).abs() < 1e-6);
    assert_eq!(last.roe_pct, Some(32.21));
    assert_eq!(last.roce_pct, Some(20.0));
    assert_eq!(last.revenue_yoy_pct, Some(10.0));
}

#[test]
fn cagr_covers_the_years_the_data_reaches() {
    let c = cagrs(&annual());
    assert_eq!(c.iter().map(|g| g.years).collect::<Vec<_>>(), vec![3, 5]);
    assert_eq!(c[0].revenue_pct, Some(10.0));
    assert_eq!(c[1].pat_pct, Some(10.0));
}

#[test]
fn fiscal_year_labels() {
    assert_eq!(fiscal_year_label(d("2026-03-31"), true), "FY26");
    assert_eq!(fiscal_year_label(d("2026-12-31"), true), "FY27");
    assert_eq!(fiscal_year_label(d("2025-09-27"), false), "FY2025");
}

// ── Banks ──

// Trimmed real NSE files for HDFC Bank, Q3 FY25 (31-Dec-2024): the filing
// list, and the consolidated and standalone banking-format XBRL. The
// consolidated file carries 0.00 placeholders for the NPA ratios.
const NSE_LIST: &str = include_str!("../fixtures/nse_results_hdfcbank.json");
const XBRL_CONSOLIDATED: &str = include_str!("../fixtures/nse_bank_xbrl_hdfcbank_consolidated.xml");
const BANK_XBRL: &str = include_str!("../fixtures/nse_bank_xbrl_hdfcbank_standalone.xml");

#[test]
fn bank_xbrl_gives_gross_and_net_npa_as_percent() {
    // Filed as fractions: 0.0142 and 0.0046.
    assert_eq!(parse_bank_xbrl(BANK_XBRL), (Some(1.42), Some(0.46)));
}

#[test]
fn consolidated_placeholder_zeros_are_not_ratios() {
    assert_eq!(parse_bank_xbrl(XBRL_CONSOLIDATED), (None, None));
}

#[test]
fn only_the_current_quarter_context_counts() {
    let xml = r#"<x:a xmlns:x="u">
<x:PercentageOfGrossNpa contextRef="FourD">0.0900</x:PercentageOfGrossNpa>
<x:PercentageOfGrossNpa contextRef="OneD" unitRef="pure">0.0207</x:PercentageOfGrossNpa>
<x:PercentageOfNpa contextRef="OneD">0.0053</x:PercentageOfNpa></x:a>"#;
    assert_eq!(parse_bank_xbrl(xml), (Some(2.07), Some(0.53)));
}

#[test]
fn xbrl_without_npa_gives_nothing() {
    assert_eq!(parse_bank_xbrl("<a:Revenue>5</a:Revenue>"), (None, None));
}

#[test]
fn the_real_nse_list_gives_the_filing_with_a_standalone_fallback() {
    let v: Value = serde_json::from_str(NSE_LIST).unwrap();
    let f = parse_nse_filings(&v);
    // The row without a file (`.../xbrl/-`) is skipped.
    assert!(f.iter().all(|x| x.xbrl.ends_with(".xml")));
    assert_eq!(f[0].to_date, d("2024-12-31"));
    assert!(f[0].consolidated);
    assert!(
        f[0].xbrl
            .ends_with("BANKING_117525_1359016_23012025122721.xml")
    );
    assert!(
        f[0].standalone
            .as_deref()
            .unwrap()
            .ends_with("BANKING_117524_1359008_23012025122553.xml")
    );
}

#[test]
fn nse_filings_keep_one_per_quarter_and_prefer_consolidated() {
    let v = json!([
        {"toDate": "30-Sep-2026", "xbrl": "https://x/solo.xml", "consolidated": "Non-Consolidated"},
        {"toDate": "30-Sep-2026", "xbrl": "https://x/cons.xml", "consolidated": "Consolidated"},
        {"toDate": "30-Sep-2026", "xbrl": "https://x/cons-old.xml", "consolidated": "Consolidated"},
        {"toDate": "30-Jun-2026", "xbrl": "https://x/jun.xml", "consolidated": "Non-Consolidated"},
        {"toDate": "31-Mar-2026", "xbrl": "-"},
        {"toDate": "31-Dec-2025", "xbrl": "https://nsearchives.nseindia.com/corporate/xbrl/-", "consolidated": "Consolidated"},
        {"toDate": "garbage", "xbrl": "https://x/bad.xml"}
    ]);
    let f = parse_nse_filings(&v);
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].to_date, d("2026-09-30"));
    // The list is newest first: the first revision wins.
    assert_eq!(f[0].xbrl, "https://x/cons.xml");
    assert_eq!(f[0].standalone.as_deref(), Some("https://x/solo.xml"));
    assert_eq!(f[1].to_date, d("2026-06-30"));
    assert!(!f[1].consolidated);
}

/// Eight quarters of bank statements.
fn bank_series() -> Vec<Value> {
    let rows = |f: &dyn Fn(usize) -> f64| -> Vec<(String, f64)> {
        QUARTER_ENDS[2..]
            .iter()
            .enumerate()
            .map(|(i, e)| (e.to_string(), f(i)))
            .collect()
    };
    vec![
        series(
            "quarterly",
            "TotalRevenue",
            &rows(&|i| 8000.0 + 100.0 * i as f64),
            "INR",
        ),
        series(
            "quarterly",
            "NetInterestIncome",
            &rows(&|i| 3000.0 + 50.0 * i as f64),
            "INR",
        ),
        series(
            "quarterly",
            "PretaxIncome",
            &rows(&|i| 2000.0 + 40.0 * i as f64),
            "INR",
        ),
        series(
            "quarterly",
            "NetIncome",
            &rows(&|i| 1500.0 + 30.0 * i as f64),
            "INR",
        ),
        series(
            "quarterly",
            "DilutedEPS",
            &rows(&|i| 20.0 + 0.4 * i as f64),
            "INR",
        ),
    ]
}

/// A Yahoo quote response for HDFCBANK.NS.
fn quote_body(sector: &str, industry: &str) -> String {
    json!({"quoteResponse": {"result": [{
        "symbol": "HDFCBANK.NS", "quoteType": "EQUITY", "longName": "HDFC Bank Limited",
        "currency": "INR", "regularMarketPrice": 1700.0,
        "sector": sector, "industry": industry
    }]}})
    .to_string()
}

/// A client whose requests go to the mock server at `server`.
fn client_for(server: &str) -> YahooClient {
    let base = server.to_string();
    YahooClient::for_tests_with_rewrite(reqwest::Client::new(), "c", move |url: &str| {
        let u = reqwest::Url::parse(url).unwrap();
        format!("{base}{}", u.path()) + &u.query().map_or(String::new(), |q| format!("?{q}"))
    })
}

/// Routes for a bank: quote, summary, timeseries, NSE list and XBRL.
fn bank_server() -> Vec<(&'static str, String)> {
    let summary = json!({"quoteSummary": {"result": [{"earningsHistory": {"history": [
        {"quarter": {"raw": 1767139200}, "epsActual": {"raw": 21.2}, "epsEstimate": {"raw": 20.0},
         "surprisePercent": {"raw": 0.06}}
    ]}}]}});
    let filings: Vec<Value> = ["30-Sep-2026", "30-Jun-2026", "31-Mar-2026", "31-Dec-2025"]
        .iter()
        .enumerate()
        .map(|(i, dt)| {
            json!({"toDate": dt, "consolidated": "Consolidated",
                   "xbrl": format!("https://nsearchives.nseindia.com/x/q{i}.xml")})
        })
        .collect();
    vec![
        (
            "corporates-financial-results",
            Value::Array(filings).to_string(),
        ),
        (".xml", BANK_XBRL.to_string()),
        (
            "fundamentals-timeseries",
            json!({"timeseries": {"result": bank_series()}}).to_string(),
        ),
        ("quoteSummary", summary.to_string()),
        (
            "/v7/finance/quote",
            quote_body("Financial Services", "Banks - Regional"),
        ),
    ]
}

/// Consolidated placeholders (0.00) make a quarter fall back to the
/// standalone filing.
#[tokio::test]
async fn a_zero_consolidated_filing_falls_back_to_standalone() {
    let mut routes = bank_server();
    let filings: Vec<Value> = vec![
        json!({"toDate": "31-Dec-2025", "consolidated": "Consolidated",
            "xbrl": "https://nsearchives.nseindia.com/x/cons.xml"}),
        json!({"toDate": "31-Dec-2025", "consolidated": "Non-Consolidated",
            "xbrl": "https://nsearchives.nseindia.com/x/solo.xml"}),
    ];
    routes.retain(|(n, _)| *n != "corporates-financial-results");
    routes.insert(
        0,
        (
            "corporates-financial-results",
            Value::Array(filings).to_string(),
        ),
    );
    routes.insert(0, ("cons.xml", XBRL_CONSOLIDATED.to_string()));
    let server = crate::testutil::MockServer::start_routed(routes).await;
    let c = collect(
        &client_for(&server.url),
        "HDFCBANK",
        Market::In,
        None,
        false,
        true,
    )
    .await
    .unwrap();
    let q = c
        .quarterly
        .iter()
        .find(|p| p.end == d("2025-12-31"))
        .unwrap();
    assert_eq!((q.gnpa_pct, q.nnpa_pct), (Some(1.42), Some(0.46)));
}

/// An in-memory `cache.db` that lives for the test run.
fn memory_db() -> L2 {
    Box::leak(Box::new(Mutex::new(CacheDb::open_in_memory().unwrap())))
}

#[tokio::test]
async fn a_bank_shows_nii_asset_quality_and_the_surprise() {
    let server = crate::testutil::MockServer::start_routed(bank_server()).await;
    let client = client_for(&server.url);
    let c = collect(&client, "HDFCBANK", Market::In, None, false, true)
        .await
        .unwrap();
    assert!(c.is_financial);
    assert_eq!(c.quarterly.len(), 8);
    let last = c.quarterly.last().unwrap();
    assert_eq!(last.nii, Some(3000.0 + 50.0 * 7.0));
    assert_eq!((last.gnpa_pct, last.nnpa_pct), (Some(1.42), Some(0.46)));
    // Banks have no operating profit: no OPM either.
    assert_eq!(last.opm(), None);
    // 1767139200 = 2025-12-31.
    let s = c
        .quarterly
        .iter()
        .find(|p| p.end == d("2025-12-31"))
        .and_then(|p| p.surprise.as_ref())
        .unwrap();
    assert_eq!(s.outcome, "beat");
    assert!(c.sources.iter().any(|s| s.contains("NSE")));
    // JSON and CSV carry the bank rows.
    let j = quarterly_json(&c, 8);
    assert_eq!(j["quarters"][7]["gnpa_pct"], 1.42);
    assert_eq!(j["currency"], "INR");
    let csv = output::to_csv(&quarterly_csv(&c, 8)).unwrap();
    assert!(csv.lines().next().unwrap().contains("gnpa_pct"));
    assert_eq!(csv.lines().count(), 9);
}

#[tokio::test]
async fn a_failing_nse_list_leaves_the_bank_rows_out_with_a_note() {
    let mut routes = bank_server();
    routes.retain(|(n, _)| *n != "corporates-financial-results");
    let server = crate::testutil::MockServer::start_routed(routes).await;
    let c = collect(
        &client_for(&server.url),
        "HDFCBANK",
        Market::In,
        None,
        false,
        true,
    )
    .await
    .unwrap();
    assert!(c.quarterly.iter().all(|p| p.gnpa_pct.is_none()));
    assert!(c.warnings.iter().any(|w| w.contains("NSE asset quality")));
    assert_eq!(c.quarterly.len(), 8);
}

#[tokio::test]
async fn a_non_bank_never_asks_nse() {
    let routes = vec![
        (
            "fundamentals-timeseries",
            json!({"timeseries": {"result": nonbank_series()}}).to_string(),
        ),
        (
            "quoteSummary",
            json!({"quoteSummary": {"result": [{}]}}).to_string(),
        ),
        ("/v7/finance/quote", quote_body("Technology", "IT Services")),
    ];
    let server = crate::testutil::MockServer::start_routed(routes).await;
    let c = collect(
        &client_for(&server.url),
        "TCS",
        Market::In,
        None,
        false,
        true,
    )
    .await
    .unwrap();
    assert_eq!(c.quarterly.len(), 10);
    assert!(
        server
            .requests()
            .iter()
            .all(|r| !r.target.contains("nseindia") && !r.target.contains("corporates"))
    );
}

#[tokio::test]
async fn timeseries_is_asked_with_explicit_period_bounds_for_many_years() {
    let routes = vec![
        (
            "fundamentals-timeseries",
            json!({"timeseries": {"result": nonbank_series()}}).to_string(),
        ),
        (
            "quoteSummary",
            json!({"quoteSummary": {"result": [{}]}}).to_string(),
        ),
        ("/v7/finance/quote", quote_body("Technology", "IT Services")),
    ];
    let server = crate::testutil::MockServer::start_routed(routes).await;
    collect(
        &client_for(&server.url),
        "TCS",
        Market::In,
        None,
        false,
        true,
    )
    .await
    .unwrap();
    let req = server
        .requests()
        .into_iter()
        .find(|r| r.target.contains("fundamentals-timeseries"))
        .unwrap();
    let q = |k: &str| -> i64 {
        let from = req.target.find(&format!("{k}=")).unwrap() + k.len() + 1;
        req.target[from..]
            .split('&')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    assert!(q("period2") - q("period1") >= 365 * 10 * 86_400);
    assert!(req.target.contains("quarterlyTotalRevenue"));
}

// ── Cache ──

#[tokio::test]
async fn a_repeat_call_is_served_from_financial_results_without_a_request() {
    let routes = vec![
        (
            "fundamentals-timeseries",
            json!({"timeseries": {"result": nonbank_series()}}).to_string(),
        ),
        (
            "quoteSummary",
            json!({"quoteSummary": {"result": [{}]}}).to_string(),
        ),
        ("/v7/finance/quote", quote_body("Technology", "IT Services")),
    ];
    let server = crate::testutil::MockServer::start_routed(routes).await;
    let db = memory_db();
    let first = collect(
        &client_for(&server.url),
        "TCS",
        Market::In,
        Some(db),
        false,
        true,
    )
    .await
    .unwrap();
    assert!(!first.from_cache);
    let stored = db.lock().unwrap().get_results("TCS.NS").unwrap();
    assert_eq!(stored.len(), 10, "every fetched quarter is persisted");
    let before = server
        .requests()
        .iter()
        .filter(|r| r.target.contains("timeseries") || r.target.contains("quoteSummary"))
        .count();

    let second = collect(
        &client_for(&server.url),
        "TCS",
        Market::In,
        Some(db),
        false,
        true,
    )
    .await
    .unwrap();
    assert!(second.from_cache);
    assert_eq!(
        serde_json::to_value(&second.quarterly).unwrap(),
        serde_json::to_value(&first.quarterly).unwrap()
    );
    let after = server
        .requests()
        .iter()
        .filter(|r| r.target.contains("timeseries") || r.target.contains("quoteSummary"))
        .count();
    assert_eq!(before, after, "no statements request on the second call");
}

#[test]
fn stale_cached_results_are_not_served() {
    let db = CacheDb::open_in_memory().unwrap();
    let mut p = ResultPeriod::new(d("2026-09-30"));
    p.revenue = Some(1.0);
    db.put_result(
        "X",
        "2026-09-30",
        false,
        "yahoo",
        &serde_json::to_string(&p).unwrap(),
    )
    .unwrap();
    assert!(load_periods(&db, "X", 3600).is_some());
    assert!(load_periods(&db, "X", -1).is_none());
    assert!(load_periods(&db, "Y", 3600).is_none());
}

#[test]
fn the_default_store_is_off_in_unit_tests() {
    assert!(default_store().is_none());
}

// ── Scoreboard ──

/// A `Collected` holding `quarters`.
fn collected(symbol: &str, quarters: Vec<ResultPeriod>) -> Collected {
    Collected {
        symbol: symbol.into(),
        name: symbol.into(),
        currency: Some("INR".into()),
        is_india: true,
        is_financial: false,
        quarterly: quarters,
        annual: vec![],
        sources: vec![],
        warnings: vec![],
        from_cache: false,
    }
}

/// A quarter with revenue, PAT and an optional surprise outcome.
fn q(end: &str, revenue: f64, pat: f64, outcome: Option<&str>) -> ResultPeriod {
    ResultPeriod {
        end: d(end),
        revenue: Some(revenue),
        operating_profit: Some(revenue * 0.2),
        pat: Some(pat),
        surprise: outcome.map(|o| EpsSurprise {
            actual: 1.0,
            estimate: 1.0,
            surprise: 0.0,
            outcome: o.into(),
        }),
        ..Default::default()
    }
}

#[test]
fn the_scoreboard_aggregates_reporters_and_lists_the_pending() {
    let a = collected(
        "A.NS",
        vec![
            q("2025-09-30", 100.0, 10.0, None),
            q("2026-09-30", 120.0, 15.0, Some("beat")),
        ],
    );
    let b = collected(
        "B.NS",
        vec![
            q("2025-09-30", 300.0, 30.0, None),
            q("2026-09-30", 330.0, 27.0, Some("miss")),
        ],
    );
    let c = collected(
        "C.NS",
        vec![
            q("2025-06-30", 50.0, 5.0, None),
            q("2026-06-30", 60.0, 6.0, None),
        ],
    );
    let board = scoreboard("test", 4, vec!["D.NS".into()], &[a, b, c], Sort::Pat);
    assert_eq!(board.quarter.as_deref(), Some("Q2 FY27"));
    assert_eq!((board.total, board.reported), (4, 2));
    assert_eq!(board.pending, vec!["C.NS"]);
    assert_eq!(board.failed, vec!["D.NS"]);
    // Revenue: 450 against 400; net profit: 42 against 40.
    assert_eq!(board.aggregate.revenue_yoy_pct, Some(12.5));
    assert_eq!(board.aggregate.pat_yoy_pct, Some(5.0));
    assert_eq!((board.beats, board.in_line, board.misses), (1, 0, 1));
    // Highest net profit growth first.
    assert_eq!(board.reporters[0].symbol, "A.NS");
    assert_eq!(board.reporters[0].pat_yoy_pct, Some(50.0));
    assert_eq!(board.reporters[1].pat_yoy_pct, Some(-10.0));
}

#[test]
fn an_empty_scoreboard_is_not_an_error() {
    let board = scoreboard(
        "none",
        3,
        vec!["A".into(), "B".into(), "C".into()],
        &[],
        Sort::Date,
    );
    assert_eq!(board.reported, 0);
    assert_eq!(board.quarter, None);
}

#[test]
fn scoreboard_json_and_csv_agree_on_fields() {
    let a = collected(
        "A.NS",
        vec![
            q("2025-09-30", 100.0, 10.0, None),
            q("2026-09-30", 120.0, 15.0, None),
        ],
    );
    let board = scoreboard("t", 1, vec![], &[a], Sort::Date);
    let rows = reporters_csv(&board);
    let (missing, extra) = output::missing_columns(&rows[0]);
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{missing:?} {extra:?}"
    );
    let j = serde_json::to_value(&board).unwrap();
    assert_eq!(j["reporters"][0]["symbol"], "A.NS");
}

#[test]
fn csv_rows_match_their_columns() {
    let c = collected("A.NS", nonbank());
    let (m, e) = output::missing_columns(&quarterly_csv(&c, 3)[0]);
    assert!(m.is_empty() && e.is_empty(), "{m:?} {e:?}");
    let mut c = collected("A.NS", vec![]);
    c.annual = annual();
    let (m, e) = output::missing_columns(&annual_csv(&c, 3)[0]);
    assert!(m.is_empty() && e.is_empty(), "{m:?} {e:?}");
}

#[test]
fn us_quarters_are_labelled_by_calendar_quarter() {
    let views = quarter_views(&nonbank(), 8, false);
    assert_eq!(views.len(), 8);
    assert_eq!(views.last().unwrap().quarter, "Q3 2026");
    assert_eq!(views.last().unwrap().yoy.revenue, Some(16.0));
}

#[test]
fn a_set_with_no_shared_quarter_still_shows_the_newest() {
    let a = collected(
        "A.NS",
        vec![
            q("2025-09-30", 100.0, 10.0, None),
            q("2026-09-30", 120.0, 15.0, None),
        ],
    );
    let b = collected(
        "B.NS",
        vec![
            q("2025-06-30", 100.0, 10.0, None),
            q("2026-06-30", 110.0, 11.0, None),
        ],
    );
    let board = scoreboard("t", 2, vec![], &[a, b], Sort::Date);
    assert_eq!(board.reported, 1);
    assert_eq!(board.pending, vec!["B.NS"]);
}

#[tokio::test]
async fn a_cache_hit_keeps_the_statement_currency() {
    let routes = vec![
        (
            "fundamentals-timeseries",
            json!({"timeseries": {"result": nonbank_series()}}).to_string(),
        ),
        (
            "quoteSummary",
            json!({"quoteSummary": {"result": [{}]}}).to_string(),
        ),
        // Quote currency differs from the statements' INR.
        (
            "/v7/finance/quote",
            quote_body("Technology", "IT Services").replace("INR", "GBp"),
        ),
    ];
    let server = crate::testutil::MockServer::start_routed(routes).await;
    let db = memory_db();
    for _ in 0..2 {
        let c = collect(
            &client_for(&server.url),
            "TCS",
            Market::In,
            Some(db),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(c.currency.as_deref(), Some("INR"));
    }
}

#[tokio::test]
async fn saved_symbols_are_not_resolved_again() {
    let routes = vec![
        (
            "fundamentals-timeseries",
            json!({"timeseries": {"result": nonbank_series()}}).to_string(),
        ),
        (
            "quoteSummary",
            json!({"quoteSummary": {"result": [{}]}}).to_string(),
        ),
        ("/v7/finance/quote", quote_body("Technology", "IT Services")),
    ];
    let server = crate::testutil::MockServer::start_routed(routes).await;
    let c = collect(
        &client_for(&server.url),
        "AAPL",
        Market::In,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(c.symbol, "AAPL");
}

#[tokio::test]
async fn results_fall_back_to_keyed_providers_when_yahoo_has_no_statements() {
    use crate::providers::{Provider, ProviderKind, router::Router};
    // Yahoo answers the quote only: statements and earnings 404.
    let yahoo = crate::testutil::MockServer::start_routed(vec![(
        "/v7/finance/quote",
        json!({"quoteResponse": {"result": [{
            "symbol": "AAPL", "quoteType": "EQUITY", "longName": "Apple Inc.",
            "currency": "USD", "regularMarketPrice": 260.0, "marketCap": 3.9e12
        }]}})
        .to_string(),
    )])
    .await;
    let income = json!([
        {"date": "2025-06-28", "period": "Q3", "reportedCurrency": "USD", "revenue": 94036.0, "netIncome": 23434.0, "epsDiluted": 1.57},
        {"date": "2025-09-27", "period": "Q4", "reportedCurrency": "USD", "revenue": 102466.0, "netIncome": 27466.0, "epsDiluted": 1.85},
        {"date": "2025-09-27", "period": "FY", "reportedCurrency": "USD", "revenue": 416161.0, "netIncome": 112010.0}
    ]);
    let fmp = crate::testutil::MockServer::start_routed(vec![
        ("/income-statement", income.to_string()),
        ("/balance-sheet-statement", "[]".into()),
        ("/cash-flow-statement", "[]".into()),
    ])
    .await;
    let finnhub = crate::testutil::MockServer::start_routed(vec![
        ("/stock/profile2", r#"{"name":"Apple Inc"}"#.into()),
        ("/stock/recommendation", "[]".into()),
        (
            "/stock/earnings",
            r#"[{"actual":1.85,"estimate":1.7,"period":"2025-09-30","surprisePercent":8.8}]"#
                .into(),
        ),
    ])
    .await;
    let router = Router::new(vec![
        Provider::new(ProviderKind::Fmp, "k".into(), Some(fmp.url.clone())),
        Provider::new(ProviderKind::Finnhub, "k".into(), Some(finnhub.url.clone())),
    ]);
    let client = client_for(&yahoo.url).with_router(router);
    let c = collect(&client, "AAPL", Market::Us, None, false, true)
        .await
        .unwrap();
    assert_eq!(c.quarterly.len(), 2);
    assert_eq!(c.annual.len(), 1);
    assert_eq!(c.currency.as_deref(), Some("USD"));
    assert!(
        c.sources
            .iter()
            .any(|s| s.contains("Financial Modeling Prep"))
    );
    assert!(
        c.sources
            .iter()
            .any(|s| s.contains("Finnhub earnings surprises"))
    );
    let s = c.quarterly[1].surprise.as_ref().expect("surprise attached");
    assert_eq!(s.outcome, "beat");
}

#[tokio::test]
async fn results_error_stays_the_yahoo_one_without_keyed_providers() {
    let yahoo = crate::testutil::MockServer::start_routed(vec![(
        "/v7/finance/quote",
        quote_body("Technology", "Software"),
    )])
    .await;
    let client = client_for(&yahoo.url);
    let err = collect(&client, "HDFCBANK", Market::In, None, false, true)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("Yahoo financial statements unavailable"));
}

// ── SEC EDGAR ──

/// A US quote (the symbol is fixed by the test client's routes).
fn us_quote_body() -> String {
    json!({"quoteResponse": {"result": [{
        "symbol": "ACME", "quoteType": "EQUITY", "longName": "Calendar Corp",
        "currency": "USD", "regularMarketPrice": 50.0,
        "sector": "Technology", "industry": "Software"
    }]}})
    .to_string()
}

/// Yahoo's last four US quarters: revenue 999 (EDGAR says otherwise), with
/// a pre-tax figure only Yahoo has, and a 2025-12-31 quarter EDGAR has not
/// filed yet.
fn us_yahoo_series() -> Vec<Value> {
    let ends = ["2025-03-31", "2025-06-30", "2025-09-30", "2025-12-31"];
    let rows = |v: f64| -> Vec<(String, f64)> { ends.iter().map(|e| (e.to_string(), v)).collect() };
    vec![
        series("quarterly", "TotalRevenue", &rows(999.0), "USD"),
        series("quarterly", "PretaxIncome", &rows(7.0), "USD"),
        series("quarterly", "FreeCashFlow", &rows(1.0), "USD"),
    ]
}

fn us_routes(edgar: bool) -> Vec<(&'static str, String)> {
    let mut routes = vec![
        (
            "fundamentals-timeseries",
            json!({"timeseries": {"result": us_yahoo_series()}}).to_string(),
        ),
        (
            "quoteSummary",
            json!({"quoteSummary": {"result": [{}]}}).to_string(),
        ),
        ("/v7/finance/quote", us_quote_body()),
    ];
    if edgar {
        routes.push((
            "company_tickers.json",
            json!({"0": {"cik_str": 1234, "ticker": "ACME", "title": "Calendar Corp"}}).to_string(),
        ));
        routes.push((
            "companyfacts/CIK0000001234.json",
            crate::providers::edgar::tests::calendar_filer(),
        ));
    }
    routes
}

#[tokio::test]
async fn us_results_use_edgar_for_the_long_history_and_yahoo_for_the_rest() {
    let server = crate::testutil::MockServer::start_routed(us_routes(true)).await;
    let db = memory_db();
    let c = collect(
        &client_for(&server.url),
        "ACME",
        Market::Us,
        Some(db),
        false,
        true,
    )
    .await
    .unwrap();
    // 15 EDGAR quarters plus the one only Yahoo has.
    assert_eq!(c.quarterly.len(), 16);
    assert_eq!(c.annual.len(), 3);
    assert!(c.sources.iter().any(|s| s.contains("SEC EDGAR")));
    assert!(c.sources.iter().any(|s| s.contains("Yahoo")));
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    let q = |end: &str| c.quarterly.iter().find(|p| p.end == d(end)).unwrap();
    // EDGAR wins what it has, Yahoo keeps what EDGAR lacks.
    assert_eq!(q("2025-03-31").revenue, Some(220.0));
    assert_eq!(q("2025-03-31").pbt, Some(7.0));
    assert_eq!(q("2022-12-31").revenue, Some(130.0));
    assert_eq!(q("2025-12-31").revenue, Some(999.0));
    assert!(!q("2025-12-31").edgar && q("2025-09-30").edgar);
    // Free cash flow follows the EDGAR cash flow and capex.
    let p = q("2025-09-30");
    assert_eq!(p.fcf, Some(p.ocf.unwrap() - p.capex.unwrap().abs()));
    // Every period lands in cache.db, tagged with the source.
    let stored = db.lock().unwrap().get_results("ACME").unwrap();
    assert_eq!(stored.len(), 16 + 3);
    let tags = stored
        .iter()
        .filter(|(e, _, j)| j.contains("\"edgar\":true") && !e.is_empty());
    assert_eq!(tags.count(), 15 + 3);
    // EDGAR was asked politely and with a client name.
    let sec: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.target.contains("company"))
        .collect();
    assert_eq!(sec.len(), 2);
    for r in &sec {
        let ua = r.header("user-agent").unwrap();
        assert!(ua.starts_with("stockwise/") && !ua.contains('@'), "{ua}");
    }
    // The second call is served from cache.db, naming both sources.
    let again = collect(
        &client_for(&server.url),
        "ACME",
        Market::Us,
        Some(db),
        false,
        true,
    )
    .await
    .unwrap();
    assert!(again.from_cache);
    assert_eq!(again.quarterly.len(), 16);
    assert!(again.sources[0].contains("SEC EDGAR"));
}

#[tokio::test]
async fn a_failing_edgar_leaves_yahoo_results_with_a_note() {
    let server = crate::testutil::MockServer::start_routed(us_routes(false)).await;
    let c = collect(
        &client_for(&server.url),
        "ACME",
        Market::Us,
        None,
        false,
        true,
    )
    .await
    .unwrap();
    assert_eq!(c.quarterly.len(), 4);
    assert!(c.warnings.iter().any(|w| w.contains("SEC EDGAR")));
    assert!(!c.sources.iter().any(|s| s.contains("EDGAR")));
}

#[tokio::test]
async fn edgar_alone_carries_the_results_when_yahoo_statements_fail() {
    let mut routes = us_routes(true);
    routes.retain(|(n, _)| *n != "fundamentals-timeseries");
    let server = crate::testutil::MockServer::start_routed(routes).await;
    let c = collect(
        &client_for(&server.url),
        "ACME",
        Market::Us,
        None,
        false,
        true,
    )
    .await
    .unwrap();
    assert_eq!(c.quarterly.len(), 15);
    assert_eq!(c.currency.as_deref(), Some("USD"));
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("Yahoo financial statements"))
    );
}

#[tokio::test]
async fn other_markets_and_the_scoreboard_never_ask_edgar() {
    let server = crate::testutil::MockServer::start_routed(us_routes(true)).await;
    let client = client_for(&server.url);
    collect(&client, "ACME", Market::In, None, false, true)
        .await
        .unwrap();
    season(
        &client,
        &["ACME".to_string()],
        "Test",
        Market::Us,
        None,
        Sort::Date,
    )
    .await;
    assert!(
        server
            .requests()
            .iter()
            .all(|r| !r.target.contains("company_tickers") && !r.target.contains("companyfacts"))
    );
}

#[tokio::test]
async fn edgar_operating_profit_survives_the_merge() {
    let server = crate::testutil::MockServer::start_routed(us_routes(true)).await;
    let c = collect(
        &client_for(&server.url),
        "ACME",
        Market::Us,
        None,
        false,
        true,
    )
    .await
    .unwrap();
    let p = c
        .quarterly
        .iter()
        .find(|p| p.end == d("2022-12-31"))
        .unwrap();
    assert_eq!(p.operating_profit, Some(13.0));
    assert!(p.fcf.is_some());
}

#[tokio::test]
async fn partial_results_and_scoreboard_entries_are_not_served_to_edgar_requests() {
    // EDGAR down: nothing is cached.
    let server = crate::testutil::MockServer::start_routed(us_routes(false)).await;
    let db = memory_db();
    collect(
        &client_for(&server.url),
        "ACME",
        Market::Us,
        Some(db),
        false,
        true,
    )
    .await
    .unwrap();
    assert!(db.lock().unwrap().get_results("ACME").unwrap().is_empty());
    // A scoreboard run caches Yahoo only; a results request still asks EDGAR.
    let server = crate::testutil::MockServer::start_routed(us_routes(true)).await;
    let db = memory_db();
    let client = client_for(&server.url);
    season(
        &client,
        &["ACME".to_string()],
        "T",
        Market::Us,
        Some(db),
        Sort::Date,
    )
    .await;
    let c = collect(&client, "ACME", Market::Us, Some(db), false, true)
        .await
        .unwrap();
    assert!(!c.from_cache);
    assert_eq!(c.quarterly.len(), 16);
}
