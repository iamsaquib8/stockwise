use crate::clock::{HolidayTable, Session, TzRule};
use crate::i18n::{Msg, t};
use crate::tax;
use clap::ValueEnum;
use clap::builder::PossibleValue;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Market {
    /// US stock market (NYSE/NASDAQ)
    Us,
    /// Indian stock market (NSE/BSE)
    In,
    /// Crypto, trades 24/7: BTC → BTC-USD (BTC-INR with --quote-currency inr)
    Crypto,
    /// Currency pairs, 24x5: USDINR → USDINR=X
    Fx,
    /// Gold and silver: COMEX futures (GOLD → GC=F), Indian gold/silver
    /// ETFs and Sovereign Gold Bonds (bare tickers → NSE)
    Commodity,
    /// A stock exchange from the global market table (`-m uk`, `-m jp`, ...)
    Global(Exchange),
}

/// Every `-m` value, in `--help` order.
const ALL_MARKETS: [Market; 12] = [
    Market::Us,
    Market::In,
    Market::Crypto,
    Market::Fx,
    Market::Commodity,
    Market::Global(Exchange::Uk),
    Market::Global(Exchange::De),
    Market::Global(Exchange::Jp),
    Market::Global(Exchange::Hk),
    Market::Global(Exchange::Sg),
    Market::Global(Exchange::Ca),
    Market::Global(Exchange::Au),
];

// Written by hand (not derived) because the global exchanges come from the
// market table: `-m uk` is `Market::Global(Exchange::Uk)`.
impl ValueEnum for Market {
    fn value_variants<'a>() -> &'a [Self] {
        &ALL_MARKETS
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Market::Us => PossibleValue::new("us").help("US stock market (NYSE/NASDAQ)"),
            Market::In => PossibleValue::new("in")
                .alias("india")
                .help("Indian stock market (NSE/BSE)"),
            Market::Crypto => PossibleValue::new("crypto")
                .help("Crypto, trades 24/7: BTC → BTC-USD (BTC-INR with --quote-currency inr)"),
            Market::Fx => PossibleValue::new("fx")
                .alias("forex")
                .help("Currency pairs, 24x5: USDINR → USDINR=X"),
            Market::Commodity => PossibleValue::new("commodity")
                .aliases(["commodities", "gold"])
                .help("Gold and silver: GOLD → GC=F (COMEX); ETFs and SGBs → NSE"),
            Market::Global(x) => {
                let info = x.info();
                PossibleValue::new(info.code)
                    .aliases(info.aliases.iter().copied())
                    .help(info.help)
            }
        })
    }
}

/// A stock exchange outside India and the US. Everything StockWise knows
/// about it lives in its [`MarketInfo`] row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Exchange {
    Uk,
    De,
    Jp,
    Hk,
    Sg,
    Ca,
    Au,
}

impl Exchange {
    pub const ALL: [Exchange; 7] = [
        Exchange::Uk,
        Exchange::De,
        Exchange::Jp,
        Exchange::Hk,
        Exchange::Sg,
        Exchange::Ca,
        Exchange::Au,
    ];

    /// This exchange's row in [`GLOBAL_MARKETS`].
    pub fn info(self) -> &'static MarketInfo {
        &GLOBAL_MARKETS[self as usize]
    }
}

/// One row of the market table: symbols, currency, benchmark and sector
/// lists, session hours and the holiday calendar of a stock exchange.
pub struct MarketInfo {
    /// The `-m` value (`uk`) and, upper-cased, the id saved in data files.
    pub code: &'static str,
    /// Other `-m` spellings (`gb`, `lse`, `london`).
    pub aliases: &'static [&'static str],
    /// One line for `--help`.
    pub help: &'static str,
    /// English country name; the stable key in JSON (`United Kingdom`).
    pub country: &'static str,
    /// Short localized name for titles.
    pub title: Msg,
    /// `London Stock Exchange`.
    pub exchange_name: &'static str,
    /// `LSE`.
    pub short: &'static str,
    /// Yahoo Finance suffix bare tickers get (`VOD` → `VOD.L`).
    pub suffix: &'static str,
    /// Numeric tickers are zero-padded to this many digits (HKEX: `5` →
    /// `0005.HK`); 0 leaves them alone.
    pub pad_digits: usize,
    /// ISO code of the market's currency.
    pub currency: &'static str,
    /// The unit Yahoo quotes shares in, when it is a minor unit of
    /// `currency` (London: `GBp`, pence).
    pub quote_unit: Option<&'static str>,
    /// Benchmark indices; the first is the market's main index.
    pub indices: &'static [(&'static str, &'static str)],
    /// Sector indices or ETFs, where Yahoo carries a usable set.
    pub sectors: &'static [(&'static str, &'static str)],
    /// Large caps scanned by movers, screeners and the bots.
    pub universe: &'static [&'static str],
    /// Time zone and regular trading hours.
    pub session: Session,
    /// Trading holidays and half days (see `clock.rs`).
    pub holidays: HolidayTable,
    /// Capital-gains rules `stockwise tax` can apply, if any.
    pub tax: Option<tax::Country>,
}

impl MarketInfo {
    /// The Yahoo symbol for a bare ticker on this exchange.
    pub fn yahoo_symbol(&self, ticker: &str) -> String {
        let ticker = ticker.trim().to_ascii_uppercase();
        let numeric = !ticker.is_empty() && ticker.chars().all(|c| c.is_ascii_digit());
        let ticker = if self.pad_digits > 0 && numeric {
            // `00700` and `700` are both Tencent: 0700.HK.
            let bare = ticker.trim_start_matches('0');
            format!("{:0>width$}", bare, width = self.pad_digits)
        } else {
            ticker
        };
        format!("{}{}", ticker, self.suffix)
    }

    /// The market's main index (`^FTSE`, "FTSE 100").
    pub fn benchmark(&self) -> (&'static str, &'static str) {
        self.indices[0]
    }
}

/// The global market table, in [`Exchange`] order. Symbols were checked
/// against Yahoo Finance; holiday tables live in `clock.rs`.
pub static GLOBAL_MARKETS: [MarketInfo; 7] = [
    MarketInfo {
        code: "uk",
        aliases: &["gb", "lse", "london"],
        help: "London Stock Exchange: VOD → VOD.L (prices in pence, GBp)",
        country: "United Kingdom",
        title: Msg::MarketUk,
        exchange_name: "London Stock Exchange",
        short: "LSE",
        suffix: ".L",
        pad_digits: 0,
        currency: "GBP",
        quote_unit: Some("GBp"),
        indices: &[
            ("^FTSE", "FTSE 100"),
            ("^FTMC", "FTSE 250"),
            ("^FTAS", "FTSE All-Share"),
        ],
        sectors: &[],
        universe: &[
            "SHEL.L", "AZN.L", "HSBA.L", "ULVR.L", "BP.L", "RIO.L", "GSK.L", "REL.L", "DGE.L",
            "BATS.L", "LSEG.L", "VOD.L", "BARC.L", "LLOY.L", "NG.L",
        ],
        session: Session {
            tz: TzRule::Eu(0),
            tz_label: "London",
            open: (8, 0),
            close: (16, 30),
            lunch: None,
            half_day_close: Some((12, 30)),
        },
        holidays: crate::clock::LSE,
        tax: Some(tax::Country::Uk),
    },
    MarketInfo {
        code: "de",
        aliases: &["germany", "xetra", "frankfurt"],
        help: "Deutsche Börse Xetra: SAP → SAP.DE (EUR)",
        country: "Germany",
        title: Msg::MarketDe,
        exchange_name: "Deutsche Börse Xetra",
        short: "Xetra",
        suffix: ".DE",
        pad_digits: 0,
        currency: "EUR",
        quote_unit: None,
        indices: &[
            ("^GDAXI", "DAX"),
            ("^MDAXI", "MDAX"),
            ("^SDAXI", "SDAX"),
            ("^TECDAX", "TecDAX"),
        ],
        sectors: &[],
        universe: &[
            "SAP.DE", "SIE.DE", "ALV.DE", "DTE.DE", "MBG.DE", "BMW.DE", "BAS.DE", "MUV2.DE",
            "IFX.DE", "ADS.DE", "DBK.DE", "VOW3.DE", "AIR.DE", "RHM.DE", "DHL.DE",
        ],
        session: Session {
            tz: TzRule::Eu(1),
            tz_label: "Frankfurt",
            open: (9, 0),
            close: (17, 30),
            lunch: None,
            half_day_close: None,
        },
        holidays: crate::clock::XETRA,
        tax: None,
    },
    MarketInfo {
        code: "jp",
        aliases: &["japan", "tse", "tokyo"],
        help: "Tokyo Stock Exchange: 7203 → 7203.T (JPY)",
        country: "Japan",
        title: Msg::MarketJp,
        exchange_name: "Tokyo Stock Exchange",
        short: "TSE",
        suffix: ".T",
        pad_digits: 0,
        currency: "JPY",
        quote_unit: None,
        indices: &[("^N225", "Nikkei 225"), ("1306.T", "TOPIX (1306 ETF)")],
        // NEXT FUNDS TOPIX-17 sector ETFs.
        sectors: &[
            ("1617.T", "Foods"),
            ("1618.T", "Energy"),
            ("1619.T", "Construction"),
            ("1620.T", "Materials & Chem."),
            ("1621.T", "Pharma"),
            ("1622.T", "Autos"),
            ("1623.T", "Steel & Metals"),
            ("1624.T", "Machinery"),
            ("1625.T", "Electronics"),
            ("1626.T", "IT & Services"),
            ("1627.T", "Power & Gas"),
            ("1628.T", "Transport"),
            ("1629.T", "Trading Cos."),
            ("1630.T", "Retail"),
            ("1631.T", "Banks"),
            ("1632.T", "Financials"),
            ("1633.T", "Real Estate"),
        ],
        universe: &[
            "7203.T", "6758.T", "8306.T", "9984.T", "6861.T", "9432.T", "8035.T", "6501.T",
            "7974.T", "9983.T", "4063.T", "8316.T", "6098.T", "7267.T",
        ],
        session: Session {
            tz: TzRule::Fixed(9 * 60),
            tz_label: "JST",
            open: (9, 0),
            close: (15, 30),
            lunch: Some(((11, 30), (12, 30))),
            half_day_close: None,
        },
        holidays: crate::clock::JPX,
        tax: None,
    },
    MarketInfo {
        code: "hk",
        aliases: &["hongkong", "hkex"],
        help: "Hong Kong Exchange: 5 → 0005.HK, 700 → 0700.HK (HKD)",
        country: "Hong Kong",
        title: Msg::MarketHk,
        exchange_name: "Hong Kong Exchanges",
        short: "HKEX",
        suffix: ".HK",
        pad_digits: 4,
        currency: "HKD",
        quote_unit: None,
        indices: &[
            ("^HSI", "Hang Seng"),
            ("^HSCE", "HS China Enterprises"),
            ("HSTECH.HK", "Hang Seng TECH"),
        ],
        sectors: &[
            ("^HSNF", "Finance"),
            ("^HSNU", "Utilities"),
            ("^HSNP", "Properties"),
            ("^HSNC", "Commerce & Industry"),
        ],
        universe: &[
            "0700.HK", "9988.HK", "0005.HK", "1299.HK", "0941.HK", "3690.HK", "0388.HK", "1810.HK",
            "0939.HK", "1398.HK", "2318.HK", "0883.HK", "0001.HK", "0016.HK",
        ],
        session: Session {
            tz: TzRule::Fixed(8 * 60),
            tz_label: "HKT",
            open: (9, 30),
            close: (16, 0),
            lunch: Some(((12, 0), (13, 0))),
            half_day_close: Some((12, 0)),
        },
        holidays: crate::clock::HKEX,
        tax: None,
    },
    MarketInfo {
        code: "sg",
        aliases: &["singapore", "sgx"],
        help: "Singapore Exchange: D05 → D05.SI (SGD)",
        country: "Singapore",
        title: Msg::MarketSg,
        exchange_name: "Singapore Exchange",
        short: "SGX",
        suffix: ".SI",
        pad_digits: 0,
        currency: "SGD",
        quote_unit: None,
        indices: &[("^STI", "Straits Times")],
        sectors: &[],
        universe: &[
            "D05.SI", "O39.SI", "U11.SI", "Z74.SI", "C6L.SI", "S68.SI", "C38U.SI", "A17U.SI",
            "BN4.SI", "F34.SI", "Y92.SI", "G13.SI", "S63.SI", "9CI.SI",
        ],
        session: Session {
            tz: TzRule::Fixed(8 * 60),
            tz_label: "SGT",
            open: (9, 0),
            close: (17, 0),
            lunch: Some(((12, 0), (13, 0))),
            half_day_close: Some((12, 0)),
        },
        holidays: crate::clock::SGX,
        tax: None,
    },
    MarketInfo {
        code: "ca",
        aliases: &["canada", "tsx", "toronto"],
        help: "Toronto Stock Exchange: RY → RY.TO (CAD)",
        country: "Canada",
        title: Msg::MarketCa,
        exchange_name: "Toronto Stock Exchange",
        short: "TSX",
        suffix: ".TO",
        pad_digits: 0,
        currency: "CAD",
        quote_unit: None,
        indices: &[("^GSPTSE", "S&P/TSX Composite")],
        // iShares S&P/TSX capped sector ETFs.
        sectors: &[
            ("XFN.TO", "Financials"),
            ("XEG.TO", "Energy"),
            ("XMA.TO", "Materials"),
            ("XIT.TO", "Technology"),
            ("XUT.TO", "Utilities"),
            ("XST.TO", "Cons. Staples"),
            ("XRE.TO", "Real Estate"),
            ("XGD.TO", "Gold Miners"),
        ],
        universe: &[
            "RY.TO", "TD.TO", "SHOP.TO", "ENB.TO", "CNR.TO", "CP.TO", "BN.TO", "BMO.TO", "BNS.TO",
            "CNQ.TO", "SU.TO", "TRI.TO", "ATD.TO", "CSU.TO", "MFC.TO",
        ],
        session: Session {
            tz: TzRule::UsEastern,
            tz_label: "ET",
            open: (9, 30),
            close: (16, 0),
            lunch: None,
            half_day_close: Some((13, 0)),
        },
        holidays: crate::clock::TSX,
        tax: None,
    },
    MarketInfo {
        code: "au",
        aliases: &["australia", "asx", "sydney"],
        help: "Australian Securities Exchange: BHP → BHP.AX (AUD)",
        country: "Australia",
        title: Msg::MarketAu,
        exchange_name: "Australian Securities Exchange",
        short: "ASX",
        suffix: ".AX",
        pad_digits: 0,
        currency: "AUD",
        quote_unit: None,
        indices: &[
            ("^AXJO", "S&P/ASX 200"),
            ("^AORD", "All Ordinaries"),
            ("^AXKO", "S&P/ASX 300"),
        ],
        // S&P/ASX 200 sector indices.
        sectors: &[
            ("^AXFJ", "Financials"),
            ("^AXMJ", "Materials"),
            ("^AXEJ", "Energy"),
            ("^AXHJ", "Health Care"),
            ("^AXIJ", "Info Tech"),
            ("^AXDJ", "Cons. Discret."),
            ("^AXSJ", "Cons. Staples"),
            ("^AXNJ", "Industrials"),
            ("^AXUJ", "Utilities"),
            ("^AXPJ", "A-REIT"),
            ("^AXTJ", "Communication"),
        ],
        universe: &[
            "BHP.AX", "CBA.AX", "CSL.AX", "NAB.AX", "WBC.AX", "ANZ.AX", "MQG.AX", "WES.AX",
            "WDS.AX", "FMG.AX", "RIO.AX", "TLS.AX", "WOW.AX", "GMG.AX", "TCL.AX",
        ],
        session: Session {
            tz: TzRule::AuEastern,
            tz_label: "Sydney",
            open: (10, 0),
            close: (16, 0),
            lunch: None,
            half_day_close: Some((14, 10)),
        },
        holidays: crate::clock::ASX,
        tax: None,
    },
];

/// The global exchange a Yahoo symbol trades on: from its suffix (`VOD.L`
/// → UK) or, for an index, from the market table (`^N225` → Japan).
pub fn exchange_of(symbol: &str) -> Option<Exchange> {
    let upper = symbol.trim().to_ascii_uppercase();
    Exchange::ALL.into_iter().find(|x| {
        let info = x.info();
        upper.ends_with(info.suffix)
            || info
                .indices
                .iter()
                .chain(info.sectors)
                .any(|(s, _)| s.eq_ignore_ascii_case(&upper))
    })
}

/// What kind of instrument a Yahoo symbol is. Decides how prices are
/// formatted (crypto and FX need more decimals) and whether exchange hours
/// apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetClass {
    Stock,
    Index,
    Crypto,
    Fx,
}

/// Currencies a crypto pair can be quoted in on Yahoo (`BTC-USD`, `ETH-INR`).
const CRYPTO_QUOTES: &[&str] = &["USD", "INR", "EUR", "GBP", "JPY", "USDT", "BTC", "ETH"];

impl AssetClass {
    pub fn of(symbol: &str) -> Self {
        let upper = symbol.trim().to_ascii_uppercase();
        if upper.starts_with('^') {
            return AssetClass::Index;
        }
        if upper.ends_with("=X") {
            return AssetClass::Fx;
        }
        // `BTC-USD` is crypto, `BRK-B` and `BAJAJ-AUTO.NS` are stocks.
        if !upper.contains('.')
            && let Some((base, quote)) = upper.rsplit_once('-')
            && !base.is_empty()
            && CRYPTO_QUOTES.contains(&quote)
        {
            return AssetClass::Crypto;
        }
        AssetClass::Stock
    }

    /// Whether the instrument trades around the clock (no exchange session).
    pub fn trades_24x7(self) -> bool {
        self == AssetClass::Crypto
    }
}

static QUOTE_CURRENCY: OnceLock<String> = OnceLock::new();

/// Set the currency bare crypto tickers are quoted in (`--quote-currency`).
/// Only the first call wins; it is made once at startup.
pub fn set_quote_currency(code: &str) {
    let code = code.trim().to_ascii_uppercase();
    if !code.is_empty() {
        let _ = QUOTE_CURRENCY.set(code);
    }
}

/// Currency for bare crypto tickers: `USD` unless `--quote-currency` or
/// `STOCKWISE_QUOTE_CURRENCY` says otherwise.
pub fn quote_currency() -> &'static str {
    QUOTE_CURRENCY.get().map(String::as_str).unwrap_or("USD")
}

/// Normalise a user-typed symbol for Yahoo Finance.
///
/// In the Indian market a bare ticker gets the NSE suffix (`TCS` → `TCS.NS`)
/// and a bare BSE scrip code the BSE one (`500325` → `500325.BO`; NSE
/// symbols are never all digits). Symbols that already say where they trade
/// are passed through untouched:
/// an exchange suffix (`RELIANCE.BO`, `VOD.L`), an index (`^NSEI`), or a
/// currency/futures pair (`USDINR=X`, `BTC-USD`).
///
/// In the crypto market a bare coin gets the quote currency (`BTC` →
/// `BTC-USD`); in the FX market a pair gets Yahoo's `=X` suffix (`USDINR`,
/// `usd/inr` → `USDINR=X`). A global exchange adds its suffix from the
/// market table (`-m uk VOD` → `VOD.L`, `-m hk 5` → `0005.HK`). The
/// commodity market knows `GOLD` and `SILVER` (COMEX futures `GC=F`,
/// `SI=F`); other bare tickers are NSE-listed ETFs and bonds (`GOLDBEES` →
/// `GOLDBEES.NS`).
///
/// In every market a symbol in a StockWise namespace (`MF:120503`,
/// `MF:INF179K01UT0`) is kept as typed, upper-cased: it is not a Yahoo
/// ticker, so no exchange suffix belongs on it.
pub fn resolve_symbol(symbol: &str, market: Market) -> String {
    resolve_symbol_in(symbol, market, quote_currency())
}

/// `MF:120503` (a mutual fund by AMFI code), `MF:INF179K01UT0` (by ISIN): a
/// `PREFIX:ID` symbol StockWise prices itself. Yahoo tickers never contain
/// a colon.
pub fn is_namespaced(symbol: &str) -> bool {
    symbol.contains(':')
}

/// `PREFIX:ID` upper-cased, with stray spaces around the colon removed.
fn namespaced(upper: &str) -> Option<String> {
    let (prefix, id) = upper.split_once(':')?;
    Some(format!("{}:{}", prefix.trim(), id.trim()))
}

/// A namespaced symbol that an older StockWise stored with a market suffix
/// (`MF:120503.NS`, `MF:120503-USD`, `MF:INF179K01UT0.L`), repaired; None
/// when `symbol` needs no repair. Idempotent: a repaired symbol has no
/// suffix left to strip.
pub fn repair_namespaced(symbol: &str) -> Option<String> {
    let upper = symbol.trim().to_uppercase();
    if !is_namespaced(&upper) {
        return None;
    }
    let crypto: Vec<String> = CRYPTO_QUOTES.iter().map(|q| format!("-{q}")).collect();
    let suffixes = [".NS", ".BO"]
        .into_iter()
        .chain(GLOBAL_MARKETS.iter().map(|m| m.suffix))
        .chain(crypto.iter().map(String::as_str));
    for suffix in suffixes {
        if let Some(base) = upper.strip_suffix(suffix)
            && base.split_once(':').is_some_and(|(_, id)| !id.is_empty())
        {
            return namespaced(base);
        }
    }
    namespaced(&upper).filter(|fixed| *fixed != symbol)
}

fn resolve_symbol_in(symbol: &str, market: Market, crypto_quote: &str) -> String {
    let upper = symbol.trim().to_uppercase();
    if let Some(ns) = namespaced(&upper) {
        return ns;
    }
    let explicit = upper.contains('.')
        || upper.starts_with('^')
        || upper.contains('=')
        || upper.ends_with("-USD")
        || upper.ends_with("-INR");
    match market {
        Market::Us => upper,
        Market::In if explicit => upper,
        Market::In if !upper.is_empty() && upper.chars().all(|c| c.is_ascii_digit()) => {
            format!("{}.BO", upper)
        }
        Market::In => format!("{}.NS", upper),
        Market::Crypto if upper.contains('-') || explicit => upper,
        Market::Crypto => format!("{}-{}", upper, crypto_quote),
        Market::Fx if upper.contains('=') || upper.starts_with('^') => upper,
        Market::Fx => {
            let pair: String = upper
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            format!("{}=X", pair)
        }
        Market::Commodity if explicit => upper,
        Market::Commodity => match commodity_future(&upper) {
            Some(future) => future.to_string(),
            None => format!("{}.NS", upper),
        },
        Market::Global(_) if explicit => upper,
        Market::Global(x) => x.info().yahoo_symbol(&upper),
    }
}

/// COMEX futures for the metal names `-m commodity` understands.
pub fn commodity_future(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_uppercase().as_str() {
        "GOLD" | "XAU" | "XAUUSD" => Some("GC=F"),
        "SILVER" | "XAG" | "XAGUSD" => Some("SI=F"),
        _ => None,
    }
}

/// Hint shown when a symbol is not found, pointing at the most common cause:
/// a US ticker looked up in the default Indian market.
pub fn not_found_hint(market: Market) -> &'static str {
    t(match market {
        Market::In => Msg::HintNotFoundIn,
        Market::Us => Msg::HintNotFoundUs,
        Market::Crypto => Msg::HintNotFoundCrypto,
        Market::Fx => Msg::HintNotFoundFx,
        Market::Commodity => Msg::HintNotFoundCommodity,
        Market::Global(_) => Msg::HintNotFoundGlobal,
    })
}

impl Market {
    /// Short name for titles: "India", "US", "Crypto", "FX", "Japan".
    pub fn label(self) -> &'static str {
        match self {
            Market::In => "India",
            Market::Us => "US",
            Market::Crypto => "Crypto",
            Market::Fx => "FX",
            Market::Commodity => "Commodities",
            Market::Global(x) => x.info().country,
        }
    }

    /// The `-m` spelling (`in`, `us`, `uk`, `commodity`).
    pub fn code(self) -> &'static str {
        match self {
            Market::In => "in",
            Market::Us => "us",
            Market::Crypto => "crypto",
            Market::Fx => "fx",
            Market::Commodity => "commodity",
            Market::Global(x) => x.info().code,
        }
    }

    /// The upper-case id saved in data files (`IN`, `US`, `UK`). Parse it
    /// back with `Market::from_str(id, true)`.
    pub fn id(self) -> &'static str {
        match self {
            Market::In => "IN",
            Market::Us => "US",
            Market::Crypto => "CRYPTO",
            Market::Fx => "FX",
            Market::Commodity => "COMMODITY",
            Market::Global(x) => match x {
                Exchange::Uk => "UK",
                Exchange::De => "DE",
                Exchange::Jp => "JP",
                Exchange::Hk => "HK",
                Exchange::Sg => "SG",
                Exchange::Ca => "CA",
                Exchange::Au => "AU",
            },
        }
    }

    /// The market table row of a global exchange; None for India, the US,
    /// crypto, FX and commodities.
    pub fn info(self) -> Option<&'static MarketInfo> {
        match self {
            Market::Global(x) => Some(x.info()),
            _ => None,
        }
    }

    /// Whether this is a stock exchange (with sessions, sectors, fundamentals).
    pub fn is_stock(self) -> bool {
        matches!(self, Market::In | Market::Us | Market::Global(_))
    }

    /// Fail clearly for commands built on stock-exchange concepts — trading
    /// sessions, sectors, fundamentals — instead of running them on coins or
    /// currency pairs.
    pub fn require_stocks(self, command: &str) -> anyhow::Result<()> {
        if self.is_stock() {
            return Ok(());
        }
        anyhow::bail!(
            "`{}` works on stock markets only (-m in, -m us or a global exchange such as -m uk); {} {}",
            command,
            self.label(),
            match self {
                Market::Crypto => "trade 24/7 without sessions, sectors or fundamentals",
                Market::Commodity =>
                    "are futures, ETFs and bonds without sectors or fundamentals — try `stockwise commodity`",
                _ => "trades 24x5 without exchange sessions, sectors or fundamentals",
            }
        )
    }

    /// Like [`Market::require_stocks`], for the bots and the simulator: they
    /// size positions from the capital you type, so they also need prices
    /// in the currency's major unit (London quotes in pence).
    pub fn require_bot_support(self, command: &str) -> anyhow::Result<()> {
        self.require_stocks(command)?;
        if let Some(info) = self.info()
            && let Some(unit) = info.quote_unit
        {
            anyhow::bail!(
                "`{}` sizes positions from the capital you give, and {} prices are quoted in {} \
                 ({} / 100) — not supported for -m {} yet. Quotes, charts, technicals, \
                 backtests and alerts work",
                command,
                info.short,
                unit,
                info.currency,
                info.code
            );
        }
        Ok(())
    }

    /// Like [`Market::require_stocks`], for commands built on sector
    /// indices: some exchanges have no usable set on Yahoo.
    pub fn require_sectors(self, command: &str) -> anyhow::Result<()> {
        self.require_stocks(command)?;
        if self.sectors().is_empty() {
            let with: Vec<&str> = Exchange::ALL
                .iter()
                .filter(|x| !x.info().sectors.is_empty())
                .map(|x| x.info().code)
                .collect();
            anyhow::bail!(
                "`{}` needs sector indices or ETFs, and Yahoo Finance has no usable set for {} — \
                 it works with -m in, -m us and -m {}",
                command,
                self.label(),
                with.join(", -m ")
            );
        }
        Ok(())
    }

    /// Currency symbol for amounts the user types (capital, SIP budget).
    pub fn currency_symbol(self) -> &'static str {
        match self {
            Market::In | Market::Commodity => "\u{20B9}",
            Market::Us | Market::Fx => "$",
            Market::Crypto => currency_symbol(Some(quote_currency())),
            Market::Global(x) => currency_symbol(Some(x.info().currency)),
        }
    }

    /// Symbols scanned by movers, screeners and the bots.
    pub fn universe(self) -> Vec<String> {
        let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect();
        match self {
            Market::In => owned(INDIA_POPULAR),
            Market::Us => owned(US_POPULAR),
            Market::Crypto => crypto_majors().into_iter().map(|(s, _)| s).collect(),
            Market::Fx => FX_MAJORS.iter().map(|(s, _)| s.to_string()).collect(),
            Market::Commodity => COMMODITY_UNIVERSE
                .iter()
                .map(|(s, _)| s.to_string())
                .collect(),
            Market::Global(x) => owned(x.info().universe),
        }
    }

    /// Sector indices or ETFs; empty for crypto, FX, commodities and
    /// exchanges without a set on Yahoo.
    pub fn sectors(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Market::In => INDIA_SECTOR_INDICES,
            Market::Us => US_SECTOR_ETFS,
            Market::Crypto | Market::Fx | Market::Commodity => &[],
            Market::Global(x) => x.info().sectors,
        }
    }
}

/// The largest coins, quoted in the active quote currency.
pub fn crypto_majors() -> Vec<(String, &'static str)> {
    let quote = quote_currency();
    CRYPTO_MAJORS
        .iter()
        .map(|(coin, name)| (format!("{}-{}", coin, quote), *name))
        .collect()
}

/// A one-line note on trading hours for 24/7 and 24x5 instruments, or
/// `None` for exchange-listed ones (whose sessions Yahoo already reflects).
pub fn session_note(class: AssetClass, now: chrono::DateTime<chrono::Utc>) -> Option<Msg> {
    use chrono::{Datelike, Timelike, Weekday};
    match class {
        AssetClass::Crypto => Some(Msg::SessionCrypto),
        AssetClass::Fx => {
            // The FX week runs from Sunday ~21:00 UTC to Friday ~21:00 UTC.
            let closed = match now.weekday() {
                Weekday::Sat => true,
                Weekday::Fri => now.hour() >= 21,
                Weekday::Sun => now.hour() < 21,
                _ => false,
            };
            Some(if closed {
                Msg::SessionFxWeekend
            } else {
                Msg::SessionFxOpen
            })
        }
        AssetClass::Stock | AssetClass::Index => None,
    }
}

/// Get the currency symbol for display. London prices come in pence
/// (`GBp`), shown as `GBp 118.20` here; `display::format_price` writes
/// `118.20p`.
pub fn currency_symbol(currency: Option<&str>) -> &'static str {
    match currency {
        Some("INR") => "\u{20B9}",
        Some("EUR") => "\u{20AC}",
        Some("GBP") => "\u{00A3}",
        Some("GBp") | Some("GBX") => "GBp ",
        Some("JPY") => "\u{00A5}",
        Some("HKD") => "HK$",
        Some("SGD") => "S$",
        Some("CAD") => "C$",
        Some("AUD") => "A$",
        _ => "$",
    }
}

/// A minor currency unit Yahoo quotes in, as (major currency, minor units
/// per major unit): London's pence `GBp` (also written `GBX`) → (`GBP`, 100).
pub fn minor_unit(currency: &str) -> Option<(&'static str, f64)> {
    match currency {
        "GBp" | "GBX" => Some(("GBP", 100.0)),
        _ => None,
    }
}

/// `amount` in `currency` expressed in the major unit: 118.2 GBp → 1.182 GBP.
/// Other currencies are returned unchanged.
pub fn to_major(amount: f64, currency: &str) -> (f64, String) {
    match minor_unit(currency) {
        Some((major, per)) => (amount / per, major.to_string()),
        None => (amount, currency.to_string()),
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
    "AAPL", "MSFT", "GOOG", "AMZN", "NVDA", "META", "TSLA", "BRK-B", "JPM", "V", "UNH", "XOM",
    "JNJ", "WMT", "MA", "PG", "HD", "COST", "MRK", "ABBV", "CRM", "AVGO", "PEP", "KO", "TMO",
    "ADBE", "NFLX", "CSCO", "ACN", "AMD",
];

pub const INDIA_POPULAR: &[&str] = &[
    "RELIANCE.NS",
    "TCS.NS",
    "HDFCBANK.NS",
    "INFY.NS",
    "ICICIBANK.NS",
    "HINDUNILVR.NS",
    "SBIN.NS",
    "BHARTIARTL.NS",
    "ITC.NS",
    "KOTAKBANK.NS",
    "LT.NS",
    "AXISBANK.NS",
    "ASIANPAINT.NS",
    "MARUTI.NS",
    "TITAN.NS",
    "SUNPHARMA.NS",
    "TMPV.NS", // Tata Motors Passenger Vehicles; TATAMOTORS.NS no longer quotes after the demerger
    "BAJFINANCE.NS",
    "WIPRO.NS",
    "HCLTECH.NS",
    "ONGC.NS",
    "NTPC.NS",
    "POWERGRID.NS",
    "TATASTEEL.NS",
    "ADANIENT.NS",
    "ULTRACEMCO.NS",
    "JSWSTEEL.NS",
    "TECHM.NS",
    "INDUSINDBK.NS",
    "DRREDDY.NS",
];

/// An NSE index whose members a scan covers (`movers`, `heatmap`, `screen`
/// with `--universe`). The member list comes from NSE; the built-in lists
/// below are the offline fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Universe {
    /// NIFTY 50
    #[value(name = "nifty50")]
    Nifty50,
    /// NIFTY NEXT 50
    #[value(name = "niftynext50", alias = "next50")]
    NiftyNext50,
    /// NIFTY 100
    #[value(name = "nifty100")]
    Nifty100,
    /// NIFTY BANK
    #[value(name = "banknifty", alias = "niftybank")]
    BankNifty,
    /// NIFTY IT
    #[value(name = "niftyit")]
    NiftyIt,
}

impl Universe {
    /// NSE's name for the index.
    pub fn nse_index(self) -> &'static str {
        match self {
            Universe::Nifty50 => "NIFTY 50",
            Universe::NiftyNext50 => "NIFTY NEXT 50",
            Universe::Nifty100 => "NIFTY 100",
            Universe::BankNifty => "NIFTY BANK",
            Universe::NiftyIt => "NIFTY IT",
        }
    }

    /// What to scan when NSE can't list the members: the built-in large
    /// caps (all NIFTY 50 members, so also inside NIFTY 100), or a built-in
    /// list of the sector's members. `None` for NIFTY NEXT 50, which has no
    /// built-in list.
    pub fn fallback(self) -> Option<Vec<String>> {
        let list: &[&str] = match self {
            Universe::Nifty50 | Universe::Nifty100 => INDIA_POPULAR,
            Universe::BankNifty => BANK_NIFTY,
            Universe::NiftyIt => NIFTY_IT,
            Universe::NiftyNext50 => return None,
        };
        Some(list.iter().map(|s| s.to_string()).collect())
    }
}

/// NIFTY BANK members (2025), the offline fallback for `--universe banknifty`.
pub const BANK_NIFTY: &[&str] = &[
    "HDFCBANK.NS",
    "ICICIBANK.NS",
    "SBIN.NS",
    "KOTAKBANK.NS",
    "AXISBANK.NS",
    "INDUSINDBK.NS",
    "BANKBARODA.NS",
    "PNB.NS",
    "CANBK.NS",
    "FEDERALBNK.NS",
    "IDFCFIRSTB.NS",
    "AUBANK.NS",
];

/// NIFTY IT members (2025), the offline fallback for `--universe niftyit`.
pub const NIFTY_IT: &[&str] = &[
    "TCS.NS",
    "INFY.NS",
    "HCLTECH.NS",
    "WIPRO.NS",
    "TECHM.NS",
    "LTIM.NS",
    "PERSISTENT.NS",
    "COFORGE.NS",
    "MPHASIS.NS",
    "OFSS.NS",
];

/// Coins for `markets --crypto` and the crypto universe, without the quote
/// currency (added by `crypto_majors`).
pub const CRYPTO_MAJORS: &[(&str, &str)] = &[
    ("BTC", "Bitcoin"),
    ("ETH", "Ethereum"),
    ("SOL", "Solana"),
    ("XRP", "XRP"),
    ("BNB", "BNB"),
    ("DOGE", "Dogecoin"),
    ("ADA", "Cardano"),
    ("TRX", "TRON"),
];

/// Currency pairs for `markets --fx` and the FX universe.
pub const FX_MAJORS: &[(&str, &str)] = &[
    ("USDINR=X", "USD/INR"),
    ("EURINR=X", "EUR/INR"),
    ("GBPINR=X", "GBP/INR"),
    ("EURUSD=X", "EUR/USD"),
    ("GBPUSD=X", "GBP/USD"),
    ("USDJPY=X", "USD/JPY"),
];

/// Gold and silver for `-m commodity`: COMEX futures (USD per troy ounce)
/// and NSE-listed ETFs (₹ per unit).
pub const COMMODITY_UNIVERSE: &[(&str, &str)] = &[
    ("GC=F", "Gold (COMEX)"),
    ("SI=F", "Silver (COMEX)"),
    ("GOLDBEES.NS", "Nippon India Gold BeES"),
    ("SETFGOLD.NS", "SBI Gold ETF"),
    ("HDFCGOLD.NS", "HDFC Gold ETF"),
    ("SILVERBEES.NS", "Nippon India Silver ETF"),
];

/// Predefined screener categories
pub const SCREENER_CATEGORIES: &[&str] =
    &["undervalued", "growth", "dividend", "momentum", "bluechip"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_symbol_us() {
        assert_eq!(resolve_symbol("aapl", Market::Us), "AAPL");
        assert_eq!(resolve_symbol("MSFT", Market::Us), "MSFT");
    }

    #[test]
    fn test_resolve_symbol_india() {
        assert_eq!(resolve_symbol("reliance", Market::In), "RELIANCE.NS");
        assert_eq!(resolve_symbol("TCS", Market::In), "TCS.NS");
        // Already suffixed
        assert_eq!(resolve_symbol("RELIANCE.NS", Market::In), "RELIANCE.NS");
        assert_eq!(resolve_symbol("RELIANCE.BO", Market::In), "RELIANCE.BO");
    }

    #[test]
    fn test_currency_symbol() {
        assert_eq!(currency_symbol(Some("INR")), "\u{20B9}");
        assert_eq!(currency_symbol(Some("USD")), "$");
        assert_eq!(currency_symbol(None), "$");
    }

    #[test]
    fn test_is_inr() {
        assert!(is_inr(Some("INR")));
        assert!(!is_inr(Some("USD")));
        assert!(!is_inr(None));
    }

    #[test]
    fn test_resolve_symbol_india_passes_through_explicit_symbols() {
        assert_eq!(resolve_symbol("^nsei", Market::In), "^NSEI");
        assert_eq!(resolve_symbol("^GSPC", Market::In), "^GSPC");
        assert_eq!(resolve_symbol("usdinr=x", Market::In), "USDINR=X");
        assert_eq!(resolve_symbol("btc-usd", Market::In), "BTC-USD");
        assert_eq!(resolve_symbol("vod.l", Market::In), "VOD.L");
        assert_eq!(resolve_symbol(" m&m ", Market::In), "M&M.NS");
        assert_eq!(resolve_symbol("BAJAJ-AUTO", Market::In), "BAJAJ-AUTO.NS");
    }

    #[test]
    fn bare_scrip_codes_resolve_to_bse() {
        assert_eq!(resolve_symbol("500325", Market::In), "500325.BO");
        assert_eq!(resolve_symbol(" 532540 ", Market::In), "532540.BO");
        // Tickers that merely start with digits stay on NSE.
        assert_eq!(resolve_symbol("3MINDIA", Market::In), "3MINDIA.NS");
        assert_eq!(resolve_symbol("500325.NS", Market::In), "500325.NS");
        assert_eq!(resolve_symbol("500325", Market::Us), "500325");
    }

    #[test]
    fn universes_name_nse_indices_with_offline_fallbacks() {
        assert_eq!(Universe::Nifty50.nse_index(), "NIFTY 50");
        assert_eq!(Universe::BankNifty.nse_index(), "NIFTY BANK");
        assert_eq!(
            Universe::from_str("next50", true),
            Ok(Universe::NiftyNext50)
        );
        assert_eq!(Universe::from_str("NIFTY100", true), Ok(Universe::Nifty100));
        assert_eq!(
            Universe::Nifty50.fallback().unwrap().len(),
            INDIA_POPULAR.len()
        );
        assert!(
            Universe::BankNifty
                .fallback()
                .unwrap()
                .iter()
                .all(|s| s.ends_with(".NS"))
        );
        assert!(Universe::NiftyNext50.fallback().is_none());
    }

    #[test]
    fn test_resolve_symbol_us_lowercase() {
        assert_eq!(resolve_symbol("tsla", Market::Us), "TSLA");
    }

    #[test]
    fn test_resolve_symbol_india_bo_suffix() {
        // .BO suffix preserved
        assert_eq!(resolve_symbol("RELIANCE.BO", Market::In), "RELIANCE.BO");
    }

    #[test]
    fn test_resolve_symbol_india_lowercase_with_ns() {
        // If already has .NS, don't double-append
        assert_eq!(resolve_symbol("tcs.ns", Market::In), "TCS.NS");
    }

    #[test]
    fn test_resolve_symbol_us_already_upper() {
        assert_eq!(resolve_symbol("AAPL", Market::Us), "AAPL");
    }

    #[test]
    fn test_currency_symbol_other_currencies() {
        assert_eq!(currency_symbol(Some("GBP")), "\u{00A3}");
        assert_eq!(currency_symbol(Some("EUR")), "\u{20AC}");
        assert_eq!(currency_symbol(Some("JPY")), "\u{00A5}");
        // Unknown currencies fall back to $
        assert_eq!(currency_symbol(Some("CHF")), "$");
    }

    #[test]
    fn test_popular_lists_non_empty() {
        assert!(!US_POPULAR.is_empty());
        assert!(!INDIA_POPULAR.is_empty());
    }

    #[test]
    fn test_india_popular_has_ns_suffix() {
        // All India popular symbols should have .NS suffix
        for sym in INDIA_POPULAR {
            assert!(sym.ends_with(".NS"), "{} missing .NS suffix", sym);
        }
    }

    #[test]
    fn test_screener_categories_non_empty() {
        assert!(!SCREENER_CATEGORIES.is_empty());
        assert!(SCREENER_CATEGORIES.contains(&"undervalued"));
        assert!(SCREENER_CATEGORIES.contains(&"dividend"));
    }

    #[test]
    fn crypto_resolves_bare_coins_to_the_quote_currency() {
        assert_eq!(resolve_symbol_in("btc", Market::Crypto, "USD"), "BTC-USD");
        assert_eq!(resolve_symbol_in("ETH", Market::Crypto, "INR"), "ETH-INR");
        // Full pairs and other explicit symbols pass through.
        assert_eq!(
            resolve_symbol_in("btc-inr", Market::Crypto, "USD"),
            "BTC-INR"
        );
        assert_eq!(
            resolve_symbol_in("ETH-EUR", Market::Crypto, "USD"),
            "ETH-EUR"
        );
        assert_eq!(resolve_symbol_in("^GSPC", Market::Crypto, "USD"), "^GSPC");
    }

    #[test]
    fn namespaced_symbols_pass_through_in_every_market() {
        for &market in Market::value_variants() {
            for (typed, stored) in [
                ("MF:120503", "MF:120503"),
                ("mf:inf179k01ut0", "MF:INF179K01UT0"),
                (" MF: 120503 ", "MF:120503"),
            ] {
                assert_eq!(
                    resolve_symbol_in(typed, market, "USD"),
                    stored,
                    "{typed} in {}",
                    market.code()
                );
            }
        }
        assert!(is_namespaced("MF:120503") && !is_namespaced("TCS.NS"));
    }

    #[test]
    fn repairs_market_suffixes_on_namespaced_symbols() {
        for (stored, fixed) in [
            ("MF:120503.NS", "MF:120503"),
            ("MF:120503.BO", "MF:120503"),
            ("MF:INF179K01UT0.NS", "MF:INF179K01UT0"),
            ("MF:120503-USD", "MF:120503"),
            ("MF:120503-INR", "MF:120503"),
            ("MF:120503.L", "MF:120503"),
            ("MF:120503.HK", "MF:120503"),
            ("mf:120503", "MF:120503"),
        ] {
            assert_eq!(
                repair_namespaced(stored).as_deref(),
                Some(fixed),
                "{stored}"
            );
            // Idempotent: the repaired symbol needs nothing more.
            assert_eq!(repair_namespaced(fixed), None, "{fixed}");
        }
        for untouched in ["MF:120503", "TCS.NS", "BTC-USD", "VOD.L", "MF:.NS"] {
            assert_eq!(repair_namespaced(untouched), None, "{untouched}");
        }
    }

    #[test]
    fn fx_resolves_pairs_to_yahoo_symbols() {
        assert_eq!(resolve_symbol_in("USDINR", Market::Fx, "USD"), "USDINR=X");
        assert_eq!(resolve_symbol_in("usd/inr", Market::Fx, "USD"), "USDINR=X");
        assert_eq!(resolve_symbol_in("eur-usd", Market::Fx, "USD"), "EURUSD=X");
        assert_eq!(resolve_symbol_in("USDINR=X", Market::Fx, "USD"), "USDINR=X");
        assert_eq!(resolve_symbol_in("INR", Market::Fx, "USD"), "INR=X");
    }

    #[test]
    fn asset_class_from_symbol() {
        assert_eq!(AssetClass::of("BTC-USD"), AssetClass::Crypto);
        assert_eq!(AssetClass::of("eth-inr"), AssetClass::Crypto);
        assert_eq!(AssetClass::of("USDINR=X"), AssetClass::Fx);
        assert_eq!(AssetClass::of("^NSEI"), AssetClass::Index);
        assert_eq!(AssetClass::of("BRK-B"), AssetClass::Stock);
        assert_eq!(AssetClass::of("BAJAJ-AUTO.NS"), AssetClass::Stock);
        assert_eq!(AssetClass::of("TCS.NS"), AssetClass::Stock);
        assert!(AssetClass::of("BTC-USD").trades_24x7());
        assert!(!AssetClass::of("USDINR=X").trades_24x7());
    }

    #[test]
    fn stock_only_commands_reject_crypto_and_fx() {
        assert!(Market::In.require_stocks("sectors").is_ok());
        assert!(Market::Us.require_stocks("sectors").is_ok());
        let err = Market::Crypto
            .require_stocks("sectors")
            .unwrap_err()
            .to_string();
        assert!(err.contains("24/7"), "{err}");
        assert!(Market::Fx.require_stocks("intraday").is_err());
        assert!(Market::Crypto.sectors().is_empty());
    }

    #[test]
    fn universes_match_the_market() {
        assert!(Market::In.universe().iter().all(|s| s.ends_with(".NS")));
        assert!(Market::Fx.universe().iter().all(|s| s.ends_with("=X")));
        assert!(
            Market::Crypto
                .universe()
                .iter()
                .all(|s| AssetClass::of(s) == AssetClass::Crypto)
        );
    }

    #[test]
    fn global_exchanges_resolve_bare_tickers_with_their_suffix() {
        let m = |x| Market::Global(x);
        assert_eq!(resolve_symbol("VOD", m(Exchange::Uk)), "VOD.L");
        assert_eq!(resolve_symbol("sap", m(Exchange::De)), "SAP.DE");
        assert_eq!(resolve_symbol("7203", m(Exchange::Jp)), "7203.T");
        // HKEX codes are zero-padded to four digits on Yahoo.
        assert_eq!(resolve_symbol("5", m(Exchange::Hk)), "0005.HK");
        assert_eq!(resolve_symbol("700", m(Exchange::Hk)), "0700.HK");
        assert_eq!(resolve_symbol("9988", m(Exchange::Hk)), "9988.HK");
        assert_eq!(resolve_symbol("00700", m(Exchange::Hk)), "0700.HK");
        assert_eq!(resolve_symbol("80737", m(Exchange::Hk)), "80737.HK");
        assert_eq!(resolve_symbol("d05", m(Exchange::Sg)), "D05.SI");
        assert_eq!(resolve_symbol("RY", m(Exchange::Ca)), "RY.TO");
        assert_eq!(resolve_symbol(" bhp ", m(Exchange::Au)), "BHP.AX");
        // Explicit symbols pass through, as in the other markets.
        assert_eq!(resolve_symbol("^ftse", m(Exchange::Uk)), "^FTSE");
        assert_eq!(resolve_symbol("TCS.NS", m(Exchange::Uk)), "TCS.NS");
        assert_eq!(resolve_symbol("USDJPY=X", m(Exchange::Jp)), "USDJPY=X");
    }

    #[test]
    fn commodity_market_knows_metals_and_sends_the_rest_to_nse() {
        assert_eq!(resolve_symbol("gold", Market::Commodity), "GC=F");
        assert_eq!(resolve_symbol("XAU", Market::Commodity), "GC=F");
        assert_eq!(resolve_symbol("silver", Market::Commodity), "SI=F");
        assert_eq!(resolve_symbol("goldbees", Market::Commodity), "GOLDBEES.NS");
        assert_eq!(
            resolve_symbol("SGBAUG28V", Market::Commodity),
            "SGBAUG28V.NS"
        );
        assert_eq!(resolve_symbol("GC=F", Market::Commodity), "GC=F");
        assert_eq!(resolve_symbol("MGC=F", Market::Commodity), "MGC=F");
        assert!(Market::Commodity.require_stocks("sectors").is_err());
        assert!(Market::Commodity.sectors().is_empty());
        let err = Market::Global(Exchange::Uk)
            .require_sectors("sectors")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("United Kingdom") && err.contains("-m jp"),
            "{err}"
        );
        assert!(
            Market::Global(Exchange::Au)
                .require_sectors("sectors")
                .is_ok()
        );
        assert!(Market::In.require_sectors("rotation").is_ok());
        // Bots size positions in pounds; London quotes in pence.
        let err = Market::Global(Exchange::Uk)
            .require_bot_support("intraday")
            .unwrap_err()
            .to_string();
        assert!(err.contains("GBp") && err.contains("-m uk"), "{err}");
        assert!(
            Market::Global(Exchange::Jp)
                .require_bot_support("intraday")
                .is_ok()
        );
        assert!(Market::Crypto.require_bot_support("daemon").is_err());
    }

    #[test]
    fn every_market_parses_from_its_code_aliases_and_id() {
        for m in ALL_MARKETS {
            assert_eq!(Market::from_str(m.code(), true), Ok(m), "{m:?}");
            assert_eq!(Market::from_str(m.id(), true), Ok(m), "{m:?}");
            assert_eq!(m.id(), m.code().to_uppercase());
        }
        assert_eq!(
            Market::from_str("UK", true),
            Ok(Market::Global(Exchange::Uk))
        );
        assert_eq!(
            Market::from_str("lse", true),
            Ok(Market::Global(Exchange::Uk))
        );
        assert_eq!(
            Market::from_str("japan", true),
            Ok(Market::Global(Exchange::Jp))
        );
        assert_eq!(Market::from_str("india", true), Ok(Market::In));
        assert_eq!(Market::from_str("forex", true), Ok(Market::Fx));
        assert_eq!(Market::from_str("gold", true), Ok(Market::Commodity));
        assert!(Market::from_str("mars", true).is_err());
        // No two markets share a spelling.
        let mut names: Vec<String> = Vec::new();
        for m in ALL_MARKETS {
            let pv = m.to_possible_value().unwrap();
            for name in pv.get_name_and_aliases() {
                assert!(!names.contains(&name.to_string()), "{name} twice");
                names.push(name.to_string());
            }
        }
    }

    #[test]
    fn market_table_rows_are_consistent() {
        for (i, x) in Exchange::ALL.into_iter().enumerate() {
            let info = x.info();
            assert_eq!(x as usize, i);
            assert_eq!(Market::Global(x).code(), info.code);
            assert!(info.suffix.starts_with('.'), "{}", info.code);
            assert!(!info.indices.is_empty(), "{} has no index", info.code);
            assert!(
                info.universe.iter().all(|s| s.ends_with(info.suffix)),
                "{} universe",
                info.code
            );
            assert!(info.universe.len() >= 10, "{} universe", info.code);
            assert_eq!(exchange_of(info.universe[0]), Some(x));
            assert_eq!(Market::Global(x).universe().len(), info.universe.len());
            assert!(Market::Global(x).is_stock());
            let (oh, om) = info.session.open;
            let (ch, cm) = info.session.close;
            assert!((oh, om) < (ch, cm), "{} hours", info.code);
            // Tax rules exist only for India, the US and the UK.
            assert_eq!(
                info.tax.is_some(),
                x == Exchange::Uk,
                "{} tax treatment",
                info.code
            );
        }
        assert_eq!(exchange_of("^HSI"), Some(Exchange::Hk));
        assert_eq!(exchange_of("^AXFJ"), Some(Exchange::Au));
        assert_eq!(exchange_of("RELIANCE.NS"), None);
        assert_eq!(exchange_of("AAPL"), None);
    }

    #[test]
    fn currency_symbols_and_minor_units() {
        assert_eq!(currency_symbol(Some("HKD")), "HK$");
        assert_eq!(currency_symbol(Some("SGD")), "S$");
        assert_eq!(currency_symbol(Some("CAD")), "C$");
        assert_eq!(currency_symbol(Some("AUD")), "A$");
        assert_eq!(currency_symbol(Some("GBp")), "GBp ");
        assert_eq!(minor_unit("GBp"), Some(("GBP", 100.0)));
        assert_eq!(minor_unit("GBX"), Some(("GBP", 100.0)));
        assert_eq!(minor_unit("GBP"), None);
        let (pounds, cur) = to_major(118.2, "GBp");
        assert!((pounds - 1.182).abs() < 1e-12);
        assert_eq!(cur, "GBP");
        assert_eq!(to_major(214.4, "EUR"), (214.4, "EUR".to_string()));
        assert_eq!(Market::Global(Exchange::Jp).currency_symbol(), "\u{00A5}");
        assert_eq!(Market::Commodity.currency_symbol(), "\u{20B9}");
    }

    #[test]
    fn session_notes_know_crypto_and_fx_hours() {
        use chrono::TimeZone;
        let saturday = chrono::Utc
            .with_ymd_and_hms(2026, 10, 24, 12, 0, 0)
            .unwrap();
        let tuesday = chrono::Utc
            .with_ymd_and_hms(2026, 10, 20, 12, 0, 0)
            .unwrap();
        let sunday_evening = chrono::Utc
            .with_ymd_and_hms(2026, 10, 25, 22, 0, 0)
            .unwrap();
        assert_eq!(
            session_note(AssetClass::Fx, saturday),
            Some(Msg::SessionFxWeekend)
        );
        assert_eq!(
            session_note(AssetClass::Fx, tuesday),
            Some(Msg::SessionFxOpen)
        );
        assert_eq!(
            session_note(AssetClass::Fx, sunday_evening),
            Some(Msg::SessionFxOpen)
        );
        assert_eq!(
            session_note(AssetClass::Crypto, saturday),
            Some(Msg::SessionCrypto)
        );
        assert_eq!(session_note(AssetClass::Stock, tuesday), None);
    }
}
