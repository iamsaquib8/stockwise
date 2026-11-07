//! Twelve Data (<https://twelvedata.com>). The free Basic plan: 8 credits a
//! minute, 800 a day (one credit per symbol), US stocks, FX and crypto. NSE
//! and BSE need the paid Grow plan; on Basic those requests are refused and
//! Yahoo answers. Basic data is licensed for non-display use only — check
//! Twelve Data's terms before showing it to others.
//!
//! Prices arrive as strings; errors as `{"status": "error", "code": ...,
//! "message": ...}`.

use super::{Auth, Bar, Http, ProviderError, Series, int, num};
use crate::api::{Quote, SearchResult};
use anyhow::Result;
use chrono::NaiveDate;
use std::time::Duration;

pub const BASE_URL: &str = "https://api.twelvedata.com";
const AUTH: Auth = Auth::Header("authorization", "apikey ");

/// `(symbol, exchange)` for a Yahoo symbol: `BTC-USD` → `BTC/USD`,
/// `USDINR=X` → `USD/INR`, `RELIANCE.NS` → (`RELIANCE`, `NSE`).
pub fn target(symbol: &str) -> Option<(String, Option<&'static str>)> {
    use crate::market::AssetClass;
    let upper = symbol.trim().to_ascii_uppercase();
    match AssetClass::of(&upper) {
        AssetClass::Index => None,
        AssetClass::Crypto => Some((upper.replacen('-', "/", 1), None)),
        AssetClass::Fx => {
            let pair = upper.trim_end_matches("=X");
            // Currency codes are ASCII letters; anything else would also make
            // the byte slicing below split a character.
            if !pair.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            match pair.len() {
                6 => Some((format!("{}/{}", &pair[..3], &pair[3..]), None)),
                3 => Some((format!("USD/{}", pair), None)),
                _ => None,
            }
        }
        AssetClass::Stock => match upper.rsplit_once('.') {
            None => Some((upper, None)),
            Some((base, "NS")) => Some((base.to_string(), Some("NSE"))),
            Some((base, "BO")) => Some((base.to_string(), Some("BSE"))),
            Some(_) => None,
        },
    }
}

/// Twelve Data symbol + exchange → Yahoo symbol.
pub fn to_yahoo(symbol: &str, exchange: Option<&str>, instrument_type: Option<&str>) -> String {
    let upper = symbol.to_ascii_uppercase();
    if let Some((base, quote)) = upper.split_once('/') {
        return if instrument_type.is_some_and(|t| t.contains("Digital")) {
            format!("{}-{}", base, quote)
        } else {
            format!("{}{}=X", base, quote)
        };
    }
    match exchange {
        Some("NSE") => format!("{}.NS", upper),
        Some("BSE") => format!("{}.BO", upper),
        _ => upper,
    }
}

fn check(json: &serde_json::Value) -> Result<(), ProviderError> {
    if json["status"].as_str() != Some("error") {
        return Ok(());
    }
    let msg = json["message"].as_str().unwrap_or("error").to_string();
    Err(match json["code"].as_u64() {
        Some(429) => ProviderError::RateLimited(msg),
        Some(401) | Some(403) => ProviderError::Denied(msg),
        _ => ProviderError::Other(msg),
    })
}

pub struct TwelveData {
    pub(super) http: Http,
}

impl TwelveData {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    fn params<'a>(symbol: &'a str, exchange: Option<&'a str>) -> Vec<(&'a str, &'a str)> {
        let mut params = vec![("symbol", symbol)];
        if let Some(ex) = exchange {
            params.push(("exchange", ex));
        }
        params
    }

    pub async fn quote(&self, symbol: &str) -> Result<Option<Quote>> {
        let Some((sym, exchange)) = target(symbol) else {
            return Ok(None);
        };
        let json = self
            .http
            .get_json(
                "/quote",
                &Self::params(&sym, exchange),
                AUTH,
                Duration::from_secs(60),
                check,
            )
            .await?;
        Ok(parse_quote(&json))
    }

    pub async fn daily(&self, symbol: &str, start: Option<NaiveDate>) -> Result<Series> {
        let (sym, exchange) = target(symbol)
            .ok_or_else(|| anyhow::anyhow!("Twelve Data does not cover {}", symbol))?;
        let start_date = start.map(|d| d.format("%Y-%m-%d").to_string());
        let mut params = Self::params(&sym, exchange);
        params.extend([
            ("interval", "1day"),
            ("outputsize", "5000"),
            ("order", "asc"),
        ]);
        if let Some(d) = &start_date {
            params.push(("start_date", d));
        }
        let json = self
            .http
            .get_json(
                "/time_series",
                &params,
                AUTH,
                Duration::from_secs(600),
                check,
            )
            .await?;
        Ok(parse_time_series(&json))
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let json = self
            .http
            .get_json(
                "/symbol_search",
                &[("symbol", query), ("outputsize", "10")],
                AUTH,
                Duration::from_secs(3600),
                check,
            )
            .await?;
        Ok(parse_search(&json))
    }
}

pub fn parse_quote(json: &serde_json::Value) -> Option<Quote> {
    let price = num(&json["close"])?;
    Some(Quote {
        short_name: json["name"].as_str().map(str::to_string),
        regular_market_price: Some(price),
        // The time of the bar the price belongs to, not the time of the call.
        regular_market_time: json["timestamp"].as_i64(),
        regular_market_open: num(&json["open"]),
        regular_market_day_high: num(&json["high"]),
        regular_market_day_low: num(&json["low"]),
        regular_market_volume: int(&json["volume"]),
        regular_market_previous_close: num(&json["previous_close"]),
        regular_market_change: num(&json["change"]),
        regular_market_change_percent: num(&json["percent_change"]),
        average_daily_volume_3_month: int(&json["average_volume"]),
        fifty_two_week_low: num(&json["fifty_two_week"]["low"]),
        fifty_two_week_high: num(&json["fifty_two_week"]["high"]),
        currency: json["currency"].as_str().map(str::to_string),
        exchange: json["exchange"].as_str().map(str::to_string),
        ..Default::default()
    })
}

pub fn parse_time_series(json: &serde_json::Value) -> Series {
    let bars = json["values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(Bar {
                date: NaiveDate::parse_from_str(v["datetime"].as_str()?.get(..10)?, "%Y-%m-%d")
                    .ok()?,
                open: num(&v["open"]),
                high: num(&v["high"]),
                low: num(&v["low"]),
                close: num(&v["close"]),
                volume: int(&v["volume"]),
            })
        })
        .collect();
    let meta = &json["meta"];
    // Stocks say `currency`; crypto and FX name the quote currency in words,
    // so take it from the pair instead.
    let currency = meta["currency"].as_str().map(str::to_string).or_else(|| {
        meta["symbol"]
            .as_str()
            .and_then(|s| s.split_once('/'))
            .map(|(_, quote)| quote.to_string())
    });
    Series { bars, currency }.sorted()
}

pub fn parse_search(json: &serde_json::Value) -> Vec<SearchResult> {
    json["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let instrument = r["instrument_type"].as_str();
            Some(SearchResult {
                symbol: Some(to_yahoo(
                    r["symbol"].as_str()?,
                    r["exchange"].as_str(),
                    instrument,
                )),
                short_name: r["instrument_name"].as_str().map(str::to_string),
                long_name: None,
                quote_type: instrument.map(|t| t.to_ascii_uppercase()),
                exchange: r["exchange"].as_str().map(str::to_string),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{MarketData, Provider, ProviderKind};
    use crate::testutil::MockServer;

    const QUOTE: &str = r#"{"symbol":"AAPL","name":"Apple Inc.","exchange":"NASDAQ","mic_code":"XNGS","currency":"USD","datetime":"2026-10-20","timestamp":1792503000,"last_quote_at":1792525380,"open":"339.83","high":"343.06","low":"338.71","close":"339.21","volume":"797442","previous_close":"342.94000","change":"-3.73000","percent_change":"-1.087655","average_volume":"35736594","is_market_open":true,"fifty_two_week":{"low":"243.42000","high":"348.14999","low_change":"95.79000","high_change":"-8.93999","low_change_percent":"39.35174","high_change_percent":"-2.56786","range":"243.419998 - 348.149994"}}"#;
    const SERIES: &str = r#"{"meta":{"symbol":"AAPL","interval":"1day","currency":"USD","exchange_timezone":"America/New_York","exchange":"NASDAQ","mic_code":"XNGS","type":"Common Stock"},"values":[{"datetime":"2026-10-20","open":"339.83","high":"343.06","low":"338.71","close":"339.21","volume":"797442"},{"datetime":"2026-10-19","open":"340.78000","high":"346","low":"340.12000","close":"342.94000","volume":"34547800"}],"status":"ok"}"#;
    const CRYPTO_SERIES: &str = r#"{"meta":{"symbol":"BTC/USD","interval":"1day","currency_base":"Bitcoin","currency_quote":"US Dollar","exchange":"Binance","type":"Digital Currency"},"values":[{"datetime":"2026-10-20","open":"104000","high":"106000","low":"103000","close":"105500"}],"status":"ok"}"#;
    const SEARCH: &str = r#"{"data":[{"symbol":"RELIANCE","instrument_name":"Reliance Industries Limited","exchange":"NSE","mic_code":"XNSE","exchange_timezone":"Asia/Kolkata","instrument_type":"Common Stock","country":"India","currency":"INR"},{"symbol":"BTC/USD","instrument_name":"Bitcoin US Dollar","exchange":"Binance","instrument_type":"Digital Currency","country":"","currency":""},{"symbol":"EUR/USD","instrument_name":"Euro / US Dollar","exchange":"Forex","instrument_type":"Physical Currency","country":"","currency":""}],"status":"ok"}"#;

    #[test]
    fn maps_symbols_both_ways() {
        assert_eq!(target("AAPL"), Some(("AAPL".into(), None)));
        assert_eq!(
            target("RELIANCE.NS"),
            Some(("RELIANCE".into(), Some("NSE")))
        );
        assert_eq!(target("BTC-INR"), Some(("BTC/INR".into(), None)));
        assert_eq!(target("USDINR=X"), Some(("USD/INR".into(), None)));
        assert_eq!(target("^NSEI"), None);
        assert_eq!(target("VOD.L"), None);
        assert_eq!(to_yahoo("RELIANCE", Some("NSE"), None), "RELIANCE.NS");
        assert_eq!(
            to_yahoo("BTC/USD", Some("Binance"), Some("Digital Currency")),
            "BTC-USD"
        );
        assert_eq!(
            to_yahoo("EUR/USD", Some("Forex"), Some("Physical Currency")),
            "EURUSD=X"
        );
    }

    #[test]
    fn parses_quote_fixture_with_string_numbers() {
        let q = parse_quote(&serde_json::from_str(QUOTE).unwrap()).unwrap();
        assert_eq!(q.regular_market_time, Some(1_792_503_000));
        assert_eq!(q.regular_market_price, Some(339.21));
        assert_eq!(q.regular_market_change, Some(-3.73));
        assert_eq!(q.regular_market_change_percent, Some(-1.087655));
        assert_eq!(q.fifty_two_week_high, Some(348.14999));
        assert_eq!(q.average_daily_volume_3_month, Some(35_736_594));
        assert_eq!(q.currency.as_deref(), Some("USD"));
        assert_eq!(q.short_name.as_deref(), Some("Apple Inc."));
    }

    #[test]
    fn parses_time_series_in_date_order() {
        let s = parse_time_series(&serde_json::from_str(SERIES).unwrap());
        assert_eq!(s.bars.len(), 2);
        assert_eq!(s.bars[0].date.to_string(), "2026-10-19");
        assert_eq!(s.bars[1].close, Some(339.21));
        let c = parse_time_series(&serde_json::from_str(CRYPTO_SERIES).unwrap());
        assert_eq!(c.currency.as_deref(), Some("USD"));
        assert_eq!(c.bars[0].volume, None);
    }

    #[test]
    fn parses_search_back_to_yahoo_symbols() {
        let r = parse_search(&serde_json::from_str(SEARCH).unwrap());
        let syms: Vec<_> = r.iter().filter_map(|r| r.symbol.as_deref()).collect();
        assert_eq!(syms, ["RELIANCE.NS", "BTC-USD", "EURUSD=X"]);
    }

    #[test]
    fn classifies_errors_by_code() {
        let limit = serde_json::json!({"code": 429, "message": "You have run out of API credits", "status": "error"});
        assert!(matches!(check(&limit), Err(ProviderError::RateLimited(_))));
        let plan = serde_json::json!({"code": 403, "message": "available starting with Grow", "status": "error"});
        assert!(matches!(check(&plan), Err(ProviderError::Denied(_))));
        assert!(check(&serde_json::from_str(QUOTE).unwrap()).is_ok());
    }

    #[tokio::test]
    async fn quote_uses_apikey_header_and_exchange_param() {
        let server = MockServer::start(vec![(200, QUOTE.into())]).await;
        let p = Provider::new(
            ProviderKind::TwelveData,
            "td-secret".into(),
            Some(server.url.clone()),
        );
        let q = p.quotes(&["RELIANCE.NS"]).await.unwrap();
        assert_eq!(q[0].symbol.as_deref(), Some("RELIANCE.NS"));
        let req = &server.requests()[0];
        assert_eq!(req.target, "/quote?symbol=RELIANCE&exchange=NSE");
        assert_eq!(req.header("authorization"), Some("apikey td-secret"));
    }

    #[tokio::test]
    async fn per_run_budget_caps_requests() {
        let server = MockServer::start(vec![(200, QUOTE.into())]).await;
        let p = Provider::new(
            ProviderKind::TwelveData,
            "td-secret".into(),
            Some(server.url.clone()),
        );
        let symbols: Vec<String> = (0..12).map(|i| format!("SYM{i}")).collect();
        let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
        let q = p.quotes(&refs).await.unwrap();
        assert_eq!(q.len(), 8, "free tier: 8 credits a minute");
        assert_eq!(server.requests().len(), 8);
        assert_eq!(p.remaining(), 0);
    }
}
