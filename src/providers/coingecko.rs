//! CoinGecko (<https://www.coingecko.com/en/api>): prices and daily history
//! for the large coins. A backup for crypto when Yahoo fails.
//!
//! The public API needs no key (about 5 calls a minute from a shared pool).
//! A free Demo key (`COINGECKO_API_KEY`, sent as `x-cg-demo-api-key`) gives
//! a steadier allowance; it is optional. History on the free plans reaches
//! back 365 days. CoinGecko asks free users to credit it, so output that
//! shows its data says "Powered by CoinGecko".

use super::{Auth, Bar, Http, ProviderError, Series, num};
use crate::api::Quote;
use anyhow::{Result, bail};
use chrono::NaiveDate;
use std::time::Duration;

pub const BASE_URL: &str = "https://api.coingecko.com/api/v3";

/// Ticker → CoinGecko coin id, for the coins large enough to be worth a
/// fixed list (ids are not tickers: many coins share a ticker).
const COINS: &[(&str, &str)] = &[
    ("BTC", "bitcoin"),
    ("ETH", "ethereum"),
    ("USDT", "tether"),
    ("XRP", "ripple"),
    ("BNB", "binancecoin"),
    ("SOL", "solana"),
    ("USDC", "usd-coin"),
    ("DOGE", "dogecoin"),
    ("TRX", "tron"),
    ("ADA", "cardano"),
    ("LINK", "chainlink"),
    ("AVAX", "avalanche-2"),
    ("XLM", "stellar"),
    ("TON", "the-open-network"),
    ("SHIB", "shiba-inu"),
    ("DOT", "polkadot"),
    ("LTC", "litecoin"),
    ("BCH", "bitcoin-cash"),
    ("UNI", "uniswap"),
    ("NEAR", "near"),
    ("ATOM", "cosmos"),
    ("ETC", "ethereum-classic"),
];

/// Quote currencies CoinGecko prices in that Yahoo crypto symbols use.
const VS: &[&str] = &["USD", "INR", "EUR", "GBP", "JPY", "BTC", "ETH"];

/// `(coin id, quote currency)` for a Yahoo crypto symbol: `BTC-USD` →
/// (`bitcoin`, `USD`).
pub fn target(symbol: &str) -> Option<(&'static str, &'static str)> {
    let upper = symbol.trim().to_ascii_uppercase();
    let (coin, vs) = upper.rsplit_once('-')?;
    let id = COINS.iter().find(|(t, _)| *t == coin)?.1;
    let vs = *VS.iter().find(|v| **v == vs)?;
    (coin != vs).then_some((id, vs))
}

/// Errors arrive as `{"status": {"error_code": 429, "error_message": ...}}`
/// or `{"error": "coin not found"}`.
fn check(json: &serde_json::Value) -> Result<(), ProviderError> {
    if let Some(code) = json["status"]["error_code"].as_u64() {
        let msg = json["status"]["error_message"]
            .as_str()
            .unwrap_or("error")
            .to_string();
        return Err(match code {
            429 => ProviderError::RateLimited(msg),
            401 | 403 | 10002 | 10010 | 10011 => ProviderError::Denied(msg),
            _ => ProviderError::Other(msg),
        });
    }
    match json["error"].as_str() {
        Some(msg) => Err(ProviderError::Other(msg.to_string())),
        None => Ok(()),
    }
}

pub struct CoinGecko {
    pub(super) http: Http,
}

impl CoinGecko {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    /// The optional Demo key goes in a header; without one, no auth at all.
    fn auth(&self) -> Auth {
        if self.http.key.is_empty() {
            Auth::None
        } else {
            Auth::Header("x-cg-demo-api-key", "")
        }
    }

    /// Prices for every symbol CoinGecko covers, in one request.
    pub async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let wanted: Vec<(&str, &str, &str)> = symbols
            .iter()
            .filter_map(|s| target(s).map(|(id, vs)| (*s, id, vs)))
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids: Vec<&str> = wanted.iter().map(|w| w.1).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut vs: Vec<String> = wanted.iter().map(|w| w.2.to_ascii_lowercase()).collect();
        vs.sort_unstable();
        vs.dedup();
        let (ids, vs) = (ids.join(","), vs.join(","));
        let json = self
            .http
            .get_json(
                "/simple/price",
                &[
                    ("ids", ids.as_str()),
                    ("vs_currencies", vs.as_str()),
                    ("include_24hr_change", "true"),
                    ("include_last_updated_at", "true"),
                ],
                self.auth(),
                Duration::from_secs(120),
                check,
            )
            .await?;
        Ok(wanted
            .into_iter()
            .filter_map(|(symbol, id, vs)| {
                let mut q = parse_price(&json[id], vs)?;
                q.symbol = Some(symbol.to_string());
                Some(q)
            })
            .collect())
    }

    /// Daily prices from `start` (at most 365 days back on the free plans).
    pub async fn daily(&self, symbol: &str, start: Option<NaiveDate>) -> Result<Series> {
        let Some((id, vs)) = target(symbol) else {
            bail!("CoinGecko does not cover {symbol}");
        };
        let today = chrono::Utc::now().date_naive();
        let days = start
            .map_or(365, |s| (today - s).num_days() + 1)
            .clamp(1, 365)
            .to_string();
        let vs_lower = vs.to_ascii_lowercase();
        let json = self
            .http
            .get_json(
                &format!("/coins/{id}/market_chart"),
                &[
                    ("vs_currency", vs_lower.as_str()),
                    ("days", days.as_str()),
                    ("interval", "daily"),
                ],
                self.auth(),
                Duration::from_secs(3600),
                check,
            )
            .await?;
        Ok(parse_market_chart(&json, vs))
    }
}

/// One coin's entry of `/simple/price`: `{"usd": 63944, "usd_24h_change":
/// -1.2, "last_updated_at": 1792...}`.
pub fn parse_price(coin: &serde_json::Value, vs: &str) -> Option<Quote> {
    let key = vs.to_ascii_lowercase();
    let price = num(&coin[&key]).filter(|p| *p > 0.0)?;
    let mut q = Quote {
        regular_market_price: Some(price),
        regular_market_time: coin["last_updated_at"].as_i64(),
        currency: Some(vs.to_string()),
        ..Default::default()
    };
    // The change over the last 24 hours: crypto has no daily close.
    if let Some(pct) = num(&coin[format!("{key}_24h_change")])
        && pct > -100.0
    {
        let prev = price / (1.0 + pct / 100.0);
        q.regular_market_previous_close = Some(prev);
        q.regular_market_change = Some(price - prev);
        q.regular_market_change_percent = Some(pct);
    }
    Some(q)
}

/// `{"prices": [[ms, price], ...]}` → one bar per UTC day, the day's last
/// price as the close. Volumes are left out: CoinGecko's are in money, not
/// coins.
pub fn parse_market_chart(json: &serde_json::Value, vs: &str) -> Series {
    let mut bars: Vec<Bar> = Vec::new();
    for point in json["prices"].as_array().into_iter().flatten() {
        let (Some(ms), Some(price)) = (point[0].as_i64(), num(&point[1])) else {
            continue;
        };
        let Some(date) = chrono::DateTime::from_timestamp_millis(ms).map(|t| t.date_naive()) else {
            continue;
        };
        if price <= 0.0 {
            continue;
        }
        match bars.last_mut() {
            Some(last) if last.date == date => last.close = Some(price),
            _ => bars.push(Bar {
                date,
                open: None,
                high: None,
                low: None,
                close: Some(price),
                volume: None,
            }),
        }
    }
    Series {
        bars,
        currency: Some(vs.to_string()),
    }
    .sorted()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{MarketData, Provider, ProviderKind};
    use crate::testutil::MockServer;

    const PRICE: &str = include_str!("fixtures/coingecko_simple_price.json");
    const CHART: &str = include_str!("fixtures/coingecko_market_chart.json");

    #[test]
    fn maps_large_coins_only() {
        assert_eq!(target("BTC-USD"), Some(("bitcoin", "USD")));
        assert_eq!(target("eth-inr"), Some(("ethereum", "INR")));
        assert_eq!(target("ETH-BTC"), Some(("ethereum", "BTC")));
        assert_eq!(target("BTC-BTC"), None);
        assert_eq!(target("PEPE-USD"), None, "not on the fixed list");
        assert_eq!(target("BTC-USDT"), None, "not a CoinGecko quote currency");
        assert_eq!(target("USDINR=X"), None);
        assert_eq!(target("AAPL"), None);
    }

    #[test]
    fn parses_price_with_24h_change() {
        let json: serde_json::Value = serde_json::from_str(PRICE).unwrap();
        let q = parse_price(&json["bitcoin"], "USD").unwrap();
        assert_eq!(q.regular_market_price, Some(64_000.0));
        assert_eq!(q.regular_market_change_percent, Some(-2.5));
        let prev = q.regular_market_previous_close.unwrap();
        assert!((prev - 64_000.0 / 0.975).abs() < 1e-6);
        assert_eq!(q.regular_market_time, Some(1_794_900_000));
        assert!(parse_price(&json["bitcoin"], "JPY").is_none());
    }

    #[test]
    fn one_bar_per_day_with_the_last_price() {
        let s = parse_market_chart(&serde_json::from_str(CHART).unwrap(), "USD");
        let dates: Vec<String> = s.bars.iter().map(|b| b.date.to_string()).collect();
        assert_eq!(
            dates,
            ["2026-11-14", "2026-11-15", "2026-11-16", "2026-11-17"]
        );
        // 2026-11-17 has a midnight point and a later one: the later wins.
        assert_eq!(s.bars[3].close, Some(64_000.0));
        assert_eq!(s.bars[0].volume, None);
    }

    #[test]
    fn classifies_errors() {
        let limit = serde_json::json!({"status": {"error_code": 429, "error_message": "You've exceeded the Rate Limit"}});
        assert!(matches!(check(&limit), Err(ProviderError::RateLimited(_))));
        let missing = serde_json::json!({"error": "coin not found"});
        assert!(matches!(check(&missing), Err(ProviderError::Other(_))));
        assert!(check(&serde_json::from_str(PRICE).unwrap()).is_ok());
    }

    #[tokio::test]
    async fn quotes_batch_into_one_keyless_request() {
        let server = MockServer::start(vec![(200, PRICE.into())]).await;
        let p = Provider::new(
            ProviderKind::CoinGecko,
            String::new(),
            Some(server.url.clone()),
        );
        let q = p.quotes(&["BTC-USD", "ETH-USD", "PEPE-USD"]).await.unwrap();
        let syms: Vec<_> = q.iter().filter_map(|q| q.symbol.as_deref()).collect();
        assert_eq!(syms, ["BTC-USD", "ETH-USD"]);
        assert_eq!(q[0].data_source.as_deref(), Some("CoinGecko"));
        let reqs = server.requests();
        assert_eq!(reqs.len(), 1);
        assert!(
            reqs[0]
                .target
                .starts_with("/simple/price?ids=bitcoin%2Cethereum&vs_currencies=usd")
        );
        assert_eq!(reqs[0].header("x-cg-demo-api-key"), None);
    }

    #[tokio::test]
    async fn a_demo_key_goes_in_its_header() {
        let server = MockServer::start(vec![(200, PRICE.into())]).await;
        let p = Provider::new(
            ProviderKind::CoinGecko,
            "cg-demo".into(),
            Some(server.url.clone()),
        );
        p.quotes(&["BTC-USD"]).await.unwrap();
        assert_eq!(
            server.requests()[0].header("x-cg-demo-api-key"),
            Some("cg-demo")
        );
    }
}
