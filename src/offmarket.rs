//! Prices for holdings Yahoo Finance does not quote: Indian mutual funds
//! (`MF:<AMFI code>`, `MF:<ISIN>`) at AMFI's latest end-of-day NAV, and
//! Sovereign Gold Bonds at NSE's last traded price — or, when NSE has no
//! price, at the gram of gold each unit stands for, labelled as an
//! estimate. Missing data is never a price of 0: the holding stays
//! unpriced and a note says why.
//!
//! The lookups are pure and fixture-tested; [`price`] fetches what they
//! need through a [`DataSource`], so the CLI, `tui` and `serve` value these
//! holdings the same way.

use crate::api::Quote;
use crate::commodity::{self as cm, NseSgb};
use crate::funds::Scheme;
use crate::views::DataSource;
use chrono::NaiveDate;
use serde::Serialize;

/// Where a holding's price came from, as `portfolio --json` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    /// A market quote (Yahoo Finance or the configured provider).
    Quote,
    /// AMFI's end-of-day NAV.
    AmfiNav,
    /// NSE's last traded price for the SGB.
    NseSgb,
    /// No SGB price: one gram of gold at the international spot price in ₹.
    GoldEstimate,
}

/// How one holding outside Yahoo was priced.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub symbol: String,
    /// The scheme's name with plan and option, or the SGB's series.
    pub name: Option<String>,
    pub source: PriceSource,
    /// The NAV's date (funds), or when a saved copy of NSE's SGB list was
    /// fetched (NSE could not be reached).
    pub as_of: Option<NaiveDate>,
    /// AMFI's category, e.g. "Equity Scheme - Flexi Cap Fund".
    pub category: Option<String>,
}

/// Something to tell the reader about some of these holdings.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// The holdings it is about.
    pub symbols: Vec<String>,
    pub text: String,
}

impl Note {
    /// The note with its holdings in front, unless the text already names
    /// its only one.
    pub fn line(&self) -> String {
        match self.symbols.as_slice() {
            [one] if self.text.contains(one.as_str()) => self.text.clone(),
            symbols => format!("{}: {}", symbols.join(", "), self.text),
        }
    }
}

/// Prices for the holdings Yahoo does not quote.
#[derive(Clone, Debug, Default)]
pub struct OffMarket {
    /// One quote per priced holding (₹; the NAV or SGB price).
    pub quotes: Vec<Quote>,
    pub marks: Vec<Mark>,
    pub notes: Vec<Note>,
}

impl OffMarket {
    fn extend(&mut self, other: OffMarket) {
        self.quotes.extend(other.quotes);
        self.marks.extend(other.marks);
        self.notes.extend(other.notes);
    }

    /// Add these quotes to `quotes`, replacing any for the same symbol.
    pub fn merge_into(&self, quotes: &mut Vec<Quote>) {
        for q in &self.quotes {
            quotes.retain(|old| old.symbol != q.symbol);
            quotes.push(q.clone());
        }
    }

    /// Only what concerns `symbols` (a dashboard prices the watchlist and
    /// alerts too, but the portfolio notes are about holdings).
    pub fn restrict(&self, symbols: &[String]) -> OffMarket {
        let wanted = |s: &String| symbols.contains(s);
        OffMarket {
            quotes: self
                .quotes
                .iter()
                .filter(|q| q.symbol.as_ref().is_some_and(wanted))
                .cloned()
                .collect(),
            marks: self
                .marks
                .iter()
                .filter(|m| wanted(&m.symbol))
                .cloned()
                .collect(),
            notes: self
                .notes
                .iter()
                .filter_map(|n| {
                    let symbols: Vec<String> =
                        n.symbols.iter().filter(|s| wanted(s)).cloned().collect();
                    (!symbols.is_empty()).then(|| Note {
                        symbols,
                        text: n.text.clone(),
                    })
                })
                .collect(),
        }
    }

    pub fn mark(&self, symbol: &str) -> Option<&Mark> {
        self.marks.iter().find(|m| m.symbol == symbol)
    }
}

// ── Which holdings ──

/// The scheme in `MF:120503` (an AMFI code) or `MF:INF179K01UT0` (an ISIN).
pub fn fund_id(symbol: &str) -> Option<&str> {
    let (prefix, id) = symbol.split_once(':')?;
    let id = id.trim();
    (prefix.trim().eq_ignore_ascii_case("MF") && !id.is_empty()).then_some(id)
}

pub fn is_fund(symbol: &str) -> bool {
    fund_id(symbol).is_some()
}

/// Exchange-traded funds that Indian tax does not treat like equity shares
/// or equity-oriented funds (no Sec. 111A/112A, no ₹1.25 lakh exemption).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtfKind {
    Gold,
    Silver,
    /// Funds that hold foreign shares (Nasdaq, S&P, Hang Seng).
    International,
    /// Liquid and overnight ETFs.
    Liquid,
    /// Bond, gilt and SDL ETFs.
    Debt,
}

impl EtfKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gold => "gold ETF",
            Self::Silver => "silver ETF",
            Self::International => "international ETF",
            Self::Liquid => "liquid ETF",
            Self::Debt => "debt ETF",
        }
    }

    /// How a sale is taxed, for the "other rules" lists.
    pub fn rule(self) -> &'static str {
        match self {
            Self::Gold | Self::Silver | Self::International => {
                "not an equity-oriented fund: 12.5% after 12 months with no ₹1.25 lakh exemption, \
                 short-term gains at your slab rate"
            }
            Self::Liquid | Self::Debt => {
                "debt ETF: units bought from 1 Apr 2023 are taxed at your slab rate however long \
                 you hold them (Sec. 50AA); older units at 12.5% after 12 months"
            }
        }
    }
}

/// Well-known NSE ETFs that are not equity-oriented, by symbol. A table of
/// names beats treating every ETF as a listed share; an ETF missing from it
/// is still counted like one (see the `tax` docs).
const NON_EQUITY_ETFS: &[(&str, EtfKind)] = &[
    ("GOLDBEES", EtfKind::Gold),
    ("GOLDIETF", EtfKind::Gold),
    ("GOLDCASE", EtfKind::Gold),
    ("GOLDSHARE", EtfKind::Gold),
    ("AXISGOLD", EtfKind::Gold),
    ("HDFCGOLD", EtfKind::Gold),
    ("KOTAKGOLD", EtfKind::Gold),
    ("SBIGETS", EtfKind::Gold),
    ("LICMFGOLD", EtfKind::Gold),
    ("BSLGOLDETF", EtfKind::Gold),
    ("IVZINGOLD", EtfKind::Gold),
    ("QGOLDHALF", EtfKind::Gold),
    ("TATAGOLD", EtfKind::Gold),
    ("GOLD1", EtfKind::Gold),
    ("UNIONGOLD", EtfKind::Gold),
    ("GOLDETF", EtfKind::Gold),
    ("EGOLD", EtfKind::Gold),
    ("BBNPPGOLD", EtfKind::Gold),
    ("GOLDETFADD", EtfKind::Gold),
    ("MOGOLD", EtfKind::Gold),
    ("GROWWGOLD", EtfKind::Gold),
    ("ZGOLD", EtfKind::Gold),
    ("ICICIGOLD", EtfKind::Gold),
    ("NIPGOLD", EtfKind::Gold),
    ("SILVERBEES", EtfKind::Silver),
    ("SILVERIETF", EtfKind::Silver),
    ("SILVER1", EtfKind::Silver),
    ("HDFCSILVER", EtfKind::Silver),
    ("AXISILVER", EtfKind::Silver),
    ("TATSILV", EtfKind::Silver),
    ("SILVRETF", EtfKind::Silver),
    ("ESILVER", EtfKind::Silver),
    ("SBISILVER", EtfKind::Silver),
    ("SILVERETF", EtfKind::Silver),
    ("SILVERADD", EtfKind::Silver),
    ("SILVERCASE", EtfKind::Silver),
    ("MOSILVER", EtfKind::Silver),
    ("GROWWSLVR", EtfKind::Silver),
    ("KOTAKSILVE", EtfKind::Silver),
    ("SILVER360", EtfKind::Silver),
    ("MON100", EtfKind::International),
    ("MONQ50", EtfKind::International),
    ("MAFANG", EtfKind::International),
    ("MAHKTECH", EtfKind::International),
    ("MASPTOP50", EtfKind::International),
    ("HNGSNGBEES", EtfKind::International),
    ("N100", EtfKind::International),
    ("MASPTOP50", EtfKind::International),
    ("MAHKTECH", EtfKind::International),
    ("NASDAQ100", EtfKind::International),
    ("HNGSNGBEES", EtfKind::International),
    ("LIQUIDBEES", EtfKind::Liquid),
    ("LIQUIDCASE", EtfKind::Liquid),
    ("LIQUIDIETF", EtfKind::Liquid),
    ("LIQUID", EtfKind::Liquid),
    ("LIQUIDADD", EtfKind::Liquid),
    ("LIQUIDSHRI", EtfKind::Liquid),
    ("LIQUIDSBI", EtfKind::Liquid),
    ("LIQUIDETF", EtfKind::Liquid),
    ("CASHIETF", EtfKind::Liquid),
    ("GROWWLIQID", EtfKind::Liquid),
    ("GILT5YBEES", EtfKind::Debt),
    ("LTGILTBEES", EtfKind::Debt),
    ("LICNETFGSC", EtfKind::Debt),
    ("SETF10GILT", EtfKind::Debt),
    ("NETFGILT5Y", EtfKind::Debt),
    ("NETFLTGILT", EtfKind::Debt),
    ("GSEC10IETF", EtfKind::Debt),
    ("GSEC5IETF", EtfKind::Debt),
    ("GSEC10YEAR", EtfKind::Debt),
    ("GSEC10ABSL", EtfKind::Debt),
    ("SDL26BEES", EtfKind::Debt),
    ("BBETF0432", EtfKind::Debt),
    ("LIQUID1", EtfKind::Liquid),
    ("LIQUIDBETF", EtfKind::Liquid),
];

/// NSE's list of exchange-traded funds, with each one's underlying index or
/// asset: `Symbol,Underlying,SecurityName,DateofListing,MarketLot,
/// ISINNumber,FaceValue`. Cached for a day like the equity list.
pub const NSE_ETF_LIST: &str =
    "https://nsearchives.nseindia.com/content/equities/eq_etfseclist.csv";

/// ETFs classified from their category (NSE's underlying, or an AMFI
/// category and name) this run: `(bare symbol, kind)`, non-equity only.
/// Empty until [`load_etf_categories`] has run; [`non_equity_etf`] then
/// falls back to [`NON_EQUITY_ETFS`] and the symbol's name.
static ETF_CATEGORIES: std::sync::RwLock<Vec<(String, EtfKind)>> =
    std::sync::RwLock::new(Vec::new());

/// Replace the ETFs known by category (see [`ETF_CATEGORIES`]).
pub fn set_etf_categories(list: Vec<(String, EtfKind)>) {
    if let Ok(mut g) = ETF_CATEGORIES.write() {
        *g = list;
    }
}

fn etf_by_category(bare: &str) -> Option<EtfKind> {
    let g = ETF_CATEGORIES.read().ok()?;
    g.iter().find(|(s, _)| s == bare).map(|(_, k)| *k)
}

/// The kind of ETF its category says it is: an underlying such as "Domestic
/// price of Gold", "NASDAQ 100 Index", "Nifty 1D Rate Index" or "CRISIL IBX
/// Gilt Index", or an AMFI category and scheme name. `None` for an equity
/// index (Nifty 50, Sensex, a sector), which is taxed like listed shares.
pub fn etf_kind_from_category(text: &str) -> Option<EtfKind> {
    let t = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["gold"]) {
        return Some(EtfKind::Gold);
    }
    if has(&["silver"]) {
        return Some(EtfKind::Silver);
    }
    if has(&[
        "liquid",
        "overnight",
        "money market",
        "1d rate",
        "1 day rate",
        "tri-party",
        "triparty",
    ]) {
        return Some(EtfKind::Liquid);
    }
    if has(&[
        "gilt",
        "g-sec",
        "gsec",
        "government securit",
        "sdl",
        "bond",
        "debt",
        "t-bill",
        "treasury",
        "crisil ibx",
        "target maturity",
    ]) {
        return Some(EtfKind::Debt);
    }
    if has(&[
        "nasdaq",
        "s&p 500",
        "s&p500",
        "hang seng",
        "fang",
        "nyse",
        "overseas",
        "international",
        "global",
        "japan",
        "taiwan",
        "china",
        "fof overseas",
    ]) {
        return Some(EtfKind::International);
    }
    None
}

/// Rows of NSE's ETF list: `(symbol, underlying, name)`. Columns are found
/// by name, so a reordered or extended file still reads.
pub fn parse_nse_etf_list(text: &str) -> Vec<(String, String, String)> {
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let cols: Vec<String> = crate::brokers::csv_fields(header)
        .iter()
        .map(|c| c.trim().to_lowercase().replace(' ', ""))
        .collect();
    let at = |name: &str| cols.iter().position(|c| c == name);
    let (Some(sym), Some(under)) = (at("symbol"), at("underlying")) else {
        return Vec::new();
    };
    let name = at("securityname");
    lines
        .filter_map(|l| {
            let f = crate::brokers::csv_fields(l);
            let s = f.get(sym)?.trim().to_ascii_uppercase();
            (!s.is_empty()).then(|| {
                (
                    s,
                    f.get(under)
                        .map(|u| u.trim().to_string())
                        .unwrap_or_default(),
                    name.and_then(|n| f.get(n))
                        .map(|n| n.trim().to_string())
                        .unwrap_or_default(),
                )
            })
        })
        .collect()
}

/// Whether an NSE underlying names a domestic equity index (Nifty, Sensex,
/// a BSE index): such an ETF is equity whatever its scheme name says.
fn names_domestic_equity_index(underlying: &str) -> bool {
    let u = underlying.to_lowercase();
    ["nifty", "sensex", "bse", "s&p bse"]
        .iter()
        .any(|w| u.contains(w))
}

/// The non-equity ETFs in NSE's list, classified by their underlying (and
/// name when the underlying says nothing).
pub fn classify_etf_list(rows: &[(String, String, String)]) -> Vec<(String, EtfKind)> {
    rows.iter()
        .filter_map(|(sym, under, name)| {
            let kind = etf_kind_from_category(under).or_else(|| {
                // An empty or opaque underlying (an index code such as
                // `GLD_IDX`): the scheme name decides. A named domestic
                // equity index stays equity.
                (!names_domestic_equity_index(under))
                    .then(|| etf_kind_from_category(name))
                    .flatten()
            })?;
            Some((sym.clone(), kind))
        })
        .collect()
}

/// Load NSE's ETF list (cached; best effort) and classify every ETF in it by
/// category, so `tax`, `tax export`, `tax advance` and `harvest` know a gold,
/// silver, international, liquid or debt ETF that is not in the table yet.
/// Without the list (offline, nothing cached) the table and name patterns
/// decide alone.
pub async fn load_etf_categories() {
    let text = async {
        let client = crate::funds::http_client()?;
        crate::funds::cached_text(
            &client,
            "nse_etf_list.csv",
            NSE_ETF_LIST,
            "NSE",
            valid_etf_list,
        )
        .await
    }
    .await;
    // Only a list that parses into rows replaces the map; anything else
    // leaves the current one (and the table) in charge.
    if let Ok(text) = text
        && valid_etf_list(&text)
    {
        set_etf_categories(classify_etf_list(&parse_nse_etf_list(&text)));
    }
}

/// Whether `text` is NSE's ETF list as [`parse_nse_etf_list`] reads it:
/// the `Symbol` and `Underlying` columns and at least one row.
pub fn valid_etf_list(text: &str) -> bool {
    !parse_nse_etf_list(text).is_empty()
}

/// Which kind of non-equity ETF a `GOLDBEES` / `GOLDBEES.NS` symbol is, or
/// `None` for shares, equity ETFs (`NIFTYBEES`) and anything unknown. Beyond
/// the table, Nippon-style `…BEES` names that say gold, silver, liquid, gilt,
/// Nasdaq or Hang Seng, and Bharat Bond (`EBBETF…`) names, count too.
pub fn non_equity_etf(symbol: &str) -> Option<EtfKind> {
    let upper = symbol.trim().to_ascii_uppercase();
    let bare = upper
        .strip_suffix(".NS")
        .or_else(|| upper.strip_suffix(".BO"))
        .unwrap_or(&upper);
    if let Some((_, kind)) = NON_EQUITY_ETFS.iter().find(|(s, _)| *s == bare) {
        return Some(*kind);
    }
    if let Some(kind) = etf_by_category(bare) {
        return Some(kind);
    }
    if bare.starts_with("EBBETF") || bare.starts_with("BBETF") {
        return Some(EtfKind::Debt);
    }
    if bare.starts_with("LIQUID") {
        return Some(EtfKind::Liquid);
    }
    if bare.ends_with("BEES") {
        let has = |w: &str| bare.contains(w);
        if has("GOLD") {
            return Some(EtfKind::Gold);
        }
        if has("SILVER") {
            return Some(EtfKind::Silver);
        }
        if has("LIQUID") {
            return Some(EtfKind::Liquid);
        }
        if has("GILT") {
            return Some(EtfKind::Debt);
        }
        if has("NASDAQ") || has("HNGSNG") || has("SNP") {
            return Some(EtfKind::International);
        }
    }
    // Other fund houses' ETF names (`…IETF`, `…ETF`, `…CASE`, `…ADD`) that
    // say what they hold.
    let etf_like = ["IETF", "ETF", "CASE", "ADD"]
        .iter()
        .any(|s| bare.ends_with(s) && bare.len() > s.len());
    if etf_like {
        let has = |w: &[&str]| w.iter().any(|w| bare.contains(w));
        if has(&["GOLD"]) {
            return Some(EtfKind::Gold);
        }
        if has(&["SILVER", "SILVR", "SLVR"]) {
            return Some(EtfKind::Silver);
        }
        if has(&["LIQUID", "LIQID"]) {
            return Some(EtfKind::Liquid);
        }
        if has(&["GILT", "GSEC", "SDL", "BOND"]) {
            return Some(EtfKind::Debt);
        }
        if has(&["NASDAQ", "NASDQ", "HNGSNG", "HANGSENG", "FANG", "SP500"]) {
            return Some(EtfKind::International);
        }
    }
    None
}

/// The NSE symbol or ISIN of a Sovereign Gold Bond holding: `SGBNOV26.NS`
/// and `SGBNOV26` → `SGBNOV26`, an outstanding tranche's ISIN
/// (`IN0020180314`) as is. Only `SGB…` symbols with a digit count, so a
/// stock ticker that happens to start with SGB is not mistaken for one.
pub fn sgb_id(symbol: &str) -> Option<String> {
    let upper = symbol.trim().to_ascii_uppercase();
    let bare = upper
        .strip_suffix(".NS")
        .or_else(|| upper.strip_suffix(".BO"))
        .unwrap_or(&upper);
    let listed = bare.len() > 4
        && bare.starts_with("SGB")
        && bare.chars().all(|c| c.is_ascii_alphanumeric())
        && bare.chars().any(|c| c.is_ascii_digit());
    let isin = cm::SGB_TRANCHES.iter().any(|t| t.isin == bare);
    (listed || isin).then(|| bare.to_string())
}

pub fn is_sgb(symbol: &str) -> bool {
    sgb_id(symbol).is_some()
}

/// How an Indian sale or holding is taxed, decided from its symbol and, for a
/// mutual fund, its AMFI category. `tax in`, `tax export in`, `tax advance`
/// and `harvest` all use this, so one portfolio gives the same sales in
/// Sec. 111A/112A in every command.
#[derive(Clone, Debug, PartialEq)]
pub enum IndiaClass {
    /// A listed share, or an exchange-traded equity ETF: Sec. 111A/112A.
    Listed,
    /// An equity-oriented mutual fund: Sec. 111A/112A, like listed shares.
    EquityFund,
    /// Crypto and other virtual digital assets: Sec. 115BBH (30% flat, no
    /// set-off of losses), reported in Schedule VDA.
    Vda,
    /// Everything else (debt and other funds, gold, silver, liquid and
    /// international ETFs, Sovereign Gold Bonds): left out of 111A/112A, with
    /// the rule that applies.
    Other(String),
}

/// Classify `symbol` for Indian capital-gains tax. `mark` is its AMFI mark
/// (see [`OffMarket::mark`]); a fund without a category is `Other`, because
/// nothing says it is equity-oriented.
pub fn india_class(symbol: &str, mark: Option<&Mark>) -> IndiaClass {
    if crate::market::AssetClass::of(symbol) == crate::market::AssetClass::Crypto {
        return IndiaClass::Vda;
    }
    if is_fund(symbol) {
        return match mark.and_then(|m| m.category.as_deref()) {
            Some(cat)
                if is_equity_oriented(cat, mark.and_then(|m| m.name.as_deref()).unwrap_or("")) =>
            {
                IndiaClass::EquityFund
            }
            Some(cat) => IndiaClass::Other(format!(
                "{cat} — not an equity-oriented fund: units bought from 1 Apr 2023 are taxed at \
                 your slab rate, older units at 12.5% after 24 months"
            )),
            None => IndiaClass::Other(
                "no AMFI category (not in AMFI's NAV file, or the file could not be loaded), so \
                 whether it is equity-oriented is unknown"
                    .into(),
            ),
        };
    }
    if let Some(kind) = non_equity_etf(symbol) {
        return IndiaClass::Other(format!("{} — {}", kind.name(), kind.rule()));
    }
    if is_sgb(symbol) {
        return IndiaClass::Other(
            "Sovereign Gold Bond — tax-free when RBI redeems it; sold on the exchange, 12.5% \
             after 12 months (no ₹1.25 lakh exemption), else your slab rate"
                .into(),
        );
    }
    if is_mld(symbol) {
        return IndiaClass::Other(
            "market-linked debenture — short-term at your slab rate however long you hold it \
             (Sec. 50AA)"
                .into(),
        );
    }
    IndiaClass::Listed
}

/// A market-linked debenture: listed with `MLD` and a digit in the symbol.
pub fn is_mld(symbol: &str) -> bool {
    let upper = symbol.to_ascii_uppercase();
    let bare = upper
        .strip_suffix(".NS")
        .or_else(|| upper.strip_suffix(".BO"))
        .unwrap_or(&upper);
    bare.contains("MLD") && bare.chars().any(|c| c.is_ascii_digit())
}

/// Whether an AMFI category and scheme name describe a debt-oriented fund
/// (more than 65% in debt and money-market instruments): debt schemes,
/// liquid, overnight and money-market funds, gilt and target-maturity index
/// funds.
pub fn is_debt_oriented(category: &str, name: &str) -> bool {
    let cat = category.to_lowercase();
    if cat.contains("debt scheme") || cat.contains("income") {
        return true;
    }
    let both = format!("{cat} {}", name.to_lowercase());
    [
        "liquid",
        "overnight",
        "money market",
        "gilt",
        "g-sec",
        "gsec",
        "sdl",
        "t-bill",
        "target maturity",
        "bharat bond",
        "crisil ibx",
        "corporate bond",
        "banking and psu",
    ]
    .iter()
    .any(|w| both.contains(w))
}

/// What kind of non-equity asset `symbol` is for Indian capital gains (Sec.
/// 50AA, 112, 115BBH), or `None` for listed shares, equity ETFs,
/// equity-oriented funds and a fund whose category is unknown.
pub fn india_asset(symbol: &str, mark: Option<&Mark>) -> Option<crate::tax::IndiaAsset> {
    use crate::tax::IndiaAsset;
    if crate::market::AssetClass::of(symbol) == crate::market::AssetClass::Crypto {
        return Some(IndiaAsset::Vda);
    }
    if is_fund(symbol) {
        let cat = mark.and_then(|m| m.category.as_deref())?;
        let name = mark.and_then(|m| m.name.as_deref()).unwrap_or("");
        if is_equity_oriented(cat, name) {
            return None;
        }
        if name.to_lowercase().contains("market linked") {
            return Some(IndiaAsset::MarketLinkedDebenture);
        }
        return Some(if is_debt_oriented(cat, name) {
            IndiaAsset::DebtFund { listed: false }
        } else {
            IndiaAsset::OtherFund { listed: false }
        });
    }
    if let Some(kind) = non_equity_etf(symbol) {
        return Some(match kind {
            EtfKind::Liquid | EtfKind::Debt => IndiaAsset::DebtFund { listed: true },
            EtfKind::Gold | EtfKind::Silver | EtfKind::International => {
                IndiaAsset::OtherFund { listed: true }
            }
        });
    }
    if is_sgb(symbol) {
        return Some(IndiaAsset::GoldBond);
    }
    if is_mld(symbol) {
        return Some(IndiaAsset::MarketLinkedDebenture);
    }
    None
}

/// Whether a symbol is priced here rather than by a market quote.
pub fn is_off_market(symbol: &str) -> bool {
    is_fund(symbol) || is_sgb(symbol)
}

/// Units rather than shares: fund units, SGB units (grams).
pub fn unit_word(symbol: &str) -> &'static str {
    if is_off_market(symbol) {
        "units"
    } else {
        "shares"
    }
}

// ── Mutual funds ──

/// The scheme an id names: an AMFI code, or the ISIN of its growth/payout
/// or reinvestment option.
pub fn find_scheme<'a>(schemes: &'a [Scheme], id: &str) -> Option<&'a Scheme> {
    match id.parse::<u32>() {
        Ok(code) => schemes.iter().find(|s| s.code == code),
        Err(_) => schemes.iter().find(|s| {
            [&s.isin_growth, &s.isin_reinvest]
                .into_iter()
                .flatten()
                .any(|isin| isin.eq_ignore_ascii_case(id))
        }),
    }
}

/// Whether Indian tax treats the scheme as equity-oriented (65%+ in
/// domestic equity), going by its SEBI category: equity schemes (ELSS
/// included), arbitrage, aggressive hybrid and equity savings funds, and
/// index funds/ETFs on a Nifty or Sensex equity index. Everything else —
/// debt, gold, international, other hybrids, fund-of-funds — is not.
pub fn is_equity_oriented(category: &str, name: &str) -> bool {
    let cat = category.to_lowercase();
    if cat.contains("equity scheme") || cat.contains("elss") {
        return true;
    }
    if ["arbitrage", "aggressive hybrid", "equity savings"]
        .iter()
        .any(|c| cat.contains(c))
    {
        return true;
    }
    let name = name.to_lowercase();
    let index_like = cat.contains("index fund") || cat.contains("etf");
    let equity_index = ["nifty", "sensex"].iter().any(|w| name.contains(w));
    let debt_words = [
        "g-sec",
        "gsec",
        "gilt",
        "sdl",
        "bond",
        "debt",
        "liquid",
        "crisil",
        "gold",
        "silver",
        "t-bill",
        "money market",
        "overnight",
        "target maturity",
    ];
    index_like && equity_index && !debt_words.iter().any(|w| name.contains(w))
}

/// Value `symbols` (funds only; others are ignored) at AMFI's NAVs, or say
/// why not when `navs` could not be loaded (the error text).
pub fn price_funds(symbols: &[String], navs: Result<&[Scheme], &str>) -> OffMarket {
    let funds: Vec<&String> = symbols.iter().filter(|s| is_fund(s)).collect();
    let mut out = OffMarket::default();
    if funds.is_empty() {
        return out;
    }
    let schemes = match navs {
        Ok(s) => s,
        Err(e) => {
            out.notes.push(Note {
                symbols: funds.iter().map(|s| s.to_string()).collect(),
                text: format!(
                    "AMFI's NAV file could not be loaded ({e}), so these funds have no price"
                ),
            });
            return out;
        }
    };
    let newest = schemes.iter().filter_map(|s| s.date).max();
    for sym in funds {
        let id = fund_id(sym).unwrap_or_default();
        let Some(scheme) = find_scheme(schemes, id) else {
            out.notes.push(Note {
                symbols: vec![sym.clone()],
                text: format!(
                    "{sym} is not in AMFI's NAV file — check the code or ISIN with `stockwise mf search`"
                ),
            });
            continue;
        };
        let name = scheme.full_name();
        let Some(nav) = scheme.nav else {
            out.notes.push(Note {
                symbols: vec![sym.clone()],
                text: format!("AMFI publishes no NAV for {sym} ({name})"),
            });
            continue;
        };
        if let (Some(date), Some(newest)) = (scheme.date, newest)
            && (newest - date).num_days() > 30
        {
            out.notes.push(Note {
                symbols: vec![sym.clone()],
                text: format!(
                    "AMFI's last NAV for {sym} is from {} — the scheme may have matured or merged",
                    date.format("%d %b %Y")
                ),
            });
        }
        out.quotes.push(Quote {
            symbol: Some(sym.clone()),
            short_name: Some(name.clone()),
            long_name: Some(name.clone()),
            regular_market_price: Some(nav),
            currency: Some("INR".into()),
            ..Default::default()
        });
        out.marks.push(Mark {
            symbol: sym.clone(),
            name: Some(name),
            source: PriceSource::AmfiNav,
            as_of: scheme.date,
            category: scheme.category.clone(),
        });
    }
    out
}

// ── Sovereign Gold Bonds ──

/// The reference tranche for an SGB id (symbol or ISIN).
fn tranche(id: &str) -> Option<&'static cm::Tranche> {
    cm::SGB_TRANCHES
        .iter()
        .find(|t| t.symbol.eq_ignore_ascii_case(id) || t.isin.eq_ignore_ascii_case(id))
}

/// NSE's row for an SGB id: by symbol or ISIN, directly or through the
/// tranche table (a holding stored by ISIN, a list row by symbol).
fn nse_row<'a>(rows: &'a [NseSgb], id: &str) -> Option<&'a NseSgb> {
    let t = tranche(id);
    rows.iter().find(|r| {
        r.symbol.eq_ignore_ascii_case(id)
            || r.isin
                .as_deref()
                .is_some_and(|i| i.eq_ignore_ascii_case(id))
            || t.is_some_and(|t| {
                r.symbol.eq_ignore_ascii_case(t.symbol)
                    || r.isin
                        .as_deref()
                        .is_some_and(|i| i.eq_ignore_ascii_case(t.isin))
            })
    })
}

/// The SGB's last price on NSE, if the list has one (0 = no trade).
pub fn listed_price(rows: &[NseSgb], id: &str) -> Option<f64> {
    nse_row(rows, id).and_then(|r| r.price)
}

/// What [`price_sgbs`] needs from NSE.
pub struct SgbPrices<'a> {
    /// NSE's list, or why it could not be loaded.
    pub list: Result<&'a [NseSgb], &'a str>,
    /// When the list is an older saved copy: the day it was fetched.
    pub saved_on: Option<NaiveDate>,
}

/// Value SGB holdings among `symbols`: NSE's last price; else a price Yahoo
/// already gave (`yahoo`); else one gram of gold at spot (`gold_per_gram`),
/// marked as an estimate. Each unit of an SGB is one gram of gold.
pub fn price_sgbs(
    symbols: &[String],
    nse: SgbPrices<'_>,
    yahoo: &[Quote],
    gold_per_gram: Option<f64>,
) -> OffMarket {
    let mut out = OffMarket::default();
    let mut estimated = Vec::new();
    let mut unpriced = Vec::new();
    for sym in symbols {
        let Some(id) = sgb_id(sym) else { continue };
        let row = nse.list.ok().and_then(|rows| nse_row(rows, &id));
        let name = tranche(&id)
            .map(|t| format!("SGB {}", t.series))
            .or_else(|| row.and_then(|r| r.name.clone()));
        let quote = |price: f64| Quote {
            symbol: Some(sym.clone()),
            short_name: name.clone(),
            regular_market_price: Some(price),
            currency: Some("INR".into()),
            ..Default::default()
        };
        if let Some(price) = row.and_then(|r| r.price) {
            out.quotes.push(quote(price));
            out.marks.push(Mark {
                symbol: sym.clone(),
                name: name.clone(),
                source: PriceSource::NseSgb,
                as_of: nse.saved_on,
                category: None,
            });
        } else if yahoo
            .iter()
            .any(|q| q.symbol.as_deref() == Some(sym) && q.regular_market_price.is_some())
        {
            // Yahoo's quote stands; nothing to add.
        } else if let Some(gold) = gold_per_gram {
            out.quotes.push(quote(crate::output::round(gold, 2)));
            out.marks.push(Mark {
                symbol: sym.clone(),
                name,
                source: PriceSource::GoldEstimate,
                as_of: None,
                category: None,
            });
            estimated.push(sym.clone());
        } else {
            unpriced.push(sym.clone());
        }
    }
    let why = match nse.list {
        Err(e) => format!("NSE's SGB list could not be loaded ({e})"),
        Ok(_) => "NSE shows no trade for them today".to_string(),
    };
    if !estimated.is_empty() {
        out.notes.push(Note {
            text: format!(
                "Estimated at one gram of gold at the international spot price (no import duty or GST): {why}"
            ),
            symbols: estimated,
        });
    }
    if !unpriced.is_empty() {
        out.notes.push(Note {
            text: format!("No price: {why}, and no gold price either"),
            symbols: unpriced,
        });
    }
    let priced: Vec<String> = out.marks.iter().map(|m| m.symbol.clone()).collect();
    if !priced.is_empty() {
        out.notes.push(Note {
            symbols: priced,
            text: format!(
                "SGB values leave out the {}% yearly interest, which is paid to your bank every six months",
                cm::SGB_COUPON_PCT
            ),
        });
    }
    out
}

// ── Fetching ──

/// Price the funds and SGBs among `symbols` through `src`. `yahoo` holds
/// the market quotes already fetched for the rest (an SGB Yahoo priced
/// keeps that price when NSE has none). Never fails: what cannot be priced
/// is left out with a note.
pub async fn price(src: &impl DataSource, symbols: &[String], yahoo: &[Quote]) -> OffMarket {
    let mut out = OffMarket::default();
    if symbols.iter().any(|s| is_fund(s)) {
        out.extend(match src.fund_navs().await {
            Ok(schemes) => price_funds(symbols, Ok(&schemes)),
            Err(e) => price_funds(symbols, Err(&format!("{e:#}"))),
        });
    }
    let sgbs: Vec<String> = symbols.iter().filter(|s| is_sgb(s)).cloned().collect();
    if sgbs.is_empty() {
        return out;
    }
    let list = src.sgb_list().await;
    let (rows, saved_on, error) = match &list {
        Ok(l) => (
            Some(l.rows.as_slice()),
            l.saved_at.map(|t| t.date_naive()),
            String::new(),
        ),
        Err(e) => (None, None, format!("{e:#}")),
    };
    let needs_gold = sgbs.iter().any(|s| {
        let id = sgb_id(s).unwrap_or_default();
        rows.and_then(|r| listed_price(r, &id)).is_none()
            && !yahoo
                .iter()
                .any(|q| q.symbol.as_deref() == Some(s) && q.regular_market_price.is_some())
    });
    let gold = if needs_gold {
        src.quotes(&[cm::GOLD_FUTURE, cm::USDINR])
            .await
            .ok()
            .and_then(|q| cm::gold_inr_per_gram(&q))
    } else {
        None
    };
    out.extend(price_sgbs(
        &sgbs,
        SgbPrices {
            list: rows.ok_or(error.as_str()),
            saved_on,
        },
        yahoo,
        gold,
    ));
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn non_equity_etfs_are_told_from_shares_and_equity_etfs() {
        for (sym, kind) in [
            ("GOLDBEES", EtfKind::Gold),
            ("GOLDBEES.NS", EtfKind::Gold),
            ("goldbees.ns", EtfKind::Gold),
            ("SILVERBEES.NS", EtfKind::Silver),
            ("LIQUIDBEES.NS", EtfKind::Liquid),
            ("LIQUIDCASE.NS", EtfKind::Liquid),
            ("MON100.NS", EtfKind::International),
            ("HNGSNGBEES.NS", EtfKind::International),
            ("EBBETF0433.NS", EtfKind::Debt),
            ("LTGILTBEES.NS", EtfKind::Debt),
            // A name that says gold on a BEES fund not in the table.
            ("NEWGOLDBEES.NS", EtfKind::Gold),
        ] {
            assert_eq!(non_equity_etf(sym), Some(kind), "{sym}");
        }
        // Equity ETFs, shares (even with gold or silver in the name), funds, SGBs.
        for sym in [
            "NIFTYBEES.NS",
            "BANKBEES.NS",
            "JUNIORBEES.NS",
            "MID150BEES.NS",
            "CPSEETF.NS",
            "SILVERTUC.NS",
            "GOLDIAM.NS",
            "TITAN.NS",
            "RELIANCE.NS",
            "AAPL",
            "MF:120503",
            "SGBNOV26.NS",
        ] {
            assert_eq!(non_equity_etf(sym), None, "{sym}");
        }
    }

    #[test]
    fn etfs_are_classified_by_category_from_the_nse_list() {
        let rows = parse_nse_etf_list(include_str!("fixtures/nse_etf_list.csv"));
        assert_eq!(rows.len(), 10);
        let classified = classify_etf_list(&rows);
        let kind = |s: &str| classified.iter().find(|(x, _)| x == s).map(|(_, k)| *k);
        // New ETFs whose symbol says nothing of what they hold.
        for (sym, want) in [
            ("ABCGLDX", EtfKind::Gold),
            ("SLVRX1", EtfKind::Silver),
            ("XYZNQ100", EtfKind::International),
            ("HSTECHX", EtfKind::International),
            ("LMNT1D", EtfKind::Liquid),
            ("DEFG30", EtfKind::Debt),
            ("BHARAT31", EtfKind::Debt),
        ] {
            assert_eq!(kind(sym), Some(want), "{sym} must not be taxed as equity");
        }
        // Equity index ETFs stay listed shares.
        for sym in ["NIFTYBEES", "PQRCPSE", "MIDQ50"] {
            assert_eq!(kind(sym), None, "{sym}");
        }
        // Without the list, the table and names decide: these are unknown.
        assert_eq!(non_equity_etf("ABCGLDX.NS"), None);
        set_etf_categories(classified);
        assert_eq!(non_equity_etf("ABCGLDX.NS"), Some(EtfKind::Gold));
        assert!(matches!(
            india_class("XYZNQ100.NS", None),
            IndiaClass::Other(r) if r.contains("international ETF")
        ));
        assert_eq!(non_equity_etf("NIFTYBEES.NS"), None);
        // MOM100 tracks the Nifty Midcap 100: equity. MON100 is the Nasdaq one.
        assert_eq!(india_class("MOM100.NS", None), IndiaClass::Listed);
        assert_eq!(non_equity_etf("MON100.NS"), Some(EtfKind::International));
    }

    #[test]
    fn an_opaque_underlying_code_falls_back_to_the_scheme_name() {
        let rows = vec![
            (
                "GLDX".to_string(),
                "GLD_IDX".to_string(),
                "Domestic Gold ETF".to_string(),
            ),
            (
                "NIFX".to_string(),
                "Nifty 50".to_string(),
                "Gold Standard Nifty ETF".to_string(),
            ),
        ];
        let c = classify_etf_list(&rows);
        assert_eq!(c, vec![("GLDX".to_string(), EtfKind::Gold)]);
    }

    #[test]
    fn a_header_alone_is_not_a_valid_etf_list() {
        assert!(!valid_etf_list("symbol,underlying_code\nABC,X\n"));
        assert!(!valid_etf_list("Symbol,Underlying\n"));
        assert!(valid_etf_list(include_str!("fixtures/nse_etf_list.csv")));
    }

    #[test]
    fn market_linked_debentures_are_never_listed_shares() {
        assert!(
            matches!(india_class("ABCMLD2028.NS", None), IndiaClass::Other(r) if r.contains("50AA"))
        );
        assert_eq!(
            india_asset("ABCMLD2028.NS", None),
            Some(crate::tax::IndiaAsset::MarketLinkedDebenture)
        );
    }

    #[test]
    fn etf_names_from_other_fund_houses_fall_back_to_the_symbol() {
        for (sym, kind) in [
            ("GOLDETF.NS", EtfKind::Gold),
            ("SILVERADD.NS", EtfKind::Silver),
            ("GSEC10IETF.NS", EtfKind::Debt),
            ("XGSECETF.NS", EtfKind::Debt),
            ("ABSLNASDQETF.NS", EtfKind::International),
            ("BBETF0432.NS", EtfKind::Debt),
        ] {
            assert_eq!(non_equity_etf(sym), Some(kind), "{sym}");
        }
        assert_eq!(non_equity_etf("CPSEETF.NS"), None);
        assert_eq!(non_equity_etf("BANKETF.NS"), None);
    }

    #[test]
    fn india_asset_tells_debt_from_other_funds_and_crypto() {
        use crate::tax::IndiaAsset;
        let mark = |cat: &str, name: &str| Mark {
            symbol: "MF:1".into(),
            name: Some(name.into()),
            source: PriceSource::AmfiNav,
            as_of: None,
            category: Some(cat.into()),
        };
        let liquid = mark("Debt Scheme - Liquid Fund", "Some Liquid Fund - Growth");
        assert_eq!(
            india_asset("MF:1", Some(&liquid)),
            Some(IndiaAsset::DebtFund { listed: false })
        );
        let fof = mark("Other Scheme - FoF Overseas", "Some US Equity FoF");
        assert_eq!(
            india_asset("MF:1", Some(&fof)),
            Some(IndiaAsset::OtherFund { listed: false })
        );
        let equity = mark("Equity Scheme - Flexi Cap Fund", "Flexi Cap - Growth");
        assert_eq!(india_asset("MF:1", Some(&equity)), None);
        assert_eq!(india_asset("BTC-INR", None), Some(IndiaAsset::Vda));
        assert_eq!(
            india_asset("LIQUIDBEES.NS", None),
            Some(IndiaAsset::DebtFund { listed: true })
        );
        assert_eq!(
            india_asset("GOLDBEES.NS", None),
            Some(IndiaAsset::OtherFund { listed: true })
        );
        assert_eq!(india_asset("SGBNOV26.NS", None), Some(IndiaAsset::GoldBond));
        assert_eq!(india_asset("RELIANCE.NS", None), None);
    }

    #[test]
    fn etf_rules_say_how_the_gain_is_taxed() {
        assert!(EtfKind::Gold.rule().contains("no ₹1.25 lakh exemption"));
        assert!(EtfKind::Liquid.rule().contains("slab rate"));
        assert_eq!(EtfKind::Silver.name(), "silver ETF");
    }

    /// The AMFI fixture from `funds` (PPFAS equity plans, an Axis plan with
    /// no NAV) plus a debt fund and a wound-up scheme.
    pub(crate) fn navall() -> String {
        format!(
            "{}{}",
            crate::funds::tests::NAVALL,
            "Open Ended Schemes(Debt Scheme - Liquid Fund)\r
HDFC Mutual Fund\r
119091;INF179KB1HP9;-;HDFC Liquid Fund;Direct Plan;Growth;5021.1234;19-Oct-2026\r
119092;INF179KB1HQ7;-;HDFC Liquid Fund;Direct Plan;Daily IDCW;N.A.;19-Oct-2026\r
Close Ended Schemes(Income)\r
Old AMC\r
100001;INF000A00001;-;Old Fixed Maturity Plan;Regular Plan;Growth;11.5000;01-Mar-2024\r
"
        )
    }

    fn schemes() -> Vec<Scheme> {
        crate::funds::parse_navall(&navall())
    }

    fn syms(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn sgb(symbol: &str, isin: &str, price: Option<f64>) -> NseSgb {
        NseSgb {
            symbol: symbol.into(),
            isin: Some(isin.into()),
            name: Some(format!("{symbol} name")),
            issue_price: None,
            price,
            change_percent: None,
            volume: None,
        }
    }

    fn price_of(out: &OffMarket, symbol: &str) -> Option<f64> {
        out.quotes
            .iter()
            .find(|q| q.symbol.as_deref() == Some(symbol))
            .and_then(|q| q.regular_market_price)
    }

    #[test]
    fn recognises_fund_and_sgb_symbols() {
        assert_eq!(fund_id("MF:120503"), Some("120503"));
        assert_eq!(fund_id("mf:INF179K01UT0"), Some("INF179K01UT0"));
        assert_eq!(fund_id("MF:"), None);
        assert_eq!(fund_id("TCS.NS"), None);
        assert_eq!(sgb_id("SGBNOV26.NS").as_deref(), Some("SGBNOV26"));
        assert_eq!(sgb_id("sgbdec26").as_deref(), Some("SGBDEC26"));
        assert_eq!(sgb_id("SGBMAR31IV.BO").as_deref(), Some("SGBMAR31IV"));
        assert_eq!(sgb_id("IN0020180314").as_deref(), Some("IN0020180314"));
        // Not every SGB-looking ticker or G-sec ISIN is a gold bond.
        assert_eq!(sgb_id("SGBL.NS"), None);
        assert_eq!(sgb_id("IN0020230085"), None);
        assert_eq!(sgb_id("TCS.NS"), None);
        assert!(is_off_market("MF:1") && is_off_market("SGBJUN31I.NS"));
        assert_eq!(unit_word("MF:1"), "units");
        assert_eq!(unit_word("TCS.NS"), "shares");
    }

    #[test]
    fn funds_are_found_by_code_or_either_isin() {
        let s = schemes();
        assert_eq!(find_scheme(&s, "122639").unwrap().code, 122639);
        assert_eq!(find_scheme(&s, "INF879O01027").unwrap().code, 122639);
        assert_eq!(find_scheme(&s, "inf879o01308").unwrap().code, 153964);
        assert!(find_scheme(&s, "999999").is_none());
        assert!(find_scheme(&s, "INF000X99999").is_none());
    }

    #[test]
    fn funds_are_priced_at_the_latest_nav_with_its_date() {
        let s = schemes();
        let symbols = syms(&["MF:122639", "MF:INF879O01308", "TCS.NS", "MF:119091"]);
        let out = price_funds(&symbols, Ok(&s));
        assert_eq!(price_of(&out, "MF:122639"), Some(90.3546));
        // The IDCW plan, by its reinvestment ISIN.
        assert_eq!(price_of(&out, "MF:INF879O01308"), Some(90.3546));
        assert_eq!(price_of(&out, "TCS.NS"), None, "stocks are left to Yahoo");
        let m = out.mark("MF:122639").unwrap();
        assert_eq!(m.source, PriceSource::AmfiNav);
        assert_eq!(m.as_of, NaiveDate::from_ymd_opt(2026, 10, 19));
        assert_eq!(
            m.name.as_deref(),
            Some("Parag Parikh Flexi Cap Fund - Direct Plan - Growth")
        );
        assert_eq!(
            out.mark("MF:119091").unwrap().category.as_deref(),
            Some("Debt Scheme - Liquid Fund")
        );
        let q = out
            .quotes
            .iter()
            .find(|q| q.symbol.as_deref() == Some("MF:122639"));
        assert_eq!(q.unwrap().currency.as_deref(), Some("INR"));
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn unknown_or_navless_funds_are_unpriced_with_a_note_never_zero() {
        let s = schemes();
        let symbols = syms(&["MF:999999", "MF:119092", "MF:100001"]);
        let out = price_funds(&symbols, Ok(&s));
        assert_eq!(price_of(&out, "MF:999999"), None);
        assert_eq!(price_of(&out, "MF:119092"), None, "N.A. is not 0");
        let texts: Vec<&str> = out.notes.iter().map(|n| n.text.as_str()).collect();
        assert!(
            texts[0].contains("MF:999999 is not in AMFI's NAV file"),
            "{texts:?}"
        );
        assert!(texts[0].contains("stockwise mf search"));
        assert!(texts[1].contains("no NAV for MF:119092"), "{texts:?}");
        // A wound-up scheme keeps its last NAV, with a warning.
        assert_eq!(price_of(&out, "MF:100001"), Some(11.5));
        assert!(texts[2].contains("01 Mar 2024"), "{texts:?}");

        let down = price_funds(&symbols, Err("HTTP 503"));
        assert!(down.quotes.is_empty());
        assert_eq!(down.notes.len(), 1);
        assert_eq!(down.notes[0].symbols.len(), 3);
        assert!(down.notes[0].text.contains("HTTP 503"));
    }

    #[test]
    fn equity_orientation_follows_the_sebi_category() {
        let eq = |cat: &str, name: &str| is_equity_oriented(cat, name);
        assert!(eq(
            "Equity Scheme - Flexi Cap Fund",
            "Parag Parikh Flexi Cap Fund"
        ));
        assert!(eq("Equity Scheme - ELSS", "Axis ELSS Tax Saver"));
        assert!(eq("Hybrid Scheme - Arbitrage Fund", "Kotak Arbitrage Fund"));
        assert!(eq("Other Scheme - Index Funds", "UTI Nifty 50 Index Fund"));
        assert!(eq("Other Scheme - Other  ETFs", "Kotak Nifty PSU Bank ETF"));
        assert!(!eq(
            "Other Scheme - Index Funds",
            "Edelweiss Nifty PSU Bond Plus SDL"
        ));
        assert!(!eq("Debt Scheme - Liquid Fund", "HDFC Liquid Fund"));
        assert!(!eq("Hybrid Scheme - Conservative Hybrid Fund", "X"));
        assert!(!eq(
            "Other Scheme - Index Funds",
            "Bharat Bond ETF - April 2030"
        ));
        assert!(!eq(
            "Other Scheme - Index Funds",
            "Nifty SDL Apr 2027 Index Fund"
        ));
        assert!(!eq("Other Scheme - Gold ETF", "Nippon Gold ETF"));
        assert!(!eq("Other Scheme - FoF Overseas", "Nasdaq 100 FoF"));
    }

    #[test]
    fn sgbs_use_nse_prices_then_yahoo_then_a_labelled_gold_estimate() {
        let rows = vec![
            sgb("SGBNOV26", "IN0020180314", Some(12_500.0)),
            sgb("SGBDEC26", "IN0020180389", None),
        ];
        let symbols = syms(&[
            "SGBNOV26.NS",
            "IN0020180314",
            "SGBDEC26.NS",
            "SGBJAN27.NS",
            "TCS.NS",
        ]);
        let yahoo = vec![Quote {
            symbol: Some("SGBJAN27.NS".into()),
            regular_market_price: Some(12_000.0),
            ..Default::default()
        }];
        let out = price_sgbs(
            &symbols,
            SgbPrices {
                list: Ok(&rows),
                saved_on: None,
            },
            &yahoo,
            Some(11_800.456),
        );
        assert_eq!(price_of(&out, "SGBNOV26.NS"), Some(12_500.0));
        assert_eq!(
            price_of(&out, "IN0020180314"),
            Some(12_500.0),
            "ISIN finds the row"
        );
        assert_eq!(out.mark("SGBNOV26.NS").unwrap().source, PriceSource::NseSgb);
        assert_eq!(
            out.mark("SGBNOV26.NS").unwrap().name.as_deref(),
            Some("SGB 2018-19 Series III")
        );
        // No trade today: the gold value, labelled as an estimate.
        assert_eq!(price_of(&out, "SGBDEC26.NS"), Some(11_800.46));
        assert_eq!(
            out.mark("SGBDEC26.NS").unwrap().source,
            PriceSource::GoldEstimate
        );
        // Yahoo's own quote is kept as it is.
        assert_eq!(price_of(&out, "SGBJAN27.NS"), None);
        assert!(out.mark("SGBJAN27.NS").is_none());
        assert_eq!(price_of(&out, "TCS.NS"), None);
        let estimate = &out.notes[0];
        assert_eq!(estimate.symbols, ["SGBDEC26.NS"]);
        assert!(estimate.text.starts_with("Estimated"), "{}", estimate.text);
        assert!(
            out.notes
                .iter()
                .any(|n| n.text.contains("2.5% yearly interest"))
        );
    }

    #[test]
    fn sgbs_when_nse_refuses() {
        let symbols = syms(&["SGBNOV26.NS"]);
        let refused = SgbPrices {
            list: Err("HTTP 403"),
            saved_on: None,
        };
        let out = price_sgbs(&symbols, refused, &[], Some(12_000.0));
        assert_eq!(
            out.mark("SGBNOV26.NS").unwrap().source,
            PriceSource::GoldEstimate
        );
        assert!(out.notes[0].text.contains("HTTP 403"), "{:?}", out.notes);

        // No gold price either: unpriced, never 0.
        let refused = SgbPrices {
            list: Err("HTTP 403"),
            saved_on: None,
        };
        let out = price_sgbs(&symbols, refused, &[], None);
        assert!(out.quotes.is_empty());
        assert!(out.notes[0].text.starts_with("No price"), "{:?}", out.notes);
    }

    #[test]
    fn restrict_keeps_only_the_named_holdings() {
        let s = schemes();
        let out = price_funds(&syms(&["MF:122639", "MF:999999"]), Ok(&s));
        let only = out.restrict(&syms(&["MF:122639"]));
        assert_eq!(only.quotes.len(), 1);
        assert_eq!(only.marks.len(), 1);
        assert!(only.notes.is_empty());
        let mut quotes = vec![Quote {
            symbol: Some("MF:122639".into()),
            regular_market_price: None,
            ..Default::default()
        }];
        out.merge_into(&mut quotes);
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].regular_market_price, Some(90.3546));
    }
}
