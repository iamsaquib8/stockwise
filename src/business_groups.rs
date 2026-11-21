//! Indian business groups: listed companies that share a promoter.
//!
//! A portfolio spread over TCS, Tata Steel, Titan and Trent holds four
//! stocks but one promoter: governance news, a group-level debt scare
//! (Adani, January 2023) or a holding-company sale moves them together.
//! `portfolio xray` adds up each group's direct and look-through weight.
//!
//! The table lists the main listed companies of the larger groups by NSE
//! symbol. It is not exhaustive, and promoter holdings change; a company is
//! in a group when the group's promoter controls it.

use serde::{Deserialize, Serialize};

/// A group above this share of the portfolio gets a notice.
pub const GROUP_NOTICE_PCT: f64 = 20.0;

/// (group, NSE symbols without the `.NS` suffix).
pub const GROUPS: &[(&str, &[&str])] = &[
    (
        "Tata",
        &[
            "TCS",
            "TATAMOTORS",
            "TMPV",
            "TMCV",
            "TATASTEEL",
            "TITAN",
            "TRENT",
            "TATAPOWER",
            "TATACONSUM",
            "TATACOMM",
            "TATAELXSI",
            "TATACHEM",
            "TATAINVEST",
            "TATATECH",
            "INDHOTEL",
            "VOLTAS",
            "TEJASNET",
            "NELCO",
            "RALLIS",
        ],
    ),
    (
        "Adani",
        &[
            "ADANIENT",
            "ADANIPORTS",
            "ADANIGREEN",
            "ADANIPOWER",
            "ADANIENSOL",
            "ATGL",
            "AWL",
            "AMBUJACEM",
            "ACC",
            "NDTV",
            "SANGHIIND",
        ],
    ),
    (
        "Reliance",
        &["RELIANCE", "JIOFIN", "NETWORK18", "TV18BRDCST"],
    ),
    (
        "Bajaj",
        &[
            "BAJFINANCE",
            "BAJAJFINSV",
            "BAJAJ-AUTO",
            "BAJAJHLDNG",
            "BAJAJHFL",
            "BAJAJELEC",
        ],
    ),
    (
        "Aditya Birla",
        &[
            "GRASIM",
            "ULTRACEMCO",
            "HINDALCO",
            "ABCAPITAL",
            "ABFRL",
            "IDEA",
            "ABSLAMC",
            "ABLBL",
        ],
    ),
    (
        "Mahindra",
        &["M&M", "TECHM", "M&MFIN", "MAHLIFE", "MHRIL", "MAHLOG"],
    ),
    ("HDFC", &["HDFCBANK", "HDFCLIFE", "HDFCAMC"]),
    ("ICICI", &["ICICIBANK", "ICICIPRULI", "ICICIGI"]),
    ("Kotak", &["KOTAKBANK"]),
    ("L&T", &["LT", "LTIM", "LTTS", "LTF"]),
    ("JSW", &["JSWSTEEL", "JSWENERGY", "JSWINFRA", "JSWHL"]),
    ("Vedanta", &["VEDL", "HINDZINC"]),
    (
        "Murugappa",
        &[
            "CHOLAFIN",
            "CHOLAHLDNG",
            "TIINDIA",
            "CARBORUNIV",
            "COROMANDEL",
            "EIDPARRY",
        ],
    ),
    ("ITC", &["ITC", "ITCHOTELS"]),
    (
        "Hinduja",
        &["ASHOKLEY", "INDUSINDBK", "HINDUJAGL", "GOCOLORS"],
    ),
    (
        "Godrej",
        &["GODREJCP", "GODREJPROP", "GODREJIND", "GODREJAGRO"],
    ),
    ("Bharti", &["BHARTIARTL", "BHARTIHEXA", "INDUSTOWER"]),
    ("RPSG", &["CESC", "PCBL", "FIRSTSOUR"]),
    ("Wadia", &["BRITANNIA", "BOMDYEING"]),
    (
        "Public sector (Government of India)",
        &[
            "SBIN",
            "LICI",
            "ONGC",
            "NTPC",
            "POWERGRID",
            "COALINDIA",
            "IOC",
            "BPCL",
            "GAIL",
            "BEL",
            "HAL",
            "BANKBARODA",
            "PNB",
            "CANBK",
            "RECLTD",
            "PFC",
            "IRFC",
            "NHPC",
            "SBILIFE",
            "SBICARD",
        ],
    ),
];

/// The group `symbol` (`TCS.NS`, `TCS.BO` or `TCS`) belongs to.
pub fn group_of(symbol: &str) -> Option<&'static str> {
    let bare = crate::xray::canonical_ticker(symbol);
    GROUPS
        .iter()
        .find(|(_, members)| members.contains(&bare.as_str()))
        .map(|(g, _)| *g)
}

/// One group's weight in the portfolio, directly and through funds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupExposure {
    pub group: String,
    pub value: f64,
    pub weight_pct: f64,
    /// Member companies held, largest first.
    pub companies: Vec<String>,
    /// Above [`GROUP_NOTICE_PCT`].
    pub is_concentrated: bool,
}

/// Groups behind `exposures` (symbol, value), largest first. Only groups
/// with at least one company held are listed.
pub fn group_exposure(exposures: &[(String, f64)], total_value: f64) -> Vec<GroupExposure> {
    let mut out: Vec<GroupExposure> = Vec::new();
    let mut members: Vec<(&'static str, String, f64)> = exposures
        .iter()
        .filter(|(_, v)| *v > 0.0)
        .filter_map(|(s, v)| Some((group_of(s)?, crate::xray::canonical_ticker(s), *v)))
        .collect();
    members.sort_by(|a, b| b.2.total_cmp(&a.2));
    for (group, sym, value) in members {
        let g = match out.iter_mut().find(|g| g.group == group) {
            Some(g) => g,
            None => {
                out.push(GroupExposure {
                    group: group.to_string(),
                    value: 0.0,
                    weight_pct: 0.0,
                    companies: Vec::new(),
                    is_concentrated: false,
                });
                out.last_mut().expect("just pushed")
            }
        };
        g.value += value;
        if !g.companies.contains(&sym) {
            g.companies.push(sym);
        }
    }
    for g in &mut out {
        g.weight_pct = if total_value > 0.0 {
            g.value / total_value * 100.0
        } else {
            0.0
        };
        g.is_concentrated = g.weight_pct > GROUP_NOTICE_PCT;
        g.value = (g.value * 100.0).round() / 100.0;
        g.weight_pct = (g.weight_pct * 100.0).round() / 100.0;
    }
    out.sort_by(|a, b| b.value.total_cmp(&a.value));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_map_to_their_group_with_or_without_a_suffix() {
        assert_eq!(group_of("TCS.NS"), Some("Tata"));
        assert_eq!(group_of("titan.bo"), Some("Tata"));
        assert_eq!(group_of("ADANIPORTS"), Some("Adani"));
        assert_eq!(group_of("M&M.NS"), Some("Mahindra"));
        assert_eq!(group_of("AAPL"), None);
        assert_eq!(group_of("INFY.NS"), None);
    }

    #[test]
    fn four_tata_stocks_are_one_group() {
        let held = vec![
            ("TCS.NS".to_string(), 10_000.0),
            ("TATASTEEL.NS".to_string(), 8_000.0),
            ("TITAN.NS".to_string(), 7_000.0),
            ("TRENT.NS".to_string(), 5_000.0),
            ("INFY.NS".to_string(), 20_000.0),
            ("HDFCBANK.NS".to_string(), 5_000.0),
        ];
        let g = group_exposure(&held, 55_000.0);
        assert_eq!(g[0].group, "Tata");
        assert_eq!(g[0].value, 30_000.0);
        assert!((g[0].weight_pct - 54.55).abs() < 1e-9);
        assert!(g[0].is_concentrated);
        assert_eq!(g[0].companies, ["TCS", "TATASTEEL", "TITAN", "TRENT"]);
        assert_eq!(g[1].group, "HDFC");
        assert!(!g[1].is_concentrated);
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn no_symbol_is_in_two_groups() {
        let mut seen: Vec<&str> = Vec::new();
        for (_, members) in GROUPS {
            for m in *members {
                assert!(!seen.contains(m), "{m} is listed twice");
                seen.push(m);
            }
        }
    }
}
