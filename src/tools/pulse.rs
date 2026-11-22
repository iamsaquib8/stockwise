//! Tools for what is moving now: `whats_hot`, `today`, `news_feed`,
//! `monitor_status` and `digest`. Their definitions are in [`super::TOOLS`];
//! the main ones publish an output schema (see `schema.rs`).

use super::args::Args;
use crate::market::{self, Market};
use crate::views::DataSource;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Run one of this module's tools.
pub(super) async fn run<D: DataSource>(src: &D, name: &str, a: &Args, m: Market) -> Result<Value> {
    if name == "monitor_status" {
        let state: crate::monitor::MonitorState = crate::store::load(crate::monitor::STATE_FILE)
            .with_context(|| format!("Could not read {}", crate::monitor::STATE_FILE))?;
        return Ok(json!({
            "thresholds": crate::monitor::Thresholds::from_config(),
            "worker_running": crate::worker::running_pid().is_some(),
            "last_pass": state.last_pass,
            "last_digest": state.last_digest,
            "recent": state.recent,
        }));
    }
    let client = src.yahoo().await?;
    match name {
        "whats_hot" => {
            if !matches!(m, Market::In | Market::Us) {
                bail!("whats_hot covers India (market in) and the US (market us)");
            }
            let cap = a
                .str("cap")
                .and_then(|c| <market::CapFilter as clap::ValueEnum>::from_str(c, true).ok());
            let limit = a.usize_or("limit", 8).clamp(1, 25);
            let r = crate::commands::build_hot(client, m, cap, None, limit, true).await?;
            Ok(serde_json::to_value(r)?)
        }
        "today" => {
            let days = a.usize_or("days", 2).clamp(1, 14) as u32;
            let p = crate::pulse::gather(
                client,
                m,
                crate::pulse::Options {
                    days,
                    ..Default::default()
                },
            )
            .await?;
            let feed: Vec<_> = crate::commands::today_feed(&p)
                .into_iter()
                .take(15)
                .collect();
            Ok(json!({ "pulse": p, "feed": feed }))
        }
        "news_feed" => {
            let days = a.usize_or("days", 2).clamp(1, 14) as u32;
            let limit = a.usize_or("limit", 20).clamp(1, 60);
            let symbols = a.list("symbols");
            let entries: Vec<crate::news::FeedEntry> = if symbols.is_empty() {
                // Holdings (weighted by value) and the watchlist.
                let p = crate::pulse::gather(
                    client,
                    m,
                    crate::pulse::Options {
                        days,
                        news: false,
                        events: false,
                        ..Default::default()
                    },
                )
                .await?;
                p.rows
                    .iter()
                    .filter(|r| !market::is_namespaced(&r.symbol))
                    .map(|r| crate::news::FeedEntry {
                        symbol: r.symbol.clone(),
                        company: r.name.clone(),
                        weight: if r.held {
                            r.weight
                        } else {
                            crate::news::WATCHLIST_WEIGHT
                        },
                    })
                    .collect()
            } else {
                symbols
                    .iter()
                    .map(|s| crate::news::FeedEntry {
                        symbol: market::resolve_symbol(s.trim(), m),
                        company: None,
                        weight: crate::news::WATCHLIST_WEIGHT,
                    })
                    .collect()
            };
            let (items, failures) = crate::news::feed(client, &entries, days, true).await;
            Ok(json!({
                "days": days,
                "symbols": entries.iter().map(|e| e.symbol.clone()).collect::<Vec<_>>(),
                "items": items.into_iter().take(limit).collect::<Vec<_>>(),
                "failures": failures,
            }))
        }
        "digest" => {
            let mut st: crate::monitor::MonitorState =
                crate::store::load(crate::monitor::STATE_FILE)
                    .with_context(|| format!("Could not read {}", crate::monitor::STATE_FILE))?;
            let (d, _) = crate::commands::build_digest(client, m, Vec::new(), &mut st).await?;
            let msg = crate::digest::render(&d);
            Ok(json!({ "digest": d, "text": format!("{}\n{}", msg.title, msg.body) }))
        }
        other => bail!("Tool '{other}' has no handler here"),
    }
}
