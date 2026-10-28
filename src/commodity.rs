//! Gold and silver: international spot in ₹ per 10 g / per kg, COMEX
//! futures, Indian gold/silver ETFs and Sovereign Gold Bonds.
//!
//! Spot is the COMEX front-month future (Yahoo `GC=F`, `SI=F`, USD per troy
//! ounce) converted at `USDINR=X`. Yahoo has no free spot quote, and MCX
//! futures are not available from a public feed, so every row says where
//! its number comes from. Indian retail and MCX prices include import duty
//! and GST; these international prices do not.
//!
//! The functions here turn quotes into rows and never fetch anything, so
//! they are tested with fixtures.

use crate::api::Quote;
use crate::output::round_opt;
use chrono::{Months, NaiveDate};
use serde::Serialize;
use serde_json::Value;

/// Grams in a troy ounce.
pub const TROY_OUNCE_GRAMS: f64 = 31.103_476_8;

pub const GOLD_FUTURE: &str = "GC=F";
pub const SILVER_FUTURE: &str = "SI=F";
pub const USDINR: &str = "USDINR=X";

/// COMEX contracts on Yahoo: (symbol, metal, contract).
pub const FUTURES: &[(&str, &str, &str)] = &[
    ("GC=F", "gold", "COMEX Gold (100 oz)"),
    ("MGC=F", "gold", "COMEX Micro Gold (10 oz)"),
    ("SI=F", "silver", "COMEX Silver (5,000 oz)"),
    ("SIL=F", "silver", "COMEX Micro Silver (1,000 oz)"),
];

/// NSE-listed gold and silver ETFs: (symbol, metal, name). Checked
/// against Yahoo Finance; prices are ₹ per unit.
pub const ETFS: &[(&str, &str, &str)] = &[
    ("GOLDBEES.NS", "gold", "Nippon India ETF Gold BeES"),
    ("SETFGOLD.NS", "gold", "SBI Gold ETF"),
    ("HDFCGOLD.NS", "gold", "HDFC Gold ETF"),
    ("GOLDIETF.NS", "gold", "ICICI Prudential Gold ETF"),
    ("GOLD1.NS", "gold", "Kotak Gold ETF"),
    ("GOLDCASE.NS", "gold", "Zerodha Gold ETF"),
    ("SILVERBEES.NS", "silver", "Nippon India Silver ETF"),
    ("SILVERIETF.NS", "silver", "ICICI Prudential Silver ETF"),
    ("HDFCSILVER.NS", "silver", "HDFC Silver ETF"),
    ("SILVER1.NS", "silver", "Kotak Silver ETF"),
];

/// `usd_per_oz` in rupees for `grams` of metal.
pub fn inr_for_grams(usd_per_oz: f64, usdinr: f64, grams: f64) -> f64 {
    usd_per_oz * usdinr / TROY_OUNCE_GRAMS * grams
}

fn find<'a>(quotes: &'a [Quote], symbol: &str) -> Option<&'a Quote> {
    quotes.iter().find(|q| {
        q.symbol
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(symbol))
    })
}

/// Today's price and the previous close, from a quote.
fn price_and_previous(q: &Quote) -> Option<(f64, Option<f64>)> {
    let price = q.regular_market_price?;
    let previous = q
        .regular_market_previous_close
        .or_else(|| q.regular_market_change.map(|c| price - c))
        .filter(|p| *p > 0.0);
    Some((price, previous))
}

/// One line of `commodity spot` (field order = JSON key order).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SpotRow {
    /// `gold` or `silver`.
    pub metal: String,
    /// `USD/oz`, `INR/g`, `INR/10g` or `INR/kg`.
    pub unit: String,
    pub price: Option<f64>,
    pub currency: String,
    pub change_percent: Option<f64>,
    pub source: String,
}

impl crate::output::CsvRow for SpotRow {
    const COLUMNS: &'static [&'static str] = &[
        "metal",
        "unit",
        "price",
        "currency",
        "change_percent",
        "source",
    ];
}

/// Spot rows for gold (USD/oz, ₹/g, ₹/10 g) and silver (USD/oz, ₹/g,
/// ₹/kg). A missing quote gives a row without a price, never a 0.
pub fn spot_rows(quotes: &[Quote]) -> Vec<SpotRow> {
    let fx = find(quotes, USDINR).and_then(price_and_previous);
    let mut rows = Vec::new();
    for (metal, future, inr_units) in [
        (
            "gold",
            GOLD_FUTURE,
            &[("INR/g", 1.0), ("INR/10g", 10.0)][..],
        ),
        (
            "silver",
            SILVER_FUTURE,
            &[("INR/g", 1.0), ("INR/kg", 1000.0)][..],
        ),
    ] {
        let q = find(quotes, future);
        let usd = q.and_then(price_and_previous);
        rows.push(SpotRow {
            metal: metal.into(),
            unit: "USD/oz".into(),
            price: usd.map(|(p, _)| p),
            currency: "USD".into(),
            change_percent: q.and_then(|q| q.regular_market_change_percent),
            source: format!("COMEX {future}"),
        });
        for (unit, grams) in inr_units {
            let (price, change) = match (usd, fx) {
                (Some((p, prev_p)), Some((r, prev_r))) => {
                    let now = inr_for_grams(p, r, *grams);
                    let change = prev_p
                        .zip(prev_r)
                        .map(|(pp, pr)| (now / inr_for_grams(pp, pr, *grams) - 1.0) * 100.0);
                    (Some(now), change)
                }
                _ => (None, None),
            };
            rows.push(SpotRow {
                metal: metal.into(),
                unit: (*unit).into(),
                price: round_opt(price, 2),
                currency: "INR".into(),
                change_percent: round_opt(change, 2),
                source: format!("COMEX {future} × {USDINR}"),
            });
        }
    }
    rows
}

/// Gold in ₹ per gram (international, before duty), if both quotes exist.
pub fn gold_inr_per_gram(quotes: &[Quote]) -> Option<f64> {
    let usd = find(quotes, GOLD_FUTURE)?.regular_market_price?;
    let fx = find(quotes, USDINR)?.regular_market_price?;
    Some(inr_for_grams(usd, fx, 1.0))
}

/// One instrument in `commodity futures` / `commodity etfs`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InstrumentRow {
    pub symbol: String,
    pub metal: String,
    pub name: String,
    /// Yahoo's short name: the contract month for futures ("Gold Dec 26").
    pub contract: Option<String>,
    pub price: Option<f64>,
    pub change: Option<f64>,
    pub change_percent: Option<f64>,
    pub volume: Option<u64>,
    pub currency: Option<String>,
}

impl crate::output::CsvRow for InstrumentRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "metal",
        "name",
        "contract",
        "price",
        "change",
        "change_percent",
        "volume",
        "currency",
    ];
}

/// Rows for a fixed list, in list order; instruments without a quote keep
/// their row with empty prices.
pub fn instrument_rows(list: &[(&str, &str, &str)], quotes: &[Quote]) -> Vec<InstrumentRow> {
    list.iter()
        .map(|(symbol, metal, name)| {
            let q = find(quotes, symbol);
            InstrumentRow {
                symbol: symbol.to_string(),
                metal: metal.to_string(),
                name: name.to_string(),
                contract: q.and_then(|q| q.short_name.clone()),
                price: q.and_then(|q| q.regular_market_price),
                change: q.and_then(|q| q.regular_market_change),
                change_percent: q.and_then(|q| q.regular_market_change_percent),
                volume: q.and_then(|q| q.regular_market_volume),
                currency: q.and_then(|q| q.currency.clone()),
            }
        })
        .collect()
}

// ── Sovereign Gold Bonds ──

/// NSE's list of traded SGB tranches (series `GB`).
pub const NSE_SGB_PATH: &str = "/api/sovereign-gold-bonds";

/// Interest on SGBs still outstanding: 2.5% a year on the issue price,
/// paid half-yearly (RBI SGB FAQ).
pub const SGB_COUPON_PCT: f64 = 2.5;

/// An outstanding SGB tranche: reference data that NSE's list lacks
/// (exact issue date, hence maturity). Issue dates and prices come from
/// public tranche tables (33 of the 44 issue dates checked against RBI press
/// releases); the NSE symbols come from a public list and are only used when
/// NSE's own list cannot be fetched. Tranches that have matured are left out.
#[derive(Debug, Clone, Copy)]
pub struct Tranche {
    pub symbol: &'static str,
    pub isin: &'static str,
    pub series: &'static str,
    pub issued: (i32, u32, u32),
    /// ₹ per gram (the nominal, offline issue price).
    pub issue_price: f64,
}

impl Tranche {
    pub fn issue_date(&self) -> NaiveDate {
        let (y, m, d) = self.issued;
        NaiveDate::from_ymd_opt(y, m, d).expect("valid tranche date")
    }

    /// Nominal final redemption: 8 years after issue (RBI moves it a day or
    /// two earlier when that is a holiday).
    pub fn maturity(&self) -> NaiveDate {
        self.issue_date() + Months::new(96)
    }

    /// Premature redemption opens 5 years after issue.
    pub fn early_redemption_from(&self) -> NaiveDate {
        self.issue_date() + Months::new(60)
    }
}

pub const SGB_TRANCHES: &[Tranche] = &[
    Tranche {
        symbol: "SGBNOV26",
        isin: "IN0020180314",
        series: "2018-19 Series III",
        issued: (2018, 11, 13),
        issue_price: 3183.0,
    },
    Tranche {
        symbol: "SGBDEC26",
        isin: "IN0020180389",
        series: "2018-19 Series IV",
        issued: (2019, 1, 1),
        issue_price: 3119.0,
    },
    Tranche {
        symbol: "SGBJAN27",
        isin: "IN0020180462",
        series: "2018-19 Series V",
        issued: (2019, 1, 22),
        issue_price: 3214.0,
    },
    Tranche {
        symbol: "SGBFEB27",
        isin: "IN0020180561",
        series: "2018-19 Series VI",
        issued: (2019, 2, 12),
        issue_price: 3326.0,
    },
    Tranche {
        symbol: "SGBJUN27",
        isin: "IN0020190073",
        series: "2019-20 Series I",
        issued: (2019, 6, 11),
        issue_price: 3196.0,
    },
    Tranche {
        symbol: "SGBJUL27",
        isin: "IN0020190081",
        series: "2019-20 Series II",
        issued: (2019, 7, 16),
        issue_price: 3443.0,
    },
    Tranche {
        symbol: "SGBAUG27",
        isin: "IN0020190107",
        series: "2019-20 Series III",
        issued: (2019, 8, 14),
        issue_price: 3499.0,
    },
    Tranche {
        symbol: "SGBSEP27",
        isin: "IN0020190115",
        series: "2019-20 Series IV",
        issued: (2019, 9, 17),
        issue_price: 3890.0,
    },
    Tranche {
        symbol: "SGBOCT27",
        isin: "IN0020190370",
        series: "2019-20 Series V",
        issued: (2019, 10, 15),
        issue_price: 3788.0,
    },
    Tranche {
        symbol: "SGBOCT27VI",
        isin: "IN0020190388",
        series: "2019-20 Series VI",
        issued: (2019, 10, 30),
        issue_price: 3835.0,
    },
    Tranche {
        symbol: "SGBDC27VII",
        isin: "IN0020190461",
        series: "2019-20 Series VII",
        issued: (2019, 12, 10),
        issue_price: 3795.0,
    },
    Tranche {
        symbol: "SGBJ28VIII",
        isin: "IN0020190537",
        series: "2019-20 Series VIII",
        issued: (2020, 1, 21),
        issue_price: 4016.0,
    },
    Tranche {
        symbol: "SGBFEB28IX",
        isin: "IN0020190545",
        series: "2019-20 Series IX",
        issued: (2020, 2, 11),
        issue_price: 4070.0,
    },
    Tranche {
        symbol: "SGBMAR28X",
        isin: "IN0020190552",
        series: "2019-20 Series X",
        issued: (2020, 3, 11),
        issue_price: 4260.0,
    },
    Tranche {
        symbol: "SGBAPR28I",
        isin: "IN0020200062",
        series: "2020-21 Series I",
        issued: (2020, 4, 28),
        issue_price: 4639.0,
    },
    Tranche {
        symbol: "SGBMAY28",
        isin: "IN0020200088",
        series: "2020-21 Series II",
        issued: (2020, 5, 19),
        issue_price: 4590.0,
    },
    Tranche {
        symbol: "SGBJUN28",
        isin: "IN0020200104",
        series: "2020-21 Series III",
        issued: (2020, 6, 16),
        issue_price: 4677.0,
    },
    Tranche {
        symbol: "SGBJUL28IV",
        isin: "IN0020200146",
        series: "2020-21 Series IV",
        issued: (2020, 7, 14),
        issue_price: 4852.0,
    },
    Tranche {
        symbol: "SGBAUG28V",
        isin: "IN0020200161",
        series: "2020-21 Series V",
        issued: (2020, 8, 11),
        issue_price: 5334.0,
    },
    Tranche {
        symbol: "SGBSEP28VI",
        isin: "IN0020200195",
        series: "2020-21 Series VI",
        issued: (2020, 9, 8),
        issue_price: 5117.0,
    },
    Tranche {
        symbol: "SGBOC28VII",
        isin: "IN0020200203",
        series: "2020-21 Series VII",
        issued: (2020, 10, 20),
        issue_price: 5051.0,
    },
    Tranche {
        symbol: "SGBN28VIII",
        isin: "IN0020200286",
        series: "2020-21 Series VIII",
        issued: (2020, 11, 18),
        issue_price: 5177.0,
    },
    Tranche {
        symbol: "SGBJAN29IX",
        isin: "IN0020200377",
        series: "2020-21 Series IX",
        issued: (2021, 1, 5),
        issue_price: 5000.0,
    },
    Tranche {
        symbol: "SGBJAN29X",
        isin: "IN0020200385",
        series: "2020-21 Series X",
        issued: (2021, 1, 19),
        issue_price: 5104.0,
    },
    Tranche {
        symbol: "SGBFEB29XI",
        isin: "IN0020200393",
        series: "2020-21 Series XI",
        issued: (2021, 2, 9),
        issue_price: 4912.0,
    },
    Tranche {
        symbol: "SGBMR29XII",
        isin: "IN0020200427",
        series: "2020-21 Series XII",
        issued: (2021, 3, 9),
        issue_price: 4662.0,
    },
    Tranche {
        symbol: "SGBMAY29I",
        isin: "IN0020210053",
        series: "2021-22 Series I",
        issued: (2021, 5, 25),
        issue_price: 4777.0,
    },
    Tranche {
        symbol: "SGBJUN29II",
        isin: "IN0020210061",
        series: "2021-22 Series II",
        issued: (2021, 6, 1),
        issue_price: 4842.0,
    },
    Tranche {
        symbol: "SGBJU29III",
        isin: "IN0020210087",
        series: "2021-22 Series III",
        issued: (2021, 6, 8),
        issue_price: 4889.0,
    },
    Tranche {
        symbol: "SGBJUL29IV",
        isin: "IN0020210111",
        series: "2021-22 Series IV",
        issued: (2021, 7, 20),
        issue_price: 4807.0,
    },
    Tranche {
        symbol: "SGBAUG29V",
        isin: "IN0020210129",
        series: "2021-22 Series V",
        issued: (2021, 8, 17),
        issue_price: 4790.0,
    },
    Tranche {
        symbol: "SGBSEP29VI",
        isin: "IN0020210145",
        series: "2021-22 Series VI",
        issued: (2021, 9, 7),
        issue_price: 4732.0,
    },
    Tranche {
        symbol: "SGBNV29VII",
        isin: "IN0020210178",
        series: "2021-22 Series VII",
        issued: (2021, 11, 2),
        issue_price: 4761.0,
    },
    Tranche {
        symbol: "SGBD29VIII",
        isin: "IN0020210228",
        series: "2021-22 Series VIII",
        issued: (2021, 12, 7),
        issue_price: 4791.0,
    },
    Tranche {
        symbol: "SGBJAN30IX",
        isin: "IN0020210236",
        series: "2021-22 Series IX",
        issued: (2022, 1, 18),
        issue_price: 4786.0,
    },
    Tranche {
        symbol: "SGBMAR30X",
        isin: "IN0020210319",
        series: "2021-22 Series X",
        issued: (2022, 3, 8),
        issue_price: 5109.0,
    },
    Tranche {
        symbol: "SGBJUN30",
        isin: "IN0020220045",
        series: "2022-23 Series I",
        issued: (2022, 6, 28),
        issue_price: 5091.0,
    },
    Tranche {
        symbol: "SGBAUG30",
        isin: "IN0020220078",
        series: "2022-23 Series II",
        issued: (2022, 8, 30),
        issue_price: 5197.0,
    },
    Tranche {
        symbol: "SGBDE30III",
        isin: "IN0020220110",
        series: "2022-23 Series III",
        issued: (2022, 12, 27),
        issue_price: 5409.0,
    },
    Tranche {
        symbol: "SGBMAR31IV",
        isin: "IN0020220169",
        series: "2022-23 Series IV",
        issued: (2023, 3, 14),
        issue_price: 5611.0,
    },
    Tranche {
        symbol: "SGBJUN31I",
        isin: "IN0020230069",
        series: "2023-24 Series I",
        issued: (2023, 6, 27),
        issue_price: 5926.0,
    },
    Tranche {
        symbol: "SGBSEP31II",
        isin: "IN0020230093",
        series: "2023-24 Series II",
        issued: (2023, 9, 20),
        issue_price: 5923.0,
    },
    Tranche {
        symbol: "SGBDEC31III",
        isin: "IN0020230168",
        series: "2023-24 Series III",
        issued: (2023, 12, 28),
        issue_price: 6199.0,
    },
    Tranche {
        symbol: "SGBFEB32IV",
        isin: "IN0020230184",
        series: "2023-24 Series IV",
        issued: (2024, 2, 21),
        issue_price: 6263.0,
    },
];

/// One row of NSE's SGB list, as StockWise reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct NseSgb {
    pub symbol: String,
    pub isin: Option<String>,
    pub name: Option<String>,
    pub issue_price: Option<f64>,
    pub price: Option<f64>,
    pub change_percent: Option<f64>,
    pub volume: Option<u64>,
}

/// A JSON number or numeric string (`"6014.9"`, `"1,234"`), as NSE mixes both.
fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.replace(',', "").trim().parse().ok(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}

/// Rows of `/api/sovereign-gold-bonds`. Errors when the shape is not the
/// one StockWise knows (so a redesign is reported, not shown as empty).
pub fn parse_nse_sgb(json: &Value) -> Result<Vec<NseSgb>, &'static str> {
    let data = json
        .get("data")
        .and_then(Value::as_array)
        .ok_or("no data array")?;
    Ok(data
        .iter()
        .filter_map(|row| {
            let symbol = row.get("symbol")?.as_str()?.trim().to_string();
            let meta = row.get("meta");
            let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_string);
            Some(NseSgb {
                isin: text(meta.and_then(|m| m.get("isin"))),
                name: text(meta.and_then(|m| m.get("companyName"))),
                issue_price: row.get("issue_price").and_then(num),
                // A last price of 0 means no trade, not a free bond.
                price: row.get("ltP").and_then(num).filter(|p| *p > 0.0),
                change_percent: row.get("per").and_then(num),
                volume: row.get("qty").and_then(num).map(|q| q.max(0.0) as u64),
                symbol,
            })
        })
        .collect())
}

/// NSE's SGB list as [`nse_sgb_list`] returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct SgbList {
    pub rows: Vec<NseSgb>,
    /// When NSE could not be reached and an older saved copy is used: when
    /// that copy was fetched.
    pub saved_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// NSE's list of traded SGBs through the shared NSE session (polite,
/// cached 5 minutes); a copy is kept on disk so a refusal still shows the
/// last prices seen, labelled with their date.
pub async fn nse_sgb_list() -> anyhow::Result<SgbList> {
    use crate::pagecache;
    use anyhow::{Context, bail};
    use std::time::Duration;
    const CACHE: &str = "nse_sgb.json";
    let valid = |t: &str| {
        serde_json::from_str::<serde_json::Value>(t)
            .ok()
            .is_some_and(|v| parse_nse_sgb(&v).is_ok_and(|rows| !rows.is_empty()))
    };
    let parse = |text: &str| -> anyhow::Result<Vec<NseSgb>> {
        let json: serde_json::Value = serde_json::from_str(text).context("NSE sent no JSON")?;
        parse_nse_sgb(&json).map_err(|e| anyhow::anyhow!("NSE changed its SGB list format ({e})"))
    };
    let cached = pagecache::read(CACHE, Duration::from_secs(300), valid);
    if let Some(page) = &cached
        && !page.stale
    {
        return Ok(SgbList {
            rows: parse(&page.text)?,
            saved_at: None,
        });
    }
    let session = crate::providers::nse::shared_session();
    match session
        .get_json(NSE_SGB_PATH, &[], Duration::from_secs(300))
        .await
    {
        Ok(json) => {
            let rows = parse_nse_sgb(&json)
                .map_err(|e| anyhow::anyhow!("NSE changed its SGB list format ({e})"))?;
            if rows.is_empty() {
                bail!("NSE's SGB list is empty");
            }
            pagecache::write(CACHE, &json.to_string());
            Ok(SgbList {
                rows,
                saved_at: None,
            })
        }
        Err(e) => {
            let page = pagecache::fall_back(cached, anyhow::anyhow!("{e}"))?;
            Ok(SgbList {
                rows: parse(&page.text)?,
                saved_at: Some(page.fetched),
            })
        }
    }
}

/// One line of `commodity sgb` (field order = JSON key order).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SgbRow {
    pub symbol: String,
    pub isin: Option<String>,
    pub series: Option<String>,
    pub issue_date: Option<NaiveDate>,
    /// ₹ per gram paid at issue.
    pub issue_price: Option<f64>,
    /// Nominal final redemption (issue + 8 years).
    pub maturity: Option<NaiveDate>,
    pub early_redemption_from: Option<NaiveDate>,
    pub years_to_maturity: Option<f64>,
    /// Last traded price on NSE, ₹ per unit (1 g).
    pub price: Option<f64>,
    pub change_percent: Option<f64>,
    pub volume: Option<u64>,
    /// 1 g of gold at the international spot price in ₹ (no duty or GST).
    pub gold_value: Option<f64>,
    /// Price over `gold_value`, in percent (negative = discount).
    pub premium_percent: Option<f64>,
    /// ₹ of interest a year per unit: 2.5% of the issue price.
    pub interest_per_year: Option<f64>,
    /// That interest over today's price, in percent.
    pub current_yield_percent: Option<f64>,
}

impl crate::output::CsvRow for SgbRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "isin",
        "series",
        "issue_date",
        "issue_price",
        "maturity",
        "early_redemption_from",
        "years_to_maturity",
        "price",
        "change_percent",
        "volume",
        "gold_value",
        "premium_percent",
        "interest_per_year",
        "current_yield_percent",
    ];
}

fn tranche_for(isin: Option<&str>, symbol: &str) -> Option<&'static Tranche> {
    SGB_TRANCHES.iter().find(|t| {
        isin.is_some_and(|i| i.eq_ignore_ascii_case(t.isin))
            || t.symbol.eq_ignore_ascii_case(symbol)
    })
}

/// SGB rows sorted by maturity. With NSE's list, one row per traded
/// tranche (joined to the reference table by ISIN); without it, one row
/// per outstanding tranche and no market price — never a guessed one.
pub fn sgb_rows(
    nse: Option<&[NseSgb]>,
    gold_per_gram: Option<f64>,
    today: NaiveDate,
) -> Vec<SgbRow> {
    let row = |symbol: String, live: Option<&NseSgb>, t: Option<&Tranche>| {
        let issue_price = t
            .map(|t| t.issue_price)
            .or(live.and_then(|l| l.issue_price));
        let price = live.and_then(|l| l.price);
        let maturity = t.map(Tranche::maturity);
        let interest = issue_price.map(|p| p * SGB_COUPON_PCT / 100.0);
        SgbRow {
            symbol,
            isin: t
                .map(|t| t.isin.to_string())
                .or(live.and_then(|l| l.isin.clone())),
            series: t.map(|t| t.series.to_string()),
            issue_date: t.map(Tranche::issue_date),
            issue_price,
            maturity,
            early_redemption_from: t.map(Tranche::early_redemption_from),
            years_to_maturity: round_opt(
                maturity.map(|m| (m - today).num_days() as f64 / 365.25),
                2,
            ),
            price,
            change_percent: live.and_then(|l| l.change_percent),
            volume: live.and_then(|l| l.volume),
            gold_value: round_opt(gold_per_gram, 2),
            premium_percent: round_opt(
                price.zip(gold_per_gram).map(|(p, g)| (p / g - 1.0) * 100.0),
                2,
            ),
            interest_per_year: round_opt(interest, 2),
            current_yield_percent: round_opt(interest.zip(price).map(|(i, p)| i / p * 100.0), 2),
        }
    };
    let mut rows: Vec<SgbRow> = match nse {
        Some(list) => list
            .iter()
            .map(|l| {
                row(
                    l.symbol.clone(),
                    Some(l),
                    tranche_for(l.isin.as_deref(), &l.symbol),
                )
            })
            .collect(),
        None => SGB_TRANCHES
            .iter()
            .filter(|t| t.maturity() > today)
            .map(|t| row(t.symbol.to_string(), None, Some(t)))
            .collect(),
    };
    rows.sort_by_key(|r| (r.maturity.is_none(), r.maturity, r.symbol.clone()));
    rows
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn quote(symbol: &str, price: f64, previous: f64, currency: &str) -> Quote {
        Quote {
            symbol: Some(symbol.to_string()),
            short_name: Some(format!("{symbol} name")),
            regular_market_price: Some(price),
            regular_market_previous_close: Some(previous),
            regular_market_change: Some(price - previous),
            regular_market_change_percent: Some((price / previous - 1.0) * 100.0),
            currency: Some(currency.to_string()),
            ..Default::default()
        }
    }

    pub fn fixture() -> Vec<Quote> {
        vec![
            quote("GC=F", 4000.0, 4050.0, "USD"),
            quote("SI=F", 50.0, 50.0, "USD"),
            quote("USDINR=X", 95.0, 94.0, "INR"),
        ]
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn ounces_to_grams_in_rupees() {
        // $3110.34768/oz at ₹100/$ = ₹10,000 a gram.
        assert!(close(inr_for_grams(3_110.347_68, 100.0, 1.0), 10_000.0));
        assert!(close(inr_for_grams(3_110.347_68, 100.0, 10.0), 100_000.0));
        assert!(close(inr_for_grams(31.103_476_8, 100.0, 1000.0), 100_000.0));
    }

    #[test]
    fn spot_rows_convert_and_combine_changes() {
        let rows = spot_rows(&fixture());
        let units: Vec<_> = rows
            .iter()
            .map(|r| format!("{}:{}", r.metal, r.unit))
            .collect();
        assert_eq!(
            units,
            [
                "gold:USD/oz",
                "gold:INR/g",
                "gold:INR/10g",
                "silver:USD/oz",
                "silver:INR/g",
                "silver:INR/kg"
            ]
        );
        let ten_g = &rows[2];
        let want = 4000.0 * 95.0 / TROY_OUNCE_GRAMS * 10.0;
        assert!((ten_g.price.unwrap() - want).abs() < 0.005);
        assert_eq!(ten_g.currency, "INR");
        // Gold −1.23% in USD, rupee 1.06% weaker: (4000×95)/(4050×94) − 1.
        assert_eq!(ten_g.change_percent, Some(-0.18));
        // Silver flat in USD: the ₹ change is the currency move alone.
        let kg = &rows[5];
        assert_eq!(kg.change_percent, Some(1.06));
        assert!(close(
            gold_inr_per_gram(&fixture()).unwrap(),
            4000.0 * 95.0 / TROY_OUNCE_GRAMS
        ));
    }

    #[test]
    fn missing_quotes_never_become_zero() {
        // No exchange rate: USD rows keep their price, ₹ rows have none.
        let quotes = vec![quote("GC=F", 4000.0, 4050.0, "USD")];
        let rows = spot_rows(&quotes);
        assert_eq!(rows[0].price, Some(4000.0));
        assert!(rows[1..3].iter().all(|r| r.price.is_none()));
        assert!(rows[3..].iter().all(|r| r.price.is_none()));
        assert_eq!(gold_inr_per_gram(&quotes), None);
        let etfs = instrument_rows(ETFS, &quotes);
        assert_eq!(etfs.len(), ETFS.len());
        assert!(etfs.iter().all(|r| r.price.is_none()));
    }

    /// Shaped like NSE's `/api/sovereign-gold-bonds` (field names and mixed
    /// string/number types from a published sample); the values are test
    /// data, not market prices.
    const NSE_SGB: &str = r#"{
        "timestamp": "23-Oct-2026 16:00:00",
        "data": [
            {"symbol": "SGBAUG28V", "issue_price": "5334", "ltP": "12500.5", "chn": "100",
             "per": "0.81", "qty": "1,250", "prevClose": "12400.5", "maturityDate": "AUG28",
             "yPC": 12.5, "meta": {"symbol": "SGBAUG28V", "isin": "IN0020200161",
             "companyName": "Sovereign Gold Bonds 2.50% AUG 2028 Tr-V", "activeSeries": ["GB"]}},
            {"symbol": "SGBFEB32IV", "issue_price": "6263", "ltP": "0", "chn": "0", "per": "0",
             "qty": "0", "meta": {"isin": "IN0020230184"}},
            {"symbol": "SGBNEW99", "issue_price": 7000, "ltP": 11000, "per": -0.5, "qty": 3,
             "meta": {"isin": "IN0029999999"}}
        ]
    }"#;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn nse_sgb_list_parses_strings_and_numbers() {
        let json: Value = serde_json::from_str(NSE_SGB).unwrap();
        let rows = parse_nse_sgb(&json).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].symbol, "SGBAUG28V");
        assert_eq!(rows[0].isin.as_deref(), Some("IN0020200161"));
        assert_eq!(rows[0].price, Some(12_500.5));
        assert_eq!(rows[0].issue_price, Some(5334.0));
        assert_eq!(rows[0].volume, Some(1250));
        // No trade today: no price, not ₹0.
        assert_eq!(rows[1].price, None);
        assert_eq!(rows[2].price, Some(11_000.0));
        assert!(parse_nse_sgb(&serde_json::json!({"error": "x"})).is_err());
    }

    #[test]
    fn sgb_rows_join_nse_prices_to_tranches() {
        let json: Value = serde_json::from_str(NSE_SGB).unwrap();
        let live = parse_nse_sgb(&json).unwrap();
        let today = day(2026, 10, 25);
        let rows = sgb_rows(Some(&live), Some(12_000.0), today);
        assert_eq!(rows.len(), 3);
        let aug28 = rows.iter().find(|r| r.symbol == "SGBAUG28V").unwrap();
        assert_eq!(aug28.series.as_deref(), Some("2020-21 Series V"));
        assert_eq!(aug28.issue_date, Some(day(2020, 8, 11)));
        assert_eq!(aug28.maturity, Some(day(2028, 8, 11)));
        assert_eq!(aug28.early_redemption_from, Some(day(2025, 8, 11)));
        // 12,500.5 / 12,000 − 1 = +4.17%.
        assert_eq!(aug28.premium_percent, Some(4.17));
        // 2.5% of ₹5,334 = ₹133.35 a year: 1.07% of today's price.
        assert_eq!(aug28.interest_per_year, Some(133.35));
        assert_eq!(aug28.current_yield_percent, Some(1.07));
        let feb32 = rows.iter().find(|r| r.symbol == "SGBFEB32IV").unwrap();
        assert_eq!(feb32.price, None);
        assert_eq!(feb32.premium_percent, None);
        assert_eq!(feb32.current_yield_percent, None);
        // A tranche missing from the reference table keeps NSE's data.
        let new = rows.last().unwrap();
        assert_eq!(new.symbol, "SGBNEW99");
        assert_eq!(new.maturity, None);
        assert_eq!(new.issue_price, Some(7000.0));
        // Sorted by maturity.
        assert_eq!(rows[0].symbol, "SGBAUG28V");
    }

    #[test]
    fn sgb_fallback_lists_outstanding_tranches_without_prices() {
        let rows = sgb_rows(None, None, day(2026, 10, 25));
        assert_eq!(rows.len(), SGB_TRANCHES.len());
        assert!(
            rows.iter()
                .all(|r| r.price.is_none() && r.premium_percent.is_none())
        );
        assert_eq!(rows[0].maturity, Some(day(2026, 11, 13)));
        // After November 2026 the first tranche has matured and drops out.
        let later = sgb_rows(None, None, day(2026, 12, 1));
        assert_eq!(later.len(), SGB_TRANCHES.len() - 1);
    }

    #[test]
    fn tranche_table_is_consistent() {
        let mut isins = std::collections::HashSet::new();
        for t in SGB_TRANCHES {
            assert!(isins.insert(t.isin), "{} twice", t.isin);
            assert!(
                t.isin.starts_with("IN002") && t.isin.len() == 12,
                "{}",
                t.isin
            );
            assert!(t.symbol.starts_with("SGB"), "{}", t.symbol);
            assert!((2000.0..10_000.0).contains(&t.issue_price), "{}", t.symbol);
            assert_eq!(
                (t.maturity() - t.issue_date()).num_days() / 365,
                8,
                "{}",
                t.symbol
            );
        }
        let last = SGB_TRANCHES.last().unwrap();
        assert_eq!(last.issue_date(), day(2024, 2, 21));
    }

    #[test]
    fn instrument_rows_follow_the_list() {
        let quotes = vec![quote("SI=F", 50.0, 49.0, "USD")];
        let rows = instrument_rows(FUTURES, &quotes);
        assert_eq!(rows[2].symbol, "SI=F");
        assert_eq!(rows[2].price, Some(50.0));
        assert_eq!(rows[2].contract.as_deref(), Some("SI=F name"));
        assert_eq!(rows[0].price, None);
        assert!(ETFS.iter().all(|(s, ..)| s.ends_with(".NS")));
    }
}
