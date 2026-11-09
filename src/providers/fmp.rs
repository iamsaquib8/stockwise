//! Financial Modeling Prep (<https://site.financialmodelingprep.com>). Keyed
//! (`FMP_API_KEY`); the free plan allows 250 calls a day and mostly US
//! listings, paid plans reach other exchanges. Capabilities here: batch
//! quotes in one request, company profile, quarterly and annual income /
//! balance-sheet / cash-flow statements, and the company screener.
//!
//! Built from FMP's documented "stable" API (`/stable/batch-quote`,
//! `/stable/profile`, `/stable/income-statement`, `/stable/balance-sheet-statement`,
//! `/stable/cash-flow-statement`, `/stable/company-screener`); not checked
//! against the live API. FMP takes the key as the `apikey` query parameter:
//! StockWise adds it only when sending, so it never reaches the cache key,
//! an error message or any output. Plan restrictions come back as HTTP 402
//! or 403 or an `{"Error Message": ...}` body.

use super::router::{Profile, ScreenQuery};
use super::{Auth, Http, ProviderError, int, num};
use crate::api::Quote;
use crate::results::{ResultPeriod, Statements};
use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

pub const BASE_URL: &str = "https://financialmodelingprep.com/stable";
const AUTH: Auth = Auth::Query("apikey");
/// Quarters and years of statements asked for.
const QUARTERS: &str = "20";
const YEARS: &str = "10";

/// FMP names stocks as Yahoo does (`AAPL`, `RELIANCE.NS`, `HSBA.L`); indices,
/// FX and crypto are left to other sources.
pub fn ticker(symbol: &str) -> Option<String> {
    use crate::market::AssetClass;
    let upper = symbol.trim().to_ascii_uppercase();
    match AssetClass::of(&upper) {
        AssetClass::Stock if !upper.is_empty() => Some(upper),
        _ => None,
    }
}

fn check(json: &Value) -> Result<(), ProviderError> {
    let Some(msg) = json["Error Message"].as_str() else {
        return Ok(());
    };
    let lower = msg.to_ascii_lowercase();
    Err(if lower.contains("limit") {
        ProviderError::RateLimited(msg.to_string())
    } else if lower.contains("api key") || lower.contains("subscription") {
        ProviderError::Denied(msg.to_string())
    } else {
        ProviderError::Other(msg.to_string())
    })
}

pub struct Fmp {
    pub(super) http: Http,
}

impl Fmp {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    async fn get(&self, path: &str, query: &[(&str, &str)], ttl: Duration) -> Result<Value> {
        self.http.get_json(path, query, AUTH, ttl, check).await
    }

    /// Quotes for every supported symbol in one request.
    pub async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let tickers: Vec<String> = symbols.iter().filter_map(|s| ticker(s)).collect();
        if tickers.is_empty() {
            return Ok(Vec::new());
        }
        let joined = tickers.join(",");
        let json = self
            .get(
                "/batch-quote",
                &[("symbols", &joined)],
                Duration::from_secs(30),
            )
            .await?;
        let mut out = parse_quotes(&json);
        // Keep the caller's spelling of each symbol.
        for q in &mut out {
            if let Some(s) = symbols.iter().find(|s| {
                q.symbol
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(s.trim()))
            }) {
                q.symbol = Some(s.to_string());
            }
        }
        Ok(out)
    }

    pub async fn profile(&self, symbol: &str) -> Result<Option<Profile>> {
        let Some(t) = ticker(symbol) else {
            return Ok(None);
        };
        let json = self
            .get(
                "/profile",
                &[("symbol", &t)],
                Duration::from_secs(24 * 3600),
            )
            .await?;
        Ok(parse_profile(&json))
    }

    /// Quarterly and annual statements: income, balance sheet and cash flow
    /// for each, six requests.
    pub async fn statements(&self, symbol: &str) -> Result<Statements> {
        let t = ticker(symbol)
            .ok_or_else(|| anyhow::anyhow!("Financial Modeling Prep does not cover {symbol}"))?;
        let ttl = Duration::from_secs(6 * 3600);
        let mut fetched = Vec::new();
        for (period, limit) in [("quarter", QUARTERS), ("annual", YEARS)] {
            let query = [("symbol", t.as_str()), ("period", period), ("limit", limit)];
            let income = self.get("/income-statement", &query, ttl).await?;
            let balance = self.get("/balance-sheet-statement", &query, ttl).await?;
            let cash = self.get("/cash-flow-statement", &query, ttl).await?;
            fetched.push(parse_periods(&income, &balance, &cash, period == "annual"));
        }
        let annual = fetched.pop().unwrap_or_default();
        let quarterly = fetched.pop().unwrap_or_default();
        let currency = quarterly
            .iter()
            .rev()
            .chain(annual.iter().rev())
            .find_map(|p| p.currency.clone());
        Ok(Statements {
            quarterly,
            annual,
            currency,
        })
    }

    /// Companies matching `query`, as quotes (symbol, name, market cap,
    /// price, volume, sector, industry, exchange).
    pub async fn screener(&self, query: &ScreenQuery) -> Result<Vec<Quote>> {
        let limit = query.limit.clamp(1, 1000).to_string();
        let min = query.market_cap_min.map(|v| format!("{v:.0}"));
        let max = query.market_cap_max.map(|v| format!("{v:.0}"));
        let mut params: Vec<(&str, &str)> = vec![("isActivelyTrading", "true"), ("limit", &limit)];
        if let Some(v) = &min {
            params.push(("marketCapMoreThan", v));
        }
        if let Some(v) = &max {
            params.push(("marketCapLowerThan", v));
        }
        for (key, value) in [
            ("exchange", &query.exchange),
            ("sector", &query.sector),
            ("country", &query.country),
        ] {
            if let Some(v) = value {
                params.push((key, v));
            }
        }
        let json = self
            .get("/company-screener", &params, Duration::from_secs(600))
            .await?;
        Ok(parse_screener(&json))
    }
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn date(v: &Value) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.as_str()?.get(..10)?, "%Y-%m-%d").ok()
}

/// `/batch-quote` (and `/quote`): a list of rows with `symbol`, `name`,
/// `price`, `change`, `changePercentage` (older plans: `changesPercentage`),
/// `volume`, `dayLow`, `dayHigh`, `yearHigh`, `yearLow`, `marketCap`, `open`,
/// `previousClose`, `exchange`, `timestamp`. Rows without a price are
/// skipped.
pub fn parse_quotes(json: &Value) -> Vec<Quote> {
    json.as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let price = num(&r["price"]).filter(|p| *p > 0.0)?;
            Some(Quote {
                symbol: text(&r["symbol"]),
                short_name: text(&r["name"]),
                regular_market_price: Some(price),
                regular_market_time: r["timestamp"].as_i64().filter(|t| *t > 0),
                regular_market_open: num(&r["open"]).filter(|v| *v > 0.0),
                regular_market_day_high: num(&r["dayHigh"]).filter(|v| *v > 0.0),
                regular_market_day_low: num(&r["dayLow"]).filter(|v| *v > 0.0),
                regular_market_previous_close: num(&r["previousClose"]).filter(|v| *v > 0.0),
                regular_market_change: num(&r["change"]),
                regular_market_change_percent: num(&r["changePercentage"])
                    .or_else(|| num(&r["changesPercentage"])),
                regular_market_volume: int(&r["volume"]),
                fifty_two_week_high: num(&r["yearHigh"]).filter(|v| *v > 0.0),
                fifty_two_week_low: num(&r["yearLow"]).filter(|v| *v > 0.0),
                market_cap: num(&r["marketCap"]).filter(|v| *v > 0.0),
                exchange: text(&r["exchange"]),
                ..Default::default()
            })
        })
        .collect()
}

/// `/profile`: a one-row list with `companyName`, `marketCap`, `currency`,
/// `exchange`, `sector`, `industry`, `country`.
pub fn parse_profile(json: &Value) -> Option<Profile> {
    let r = json.as_array()?.first()?;
    let profile = Profile {
        name: text(&r["companyName"]),
        market_cap: num(&r["marketCap"]).filter(|v| *v > 0.0),
        currency: text(&r["currency"]),
        exchange: text(&r["exchange"]),
        sector: text(&r["sector"]),
        industry: text(&r["industry"]),
        country: text(&r["country"]),
    };
    (profile.name.is_some() || profile.market_cap.is_some()).then_some(profile)
}

/// `/company-screener`: rows with `symbol`, `companyName`, `marketCap`,
/// `sector`, `industry`, `price`, `volume`, `exchange`, `isActivelyTrading`.
pub fn parse_screener(json: &Value) -> Vec<Quote> {
    json.as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Quote {
                symbol: Some(text(&r["symbol"])?),
                short_name: text(&r["companyName"]),
                regular_market_price: num(&r["price"]).filter(|p| *p > 0.0),
                regular_market_volume: int(&r["volume"]),
                market_cap: num(&r["marketCap"]).filter(|v| *v > 0.0),
                sector: text(&r["sector"]),
                industry: text(&r["industry"]),
                exchange: text(&r["exchangeShortName"]).or_else(|| text(&r["exchange"])),
                ..Default::default()
            })
        })
        .collect()
}

/// Join the three statements by period-end date, oldest first. Amounts stay
/// in `reportedCurrency`. Rows with no revenue and no net income are
/// dropped.
pub fn parse_periods(
    income: &Value,
    balance: &Value,
    cash: &Value,
    annual: bool,
) -> Vec<ResultPeriod> {
    let mut by_date: BTreeMap<NaiveDate, ResultPeriod> = BTreeMap::new();
    let rows = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let keep = |r: &Value| {
        let fy = r["period"].as_str() == Some("FY");
        fy == annual
    };
    for r in rows(income).iter().filter(|r| keep(r)) {
        let Some(end) = date(&r["date"]) else {
            continue;
        };
        let p = by_date.entry(end).or_insert_with(|| ResultPeriod::new(end));
        let da = num(&r["depreciationAndAmortization"]);
        p.revenue = num(&r["revenue"]);
        p.ebit = num(&r["operatingIncome"]);
        p.operating_profit = match (p.ebit, da) {
            (Some(o), Some(d)) => Some(o + d),
            (o, _) => o,
        };
        p.depreciation = da;
        p.interest = num(&r["interestExpense"]);
        p.pbt = num(&r["incomeBeforeTax"]);
        p.tax = num(&r["incomeTaxExpense"]);
        p.pat = num(&r["netIncome"]);
        p.eps = num(&r["epsDiluted"]).or_else(|| num(&r["eps"]));
        p.currency = text(&r["reportedCurrency"]);
    }
    for r in rows(balance).iter().filter(|r| keep(r)) {
        if let Some(p) = date(&r["date"]).and_then(|d| by_date.get_mut(&d)) {
            p.equity = num(&r["totalStockholdersEquity"]);
            p.total_assets = num(&r["totalAssets"]);
            p.current_liabilities = num(&r["totalCurrentLiabilities"]);
        }
    }
    for r in rows(cash).iter().filter(|r| keep(r)) {
        if let Some(p) = date(&r["date"]).and_then(|d| by_date.get_mut(&d)) {
            p.ocf = num(&r["operatingCashFlow"]);
            p.capex = num(&r["capitalExpenditure"]);
            p.fcf = num(&r["freeCashFlow"]).or_else(|| match (p.ocf, p.capex) {
                (Some(o), Some(c)) => Some(o - c.abs()),
                _ => None,
            });
        }
    }
    by_date
        .into_values()
        .filter(|p| p.revenue.is_some() || p.pat.is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{MarketData, Provider, ProviderKind};
    use crate::testutil::MockServer;

    const BATCH: &str = r#"[{"symbol":"AAPL","name":"Apple Inc.","price":261.74,"changePercentage":-0.479,"change":-1.26,"volume":48000000,"dayLow":260.68,"dayHigh":263.31,"yearHigh":288.62,"yearLow":169.21,"marketCap":3900000000000,"priceAvg50":250.1,"exchange":"NASDAQ","open":261.07,"previousClose":263,"timestamp":1700000000},{"symbol":"NOPE","name":"No price","price":0}]"#;
    const INCOME: &str = r#"[{"date":"2025-09-27","symbol":"AAPL","reportedCurrency":"USD","period":"Q4","revenue":102466000000,"operatingIncome":31030000000,"depreciationAndAmortization":2800000000,"interestExpense":0,"incomeBeforeTax":32000000000,"incomeTaxExpense":4500000000,"netIncome":27466000000,"epsDiluted":1.85,"eps":1.86},{"date":"2025-06-28","period":"Q3","reportedCurrency":"USD","revenue":94036000000,"operatingIncome":28202000000,"netIncome":23434000000,"eps":1.58},{"date":"2025-09-27","period":"FY","reportedCurrency":"USD","revenue":416161000000,"netIncome":112010000000}]"#;
    const BALANCE: &str = r#"[{"date":"2025-09-27","period":"Q4","totalStockholdersEquity":73733000000,"totalAssets":359241000000,"totalCurrentLiabilities":165631000000}]"#;
    const CASH: &str = r#"[{"date":"2025-09-27","period":"Q4","operatingCashFlow":29728000000,"capitalExpenditure":-3242000000,"freeCashFlow":26486000000}]"#;
    const PROFILE: &str = r#"[{"symbol":"AAPL","companyName":"Apple Inc.","marketCap":3900000000000,"currency":"USD","exchange":"NASDAQ","sector":"Technology","industry":"Consumer Electronics","country":"US"}]"#;
    const SCREEN: &str = r#"[{"symbol":"MSFT","companyName":"Microsoft Corporation","marketCap":3500000000000,"sector":"Technology","industry":"Software - Infrastructure","price":470.1,"volume":20000000,"exchange":"NASDAQ Global Select","exchangeShortName":"NASDAQ","isActivelyTrading":true},{"companyName":"No symbol"}]"#;

    #[test]
    fn maps_stock_symbols_only() {
        assert_eq!(ticker("aapl"), Some("AAPL".into()));
        assert_eq!(ticker("RELIANCE.NS"), Some("RELIANCE.NS".into()));
        assert_eq!(ticker("^GSPC"), None);
        assert_eq!(ticker("ETH-USD"), None);
        assert_eq!(ticker("EURUSD=X"), None);
    }

    #[test]
    fn parses_batch_quote_and_skips_rows_without_a_price() {
        let q = parse_quotes(&serde_json::from_str(BATCH).unwrap());
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].symbol.as_deref(), Some("AAPL"));
        assert_eq!(q[0].regular_market_price, Some(261.74));
        assert_eq!(q[0].market_cap, Some(3.9e12));
        assert_eq!(q[0].fifty_two_week_high, Some(288.62));
        assert_eq!(q[0].regular_market_volume, Some(48_000_000));
        assert_eq!(q[0].regular_market_change_percent, Some(-0.479));
    }

    #[test]
    fn joins_statements_by_date_and_splits_quarters_from_years() {
        let (i, b, c) = (
            serde_json::from_str(INCOME).unwrap(),
            serde_json::from_str(BALANCE).unwrap(),
            serde_json::from_str(CASH).unwrap(),
        );
        let q = parse_periods(&i, &b, &c, false);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].end.to_string(), "2025-06-28", "oldest first");
        let last = &q[1];
        assert_eq!(last.revenue, Some(102_466_000_000.0));
        assert_eq!(
            last.operating_profit,
            Some(31_030_000_000.0 + 2_800_000_000.0)
        );
        assert_eq!(last.eps, Some(1.85), "diluted first");
        assert_eq!(last.equity, Some(73_733_000_000.0));
        assert_eq!(last.fcf, Some(26_486_000_000.0));
        assert_eq!(last.currency.as_deref(), Some("USD"));
        let a = parse_periods(&i, &b, &c, true);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].pat, Some(112_010_000_000.0));
    }

    #[test]
    fn parses_profile_and_screener_rows() {
        let p = parse_profile(&serde_json::from_str(PROFILE).unwrap()).unwrap();
        assert_eq!(p.sector.as_deref(), Some("Technology"));
        assert_eq!(p.market_cap, Some(3.9e12));
        let s = parse_screener(&serde_json::from_str(SCREEN).unwrap());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].exchange.as_deref(), Some("NASDAQ"));
        assert_eq!(s[0].market_cap, Some(3.5e12));
    }

    #[test]
    fn errors_are_classified() {
        assert!(matches!(
            check(&serde_json::json!({"Error Message": "Limit Reach . Please upgrade your plan"})),
            Err(ProviderError::RateLimited(_))
        ));
        assert!(matches!(
            check(&serde_json::json!({"Error Message": "Invalid API KEY."})),
            Err(ProviderError::Denied(_))
        ));
        assert!(check(&serde_json::from_str(BATCH).unwrap()).is_ok());
    }

    #[tokio::test]
    async fn one_batch_request_prices_every_symbol_and_the_key_stays_out_of_output() {
        let server = MockServer::start(vec![(200, BATCH.into())]).await;
        let p = Provider::new(
            ProviderKind::Fmp,
            "fmp-secret".into(),
            Some(server.url.clone()),
        );
        let q = p.quotes(&["aapl", "NOPE", "^GSPC"]).await.unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].symbol.as_deref(), Some("aapl"), "caller's spelling");
        assert_eq!(q[0].data_source.as_deref(), Some("Financial Modeling Prep"));
        assert_eq!(server.requests().len(), 1);
        assert!(
            server.requests()[0]
                .target
                .starts_with("/batch-quote?symbols=AAPL%2CNOPE")
        );
    }

    #[tokio::test]
    async fn a_refused_key_is_reported_without_the_key() {
        let server = MockServer::start(vec![(
            200,
            r#"{"Error Message":"Invalid API KEY. fmp-secret"}"#.into(),
        )])
        .await;
        let p = Provider::new(
            ProviderKind::Fmp,
            "fmp-secret".into(),
            Some(server.url.clone()),
        );
        let err = p.quotes(&["AAPL"]).await.unwrap_err();
        assert!(!format!("{err:#}").contains("fmp-secret"));
        assert_eq!(p.remaining(), 0, "refused key pauses the provider");
    }
}
