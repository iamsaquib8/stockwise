//! Capability-based routing over the configured providers.
//!
//! Each provider declares what it can do ([`Capability`]): quotes, charts,
//! fundamentals (profile, analyst trend, earnings surprises), statements and
//! screeners. The router picks, per capability, the best configured provider
//! that covers the symbol and still has free-tier calls left in its window
//! (`Provider::can_serve`), falls through to the next one when it fails, and
//! merges fields: when two providers each supply part of a [`Quote`], the
//! first to answer keeps its fields and the others fill only what is empty.
//! `meta.sources` on the quote names every provider that contributed.
//!
//! Yahoo Finance stays the base of `YahooClient`; the router is what answers
//! when Yahoo fails or leaves holes. Keyed providers join the router when
//! their key variable is set (`FINNHUB_API_KEY`, `FMP_API_KEY`).

use super::{MarketData, Provider, ProviderKind};
use crate::api::{Quote, QuoteMeta};
use crate::research::Surprise;
use crate::results::Statements;
use anyhow::{Result, bail};

/// What a provider can be asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Quote,
    Chart,
    Search,
    /// Company profile (market cap, industry), analyst trend, earnings
    /// surprises.
    Fundamentals,
    /// Quarterly and annual income, balance-sheet and cash-flow statements.
    Statements,
    Screener,
}

/// Most symbols a quote request fills holes for (one extra request each).
/// Bigger tables stay as the first source gave them.
pub const FILL_MAX: usize = 10;
/// Past this many symbols, a batch endpoint beats one request per symbol.
const BATCH_PREFERRED: usize = 5;

impl ProviderKind {
    /// What this provider offers.
    pub fn capabilities(self) -> &'static [Capability] {
        use Capability::*;
        match self {
            ProviderKind::Finnhub => &[Quote, Fundamentals],
            ProviderKind::Fmp => &[Quote, Fundamentals, Statements, Screener],
            ProviderKind::Nse | ProviderKind::Bse | ProviderKind::Exchange => {
                &[Quote, Chart, Search]
            }
            ProviderKind::AlphaVantage | ProviderKind::Massive | ProviderKind::TwelveData => {
                &[Quote, Chart, Search]
            }
        }
    }

    /// Whether the provider's quote includes the market cap (Finnhub's
    /// does not: its profile does).
    fn quote_carries_market_cap(self) -> bool {
        self == ProviderKind::Fmp
    }

    /// Order among providers for one capability: lower goes first. Finnhub
    /// answers single US symbols fast (60 a minute); FMP is the one request
    /// for a long list and the only statements and screener source.
    fn rank(self, cap: Capability, batch: usize) -> u8 {
        match (self, cap) {
            (ProviderKind::Finnhub, Capability::Quote) if batch > BATCH_PREFERRED => 2,
            (ProviderKind::Finnhub, _) => 0,
            (ProviderKind::Fmp, Capability::Quote) if batch > BATCH_PREFERRED => 0,
            (ProviderKind::Fmp, _) => 1,
            _ => 5,
        }
    }
}

/// A company profile. Money is in the listing currency, in whole units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub name: Option<String>,
    pub market_cap: Option<f64>,
    pub currency: Option<String>,
    pub exchange: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub country: Option<String>,
}

impl Profile {
    /// Fill the quote's empty fields; whether any changed.
    pub fn fill(&self, q: &mut Quote) -> bool {
        let mut changed = false;
        macro_rules! set {
            ($field:ident, $value:expr) => {
                if q.$field.is_none() && $value.is_some() {
                    q.$field = $value.clone();
                    changed = true;
                }
            };
        }
        set!(market_cap, self.market_cap);
        set!(currency, self.currency);
        set!(exchange, self.exchange);
        set!(sector, self.sector);
        set!(industry, self.industry);
        if q.short_name.is_none() && q.long_name.is_none() && self.name.is_some() {
            q.short_name = self.name.clone();
            changed = true;
        }
        changed
    }
}

/// Analyst counts for one month.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recommendation {
    pub period: String,
    pub strong_buy: u32,
    pub buy: u32,
    pub hold: u32,
    pub sell: u32,
    pub strong_sell: u32,
}

impl Recommendation {
    pub fn total(&self) -> u32 {
        self.strong_buy + self.buy + self.hold + self.sell + self.strong_sell
    }

    /// Mean rating on Yahoo's scale: 1 strong buy … 5 strong sell.
    pub fn mean(&self) -> Option<f64> {
        let n = self.total();
        (n > 0).then(|| {
            (self.strong_buy as f64
                + 2.0 * self.buy as f64
                + 3.0 * self.hold as f64
                + 4.0 * self.sell as f64
                + 5.0 * self.strong_sell as f64)
                / n as f64
        })
    }

    /// Yahoo's `recommendationKey` for the mean.
    pub fn key(&self) -> Option<&'static str> {
        Some(match self.mean()? {
            m if m < 1.5 => "strong_buy",
            m if m < 2.5 => "buy",
            m if m < 3.5 => "hold",
            m if m < 4.5 => "underperform",
            _ => "sell",
        })
    }
}

/// Profile, analyst trend and earnings surprises for one company.
#[derive(Clone, Debug, Default)]
pub struct Fundamentals {
    pub profile: Option<Profile>,
    pub recommendation: Option<Recommendation>,
    pub surprises: Vec<Surprise>,
}

impl Fundamentals {
    /// Fill the quote's empty profile and analyst fields; whether any changed.
    pub fn fill(&self, q: &mut Quote) -> bool {
        let mut changed = self.profile.as_ref().is_some_and(|p| p.fill(q));
        if let Some(r) = &self.recommendation
            && q.recommendation_mean.is_none()
        {
            q.recommendation_mean = r.mean();
            q.recommendation_key = r.key().map(str::to_string);
            q.number_of_analyst_opinions = Some(r.total());
            changed = true;
        }
        changed
    }
}

/// What a screener is asked for. Market caps are in the listing currency.
#[derive(Clone, Debug, Default)]
pub struct ScreenQuery {
    pub market_cap_min: Option<f64>,
    pub market_cap_max: Option<f64>,
    /// Exchange code (`NASDAQ`, `NYSE`, `NSE`, ...).
    pub exchange: Option<String>,
    pub sector: Option<String>,
    /// Two-letter country code.
    pub country: Option<String>,
    pub limit: usize,
}

/// Fill every empty field of `base` from `other`; whether any changed.
pub fn fill_quote(base: &mut Quote, other: &Quote) -> bool {
    let mut changed = false;
    macro_rules! fill {
        ($($field:ident),* $(,)?) => {
            $(if base.$field.is_none() && other.$field.is_some() {
                base.$field = other.$field.clone();
                changed = true;
            })*
        };
    }
    fill!(
        short_name,
        long_name,
        regular_market_price,
        regular_market_change,
        regular_market_change_percent,
        regular_market_volume,
        regular_market_open,
        regular_market_day_high,
        regular_market_day_low,
        regular_market_previous_close,
        regular_market_time,
        fifty_day_average,
        two_hundred_day_average,
        fifty_two_week_high,
        fifty_two_week_low,
        market_cap,
        trailing_pe,
        forward_pe,
        trailing_annual_dividend_yield,
        eps_trailing_twelve_months,
        eps_forward,
        price_to_book,
        book_value,
        exchange,
        currency,
        average_daily_volume_3_month,
        sector,
        industry,
        target_mean_price,
        recommendation_mean,
        recommendation_key,
        number_of_analyst_opinions,
        beta,
        dividend_yield,
    );
    changed
}

/// A quote missing a field a backup quote can supply: the market cap that
/// decides the cap tier, or the previous close the change is measured from.
fn has_gap(q: &Quote) -> bool {
    q.market_cap.is_none() || q.regular_market_previous_close.is_none()
}

fn same(q: &Quote, symbol: &str) -> bool {
    q.symbol
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case(symbol.trim()))
}

/// Add `name` to the quote's `meta.sources`, after `base` when it is the
/// first entry.
fn note_source(q: &mut Quote, base: Option<&str>, name: &str) {
    let meta = q.meta.get_or_insert_with(QuoteMeta::default);
    if meta.sources.is_empty()
        && let Some(base) = base
    {
        meta.sources.push(base.to_string());
    }
    if !meta.sources.iter().any(|s| s == name) {
        meta.sources.push(name.to_string());
    }
}

/// The keyed providers that came with their key variable set.
#[derive(Default)]
pub struct Router {
    members: Vec<Provider>,
}

impl Router {
    /// Join every keyed provider whose key variable is set, except `skip`
    /// (the one `STOCKWISE_DATA_PROVIDER` already made the primary).
    pub fn from_env(skip: Option<ProviderKind>, cache: Option<&crate::api::SharedCache>) -> Self {
        Self::from_lookup(skip, cache, |k| std::env::var(k).ok())
    }

    fn from_lookup(
        skip: Option<ProviderKind>,
        cache: Option<&crate::api::SharedCache>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let members = [ProviderKind::Finnhub, ProviderKind::Fmp]
            .into_iter()
            .filter(|k| Some(*k) != skip)
            .filter_map(|kind| {
                let key = kind
                    .key_envs()
                    .iter()
                    .filter_map(|k| env(k))
                    .map(|k| k.trim().to_string())
                    .find(|k| !k.is_empty())?;
                let provider = Provider::new(kind, key, None);
                Some(match cache {
                    Some(c) => provider.with_cache(c.clone()),
                    None => provider,
                })
            })
            .collect();
        Self { members }
    }

    pub fn new(members: Vec<Provider>) -> Self {
        Self { members }
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn members(&self) -> &[Provider] {
        &self.members
    }

    /// Providers offering `cap`, best first for a request of `batch` symbols.
    pub fn serving(&self, cap: Capability, batch: usize) -> Vec<&Provider> {
        let mut list: Vec<&Provider> = self
            .members
            .iter()
            .filter(|p| p.kind().capabilities().contains(&cap))
            .collect();
        list.sort_by_key(|p| p.kind().rank(cap, batch));
        list
    }

    /// Quotes for `symbols`, in request order: `have` (what Yahoo gave,
    /// named `base`) plus whatever the router adds. A symbol without a usable
    /// quote goes to the best provider that covers it and has calls left;
    /// then, for a short request, a quote still missing its market cap or previous close is
    /// filled by the other providers' quotes and profiles. Quotes a provider
    /// touched carry `meta.sources`.
    pub async fn complete(&self, symbols: &[&str], have: Vec<Quote>, base: &str) -> Vec<Quote> {
        if self.members.is_empty() {
            return have;
        }
        let mut have = have;
        let mut slots: Vec<Option<Quote>> = symbols
            .iter()
            .map(|s| {
                have.iter()
                    .position(|q| same(q, s) && q.is_usable())
                    .map(|i| have.remove(i))
            })
            .collect();
        let mut tried: Vec<Vec<ProviderKind>> = vec![Vec::new(); symbols.len()];

        // 1. Symbols with no usable quote: first provider that answers.
        for p in self.serving(Capability::Quote, symbols.len()) {
            let ask: Vec<&str> = (0..symbols.len())
                .filter(|&i| slots[i].is_none() && p.supports(symbols[i]))
                .map(|i| symbols[i])
                .collect();
            if ask.is_empty() || !p.can_serve(&ask) {
                continue;
            }
            for s in &ask {
                let i = symbols.iter().position(|x| x == s).unwrap_or(0);
                tried[i].push(p.kind());
            }
            let Ok(got) = p.quotes(&ask).await else {
                continue;
            };
            for mut q in got {
                if !q.is_usable() {
                    continue;
                }
                if let Some(i) =
                    (0..symbols.len()).find(|&i| slots[i].is_none() && same(&q, symbols[i]))
                {
                    q.symbol = Some(symbols[i].to_string());
                    note_source(&mut q, None, p.name());
                    slots[i] = Some(q);
                }
            }
        }

        // 2. Holes in a short request: market cap decides the cap tier.
        if symbols.len() <= FILL_MAX {
            for p in self.serving(Capability::Quote, symbols.len()) {
                let ask: Vec<&str> = (0..symbols.len())
                    .filter(|&i| {
                        slots[i].as_ref().is_some_and(has_gap)
                            && !tried[i].contains(&p.kind())
                            && p.kind().quote_carries_market_cap()
                            && p.supports(symbols[i])
                    })
                    .map(|i| symbols[i])
                    .collect();
                if ask.is_empty() || !p.can_serve(&ask) {
                    continue;
                }
                let Ok(got) = p.quotes(&ask).await else {
                    continue;
                };
                for other in got {
                    if let Some(i) = (0..symbols.len()).find(|&i| same(&other, symbols[i]))
                        && let Some(q) = slots[i].as_mut()
                        && fill_quote(q, &other)
                    {
                        let from_yahoo = (!q.meta.as_ref().is_some_and(|m| !m.sources.is_empty()))
                            .then_some(base);
                        note_source(q, from_yahoo, p.name());
                        tried[i].push(p.kind());
                    }
                }
            }
            for p in self.serving(Capability::Fundamentals, symbols.len()) {
                for i in 0..symbols.len() {
                    let Some(q) = slots[i].as_mut() else { continue };
                    if q.market_cap.is_some() || !p.supports(symbols[i]) || p.remaining() == 0 {
                        continue;
                    }
                    if let Ok(Some(profile)) = p.profile(symbols[i]).await
                        && profile.fill(q)
                    {
                        let from_yahoo = (!q.meta.as_ref().is_some_and(|m| !m.sources.is_empty()))
                            .then_some(base);
                        note_source(q, from_yahoo, p.name());
                    }
                }
            }
        }

        // A quote Yahoo had no price for must not repeat one that is now filled.
        have.retain(|h| {
            !symbols
                .iter()
                .enumerate()
                .any(|(i, s)| same(h, s) && slots[i].is_some())
        });
        slots.into_iter().flatten().chain(have).collect()
    }

    /// Statements from the first provider that has them.
    pub async fn statements(&self, symbol: &str) -> Result<(Statements, &'static str)> {
        let mut last = None;
        for p in self.serving(Capability::Statements, 1) {
            if !p.supports(symbol) {
                continue;
            }
            match p.statements(symbol).await {
                Ok(s) if !s.quarterly.is_empty() => return Ok((s, p.name())),
                Ok(_) => {
                    last = Some(anyhow::anyhow!(
                        "{} has no statements for {symbol}",
                        p.name()
                    ))
                }
                Err(e) => last = Some(e),
            }
        }
        match last {
            Some(e) => Err(e),
            None => bail!("No configured provider serves statements (set FMP_API_KEY)"),
        }
    }

    /// Profile, analyst trend and surprises, merged field by field across
    /// providers (the first to supply a part keeps it). Returns the sources.
    pub async fn fundamentals(&self, symbol: &str) -> Result<(Fundamentals, Vec<&'static str>)> {
        let mut merged = Fundamentals::default();
        let mut sources = Vec::new();
        let mut last = None;
        for p in self.serving(Capability::Fundamentals, 1) {
            if !p.supports(symbol) {
                continue;
            }
            match p.fundamentals(symbol).await {
                Ok(Some(f)) => {
                    let mut used = false;
                    if merged.profile.is_none() && f.profile.is_some() {
                        merged.profile = f.profile;
                        used = true;
                    }
                    if merged.recommendation.is_none() && f.recommendation.is_some() {
                        merged.recommendation = f.recommendation;
                        used = true;
                    }
                    if merged.surprises.is_empty() && !f.surprises.is_empty() {
                        merged.surprises = f.surprises;
                        used = true;
                    }
                    if used {
                        sources.push(p.name());
                    }
                }
                Ok(None) => {}
                Err(e) => last = Some(e),
            }
        }
        if sources.is_empty() {
            return Err(last.unwrap_or_else(|| {
                anyhow::anyhow!("No configured provider has fundamentals for {symbol}")
            }));
        }
        Ok((merged, sources))
    }

    /// Screener rows from the first provider that offers one.
    pub async fn screener(&self, query: &ScreenQuery) -> Result<(Vec<Quote>, &'static str)> {
        let mut last = None;
        for p in self.serving(Capability::Screener, 1) {
            match p.screener(query).await {
                Ok(rows) if !rows.is_empty() => return Ok((rows, p.name())),
                Ok(_) => {}
                Err(e) => last = Some(e),
            }
        }
        match last {
            Some(e) => Err(e),
            None => bail!("No configured provider offers a screener (set FMP_API_KEY)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockServer;

    fn provider(kind: ProviderKind, base: &str) -> Provider {
        Provider::new(kind, "test-key".into(), Some(base.to_string()))
    }

    fn q(sym: &str, price: Option<f64>) -> Quote {
        Quote {
            symbol: Some(sym.into()),
            regular_market_price: price,
            ..Default::default()
        }
    }

    #[test]
    fn capabilities_say_who_does_what() {
        assert!(
            ProviderKind::Fmp
                .capabilities()
                .contains(&Capability::Statements)
        );
        assert!(
            ProviderKind::Fmp
                .capabilities()
                .contains(&Capability::Screener)
        );
        assert!(
            !ProviderKind::Finnhub
                .capabilities()
                .contains(&Capability::Statements)
        );
        assert!(
            ProviderKind::Finnhub
                .capabilities()
                .contains(&Capability::Fundamentals)
        );
    }

    #[test]
    fn ranking_prefers_finnhub_for_one_symbol_and_fmp_for_a_list() {
        let r = Router::from_lookup(None, None, |k| {
            matches!(k, "FINNHUB_API_KEY" | "FMP_API_KEY").then(|| "k".to_string())
        });
        let names = |batch| {
            r.serving(Capability::Quote, batch)
                .iter()
                .map(|p| p.kind())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(1), [ProviderKind::Finnhub, ProviderKind::Fmp]);
        assert_eq!(names(30), [ProviderKind::Fmp, ProviderKind::Finnhub]);
        assert_eq!(
            r.serving(Capability::Statements, 1).len(),
            1,
            "only FMP has statements"
        );
    }

    #[test]
    fn keys_decide_membership_and_the_primary_is_skipped() {
        let only_fmp =
            Router::from_lookup(None, None, |k| (k == "FMP_API_KEY").then(|| "k".into()));
        assert_eq!(only_fmp.members().len(), 1);
        let blank = Router::from_lookup(None, None, |_| Some("  ".into()));
        assert!(blank.is_empty());
        let skip = Router::from_lookup(Some(ProviderKind::Fmp), None, |_| Some("k".into()));
        assert_eq!(skip.members().len(), 1);
        assert_eq!(skip.members()[0].kind(), ProviderKind::Finnhub);
    }

    #[test]
    fn fill_keeps_the_first_value_and_supplies_the_rest() {
        let mut base = Quote {
            regular_market_price: Some(100.0),
            ..Default::default()
        };
        let other = Quote {
            regular_market_price: Some(101.0),
            market_cap: Some(5e9),
            sector: Some("Technology".into()),
            ..Default::default()
        };
        assert!(fill_quote(&mut base, &other));
        assert_eq!(base.regular_market_price, Some(100.0));
        assert_eq!(base.market_cap, Some(5e9));
        assert!(!fill_quote(&mut base, &other), "nothing left to fill");
    }

    #[tokio::test]
    async fn yahoo_failure_falls_through_to_the_router() {
        let fmp = MockServer::start(vec![(
            200,
            r#"[{"symbol":"AAPL","name":"Apple Inc.","price":261.74,"marketCap":3.9e12}]"#.into(),
        )])
        .await;
        let router = Router::new(vec![provider(ProviderKind::Fmp, &fmp.url)]);
        let out = router
            .complete(&["AAPL"], Vec::new(), "Yahoo Finance")
            .await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].regular_market_price, Some(261.74));
        assert_eq!(
            out[0].meta.as_ref().unwrap().sources,
            ["Financial Modeling Prep"]
        );
    }

    #[tokio::test]
    async fn two_providers_each_supply_part_of_one_quote_with_provenance() {
        // Finnhub: price only. FMP: market cap. Order for one symbol: Finnhub first.
        let finnhub = MockServer::start(vec![(
            200,
            r#"{"c":261.74,"d":-1.26,"dp":-0.479,"h":263.3,"l":260.6,"o":261.0,"pc":263,"t":1700000000}"#.into(),
        )])
        .await;
        let fmp = MockServer::start(vec![(
            200,
            r#"[{"symbol":"AAPL","name":"Apple Inc.","price":262.5,"marketCap":3.9e12,"exchange":"NASDAQ"}]"#.into(),
        )])
        .await;
        let router = Router::new(vec![
            provider(ProviderKind::Fmp, &fmp.url),
            provider(ProviderKind::Finnhub, &finnhub.url),
        ]);
        let out = router
            .complete(&["AAPL"], Vec::new(), "Yahoo Finance")
            .await;
        let quote = &out[0];
        assert_eq!(
            quote.regular_market_price,
            Some(261.74),
            "first provider's price kept"
        );
        assert_eq!(
            quote.market_cap,
            Some(3.9e12),
            "second provider fills the cap"
        );
        assert_eq!(quote.short_name.as_deref(), Some("Apple Inc."));
        assert_eq!(
            quote.meta.as_ref().unwrap().sources,
            ["Finnhub", "Financial Modeling Prep"]
        );
    }

    #[tokio::test]
    async fn a_profile_fills_a_missing_market_cap_on_a_yahoo_quote() {
        let finnhub = MockServer::start_routed(vec![(
            "/stock/profile2",
            r#"{"currency":"USD","finnhubIndustry":"Technology","name":"Apple Inc","marketCapitalization":3900000}"#.into(),
        )])
        .await;
        let router = Router::new(vec![provider(ProviderKind::Finnhub, &finnhub.url)]);
        let out = router
            .complete(&["AAPL"], vec![q("AAPL", Some(260.0))], "Yahoo Finance")
            .await;
        assert_eq!(out[0].regular_market_price, Some(260.0));
        assert_eq!(out[0].market_cap, Some(3.9e12));
        assert_eq!(
            out[0].meta.as_ref().unwrap().sources,
            ["Yahoo Finance", "Finnhub"]
        );
    }

    #[tokio::test]
    async fn a_missing_previous_close_is_filled_though_the_market_cap_is_there() {
        let fmp = MockServer::start(vec![(
            200,
            r#"[{"symbol":"AAPL","price":261.0,"previousClose":263,"marketCap":3.9e12}]"#.into(),
        )])
        .await;
        let router = Router::new(vec![provider(ProviderKind::Fmp, &fmp.url)]);
        let mut yahoo = q("AAPL", Some(260.0));
        yahoo.market_cap = Some(3.8e12);
        let out = router
            .complete(&["AAPL"], vec![yahoo], "Yahoo Finance")
            .await;
        assert_eq!(out[0].regular_market_price, Some(260.0));
        assert_eq!(out[0].market_cap, Some(3.8e12));
        assert_eq!(out[0].regular_market_previous_close, Some(263.0));
        assert_eq!(
            out[0].meta.as_ref().unwrap().sources,
            ["Yahoo Finance", "Financial Modeling Prep"]
        );
    }

    #[tokio::test]
    async fn complete_quotes_cost_no_requests_and_carry_no_meta() {
        let finnhub = MockServer::start(vec![(200, "{}".into())]).await;
        let router = Router::new(vec![provider(ProviderKind::Finnhub, &finnhub.url)]);
        let mut full = q("AAPL", Some(260.0));
        full.market_cap = Some(1e12);
        full.regular_market_previous_close = Some(259.0);
        let out = router
            .complete(&["AAPL"], vec![full], "Yahoo Finance")
            .await;
        assert!(out[0].meta.is_none());
        assert!(finnhub.requests().is_empty());
    }

    #[tokio::test]
    async fn a_provider_without_calls_left_is_skipped_for_the_next() {
        // Finnhub's window is spent by 60 calls; FMP answers instead.
        let finnhub = MockServer::start(vec![(200, r#"{"c":1,"t":1}"#.into())]).await;
        let fmp = MockServer::start(vec![(
            200,
            r#"[{"symbol":"AAPL","price":262.5,"marketCap":3.9e12}]"#.into(),
        )])
        .await;
        let fh = provider(ProviderKind::Finnhub, &finnhub.url);
        for _ in 0..60 {
            let _ = fh.quotes(&["MSFT"]).await;
        }
        assert_eq!(fh.remaining(), 0);
        let router = Router::new(vec![fh, provider(ProviderKind::Fmp, &fmp.url)]);
        let out = router
            .complete(&["AAPL"], Vec::new(), "Yahoo Finance")
            .await;
        assert_eq!(out[0].regular_market_price, Some(262.5));
        assert_eq!(finnhub.requests().len(), 60, "no 61st request");
    }

    #[tokio::test]
    async fn statements_come_from_the_provider_that_has_them() {
        let fmp = MockServer::start_routed(vec![
            (
                "/income-statement",
                r#"[{"date":"2025-09-27","period":"Q4","reportedCurrency":"USD","revenue":100,"netIncome":20}]"#.into(),
            ),
            ("/balance-sheet-statement", "[]".into()),
            ("/cash-flow-statement", "[]".into()),
        ])
        .await;
        let router = Router::new(vec![provider(ProviderKind::Fmp, &fmp.url)]);
        let (s, source) = router.statements("AAPL").await.unwrap();
        assert_eq!(source, "Financial Modeling Prep");
        assert_eq!(s.quarterly.len(), 1);
        assert_eq!(s.currency.as_deref(), Some("USD"));
        let none = Router::new(vec![]);
        assert!(none.statements("AAPL").await.is_err());
    }

    #[tokio::test]
    async fn fundamentals_merge_profile_trend_and_surprises() {
        let finnhub = MockServer::start_routed(vec![
            ("/stock/profile2", r#"{"name":"Apple Inc","marketCapitalization":3900000,"currency":"USD"}"#.into()),
            ("/stock/recommendation", r#"[{"period":"2026-10-01","strongBuy":10,"buy":10,"hold":5,"sell":0,"strongSell":0}]"#.into()),
            ("/stock/earnings", r#"[{"actual":1.6,"estimate":1.5,"period":"2026-09-30","surprisePercent":6.67}]"#.into()),
        ])
        .await;
        let router = Router::new(vec![provider(ProviderKind::Finnhub, &finnhub.url)]);
        let (f, sources) = router.fundamentals("AAPL").await.unwrap();
        assert_eq!(sources, ["Finnhub"]);
        assert_eq!(f.surprises.len(), 1);
        let mut quote = q("AAPL", Some(1.0));
        assert!(f.fill(&mut quote));
        assert_eq!(quote.market_cap, Some(3.9e12));
        assert_eq!(quote.number_of_analyst_opinions, Some(25));
        assert_eq!(quote.recommendation_key.as_deref(), Some("buy"));
    }

    #[tokio::test]
    async fn screener_rows_come_from_fmp() {
        let fmp = MockServer::start(vec![(
            200,
            r#"[{"symbol":"MSFT","companyName":"Microsoft","marketCap":3.5e12,"price":470.1}]"#
                .into(),
        )])
        .await;
        let router = Router::new(vec![provider(ProviderKind::Fmp, &fmp.url)]);
        let query = ScreenQuery {
            market_cap_min: Some(1e11),
            exchange: Some("NASDAQ".into()),
            limit: 50,
            ..Default::default()
        };
        let (rows, source) = router.screener(&query).await.unwrap();
        assert_eq!(rows[0].symbol.as_deref(), Some("MSFT"));
        assert_eq!(source, "Financial Modeling Prep");
        let target = &fmp.requests()[0].target;
        assert!(target.contains("marketCapMoreThan=100000000000"));
        assert!(target.contains("exchange=NASDAQ"));
    }
}
