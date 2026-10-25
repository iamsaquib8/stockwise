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
        "psu",
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
