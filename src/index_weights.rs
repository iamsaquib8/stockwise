//! Shipped weights of the indices the common index ETFs and index funds
//! track (NIFTY 50, NIFTY BANK, NIFTY NEXT 50, S&P 500, NASDAQ-100), for a
//! look-through when the fund's own holdings cannot be read.
//!
//! Each table lists the largest constituents with approximate weights from
//! the index providers' factsheets ([`AS_OF`]) and the index's sector split.
//! What the listed constituents leave over ([`IndexWeights::other_pct`]) is
//! spread across the rest of the index and is not attributed to any company.
//! Weights drift with prices and rebalances: they are a guide to where the
//! money sits, not exact figures.

use crate::funds::{FundProfile, Holding};

/// When the tables were taken (index factsheets).
pub const AS_OF: &str = "Oct 2025";

/// One index: its largest constituents and its sector split, in percent.
#[derive(Debug)]
pub struct IndexWeights {
    pub index: &'static str,
    /// (Yahoo symbol, name, weight %).
    pub constituents: &'static [(&'static str, &'static str, f64)],
    /// (sector, weight %), covering the whole index.
    pub sectors: &'static [(&'static str, f64)],
}

impl IndexWeights {
    /// The share of the index not in [`IndexWeights::constituents`], percent.
    pub fn other_pct(&self) -> f64 {
        (100.0 - self.constituents.iter().map(|c| c.2).sum::<f64>()).max(0.0)
    }

    /// The listed constituents as `(symbol, fraction)`.
    pub fn holdings(&self) -> Vec<(String, f64)> {
        self.constituents
            .iter()
            .map(|(s, _, w)| (s.to_string(), w / 100.0))
            .collect()
    }

    /// A fund profile for `fund` holding the index: constituents and sectors
    /// as fractions.
    pub fn fund_profile(
        &self,
        fund: &str,
        name: Option<String>,
        currency: Option<String>,
    ) -> FundProfile {
        FundProfile {
            symbol: fund.to_string(),
            name,
            family: None,
            category: Some(format!("Index fund ({})", self.index)),
            expense_ratio: None,
            currency,
            holdings: self
                .constituents
                .iter()
                .map(|(s, n, w)| Holding {
                    symbol: Some(s.to_string()),
                    name: Some(n.to_string()),
                    weight: w / 100.0,
                })
                .collect(),
            sectors: self
                .sectors
                .iter()
                .map(|(s, w)| (s.to_string(), w / 100.0))
                .collect(),
        }
    }
}

const NIFTY_50: IndexWeights = IndexWeights {
    index: "NIFTY 50",
    constituents: &[
        ("HDFCBANK.NS", "HDFC Bank", 13.1),
        ("ICICIBANK.NS", "ICICI Bank", 8.8),
        ("RELIANCE.NS", "Reliance Industries", 8.4),
        ("INFY.NS", "Infosys", 4.9),
        ("BHARTIARTL.NS", "Bharti Airtel", 4.6),
        ("LT.NS", "Larsen & Toubro", 3.9),
        ("ITC.NS", "ITC", 3.5),
        ("SBIN.NS", "State Bank of India", 3.2),
        ("AXISBANK.NS", "Axis Bank", 3.0),
        ("TCS.NS", "Tata Consultancy Services", 2.9),
        ("KOTAKBANK.NS", "Kotak Mahindra Bank", 2.7),
        ("M&M.NS", "Mahindra & Mahindra", 2.6),
        ("BAJFINANCE.NS", "Bajaj Finance", 2.3),
        ("HINDUNILVR.NS", "Hindustan Unilever", 1.9),
        ("ETERNAL.NS", "Eternal", 1.8),
        ("MARUTI.NS", "Maruti Suzuki", 1.8),
        ("SUNPHARMA.NS", "Sun Pharmaceutical", 1.6),
        ("HCLTECH.NS", "HCL Technologies", 1.4),
        ("NTPC.NS", "NTPC", 1.4),
        ("ULTRACEMCO.NS", "UltraTech Cement", 1.3),
        ("TITAN.NS", "Titan", 1.3),
        ("BEL.NS", "Bharat Electronics", 1.2),
        ("TATASTEEL.NS", "Tata Steel", 1.2),
        ("POWERGRID.NS", "Power Grid", 1.1),
    ],
    sectors: &[
        ("Financial Services", 37.5),
        ("Consumer Cyclical", 11.5),
        ("Technology", 10.5),
        ("Energy", 10.0),
        ("Consumer Defensive", 6.5),
        ("Basic Materials", 6.4),
        ("Industrials", 6.0),
        ("Communication Services", 4.6),
        ("Healthcare", 4.0),
        ("Utilities", 3.0),
    ],
};

const NIFTY_BANK: IndexWeights = IndexWeights {
    index: "NIFTY BANK",
    constituents: &[
        ("HDFCBANK.NS", "HDFC Bank", 28.0),
        ("ICICIBANK.NS", "ICICI Bank", 24.5),
        ("SBIN.NS", "State Bank of India", 9.5),
        ("KOTAKBANK.NS", "Kotak Mahindra Bank", 8.3),
        ("AXISBANK.NS", "Axis Bank", 8.3),
        ("FEDERALBNK.NS", "Federal Bank", 3.0),
        ("IDFCFIRSTB.NS", "IDFC First Bank", 3.0),
        ("BANKBARODA.NS", "Bank of Baroda", 2.9),
        ("CANBK.NS", "Canara Bank", 2.7),
        ("AUBANK.NS", "AU Small Finance Bank", 2.6),
        ("INDUSINDBK.NS", "IndusInd Bank", 2.5),
        ("PNB.NS", "Punjab National Bank", 2.5),
    ],
    sectors: &[("Financial Services", 100.0)],
};

const NIFTY_NEXT_50: IndexWeights = IndexWeights {
    index: "NIFTY NEXT 50",
    constituents: &[
        ("VEDL.NS", "Vedanta", 3.9),
        ("TVSMOTOR.NS", "TVS Motor", 3.6),
        ("DMART.NS", "Avenue Supermarts", 3.0),
        ("PFC.NS", "Power Finance Corporation", 3.0),
        ("BAJAJHLDNG.NS", "Bajaj Holdings", 3.0),
        ("DIVISLAB.NS", "Divi's Laboratories", 3.0),
        ("LODHA.NS", "Lodha Developers", 2.5),
        ("TORNTPHARM.NS", "Torrent Pharmaceuticals", 2.5),
    ],
    sectors: &[
        ("Financial Services", 27.0),
        ("Consumer Cyclical", 14.0),
        ("Industrials", 12.0),
        ("Basic Materials", 10.0),
        ("Energy", 9.0),
        ("Consumer Defensive", 9.0),
        ("Utilities", 8.0),
        ("Healthcare", 6.0),
        ("Technology", 3.0),
        ("Communication Services", 2.0),
    ],
};

const SP_500: IndexWeights = IndexWeights {
    index: "S&P 500",
    constituents: &[
        ("NVDA", "NVIDIA", 7.8),
        ("MSFT", "Microsoft", 6.8),
        ("AAPL", "Apple", 6.6),
        ("AMZN", "Amazon", 3.8),
        ("META", "Meta Platforms", 2.9),
        ("AVGO", "Broadcom", 2.7),
        ("GOOGL", "Alphabet (A)", 2.5),
        ("TSLA", "Tesla", 2.2),
        ("GOOG", "Alphabet (C)", 2.0),
        ("BRK-B", "Berkshire Hathaway", 1.6),
        ("JPM", "JPMorgan Chase", 1.5),
        ("LLY", "Eli Lilly", 1.2),
        ("V", "Visa", 1.0),
        ("ORCL", "Oracle", 1.0),
        ("XOM", "Exxon Mobil", 0.9),
        ("MA", "Mastercard", 0.8),
        ("NFLX", "Netflix", 0.8),
        ("WMT", "Walmart", 0.8),
        ("COST", "Costco", 0.7),
        ("JNJ", "Johnson & Johnson", 0.7),
    ],
    sectors: &[
        ("Technology", 35.3),
        ("Financial Services", 13.0),
        ("Consumer Cyclical", 10.5),
        ("Communication Services", 10.0),
        ("Healthcare", 9.0),
        ("Industrials", 8.3),
        ("Consumer Defensive", 4.8),
        ("Energy", 2.9),
        ("Utilities", 2.4),
        ("Real Estate", 2.0),
        ("Basic Materials", 1.8),
    ],
};

const NASDAQ_100: IndexWeights = IndexWeights {
    index: "NASDAQ 100",
    constituents: &[
        ("NVDA", "NVIDIA", 9.8),
        ("MSFT", "Microsoft", 8.5),
        ("AAPL", "Apple", 8.4),
        ("AMZN", "Amazon", 5.4),
        ("AVGO", "Broadcom", 5.3),
        ("META", "Meta Platforms", 3.7),
        ("TSLA", "Tesla", 3.4),
        ("GOOGL", "Alphabet (A)", 3.2),
        ("GOOG", "Alphabet (C)", 3.0),
        ("NFLX", "Netflix", 2.3),
        ("COST", "Costco", 2.3),
        ("PLTR", "Palantir", 2.0),
        ("AMD", "Advanced Micro Devices", 1.8),
        ("CSCO", "Cisco", 1.5),
    ],
    sectors: &[
        ("Technology", 55.0),
        ("Communication Services", 15.0),
        ("Consumer Cyclical", 13.0),
        ("Healthcare", 5.0),
        ("Consumer Defensive", 5.0),
        ("Industrials", 4.0),
        ("Utilities", 1.5),
        ("Basic Materials", 1.0),
        ("Energy", 0.5),
    ],
};

const TABLES: [&IndexWeights; 5] = [&NIFTY_50, &NIFTY_BANK, &NIFTY_NEXT_50, &SP_500, &NASDAQ_100];

/// The shipped table for `index` ("NIFTY 50", "S&P 500", ...).
pub fn weights(index: &str) -> Option<&'static IndexWeights> {
    let want = index.trim().to_ascii_uppercase().replace('-', " ");
    TABLES.iter().copied().find(|t| t.index == want)
}

/// Normalize separators (including Unicode dashes) and repeated whitespace so
/// weighting qualifiers like `Equal–Weight` match reliably.
fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_space = true;
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == '&' {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Whether `words` contains `pat` as an exact supported benchmark (not an
/// index variant such as `Nifty50 Value 20`, `Nifty 50 Equal Weight`, or
/// `S&P 500 Growth`).
fn has_exact_benchmark(words: &[&str], pat: &[&str], allow_junior: bool) -> bool {
    let n = pat.len();
    words.windows(n).enumerate().any(|(i, win)| {
        if win != pat {
            return false;
        }
        if i > 0 && matches!(words[i - 1], "ex" | "non" | "anti" | "equal" | "weight") {
            return false;
        }
        if (pat == ["next", "50"] || pat == ["next50"])
            && i > 0
            && matches!(words[i - 1], "bse" | "sensex")
        {
            return false;
        }
        match words.get(i + n).copied() {
            None => true,
            Some(
                "index" | "etf" | "bees" | "fund" | "fof" | "plan" | "scheme" | "tri" | "pr"
                | "trust" | "ucits",
            ) => true,
            Some("junior") if allow_junior => matches!(
                words.get(i + n + 1).copied(),
                None | Some("index" | "etf" | "bees" | "fund" | "fof" | "plan")
            ),
            Some("exchange") => words.get(i + n + 1).copied() == Some("traded"),
            _ => false,
        }
    })
}

/// The index a fund tracks, from its symbol (a known ETF) or, for an index
/// fund, its scheme name ("... Nifty 50 Index Fund"). Returns `None` for
/// equal-weight and other unsupported benchmark variants so they never receive
/// cap-weighted constituent tables.
pub fn index_of_fund(symbol: &str, name: Option<&str>) -> Option<&'static str> {
    if let Some(i) = crate::xray::index_for_etf(symbol) {
        return Some(i);
    }
    let norm = normalize_name(name?);
    let words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
    let indexed = words.iter().any(|&w| matches!(w, "index" | "etf" | "bees"));
    // An equal-weight fund holds the same names in other proportions: the
    // cap-weighted tables would misstate it.
    if !indexed || norm.contains("equal weight") || norm.contains("equalweight") {
        return None;
    }
    if has_exact_benchmark(&words, &["nifty", "next", "50"], true)
        || has_exact_benchmark(&words, &["nifty", "next50"], true)
        || has_exact_benchmark(&words, &["next", "50"], true)
        || has_exact_benchmark(&words, &["next50"], true)
    {
        Some("NIFTY NEXT 50")
    } else if has_exact_benchmark(&words, &["nifty", "bank"], false)
        || has_exact_benchmark(&words, &["bank", "nifty"], false)
    {
        Some("NIFTY BANK")
    } else if has_exact_benchmark(&words, &["nifty", "50"], false)
        || has_exact_benchmark(&words, &["nifty50"], false)
    {
        Some("NIFTY 50")
    } else if has_exact_benchmark(&words, &["s&p", "500"], false)
        || has_exact_benchmark(&words, &["s&p500"], false)
    {
        Some("S&P 500")
    } else if has_exact_benchmark(&words, &["nasdaq", "100"], false)
        || has_exact_benchmark(&words, &["nasdaq100"], false)
    {
        Some("NASDAQ 100")
    } else {
        None
    }
}

/// Whether `symbol`/`name` identifies an index fund or ETF (including
/// equal-weight and unsupported index variants that have no shipped table).
pub fn is_index_fund(symbol: &str, name: Option<&str>) -> bool {
    if index_of_fund(symbol, name).is_some() {
        return true;
    }
    let Some(raw) = name else {
        return false;
    };
    let norm = normalize_name(raw);
    let words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
    words.iter().any(|&w| matches!(w, "etf" | "bees"))
        || (words.contains(&"index")
            && (words.iter().any(|&w| {
                matches!(
                    w,
                    "fund" | "fof" | "plan" | "scheme" | "direct" | "regular" | "growth" | "idcw"
                )
            }) || words.iter().any(|&w| {
                matches!(
                    w,
                    "nifty"
                        | "nifty50"
                        | "s&p"
                        | "s&p500"
                        | "nasdaq"
                        | "nasdaq100"
                        | "sensex"
                        | "bse"
                        | "msci"
                        | "ftse"
                        | "russell"
                )
            })))
}

/// The shipped weights for the index `symbol`/`name` tracks, if any.
pub fn for_fund(symbol: &str, name: Option<&str>) -> Option<&'static IndexWeights> {
    index_of_fund(symbol, name).and_then(weights)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nifty_500_and_equal_weight_funds_get_no_nifty_50_table() {
        assert_eq!(
            index_of_fund("MF:3", Some("Motilal Oswal Nifty 500 Index Fund")),
            None
        );
        assert_eq!(
            index_of_fund("MF:4", Some("DSP Nifty 50 Equal Weight Index Fund")),
            None
        );
        assert_eq!(
            index_of_fund("MF:4B", Some("DSP Nifty 50 Equal\u{2013}Weight Index Fund")),
            None
        );
        assert_eq!(
            index_of_fund("MF:6", Some("ICICI Prudential Nifty50 Value 20 Index Fund")),
            None
        );
        assert_eq!(index_of_fund("MF:7", Some("SPDR S&P 500 Growth ETF")), None);
        assert!(is_index_fund(
            "EQWT",
            Some("DSP Nifty 50 Equal\u{2013}Weight Index Fund")
        ));
        assert!(is_index_fund(
            "VAL20",
            Some("ICICI Prudential Nifty50 Value 20 Index Fund")
        ));
        assert!(!is_index_fund("TCS.NS", Some("Tata Consultancy Services")));
        assert_eq!(
            index_of_fund("MF:5", Some("UTI Nifty 50 Index Fund - Direct")),
            Some("NIFTY 50")
        );
    }

    #[test]
    fn tables_add_up() {
        for t in TABLES {
            let sectors: f64 = t.sectors.iter().map(|s| s.1).sum();
            assert!(
                (sectors - 100.0).abs() < 0.05,
                "{} sectors {sectors}",
                t.index
            );
            let listed: f64 = t.constituents.iter().map(|c| c.2).sum();
            assert!(listed <= 100.0 && t.other_pct() >= 0.0, "{}", t.index);
        }
    }

    #[test]
    fn funds_find_their_index() {
        assert_eq!(index_of_fund("NIFTYBEES.NS", None), Some("NIFTY 50"));
        assert_eq!(index_of_fund("SPY", None), Some("S&P 500"));
        assert_eq!(index_of_fund("QQQ", None), Some("NASDAQ 100"));
        assert_eq!(
            index_of_fund("MF:120716", Some("UTI Nifty 50 Index Fund - Direct Plan")),
            Some("NIFTY 50")
        );
        assert_eq!(
            index_of_fund("MF:1", Some("ICICI Prudential Nifty Next 50 Index Fund")),
            Some("NIFTY NEXT 50")
        );
        // An active fund naming its benchmark is not an index fund.
        assert_eq!(
            index_of_fund("MF:2", Some("Some Bluechip Fund (Nifty 50 TRI)")),
            None
        );
        assert!(weights("nasdaq-100").is_some());
        let hdfc = for_fund("NIFTYBEES.NS", None).unwrap().constituents[0];
        assert_eq!(hdfc.0, "HDFCBANK.NS");
        assert!((hdfc.2 - 13.0).abs() < 1.0);
    }
}
