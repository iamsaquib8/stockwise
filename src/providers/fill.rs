//! Field-by-field completion of a stock's fundamentals after Yahoo.
//!
//! Yahoo's v7 quote carries valuation (P/E, P/B, EPS, market cap) but none of
//! ROE, margins, debt/equity, beta, revenue growth or sector; those come from
//! `quoteSummary`, which fails or comes back partial often enough to matter.
//! [`complete_fundamentals`] fills only the fields still empty, from:
//!
//! - **US stocks:** SEC EDGAR company facts (ROE, D/E, margin, revenue
//!   growth, EPS), then Finnhub and FMP when their keys are set (also beta,
//!   P/E, P/B, sector and market cap).
//! - **Indian stocks:** NSE `quote-equity` (P/E, sector P/E, industry,
//!   market cap), NSE quarterly results (EPS, margin, revenue growth), then
//!   BSE's scrip header (P/E, EPS, ROE, P/B).
//!
//! Every field a fallback supplied is recorded in `meta.field_sources`, and
//! every source that failed in `meta.warnings`. Nothing is defaulted: a field
//! no source has stays empty and is listed in `meta.missing`.

use crate::api::{Quote, YahooClient};
use crate::provenance::{self, Meta};

/// Whether `symbol` is an Indian listing.
fn is_india(symbol: &str, q: &Quote) -> bool {
    symbol.ends_with(".NS")
        || symbol.ends_with(".BO")
        || crate::market::is_inr(q.currency.as_deref())
}

fn missing_any(q: &Quote) -> bool {
    provenance::stock_field_checks(q).iter().any(|(_, ok)| !ok)
}

/// Set `slot` from `value` when it is empty; record the source.
fn put(slot: &mut Option<f64>, value: Option<f64>, field: &str, source: &str, meta: &mut Meta) {
    if slot.is_none()
        && let Some(v) = value.filter(|v| v.is_finite())
    {
        *slot = Some(v);
        meta.record(field, source);
    }
}

/// Fill the empty fundamentals of `q` from the fallbacks for its market and
/// bring `meta` up to date (sources, field sources, warnings, missing,
/// coverage). Only stocks are completed; other instruments just get their
/// meta.
pub async fn complete_fundamentals(client: &YahooClient, q: &mut Quote, meta: &mut Meta) {
    for s in provenance::quote_sources(q) {
        meta.add_source(&s);
    }
    if let Some(ts) = q.regular_market_time {
        meta.set_as_of_unix(ts);
    }
    let symbol = q.symbol.clone().unwrap_or_default();
    if crate::market::AssetClass::of(&symbol) != crate::market::AssetClass::Stock {
        meta.set_fields(&provenance::quote_field_checks(q));
        return;
    }
    if missing_any(q) {
        if is_india(&symbol, q) {
            fill_india(client, &symbol, q, meta).await;
        } else {
            fill_us(client, &symbol, q, meta).await;
        }
        if missing_any(q) && client.has_router() {
            fill_keyed(client, &symbol, q, meta).await;
        }
    }
    meta.set_fields(&provenance::stock_field_checks(q));
}

async fn fill_us(client: &YahooClient, symbol: &str, q: &mut Quote, meta: &mut Meta) {
    use provenance::*;
    if super::edgar::ticker(symbol).is_none() {
        return;
    }
    match super::edgar::fetch(client, symbol).await {
        Ok(Some(st)) => {
            let r = st.ratios();
            put(&mut q.return_on_equity, r.roe, F_ROE, EDGAR, meta);
            put(&mut q.debt_to_equity, r.debt_to_equity, F_DE, EDGAR, meta);
            put(
                &mut q.profit_margins,
                r.profit_margin,
                F_MARGIN,
                EDGAR,
                meta,
            );
            put(
                &mut q.revenue_growth,
                r.revenue_growth,
                F_GROWTH,
                EDGAR,
                meta,
            );
            put(
                &mut q.eps_trailing_twelve_months,
                r.eps_ttm,
                F_EPS,
                EDGAR,
                meta,
            );
            if q.trailing_pe.is_none()
                && let (Some(p), Some(e)) = (q.regular_market_price, q.eps_trailing_twelve_months)
                && e > 0.0
            {
                q.trailing_pe = Some(p / e);
                meta.record(F_PE, EDGAR);
            }
            if q.long_name.is_none() && !st.company.is_empty() {
                q.long_name = Some(st.company.clone());
            }
        }
        Ok(None) => {}
        Err(e) => meta.warn(format!("{EDGAR} unavailable ({e:#})")),
    }
}

async fn fill_india(client: &YahooClient, symbol: &str, q: &mut Quote, meta: &mut Meta) {
    use provenance::*;
    let ns = match symbol.strip_suffix(".BO") {
        Some(base) if !base.chars().all(|c| c.is_ascii_digit()) => format!("{base}.NS"),
        _ => symbol.to_string(),
    };
    // 1. NSE quote-equity: P/E, sector P/E, industry, market cap.
    if ns.ends_with(".NS") {
        match client.nse_equity_info(&ns).await {
            Ok(Some(info)) => {
                put(&mut q.trailing_pe, info.pe, F_PE, NSE, meta);
                if q.sector_pe.is_none() {
                    q.sector_pe = info.sector_pe;
                }
                put(
                    &mut q.market_cap,
                    info.market_cap(),
                    F_MARKET_CAP,
                    NSE,
                    meta,
                );
                if q.sector.is_none()
                    && let Some(s) = info.sector_index.clone().or(info.industry.clone())
                {
                    q.sector = Some(s);
                    meta.record(F_SECTOR, NSE);
                }
                if q.industry.is_none() {
                    q.industry = info.industry;
                }
            }
            Ok(None) => {}
            Err(e) => meta.warn(format!("{NSE} company data unavailable ({e:#})")),
        }
    }
    // 2. NSE quarterly results: EPS, margin, revenue growth.
    let wants_results = q.eps_trailing_twelve_months.is_none()
        || q.profit_margins.is_none()
        || q.revenue_growth.is_none();
    if wants_results {
        match client.nse_results(&ns).await {
            Ok(rows) if !rows.is_empty() => {
                let r = super::nse::result_ratios(&rows);
                put(
                    &mut q.eps_trailing_twelve_months,
                    r.eps_ttm,
                    F_EPS,
                    NSE_RESULTS,
                    meta,
                );
                put(
                    &mut q.profit_margins,
                    r.profit_margin,
                    F_MARGIN,
                    NSE_RESULTS,
                    meta,
                );
                put(
                    &mut q.revenue_growth,
                    r.revenue_growth,
                    F_GROWTH,
                    NSE_RESULTS,
                    meta,
                );
            }
            Ok(_) => {}
            Err(e) => meta.warn(format!("{NSE_RESULTS} unavailable ({e:#})")),
        }
    }
    // 3. BSE scrip header: P/E, EPS, ROE, P/B.
    let wants_bse = q.trailing_pe.is_none()
        || q.eps_trailing_twelve_months.is_none()
        || q.return_on_equity.is_none()
        || q.price_to_book.is_none();
    if wants_bse {
        match client.bse_ratios(symbol).await {
            Ok(Some(r)) => {
                put(&mut q.trailing_pe, r.pe, F_PE, BSE, meta);
                put(&mut q.eps_trailing_twelve_months, r.eps, F_EPS, BSE, meta);
                put(&mut q.return_on_equity, r.roe, F_ROE, BSE, meta);
                put(&mut q.price_to_book, r.pb, F_PB, BSE, meta);
                if q.sector.is_none()
                    && let Some(i) = r.industry
                {
                    q.sector = Some(i);
                    meta.record(F_SECTOR, BSE);
                }
            }
            Ok(None) => {}
            Err(e) => meta.warn(format!("{BSE} ratios unavailable ({e:#})")),
        }
    }
}

/// Finnhub and FMP (when their keys are set): ratios, then the profile for
/// sector and market cap.
async fn fill_keyed(client: &YahooClient, symbol: &str, q: &mut Quote, meta: &mut Meta) {
    match client.provider_ratios(symbol).await {
        Ok((r, sources)) => {
            let filled = r.fill(q);
            let source = sources.join(" + ");
            for f in filled {
                meta.record(f, &source);
            }
        }
        Err(e) => meta.warn(format!("Keyed providers' ratios unavailable ({e:#})")),
    }
    if q.sector.is_none() || q.market_cap.is_none() {
        match client.provider_fundamentals(symbol).await {
            Ok((f, sources)) => {
                let had_sector = q.sector.is_some();
                let had_cap = q.market_cap.is_some();
                if f.fill(q) {
                    let source = sources.join(" + ");
                    if !had_sector && q.sector.is_some() {
                        meta.record(provenance::F_SECTOR, &source);
                    }
                    if !had_cap && q.market_cap.is_some() {
                        meta.record(provenance::F_MARKET_CAP, &source);
                    }
                }
            }
            Err(e) => meta.warn(format!("Keyed providers' profile unavailable ({e:#})")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_server(server_url: &str) -> impl Fn(&str) -> String + Send + Sync + 'static + use<> {
        let base = server_url.to_string();
        move |url: &str| {
            let parsed = reqwest::Url::parse(url).unwrap();
            match parsed.query() {
                Some(q) => format!("{}{}?{}", base, parsed.path(), q),
                None => format!("{}{}", base, parsed.path()),
            }
        }
    }

    /// Yahoo's quoteSummary fails (404); EDGAR answers. ROE, D/E, margin,
    /// growth and EPS come from EDGAR, and `meta.sources` names it.
    #[tokio::test]
    async fn us_fundamentals_fill_from_edgar_when_yahoo_fails() {
        let facts = super::super::edgar::tests::calendar_filer_with_balance_sheet();
        let server = crate::testutil::MockServer::start_routed(vec![
            (
                "/files/company_tickers.json",
                r#"{"0":{"cik_str":1234,"ticker":"CALX","title":"Calendar Corp"}}"#.to_string(),
            ),
            ("/api/xbrl/companyfacts/CIK0000001234.json", facts),
        ])
        .await;
        let client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "crumb",
            to_server(&server.url),
        );
        // What the v7 quote gives: price, P/E, P/B, EPS, market cap.
        let mut q = Quote {
            symbol: Some("CALX".into()),
            regular_market_price: Some(100.0),
            trailing_pe: Some(20.0),
            price_to_book: Some(3.0),
            market_cap: Some(1e9),
            currency: Some("USD".into()),
            ..Default::default()
        };
        // quoteSummary fails: nothing fills ROE etc.
        let summary = client.quote_summary("CALX", &["financialData"]).await;
        assert!(summary.is_err());
        let mut meta = Meta::default();
        complete_fundamentals(&client, &mut q, &mut meta).await;

        assert!((q.return_on_equity.unwrap() - 0.2).abs() < 1e-9);
        assert!((q.profit_margins.unwrap() - 0.1).abs() < 1e-9);
        assert!((q.revenue_growth.unwrap() - 0.2).abs() < 1e-9);
        assert!(q.debt_to_equity.is_some());
        assert_eq!(q.eps_trailing_twelve_months, Some(9.0));
        assert_eq!(meta.sources, vec![provenance::YAHOO, provenance::EDGAR]);
        assert_eq!(
            meta.field_sources
                .get(provenance::F_ROE)
                .map(String::as_str),
            Some(provenance::EDGAR)
        );
        // Beta and sector have no source here: listed, not defaulted.
        assert_eq!(meta.missing, vec![provenance::F_BETA, provenance::F_SECTOR]);
        assert!(q.beta.is_none());
        assert_eq!(meta.coverage, 80.0);
        assert!(meta.footer().contains("Sources: Yahoo Finance, SEC EDGAR"));
        assert!(meta.footer().contains("missing: beta, sector"));
    }

    #[tokio::test]
    async fn a_failing_edgar_is_a_warning_not_silence() {
        let server = crate::testutil::MockServer::start_routed(vec![]).await;
        let client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "crumb",
            to_server(&server.url),
        );
        let mut q = Quote {
            symbol: Some("CALX".into()),
            regular_market_price: Some(100.0),
            currency: Some("USD".into()),
            ..Default::default()
        };
        let mut meta = Meta::default();
        complete_fundamentals(&client, &mut q, &mut meta).await;
        assert!(
            meta.warnings
                .iter()
                .any(|w| w.starts_with("SEC EDGAR unavailable")),
            "{:?}",
            meta.warnings
        );
        assert_eq!(meta.coverage, 0.0);
        assert_eq!(meta.sources, vec![provenance::YAHOO]);
    }

    #[tokio::test]
    async fn an_sec_refusal_says_how_to_fix_it() {
        let server = crate::testutil::MockServer::start(vec![(403, "Forbidden".into())]).await;
        let client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "crumb",
            to_server(&server.url),
        );
        let mut q = Quote {
            symbol: Some("CALX".into()),
            regular_market_price: Some(100.0),
            currency: Some("USD".into()),
            ..Default::default()
        };
        let mut meta = Meta::default();
        complete_fundamentals(&client, &mut q, &mut meta).await;
        let w = meta.warnings.join(" ");
        assert!(
            w.contains("HTTP 403") && w.contains("STOCKWISE_SEC_USER_AGENT"),
            "{w}"
        );
    }

    #[tokio::test]
    async fn indian_fundamentals_fill_from_nse_and_bse() {
        let results = serde_json::json!({"resCmpData": [
            {"re_to_dt": "30-SEP-2025", "re_net_sale": 240000, "re_net_profit": 24000, "re_dil_eps": 17.5},
            {"re_to_dt": "30-JUN-2025", "re_net_sale": 230000, "re_net_profit": 23000, "re_dil_eps": 16.5},
            {"re_to_dt": "31-MAR-2025", "re_net_sale": 220000, "re_net_profit": 22000, "re_dil_eps": 15.0},
            {"re_to_dt": "31-DEC-2024", "re_net_sale": 210000, "re_net_profit": 21000, "re_dil_eps": 14.0},
            {"re_to_dt": "30-SEP-2024", "re_net_sale": 200000, "re_net_profit": 20000, "re_dil_eps": 13.0}
        ]})
        .to_string();
        let server = crate::testutil::MockServer::start_routed(vec![
            (
                "/api/quote-equity",
                include_str!("fixtures/nse_quote_equity.json").to_string(),
            ),
            ("/api/results-comparision", results),
            (
                "/ListofScripData/w",
                include_str!("fixtures/bse_scrip_list.json").to_string(),
            ),
            (
                "/ComHeadernew/w",
                r#"{"PE":"24.50","EPS":"60.68","ROE":"8.40","PB":"2.10"}"#.to_string(),
            ),
        ])
        .await;
        let mut client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "crumb",
            to_server(&server.url),
        );
        client.exchange_backup = true;
        client.set_test_exchanges(&server.url);
        let mut q = Quote {
            symbol: Some("500325.BO".into()),
            regular_market_price: Some(1451.6),
            currency: Some("INR".into()),
            price_to_book: Some(2.0),
            ..Default::default()
        };
        // A numeric BSE code has no NSE twin: BSE only.
        let mut meta = Meta::default();
        complete_fundamentals(&client, &mut q, &mut meta).await;
        assert_eq!(q.trailing_pe, Some(24.5));
        assert!((q.return_on_equity.unwrap() - 0.084).abs() < 1e-12);
        assert_eq!(
            meta.field_sources
                .get(provenance::F_ROE)
                .map(String::as_str),
            Some(provenance::BSE)
        );

        let mut q = Quote {
            symbol: Some("RELIANCE.NS".into()),
            regular_market_price: Some(1451.6),
            currency: Some("INR".into()),
            ..Default::default()
        };
        let mut meta = Meta::default();
        complete_fundamentals(&client, &mut q, &mut meta).await;
        assert_eq!(q.trailing_pe, Some(24.12));
        assert_eq!(q.sector_pe, Some(19.33));
        assert_eq!(q.sector.as_deref(), Some("NIFTY OIL & GAS"));
        assert!(q.market_cap.is_some());
        assert_eq!(q.eps_trailing_twelve_months, Some(63.0));
        assert!((q.profit_margins.unwrap() - 0.1).abs() < 1e-12);
        assert!((q.revenue_growth.unwrap() - 0.2).abs() < 1e-12);
        assert_eq!(q.price_to_book, Some(2.1));
        assert_eq!(
            meta.sources,
            vec![
                provenance::YAHOO,
                provenance::NSE,
                provenance::NSE_RESULTS,
                provenance::BSE
            ]
        );
        assert_eq!(meta.missing, vec![provenance::F_DE, provenance::F_BETA]);
    }
}
