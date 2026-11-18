//! Frankfurter (<https://frankfurter.dev>): the European Central Bank's daily
//! reference rates, with no API key and no quota. A keyless backup for FX
//! pairs when Yahoo fails: `-m fx` quotes and history, and the rates that
//! convert portfolio and statement amounts.
//!
//! These are reference rates, set once a working day around 16:00 CET. They
//! are not live quotes, so the source is named "ECB reference rate, daily".
//! There are no bars on weekends or TARGET holidays, and no open, high, low
//! or volume: each bar is the day's single rate.
//!
//! Endpoints (v1): `/v1/latest?base=USD&symbols=INR` and the time series
//! `/v1/2026-01-01..?base=USD&symbols=INR` (open-ended: up to the latest).

use super::{Bar, Http, ProviderError, Series, num};
use crate::api::Quote;
use anyhow::{Result, bail};
use chrono::NaiveDate;
use std::time::Duration;

pub const BASE_URL: &str = "https://api.frankfurter.dev";

/// Currencies the ECB publishes reference rates for (the euro included).
const CURRENCIES: &[&str] = &[
    "AUD", "BRL", "CAD", "CHF", "CNY", "CZK", "DKK", "EUR", "GBP", "HKD", "HUF", "IDR", "ILS",
    "INR", "ISK", "JPY", "KRW", "MXN", "MYR", "NOK", "NZD", "PHP", "PLN", "RON", "SEK", "SGD",
    "THB", "TRY", "USD", "ZAR",
];

/// Days of rates a quote asks for, to find the last two published days
/// across a weekend and a holiday or two.
const QUOTE_LOOKBACK_DAYS: i64 = 10;

/// `(base, quote)` for a Yahoo FX symbol whose two currencies the ECB
/// publishes: `EURUSD=X` → (`EUR`, `USD`), `INR=X` → (`USD`, `INR`).
pub fn pair(symbol: &str) -> Option<(String, String)> {
    let upper = symbol.trim().to_ascii_uppercase();
    let code = upper.strip_suffix("=X")?;
    if !code.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let (base, quote) = match code.len() {
        6 => (&code[..3], &code[3..]),
        3 => ("USD", code),
        _ => return None,
    };
    (base != quote && CURRENCIES.contains(&base) && CURRENCIES.contains(&quote))
        .then(|| (base.to_string(), quote.to_string()))
}

/// Frankfurter answers errors with `{"message": "..."}` and no rates.
fn check(json: &serde_json::Value) -> Result<(), ProviderError> {
    if json.get("rates").is_some() {
        return Ok(());
    }
    Err(ProviderError::Other(
        json["message"]
            .as_str()
            .unwrap_or("no rates in the response")
            .to_string(),
    ))
}

pub struct Frankfurter {
    pub(super) http: Http,
}

impl Frankfurter {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    /// Daily rates for `symbol` from `start` (or from 1999, the first ECB
    /// rates) to the latest published day.
    pub async fn daily(&self, symbol: &str, start: Option<NaiveDate>) -> Result<Series> {
        let Some((base, quote)) = pair(symbol) else {
            bail!("Frankfurter has no ECB reference rate for {symbol}");
        };
        let from = start
            .unwrap_or_else(|| NaiveDate::from_ymd_opt(1999, 1, 4).unwrap_or_default())
            .format("%Y-%m-%d");
        let json = self
            .http
            .get_json(
                &format!("/v1/{from}.."),
                &[("base", base.as_str()), ("symbols", quote.as_str())],
                super::Auth::None,
                // Published once a working day: a few hours is fresh enough.
                Duration::from_secs(6 * 3600),
                check,
            )
            .await?;
        Ok(parse_series(&json, &quote))
    }

    /// The latest reference rate, with the change from the day before.
    pub async fn quote(&self, symbol: &str) -> Result<Option<Quote>> {
        if pair(symbol).is_none() {
            return Ok(None);
        }
        let today = chrono::Utc::now().date_naive();
        let series = self
            .daily(
                symbol,
                Some(today - chrono::Duration::days(QUOTE_LOOKBACK_DAYS)),
            )
            .await?;
        Ok(quote_from_series(symbol, &series))
    }
}

/// A quote from the last two daily rates, dated by the day it was set.
pub fn quote_from_series(symbol: &str, series: &Series) -> Option<Quote> {
    let mut q = super::quote_from_bars(series)?;
    let date = series.bars.last()?.date;
    q.regular_market_time = super::trade_time(symbol, date);
    Some(q)
}

/// `{"rates": {"2026-11-02": {"INR": 94.91}, ...}}` → one bar per day, the
/// rate as the close.
pub fn parse_series(json: &serde_json::Value, quote: &str) -> Series {
    let bars = json["rates"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(date, rates)| {
            Some(Bar {
                date: NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?,
                open: None,
                high: None,
                low: None,
                close: Some(num(&rates[quote])?).filter(|r| *r > 0.0),
                volume: None,
            })
        })
        .filter(|b| b.close.is_some())
        .collect();
    Series {
        bars,
        currency: Some(quote.to_string()),
    }
    .sorted()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{MarketData, Provider, ProviderKind};
    use crate::testutil::MockServer;

    const SERIES: &str = include_str!("fixtures/frankfurter_usd_inr.json");

    #[test]
    fn maps_yahoo_pairs_to_ecb_currencies() {
        assert_eq!(pair("USDINR=X"), Some(("USD".into(), "INR".into())));
        assert_eq!(pair("eurusd=x"), Some(("EUR".into(), "USD".into())));
        assert_eq!(pair("INR=X"), Some(("USD".into(), "INR".into())));
        // Not an ECB currency, not FX, or the same currency twice.
        assert_eq!(pair("USDAED=X"), None);
        assert_eq!(pair("BTC-USD"), None);
        assert_eq!(pair("AAPL"), None);
        assert_eq!(pair("USDUSD=X"), None);
        assert_eq!(pair("DX-Y.NYB"), None);
    }

    #[test]
    fn parses_the_time_series_into_daily_closes() {
        let s = parse_series(&serde_json::from_str(SERIES).unwrap(), "INR");
        assert_eq!(s.bars.len(), 8);
        assert_eq!(s.bars[0].date.to_string(), "2026-10-30");
        assert_eq!(s.bars[7].close, Some(93.93));
        assert_eq!(s.bars[7].open, None, "a reference rate has no OHLC");
        assert_eq!(s.currency.as_deref(), Some("INR"));
        let q = quote_from_series("USDINR=X", &s).unwrap();
        assert_eq!(q.regular_market_price, Some(93.93));
        assert_eq!(q.regular_market_previous_close, Some(93.93));
        assert_eq!(q.regular_market_change, Some(0.0));
    }

    #[test]
    fn an_error_body_is_an_error() {
        let e = serde_json::json!({"message": "not found"});
        assert!(check(&e).is_err());
        assert!(check(&serde_json::from_str(SERIES).unwrap()).is_ok());
    }

    #[tokio::test]
    async fn quote_asks_without_a_key_and_names_the_ecb() {
        let server = MockServer::start(vec![(200, SERIES.into())]).await;
        let p = Provider::new(
            ProviderKind::Frankfurter,
            String::new(),
            Some(server.url.clone()),
        );
        let q = p.quotes(&["USDINR=X"]).await.unwrap();
        assert_eq!(q[0].regular_market_price, Some(93.93));
        assert_eq!(q[0].currency.as_deref(), Some("INR"));
        assert_eq!(
            q[0].data_source.as_deref(),
            Some(ProviderKind::Frankfurter.name())
        );
        let req = &server.requests()[0];
        assert!(req.target.starts_with("/v1/"), "{}", req.target);
        assert!(
            req.target.contains("..?base=USD&symbols=INR"),
            "{}",
            req.target
        );
        assert_eq!(req.header("authorization"), None);
    }
}
