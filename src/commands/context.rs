//! `stockwise use`: the default market and the active portfolio.

use super::*;
use crate::config;
use clap::ValueEnum;

/// What `stockwise use` was asked to change.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct UseRequest {
    pub market: Option<Market>,
    /// `Some(None)` is the default portfolio.
    pub portfolio: Option<Option<String>>,
}

/// Sort `targets` into a market and a portfolio. A word that is a market
/// (`us`, `uk`, `india`) is the market; anything else is a portfolio name.
pub(crate) fn parse_use_targets(targets: &[String]) -> Result<UseRequest> {
    let mut req = UseRequest::default();
    for t in targets {
        if let Ok(m) = Market::from_str(t.trim(), true) {
            if let Some(prev) = req.market {
                bail!(
                    "Two markets given ({} and {}); pick one",
                    prev.code(),
                    m.code()
                );
            }
            req.market = Some(m);
        } else {
            let name = crate::portfolio::normalize_name(Some(t))?;
            if req.portfolio.is_some() {
                bail!(
                    "`{}` is not a market, and a portfolio is already given. Markets: {}",
                    t,
                    market_codes()
                );
            }
            req.portfolio = Some(name);
        }
    }
    Ok(req)
}

fn market_codes() -> String {
    Market::value_variants()
        .iter()
        .map(|m| m.code())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Apply `req` to the config file at `path`: the market and portfolio
/// lines, and the binding with `bind` / `unbind`. `active` is the portfolio
/// in use now (for `--bind` without a portfolio). Returns the market the
/// file now names, if it changed.
pub(crate) fn apply_use(
    path: &std::path::Path,
    req: &UseRequest,
    bind: bool,
    unbind: bool,
    active: Option<&str>,
    bindings: &std::collections::BTreeMap<String, String>,
) -> Result<Option<Market>> {
    let portfolio: Option<&str> = match &req.portfolio {
        Some(p) => p.as_deref(),
        None => active,
    };
    let key = portfolio.unwrap_or("default");
    if bind {
        let Some(m) = req.market else {
            bail!("--bind needs a market, e.g. `stockwise use {key} us --bind`");
        };
        config::set_portfolio_market(path, key, Some(m.code()))?;
    }
    if unbind {
        config::set_portfolio_market(path, key, None)?;
    }
    if let Some(p) = &req.portfolio {
        config::set_top_level(path, "portfolio", p.as_deref().unwrap_or("default"))?;
    }
    // A portfolio picked on its own brings its bound market along.
    let market = req.market.or_else(|| {
        if req.portfolio.is_none() || unbind {
            return None;
        }
        bindings
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .and_then(|(_, v)| Market::from_str(v, true).ok())
    });
    if let Some(m) = market {
        config::set_market(path, &crate::value_name(m))?;
    }
    Ok(market)
}

pub fn cmd_use(
    targets: &[String],
    bind: bool,
    unbind: bool,
    market: Market,
    sources: config::CliSources,
    json: bool,
) -> Result<()> {
    let req = parse_use_targets(targets)?;
    if req == UseRequest::default() && !bind && !unbind {
        return show_context(market, sources, json);
    }
    if let Some(Some(name)) = &req.portfolio {
        let known = crate::portfolio::list_names().unwrap_or_default();
        if !known.iter().any(|n| n == name) {
            let list = if known.is_empty() {
                "none yet".to_string()
            } else {
                known.join(", ")
            };
            bail!(
                "No portfolio named `{name}` (saved portfolios: {list}). Create it with \
                 `stockwise --portfolio {name} portfolio add ...`, or pick a market: {}",
                market_codes()
            );
        }
    }
    let path = config::path()
        .context("Cannot determine the config directory — set STOCKWISE_CONFIG to a file path")?;
    let cfg = config::get();
    let active = cfg
        .portfolio
        .as_deref()
        .and_then(|p| crate::portfolio::normalize_name(Some(p)).ok().flatten());
    let new_market = apply_use(
        &path,
        &req,
        bind,
        unbind,
        active.as_deref(),
        &cfg.portfolio_markets,
    )?;

    if json {
        let portfolio = match &req.portfolio {
            Some(p) => p.clone(),
            None => active.clone(),
        };
        return output::print_json(&serde_json::json!({
            "config": path.display().to_string(),
            "market": new_market.map(|m| m.code()),
            "portfolio": req.portfolio.as_ref().map(|p| p.clone().unwrap_or_else(|| "default".into())),
            "bound": (bind || unbind).then(|| serde_json::json!({
                "portfolio": portfolio.unwrap_or_else(|| "default".into()),
                "market": if unbind { None } else { req.market.map(|m| m.code()) },
            })),
        }));
    }

    if let Some(p) = &req.portfolio {
        println!(
            "  {} Active portfolio: {}",
            "✓".green(),
            p.as_deref().unwrap_or("default").cyan()
        );
    }
    if let Some(m) = new_market {
        println!("  {} Active market: {}", "✓".green(), market_summary(m));
    }
    let key = match &req.portfolio {
        Some(p) => p.clone(),
        None => active,
    }
    .unwrap_or_else(|| "default".into());
    if bind && let Some(m) = req.market {
        println!(
            "  {} Portfolio {} is bound to {}",
            "✓".green(),
            key.cyan(),
            m.code().cyan()
        );
    }
    if unbind {
        println!(
            "  {} Portfolio {} has no market binding",
            "✓".green(),
            key.cyan()
        );
    }
    println!("  {}", format!("Saved in {}", path.display()).dimmed());
    if std::env::var("STOCKWISE_MARKET").is_ok_and(|v| !v.trim().is_empty()) && new_market.is_some()
    {
        println!(
            "  {}",
            "! STOCKWISE_MARKET is set in this shell and still wins; unset it to use this market"
                .yellow()
        );
    }
    if std::env::var("STOCKWISE_PORTFOLIO").is_ok_and(|v| !v.trim().is_empty())
        && req.portfolio.is_some()
    {
        println!(
            "  {}",
            "! STOCKWISE_PORTFOLIO is set in this shell and still wins".yellow()
        );
    }
    Ok(())
}

/// `US (NYSE/NASDAQ · USD · benchmark S&P 500)`.
fn market_summary(m: Market) -> String {
    let detail = match m {
        Market::In => "NSE/BSE · INR · benchmark NIFTY 50".to_string(),
        Market::Us => "NYSE/NASDAQ · USD · benchmark S&P 500".to_string(),
        Market::Crypto => format!("24/7 · quoted in {}", market::quote_currency()),
        Market::Fx => "currency pairs · 24x5".to_string(),
        Market::Commodity => "COMEX gold and silver · NSE ETFs and SGBs".to_string(),
        Market::Global(x) => {
            let info = x.info();
            format!(
                "{} · {} · benchmark {}",
                info.short,
                info.currency,
                info.benchmark().1
            )
        }
    };
    format!("{} ({})", m.label().cyan(), detail)
}

fn show_context(market: Market, sources: config::CliSources, json: bool) -> Result<()> {
    let portfolio = crate::portfolio::active().unwrap_or("default");
    let cfg = config::get();
    let bound = cfg.portfolio_market(crate::portfolio::active());
    if json {
        return output::print_json(&serde_json::json!({
            "market": market.code(),
            "market_source": sources.market,
            "portfolio": portfolio,
            "portfolio_source": sources.portfolio,
            "portfolio_market": bound,
            "bindings": cfg.portfolio_markets,
        }));
    }
    print_header("StockWise context");
    print_kv(
        "Market",
        &format!(
            "{}  {}",
            market_summary(market),
            format!("[{}]", sources.market).dimmed()
        ),
    );
    print_kv(
        "Portfolio",
        &format!(
            "{}  {}",
            portfolio.cyan(),
            format!("[{}]", sources.portfolio).dimmed()
        ),
    );
    if !cfg.portfolio_markets.is_empty() {
        let list: Vec<String> = cfg
            .portfolio_markets
            .iter()
            .map(|(p, m)| format!("{p} → {m}"))
            .collect();
        print_kv("Bindings", &list.join(", "));
    }
    println!();
    println!(
        "  {}",
        "Change with `stockwise use MARKET` or `stockwise use PORTFOLIO`; \
         -m and STOCKWISE_MARKET win for one command."
            .dimmed()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::Exchange;
    use std::collections::BTreeMap;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn temp_config(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sw-use-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    fn read(path: &std::path::Path) -> config::Config {
        config::parse(&std::fs::read_to_string(path).unwrap(), &[]).unwrap()
    }

    #[test]
    fn targets_sort_into_market_and_portfolio() {
        let r = parse_use_targets(&strings(&["us"])).unwrap();
        assert_eq!((r.market, r.portfolio), (Some(Market::Us), None));
        let r = parse_use_targets(&strings(&["Long-Term", "UK"])).unwrap();
        assert_eq!(r.market, Some(Market::Global(Exchange::Uk)));
        assert_eq!(r.portfolio, Some(Some("long-term".into())));
        let r = parse_use_targets(&strings(&["default"])).unwrap();
        assert_eq!(r.portfolio, Some(None));
        assert!(parse_use_targets(&strings(&["us", "in"])).is_err());
        assert!(parse_use_targets(&strings(&["a", "b"])).is_err());
        assert!(parse_use_targets(&strings(&["bad name!"])).is_err());
    }

    #[test]
    fn use_round_trip_through_the_config_file() {
        let path = temp_config("roundtrip");
        let none = BTreeMap::new();
        let req = parse_use_targets(&strings(&["us"])).unwrap();
        assert_eq!(
            apply_use(&path, &req, false, false, None, &none).unwrap(),
            Some(Market::Us)
        );
        let cfg = read(&path);
        assert_eq!(cfg.market.as_deref(), Some("us"));
        // The template's comments survive.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# StockWise configuration."));
        // The saved market is what the next run picks.
        assert_eq!(
            config::pick_market(None, None, None, false, cfg.market.as_deref()),
            Some(("us".into(), config::Source::File))
        );
        // Switch back, and pick a portfolio.
        let req = parse_use_targets(&strings(&["in", "long-term"])).unwrap();
        apply_use(&path, &req, false, false, None, &none).unwrap();
        let cfg = read(&path);
        assert_eq!(cfg.market.as_deref(), Some("in"));
        assert_eq!(cfg.portfolio.as_deref(), Some("long-term"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn portfolio_binding_sets_and_follows_the_market() {
        let path = temp_config("bind");
        let req = parse_use_targets(&strings(&["us-tech", "us"])).unwrap();
        apply_use(&path, &req, true, false, None, &BTreeMap::new()).unwrap();
        let cfg = read(&path);
        assert_eq!(cfg.portfolio_market(Some("us-tech")), Some("us"));
        assert_eq!(cfg.portfolio.as_deref(), Some("us-tech"));

        // Moving away, then picking the portfolio alone brings its market back.
        let req = parse_use_targets(&strings(&["in", "default"])).unwrap();
        apply_use(&path, &req, false, false, None, &cfg.portfolio_markets).unwrap();
        assert_eq!(read(&path).market.as_deref(), Some("in"));
        let req = parse_use_targets(&strings(&["us-tech"])).unwrap();
        assert_eq!(
            apply_use(&path, &req, false, false, None, &cfg.portfolio_markets).unwrap(),
            Some(Market::Us)
        );
        let cfg = read(&path);
        assert_eq!(cfg.market.as_deref(), Some("us"));

        // Picking the bound portfolio for one run (--portfolio) beats the
        // file's market; -m and the env var still beat the binding.
        let bound = cfg.portfolio_market(Some("us-tech"));
        assert_eq!(
            config::pick_market(None, None, bound, true, Some("in")),
            Some(("us".into(), config::Source::File))
        );
        assert_eq!(
            config::pick_market(None, Some("uk"), bound, true, Some("in")).map(|(m, _)| m),
            Some("uk".into())
        );

        // --bind without a portfolio binds the active one; --unbind removes it.
        let req = parse_use_targets(&strings(&["uk"])).unwrap();
        apply_use(&path, &req, true, false, Some("isa"), &BTreeMap::new()).unwrap();
        assert_eq!(read(&path).portfolio_market(Some("isa")), Some("uk"));
        let req = parse_use_targets(&strings(&["us-tech"])).unwrap();
        apply_use(&path, &req, false, true, None, &BTreeMap::new()).unwrap();
        let cfg = read(&path);
        assert_eq!(cfg.portfolio_market(Some("us-tech")), None);
        assert_eq!(cfg.portfolio_market(Some("isa")), Some("uk"));
        // --bind needs a market.
        let req = parse_use_targets(&strings(&["isa"])).unwrap();
        assert!(apply_use(&path, &req, true, false, None, &BTreeMap::new()).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
