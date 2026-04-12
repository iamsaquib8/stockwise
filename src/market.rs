use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum Market {
    /// US stock market (default)
    #[value(alias = "US")]
    Us,
    /// Indian stock market (NSE/BSE)
    #[value(alias = "IN", alias = "india")]
    In,
}

/// Auto-append `.NS` for Indian market if no exchange suffix is present
pub fn resolve_symbol(symbol: &str, market: Market) -> String {
    let upper = symbol.to_uppercase();
    match market {
        Market::Us => upper,
        Market::In => {
            if upper.ends_with(".NS") || upper.ends_with(".BO") {
                upper
            } else {
                format!("{}.NS", upper)
            }
        }
    }
}

/// Get the currency symbol character for display
pub fn currency_symbol(currency: Option<&str>) -> &'static str {
    match currency {
        Some("INR") => "\u{20B9}",
        _ => "$",
    }
}

/// Check if currency is INR (for lakhs/crores formatting)
pub fn is_inr(currency: Option<&str>) -> bool {
    currency == Some("INR")
}

pub const US_INDICES: &[(&str, &str)] = &[
    ("^GSPC", "S&P 500"),
    ("^DJI", "Dow Jones"),
    ("^IXIC", "NASDAQ"),
    ("^RUT", "Russell 2000"),
    ("^VIX", "VIX"),
];

pub const INDIA_INDICES: &[(&str, &str)] = &[
    ("^NSEI", "NIFTY 50"),
    ("^BSESN", "SENSEX"),
    ("^NSEBANK", "BANK NIFTY"),
    ("^CNXIT", "NIFTY IT"),
];

pub const US_SECTOR_ETFS: &[(&str, &str)] = &[
    ("XLK", "Technology"),
    ("XLF", "Financials"),
    ("XLV", "Healthcare"),
    ("XLY", "Cons. Discret."),
    ("XLP", "Cons. Staples"),
    ("XLE", "Energy"),
    ("XLI", "Industrials"),
    ("XLB", "Materials"),
    ("XLU", "Utilities"),
    ("XLRE", "Real Estate"),
    ("XLC", "Communication"),
];

pub const INDIA_SECTOR_INDICES: &[(&str, &str)] = &[
    ("^CNXIT", "IT"),
    ("^NSEBANK", "Bank"),
    ("^CNXPHARMA", "Pharma"),
    ("^CNXAUTO", "Auto"),
    ("^CNXFMCG", "FMCG"),
    ("^CNXMETAL", "Metal"),
    ("^CNXREALTY", "Realty"),
    ("^CNXENERGY", "Energy"),
];

/// Popular large-cap symbols for the movers feature
pub const US_POPULAR: &[&str] = &[
    "AAPL", "MSFT", "GOOG", "AMZN", "NVDA", "META", "TSLA", "BRK-B", "JPM", "V",
    "UNH", "XOM", "JNJ", "WMT", "MA", "PG", "HD", "COST", "MRK", "ABBV",
    "CRM", "AVGO", "PEP", "KO", "TMO", "ADBE", "NFLX", "CSCO", "ACN", "AMD",
];

pub const INDIA_POPULAR: &[&str] = &[
    "RELIANCE.NS", "TCS.NS", "HDFCBANK.NS", "INFY.NS", "ICICIBANK.NS",
    "HINDUNILVR.NS", "SBIN.NS", "BHARTIARTL.NS", "ITC.NS", "KOTAKBANK.NS",
    "LT.NS", "AXISBANK.NS", "ASIANPAINT.NS", "MARUTI.NS", "TITAN.NS",
    "SUNPHARMA.NS", "TATAMOTORS.NS", "BAJFINANCE.NS", "WIPRO.NS", "HCLTECH.NS",
    "ONGC.NS", "NTPC.NS", "POWERGRID.NS", "TATASTEEL.NS", "ADANIENT.NS",
    "ULTRACEMCO.NS", "JSWSTEEL.NS", "TECHM.NS", "INDUSINDBK.NS", "DRREDDY.NS",
];

/// Predefined screener categories
pub const SCREENER_CATEGORIES: &[&str] = &[
    "undervalued",
    "growth",
    "dividend",
    "momentum",
    "bluechip",
];
