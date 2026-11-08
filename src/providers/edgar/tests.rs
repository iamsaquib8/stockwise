use super::*;
use serde_json::{Value, json};

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// One XBRL fact as `companyfacts` lists it.
fn fact(start: &str, end: &str, val: f64, form: &str, filed: &str) -> Value {
    json!({"start": start, "end": end, "val": val, "accn": "0000000000-00-000000",
           "fy": 2024, "fp": "Q1", "form": form, "filed": filed})
}

/// A `companyfacts` document from `(concept, unit, facts)`.
fn doc(name: &str, cik: u64, concepts: Vec<(&str, &str, Vec<Value>)>) -> String {
    let mut gaap = serde_json::Map::new();
    for (c, unit, facts) in concepts {
        gaap.insert(c.to_string(), json!({"label": c, "units": {unit: facts}}));
    }
    json!({"cik": cik, "entityName": name, "facts": {"us-gaap": gaap}}).to_string()
}

/// A fiscal year: its start and the four quarter ends.
struct Year {
    start: &'static str,
    ends: [&'static str; 4],
}

/// Facts the way a filer reports a metric. `income` metrics carry the
/// three-month fact of Q1..Q3 plus year-to-date at six and nine months;
/// cash-flow metrics only year to date. Q4 is never reported on its own:
/// the 10-K carries the whole year.
fn reported(years: &[Year], q: &[[f64; 4]], income: bool, whole_years: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, y) in years.iter().enumerate() {
        let filed = |end: &str, days: i64| (d(end) + Days::days(days)).to_string();
        let mut ytd = 0.0;
        let mut prev_end = y.start.to_string();
        for k in 0..3 {
            let v = q[i][k];
            ytd += v;
            let quarter_start = if k == 0 {
                y.start.to_string()
            } else {
                (d(&prev_end) + Days::days(1)).to_string()
            };
            let f = filed(y.ends[k], 30);
            if income {
                out.push(fact(&quarter_start, y.ends[k], v, "10-Q", &f));
            }
            if k > 0 || !income {
                out.push(fact(y.start, y.ends[k], ytd, "10-Q", &f));
            }
            prev_end = y.ends[k].to_string();
        }
        if i < whole_years {
            let total = q[i].iter().sum::<f64>();
            out.push(fact(
                y.start,
                y.ends[3],
                total,
                "10-K",
                &filed(y.ends[3], 45),
            ));
        }
    }
    out
}

fn calendar_years() -> Vec<Year> {
    (2022..=2025)
        .map(|y| Year {
            start: Box::leak(format!("{y}-01-01").into_boxed_str()),
            ends: [
                Box::leak(format!("{y}-03-31").into_boxed_str()),
                Box::leak(format!("{y}-06-30").into_boxed_str()),
                Box::leak(format!("{y}-09-30").into_boxed_str()),
                Box::leak(format!("{y}-12-31").into_boxed_str()),
            ],
        })
        .collect()
}

/// Revenue 100, 110, 120, 130 in 2022, rising 10 each quarter after.
fn revenue_q() -> Vec<[f64; 4]> {
    (0..4)
        .map(|y| {
            let b = 100.0 + 40.0 * y as f64;
            [b, b + 10.0, b + 20.0, b + 30.0]
        })
        .collect()
}

/// A calendar-year filer: 2025 has only Q1..Q3 reported.
pub(crate) fn calendar_filer() -> String {
    let years = calendar_years();
    let rev = revenue_q();
    let ni: Vec<[f64; 4]> = rev.iter().map(|r| r.map(|v| v / 10.0)).collect();
    let eps: Vec<[f64; 4]> = rev.iter().map(|r| r.map(|v| v / 100.0)).collect();
    let cfo: Vec<[f64; 4]> = rev.iter().map(|r| r.map(|v| v / 5.0)).collect();
    let capex: Vec<[f64; 4]> = rev.iter().map(|r| r.map(|v| v / 20.0)).collect();
    doc(
        "Calendar Corp",
        1234,
        vec![
            ("Revenues", "USD", reported(&years, &rev, true, 3)),
            ("OperatingIncomeLoss", "USD", reported(&years, &ni, true, 3)),
            ("NetIncomeLoss", "USD", reported(&years, &ni, true, 3)),
            (
                "EarningsPerShareDiluted",
                "USD/shares",
                reported(&years, &eps, true, 3),
            ),
            (
                "NetCashProvidedByUsedInOperatingActivities",
                "USD",
                reported(&years, &cfo, false, 3),
            ),
            (
                "PaymentsToAcquirePropertyPlantAndEquipment",
                "USD",
                reported(&years, &capex, false, 3),
            ),
        ],
    )
}

fn at(rows: &[Period], end: &str) -> Period {
    rows.iter()
        .find(|p| p.end == d(end))
        .unwrap_or_else(|| panic!("no period ending {end}"))
        .clone()
}

#[test]
fn calendar_filer_gets_every_quarter_with_a_derived_q4() {
    let st = parse_company_facts(&calendar_filer()).unwrap();
    assert_eq!(st.company, "Calendar Corp");
    assert_eq!(st.cik, 1234);
    // 2022-2024 have four quarters each, 2025 the three reported.
    assert_eq!(st.quarterly.len(), 15);
    assert_eq!(st.annual.len(), 3);
    // Q4 2022 = FY (460) - (100 + 110 + 120).
    assert_eq!(at(&st.quarterly, "2022-12-31").revenue, Some(130.0));
    assert_eq!(at(&st.quarterly, "2024-12-31").revenue, Some(210.0));
    assert_eq!(at(&st.quarterly, "2025-09-30").revenue, Some(240.0));
    assert!(st.quarterly.windows(2).all(|w| w[0].end < w[1].end));
    assert!(st.quarterly.iter().all(|p| p.end <= d("2025-09-30")));
}

#[test]
fn cash_flow_quarters_come_from_year_to_date_differences() {
    let st = parse_company_facts(&calendar_filer()).unwrap();
    // 10-Qs only give 3, 6 and 9 months of operating cash flow.
    let q2 = at(&st.quarterly, "2022-06-30");
    assert_eq!(q2.operating_cash_flow, Some(22.0));
    let q3 = at(&st.quarterly, "2022-09-30");
    assert_eq!(q3.operating_cash_flow, Some(24.0));
    // Q4 = FY - nine months.
    assert_eq!(
        at(&st.quarterly, "2022-12-31").operating_cash_flow,
        Some(26.0)
    );
    // Capex is cash out: negative, like the Yahoo statements.
    assert_eq!(at(&st.quarterly, "2022-12-31").capex, Some(-6.5));
    assert_eq!(st.annual[0].capex, Some(-23.0));
}

#[test]
fn every_metric_is_read_and_the_year_adds_up() {
    let st = parse_company_facts(&calendar_filer()).unwrap();
    let fy = at(&st.annual, "2023-12-31");
    assert_eq!(
        fy.revenue,
        Some(100.0 + 40.0 + 110.0 + 40.0 + 120.0 + 40.0 + 130.0 + 40.0)
    );
    assert_eq!(fy.net_income, Some(fy.revenue.unwrap() / 10.0));
    let q4 = at(&st.quarterly, "2023-12-31");
    assert_eq!(q4.operating_income, Some(17.0));
    assert!((q4.diluted_eps.unwrap() - 1.7).abs() < 1e-9);
    let sum: f64 = st
        .quarterly
        .iter()
        .filter(|p| p.end.format("%Y").to_string() == "2023")
        .map(|p| p.revenue.unwrap())
        .sum();
    assert_eq!(Some(sum), fy.revenue);
}

#[test]
fn a_restated_fact_takes_the_latest_filing() {
    let years = calendar_years();
    let mut facts = reported(&years, &revenue_q(), true, 3);
    // Q1 2022 restated by a later 10-Q, and the same fiscal year repeated
    // (with an old, different figure) in an earlier filing.
    facts.push(fact(
        "2022-01-01",
        "2022-03-31",
        105.0,
        "10-Q",
        "2023-05-02",
    ));
    facts.push(fact("2022-01-01", "2022-03-31", 99.0, "10-Q", "2022-04-20"));
    // An 8-K figure is not a statement.
    facts.push(fact("2022-04-01", "2022-06-30", 999.0, "8-K", "2030-01-01"));
    let json = doc("Calendar Corp", 1234, vec![("Revenues", "USD", facts)]);
    let st = parse_company_facts(&json).unwrap();
    assert_eq!(at(&st.quarterly, "2022-03-31").revenue, Some(105.0));
    assert_eq!(at(&st.quarterly, "2022-06-30").revenue, Some(110.0));
    // Q4 follows the restated Q1: 460 - (105 + 110 + 120).
    assert_eq!(at(&st.quarterly, "2022-12-31").revenue, Some(125.0));
}

#[test]
fn the_better_concept_wins_and_older_years_fall_back_to_the_other() {
    let years = calendar_years();
    let rev = revenue_q();
    let old = reported(&years[..2], &rev[..2], true, 2);
    let new = reported(&years[2..], &rev[2..], true, 1);
    let json = doc(
        "Calendar Corp",
        1234,
        vec![
            ("SalesRevenueNet", "USD", old),
            (
                "RevenueFromContractWithCustomerExcludingAssessedTax",
                "USD",
                new,
            ),
        ],
    );
    let st = parse_company_facts(&json).unwrap();
    assert_eq!(st.quarterly.len(), 4 + 4 + 4 + 3);
    assert_eq!(at(&st.quarterly, "2022-03-31").revenue, Some(100.0));
    assert_eq!(at(&st.quarterly, "2024-03-31").revenue, Some(180.0));
}

/// Fiscal years ending on the last Saturday of September (52/53 weeks).
fn september_years() -> Vec<Year> {
    let table = [
        (
            "2021-09-26",
            ["2021-12-25", "2022-03-26", "2022-06-25", "2022-09-24"],
        ),
        (
            "2022-09-25",
            ["2022-12-24", "2023-03-25", "2023-06-24", "2023-09-30"],
        ),
        (
            "2023-10-01",
            ["2023-12-30", "2024-03-30", "2024-06-29", "2024-09-28"],
        ),
        (
            "2024-09-29",
            ["2024-12-28", "2025-03-29", "2025-06-28", "2025-09-27"],
        ),
    ];
    table
        .into_iter()
        .map(|(start, ends)| Year {
            start: Box::leak(start.to_string().into_boxed_str()),
            ends: ends.map(|e| &*Box::leak(e.to_string().into_boxed_str())),
        })
        .collect()
}

fn september_filer() -> String {
    let years = september_years();
    let rev: Vec<[f64; 4]> = (0..4)
        .map(|y| {
            let b = 1000.0 + 100.0 * y as f64;
            [b + 50.0, b, b + 10.0, b + 20.0]
        })
        .collect();
    let cfo: Vec<[f64; 4]> = rev.iter().map(|r| r.map(|v| v / 4.0)).collect();
    doc(
        "Harvest Inc",
        5678,
        vec![
            ("Revenues", "USD", reported(&years, &rev, true, 4)),
            (
                "NetCashProvidedByUsedInOperatingActivities",
                "USD",
                reported(&years, &cfo, false, 4),
            ),
        ],
    )
}

#[test]
fn a_september_year_end_with_52_53_week_calendars_derives_q4() {
    let st = parse_company_facts(&september_filer()).unwrap();
    // Fiscal Q1 ends in December, Q4 on the September year end.
    assert_eq!(st.quarterly.len(), 16);
    assert_eq!(st.annual.len(), 4);
    // FY2022 (start 2021-09-26): 1050 + 1000 + 1010 + Q4.
    assert_eq!(at(&st.quarterly, "2021-12-25").revenue, Some(1050.0));
    assert_eq!(at(&st.quarterly, "2022-09-24").revenue, Some(1020.0));
    // The 53-week year (ends 2023-09-30) still pairs with its Q1..Q3.
    assert_eq!(at(&st.quarterly, "2023-09-30").revenue, Some(1120.0));
    assert_eq!(at(&st.quarterly, "2025-09-27").revenue, Some(1320.0));
    assert_eq!(
        at(&st.quarterly, "2025-09-27").operating_cash_flow,
        Some(330.0)
    );
    assert_eq!(at(&st.annual, "2024-09-28").revenue, Some(4880.0));
}

#[test]
fn history_is_capped() {
    let st = parse_company_facts(&september_filer()).unwrap();
    assert!(st.quarterly.len() <= QUARTERS_KEPT);
    assert!(st.annual.len() <= YEARS_KEPT);
}

#[test]
fn a_filer_without_us_gaap_facts_has_no_statements() {
    let json = json!({"cik": 9, "entityName": "Foreign ADR", "facts": {"ifrs-full": {}}});
    let st = parse_company_facts(&json.to_string()).unwrap();
    assert!(st.quarterly.is_empty() && st.annual.is_empty());
}

#[test]
fn a_reported_fourth_quarter_beats_the_derived_one() {
    let years = calendar_years();
    let mut facts = reported(&years[..1], &revenue_q()[..1], true, 1);
    facts.push(fact(
        "2022-10-01",
        "2022-12-31",
        131.0,
        "10-K",
        "2023-02-15",
    ));
    let json = doc("Calendar Corp", 1234, vec![("Revenues", "USD", facts)]);
    let st = parse_company_facts(&json).unwrap();
    assert_eq!(at(&st.quarterly, "2022-12-31").revenue, Some(131.0));
}

#[test]
fn tickers_map_to_a_cik_and_non_us_symbols_are_refused() {
    let list = json!({
        "0": {"cik_str": 320193, "ticker": "AAPL", "title": "Apple Inc."},
        "1": {"cik_str": 1067983, "ticker": "BRK-B", "title": "Berkshire"},
    })
    .to_string();
    assert_eq!(find_cik(&list, "AAPL"), Some(320193));
    assert_eq!(find_cik(&list, "brk-b"), Some(1067983));
    assert_eq!(find_cik(&list, "NOPE"), None);
    assert_eq!(ticker("aapl").as_deref(), Some("AAPL"));
    assert_eq!(ticker("BRK-B").as_deref(), Some("BRK-B"));
    for s in ["TCS.NS", "RELIANCE.BO", "^GSPC", "BTC-USD", "USDINR=X", ""] {
        assert_eq!(ticker(s), None, "{s}");
    }
}

#[test]
fn the_user_agent_names_the_tool_and_carries_no_address() {
    assert!(USER_AGENT.starts_with("stockwise/"));
    assert!(!USER_AGENT.contains('@'));
}
