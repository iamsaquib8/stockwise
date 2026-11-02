//! Option chains: the data model, parsers for Yahoo Finance (US), NSE and
//! BSE (India) responses, and the chain-wide numbers traders read — put/call
//! ratio, max pain, open-interest walls — plus per-contract IV and greeks
//! from `greeks.rs`.
//!
//! NSE's option-chain API serves browser sessions: `NseClient` loads the
//! public option-chain page once for cookies, then makes at most two API
//! calls, spaced out, and gives up with a clear error instead of retrying.
//! BSE's derivatives API (behind bseindia.com's option-chain page) is
//! called the same way by `BseClient`: one session, calls spaced out,
//! answers cached for a minute, a clear error when BSE refuses.

use crate::api::{YahooClient, build_url};
use crate::greeks::{self, BsInput, Greeks, OptionKind};
use crate::market::{self, Market};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// NSE index option underlyings (everything else is a stock).
pub const NSE_INDICES: &[&str] = &["NIFTY", "BANKNIFTY", "FINNIFTY", "MIDCPNIFTY", "NIFTYNXT50"];

/// BSE index option underlyings and BSE's scrip codes for them.
pub const BSE_INDICES: &[(&str, u32)] = &[("SENSEX", 1), ("BANKEX", 12), ("SENSEX50", 47)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChainSource {
    Nse,
    Bse,
    Yahoo,
}

/// Classification of open-interest and price changes for a derivative contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(clippy::enum_variant_names)]
pub enum Buildup {
    /// Price up, OI up: Aggressive long positioning / bullish continuation.
    #[serde(rename = "long_buildup")]
    LongBuildup,
    /// Price up, OI down: Short covering rally.
    #[serde(rename = "short_covering")]
    ShortCovering,
    /// Price down, OI up: Aggressive short positioning / bearish continuation.
    #[serde(rename = "short_buildup")]
    ShortBuildup,
    /// Price down, OI down: Long unwinding / decline.
    #[serde(rename = "long_unwinding")]
    LongUnwinding,
}

impl Buildup {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LongBuildup => "LB",
            Self::ShortCovering => "SC",
            Self::ShortBuildup => "SB",
            Self::LongUnwinding => "LU",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::LongBuildup => "Long Build-up",
            Self::ShortCovering => "Short Covering",
            Self::ShortBuildup => "Short Build-up",
            Self::LongUnwinding => "Long Unwinding",
        }
    }

    pub fn sentiment(&self) -> &'static str {
        match self {
            Self::LongBuildup => "Bullish",
            Self::ShortCovering => "Mildly Bullish",
            Self::ShortBuildup => "Bearish",
            Self::LongUnwinding => "Mildly Bearish",
        }
    }
}

/// One call or put at one strike. Missing fields stay `None`; NSE reports
/// "no trade" as 0, which the parser turns into `None` for prices and IV.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    pub last: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub price_change: Option<f64>,
    pub volume: Option<u64>,
    pub open_interest: Option<u64>,
    pub oi_change: Option<i64>,
    /// Implied volatility as reported by the source, as a decimal.
    pub source_iv: Option<f64>,
}

impl Contract {
    /// Mid of a two-sided market, else the last traded price.
    pub fn market_price(&self) -> Option<f64> {
        match (self.bid, self.ask) {
            (Some(b), Some(a)) if b > 0.0 && a >= b => Some((a + b) / 2.0),
            _ => self.last.filter(|p| *p > 0.0),
        }
    }

    /// Classifies open-interest build-up from price change and OI change.
    pub fn buildup(&self) -> Option<Buildup> {
        let p = self.price_change?;
        let o = self.oi_change?;
        if p > 0.0 && o > 0 {
            Some(Buildup::LongBuildup)
        } else if p > 0.0 && o < 0 {
            Some(Buildup::ShortCovering)
        } else if p < 0.0 && o > 0 {
            Some(Buildup::ShortBuildup)
        } else if p < 0.0 && o < 0 {
            Some(Buildup::LongUnwinding)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChainRow {
    pub strike: f64,
    pub call: Option<Contract>,
    pub put: Option<Contract>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OptionChain {
    pub symbol: String,
    pub source: ChainSource,
    pub underlying: Option<f64>,
    pub currency: Option<String>,
    pub expiry: NaiveDate,
    pub expiries: Vec<NaiveDate>,
    /// When the source priced the chain (NSE's timestamp, Yahoo's quote time).
    pub as_of: Option<DateTime<Utc>>,
    pub rows: Vec<ChainRow>,
    /// Units of the underlying per contract: from the order-book quantities
    /// on NSE/BSE (always whole lots), 100 for standard US contracts.
    /// `None` when the chain does not show it.
    pub lot_size: Option<u32>,
}

impl OptionChain {
    /// Years from the data timestamp (or now, if unknown) to expiry: the
    /// prices are as of that moment, so IV and greeks are too.
    pub fn years_to_expiry(&self, now: DateTime<Utc>) -> f64 {
        let at = self.as_of.filter(|t| *t < now).unwrap_or(now);
        years_to_expiry(self.expiry, self.source, at)
    }
}

/// NSE's `19-Oct-2026 15:40:00` timestamps (IST) in UTC.
pub fn parse_nse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    parse_ist(s, "%d-%b-%Y %H:%M:%S")
}

/// BSE's `23 Oct 2026 | 19:42` timestamps (IST) in UTC.
pub fn parse_bse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    parse_ist(s, "%d %b %Y | %H:%M")
}

fn parse_ist(s: &str, format: &str) -> Option<DateTime<Utc>> {
    let ist = chrono::FixedOffset::east_opt(5 * 3600 + 1800)?;
    chrono::NaiveDateTime::parse_from_str(s.trim(), format)
        .ok()?
        .and_local_timezone(ist)
        .single()
        .map(|t| t.with_timezone(&Utc))
}

/// Where an option chain comes from for a user-typed symbol.
#[derive(Debug, PartialEq)]
pub enum Route {
    /// NSE symbol (`NIFTY`, `RELIANCE`) and whether it is an index.
    Nse { symbol: String, index: bool },
    /// BSE index (`SENSEX`, with its scrip code) or stock (`RELIANCE` from
    /// `RELIANCE.BO`, scrip code looked up from BSE's list).
    Bse {
        symbol: String,
        scrip_cd: Option<u32>,
    },
    /// Yahoo Finance symbol (`AAPL`, `SPY`).
    Yahoo(String),
}

/// SENSEX / BANKEX / SENSEX50 under the names people and Yahoo use.
fn bse_index(upper: &str) -> Option<(&'static str, u32)> {
    let name = match upper {
        "SENSEX" | "^BSESN" | "BSESN" | "BSE SENSEX" => "SENSEX",
        "BANKEX" | "BSE-BANK" | "BSE-BANK.BO" | "BSEBANKEX" | "BSE BANKEX" => "BANKEX",
        "SENSEX50" | "SENSEX 50" | "SX50" | "BSE SENSEX 50" => "SENSEX50",
        _ => return None,
    };
    BSE_INDICES.iter().copied().find(|(n, _)| *n == name)
}

/// Indian market → NSE (`RELIANCE`, `RELIANCE.NS`, `^NSEI` → `NIFTY`), except
/// BSE's indices (`SENSEX`, `BANKEX`) and `.BO` stocks → BSE; the US market,
/// a global exchange or another suffix → Yahoo.
pub fn route(symbol: &str, market: Market) -> Route {
    let upper = symbol.trim().to_uppercase();
    if matches!(market, Market::Us | Market::Global(_)) {
        return Route::Yahoo(market::resolve_symbol(&upper, market));
    }
    if let Some((name, code)) = bse_index(&upper) {
        return Route::Bse {
            symbol: name.to_string(),
            scrip_cd: Some(code),
        };
    }
    if let Some(stock) = upper.strip_suffix(".BO").filter(|s| !s.is_empty()) {
        return Route::Bse {
            symbol: stock.to_string(),
            scrip_cd: stock.parse().ok(),
        };
    }
    let nse = match upper.as_str() {
        "^NSEI" | "NIFTY50" | "NIFTY 50" => "NIFTY".to_string(),
        "^NSEBANK" | "BANKNIFTY" | "NIFTYBANK" | "NIFTY BANK" => "BANKNIFTY".to_string(),
        s if s.ends_with(".NS") => s.trim_end_matches(".NS").to_string(),
        s if !s.contains(['.', '^', '=']) => s.to_string(),
        _ => return Route::Yahoo(market::resolve_symbol(&upper, market)),
    };
    let index = NSE_INDICES.contains(&nse.as_str());
    Route::Nse { symbol: nse, index }
}

/// Parse `2026-10-27`, NSE's `27-Oct-2026` or BSE's `27 Oct 2026`.
pub fn parse_expiry(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(s, "%d-%b-%Y"))
        .or_else(|_| NaiveDate::parse_from_str(s, "%d %b %Y"))
        .ok()
}

/// When an expiry stops trading, in UTC: 15:30 IST on NSE and BSE, 16:00
/// New York (taken as 20:00 UTC) for US listings.
pub fn expiry_close(expiry: NaiveDate, source: ChainSource) -> DateTime<Utc> {
    let hour = match source {
        ChainSource::Nse | ChainSource::Bse => 10,
        ChainSource::Yahoo => 20,
    };
    expiry
        .and_hms_opt(hour, 0, 0)
        .expect("valid time")
        .and_utc()
}

/// Years from `now` to the expiry close (negative once expired).
pub fn years_to_expiry(expiry: NaiveDate, source: ChainSource, now: DateTime<Utc>) -> f64 {
    (expiry_close(expiry, source) - now).num_seconds() as f64 / (365.0 * 86_400.0)
}

/// The requested expiry if listed, else the nearest one still trading.
pub fn pick_expiry(
    expiries: &[NaiveDate],
    requested: Option<NaiveDate>,
    source: ChainSource,
    now: DateTime<Utc>,
) -> Result<NaiveDate> {
    if expiries.is_empty() {
        bail!("no expiries listed");
    }
    if let Some(want) = requested {
        if expiries.contains(&want) {
            return Ok(want);
        }
        let listed: Vec<String> = expiries.iter().take(8).map(|d| d.to_string()).collect();
        bail!(
            "No expiry on {}. Listed expiries: {}{}",
            want,
            listed.join(", "),
            if expiries.len() > 8 { ", ..." } else { "" }
        );
    }
    expiries
        .iter()
        .copied()
        .filter(|d| expiry_close(*d, source) > now)
        .min()
        .context("every listed expiry has passed")
}

// ── Parsers ──

fn f(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|x| x.is_finite())
}

/// Positive number or `None` (NSE writes 0 for "no price").
fn pos(v: &Value, key: &str) -> Option<f64> {
    f(v, key).filter(|x| *x > 0.0)
}

fn count(v: &Value, key: &str) -> Option<u64> {
    v.get(key)
        .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f.max(0.0) as u64)))
}

fn yahoo_contract(c: &Value) -> Contract {
    Contract {
        last: pos(c, "lastPrice"),
        bid: pos(c, "bid"),
        ask: pos(c, "ask"),
        price_change: f(c, "change"),
        volume: count(c, "volume"),
        open_interest: count(c, "openInterest"),
        oi_change: None,
        // Yahoo reports placeholder IVs like 1e-5 for dead strikes.
        source_iv: f(c, "impliedVolatility").filter(|v| *v > 0.001),
    }
}

/// Merge call and put lists into rows sorted by strike.
fn merge_rows(calls: Vec<(f64, Contract)>, puts: Vec<(f64, Contract)>) -> Vec<ChainRow> {
    let mut rows: Vec<ChainRow> = Vec::new();
    let find = |rows: &mut Vec<ChainRow>, strike: f64| -> usize {
        match rows.iter().position(|r| (r.strike - strike).abs() < 1e-9) {
            Some(i) => i,
            None => {
                rows.push(ChainRow {
                    strike,
                    call: None,
                    put: None,
                });
                rows.len() - 1
            }
        }
    };
    for (strike, c) in calls {
        let i = find(&mut rows, strike);
        rows[i].call = Some(c);
    }
    for (strike, p) in puts {
        let i = find(&mut rows, strike);
        rows[i].put = Some(p);
    }
    rows.sort_by(|a, b| a.strike.total_cmp(&b.strike));
    rows
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Lot size from order-book quantities: every bid/offer on NSE and BSE is a
/// whole number of lots, so across a chain their greatest common divisor is
/// the lot. Needs a handful of quotes to be trusted.
pub fn lot_size_from_quantities(quantities: impl IntoIterator<Item = u64>) -> Option<u32> {
    let (mut g, mut n) = (0u64, 0usize);
    for q in quantities.into_iter().filter(|q| *q > 0) {
        g = gcd(g, q);
        n += 1;
    }
    // A GCD of 1 means the quantities are not in units (no Indian contract
    // has a lot of 1), so the lot is unknown rather than 1.
    (n >= 5 && g >= 2)
        .then_some(g)
        .and_then(|g| u32::try_from(g).ok())
}

fn ts_to_date(ts: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp(ts, 0).map(|d| d.date_naive())
}

/// Parse a Yahoo `v7/finance/options` response.
pub fn parse_yahoo_chain(v: &Value) -> Result<OptionChain> {
    let result = v
        .pointer("/optionChain/result/0")
        .context("Yahoo returned no option data")?;
    let symbol = result
        .get("underlyingSymbol")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expiries: Vec<NaiveDate> = result
        .get("expirationDates")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_i64)
                .filter_map(ts_to_date)
                .collect()
        })
        .unwrap_or_default();
    let quote = result.get("quote");
    let underlying = quote.and_then(|q| pos(q, "regularMarketPrice"));
    let currency = quote
        .and_then(|q| q.get("currency"))
        .and_then(Value::as_str)
        .map(String::from);

    let opts = result
        .pointer("/options/0")
        .filter(|_| !expiries.is_empty())
        .with_context(|| format!("Yahoo Finance lists no options for {symbol}"))?;
    let expiry = opts
        .get("expirationDate")
        .and_then(Value::as_i64)
        .and_then(ts_to_date)
        .context("option data has no expiration date")?;
    let side = |key: &str| -> Vec<(f64, Contract)> {
        opts.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| Some((f(c, "strike")?, yahoo_contract(c))))
                    .collect()
            })
            .unwrap_or_default()
    };
    let as_of = quote
        .and_then(|q| q.get("regularMarketTime"))
        .and_then(Value::as_i64)
        .and_then(|t| DateTime::from_timestamp(t, 0));
    // US listed equity options are 100 shares ("REGULAR"); mini or adjusted
    // contracts say otherwise, and then the size is unknown here.
    let regular = ["calls", "puts"].iter().all(|k| {
        opts.get(*k).and_then(Value::as_array).is_none_or(|a| {
            a.iter().all(|c| {
                c.get("contractSize")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s.eq_ignore_ascii_case("REGULAR"))
            })
        })
    });

    Ok(OptionChain {
        symbol,
        source: ChainSource::Yahoo,
        underlying,
        currency,
        expiry,
        expiries,
        as_of,
        rows: merge_rows(side("calls"), side("puts")),
        lot_size: regular.then_some(100),
    })
}

fn nse_contract(c: &Value) -> Contract {
    Contract {
        last: pos(c, "lastPrice"),
        bid: pos(c, "buyPrice1").or_else(|| pos(c, "bidprice")),
        ask: pos(c, "sellPrice1").or_else(|| pos(c, "askPrice")),
        price_change: f(c, "change"),
        volume: count(c, "totalTradedVolume"),
        open_interest: count(c, "openInterest"),
        oi_change: f(c, "changeinOpenInterest").map(|x| x as i64),
        source_iv: pos(c, "impliedVolatility").map(|iv| iv / 100.0),
    }
}

/// Expiry dates from NSE's `option-chain-contract-info` response.
pub fn parse_nse_expiries(v: &Value) -> Vec<NaiveDate> {
    let mut out: Vec<NaiveDate> = v
        .get("expiryDates")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter_map(parse_expiry)
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

/// Parse NSE's `option-chain-v3` response, keeping rows for `expiry`.
pub fn parse_nse_chain(v: &Value, symbol: &str, expiry: NaiveDate) -> Result<OptionChain> {
    let records = v.get("records").context("NSE returned no option records")?;
    let data = records
        .get("data")
        .and_then(Value::as_array)
        .context("NSE returned no option rows")?;
    let mut calls = Vec::new();
    let mut puts = Vec::new();
    let mut quantities = Vec::new();
    for row in data {
        let row_expiry = row
            .get("expiryDates")
            .or_else(|| row.get("expiryDate"))
            .and_then(Value::as_str)
            .and_then(parse_expiry);
        if row_expiry.is_some_and(|d| d != expiry) {
            continue;
        }
        let Some(strike) = f(row, "strikePrice") else {
            continue;
        };
        for (side, out) in [("CE", &mut calls), ("PE", &mut puts)] {
            if let Some(c) = row.get(side).filter(|c| c.is_object()) {
                out.push((strike, nse_contract(c)));
                quantities.extend(
                    [
                        "buyQuantity1",
                        "sellQuantity1",
                        "totalBuyQuantity",
                        "totalSellQuantity",
                    ]
                    .iter()
                    .filter_map(|k| count(c, k)),
                );
            }
        }
    }
    let rows = merge_rows(calls, puts);
    if rows.is_empty() {
        bail!("NSE returned no {symbol} options for {expiry}");
    }
    let underlying = pos(records, "underlyingValue").or_else(|| {
        data.iter().find_map(|r| {
            r.get("CE")
                .or(r.get("PE"))
                .and_then(|c| pos(c, "underlyingValue"))
        })
    });
    let mut expiries = parse_nse_expiries(records);
    if expiries.is_empty() {
        expiries.push(expiry);
    }
    Ok(OptionChain {
        symbol: symbol.to_string(),
        source: ChainSource::Nse,
        underlying,
        currency: Some("INR".into()),
        expiry,
        expiries,
        as_of: records
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_nse_timestamp),
        rows,
        lot_size: lot_size_from_quantities(quantities),
    })
}

/// A BSE number: text with Indian digit grouping (`5,24,022.00`), blank
/// when there is none. Plain JSON numbers are accepted too.
fn bse_num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::String(s) => s.trim().replace(',', "").parse::<f64>().ok(),
        other => other.as_f64(),
    }
    .filter(|x| x.is_finite())
}

/// One side of a BSE chain row: call fields are `C_`-prefixed, put fields
/// are not. Blank prices mean "no trade", like NSE's zeros.
fn bse_contract(row: &Value, prefix: &str) -> Contract {
    let num = |key: &str| bse_num(row, &format!("{prefix}{key}"));
    let positive = |key: &str| num(key).filter(|x| *x > 0.0);
    Contract {
        last: positive("Last_Trd_Price"),
        bid: positive("BidPrice"),
        ask: positive("OfferPrice"),
        price_change: num("Change"),
        volume: num("Vol_Traded").map(|x| x.max(0.0) as u64),
        open_interest: num("Open_Interest").map(|x| x.max(0.0) as u64),
        oi_change: num("Absolute_Change_OI").map(|x| x as i64),
        source_iv: positive("IV").map(|iv| iv / 100.0),
    }
}

/// Expiry dates and the underlying's value from BSE's `ddlExpiry_IV`
/// response (`Table1` lists expiries, `Table2` the underlying).
pub fn parse_bse_expiries(v: &Value) -> (Vec<NaiveDate>, Option<f64>) {
    let mut out: Vec<NaiveDate> = v
        .get("Table1")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| e.get("ExpiryDate")?.as_str())
                .filter_map(parse_expiry)
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    let spot = v
        .pointer("/Table2/0")
        .and_then(|t| bse_num(t, "UlaValue"))
        .filter(|x| *x > 0.0);
    (out, spot)
}

/// BSE scrip code of a stock with options, from `ddlUnderlyingAsset`
/// (`ProductType=SO`).
pub fn parse_bse_underlyings(v: &Value, symbol: &str) -> Option<u32> {
    v.get("Table")?.as_array()?.iter().find_map(|r| {
        let code = r.get("UlaCODE")?.as_str()?.trim();
        if !code.eq_ignore_ascii_case(symbol) {
            return None;
        }
        r.get("scrip_cd")
            .and_then(|c| c.as_u64().or_else(|| c.as_str()?.trim().parse().ok()))
            .and_then(|c| u32::try_from(c).ok())
    })
}

/// Parse BSE's `DerivOptionChain_IV` response, keeping rows for `expiry`.
/// The caller fills in the full expiry list.
pub fn parse_bse_chain(v: &Value, symbol: &str, expiry: NaiveDate) -> Result<OptionChain> {
    let data = v
        .get("Table")
        .and_then(Value::as_array)
        .context("BSE returned no option rows")?;
    let mut calls = Vec::new();
    let mut puts = Vec::new();
    let mut quantities = Vec::new();
    for row in data {
        let row_expiry = row
            .get("End_TimeStamp")
            .and_then(Value::as_str)
            .and_then(parse_expiry);
        if row_expiry.is_some_and(|d| d != expiry) {
            continue;
        }
        let Some(strike) = bse_num(row, "Strike_Price1").or_else(|| bse_num(row, "Strike_Price"))
        else {
            continue;
        };
        calls.push((strike, bse_contract(row, "C_")));
        puts.push((strike, bse_contract(row, "")));
        quantities.extend(
            ["C_BIdQty", "C_OfferQty", "BIdQty", "OfferQty"]
                .iter()
                .filter_map(|k| bse_num(row, k))
                .filter(|q| *q > 0.0)
                .map(|q| q as u64),
        );
    }
    let rows = merge_rows(calls, puts);
    if rows.is_empty() {
        bail!("BSE returned no {symbol} options for {expiry}");
    }
    Ok(OptionChain {
        symbol: symbol.to_string(),
        source: ChainSource::Bse,
        underlying: data
            .iter()
            .find_map(|r| bse_num(r, "UlaValue"))
            .filter(|x| *x > 0.0),
        currency: Some("INR".into()),
        expiry,
        expiries: vec![expiry],
        as_of: v
            .pointer("/ASON/DT_TM")
            .and_then(Value::as_str)
            .and_then(parse_bse_timestamp),
        rows,
        lot_size: lot_size_from_quantities(quantities),
    })
}

// ── Chain analytics ──

/// Rows within `n` strikes of the at-the-money strike (all rows for n = 0).
pub fn near_the_money(rows: &[ChainRow], spot: Option<f64>, n: usize) -> &[ChainRow] {
    let (Some(spot), true) = (spot, n > 0) else {
        return rows;
    };
    let Some(atm) = atm_index(rows, spot) else {
        return rows;
    };
    let lo = atm.saturating_sub(n);
    let hi = (atm + n + 1).min(rows.len());
    &rows[lo..hi]
}

/// Index of the strike closest to `spot`.
pub fn atm_index(rows: &[ChainRow], spot: f64) -> Option<usize> {
    rows.iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1.strike - spot)
                .abs()
                .total_cmp(&(b.1.strike - spot).abs())
        })
        .map(|(i, _)| i)
}

/// Open interest and volume totals, and the put/call ratios.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Pcr {
    pub call_oi: u64,
    pub put_oi: u64,
    pub call_volume: u64,
    pub put_volume: u64,
    /// Put OI / call OI; `None` when there is no call OI.
    pub oi_ratio: Option<f64>,
    /// Put volume / call volume; `None` when no calls traded.
    pub volume_ratio: Option<f64>,
}

pub fn pcr(rows: &[ChainRow]) -> Pcr {
    let sum = |pick: fn(&ChainRow) -> Option<&Contract>, field: fn(&Contract) -> Option<u64>| {
        rows.iter().filter_map(pick).filter_map(field).sum::<u64>()
    };
    fn call(r: &ChainRow) -> Option<&Contract> {
        r.call.as_ref()
    }
    fn put(r: &ChainRow) -> Option<&Contract> {
        r.put.as_ref()
    }
    let (call_oi, put_oi) = (
        sum(call, |c| c.open_interest),
        sum(put, |c| c.open_interest),
    );
    let (call_volume, put_volume) = (sum(call, |c| c.volume), sum(put, |c| c.volume));
    let ratio = |a: u64, b: u64| (b > 0).then(|| a as f64 / b as f64);
    Pcr {
        call_oi,
        put_oi,
        call_volume,
        put_volume,
        oi_ratio: ratio(put_oi, call_oi),
        volume_ratio: ratio(put_volume, call_volume),
    }
}

/// The `n` strikes with the most open interest on one side, largest first.
pub fn top_oi(rows: &[ChainRow], kind: OptionKind, n: usize) -> Vec<(f64, u64)> {
    let mut v: Vec<(f64, u64)> = rows
        .iter()
        .filter_map(|r| {
            let c = match kind {
                OptionKind::Call => r.call.as_ref(),
                OptionKind::Put => r.put.as_ref(),
            }?;
            Some((r.strike, c.open_interest.filter(|oi| *oi > 0)?))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.total_cmp(&b.0)));
    v.truncate(n);
    v
}

/// Detailed information for a top open-interest strike level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopOiStrike {
    pub strike: f64,
    pub open_interest: u64,
    pub share_pct: f64,
    pub oi_change: Option<i64>,
    pub price_change: Option<f64>,
    pub last_price: Option<f64>,
    pub buildup: Option<Buildup>,
}

/// Aggregated build-up statistics across contracts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildupCount {
    pub long_buildup_oi: u64,
    pub short_covering_oi: u64,
    pub short_buildup_oi: u64,
    pub long_unwinding_oi: u64,
    pub neutral_oi: u64,
    pub long_buildup_count: usize,
    pub short_covering_count: usize,
    pub short_buildup_count: usize,
    pub long_unwinding_count: usize,
}

impl BuildupCount {
    pub fn dominant_sentiment(&self) -> &'static str {
        let bullish = self.long_buildup_oi + self.short_covering_oi;
        let bearish = self.short_buildup_oi + self.long_unwinding_oi;
        if bullish == 0 && bearish == 0 {
            "Neutral"
        } else if bullish > (bearish as f64 * 1.3) as u64 {
            "Bullish"
        } else if bearish > (bullish as f64 * 1.3) as u64 {
            "Bearish"
        } else {
            "Neutral / Mixed"
        }
    }
}

/// Breakdown of build-up categories across calls and puts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildupBreakdown {
    pub calls: BuildupCount,
    pub puts: BuildupCount,
}

/// The `n` strikes with the most open interest on one side, with full price/OI details.
pub fn top_oi_details(rows: &[ChainRow], kind: OptionKind, n: usize) -> Vec<TopOiStrike> {
    let total_oi: u64 = rows
        .iter()
        .filter_map(|r| match kind {
            OptionKind::Call => r.call.as_ref(),
            OptionKind::Put => r.put.as_ref(),
        })
        .filter_map(|c| c.open_interest)
        .sum();

    let mut list: Vec<TopOiStrike> = rows
        .iter()
        .filter_map(|r| {
            let c = match kind {
                OptionKind::Call => r.call.as_ref(),
                OptionKind::Put => r.put.as_ref(),
            }?;
            let oi = c.open_interest.filter(|x| *x > 0)?;
            let share_pct = if total_oi > 0 {
                (oi as f64 / total_oi as f64) * 100.0
            } else {
                0.0
            };
            Some(TopOiStrike {
                strike: r.strike,
                open_interest: oi,
                share_pct,
                oi_change: c.oi_change,
                price_change: c.price_change,
                last_price: c.last,
                buildup: c.buildup(),
            })
        })
        .collect();

    list.sort_by(|a, b| {
        b.open_interest
            .cmp(&a.open_interest)
            .then(a.strike.total_cmp(&b.strike))
    });
    list.truncate(n);
    list
}

/// Summarize open-interest build-up categories across calls and puts.
pub fn buildup_breakdown(rows: &[ChainRow]) -> BuildupBreakdown {
    let mut out = BuildupBreakdown::default();
    for r in rows {
        if let Some(ref c) = r.call {
            let oi = c.open_interest.unwrap_or(0);
            match c.buildup() {
                Some(Buildup::LongBuildup) => {
                    out.calls.long_buildup_oi += oi;
                    out.calls.long_buildup_count += 1;
                }
                Some(Buildup::ShortCovering) => {
                    out.calls.short_covering_oi += oi;
                    out.calls.short_covering_count += 1;
                }
                Some(Buildup::ShortBuildup) => {
                    out.calls.short_buildup_oi += oi;
                    out.calls.short_buildup_count += 1;
                }
                Some(Buildup::LongUnwinding) => {
                    out.calls.long_unwinding_oi += oi;
                    out.calls.long_unwinding_count += 1;
                }
                None => {
                    out.calls.neutral_oi += oi;
                }
            }
        }
        if let Some(ref p) = r.put {
            let oi = p.open_interest.unwrap_or(0);
            match p.buildup() {
                Some(Buildup::LongBuildup) => {
                    out.puts.long_buildup_oi += oi;
                    out.puts.long_buildup_count += 1;
                }
                Some(Buildup::ShortCovering) => {
                    out.puts.short_covering_oi += oi;
                    out.puts.short_covering_count += 1;
                }
                Some(Buildup::ShortBuildup) => {
                    out.puts.short_buildup_oi += oi;
                    out.puts.short_buildup_count += 1;
                }
                Some(Buildup::LongUnwinding) => {
                    out.puts.long_unwinding_oi += oi;
                    out.puts.long_unwinding_count += 1;
                }
                None => {
                    out.puts.neutral_oi += oi;
                }
            }
        }
    }
    out
}

/// What option buyers would collect if the underlying settled at `strike`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PainPoint {
    pub strike: f64,
    pub call_pain: f64,
    pub put_pain: f64,
    pub total: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaxPain {
    /// Settlement strike at which option holders collect the least.
    pub strike: f64,
    /// Payout at every listed strike, in underlying points × open interest.
    pub curve: Vec<PainPoint>,
}

/// Max pain: the listed strike minimising the total intrinsic value paid to
/// holders of open calls and puts at expiry. `None` without any open interest.
pub fn max_pain(rows: &[ChainRow]) -> Option<MaxPain> {
    let oi = |c: &Option<Contract>| c.as_ref().and_then(|c| c.open_interest).unwrap_or(0) as f64;
    if rows.iter().all(|r| oi(&r.call) == 0.0 && oi(&r.put) == 0.0) {
        return None;
    }
    let curve: Vec<PainPoint> = rows
        .iter()
        .map(|settle| {
            let s = settle.strike;
            let call_pain: f64 = rows
                .iter()
                .map(|r| oi(&r.call) * (s - r.strike).max(0.0))
                .sum();
            let put_pain: f64 = rows
                .iter()
                .map(|r| oi(&r.put) * (r.strike - s).max(0.0))
                .sum();
            PainPoint {
                strike: s,
                call_pain,
                put_pain,
                total: call_pain + put_pain,
            }
        })
        .collect();
    let strike = curve
        .iter()
        .min_by(|a, b| {
            a.total
                .total_cmp(&b.total)
                .then(a.strike.total_cmp(&b.strike))
        })?
        .strike;
    Some(MaxPain { strike, curve })
}

/// One side of a chain row with the IV and greeks used for display.
#[derive(Clone, Debug, Serialize)]
pub struct ContractView {
    #[serde(flatten)]
    pub contract: Contract,
    /// IV used for the greeks: solved from the market price when possible,
    /// otherwise the source's figure.
    pub iv: Option<f64>,
    pub iv_from: Option<&'static str>,
    pub greeks: Option<Greeks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buildup: Option<Buildup>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RowView {
    pub strike: f64,
    pub call: Option<ContractView>,
    pub put: Option<ContractView>,
}

/// Market inputs shared by every contract in a chain.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Assumptions {
    pub spot: f64,
    pub years: f64,
    pub rate: f64,
    pub div_yield: f64,
}

pub fn contract_view(c: &Contract, kind: OptionKind, strike: f64, a: &Assumptions) -> ContractView {
    let base = BsInput {
        spot: a.spot,
        strike,
        t: a.years,
        rate: a.rate,
        div_yield: a.div_yield,
        vol: 0.0,
    };
    let solved = c
        .market_price()
        .and_then(|p| greeks::implied_vol(kind, p, &base));
    let (iv, iv_from) = match (solved, c.source_iv) {
        (Some(v), _) => (Some(v), Some("solved")),
        (None, Some(v)) => (Some(v), Some("source")),
        (None, None) => (None, None),
    };
    let greeks = iv.and_then(|vol| greeks::greeks(kind, &BsInput { vol, ..base }));
    ContractView {
        contract: c.clone(),
        iv,
        iv_from,
        greeks,
        buildup: c.buildup(),
    }
}

pub fn row_views(rows: &[ChainRow], a: &Assumptions) -> Vec<RowView> {
    rows.iter()
        .map(|r| RowView {
            strike: r.strike,
            call: r
                .call
                .as_ref()
                .map(|c| contract_view(c, OptionKind::Call, r.strike, a)),
            put: r
                .put
                .as_ref()
                .map(|c| contract_view(c, OptionKind::Put, r.strike, a)),
        })
        .collect()
}

// ── Fetching ──

const NSE_BASE: &str = "https://www.nseindia.com";
const NSE_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/129.0 Safari/537.36";

fn nse_refused(status: reqwest::StatusCode) -> anyhow::Error {
    anyhow::anyhow!(
        "NSE refused the option-chain request (HTTP {status}). NSE's API serves browser \
         sessions and sometimes blocks scripts — wait a minute and retry, or open \
         nseindia.com/option-chain. US options (`-m us`) come from Yahoo Finance instead."
    )
}

/// Minimal NSE session: one page load for cookies, then JSON API calls
/// spaced 500 ms apart. No retry loops.
pub struct NseClient {
    client: reqwest::Client,
}

impl NseClient {
    pub async fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(NSE_UA)
            .cookie_store(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .context("Failed to build HTTP client")?;
        let resp = client
            .get(format!("{NSE_BASE}/option-chain"))
            .header(reqwest::header::ACCEPT, "text/html")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await
            .context("Cannot reach nseindia.com. Check your internet connection")?;
        if !resp.status().is_success() {
            return Err(nse_refused(resp.status()));
        }
        Ok(Self { client })
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let resp = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::REFERER, format!("{NSE_BASE}/option-chain"))
            .send()
            .await
            .context("Request to nseindia.com failed")?;
        if !resp.status().is_success() {
            return Err(nse_refused(resp.status()));
        }
        let body = resp.text().await.context("Failed to read NSE response")?;
        serde_json::from_str(&body).map_err(|_| nse_refused(reqwest::StatusCode::OK))
    }

    pub async fn expiries(&self, symbol: &str) -> Result<Vec<NaiveDate>> {
        let url = build_url(
            &format!("{NSE_BASE}/api/option-chain-contract-info"),
            None,
            &[("symbol", symbol)],
        );
        Ok(parse_nse_expiries(&self.get_json(&url).await?))
    }

    pub async fn chain(&self, symbol: &str, index: bool, expiry: NaiveDate) -> Result<Value> {
        let expiry = expiry.format("%d-%b-%Y").to_string();
        let url = build_url(
            &format!("{NSE_BASE}/api/option-chain-v3"),
            None,
            &[
                ("type", if index { "Indices" } else { "Equity" }),
                ("symbol", symbol),
                ("expiry", &expiry),
            ],
        );
        self.get_json(&url).await
    }
}

const BSE_API: &str = "https://api.bseindia.com/BseIndiaAPI/api";
const BSE_SITE: &str = "https://www.bseindia.com";
/// Chains and expiry lists are re-fetched after a minute; the list of
/// stocks with options after a day.
const BSE_CACHE_SHORT: Duration = Duration::from_secs(60);
const BSE_CACHE_LONG: Duration = Duration::from_secs(24 * 3600);

fn bse_refused(status: reqwest::StatusCode) -> anyhow::Error {
    anyhow::anyhow!(
        "BSE refused the option-chain request (HTTP {status}). BSE's API serves its own website \
         and sometimes blocks scripts — wait a minute and retry, or open \
         bseindia.com/markets/Derivatives/DeriReports/DeriOptionchain.html. NSE options \
         (NIFTY, BANKNIFTY, F&O stocks) come from NSE instead."
    )
}

/// Minimal BSE session: JSON API calls spaced 400 ms apart, each answer
/// cached on disk briefly so repeated commands don't hit BSE again. No
/// retry loops.
pub struct BseClient {
    client: reqwest::Client,
    base: String,
    cache: bool,
    calls: std::sync::atomic::AtomicUsize,
}

impl BseClient {
    pub fn new() -> Result<Self> {
        Self::build(BSE_API, true)
    }

    fn build(base: &str, cache: bool) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(NSE_UA)
            .cookie_store(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            cache,
            calls: Default::default(),
        })
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let resp = self
            .client
            .get(format!("{}/{path}", self.base))
            .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .header(reqwest::header::REFERER, format!("{BSE_SITE}/"))
            .header(reqwest::header::ORIGIN, BSE_SITE)
            .send()
            .await
            .context("Cannot reach api.bseindia.com. Check your internet connection")?;
        if !resp.status().is_success() {
            return Err(bse_refused(resp.status()));
        }
        let body = resp.text().await.context("Failed to read BSE response")?;
        serde_json::from_str(&body).map_err(|_| bse_refused(reqwest::StatusCode::OK))
    }

    /// `get_json` through a short-lived file cache in the data directory.
    async fn get_cached(&self, path: &str, max_age: Duration) -> Result<Value> {
        let name: String = path
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let file = self
            .cache
            .then(|| crate::funds::cache_path(&format!("bse_{name}.json")).ok())
            .flatten();
        if let Some(f) = &file
            && crate::funds::is_fresh(f, max_age)
            && let Ok(text) = std::fs::read_to_string(f)
            && let Ok(v) = serde_json::from_str(&text)
        {
            return Ok(v);
        }
        let v = self.get_json(path).await?;
        if let Some(f) = &file {
            let _ = crate::funds::write_atomic(f, &v.to_string());
        }
        Ok(v)
    }

    /// Scrip code of a BSE stock that has options.
    pub async fn stock_scrip(&self, symbol: &str) -> Result<u32> {
        let v = self
            .get_cached(
                "ddlUnderlyingAsset/w?ProductType=SO&scrip_cd=0",
                BSE_CACHE_LONG,
            )
            .await?;
        parse_bse_underlyings(&v, symbol).with_context(|| {
            format!(
                "BSE lists no stock options on {symbol}. BSE index options: {}; NSE stocks \
                 without the .BO suffix use NSE's chain",
                BSE_INDICES
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// Listed expiries and the underlying's value. `index` picks BSE's
    /// index-option (`IO`) or stock-option (`SO`) product.
    pub async fn expiries(
        &self,
        scrip_cd: u32,
        index: bool,
    ) -> Result<(Vec<NaiveDate>, Option<f64>)> {
        let product = if index { "IO" } else { "SO" };
        let v = self
            .get_cached(
                &format!("ddlExpiry_IV/w?ProductType={product}&scrip_cd={scrip_cd}"),
                BSE_CACHE_SHORT,
            )
            .await?;
        Ok(parse_bse_expiries(&v))
    }

    pub async fn chain(&self, scrip_cd: u32, expiry: NaiveDate) -> Result<Value> {
        let expiry = expiry.format("%d+%b+%Y").to_string();
        self.get_cached(
            &format!("DerivOptionChain_IV/w?Expiry={expiry}&scrip_cd={scrip_cd}&strprice=0"),
            BSE_CACHE_SHORT,
        )
        .await
    }
}

/// BSE chain for an index (`scrip_cd` known) or a `.BO` stock.
async fn fetch_bse_chain(
    bse: &BseClient,
    symbol: &str,
    scrip_cd: Option<u32>,
    expiry: Option<NaiveDate>,
    now: DateTime<Utc>,
) -> Result<OptionChain> {
    let index = scrip_cd.is_some_and(|c| BSE_INDICES.iter().any(|(_, code)| *code == c));
    let scrip = match scrip_cd {
        Some(c) => c,
        None => bse.stock_scrip(symbol).await?,
    };
    let (expiries, spot) = bse.expiries(scrip, index).await?;
    if expiries.is_empty() {
        bail!("BSE lists no option expiries for {symbol}");
    }
    let expiry = pick_expiry(&expiries, expiry, ChainSource::Bse, now)?;
    let raw = bse.chain(scrip, expiry).await?;
    let mut chain = parse_bse_chain(&raw, symbol, expiry)?;
    chain.expiries = expiries;
    chain.underlying = chain.underlying.or(spot);
    Ok(chain)
}

/// Option chain for `symbol` at `expiry` (default: nearest live expiry).
pub async fn fetch_chain(
    symbol: &str,
    market: Market,
    expiry: Option<NaiveDate>,
) -> Result<OptionChain> {
    let now = Utc::now();
    match route(symbol, market) {
        Route::Nse { symbol, index } => {
            let nse = NseClient::new().await?;
            let expiries = nse.expiries(&symbol).await?;
            if expiries.is_empty() {
                bail!(
                    "NSE lists no options for {symbol}. Only F&O stocks and the indices {} \
                     have options on NSE; BSE's are SENSEX, BANKEX, SENSEX50 and `SYMBOL.BO` \
                     stocks; for US options add `-m us`",
                    NSE_INDICES.join(", ")
                );
            }
            let expiry = pick_expiry(&expiries, expiry, ChainSource::Nse, now)?;
            let raw = nse.chain(&symbol, index, expiry).await?;
            parse_nse_chain(&raw, &symbol, expiry)
        }
        Route::Bse { symbol, scrip_cd } => {
            fetch_bse_chain(&BseClient::new()?, &symbol, scrip_cd, expiry, now).await
        }
        Route::Yahoo(sym) => {
            let client = YahooClient::new().await?;
            let base = "https://query2.finance.yahoo.com/v7/finance/options";
            let first_url = build_url(base, Some(&sym), &[("crumb", client.crumb())]);
            let first = client.raw_get(&first_url).await?;
            let chain = parse_yahoo_chain(&first).with_context(|| {
                format!(
                    "No option chain for {sym}{}",
                    market::not_found_hint(market)
                )
            })?;
            let want = pick_expiry(&chain.expiries, expiry, ChainSource::Yahoo, now)?;
            if want == chain.expiry {
                return Ok(chain);
            }
            let ts = want
                .and_hms_opt(0, 0, 0)
                .expect("valid time")
                .and_utc()
                .timestamp()
                .to_string();
            let url = build_url(
                base,
                Some(&sym),
                &[("date", &ts), ("crumb", client.crumb())],
            );
            parse_yahoo_chain(&client.raw_get(&url).await?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(strike: f64, call_oi: u64, put_oi: u64) -> ChainRow {
        let c = |oi| Contract {
            open_interest: Some(oi),
            ..Default::default()
        };
        ChainRow {
            strike,
            call: Some(c(call_oi)),
            put: Some(c(put_oi)),
        }
    }

    #[test]
    fn max_pain_on_fixture_chain() {
        // Settle at 100: calls 90 pay 10*100=1000; puts 110 pay 10*300=3000 → 4000
        // Settle at 110: calls 90 → 20*100, 100 → 10*500 = 7000; puts 0 → 7000
        // Settle at  90: calls 0; puts 100 → 10*200, 110 → 20*300 = 8000
        let rows = vec![
            row(90.0, 100, 50),
            row(100.0, 500, 200),
            row(110.0, 80, 300),
        ];
        let mp = max_pain(&rows).unwrap();
        assert_eq!(mp.strike, 100.0);
        let totals: Vec<f64> = mp.curve.iter().map(|p| p.total).collect();
        assert_eq!(totals, vec![8000.0, 4000.0, 7000.0]);
        assert_eq!(mp.curve[1].call_pain, 1000.0);
        assert_eq!(mp.curve[1].put_pain, 3000.0);
    }

    #[test]
    fn max_pain_needs_open_interest() {
        assert!(max_pain(&[row(100.0, 0, 0)]).is_none());
        assert!(max_pain(&[]).is_none());
    }

    #[test]
    fn pcr_and_oi_walls() {
        let mut rows = vec![
            row(90.0, 100, 50),
            row(100.0, 500, 200),
            row(110.0, 80, 300),
        ];
        rows[0].call.as_mut().unwrap().volume = Some(40);
        rows[1].put.as_mut().unwrap().volume = Some(10);
        let p = pcr(&rows);
        assert_eq!((p.call_oi, p.put_oi), (680, 550));
        assert!((p.oi_ratio.unwrap() - 550.0 / 680.0).abs() < 1e-12);
        assert_eq!(p.volume_ratio, Some(0.25));
        assert_eq!(
            top_oi(&rows, OptionKind::Put, 2),
            vec![(110.0, 300), (100.0, 200)]
        );
        assert_eq!(top_oi(&rows, OptionKind::Call, 1), vec![(100.0, 500)]);
        assert_eq!(pcr(&[]).oi_ratio, None);
    }

    #[test]
    fn routes_symbols_to_the_right_exchange() {
        let nse = |s: &str, index| Route::Nse {
            symbol: s.into(),
            index,
        };
        assert_eq!(route("nifty", Market::In), nse("NIFTY", true));
        assert_eq!(route("^NSEI", Market::In), nse("NIFTY", true));
        assert_eq!(route("^NSEBANK", Market::In), nse("BANKNIFTY", true));
        assert_eq!(route("RELIANCE.NS", Market::In), nse("RELIANCE", false));
        assert_eq!(route("M&M", Market::In), nse("M&M", false));
        assert_eq!(route("AAPL", Market::Us), Route::Yahoo("AAPL".into()));
        assert_eq!(route("VOD.L", Market::In), Route::Yahoo("VOD.L".into()));

        let bse = |s: &str, scrip_cd| Route::Bse {
            symbol: s.into(),
            scrip_cd,
        };
        assert_eq!(route("sensex", Market::In), bse("SENSEX", Some(1)));
        assert_eq!(route("^BSESN", Market::In), bse("SENSEX", Some(1)));
        assert_eq!(route("BANKEX", Market::In), bse("BANKEX", Some(12)));
        assert_eq!(route("BSE-BANK.BO", Market::In), bse("BANKEX", Some(12)));
        assert_eq!(route("sx50", Market::In), bse("SENSEX50", Some(47)));
        assert_eq!(route("RELIANCE.BO", Market::In), bse("RELIANCE", None));
        assert_eq!(route("500325.BO", Market::In), bse("500325", Some(500325)));
        // The NSE indices stay on NSE; `-m us` always means Yahoo.
        for idx in ["NIFTY", "BANKNIFTY", "FINNIFTY", "MIDCPNIFTY"] {
            assert_eq!(route(idx, Market::In), nse(idx, true));
        }
        assert_eq!(route("SENSEX", Market::Us), Route::Yahoo("SENSEX".into()));
    }

    fn bse_fixture(name: &str) -> Value {
        let text = match name {
            "chain" => include_str!("fixtures/bse_sensex_chain.json"),
            "expiries" => include_str!("fixtures/bse_sensex_expiries.json"),
            _ => include_str!("fixtures/bse_stock_underlyings.json"),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn parses_bse_fixtures() {
        // Captured from bseindia.com's option-chain page (SENSEX, 23 Oct
        // 2026 after the close): no bid/offer, blank fields for no trade.
        let d = |s| parse_expiry(s).unwrap();
        let (expiries, spot) = parse_bse_expiries(&bse_fixture("expiries"));
        assert_eq!(expiries.len(), 4);
        assert_eq!(expiries[0], d("29 Oct 2026"));
        assert_eq!(spot, Some(73123.83));

        let chain = parse_bse_chain(&bse_fixture("chain"), "SENSEX", d("2026-10-29")).unwrap();
        assert_eq!(chain.source, ChainSource::Bse);
        assert_eq!(chain.underlying, Some(73123.83));
        assert_eq!(chain.rows.len(), 8);
        assert_eq!(chain.rows[0].strike, 64600.0);
        // Deep ITM call never traded: present, but with no prices.
        let c = chain.rows[0].call.as_ref().unwrap();
        assert_eq!((c.last, c.open_interest, c.source_iv), (None, None, None));
        let p = chain.rows[0].put.as_ref().unwrap();
        assert_eq!(p.last, Some(6.0));
        assert_eq!(p.open_interest, Some(9928));
        assert_eq!(p.volume, Some(44551));
        assert!((p.source_iv.unwrap() - 0.387_481_976).abs() < 1e-6);
        let atm = &chain.rows[2];
        assert_eq!(atm.strike, 73000.0);
        let call = atm.call.as_ref().unwrap();
        assert_eq!(
            call.market_price(),
            Some(723.10),
            "no bid/offer: last price"
        );
        assert_eq!(call.oi_change, Some(0));
        // After the close BSE shows no order book, so no lot size.
        assert_eq!(chain.lot_size, None);
        assert_eq!(
            chain.as_of.unwrap().to_rfc3339(),
            "2026-10-23T14:12:00+00:00"
        );
        let err = parse_bse_chain(&bse_fixture("chain"), "SENSEX", d("2026-11-05"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no SENSEX options"), "{err}");

        let list = bse_fixture("underlyings");
        assert_eq!(parse_bse_underlyings(&list, "reliance"), Some(500325));
        assert_eq!(parse_bse_underlyings(&list, "M&M"), Some(500520));
        assert_eq!(parse_bse_underlyings(&list, "TCS"), None);
        assert_eq!(bse_num(&json!({"x": "5,24,022.00"}), "x"), Some(524022.0));
        assert_eq!(bse_num(&json!({"x": ""}), "x"), None);
        assert_eq!(parse_expiry("05 Nov 2026"), Some(d("2026-11-05")));
    }

    #[test]
    fn lot_size_is_the_gcd_of_order_quantities() {
        assert_eq!(
            lot_size_from_quantities([260, 325, 65, 1040, 96460, 0]),
            Some(65)
        );
        assert_eq!(lot_size_from_quantities([20, 40, 60, 100, 2000]), Some(20));
        // Too few quotes to trust.
        assert_eq!(lot_size_from_quantities([130, 260]), None);
        // Quantities counted in lots, not units: no usable lot size.
        assert_eq!(lot_size_from_quantities([1, 3, 7, 2, 5]), None);
        assert_eq!(lot_size_from_quantities([]), None);

        let v = json!({"records": {"underlyingValue": 22600.0, "data": [
            {"strikePrice": 22600, "expiryDates": "27-Oct-2026",
             "CE": {"lastPrice": 100.0, "buyQuantity1": 260, "sellQuantity1": 325,
                    "totalBuyQuantity": 96460, "totalSellQuantity": 81510},
             "PE": {"lastPrice": 90.0, "buyQuantity1": 65, "sellQuantity1": 1040}}
        ]}});
        let chain = parse_nse_chain(&v, "NIFTY", parse_expiry("2026-10-27").unwrap()).unwrap();
        assert_eq!(chain.lot_size, Some(65));
    }

    #[tokio::test]
    async fn bse_client_fetches_expiries_then_the_chain() {
        let server = crate::testutil::MockServer::start(vec![
            (
                200,
                include_str!("fixtures/bse_sensex_expiries.json").to_string(),
            ),
            (
                200,
                include_str!("fixtures/bse_sensex_chain.json").to_string(),
            ),
        ])
        .await;
        let bse = BseClient::build(&server.url, false).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-10-25T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let chain = fetch_bse_chain(&bse, "SENSEX", Some(1), None, now)
            .await
            .unwrap();
        assert_eq!(chain.expiries.len(), 4, "full expiry list from BSE");
        assert_eq!(chain.expiry, parse_expiry("2026-10-29").unwrap());
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].target, "/ddlExpiry_IV/w?ProductType=IO&scrip_cd=1");
        assert_eq!(
            reqs[1].target,
            "/DerivOptionChain_IV/w?Expiry=29+Oct+2026&scrip_cd=1&strprice=0"
        );
        assert_eq!(reqs[1].header("referer"), Some("https://www.bseindia.com/"));

        // A stock looks its scrip code up first, with the stock-option product.
        let server = crate::testutil::MockServer::start(vec![
            (
                200,
                include_str!("fixtures/bse_stock_underlyings.json").to_string(),
            ),
            (200, r#"{"Table":[],"Table1":[],"Table2":[]}"#.to_string()),
        ])
        .await;
        let bse = BseClient::build(&server.url, false).unwrap();
        let err = fetch_bse_chain(&bse, "RELIANCE", None, None, now)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no option expiries"), "{err}");
        assert_eq!(
            server.requests()[1].target,
            "/ddlExpiry_IV/w?ProductType=SO&scrip_cd=500325"
        );
    }

    #[tokio::test]
    async fn bse_refusals_are_clear_errors() {
        let server = crate::testutil::MockServer::start(vec![(403, "Access Denied".into())]).await;
        let bse = BseClient::build(&server.url, false).unwrap();
        let err = bse.expiries(1, true).await.unwrap_err().to_string();
        assert!(err.contains("BSE refused") && err.contains("403"), "{err}");
        assert_eq!(server.requests().len(), 1, "no retries");

        let server = crate::testutil::MockServer::start(vec![(200, "<html>".into())]).await;
        let bse = BseClient::build(&server.url, false).unwrap();
        let err = bse
            .chain(1, NaiveDate::from_ymd_opt(2026, 10, 29).unwrap())
            .await;
        assert!(err.unwrap_err().to_string().contains("BSE refused"));
    }

    #[test]
    fn expiry_parsing_and_picking() {
        let d = |s| parse_expiry(s).unwrap();
        assert_eq!(d("27-Oct-2026"), d("2026-10-27"));
        assert!(parse_expiry("27/10/2026").is_none());

        let list = vec![d("2026-10-20"), d("2026-10-27"), d("2026-11-03")];
        // 2026-10-20 11:00 UTC is after the 15:30 IST close that day.
        let now = DateTime::parse_from_rfc3339("2026-10-20T11:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            pick_expiry(&list, None, ChainSource::Nse, now).unwrap(),
            d("2026-10-27")
        );
        // ...but before the 16:00 New York close.
        assert_eq!(
            pick_expiry(&list, None, ChainSource::Yahoo, now).unwrap(),
            d("2026-10-20")
        );
        assert_eq!(
            pick_expiry(&list, Some(d("2026-11-03")), ChainSource::Nse, now).unwrap(),
            d("2026-11-03")
        );
        let err = pick_expiry(&list, Some(d("2026-12-31")), ChainSource::Nse, now)
            .unwrap_err()
            .to_string();
        assert!(err.contains("2026-10-27"), "{err}");
        let t = years_to_expiry(d("2026-10-27"), ChainSource::Nse, now);
        assert!((t * 365.0 - 6.958_333).abs() < 1e-3, "{t}");
    }

    #[test]
    fn parses_nse_v3_fixture() {
        let v = json!({
            "records": {
                "timestamp": "19-Oct-2026 15:40:00",
                "underlyingValue": 22888.95,
                "expiryDates": ["19-Oct-2026", "27-Oct-2026"],
                "data": [
                    {"strikePrice": 22900, "expiryDates": "27-Oct-2026",
                     "CE": {"lastPrice": 210.5, "buyPrice1": 209.0, "sellPrice1": 211.0,
                            "totalTradedVolume": 1500, "openInterest": 12000,
                            "changeinOpenInterest": -300, "impliedVolatility": 14.2},
                     "PE": {"lastPrice": 0, "buyPrice1": 0, "sellPrice1": 0,
                            "totalTradedVolume": 0, "openInterest": 0,
                            "changeinOpenInterest": 0, "impliedVolatility": 0}},
                    {"strikePrice": 22800, "expiryDates": "27-Oct-2026",
                     "PE": {"lastPrice": 150.0, "totalTradedVolume": 900,
                            "openInterest": 8000, "impliedVolatility": 15.0}},
                    {"strikePrice": 22800, "expiryDates": "03-Nov-2026",
                     "CE": {"lastPrice": 999.0, "openInterest": 1}}
                ]
            }
        });
        let chain = parse_nse_chain(&v, "NIFTY", parse_expiry("2026-10-27").unwrap()).unwrap();
        assert_eq!(chain.underlying, Some(22888.95));
        assert_eq!(chain.rows.len(), 2);
        assert_eq!(chain.rows[0].strike, 22800.0);
        assert!(chain.rows[0].call.is_none(), "other expiry must be dropped");
        let ce = chain.rows[1].call.as_ref().unwrap();
        assert_eq!(ce.market_price(), Some(210.0));
        assert_eq!(ce.oi_change, Some(-300));
        assert!((ce.source_iv.unwrap() - 0.142).abs() < 1e-12);
        let pe = chain.rows[1].put.as_ref().unwrap();
        assert_eq!(
            (pe.last, pe.source_iv, pe.open_interest),
            (None, None, Some(0))
        );
        assert_eq!(chain.expiries.len(), 2);
        let as_of = chain.as_of.unwrap();
        assert_eq!(as_of.to_rfc3339(), "2026-10-19T10:10:00+00:00");
        // Valued at the data time, not at a later "now".
        let later = DateTime::parse_from_rfc3339("2026-10-21T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let days = chain.years_to_expiry(later) * 365.0;
        assert!((days - (8.0 - 10.0 / 1440.0)).abs() < 1e-6, "{days}");
    }

    #[test]
    fn parses_yahoo_fixture() {
        let v = json!({"optionChain": {"result": [{
            "underlyingSymbol": "AAPL",
            "expirationDates": [1792540800, 1793145600],
            "quote": {"regularMarketPrice": 340.0, "currency": "USD"},
            "options": [{
                "expirationDate": 1792540800,
                "calls": [{"strike": 340.0, "lastPrice": 4.1, "bid": 4.0, "ask": 4.2,
                           "volume": 1200, "openInterest": 5000, "impliedVolatility": 0.21}],
                "puts": [{"strike": 335.0, "lastPrice": 2.0, "impliedVolatility": 0.00001},
                         {"strike": 340.0, "lastPrice": 3.9, "openInterest": 4000}]
            }]
        }]}});
        let chain = parse_yahoo_chain(&v).unwrap();
        assert_eq!(chain.symbol, "AAPL");
        assert_eq!(chain.lot_size, Some(100));
        assert_eq!(chain.expiry, parse_expiry("2026-10-21").unwrap());
        assert_eq!(chain.expiries.len(), 2);
        assert_eq!(chain.rows.len(), 2);
        assert!(chain.rows[0].call.is_none());
        assert_eq!(chain.rows[0].put.as_ref().unwrap().source_iv, None);
        assert_eq!(chain.rows[1].call.as_ref().unwrap().volume, Some(1200));

        let empty = json!({"optionChain": {"result": [{
            "underlyingSymbol": "RELIANCE.NS", "expirationDates": [], "options": []
        }]}});
        let err = parse_yahoo_chain(&empty).unwrap_err().to_string();
        assert!(err.contains("no options"), "{err}");
    }

    #[test]
    fn views_solve_iv_and_greeks() {
        let a = Assumptions {
            spot: 100.0,
            years: 30.0 / 365.0,
            rate: 0.05,
            div_yield: 0.0,
        };
        let inp = BsInput {
            spot: 100.0,
            strike: 105.0,
            t: a.years,
            rate: a.rate,
            div_yield: 0.0,
            vol: 0.25,
        };
        let fair = greeks::price(OptionKind::Call, &inp).unwrap();
        let c = Contract {
            bid: Some(fair - 0.01),
            ask: Some(fair + 0.01),
            source_iv: Some(0.9),
            ..Default::default()
        };
        let v = contract_view(&c, OptionKind::Call, 105.0, &a);
        assert!((v.iv.unwrap() - 0.25).abs() < 1e-6);
        assert_eq!(v.iv_from, Some("solved"));
        assert!(v.greeks.unwrap().delta > 0.0);

        // No price → falls back to the source IV; nothing → no greeks.
        let src = Contract {
            source_iv: Some(0.3),
            ..Default::default()
        };
        assert_eq!(
            contract_view(&src, OptionKind::Put, 95.0, &a).iv_from,
            Some("source")
        );
        assert!(
            contract_view(&Contract::default(), OptionKind::Put, 95.0, &a)
                .greeks
                .is_none()
        );
    }

    #[test]
    fn near_the_money_window() {
        let rows: Vec<ChainRow> = (0..10)
            .map(|i| row(100.0 + 10.0 * i as f64, 1, 1))
            .collect();
        let w = near_the_money(&rows, Some(141.0), 2);
        let strikes: Vec<f64> = w.iter().map(|r| r.strike).collect();
        assert_eq!(strikes, vec![120.0, 130.0, 140.0, 150.0, 160.0]);
        assert_eq!(near_the_money(&rows, Some(141.0), 0).len(), 10);
        assert_eq!(near_the_money(&rows, None, 2).len(), 10);
        assert_eq!(near_the_money(&rows, Some(50.0), 1).len(), 2);
    }

    #[test]
    fn buildup_classification_all_four_quadrants() {
        // Price up, OI up -> Long Build-up (LB)
        let lb = Contract {
            price_change: Some(15.5),
            oi_change: Some(1200),
            ..Default::default()
        };
        assert_eq!(lb.buildup(), Some(Buildup::LongBuildup));
        assert_eq!(lb.buildup().unwrap().code(), "LB");
        assert_eq!(lb.buildup().unwrap().sentiment(), "Bullish");

        // Price up, OI down -> Short Covering (SC)
        let sc = Contract {
            price_change: Some(8.0),
            oi_change: Some(-500),
            ..Default::default()
        };
        assert_eq!(sc.buildup(), Some(Buildup::ShortCovering));
        assert_eq!(sc.buildup().unwrap().code(), "SC");
        assert_eq!(sc.buildup().unwrap().sentiment(), "Mildly Bullish");

        // Price down, OI up -> Short Build-up (SB)
        let sb = Contract {
            price_change: Some(-12.0),
            oi_change: Some(2500),
            ..Default::default()
        };
        assert_eq!(sb.buildup(), Some(Buildup::ShortBuildup));
        assert_eq!(sb.buildup().unwrap().code(), "SB");
        assert_eq!(sb.buildup().unwrap().sentiment(), "Bearish");

        // Price down, OI down -> Long Unwinding (LU)
        let lu = Contract {
            price_change: Some(-6.5),
            oi_change: Some(-800),
            ..Default::default()
        };
        assert_eq!(lu.buildup(), Some(Buildup::LongUnwinding));
        assert_eq!(lu.buildup().unwrap().code(), "LU");
        assert_eq!(lu.buildup().unwrap().sentiment(), "Mildly Bearish");

        // Neutral / unchanged / missing fields -> None
        let flat_price = Contract {
            price_change: Some(0.0),
            oi_change: Some(500),
            ..Default::default()
        };
        assert_eq!(flat_price.buildup(), None);

        let flat_oi = Contract {
            price_change: Some(5.0),
            oi_change: Some(0),
            ..Default::default()
        };
        assert_eq!(flat_oi.buildup(), None);

        let missing = Contract {
            price_change: None,
            oi_change: Some(100),
            ..Default::default()
        };
        assert_eq!(missing.buildup(), None);
    }

    #[test]
    fn top_oi_details_and_buildup_breakdown() {
        let rows = vec![
            ChainRow {
                strike: 24000.0,
                call: Some(Contract {
                    open_interest: Some(50000),
                    oi_change: Some(5000),
                    price_change: Some(25.0),
                    last: Some(250.0),
                    ..Default::default()
                }),
                put: Some(Contract {
                    open_interest: Some(10000),
                    oi_change: Some(-1000),
                    price_change: Some(15.0),
                    last: Some(80.0),
                    ..Default::default()
                }),
            },
            ChainRow {
                strike: 24500.0,
                call: Some(Contract {
                    open_interest: Some(80000),
                    oi_change: Some(10000),
                    price_change: Some(-30.0),
                    last: Some(120.0),
                    ..Default::default()
                }),
                put: Some(Contract {
                    open_interest: Some(70000),
                    oi_change: Some(8000),
                    price_change: Some(-20.0),
                    last: Some(150.0),
                    ..Default::default()
                }),
            },
        ];

        let top_calls = top_oi_details(&rows, OptionKind::Call, 2);
        assert_eq!(top_calls.len(), 2);
        assert_eq!(top_calls[0].strike, 24500.0);
        assert_eq!(top_calls[0].open_interest, 80000);
        assert_eq!(top_calls[0].buildup, Some(Buildup::ShortBuildup));
        assert!((top_calls[0].share_pct - 61.538).abs() < 0.01);

        assert_eq!(top_calls[1].strike, 24000.0);
        assert_eq!(top_calls[1].open_interest, 50000);
        assert_eq!(top_calls[1].buildup, Some(Buildup::LongBuildup));

        let top_puts = top_oi_details(&rows, OptionKind::Put, 2);
        assert_eq!(top_puts.len(), 2);
        assert_eq!(top_puts[0].strike, 24500.0);
        assert_eq!(top_puts[0].open_interest, 70000);
        assert_eq!(top_puts[0].buildup, Some(Buildup::ShortBuildup));

        let breakdown = buildup_breakdown(&rows);
        assert_eq!(breakdown.calls.long_buildup_oi, 50000);
        assert_eq!(breakdown.calls.short_buildup_oi, 80000);
        assert_eq!(breakdown.puts.short_covering_oi, 10000);
        assert_eq!(breakdown.puts.short_buildup_oi, 70000);
    }
}
