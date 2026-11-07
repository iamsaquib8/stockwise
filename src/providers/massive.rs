//! Massive, formerly Polygon.io (<https://massive.com>). The free Basic plans
//! give US stocks, FX and crypto: 5 requests a minute, end-of-day data and
//! two years of history. No Indian exchanges and no indices on the free
//! plan. Old `api.polygon.io` keys work unchanged.
//!
//! A quote is the latest daily aggregate bar with the change against the
//! bar before it (the real-time snapshot endpoint is paid).

use super::{Auth, Bar, Http, ProviderError, Series, num, quote_from_bars, trade_time};
use crate::api::{Quote, SearchResult};
use anyhow::Result;
use chrono::NaiveDate;
use std::time::Duration;

pub const BASE_URL: &str = "https://api.massive.com";
const AUTH: Auth = Auth::Header("authorization", "Bearer ");

/// The Massive ticker for a Yahoo symbol: `AAPL`, `BRK-B` → `BRK.B`,
/// `BTC-USD` → `X:BTCUSD`, `USDINR=X` → `C:USDINR`. `None` for anything
/// outside the US market (exchange suffixes, indices).
pub fn ticker(symbol: &str) -> Option<String> {
    use crate::market::AssetClass;
    let upper = symbol.trim().to_ascii_uppercase();
    match AssetClass::of(&upper) {
        AssetClass::Index => None,
        AssetClass::Crypto => Some(format!("X:{}", upper.replace('-', ""))),
        AssetClass::Fx => {
            let pair = upper.trim_end_matches("=X");
            // Currency codes are ASCII letters; anything else would also make
            // the byte slicing below split a character.
            if !pair.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            match pair.len() {
                6 => Some(format!("C:{}", pair)),
                3 => Some(format!("C:USD{}", pair)),
                _ => None,
            }
        }
        AssetClass::Stock if upper.contains('.') => None,
        AssetClass::Stock => Some(upper.replace('-', ".")),
    }
}

/// Massive ticker → Yahoo symbol.
pub fn to_yahoo(ticker: &str) -> String {
    if let Some(pair) = ticker.strip_prefix("C:") {
        return format!("{}=X", pair);
    }
    if let Some(pair) = ticker.strip_prefix("X:") {
        // Crypto pairs end in a 3-letter fiat code (X:BTCUSD).
        if pair.len() > 3 && pair.is_ascii() {
            let (coin, fiat) = pair.split_at(pair.len() - 3);
            return format!("{}-{}", coin, fiat);
        }
        return pair.to_string();
    }
    ticker.replace('.', "-")
}

fn check(json: &serde_json::Value) -> Result<(), ProviderError> {
    match json["status"].as_str() {
        Some("ERROR") => Err(ProviderError::Other(
            super::error_text(json).unwrap_or("error").into(),
        )),
        Some("NOT_AUTHORIZED") => Err(ProviderError::Denied(
            super::error_text(json).unwrap_or("not entitled").into(),
        )),
        _ => Ok(()),
    }
}

pub struct Massive {
    pub(super) http: Http,
}

impl Massive {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    async fn aggs(
        &self,
        ticker: &str,
        from: NaiveDate,
        to: NaiveDate,
        ttl: Duration,
    ) -> Result<Series> {
        let path = format!(
            "/v2/aggs/ticker/{}/range/1/day/{}/{}",
            ticker,
            from.format("%Y-%m-%d"),
            to.format("%Y-%m-%d")
        );
        let json = self
            .http
            .get_json(
                &path,
                &[("adjusted", "true"), ("sort", "asc"), ("limit", "50000")],
                AUTH,
                ttl,
                check,
            )
            .await?;
        Ok(parse_aggs(&json, ticker))
    }

    pub async fn quote(&self, symbol: &str) -> Result<Option<Quote>> {
        let Some(t) = ticker(symbol) else {
            return Ok(None);
        };
        let today = chrono::Utc::now().date_naive();
        // Two weeks reach back over any holiday stretch for two bars.
        let series = self
            .aggs(
                &t,
                today - chrono::Duration::days(14),
                today,
                Duration::from_secs(300),
            )
            .await?;
        Ok(quote_from_bars(&series).map(|mut q| {
            // The latest bar's day, not today: free plans are end-of-day.
            q.regular_market_time = series.bars.last().and_then(|b| trade_time(symbol, b.date));
            q
        }))
    }

    pub async fn daily(&self, symbol: &str, start: Option<NaiveDate>) -> Result<Series> {
        let t =
            ticker(symbol).ok_or_else(|| anyhow::anyhow!("Massive does not cover {}", symbol))?;
        let today = chrono::Utc::now().date_naive();
        // The free plan keeps two years; older ranges fail the coverage
        // check and go to Yahoo.
        let from = start.unwrap_or(today - chrono::Duration::days(731));
        self.aggs(&t, from, today, Duration::from_secs(600)).await
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let json = self
            .http
            .get_json(
                "/v3/reference/tickers",
                &[("search", query), ("active", "true"), ("limit", "10")],
                AUTH,
                Duration::from_secs(3600),
                check,
            )
            .await?;
        Ok(parse_search(&json))
    }
}

/// Daily bars from an aggregates response. `t` is the start of the day in
/// milliseconds (New York midnight for stocks, UTC for crypto); its UTC date
/// is the trading day either way.
pub fn parse_aggs(json: &serde_json::Value, ticker: &str) -> Series {
    let bars = json["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let ms = r["t"].as_i64()?;
            Some(Bar {
                date: chrono::DateTime::from_timestamp_millis(ms)?.date_naive(),
                open: num(&r["o"]),
                high: num(&r["h"]),
                low: num(&r["l"]),
                close: num(&r["c"]),
                volume: num(&r["v"]).map(|v| v.max(0.0) as u64),
            })
        })
        .collect();
    let currency = if let Some(pair) = ticker.strip_prefix("C:").or(ticker.strip_prefix("X:")) {
        pair.get(pair.len().saturating_sub(3)..).map(str::to_string)
    } else {
        Some("USD".to_string())
    };
    Series { bars, currency }.sorted()
}

pub fn parse_search(json: &serde_json::Value) -> Vec<SearchResult> {
    json["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let quote_type = match r["market"].as_str() {
                Some("crypto") => "CRYPTOCURRENCY",
                Some("fx") => "CURRENCY",
                Some("indices") => "INDEX",
                _ if r["type"] == "ETF" => "ETF",
                _ => "EQUITY",
            };
            Some(SearchResult {
                symbol: Some(to_yahoo(r["ticker"].as_str()?)),
                short_name: r["name"].as_str().map(str::to_string),
                long_name: None,
                quote_type: Some(quote_type.to_string()),
                exchange: r["primary_exchange"].as_str().map(str::to_string),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{MarketData, Provider, ProviderKind};
    use crate::testutil::MockServer;

    const AGGS: &str = r#"{"adjusted":true,"queryCount":2,"request_id":"6a7e","results":[{"c":75.0875,"h":75.15,"l":73.7975,"n":1,"o":74.06,"t":1577941200000,"v":135647456,"vw":74.6099},{"c":74.3575,"h":75.145,"l":74.125,"o":74.2875,"t":1578027600000,"v":146535512,"vw":74.7026}],"resultsCount":2,"status":"OK","ticker":"AAPL"}"#;
    const TICKERS: &str = r#"{"count":2,"request_id":"e700","results":[{"active":true,"locale":"us","market":"stocks","name":"Berkshire Hathaway Inc. Class B","primary_exchange":"XNYS","ticker":"BRK.B","type":"CS"},{"active":true,"locale":"global","market":"crypto","name":"Bitcoin - United States dollar","ticker":"X:BTCUSD"}],"status":"OK"}"#;

    #[test]
    fn maps_symbols_both_ways() {
        assert_eq!(ticker("aapl").as_deref(), Some("AAPL"));
        assert_eq!(ticker("BRK-B").as_deref(), Some("BRK.B"));
        assert_eq!(ticker("BTC-USD").as_deref(), Some("X:BTCUSD"));
        assert_eq!(ticker("USDINR=X").as_deref(), Some("C:USDINR"));
        assert_eq!(ticker("TCS.NS"), None);
        assert_eq!(ticker("^GSPC"), None);
        assert_eq!(to_yahoo("X:BTCUSD"), "BTC-USD");
        assert_eq!(to_yahoo("C:EURUSD"), "EURUSD=X");
        assert_eq!(to_yahoo("BRK.B"), "BRK-B");
    }

    #[test]
    fn parses_aggregates_fixture() {
        let s = parse_aggs(&serde_json::from_str(AGGS).unwrap(), "AAPL");
        assert_eq!(s.bars.len(), 2);
        assert_eq!(s.bars[0].date.to_string(), "2020-01-02");
        assert_eq!(s.bars[1].date.to_string(), "2020-01-03");
        assert_eq!(s.bars[1].close, Some(74.3575));
        assert_eq!(s.bars[0].volume, Some(135_647_456));
        assert_eq!(s.currency.as_deref(), Some("USD"));
        let q = quote_from_bars(&s).unwrap();
        assert!((q.regular_market_change.unwrap() - (74.3575 - 75.0875)).abs() < 1e-9);
        let fx = parse_aggs(&serde_json::json!({"results": []}), "C:USDINR");
        assert_eq!(fx.currency.as_deref(), Some("INR"));
    }

    #[test]
    fn parses_ticker_search() {
        let r = parse_search(&serde_json::from_str(TICKERS).unwrap());
        assert_eq!(r[0].symbol.as_deref(), Some("BRK-B"));
        assert_eq!(r[0].quote_type.as_deref(), Some("EQUITY"));
        assert_eq!(r[1].symbol.as_deref(), Some("BTC-USD"));
        assert_eq!(r[1].quote_type.as_deref(), Some("CRYPTOCURRENCY"));
    }

    #[tokio::test]
    async fn quote_sends_bearer_key_and_never_puts_it_in_the_url() {
        let server = MockServer::start(vec![(200, AGGS.into())]).await;
        let p = Provider::new(
            ProviderKind::Massive,
            "ms-secret".into(),
            Some(server.url.clone()),
        );
        let q = p.quotes(&["AAPL"]).await.unwrap();
        assert_eq!(q[0].regular_market_price, Some(74.3575));
        assert_eq!(q[0].data_source.as_deref(), Some("Massive (Polygon.io)"));
        // The quote is dated by its bar (Fri 3 Jan 2020, closed 16:00 ET),
        // not by when it was asked for.
        assert_eq!(q[0].regular_market_time, Some(1_578_085_200));
        let req = &server.requests()[0];
        assert!(req.target.starts_with("/v2/aggs/ticker/AAPL/range/1/day/"));
        assert!(!req.target.contains("ms-secret"));
        assert_eq!(req.header("authorization"), Some("Bearer ms-secret"));
    }

    #[tokio::test]
    async fn refused_key_is_reported_and_stops_the_provider() {
        let server = MockServer::start(vec![(
            401,
            r#"{"status":"ERROR","request_id":"x","error":"Unknown API Key"}"#.into(),
        )])
        .await;
        let p = Provider::new(
            ProviderKind::Massive,
            "ms-secret".into(),
            Some(server.url.clone()),
        );
        let err = p.quotes(&["AAPL"]).await.unwrap_err().to_string();
        assert!(
            err.contains("Unknown API Key") && !err.contains("ms-secret"),
            "{err}"
        );
        assert_eq!(p.remaining(), 0);
    }
}
