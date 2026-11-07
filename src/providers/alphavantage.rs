//! Alpha Vantage (<https://www.alphavantage.co>). Free key: 25 requests a
//! day, end-of-day quotes, the latest 100 daily bars (`outputsize=full` and
//! intraday are premium). Covers US stocks, BSE (`RELIANCE.BO` →
//! `RELIANCE.BSE`) and a few other exchanges, FX and crypto; not NSE or
//! indices. Every value arrives as a string, and errors arrive as HTTP 200
//! with an `Information`, `Note` or `Error Message` key.

use super::{Auth, Bar, Http, ProviderError, Series, int, num, trade_time};
use crate::api::{Quote, SearchResult};
use anyhow::Result;
use chrono::NaiveDate;
use std::time::Duration;

pub const BASE_URL: &str = "https://www.alphavantage.co";
const AUTH: Auth = Auth::Query("apikey");

/// Yahoo exchange suffix → Alpha Vantage suffix.
const SUFFIXES: &[(&str, &str)] = &[
    (".BO", ".BSE"),
    (".L", ".LON"),
    (".TO", ".TRT"),
    (".V", ".TRV"),
    (".DE", ".DEX"),
    (".SS", ".SHH"),
    (".SZ", ".SHZ"),
];

#[derive(Debug, PartialEq)]
pub enum Target {
    Equity(String),
    Fx { from: String, to: String },
    Crypto { coin: String, market: String },
}

/// The Alpha Vantage request for a Yahoo symbol, or `None` if it isn't
/// covered (NSE, indices, unknown exchanges).
pub fn target(symbol: &str) -> Option<Target> {
    use crate::market::AssetClass;
    let upper = symbol.trim().to_ascii_uppercase();
    match AssetClass::of(&upper) {
        AssetClass::Index => None,
        AssetClass::Fx => {
            let pair = upper.trim_end_matches("=X");
            // Currency codes are ASCII letters; anything else would also make
            // the byte slicing below split a character.
            if !pair.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            match pair.len() {
                6 => Some(Target::Fx {
                    from: pair[..3].into(),
                    to: pair[3..].into(),
                }),
                // Yahoo's `INR=X` is USD/INR.
                3 => Some(Target::Fx {
                    from: "USD".into(),
                    to: pair.into(),
                }),
                _ => None,
            }
        }
        AssetClass::Crypto => {
            let (coin, market) = upper.rsplit_once('-')?;
            Some(Target::Crypto {
                coin: coin.into(),
                market: market.into(),
            })
        }
        AssetClass::Stock => match upper.rfind('.') {
            None => Some(Target::Equity(upper)),
            Some(dot) => SUFFIXES
                .iter()
                .find(|(yahoo, _)| upper[dot..] == **yahoo)
                .map(|(_, av)| Target::Equity(format!("{}{}", &upper[..dot], av))),
        },
    }
}

/// Trading currency of an Alpha Vantage equity symbol, where it is
/// unambiguous: US listings and BSE. (London quotes in pence, for one.)
fn equity_currency(av_symbol: &str) -> Option<&'static str> {
    match av_symbol.rsplit_once('.') {
        None => Some("USD"),
        Some((_, "BSE")) => Some("INR"),
        Some(_) => None,
    }
}

/// Alpha Vantage symbol → Yahoo symbol (`RELIANCE.BSE` → `RELIANCE.BO`).
pub fn to_yahoo(symbol: &str) -> String {
    let upper = symbol.to_ascii_uppercase();
    if let Some(dot) = upper.rfind('.')
        && let Some((yahoo, _)) = SUFFIXES.iter().find(|(_, av)| upper[dot..] == **av)
    {
        return format!("{}{}", &upper[..dot], yahoo);
    }
    upper
}

/// In-band errors: everything Alpha Vantage has to say comes back as 200.
fn check(json: &serde_json::Value) -> Result<(), ProviderError> {
    if let Some(msg) = json["Error Message"].as_str() {
        return Err(ProviderError::Other(msg.into()));
    }
    for key in ["Information", "Note"] {
        if let Some(msg) = json[key].as_str() {
            let lower = msg.to_ascii_lowercase();
            return Err(
                if lower.contains("rate limit") || lower.contains("call frequency") {
                    ProviderError::RateLimited(msg.into())
                } else if lower.contains("premium") || lower.contains("api key") {
                    ProviderError::Denied(msg.into())
                } else {
                    ProviderError::Other(msg.into())
                },
            );
        }
    }
    Ok(())
}

pub struct AlphaVantage {
    pub(super) http: Http,
}

impl AlphaVantage {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    async fn call(&self, params: &[(&str, &str)], ttl: Duration) -> Result<serde_json::Value> {
        self.http.get_json("/query", params, AUTH, ttl, check).await
    }

    pub async fn quote(&self, symbol: &str) -> Result<Option<Quote>> {
        let ttl = Duration::from_secs(60);
        match target(symbol) {
            None => Ok(None),
            Some(Target::Equity(sym)) => {
                let json = self
                    .call(&[("function", "GLOBAL_QUOTE"), ("symbol", &sym)], ttl)
                    .await?;
                Ok(parse_global_quote(&json).map(|mut q| {
                    q.currency = equity_currency(&sym).map(str::to_string);
                    // End-of-day data: dated by the day it is for, not today.
                    q.regular_market_time =
                        latest_trading_day(&json).and_then(|d| trade_time(symbol, d));
                    q
                }))
            }
            Some(Target::Fx { from, to }) => {
                let json = self.exchange_rate(&from, &to, ttl).await?;
                Ok(parse_exchange_rate(&json))
            }
            Some(Target::Crypto { coin, market }) => {
                let json = self.exchange_rate(&coin, &market, ttl).await?;
                Ok(parse_exchange_rate(&json))
            }
        }
    }

    async fn exchange_rate(
        &self,
        from: &str,
        to: &str,
        ttl: Duration,
    ) -> Result<serde_json::Value> {
        self.call(
            &[
                ("function", "CURRENCY_EXCHANGE_RATE"),
                ("from_currency", from),
                ("to_currency", to),
            ],
            ttl,
        )
        .await
    }

    pub async fn daily(&self, symbol: &str, _start: Option<NaiveDate>) -> Result<Series> {
        let ttl = Duration::from_secs(600);
        let target = target(symbol)
            .ok_or_else(|| anyhow::anyhow!("Alpha Vantage does not cover {}", symbol))?;
        let (json, key) = match &target {
            // `compact` (100 bars) is what free keys get; longer ranges fail
            // the coverage check and go to Yahoo.
            Target::Equity(sym) => (
                self.call(
                    &[
                        ("function", "TIME_SERIES_DAILY"),
                        ("symbol", sym),
                        ("outputsize", "compact"),
                    ],
                    ttl,
                )
                .await?,
                "Time Series (Daily)",
            ),
            Target::Fx { from, to } => (
                self.call(
                    &[
                        ("function", "FX_DAILY"),
                        ("from_symbol", from),
                        ("to_symbol", to),
                        ("outputsize", "compact"),
                    ],
                    ttl,
                )
                .await?,
                "Time Series FX (Daily)",
            ),
            Target::Crypto { coin, market } => (
                self.call(
                    &[
                        ("function", "DIGITAL_CURRENCY_DAILY"),
                        ("symbol", coin),
                        ("market", market),
                    ],
                    ttl,
                )
                .await?,
                "Time Series (Digital Currency Daily)",
            ),
        };
        let mut series = parse_series(&json, key);
        series.currency = match &target {
            Target::Fx { to, .. } => Some(to.clone()),
            Target::Crypto { market, .. } => Some(market.clone()),
            Target::Equity(sym) => equity_currency(sym).map(str::to_string),
        };
        if series.currency.is_none() {
            // Prices without a currency would print as `$`; Yahoo knows it.
            anyhow::bail!("Alpha Vantage gives no currency for {}", symbol);
        }
        Ok(series)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let json = self
            .call(
                &[("function", "SYMBOL_SEARCH"), ("keywords", query)],
                Duration::from_secs(3600),
            )
            .await?;
        Ok(parse_search(&json))
    }
}

pub fn parse_global_quote(json: &serde_json::Value) -> Option<Quote> {
    let g = &json["Global Quote"];
    let price = num(&g["05. price"])?;
    Some(Quote {
        regular_market_price: Some(price),
        regular_market_open: num(&g["02. open"]),
        regular_market_day_high: num(&g["03. high"]),
        regular_market_day_low: num(&g["04. low"]),
        regular_market_volume: int(&g["06. volume"]),
        regular_market_previous_close: num(&g["08. previous close"]),
        regular_market_change: num(&g["09. change"]),
        regular_market_change_percent: num(&g["10. change percent"]),
        ..Default::default()
    })
}

/// The day a `GLOBAL_QUOTE` answer is for (`07. latest trading day`).
pub fn latest_trading_day(json: &serde_json::Value) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(
        json["Global Quote"]["07. latest trading day"].as_str()?,
        "%Y-%m-%d",
    )
    .ok()
}

pub fn parse_exchange_rate(json: &serde_json::Value) -> Option<Quote> {
    let r = &json["Realtime Currency Exchange Rate"];
    let rate = num(&r["5. Exchange Rate"])?;
    Some(Quote {
        regular_market_price: Some(rate),
        currency: r["3. To_Currency Code"].as_str().map(str::to_string),
        ..Default::default()
    })
}

/// Daily bars from any of the `Time Series ...` objects (keyed by date).
pub fn parse_series(json: &serde_json::Value, key: &str) -> Series {
    let bars = json[key]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(date, v)| {
            Some(Bar {
                date: NaiveDate::parse_from_str(date.get(..10)?, "%Y-%m-%d").ok()?,
                open: num(&v["1. open"]),
                high: num(&v["2. high"]),
                low: num(&v["3. low"]),
                close: num(&v["4. close"]),
                volume: int(&v["5. volume"]),
            })
        })
        .collect();
    Series {
        bars,
        currency: None,
    }
    .sorted()
}

pub fn parse_search(json: &serde_json::Value) -> Vec<SearchResult> {
    json["bestMatches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(SearchResult {
                symbol: Some(to_yahoo(m["1. symbol"].as_str()?)),
                short_name: m["2. name"].as_str().map(str::to_string),
                long_name: None,
                quote_type: m["3. type"].as_str().map(|t| t.to_ascii_uppercase()),
                exchange: m["4. region"].as_str().map(str::to_string),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{MarketData, Provider, ProviderKind, quote_from_bars};
    use crate::testutil::MockServer;

    const GLOBAL_QUOTE: &str = r#"{"Global Quote":{"01. symbol":"IBM","02. open":"230.6800","03. high":"238.3800","04. low":"229.2965","05. price":"236.3900","06. volume":"4908740","07. latest trading day":"2026-10-19","08. previous close":"232.1400","09. change":"4.2500","10. change percent":"1.8308%"}}"#;
    const DAILY: &str = r#"{"Meta Data":{"1. Information":"Daily Prices (open, high, low, close) and Volumes","2. Symbol":"IBM","3. Last Refreshed":"2026-10-19","4. Output Size":"Compact","5. Time Zone":"US/Eastern"},"Time Series (Daily)":{"2026-10-19":{"1. open":"230.6800","2. high":"238.3800","3. low":"229.2965","4. close":"236.3900","5. volume":"4908740"},"2026-10-16":{"1. open":"228.0000","2. high":"233.0000","3. low":"227.5000","4. close":"232.1400","5. volume":"3900000"}}}"#;
    const RATE: &str = r#"{"Realtime Currency Exchange Rate":{"1. From_Currency Code":"USD","2. From_Currency Name":"United States Dollar","3. To_Currency Code":"INR","4. To_Currency Name":"Indian Rupee","5. Exchange Rate":"84.12345000","6. Last Refreshed":"2026-10-20 19:39:17","7. Time Zone":"UTC","8. Bid Price":"84.12","9. Ask Price":"84.13"}}"#;
    const SEARCH: &str = r#"{"bestMatches":[{"1. symbol":"RELIANCE.BSE","2. name":"Reliance Industries Ltd","3. type":"Equity","4. region":"India/Bombay","5. marketOpen":"09:15","6. marketClose":"15:30","7. timezone":"UTC+5.5","8. currency":"INR","9. matchScore":"0.8"},{"1. symbol":"TSCO.LON","2. name":"Tesco PLC","3. type":"Equity","4. region":"United Kingdom","5. marketOpen":"08:00","6. marketClose":"16:30","7. timezone":"UTC+01","8. currency":"GBX","9. matchScore":"0.7273"}]}"#;
    const LIMIT: &str = r#"{"Information": "We have detected your API key as XXXX and our standard API rate limit is 25 requests per day. Please subscribe to any of the premium plans at https://www.alphavantage.co/premium/ to instantly remove all daily rate limits."}"#;

    #[test]
    fn maps_yahoo_symbols_to_alpha_vantage() {
        assert_eq!(target("aapl"), Some(Target::Equity("AAPL".into())));
        assert_eq!(
            target("RELIANCE.BO"),
            Some(Target::Equity("RELIANCE.BSE".into()))
        );
        assert_eq!(target("VOD.L"), Some(Target::Equity("VOD.LON".into())));
        assert_eq!(target("RELIANCE.NS"), None);
        assert_eq!(target("^NSEI"), None);
        assert_eq!(
            target("USDINR=X"),
            Some(Target::Fx {
                from: "USD".into(),
                to: "INR".into()
            })
        );
        assert_eq!(
            target("INR=X"),
            Some(Target::Fx {
                from: "USD".into(),
                to: "INR".into()
            })
        );
        assert_eq!(
            target("BTC-INR"),
            Some(Target::Crypto {
                coin: "BTC".into(),
                market: "INR".into()
            })
        );
        // Non-ASCII input is rejected, not sliced mid-character.
        assert_eq!(target("AB\u{20B9}C=X"), None);
        assert_eq!(crate::providers::twelvedata::target("AB\u{20B9}C=X"), None);
        assert_eq!(crate::providers::massive::ticker("AB\u{20B9}C=X"), None);
        assert_eq!(
            crate::providers::massive::to_yahoo("X:B\u{20B9}"),
            "B\u{20B9}"
        );
        assert_eq!(to_yahoo("RELIANCE.BSE"), "RELIANCE.BO");
        assert_eq!(to_yahoo("IBM"), "IBM");
    }

    #[test]
    fn parses_global_quote_fixture() {
        let q = parse_global_quote(&serde_json::from_str(GLOBAL_QUOTE).unwrap()).unwrap();
        assert_eq!(q.regular_market_price, Some(236.39));
        assert_eq!(q.regular_market_open, Some(230.68));
        assert_eq!(q.regular_market_previous_close, Some(232.14));
        assert_eq!(q.regular_market_change, Some(4.25));
        assert_eq!(q.regular_market_change_percent, Some(1.8308));
        assert_eq!(q.regular_market_volume, Some(4_908_740));
        // Unknown symbol: an empty "Global Quote" object.
        assert!(parse_global_quote(&serde_json::json!({"Global Quote": {}})).is_none());
    }

    #[test]
    fn parses_exchange_rate_and_daily_series() {
        let q = parse_exchange_rate(&serde_json::from_str(RATE).unwrap()).unwrap();
        assert_eq!(q.regular_market_price, Some(84.12345));
        assert_eq!(q.currency.as_deref(), Some("INR"));
        let s = parse_series(&serde_json::from_str(DAILY).unwrap(), "Time Series (Daily)");
        assert_eq!(s.bars.len(), 2);
        assert!(s.bars[0].date < s.bars[1].date, "sorted oldest first");
        assert_eq!(s.bars[1].close, Some(236.39));
        let q = quote_from_bars(&s).unwrap();
        assert!((q.regular_market_change.unwrap() - 4.25).abs() < 1e-9);
    }

    #[test]
    fn parses_search_back_to_yahoo_symbols() {
        let r = parse_search(&serde_json::from_str(SEARCH).unwrap());
        assert_eq!(r[0].symbol.as_deref(), Some("RELIANCE.BO"));
        assert_eq!(r[1].symbol.as_deref(), Some("TSCO.L"));
        assert_eq!(r[0].quote_type.as_deref(), Some("EQUITY"));
    }

    #[test]
    fn classifies_in_band_errors() {
        let limit: serde_json::Value = serde_json::from_str(LIMIT).unwrap();
        assert!(matches!(check(&limit), Err(ProviderError::RateLimited(_))));
        let legacy = serde_json::json!({"Note": "Thank you for using Alpha Vantage! Our standard API call frequency is 5 calls per minute and 500 calls per day."});
        assert!(matches!(check(&legacy), Err(ProviderError::RateLimited(_))));
        let premium = serde_json::json!({"Information": "Thank you for using Alpha Vantage! This is a premium endpoint."});
        assert!(matches!(check(&premium), Err(ProviderError::Denied(_))));
        let bad = serde_json::json!({"Error Message": "Invalid API call."});
        assert!(matches!(check(&bad), Err(ProviderError::Other(_))));
        assert!(check(&serde_json::from_str(GLOBAL_QUOTE).unwrap()).is_ok());
    }

    #[tokio::test]
    async fn quotes_over_http_with_key_in_query_only() {
        let server = MockServer::start(vec![(200, GLOBAL_QUOTE.into())]).await;
        let p = Provider::new(
            ProviderKind::AlphaVantage,
            "av-secret".into(),
            Some(server.url.clone()),
        );
        let quotes = p.quotes(&["IBM", "TCS.NS"]).await.unwrap();
        // TCS.NS is not covered: left for Yahoo, no request made.
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].symbol.as_deref(), Some("IBM"));
        assert_eq!(quotes[0].data_source.as_deref(), Some("Alpha Vantage"));
        // Dated by "latest trading day" (Mon 19 Oct 2026, IBM closes 16:00 ET).
        assert_eq!(quotes[0].regular_market_time, Some(1_792_440_000));
        let reqs = server.requests();
        assert_eq!(reqs.len(), 1);
        assert!(
            reqs[0]
                .target
                .starts_with("/query?function=GLOBAL_QUOTE&symbol=IBM")
        );
        assert!(reqs[0].target.contains("apikey=av-secret"));
    }

    #[tokio::test]
    async fn rate_limit_stops_the_provider_for_the_run() {
        let server = MockServer::start(vec![(200, LIMIT.into())]).await;
        let p = Provider::new(
            ProviderKind::AlphaVantage,
            "av-secret".into(),
            Some(server.url.clone()),
        );
        let err = p.quotes(&["IBM"]).await.unwrap_err().to_string();
        assert!(err.contains("rate limit"), "{err}");
        assert_eq!(p.remaining(), 0);
        // No second request once the provider said stop.
        assert!(p.quotes(&["MSFT"]).await.is_err());
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn short_history_is_refused_so_yahoo_serves_it() {
        let server = MockServer::start(vec![(200, DAILY.into())]).await;
        let p = Provider::new(
            ProviderKind::AlphaVantage,
            "k-1234".into(),
            Some(server.url.clone()),
        );
        // Beyond the free plan's 100 bars, or `max`: refused without
        // spending one of the 25 daily requests.
        assert!(p.chart("IBM", "1y", "1d").await.is_err());
        assert!(p.chart("IBM", "max", "1mo").await.is_err());
        // Intraday intervals are never asked of the provider.
        assert!(p.chart("IBM", "1d", "5m").await.is_err());
        assert_eq!(server.requests().len(), 0);
        // Within the plan, but the bars don't reach back far enough.
        assert!(p.chart("IBM", "1mo", "1d").await.is_err());
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn equity_currency_only_where_unambiguous() {
        assert_eq!(equity_currency("IBM"), Some("USD"));
        assert_eq!(equity_currency("RELIANCE.BSE"), Some("INR"));
        assert_eq!(equity_currency("TSCO.LON"), None);
    }

    #[tokio::test]
    async fn unreachable_provider_is_dropped_for_the_run() {
        // Nothing listens on the discard port: the first request fails fast
        // and the remaining symbols are not attempted.
        let p = Provider::new(
            ProviderKind::AlphaVantage,
            "k-1234".into(),
            Some("http://127.0.0.1:9".into()),
        );
        assert!(p.quotes(&["IBM", "MSFT", "AAPL"]).await.is_err());
        assert_eq!(p.remaining(), 0);
    }
}
