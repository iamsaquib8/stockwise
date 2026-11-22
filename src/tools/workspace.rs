//! Tools that change the local workspace: the watchlist, alerts, saved
//! goals, target weights, the investor profile, notes on stocks, and (a
//! separate opt-in) the portfolio's lots. Off unless the host turns them on
//! (`stockwise mcp --allow-writes`, `ask --auto`). Every change goes
//! through [`crate::audit::Journal`]: the files are copied first and
//! `stockwise undo` puts them back. Nothing here can place an order.

use super::args::Args;
use super::{Kind, Param, Tool, opt, req};
use crate::alerts::{AlertCondition, AlertStore};
use crate::audit::Journal;
use crate::goals::{GoalStore, SavedGoal};
use crate::market::{self, Market};
use crate::notes::NoteStore;
use crate::portfolio::{Lot, Portfolio};
use crate::views::DataSource;
use crate::watchlist::Watchlist;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Where writes go, and which kinds are allowed.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The data directory (watchlist, alerts, goals, notes, the journal).
    pub dir: PathBuf,
    /// `config.toml` (the investor profile).
    pub config: PathBuf,
    /// The active portfolio's file.
    pub portfolio: PathBuf,
    /// Who makes the changes, for the journal: `mcp`, `ask`.
    pub source: &'static str,
    /// The portfolio's lots may change too (`portfolio_add`, `portfolio_sell`).
    pub allow_portfolio: bool,
}

impl Workspace {
    /// The user's own data directory, config file and active portfolio.
    pub fn for_user(source: &'static str, allow_portfolio: bool) -> Result<Self> {
        Ok(Self {
            dir: crate::store::data_dir()?,
            config: crate::config::path()
                .context("Cannot find the config file location (set STOCKWISE_CONFIG)")?,
            portfolio: crate::store::path(
                &crate::portfolio::file_name(crate::portfolio::active()),
            )?,
            source,
            allow_portfolio,
        })
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn journal(&self) -> Journal {
        Journal::new(&self.dir)
    }

    /// Run `change` on `paths` through the journal and describe the result.
    fn apply(
        &self,
        tool: &str,
        summary: String,
        paths: &[PathBuf],
        change: impl FnOnce() -> Result<Value>,
    ) -> Result<Value> {
        let (mut out, id) = self
            .journal()
            .record(self.source, tool, &summary, paths, change)?;
        out["changed"] = json!(summary);
        out["change_id"] = json!(id);
        out["undo"] = json!(format!("stockwise undo {}", id));
        Ok(out)
    }
}

const SYMBOLS: Param = req(
    "symbols",
    Kind::StrList { max: 20 },
    "Tickers, bare (TCS) or with an exchange suffix (TCS.NS, VOD.L); bare ones take the market's suffix",
);
const SYMBOL: Param = req(
    "symbol",
    Kind::Str,
    "Ticker, bare (TCS) or with an exchange suffix (TCS.NS, AAPL)",
);
const MARKET: Param = super::MARKET;

const CONDITIONS: &[&str] = &[
    "above",
    "below",
    "change-up",
    "change-down",
    "rsi-above",
    "rsi-below",
    "cross-above-ma",
    "cross-below-ma",
    "breakout-high",
    "breakdown-low",
    "volume-spike",
    "gap-up",
    "gap-down",
    "results-in",
];

/// Workspace changes (no lots): offered with `--allow-writes` / `ask --auto`.
pub const WRITE_TOOLS: &[Tool] = &[
    Tool {
        name: "watchlist_add",
        title: "Add to watchlist",
        description: "Add tickers to the user's watchlist. Unknown tickers are refused. The change can be undone with `stockwise undo`.",
        params: &[SYMBOLS, MARKET],
        personal: true,
    },
    Tool {
        name: "watchlist_remove",
        title: "Remove from watchlist",
        description: "Remove tickers from the user's watchlist. The change can be undone with `stockwise undo`.",
        params: &[SYMBOLS, MARKET],
        personal: true,
    },
    Tool {
        name: "alert_add",
        title: "Add an alert",
        description: "Add a price or technical alert. condition: above/below a price, change-up/down a percent, rsi-above/below a level, cross-above/below-ma a moving-average length, breakout-high/breakdown-low a look-back in days, volume-spike a multiple, gap-up/down a percent, results-in days before results.",
        params: &[
            SYMBOL,
            req(
                "condition",
                Kind::Enum(CONDITIONS),
                "What the alert watches for",
            ),
            req(
                "value",
                Kind::Number {
                    min: 0.0,
                    max: 1.0e9,
                },
                "The condition's number: a price, percent, RSI level, MA length, days or volume multiple",
            ),
            opt(
                "period",
                Kind::Integer { min: 2, max: 200 },
                "RSI period or volume-average length, for the conditions that take one",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "alert_remove",
        title: "Remove an alert",
        description: "Remove one alert by its index from alerts_list. The symbol must match that alert, so a stale index cannot remove the wrong one. Can be undone with `stockwise undo`.",
        params: &[
            req(
                "index",
                Kind::Integer {
                    min: 0,
                    max: 100_000,
                },
                "The alert's index as alerts_list shows it",
            ),
            SYMBOL,
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "goal_save",
        title: "Save a goal",
        description: "Save (or replace, by name) a goal in goals.json so goal_list tracks it: a target amount due by a date, optionally in today's money with an inflation rate, with the holdings that count towards it.",
        params: &[
            req("name", Kind::Str, "Goal name, e.g. house"),
            req(
                "target",
                Kind::Number {
                    min: 1.0,
                    max: 1.0e13,
                },
                "Amount needed, in the market's currency",
            ),
            req("by", Kind::Str, "Due date, YYYY-MM or YYYY-MM-DD"),
            opt(
                "inflation_pct",
                Kind::Number {
                    min: 0.0,
                    max: 30.0,
                },
                "Yearly inflation; the target is then in today's money",
            ),
            opt(
                "holdings",
                Kind::StrList { max: 50 },
                "Holdings that count towards it (MF:122639, NIFTYBEES)",
            ),
            opt(
                "return_pct",
                Kind::Number {
                    min: 0.0,
                    max: 40.0,
                },
                "Yearly return to plan with",
            ),
        ],
        personal: true,
    },
    Tool {
        name: "goal_remove",
        title: "Remove a goal",
        description: "Remove a saved goal by name. Can be undone with `stockwise undo`.",
        params: &[req(
            "name",
            Kind::Str,
            "The goal's name as goal_list shows it",
        )],
        personal: true,
    },
    Tool {
        name: "target_set",
        title: "Set a target weight",
        description: "Set the target weight (percent of the portfolio) of a holding or a sector, used by rebalance. Can be undone with `stockwise undo`.",
        params: &[
            req(
                "key",
                Kind::Str,
                "A held symbol (TCS), or a sector name (Technology)",
            ),
            req(
                "percent",
                Kind::Number {
                    min: 0.0,
                    max: 100.0,
                },
                "Target weight in percent",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "target_remove",
        title: "Remove a target weight",
        description: "Remove the target weight of a holding or sector. Can be undone with `stockwise undo`.",
        params: &[
            req("key", Kind::Str, "The symbol or sector the target is for"),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "profile_set",
        title: "Update the investor profile",
        description: "Change settings of the investor profile in config.toml: mode (passive, active, ultra), risk (low, med, high), tax slab and regime, horizon, and the guardrails (largest weight of one stock, of small and micro caps, risk per trade). Only the given settings change. Can be undone with `stockwise undo`.",
        params: &[
            opt(
                "mode",
                Kind::Enum(&["passive", "active", "ultra"]),
                "Profile mode",
            ),
            opt("risk", Kind::Enum(&["low", "med", "high"]), "Risk level"),
            opt(
                "tax_slab",
                Kind::Number {
                    min: 0.0,
                    max: 50.0,
                },
                "Income-tax slab, percent",
            ),
            opt(
                "tax_regime",
                Kind::Enum(&["new", "old"]),
                "Indian tax regime",
            ),
            opt(
                "horizon_years",
                Kind::Integer { min: 1, max: 60 },
                "Investment horizon in years",
            ),
            opt(
                "max_stock_weight",
                Kind::Number {
                    min: 1.0,
                    max: 100.0,
                },
                "Largest weight of one stock, percent",
            ),
            opt(
                "max_smallcap_weight",
                Kind::Number {
                    min: 0.0,
                    max: 100.0,
                },
                "Largest weight of small and micro caps together, percent",
            ),
            opt(
                "risk_per_trade",
                Kind::Number {
                    min: 0.1,
                    max: 10.0,
                },
                "Risk per trade, percent of capital",
            ),
        ],
        personal: true,
    },
    Tool {
        name: "profile_onboard",
        title: "Set up the investor profile from answers",
        description: "Save the investor profile from the answers to profile_questions: the mode follows how closely the user follows their investments, the risk level their reaction to a fall (one step lower for a horizon under 3 years or under a year of experience); the guardrails take that profile's defaults. Returns the settings and the reason for each. Can be undone with `stockwise undo`.",
        params: &[
            req(
                "involvement",
                Kind::Enum(&["rarely", "regularly", "daily"]),
                "How closely the user follows their investments",
            ),
            req(
                "fall_reaction",
                Kind::Enum(&["move_to_safety", "wait", "invest_more"]),
                "What the user would do after a 20% fall in a month",
            ),
            req(
                "horizon_years",
                Kind::Integer { min: 1, max: 60 },
                "Years until most of the money is needed",
            ),
            opt(
                "experience",
                Kind::Enum(&["new", "some", "seasoned"]),
                "Under a year, one to five years, over five years of investing",
            ),
            opt(
                "tax_slab",
                Kind::Number {
                    min: 0.0,
                    max: 50.0,
                },
                "Income-tax slab, percent",
            ),
            opt(
                "tax_regime",
                Kind::Enum(&["new", "old"]),
                "Indian tax regime",
            ),
        ],
        personal: true,
    },
    Tool {
        name: "note_set",
        title: "Save a note on a stock",
        description: "Save the user's note on a stock: why they hold or watch it, and their own limits for it (largest P/E, smallest ROE, largest portfolio weight), which fit_check measures it against. Given fields replace the old ones; clear removes the note. Can be undone with `stockwise undo`.",
        params: &[
            SYMBOL,
            opt(
                "thesis",
                Kind::Str,
                "Why the user holds or watches it, in their words",
            ),
            opt(
                "max_pe",
                Kind::Number {
                    min: 0.0,
                    max: 1000.0,
                },
                "Highest trailing P/E the user accepts",
            ),
            opt(
                "min_roe_pct",
                Kind::Number {
                    min: -100.0,
                    max: 200.0,
                },
                "Lowest return on equity the user accepts, percent",
            ),
            opt(
                "max_weight_pct",
                Kind::Number {
                    min: 0.0,
                    max: 100.0,
                },
                "Largest share of the portfolio the user wants it to be, percent",
            ),
            opt("clear", Kind::Bool, "Remove the note instead"),
            MARKET,
        ],
        personal: true,
    },
];

/// Portfolio lot changes: offered only with `--allow-portfolio`.
pub const PORTFOLIO_TOOLS: &[Tool] = &[
    Tool {
        name: "portfolio_add",
        title: "Record a purchase",
        description: "Record a purchase in the portfolio as a new lot (shares, price paid per share in the stock's own currency, date). Two steps: called without confirm it only previews the change; show that to the user and call again with confirm: true once they agree. Records only: it places no order. Can be undone with `stockwise undo`.",
        params: &[
            SYMBOL,
            req(
                "shares",
                Kind::Number {
                    min: 0.000_001,
                    max: 1.0e9,
                },
                "Number of shares or units",
            ),
            req(
                "price",
                Kind::Number {
                    min: 0.0,
                    max: 1.0e9,
                },
                "Price paid per share, in the stock's own currency",
            ),
            opt(
                "date",
                Kind::Str,
                "Purchase date, YYYY-MM-DD (default: today)",
            ),
            opt(
                "confirm",
                Kind::Bool,
                "true only after the user has seen this change's preview and agreed to it; without it, nothing is recorded and a preview comes back",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "portfolio_sell",
        title: "Record a sale",
        description: "Record a sale: the oldest lots first (FIFO), keeping the realised gain for tax. Two steps: called without confirm it only previews the lots sold and the gain; show that to the user and call again with confirm: true once they agree. Records only: it places no order. Can be undone with `stockwise undo`.",
        params: &[
            SYMBOL,
            req(
                "shares",
                Kind::Number {
                    min: 0.000_001,
                    max: 1.0e9,
                },
                "Number of shares or units sold",
            ),
            req(
                "price",
                Kind::Number {
                    min: 0.0,
                    max: 1.0e9,
                },
                "Price received per share",
            ),
            opt("date", Kind::Str, "Sale date, YYYY-MM-DD (default: today)"),
            opt(
                "confirm",
                Kind::Bool,
                "true only after the user has seen this change's preview and agreed to it; without it, nothing is recorded and a preview comes back",
            ),
            MARKET,
        ],
        personal: true,
    },
];

/// Whether a tool changes anything, and how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Workspace,
    Portfolio,
}

pub fn access(name: &str) -> Access {
    if WRITE_TOOLS.iter().any(|t| t.name == name) {
        Access::Workspace
    } else if PORTFOLIO_TOOLS.iter().any(|t| t.name == name) {
        Access::Portfolio
    } else {
        Access::Read
    }
}

/// `(destructive, idempotent)` of a write tool. Destructive: it removes or
/// replaces something the user saved (every change is still undoable with
/// `stockwise undo`). Idempotent: calling it twice with the same arguments
/// leaves the same files as calling it once.
pub fn write_effect(name: &str) -> (bool, bool) {
    match name {
        // Each call adds another alert or lot. A repeated watchlist add finds
        // the tickers already there but still writes an undo-journal entry.
        "watchlist_add" | "alert_add" | "portfolio_add" => (false, false),
        // Indexes shift after a removal; a sale uses up lots each time.
        "alert_remove" | "portfolio_sell" => (true, false),
        // Saves that stamp today's date (`created`, `updated`): a retry on a
        // later day changes the stored file.
        "goal_save" | "note_set" => (true, false),
        // Removals, and saves that replace by name or key.
        "watchlist_remove" | "goal_remove" | "target_set" | "target_remove" | "profile_set"
        | "profile_onboard" => (true, true),
        // Unknown writes: the cautious answer.
        _ => (true, false),
    }
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn date_arg(a: &Args) -> Result<String> {
    match a.str("date") {
        Some(d) => Ok(crate::portfolio::parse_date(d)?.to_string()),
        None => Ok(today()),
    }
}

fn resolve_all(a: &Args, m: Market) -> Vec<String> {
    a.list("symbols")
        .iter()
        .map(|s| market::resolve_symbol(s.trim(), m))
        .collect()
}

/// Run a write tool.
pub(super) async fn run<D: DataSource>(
    w: &Workspace,
    src: &D,
    name: &str,
    a: &Args,
    m: Market,
) -> Result<Value> {
    match name {
        "watchlist_add" => {
            let syms = resolve_all(a, m);
            crate::commands::ensure_symbols_exist(src, &syms, m).await?;
            let path = w.file("watchlist.json");
            let p = path.clone();
            w.apply(
                name,
                format!("Added {} to the watchlist", syms.join(", ")),
                &[path],
                move || {
                    crate::store::update_at(&p, |wl: &mut Watchlist| {
                        let added: Vec<&String> = syms.iter().filter(|s| wl.add(s)).collect();
                        Ok(json!({ "added": added, "watchlist": wl.symbols }))
                    })
                },
            )
        }
        "watchlist_remove" => {
            let syms = resolve_all(a, m);
            let path = w.file("watchlist.json");
            let p = path.clone();
            w.apply(
                name,
                format!("Removed {} from the watchlist", syms.join(", ")),
                &[path],
                move || {
                    crate::store::update_at(&p, |wl: &mut Watchlist| {
                        let removed: Vec<&String> = syms.iter().filter(|s| wl.remove(s)).collect();
                        if removed.is_empty() {
                            bail!("None of {} is on the watchlist", syms.join(", "));
                        }
                        Ok(json!({ "removed": removed, "watchlist": wl.symbols }))
                    })
                },
            )
        }
        "alert_add" => {
            let sym = market::resolve_symbol(a.need_str("symbol")?.trim(), m);
            let cond_name = a.need_str("condition")?;
            let cond = AlertCondition::parse(cond_name)
                .with_context(|| format!("Unknown condition '{}'", cond_name))?;
            let period = a.i64("period").map(|p| p as usize);
            let (value, period) = crate::alerts::validate(cond, a.need_f64("value")?, period)?;
            crate::commands::ensure_symbols_exist(src, std::slice::from_ref(&sym), m).await?;
            let path = w.file("alerts.json");
            let p = path.clone();
            let summary = format!("Added alert {} {} {}", sym, cond.name(), value);
            w.apply(name, summary, &[path], move || {
                crate::store::update_at(&p, |s: &mut AlertStore| {
                    s.add_with_period(&sym, cond, value, period.or(cond.default_period()));
                    Ok(json!({ "index": s.alerts.len() - 1, "alerts": s.alerts.len() }))
                })
            })
        }
        "alert_remove" => {
            let index = a.i64("index").context("Missing argument 'index'")? as usize;
            let sym = market::resolve_symbol(a.need_str("symbol")?.trim(), m);
            let path = w.file("alerts.json");
            let p = path.clone();
            w.apply(
                name,
                format!("Removed alert #{} on {}", index, sym),
                &[path],
                move || {
                    crate::store::update_at(&p, |s: &mut AlertStore| {
                        match s.alerts.get(index) {
                            Some(al) if al.symbol.eq_ignore_ascii_case(&sym) => {}
                            Some(al) => bail!(
                                "Alert #{} is on {}, not {}; list the alerts again",
                                index,
                                al.symbol,
                                sym
                            ),
                            None => bail!("No alert #{}; list the alerts again", index),
                        }
                        s.remove_by_index(index);
                        Ok(json!({ "alerts": s.alerts.len() }))
                    })
                },
            )
        }
        "goal_save" => {
            let name_ = a.need_str("name")?.trim().to_string();
            if name_.is_empty() {
                bail!("The goal needs a name");
            }
            let by = a.need_str("by")?.trim().to_string();
            crate::goals::months_until(chrono::Local::now().date_naive(), &by)
                .map_err(anyhow::Error::msg)?;
            let goal = SavedGoal {
                name: name_.clone(),
                target: a.need_f64("target")?,
                by,
                inflation_pct: a.f64("inflation_pct"),
                holdings: a.list("holdings"),
                return_pct: a.f64("return_pct"),
                created: today(),
            };
            let path = w.file("goals.json");
            let p = path.clone();
            w.apply(
                name,
                format!("Saved goal '{}'", name_),
                &[path],
                move || {
                    crate::store::update_at(&p, |g: &mut GoalStore| {
                        let replaced = match g
                            .goals
                            .iter_mut()
                            .find(|x| x.name.eq_ignore_ascii_case(&goal.name))
                        {
                            Some(old) => {
                                *old = goal.clone();
                                true
                            }
                            None => {
                                g.goals.push(goal.clone());
                                false
                            }
                        };
                        Ok(json!({ "goal": goal, "replaced": replaced }))
                    })
                },
            )
        }
        "goal_remove" => {
            let name_ = a.need_str("name")?.trim().to_string();
            let path = w.file("goals.json");
            let p = path.clone();
            w.apply(
                name,
                format!("Removed goal '{}'", name_),
                &[path],
                move || {
                    crate::store::update_at(&p, |g: &mut GoalStore| {
                        let before = g.goals.len();
                        g.goals.retain(|x| !x.name.eq_ignore_ascii_case(&name_));
                        if g.goals.len() == before {
                            bail!("No goal named '{}'", name_);
                        }
                        Ok(json!({ "goals": g.goals.len() }))
                    })
                },
            )
        }
        "target_set" | "target_remove" => {
            let key = a.need_str("key")?.to_string();
            let percent = a.f64("percent");
            let path = w.portfolio.clone();
            let p = path.clone();
            let summary = match percent {
                Some(pc) if name == "target_set" => format!("Set target {} to {}%", key, pc),
                _ => format!("Removed target {}", key),
            };
            let set = name == "target_set";
            w.apply(name, summary, &[path], move || {
                let _lock = crate::store::lock(&p)?;
                let mut pf = Portfolio::load_from(&p)?;
                let resolved = crate::commands::resolve_target_key(&pf, &key, m);
                if set {
                    pf.set_target(resolved.clone(), percent.unwrap_or(0.0));
                } else if !pf.remove_target(&resolved) {
                    bail!("No target for {}", resolved);
                }
                pf.save_to(&p)?;
                let total: f64 = pf.targets.values().sum();
                Ok(json!({ "key": resolved, "targets": pf.targets, "total_percent": total }))
            })
        }
        "profile_set" => {
            use crate::config::{ProfileMode, ProfileUpdate, RiskLevel, TaxRegime};
            let update = ProfileUpdate {
                mode: a
                    .str("mode")
                    .map(ProfileMode::parse)
                    .transpose()
                    .map_err(anyhow::Error::msg)?,
                risk: a
                    .str("risk")
                    .map(RiskLevel::parse)
                    .transpose()
                    .map_err(anyhow::Error::msg)?,
                tax_slab: a.f64("tax_slab"),
                tax_regime: a
                    .str("tax_regime")
                    .map(TaxRegime::parse)
                    .transpose()
                    .map_err(anyhow::Error::msg)?,
                horizon_years: a.i64("horizon_years").map(|v| v as u32),
                max_stock_weight: a.f64("max_stock_weight"),
                max_smallcap_weight: a.f64("max_smallcap_weight"),
                risk_per_trade: a.f64("risk_per_trade"),
                residence: None,
            };
            let given: Vec<&str> = [
                ("mode", update.mode.is_some()),
                ("risk", update.risk.is_some()),
                ("tax_slab", update.tax_slab.is_some()),
                ("tax_regime", update.tax_regime.is_some()),
                ("horizon_years", update.horizon_years.is_some()),
                ("max_stock_weight", update.max_stock_weight.is_some()),
                ("max_smallcap_weight", update.max_smallcap_weight.is_some()),
                ("risk_per_trade", update.risk_per_trade.is_some()),
            ]
            .into_iter()
            .filter(|(_, set)| *set)
            .map(|(n, _)| n)
            .collect();
            if given.is_empty() {
                bail!("Give at least one setting to change");
            }
            let path = w.config.clone();
            let p = path.clone();
            w.apply(
                name,
                format!("Updated profile: {}", given.join(", ")),
                &[path],
                move || {
                    crate::config::set_profile(&p, &update)?;
                    Ok(json!({ "updated": given }))
                },
            )
        }
        "profile_onboard" => {
            let answers = crate::onboarding::Answers {
                involvement: a.need_str("involvement")?.to_string(),
                fall_reaction: a.need_str("fall_reaction")?.to_string(),
                horizon_years: a
                    .i64("horizon_years")
                    .context("Missing argument 'horizon_years'")?
                    as u32,
                experience: a.str("experience").map(str::to_string),
                tax_slab: a.f64("tax_slab"),
                tax_regime: a.str("tax_regime").map(str::to_string),
            };
            let (update, why) = crate::onboarding::derive(&answers).map_err(anyhow::Error::msg)?;
            let summary = format!(
                "Set up the profile: {}:{}",
                update.mode.map_or("", |m| m.as_str()),
                update.risk.map_or("", |r| r.as_str())
            );
            let path = w.config.clone();
            let p = path.clone();
            w.apply(name, summary, &[path], move || {
                crate::config::reset_profile(&p).ok();
                crate::config::set_profile(&p, &update)?;
                let parsed: crate::config::Config =
                    toml::from_str(&std::fs::read_to_string(&p)?).unwrap_or_default();
                let profile =
                    crate::config::EffectiveProfile::from_parts(None, None, &parsed.profile);
                Ok(json!({ "profile": profile, "why": why }))
            })
        }
        "note_set" => {
            let sym = market::resolve_symbol(a.need_str("symbol")?.trim(), m).to_ascii_uppercase();
            let clear = a.bool("clear");
            let thesis = a.str("thesis").map(str::to_string);
            let (max_pe, min_roe, max_w) = (
                a.f64("max_pe"),
                a.f64("min_roe_pct"),
                a.f64("max_weight_pct"),
            );
            if !clear
                && thesis.is_none()
                && max_pe.is_none()
                && min_roe.is_none()
                && max_w.is_none()
            {
                bail!("Give a thesis or a limit to save, or clear to remove the note");
            }
            let path = w.file(crate::notes::FILE);
            let p = path.clone();
            let summary = if clear {
                format!("Removed the note on {}", sym)
            } else {
                format!("Saved a note on {}", sym)
            };
            w.apply(name, summary, &[path], move || {
                crate::store::update_at(&p, |n: &mut NoteStore| {
                    if clear {
                        if n.notes.remove(&sym).is_none() {
                            bail!("No note on {}", sym);
                        }
                        return Ok(json!({ "symbol": sym, "note": Value::Null }));
                    }
                    let note = n.notes.entry(sym.clone()).or_default();
                    if thesis.is_some() {
                        note.thesis = thesis;
                    }
                    if max_pe.is_some() {
                        note.max_pe = max_pe;
                    }
                    if min_roe.is_some() {
                        note.min_roe_pct = min_roe;
                    }
                    if max_w.is_some() {
                        note.max_weight_pct = max_w;
                    }
                    note.updated = today();
                    Ok(json!({ "symbol": sym, "note": note }))
                })
            })
        }
        "portfolio_add" | "portfolio_sell" => {
            if !w.allow_portfolio {
                bail!("Changing the portfolio's lots is off (start with --allow-portfolio)");
            }
            let sym = market::resolve_symbol(a.need_str("symbol")?.trim(), m);
            let shares = a.need_f64("shares")?;
            let price = a.need_f64("price")?;
            let date = date_arg(a)?;
            if name == "portfolio_add" {
                crate::commands::ensure_symbols_exist(src, std::slice::from_ref(&sym), m).await?;
            }
            let path = w.portfolio.clone();
            let p = path.clone();
            let add = name == "portfolio_add";
            let summary = format!(
                "{} {} {} at {} on {}",
                if add {
                    "Recorded a purchase of"
                } else {
                    "Recorded a sale of"
                },
                shares,
                sym,
                price,
                date
            );
            let record = move |pf: &mut Portfolio| -> Result<Value> {
                Ok(if add {
                    pf.add_lot(&sym, Lot::new(shares, price, &date, "assistant"));
                    json!({ "holding": pf.holding(&sym) })
                } else {
                    let sales = pf.sell_fifo(&sym, shares, price, &date, "assistant")?;
                    json!({
                        "lots_sold": sales.len(),
                        "realised_gain": sales.iter().map(|s| s.gain()).sum::<f64>(),
                        "holding": pf.holding(&sym),
                    })
                })
            };
            // Portfolio changes are previewed first: the user agrees to
            // the exact change before it is recorded.
            if !a.bool("confirm") {
                let mut pf = Portfolio::load_from(&p)?;
                let mut out = record(&mut pf)?;
                out["preview"] = json!(true);
                out["would_record"] = json!(summary.replacen("Recorded", "Record", 1));
                out["next"] = json!(format!(
                    "Nothing is recorded yet. Show this to the user; only if they agree, call {} again with the same arguments and confirm: true.",
                    name
                ));
                return Ok(out);
            }
            w.apply(name, summary, &[path], move || {
                let _lock = crate::store::lock(&p)?;
                let mut pf = Portfolio::load_from(&p)?;
                let out = record(&mut pf)?;
                pf.save_to(&p)?;
                Ok(out)
            })
        }
        other => bail!("Tool '{}' has no handler", other),
    }
}
