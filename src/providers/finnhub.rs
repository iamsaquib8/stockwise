//! Finnhub (<https://finnhub.io>). The free plan allows 60 calls a minute;
//! its quotes cover US stocks. Capabilities here: quote, company profile
//! (market cap, industry), analyst recommendation trends and earnings
//! surprises.
//!
//! Built from Finnhub's documented response formats (`/quote`,
//! `/stock/profile2`, `/stock/recommendation`, `/stock/earnings`); not
//! checked against the live API. The key travels in the `X-Finnhub-Token`
//! header, never in a URL.
//!
//! `/quote` answers an unknown symbol with all zeros (`"c": 0, "t": 0`);
//! errors arrive as `{"error": "..."}` or HTTP 401/403/429.

use super::router::{Fundamentals, Profile, Recommendation};
use super::{Auth, Http, ProviderError, int, num};
use crate::api::Quote;
use crate::research::Surprise;
use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;
use std::time::Duration;

pub const BASE_URL: &str = "https://finnhub.io/api/v1";
const AUTH: Auth = Auth::Header("X-Finnhub-Token", "");

/// Finnhub's free quotes are US listings: a plain ticker, `-` written as `.`
/// (`BRK-B` → `BRK.B`). Suffixed symbols, indices, FX and crypto are left to
/// other sources.
pub fn ticker(symbol: &str) -> Option<String> {
    use crate::market::AssetClass;
    let upper = symbol.trim().to_ascii_uppercase();
    match AssetClass::of(&upper) {
        AssetClass::Stock if !upper.contains('.') && !upper.is_empty() => {
            Some(upper.replace('-', "."))
        }
        _ => None,
    }
}

fn check(json: &Value) -> Result<(), ProviderError> {
    match json["error"].as_str() {
        None => Ok(()),
        Some(msg) if msg.to_ascii_lowercase().contains("limit") => {
            Err(ProviderError::RateLimited(msg.to_string()))
        }
        Some(msg) if msg.to_ascii_lowercase().contains("access") => {
            Err(ProviderError::Denied(msg.to_string()))
        }
        Some(msg) => Err(ProviderError::Other(msg.to_string())),
    }
}

pub struct Finnhub {
    pub(super) http: Http,
}

impl Finnhub {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    pub async fn quote(&self, symbol: &str) -> Result<Option<Quote>> {
        let Some(t) = ticker(symbol) else {
            return Ok(None);
        };
        let json = self
            .http
            .get_json(
                "/quote",
                &[("symbol", &t)],
                AUTH,
                Duration::from_secs(30),
                check,
            )
            .await?;
        Ok(parse_quote(&json))
    }

    pub async fn profile(&self, symbol: &str) -> Result<Option<Profile>> {
        let Some(t) = ticker(symbol) else {
            return Ok(None);
        };
        let json = self
            .http
            .get_json(
                "/stock/profile2",
                &[("symbol", &t)],
                AUTH,
                Duration::from_secs(24 * 3600),
                check,
            )
            .await?;
        Ok(parse_profile(&json))
    }

    /// Profile, latest recommendation trend and earnings surprises: three
    /// requests, so the caller asks only when it needs all three.
    pub async fn fundamentals(&self, symbol: &str) -> Result<Option<Fundamentals>> {
        let Some(t) = ticker(symbol) else {
            return Ok(None);
        };
        let profile = self.profile(symbol).await?;
        let ttl = Duration::from_secs(6 * 3600);
        let rec = self
            .http
            .get_json("/stock/recommendation", &[("symbol", &t)], AUTH, ttl, check)
            .await?;
        let earn = self
            .http
            .get_json("/stock/earnings", &[("symbol", &t)], AUTH, ttl, check)
            .await?;
        Ok(Some(Fundamentals {
            profile,
            recommendation: parse_recommendation(&rec),
            surprises: parse_surprises(&earn),
        }))
    }
}

/// `/quote`: `c` price, `d` change, `dp` change percent, `h`/`l`/`o` day
/// high, low and open, `pc` previous close, `t` time (unix seconds). Zero
/// price and time mean Finnhub knows no such symbol.
pub fn parse_quote(json: &Value) -> Option<Quote> {
    let price = num(&json["c"]).filter(|p| *p > 0.0)?;
    Some(Quote {
        regular_market_price: Some(price),
        regular_market_time: json["t"].as_i64().filter(|t| *t > 0),
        regular_market_open: num(&json["o"]).filter(|v| *v > 0.0),
        regular_market_day_high: num(&json["h"]).filter(|v| *v > 0.0),
        regular_market_day_low: num(&json["l"]).filter(|v| *v > 0.0),
        regular_market_previous_close: num(&json["pc"]).filter(|v| *v > 0.0),
        regular_market_change: num(&json["d"]),
        regular_market_change_percent: num(&json["dp"]),
        ..Default::default()
    })
}

/// `/stock/profile2`: `marketCapitalization` is in millions of the listing
/// currency. An empty object means no such company.
pub fn parse_profile(json: &Value) -> Option<Profile> {
    let name = json["name"].as_str().map(str::to_string);
    let market_cap = num(&json["marketCapitalization"])
        .filter(|m| *m > 0.0)
        .map(|m| m * 1_000_000.0);
    if name.is_none() && market_cap.is_none() {
        return None;
    }
    Some(Profile {
        name,
        market_cap,
        currency: json["currency"].as_str().map(str::to_string),
        exchange: json["exchange"].as_str().map(str::to_string),
        sector: None,
        industry: json["finnhubIndustry"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        country: json["country"].as_str().map(str::to_string),
    })
}

/// `/stock/recommendation`: one row per month, newest first.
pub fn parse_recommendation(json: &Value) -> Option<Recommendation> {
    let latest = json
        .as_array()?
        .iter()
        .max_by_key(|r| r["period"].as_str().unwrap_or("").to_string())?;
    let count = |k: &str| int(&latest[k]).unwrap_or(0) as u32;
    let rec = Recommendation {
        period: latest["period"].as_str().unwrap_or("").to_string(),
        strong_buy: count("strongBuy"),
        buy: count("buy"),
        hold: count("hold"),
        sell: count("sell"),
        strong_sell: count("strongSell"),
    };
    (rec.total() > 0).then_some(rec)
}

/// `/stock/earnings`: `period` is the quarter end, `actual` and `estimate`
/// EPS, `surprisePercent` percent of the estimate (converted here to the
/// fraction `research::Surprise` carries, as Yahoo's is). Rows without both
/// EPS figures are skipped.
pub fn parse_surprises(json: &Value) -> Vec<Surprise> {
    json.as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let actual = num(&r["actual"])?;
            let estimate = num(&r["estimate"])?;
            Some(Surprise {
                quarter: NaiveDate::parse_from_str(r["period"].as_str()?, "%Y-%m-%d").ok()?,
                actual,
                estimate,
                surprise_pct: num(&r["surprisePercent"])
                    .map(|p| p / 100.0)
                    .unwrap_or_else(|| {
                        if estimate != 0.0 {
                            (actual - estimate) / estimate.abs()
                        } else {
                            0.0
                        }
                    }),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{MarketData, Provider, ProviderKind};
    use crate::testutil::MockServer;

    const QUOTE: &str = r#"{"c":261.74,"d":-1.26,"dp":-0.479,"h":263.31,"l":260.68,"o":261.07,"pc":263,"t":1700000000}"#;
    const PROFILE: &str = r#"{"country":"US","currency":"USD","exchange":"NASDAQ NMS - GLOBAL MARKET","finnhubIndustry":"Technology","name":"Apple Inc","marketCapitalization":2900000.5,"ticker":"AAPL"}"#;
    const RECOMMENDATION: &str = r#"[{"buy":24,"hold":7,"period":"2026-10-01","sell":1,"strongBuy":13,"strongSell":0,"symbol":"AAPL"},{"buy":20,"hold":9,"period":"2026-09-01","sell":2,"strongBuy":11,"strongSell":1,"symbol":"AAPL"}]"#;
    const EARNINGS: &str = r#"[{"actual":1.64,"estimate":1.6,"period":"2026-09-30","quarter":4,"surprise":0.04,"surprisePercent":2.5,"symbol":"AAPL","year":2026},{"actual":null,"estimate":1.5,"period":"2026-06-30","symbol":"AAPL"}]"#;

    #[test]
    fn maps_only_plain_us_tickers() {
        assert_eq!(ticker("aapl"), Some("AAPL".into()));
        assert_eq!(ticker("BRK-B"), Some("BRK.B".into()));
        assert_eq!(ticker("TCS.NS"), None);
        assert_eq!(ticker("^GSPC"), None);
        assert_eq!(ticker("BTC-USD"), None);
        assert_eq!(ticker("USDINR=X"), None);
    }

    #[test]
    fn parses_quote_and_rejects_unknown_symbol_zeros() {
        let q = parse_quote(&serde_json::from_str(QUOTE).unwrap()).unwrap();
        assert_eq!(q.regular_market_price, Some(261.74));
        assert_eq!(q.regular_market_previous_close, Some(263.0));
        assert_eq!(q.regular_market_time, Some(1_700_000_000));
        assert_eq!(q.regular_market_change_percent, Some(-0.479));
        let none = serde_json::json!({"c":0,"d":null,"dp":null,"h":0,"l":0,"o":0,"pc":0,"t":0});
        assert!(parse_quote(&none).is_none());
    }

    #[test]
    fn profile_market_cap_is_millions_converted_to_units() {
        let p = parse_profile(&serde_json::from_str(PROFILE).unwrap()).unwrap();
        assert_eq!(p.market_cap, Some(2_900_000_500_000.0));
        assert_eq!(p.industry.as_deref(), Some("Technology"));
        assert_eq!(p.currency.as_deref(), Some("USD"));
        assert!(parse_profile(&serde_json::json!({})).is_none());
    }

    #[test]
    fn recommendation_takes_newest_month_and_scores_it() {
        let r = parse_recommendation(&serde_json::from_str(RECOMMENDATION).unwrap()).unwrap();
        assert_eq!(r.period, "2026-10-01");
        assert_eq!(r.total(), 45);
        let mean = r.mean().unwrap();
        assert!((mean - (13.0 + 48.0 + 21.0 + 4.0) / 45.0).abs() < 1e-9);
        assert_eq!(r.key(), Some("buy"));
    }

    #[test]
    fn surprises_skip_rows_without_both_figures() {
        let s = parse_surprises(&serde_json::from_str(EARNINGS).unwrap());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].quarter.to_string(), "2026-09-30");
        assert!(
            (s[0].surprise_pct - 0.025).abs() < 1e-12,
            "a fraction, like Yahoo's"
        );
    }

    #[test]
    fn errors_are_classified() {
        assert!(matches!(
            check(&serde_json::json!({"error": "API limit reached. Please try again later."})),
            Err(ProviderError::RateLimited(_))
        ));
        assert!(matches!(
            check(&serde_json::json!({"error": "You don't have access to this resource."})),
            Err(ProviderError::Denied(_))
        ));
        assert!(check(&serde_json::from_str(QUOTE).unwrap()).is_ok());
    }

    #[tokio::test]
    async fn key_goes_in_a_header_never_the_url() {
        let server = MockServer::start(vec![(200, QUOTE.into())]).await;
        let p = Provider::new(
            ProviderKind::Finnhub,
            "fh-secret".into(),
            Some(server.url.clone()),
        );
        let q = p.quotes(&["AAPL"]).await.unwrap();
        assert_eq!(q[0].symbol.as_deref(), Some("AAPL"));
        assert_eq!(q[0].data_source.as_deref(), Some("Finnhub"));
        let req = &server.requests()[0];
        assert_eq!(req.target, "/quote?symbol=AAPL");
        assert_eq!(req.header("x-finnhub-token"), Some("fh-secret"));
        assert!(!req.target.contains("fh-secret"));
    }

    #[tokio::test]
    async fn free_tier_window_is_sixty_a_minute() {
        let server = MockServer::start(vec![(200, QUOTE.into())]).await;
        let p = Provider::new(ProviderKind::Finnhub, "k".into(), Some(server.url.clone()));
        assert_eq!(p.remaining(), 60);
        p.quotes(&["AAPL"]).await.unwrap();
        assert_eq!(p.remaining(), 59);
    }
}
