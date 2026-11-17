//! Picking the market for a symbol when `-m` was not given.
//!
//! `stockwise NVDA` with India as the default market used to look up
//! `NVDA.NS`. Here a symbol says where it trades when it can, all offline:
//!
//! * an exchange suffix or index name (`VOD.L`, `TCS.NS`, `^GSPC`, `BTC-USD`);
//! * an ISIN: a built-in table maps well-known ones to their ticker, and
//!   otherwise the country prefix picks the market (`IN` → India, `US` → US,
//!   `GB` → London, ...);
//! * a bare ticker found in the built-in lists of exactly one market
//!   (S&P 1500, NIFTY 500, the global index lists, the large coins, the
//!   metals).
//!
//! When a symbol fits several markets, the current market is kept and a
//! one-line hint names the others. Nothing here makes a network request.

use crate::market::{self, Exchange, Market};
use std::collections::HashMap;
use std::sync::OnceLock;

/// What the resolver decided for one symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The symbol to look up: the input, or the ticker a known ISIN maps to.
    pub symbol: String,
    /// The market the symbol belongs to; the current one when unsure.
    pub market: Market,
    /// A one-line note for the user: where the symbol was found, or which
    /// other markets list it.
    pub hint: Option<String>,
    /// Whether the symbol itself names its market: an exchange suffix or
    /// index, an ISIN, or a built-in list. False when the market is only
    /// the current one by default.
    pub found: bool,
}

/// Well-known ISINs and their Yahoo symbols, so they resolve offline.
const ISIN_DIRECTORY: &[(&str, &str)] = &[
    // US
    ("US0378331005", "AAPL"),
    ("US5949181045", "MSFT"),
    ("US67066G1040", "NVDA"),
    ("US0231351067", "AMZN"),
    ("US02079K1079", "GOOG"),
    ("US02079K3059", "GOOGL"),
    ("US30303M1027", "META"),
    ("US88160R1014", "TSLA"),
    // India (NSE)
    ("INE002A01018", "RELIANCE.NS"),
    ("INE467B01029", "TCS.NS"),
    ("INE040A01034", "HDFCBANK.NS"),
    ("INE009A01021", "INFY.NS"),
    ("INE090A01021", "ICICIBANK.NS"),
    ("INE062A01020", "SBIN.NS"),
    // Elsewhere
    ("GB00BH4HKS39", "VOD.L"),
    ("DE0007164600", "SAP.DE"),
    ("JP3633400001", "7203.T"),
    // Vanguard FTSE All-World UCITS ETF (Acc), domiciled in Ireland.
    ("IE00BK5BQT80", "VWCE.DE"),
];

/// The market whose exchanges list most securities with this ISIN country
/// prefix. Countries without their own market here (Ireland, Luxembourg,
/// the Netherlands, ...) return `None`: their funds and companies list on
/// several exchanges.
pub fn isin_country_market(cc: &str) -> Option<Market> {
    Some(match cc {
        "IN" => Market::In,
        "US" => Market::Us,
        "GB" => Market::Global(Exchange::Uk),
        "DE" => Market::Global(Exchange::De),
        "JP" => Market::Global(Exchange::Jp),
        "HK" => Market::Global(Exchange::Hk),
        "SG" => Market::Global(Exchange::Sg),
        "CA" => Market::Global(Exchange::Ca),
        "AU" => Market::Global(Exchange::Au),
        _ => return None,
    })
}

/// Whether `s` is an ISIN: two letters, nine letters or digits, and a check
/// digit that matches (ISO 6166).
pub fn is_isin(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 12
        || !b[..2].iter().all(u8::is_ascii_uppercase)
        || !b[2..11]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        || !b[11].is_ascii_digit()
    {
        return false;
    }
    // Letters become two digits (A = 10), then the Luhn check runs over all.
    let digits: Vec<u32> = b
        .iter()
        .flat_map(|&c| {
            let v = if c.is_ascii_digit() {
                u32::from(c - b'0')
            } else {
                u32::from(c - b'A') + 10
            };
            if v >= 10 {
                vec![v / 10, v % 10]
            } else {
                vec![v]
            }
        })
        .collect();
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let x = d * 2;
                x / 10 + x % 10
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Bare tickers and the markets whose built-in lists hold them.
pub struct Directory(HashMap<String, Vec<Market>>);

impl Directory {
    /// An empty directory; fill it with [`Directory::add`].
    pub fn new() -> Self {
        Directory(HashMap::new())
    }

    /// Record that `market` lists `symbol` (a Yahoo symbol or bare ticker;
    /// the exchange suffix is dropped).
    pub fn add(&mut self, symbol: &str, market: Market) {
        let bare = bare_ticker(symbol);
        if bare.is_empty() || bare.starts_with('^') {
            return;
        }
        let markets = self.0.entry(bare).or_default();
        if !markets.contains(&market) {
            markets.push(market);
        }
    }

    /// Markets that list `bare`, in the order they were added.
    pub fn markets(&self, bare: &str) -> &[Market] {
        self.0.get(bare).map_or(&[], Vec::as_slice)
    }

    /// The directory built from StockWise's own lists.
    pub fn builtin() -> &'static Directory {
        static DIR: OnceLock<Directory> = OnceLock::new();
        DIR.get_or_init(|| {
            let mut d = Directory::new();
            for s in crate::nifty500::NIFTY500
                .iter()
                .chain(market::INDIA_POPULAR)
                .chain(market::BANK_NIFTY)
                .chain(market::NIFTY_IT)
            {
                d.add(s, Market::In);
            }
            for (s, _) in market::SYMBOL_DIRECTORY {
                let m = if s.ends_with(".NS") {
                    Market::In
                } else {
                    Market::Us
                };
                d.add(s, m);
            }
            for s in crate::sp500::SP500
                .iter()
                .chain(crate::global_lists::SP400)
                .chain(crate::global_lists::SP600)
                .chain(market::US_POPULAR)
            {
                d.add(s, Market::Us);
            }
            for x in Exchange::ALL {
                for s in x.info().universe {
                    d.add(s, Market::Global(x));
                }
            }
            for (s, _) in market::CRYPTO_MAJORS {
                d.add(s, Market::Crypto);
            }
            for (s, _) in market::FX_MAJORS {
                d.add(s, Market::Fx);
            }
            for s in ["GOLD", "SILVER", "XAU", "XAG", "XAUUSD", "XAGUSD"] {
                d.add(s, Market::Commodity);
            }
            d
        })
    }
}

impl Default for Directory {
    fn default() -> Self {
        Self::new()
    }
}

/// `VOD.L` → `VOD`, `USDINR=X` → `USDINR`, `BRK-B` stays.
fn bare_ticker(symbol: &str) -> String {
    let upper = symbol.trim().to_ascii_uppercase();
    let upper = upper.strip_suffix("=X").unwrap_or(&upper);
    upper.split('.').next().unwrap_or(upper).to_string()
}

/// Whether a symbol already names its exchange or instrument type.
fn is_explicit(upper: &str) -> bool {
    upper.contains('.')
        || upper.starts_with('^')
        || upper.contains('=')
        || market::AssetClass::of(upper) == market::AssetClass::Crypto
}

/// Resolve `symbol` against the built-in directory; see [`resolve_with`].
pub fn resolve(symbol: &str, current: Market) -> Resolution {
    resolve_with(Directory::builtin(), symbol, current)
}

/// Pick the market for `symbol` when `-m` was not given. `current` is the
/// market from `stockwise use`, the config file or the portfolio binding.
pub fn resolve_with(dir: &Directory, symbol: &str, current: Market) -> Resolution {
    let keep = |hint: Option<String>| Resolution {
        found: false,
        symbol: symbol.to_string(),
        market: current,
        hint,
    };
    let upper = symbol.trim().to_ascii_uppercase();
    // Namespaced (`MF:120503`), numeric (BSE codes, Tokyo and Hong Kong
    // codes) and index names (`NIFTY`, `SP500`) are left to the market.
    if upper.is_empty()
        || market::is_namespaced(&upper)
        || upper.chars().all(|c| c.is_ascii_digit())
        || market::resolve_index_alias(&upper, current).is_some()
    {
        return keep(None);
    }

    if is_isin(&upper) {
        return resolve_isin(&upper, symbol, current);
    }

    if is_explicit(&upper) {
        let m = market::market_of_symbol(&upper, current);
        // `market_of_symbol` calls any suffix it does not know US; only
        // trust it when the suffix is one StockWise knows.
        let known = upper.ends_with(".NS")
            || upper.ends_with(".BO")
            || market::exchange_of(&upper).is_some()
            || matches!(m, Market::Crypto | Market::Fx | Market::Commodity)
            || upper.starts_with('^');
        return if known {
            keep_as(symbol, m)
        } else {
            keep(None)
        };
    }

    // An Indian alias (`SBI`, `ZOMATO`) is India's.
    let mut markets: Vec<Market> = dir.markets(&upper).to_vec();
    if market::normalize_india_ticker(&upper) != upper && !markets.contains(&Market::In) {
        markets.push(Market::In);
    }
    match markets.as_slice() {
        [] => keep(None),
        [only] if *only == current => keep_as(symbol, current),
        [only] => Resolution {
            found: true,
            symbol: symbol.to_string(),
            market: *only,
            hint: Some({
                let yahoo = market::resolve_symbol(&upper, *only);
                let shown = if yahoo == upper {
                    String::new()
                } else {
                    format!(" as {yahoo}")
                };
                format!(
                    "{} → {}{}; pass -m {} to keep {}",
                    upper,
                    only.label(),
                    shown,
                    current.code(),
                    current.label()
                )
            }),
        },
        many => {
            let others: Vec<String> = many
                .iter()
                .filter(|m| **m != current)
                .map(|m| format!("{} (-m {})", market::resolve_symbol(&upper, *m), m.code()))
                .collect();
            let lead = if many.contains(&current) {
                format!("{} is also listed elsewhere", upper)
            } else {
                format!(
                    "{} is listed in several markets; using {}",
                    upper,
                    current.label()
                )
            };
            keep(Some(format!(
                "{}. Did you mean {}?",
                lead,
                others.join(" or ")
            )))
        }
    }
}

fn keep_as(symbol: &str, market: Market) -> Resolution {
    Resolution {
        found: true,
        symbol: symbol.to_string(),
        market,
        hint: None,
    }
}

fn resolve_isin(upper: &str, symbol: &str, current: Market) -> Resolution {
    if let Some((_, ticker)) = ISIN_DIRECTORY.iter().find(|(i, _)| *i == upper) {
        let m = market::market_of_symbol(ticker, current);
        return Resolution {
            found: true,
            symbol: ticker.to_string(),
            market: m,
            hint: Some(format!("{} → {} ({})", upper, ticker, m.label())),
        };
    }
    let cc = &upper[..2];
    match isin_country_market(cc) {
        // Indian ISINs also name gold bonds, which `-m commodity` prices.
        Some(Market::In) if current == Market::Commodity => keep_as(symbol, current),
        Some(m) => Resolution {
            found: true,
            symbol: symbol.to_string(),
            market: m,
            hint: Some(format!(
                "{} is a {} ISIN that is not in the offline directory; \
                 `stockwise search {}` finds its ticker",
                upper,
                m.label(),
                upper
            )),
        },
        None => Resolution {
            found: false,
            symbol: symbol.to_string(),
            market: current,
            hint: Some(format!(
                "{} is an ISIN from {}, whose securities list on several exchanges; \
                 `stockwise search {}` finds its tickers",
                upper, cc, upper
            )),
        },
    }
}

/// Resolve every symbol of one command. The market changes only when all
/// symbols that say anything agree on one; otherwise it stays `current` and
/// the hints explain why.
pub fn resolve_all(symbols: &[String], current: Market) -> (Market, Vec<Resolution>) {
    let res: Vec<Resolution> = symbols.iter().map(|s| resolve(s, current)).collect();
    let mut picked: Option<Market> = None;
    let mut agree = true;
    for r in &res {
        if r.market != current {
            match picked {
                None => picked = Some(r.market),
                Some(p) if p != r.market => agree = false,
                _ => {}
            }
        }
    }
    // A symbol that stays on the current market because it is listed there,
    // or because it is listed in several, keeps the others from moving the
    // whole command. Symbols found nowhere do not count.
    let any_current = res
        .iter()
        .any(|r| r.market == current && (r.found || r.hint.is_some()));
    let market = match picked {
        Some(p) if agree && !any_current => p,
        _ => current,
    };
    (market, res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> Directory {
        let mut d = Directory::new();
        d.add("RELIANCE.NS", Market::In);
        d.add("NVDA", Market::Us);
        d.add("SHOP", Market::Us);
        d.add("SHOP.TO", Market::Global(Exchange::Ca));
        d.add("RIO.L", Market::Global(Exchange::Uk));
        d.add("RIO.AX", Market::Global(Exchange::Au));
        d.add("BTC", Market::Crypto);
        d
    }

    #[test]
    fn isin_check_digits() {
        assert!(is_isin("US0378331005"));
        assert!(is_isin("INE002A01018"));
        assert!(is_isin("IE00BK5BQT80"));
        assert!(!is_isin("US0378331006"), "bad check digit");
        assert!(!is_isin("RELIANCE"));
        assert!(!is_isin("us0378331005"), "callers upper-case first");
        for (isin, _) in ISIN_DIRECTORY {
            assert!(is_isin(isin), "{isin} has a bad check digit");
        }
    }

    #[test]
    fn known_isins_resolve_to_their_ticker() {
        let d = dir();
        let r = resolve_with(&d, "US0378331005", Market::In);
        assert_eq!((r.symbol.as_str(), r.market), ("AAPL", Market::Us));
        let r = resolve_with(&d, "ine002a01018", Market::Us);
        assert_eq!((r.symbol.as_str(), r.market), ("RELIANCE.NS", Market::In));
        let r = resolve_with(&d, "IE00BK5BQT80", Market::In);
        assert_eq!(
            (r.symbol.as_str(), r.market),
            ("VWCE.DE", Market::Global(Exchange::De))
        );
    }

    #[test]
    fn isin_country_prefix_picks_the_market() {
        let d = dir();
        // Valid check digits, not in the built-in table.
        let r = resolve_with(&d, "US5949181045", Market::In);
        assert_eq!(r.symbol, "MSFT", "in the table");
        let r = resolve_with(&d, "US4592001014", Market::In); // IBM
        assert_eq!(r.market, Market::Us);
        assert_eq!(r.symbol, "US4592001014");
        assert!(r.hint.unwrap().contains("stockwise search"));
        let r = resolve_with(&d, "GB0007980591", Market::In); // BP
        assert_eq!(r.market, Market::Global(Exchange::Uk));
        let r = resolve_with(&d, "INE154A01025", Market::Us); // ITC
        assert_eq!(r.market, Market::In);
        // Gold bonds stay with the commodity market.
        let r = resolve_with(&d, "IN0020180314", Market::Commodity);
        assert_eq!((r.market, r.hint), (Market::Commodity, None));
        // Ireland has no market of its own here: keep the current one.
        let r = resolve_with(&d, "IE00B4L5Y983", Market::Us);
        assert_eq!(r.market, Market::Us);
        assert!(r.hint.unwrap().contains("several exchanges"));
    }

    #[test]
    fn unambiguous_bare_ticker_moves_market() {
        let d = dir();
        let r = resolve_with(&d, "NVDA", Market::In);
        assert_eq!((r.symbol.as_str(), r.market), ("NVDA", Market::Us));
        assert!(r.hint.unwrap().contains("-m in"));
        let r = resolve_with(&d, "btc", Market::Us);
        assert_eq!(r.market, Market::Crypto);
        // Already on the right market: no change, no hint.
        let r = resolve_with(&d, "RELIANCE", Market::In);
        assert_eq!((r.market, r.hint), (Market::In, None));
        // Unknown tickers stay where they are.
        let r = resolve_with(&d, "ZZZQ", Market::In);
        assert_eq!((r.market, r.hint), (Market::In, None));
    }

    #[test]
    fn ambiguous_ticker_keeps_current_market_with_hint() {
        let d = dir();
        // Listed in two markets, neither current.
        let r = resolve_with(&d, "RIO", Market::In);
        assert_eq!(r.market, Market::In);
        let hint = r.hint.unwrap();
        assert!(hint.contains("Did you mean"), "{hint}");
        assert!(hint.contains("RIO.L (-m uk)"), "{hint}");
        assert!(hint.contains("RIO.AX (-m au)"), "{hint}");
        // Listed in the current market and another: stay, name the other.
        let r = resolve_with(&d, "SHOP", Market::Us);
        assert_eq!(r.market, Market::Us);
        let hint = r.hint.unwrap();
        assert!(hint.contains("SHOP.TO (-m ca)"), "{hint}");
        assert!(!hint.contains("-m us"), "{hint}");
    }

    #[test]
    fn suffixes_and_special_symbols() {
        let d = dir();
        let r = resolve_with(&d, "VOD.L", Market::In);
        assert_eq!(r.market, Market::Global(Exchange::Uk));
        let r = resolve_with(&d, "TCS.NS", Market::Us);
        assert_eq!(r.market, Market::In);
        let r = resolve_with(&d, "BTC-USD", Market::In);
        assert_eq!(r.market, Market::Crypto);
        let r = resolve_with(&d, "^GSPC", Market::In);
        assert_eq!(r.market, Market::Us);
        // Unknown suffix, numeric codes, index aliases, namespaces: untouched.
        for s in ["FOO.XX", "500325", "7203", "NIFTY", "MF:120503"] {
            let r = resolve_with(&d, s, Market::In);
            assert_eq!((r.market, r.hint), (Market::In, None), "{s}");
        }
    }

    #[test]
    fn builtin_directory_knows_global_names() {
        let d = Directory::builtin();
        assert!(d.markets("NVDA").contains(&Market::Us));
        assert!(d.markets("RELIANCE").contains(&Market::In));
        assert!(d.markets("BTC").contains(&Market::Crypto));
        assert!(d.markets("GOLD").contains(&Market::Commodity));
        assert!(d.markets("VOD").contains(&Market::Global(Exchange::Uk)));
        assert_eq!(resolve("NVDA", Market::In).market, Market::Us);
        assert_eq!(resolve("RELIANCE", Market::Us).market, Market::In);
        assert_eq!(resolve("SBI", Market::Us).market, Market::In);
        // Rio Tinto is in both the FTSE 350 and the ASX 200 lists.
        let r = resolve("RIO", Market::In);
        assert_eq!(r.market, Market::In);
        assert!(r.hint.unwrap().contains("Did you mean"));
    }

    #[test]
    fn several_symbols_move_only_together() {
        let to_vec = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (m, _) = resolve_all(&to_vec(&["NVDA", "AAPL"]), Market::In);
        assert_eq!(m, Market::Us);
        // Mixed markets: stay.
        let (m, _) = resolve_all(&to_vec(&["NVDA", "RELIANCE"]), Market::In);
        assert_eq!(m, Market::In);
        let (m, _) = resolve_all(&to_vec(&["NVDA", "BTC"]), Market::In);
        assert_eq!(m, Market::In);
        // An index of the current market counts as disagreement too.
        let (m, _) = resolve_all(&to_vec(&["^GSPC", "RELIANCE"]), Market::Us);
        assert_eq!(m, Market::Us);
        let (m, _) = resolve_all(
            &to_vec(&["VOD.L", "RELIANCE"]),
            Market::Global(Exchange::Uk),
        );
        assert_eq!(m, Market::Global(Exchange::Uk));
        // Symbols found nowhere do not hold the others back.
        let (m, _) = resolve_all(&to_vec(&["NVDA", "ZZZQ"]), Market::In);
        assert_eq!(m, Market::Us);
    }
}
