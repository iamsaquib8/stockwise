//! Tools for AI agents: `stockwise mcp` serves them over the Model Context
//! Protocol and `stockwise ask` hands them to the configured model.
//!
//! Each tool is a name, a description, a JSON-schema input built from a
//! [`Param`] list, and a handler that returns JSON from the same functions
//! the commands' `--json` output uses. Most only read. The workspace tools
//! (`watchlist_add`, `alert_add`, `goal_save`, ...) and the portfolio tools
//! (`portfolio_add`, `portfolio_sell`) change local files and are offered
//! only when the host turns them on; every change is journalled and can be
//! undone (see [`crate::audit`]). Nothing here places orders or prints.
//! Every result is wrapped in an envelope with a short "not financial
//! advice" note and kept small enough for a model's context.

mod args;
pub(crate) mod extra;
mod handlers;
mod more;
mod pulse;
pub mod schema;
mod workspace;

pub use args::Args;
pub use workspace::{Access, PORTFOLIO_TOOLS, WRITE_TOOLS, Workspace, access};

/// Drop the Yahoo session crumb from error text before it leaves the process.
pub(crate) fn redact(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(i) = rest.find("crumb=") {
        out.push_str(&rest[..i + "crumb=".len()]);
        out.push_str("REDACTED");
        rest = &rest[i + "crumb=".len()..];
        let end = rest.find(['&', ')', ' ']).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A tool error as a client or a cloud model sees it: without the Yahoo
/// session crumb, and without local file paths (those go to stderr, the
/// local log). Used by `stockwise mcp` and by `ask`, whose tool results can
/// reach a cloud provider.
pub(crate) fn client_error(e: &anyhow::Error) -> String {
    if e.downcast_ref::<crate::views::LocalData>().is_some() {
        eprintln!("stockwise: {:#}", e);
        "Could not read the local StockWise data (portfolio, watchlist or alerts); \
         the details were kept on this machine"
            .to_string()
    } else {
        redact(&format!("{:#}", e))
    }
}

use crate::market::Market;
use crate::views::DataSource;
use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

/// Sent with every tool result.
pub const NOTE: &str = "Educational market data from StockWise, not financial advice. \
Prices can be delayed; verify before acting.";

/// Largest serialized result handed to a model. Longer lists are cut down
/// (and the envelope says so) rather than flooding the context.
pub const MAX_RESULT_BYTES: usize = 24_000;

/// What a tool parameter accepts. The JSON schema and argument checks are
/// both derived from it, so they cannot drift apart.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Str,
    /// One or more strings (`["TCS", "INFY"]`); a single string is accepted too.
    StrList {
        max: usize,
    },
    Number {
        min: f64,
        max: f64,
    },
    Integer {
        min: i64,
        max: i64,
    },
    Bool,
    Enum(&'static [&'static str]),
    /// A market code (`in`, `us`, `crypto`, `fx`, `commodity`, `uk`, ...).
    Market,
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub description: &'static str,
    pub required: bool,
}

const fn req(name: &'static str, kind: Kind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        description,
        required: true,
    }
}

const fn opt(name: &'static str, kind: Kind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        description,
        required: false,
    }
}

/// Read tools that never leave the machine: calculators and local files.
/// Every other tool fetches market data (quotes, NAVs, statements) on its
/// default path, so hosts see it as open-world, personal or not.
pub const CLOSED_WORLD: &[&str] = &[
    "holdings_lots",
    "profile",
    "profile_questions",
    "changes_list",
    "goal_sip",
    "goal_retire",
    "goal_education",
    "goal_swp",
    "goal_prepay",
    "goal_emergency",
];

/// MCP tool annotations: what a host may assume before calling a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hints {
    /// Changes nothing (`readOnlyHint`).
    pub read_only: bool,
    /// May overwrite or remove the user's data (`destructiveHint`).
    pub destructive: bool,
    /// Calling it again with the same arguments changes nothing more
    /// (`idempotentHint`).
    pub idempotent: bool,
    /// Reaches outside the machine (`openWorldHint`).
    pub open_world: bool,
}

/// Read tools that also write a local record: `picks` appends its list to
/// the track record (`signal_log.json`), once a day per list, like
/// `stockwise picks`. Not read-only, but never destructive and idempotent.
pub const LOGGING_READS: &[&str] = &["picks"];

/// The annotations of `tool`. Read tools are read-only (except
/// [`LOGGING_READS`]) and idempotent. Write tools only touch
/// local files, so they are closed-world; see [`workspace::write_effect`]
/// for which overwrite or remove and which repeat safely.
pub fn hints(tool: &Tool) -> Hints {
    match access(tool.name) {
        Access::Read => Hints {
            read_only: !LOGGING_READS.contains(&tool.name),
            destructive: false,
            idempotent: true,
            open_world: !CLOSED_WORLD.contains(&tool.name),
        },
        Access::Workspace | Access::Portfolio => {
            let (destructive, idempotent) = workspace::write_effect(tool.name);
            Hints {
                read_only: false,
                destructive,
                idempotent,
                open_world: false,
            }
        }
    }
}

/// How long a read tool may run before it is stopped: the market scans and
/// portfolio analyses get longer. Writes are never cut off half-way.
pub fn default_timeout(name: &str) -> std::time::Duration {
    let secs = match name {
        "picks" | "picks_review" | "market_conditions" | "portfolio_analyze"
        | "portfolio_checkup" | "portfolio_pme" | "backtest" | "screen" | "verdicts" | "movers"
        | "ipo_analyze" | "mf_bleed" | "income_calendar" | "compare" | "returns" | "correlate"
        | "portfolio_wrap" | "portfolio_review" | "whats_hot" | "today" | "news_feed"
        | "digest" => 180,
        _ => 60,
    };
    std::time::Duration::from_secs(secs)
}

/// One tool: its name (`^[a-z_]+$`), a title for people, a description for
/// the model, and its parameters.
#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub params: &'static [Param],
    /// Reads the local portfolio, watchlist or alerts (as opposed to public
    /// market data). Agents' hosts may ask before sending those.
    pub personal: bool,
}

impl Tool {
    /// JSON schema of the arguments.
    pub fn input_schema(&self) -> Value {
        let mut props = Map::new();
        let mut required = Vec::new();
        for p in self.params {
            props.insert(p.name.into(), param_schema(p));
            if p.required {
                required.push(json!(p.name));
            }
        }
        json!({
            "type": "object",
            "properties": props,
            "required": required,
            "additionalProperties": false,
        })
    }
}

fn param_schema(p: &Param) -> Value {
    let mut s = match p.kind {
        Kind::Str => json!({"type": "string"}),
        Kind::StrList { max } => json!({
            "type": "array",
            "items": {"type": "string"},
            "minItems": 1,
            "maxItems": max,
        }),
        Kind::Number { min, max } => json!({"type": "number", "minimum": min, "maximum": max}),
        Kind::Integer { min, max } => json!({"type": "integer", "minimum": min, "maximum": max}),
        Kind::Bool => json!({"type": "boolean"}),
        Kind::Enum(values) => json!({"type": "string", "enum": values}),
        Kind::Market => json!({"type": "string", "enum": market_codes()}),
    };
    s["description"] = json!(p.description);
    s
}

/// Every `-m` value: `us`, `in`, `crypto`, `fx`, `commodity`, `uk`, ...
pub fn market_codes() -> Vec<String> {
    use clap::ValueEnum;
    Market::value_variants()
        .iter()
        .filter_map(|m| m.to_possible_value())
        .map(|v| v.get_name().to_string())
        .collect()
}

const UNIVERSE: Param = opt(
    "universe",
    Kind::Enum(&[
        "nifty50",
        "niftynext50",
        "nifty100",
        "banknifty",
        "niftyit",
        "nifty500",
        "midcap150",
        "smallcap250",
        "bse",
        "all",
    ]),
    "India only: an NSE index, `bse` (BSE main board) or `all` (every BSE stock) instead of the default (liquid BSE stocks)",
);

const MARKET: Param = opt(
    "market",
    Kind::Market,
    "Market for bare tickers: in (NSE, adds .NS), us, crypto, fx, commodity, or an exchange \
     (uk, de, jp, hk, sg, ca, au). Defaults to the user's configured market. Tickers with a \
     suffix (TCS.NS, 500325.BO, AAPL) are used as given.",
);
const SYMBOL: Param = req(
    "symbol",
    Kind::Str,
    "Ticker, e.g. RELIANCE, TCS, AAPL (with market=us), BTC (with market=crypto)",
);
const PERIOD: Param = opt(
    "period",
    Kind::Enum(&crate::PERIODS),
    "Look-back period (default 6mo for indicators, 1y for risk)",
);

const COUNTRY: Param = opt(
    "country",
    Kind::Enum(&["in", "us", "uk"]),
    "Tax rules: in (India), us or uk (default: from the market)",
);
const YEARS: Kind = Kind::Integer { min: 1, max: 60 };
const RETURN: Kind = Kind::Number {
    min: 0.0,
    max: 50.0,
};
const INFLATION: Kind = Kind::Number {
    min: 0.0,
    max: 30.0,
};
const STEP_UP: Param = opt(
    "step_up_pct",
    Kind::Number {
        min: 0.0,
        max: 50.0,
    },
    "Raise the SIP by this percent every year (default 0)",
);
const SAVINGS: Param = opt(
    "savings",
    Kind::Number {
        min: 0.0,
        max: 1e13,
    },
    "Money already invested towards the goal (default 0)",
);

const HORIZON_MONTHS: Param = opt(
    "months",
    Kind::Integer { min: 1, max: 720 },
    "Months to the goal, for horizons under or between whole years (give years, months or by)",
);
const HORIZON_BY: Param = opt(
    "by",
    Kind::Str,
    "Target date, YYYY-MM or YYYY-MM-DD (give years, months or by)",
);
const TAX_PARAMS: [Param; 3] = [
    opt(
        "tax",
        Kind::Enum(&["equity", "arbitrage", "debt", "none"]),
        "How the redemption is taxed (default: matched to the horizon; none outside India and the US)",
    ),
    opt(
        "slab_pct",
        Kind::Number {
            min: 0.0,
            max: 50.0,
        },
        "Income-tax slab rate in percent for debt funds and FDs (default 30; US 24)",
    ),
    opt(
        "exit_load_pct",
        Kind::Number {
            min: 0.0,
            max: 50.0,
        },
        "Exit load in percent on units held under 12 months (default 1 for equity in India)",
    ),
];
const NET: Param = opt(
    "net",
    Kind::Bool,
    "Treat the target as what must be left after tax and exit load",
);
const GLIDE: Param = opt(
    "glide_path",
    Kind::Bool,
    "Move to the short-term rate over the last 24 months (goals over 60 months)",
);

/// Every tool, in the order agents see them.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "quote",
        title: "Stock quotes",
        description: "Live (often delayed) quotes for up to 25 tickers in one call: price, change, \
                      day range, volume, 52-week range, market cap, P/E. Unknown tickers are listed \
                      in `missing`.",
        params: &[
            req(
                "symbols",
                Kind::StrList { max: 25 },
                "Tickers, e.g. [\"RELIANCE\", \"TCS\"]",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "history",
        title: "Price history",
        description: "OHLCV bars for one ticker over a period (5-minute bars for 1d, daily up to \
                      6mo, weekly to 2y, monthly beyond). Returns the most recent `limit` bars \
                      (fewer if the result would be too large) plus first/last close, high, low \
                      and the change over the whole period.",
        params: &[
            SYMBOL,
            PERIOD,
            opt(
                "limit",
                Kind::Integer { min: 1, max: 250 },
                "Most recent bars to return (default 60)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "technical",
        title: "Technical indicators",
        description: "Indicators for one ticker: SMA 10/20/50/100/200, EMA 12/26, RSI(14), MACD, \
                      Bollinger Bands, ATR, VWAP, and a bullish/bearish tally of the signals.",
        params: &[SYMBOL, PERIOD, MARKET],
        personal: false,
    },
    Tool {
        name: "risk",
        title: "Risk metrics",
        description: "Risk of one ticker from daily closes over a period: return, annualized \
                      volatility and risk level, Sharpe, Sortino and Calmar ratios, max drawdown \
                      (peak, trough, length) and 95%/99% one-day value at risk.",
        params: &[SYMBOL, PERIOD, MARKET],
        personal: false,
    },
    Tool {
        name: "verdict",
        title: "6-pillar scorecard",
        description: "Quantitative 6-pillar scorecard for one ticker: valuation, growth & \
                      profitability, balance sheet, smart money & ownership, analyst consensus \
                      and technical trend, each scored 0-100 with a band (strong, moderate, weak \
                      or no data) and the facts behind it, a weighted composite (0-100) and the \
                      share of the model's inputs that had data. Pillars without data are marked \
                      \"no data\" and left out of the composite; crypto, FX and indices get only \
                      the technical pillar. The scores themselves are not a recommendation; \
                      the `action` object is StockWise's rule-based call (see `stock_action`), \
                      to be quoted as returned, never changed.",
        params: &[SYMBOL, MARKET],
        personal: false,
    },
    Tool {
        name: "stock_action",
        title: "Rule-based action for a stock",
        description: "StockWise's deterministic, rule-based action for one stock: BUY, WATCH or \
                      AVOID when not held; ADD, HOLD, TRIM or EXIT when the local portfolio holds \
                      it; NO CALL when data coverage is below 60% (with the missing fields). \
                      Returns the kind, confidence, up to three reasons, the invalidation price (a \
                      daily close below it invalidates the action), review date, position-size \
                      hint, tax note, methodology version and disclaimer, plus the composite \
                      score and coverage behind it. Present the action exactly as returned; never \
                      change it or add price levels. Educational signal, not personalised advice.",
        params: &[
            SYMBOL,
            opt(
                "horizon",
                Kind::Enum(&["long", "short"]),
                "long (years, default) or short (weeks to months: tighter invalidation, 10-session review)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "portfolio_review",
        title: "Rule-based review of holdings",
        description: "The rule-based action for every holding of the local portfolio, sorted EXIT, \
                      TRIM, ADD, HOLD, then NO CALL: each with confidence, reasons, invalidation \
                      price, review date, weight, P&L, the shares a TRIM sells to get back under \
                      the profile's max_stock_weight (look-through via funds included) and a tax \
                      note when waiting a few days for long-term rates saves tax. Quote every \
                      action as returned; never change one. Educational signal, not personalised \
                      advice.",
        params: &[
            opt(
                "horizon",
                Kind::Enum(&["long", "short"]),
                "long (years, default) or short (weeks to months)",
            ),
            opt(
                "base",
                Kind::Str,
                "Currency for totals, a 3-letter code (default: the market's)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "scorecard",
        title: "Piotroski F-Score & Altman Z-Score",
        description: "Audit-grade accounting scorecards for one ticker: Piotroski 9-Point F-Score \
                      (Profitability, Leverage/Liquidity, Operating Efficiency) and Altman Z-Score \
                      (Safe, Grey, Distress zones) with every criterion's formula and pass/fail.",
        params: &[SYMBOL, MARKET],
        personal: false,
    },
    Tool {
        name: "insights",
        title: "Fundamental & ownership insights",
        description: "Categorized positive, negative and neutral impact cards (Valuation, Growth, \
                      Margins, Balance Sheet, Ownership, Technical, Dividend, Risk) plus \
                      shareholding breakdown (Promoter, FII, DII, Public) for one ticker.",
        params: &[SYMBOL, MARKET],
        personal: false,
    },
    Tool {
        name: "events",
        title: "Corporate events & disclosures",
        description: "Recent and upcoming corporate events for one ticker: quarterly results, \
                      dividends, splits, bonuses, board meetings and exchange disclosures, with \
                      1-day and 5-day price reaction around each event.",
        params: &[
            SYMBOL,
            opt(
                "days",
                Kind::Integer { min: 7, max: 365 },
                "Look-back window in days for past events (default 90)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "quarterly_results",
        title: "Quarterly and annual financial results",
        description: "Last 8 or more quarters (or fiscal years with annual=true) of reported \
                      results for one ticker: revenue, operating profit and margin, other income, \
                      interest, depreciation, pre-tax profit, tax rate, net profit and diluted \
                      EPS, with year-on-year and quarter-on-quarter growth, margin change in \
                      basis points, a trailing-twelve-month column and EPS against consensus. \
                      Banks add net interest income and gross/net NPA when filed. Annual adds \
                      cash flow, ROE, ROCE and 3/5/10-year CAGR. Records source and currency.",
        params: &[
            SYMBOL,
            opt(
                "quarters",
                Kind::Integer { min: 1, max: 40 },
                "Quarters (or fiscal years with annual) to return, default 8",
            ),
            opt(
                "annual",
                Kind::Bool,
                "Fiscal years with cash flow and returns instead of quarters",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "search",
        title: "Find a ticker",
        description: "Search tickers by company name (\"tata motors\", \"apple\"). Use before \
                      quote when unsure of the exact symbol.",
        params: &[
            req("query", Kind::Str, "Company name or partial ticker"),
            opt(
                "limit",
                Kind::Integer { min: 1, max: 20 },
                "Most results to return (default 10)",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "markets",
        title: "Market indices",
        description: "Index levels and day change: the chosen market first (NIFTY 50, SENSEX, \
                      BANK NIFTY, India 10Y yield; or S&P 500, Nasdaq, Dow), then the other; \
                      optional crypto, FX and one benchmark per global exchange.",
        params: &[
            MARKET,
            opt("crypto", Kind::Bool, "Add the main cryptocurrencies"),
            opt("fx", Kind::Bool, "Add major currency pairs"),
            opt(
                "global",
                Kind::Bool,
                "Add each global exchange's main index",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "mf_search",
        title: "Find a mutual fund",
        description: "Search Indian mutual fund schemes (AMFI) by name words, e.g. \"parag flexi \
                      direct\". Returns scheme codes, names, latest NAV and date.",
        params: &[
            req("query", Kind::Str, "Words from the scheme name"),
            opt(
                "plan",
                Kind::Enum(&["direct", "regular", "all"]),
                "Direct or regular plans (default all)",
            ),
            opt(
                "limit",
                Kind::Integer { min: 1, max: 25 },
                "Most schemes to return (default 10)",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "mf_nav",
        title: "Mutual fund NAV and returns",
        description: "Latest NAV of an Indian mutual fund scheme by AMFI code or ISIN, with its \
                      category, fund house and expense ratio (TER) from AMFI.",
        params: &[req(
            "scheme",
            Kind::Str,
            "AMFI scheme code (e.g. 122639) or ISIN",
        )],
        personal: false,
    },
    Tool {
        name: "mf_sip",
        title: "Mutual fund SIP returns",
        description: "What a monthly SIP in an Indian mutual fund would have returned over a \
                      period: invested, current value, gain and XIRR.",
        params: &[
            req("scheme", Kind::Str, "AMFI scheme code or ISIN"),
            req(
                "monthly",
                Kind::Number {
                    min: 100.0,
                    max: 10_000_000.0,
                },
                "Monthly instalment in rupees",
            ),
            opt(
                "period",
                Kind::Enum(&["1y", "2y", "3y", "5y", "7y", "10y", "15y", "max"]),
                "How far back the SIP starts (default 5y)",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "options_chain",
        title: "Option chain",
        description: "Option chain for an index or stock (NSE/BSE for India, Yahoo for US): \
                      strikes around the money with calls and puts, OI, volume, IV and greeks. \
                      Returns `strikes` strikes each side of the money (fewer if the result \
                      would be too large; `strikes_each_side` says how many).",
        params: &[
            req(
                "symbol",
                Kind::Str,
                "Underlying: NIFTY, BANKNIFTY, FINNIFTY, SENSEX, a stock (RELIANCE), or a US ticker with market=us",
            ),
            opt(
                "expiry",
                Kind::Str,
                "Expiry date YYYY-MM-DD (default nearest)",
            ),
            opt(
                "strikes",
                Kind::Integer { min: 1, max: 20 },
                "Strikes each side of the money (default 6)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "options_pcr",
        title: "Put/call ratio",
        description: "Put/call ratio by open interest and volume for an underlying's expiry, \
                      with total call and put OI.",
        params: &[
            req("symbol", Kind::Str, "Underlying, e.g. NIFTY or BANKNIFTY"),
            opt(
                "expiry",
                Kind::Str,
                "Expiry date YYYY-MM-DD (default nearest)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "options_oi",
        title: "Open-interest analytics & build-up",
        description: "Open-interest analytics for an index or F&O stock: Put/Call Ratio (OI & \
                      volume), Max Pain strike, top Call resistance and Put support OI walls, and \
                      institutional build-up classification (Long Build-up, Short Covering, \
                      Short Build-up, Long Unwinding).",
        params: &[
            req(
                "symbol",
                Kind::Str,
                "Underlying, e.g. NIFTY, BANKNIFTY, RELIANCE, or AAPL with market=us",
            ),
            opt(
                "expiry",
                Kind::Str,
                "Expiry date YYYY-MM-DD (default nearest)",
            ),
            opt(
                "top",
                Kind::Integer { min: 1, max: 15 },
                "Top N open-interest strikes per side (default 5)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "options_maxpain",
        title: "Max pain",
        description: "Max-pain strike for an underlying's expiry (where option writers lose \
                      least) and its distance from the spot.",
        params: &[
            req("symbol", Kind::Str, "Underlying, e.g. NIFTY"),
            opt(
                "expiry",
                Kind::Str,
                "Expiry date YYYY-MM-DD (default nearest)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "options_payoff",
        title: "Option strategy payoff",
        description: "Payoff of a preset option strategy at expiry from live premiums: max \
                      profit and loss, breakevens, probability of profit and a payoff table.",
        params: &[
            req("symbol", Kind::Str, "Underlying, e.g. NIFTY"),
            req(
                "strategy",
                Kind::Enum(&[
                    "bull-call",
                    "bear-put",
                    "straddle",
                    "strangle",
                    "iron-condor",
                    "butterfly",
                    "covered-call",
                ]),
                "Preset strategy",
            ),
            opt(
                "width",
                Kind::Number {
                    min: 0.01,
                    max: 100_000.0,
                },
                "Distance between strikes in points (default: two strike steps)",
            ),
            opt(
                "expiry",
                Kind::Str,
                "Expiry date YYYY-MM-DD (default nearest)",
            ),
            opt(
                "lots",
                Kind::Integer { min: 1, max: 100 },
                "Number of lots (default 1)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "screen",
        title: "Stock screen",
        description: "Run a preset screen over the market's popular stocks: undervalued \
                      (P/E under 15), growth (forward EPS above trailing), dividend (yield over \
                      2%), momentum (top daily gainers) or bluechip (largest market caps). \
                      Returns matches with price, P/E, market cap and yield.",
        params: &[
            req(
                "category",
                Kind::Enum(crate::market::SCREENER_CATEGORIES),
                "Preset screen",
            ),
            opt(
                "limit",
                Kind::Integer { min: 1, max: 50 },
                "Most stocks to return (default 20)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "backtest",
        title: "Backtest a strategy",
        description: "Backtest a built-in strategy on one ticker's daily history with Indian or \
                      US trading costs: total return vs buy-and-hold, CAGR, max drawdown, Sharpe, \
                      win rate, trades. Past results do not predict future returns.",
        params: &[
            SYMBOL,
            opt(
                "strategy",
                Kind::Str,
                "Built-in strategy (default rsi): rsi, macd, sma, bb, breakout, mr, trend_pullback, vwap",
            ),
            opt(
                "period",
                Kind::Enum(&["6mo", "1y", "2y", "5y", "10y", "max"]),
                "History to test on (default 2y)",
            ),
            opt(
                "capital",
                Kind::Number {
                    min: 1000.0,
                    max: 1e10,
                },
                "Starting capital (default 100000)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "portfolio_summary",
        title: "Portfolio summary",
        description: "The user's StockWise portfolio valued at current prices: each holding's \
                      quantity, average cost, price, value, P&L and weight, totals and XIRR. \
                      Holdings without a price are listed in `unpriced`, never valued at 0.",
        params: &[opt(
            "base",
            Kind::Enum(&["inr", "usd"]),
            "Currency for the totals when holdings mix currencies",
        )],
        personal: true,
    },
    Tool {
        name: "profile",
        title: "Investor profile & personal guardrails",
        description: "The user's active investor profile mode (passive, active, ultra), risk level, \
                      tax parameters (regime, slab rate), horizon, and portfolio guardrails (max \
                      stock weight, max small-cap weight, risk per trade).",
        params: &[],
        personal: true,
    },
    Tool {
        name: "profile_questions",
        title: "Profile onboarding questions",
        description: "The few plain questions that set up the user's investor profile (how closely they follow investments, how they react to a fall, horizon, experience, tax slab and regime), with the answer choices. Ask them one at a time; with workspace changes on, send the answers to profile_onboard.",
        params: &[],
        personal: false,
    },
    Tool {
        name: "holdings_lots",
        title: "Holding lots",
        description: "The user's purchase lots (quantity, price, date, days held, short or long \
                      term) for one holding or all, oldest first — what FIFO sales and tax use — \
                      and their most recent sales.",
        params: &[
            opt("symbol", Kind::Str, "Only this holding"),
            opt(
                "limit",
                Kind::Integer { min: 1, max: 200 },
                "Most lots to return (default 100)",
            ),
            COUNTRY,
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "tax_estimate",
        title: "Capital gains estimate",
        description: "Estimated capital-gains tax for the user's portfolio: realized gains this \
                      tax year and unrealized gains by short/long term, with India's set-off \
                      rules and the ₹1.25 lakh LTCG exemption (or US/UK rules). An estimate to \
                      check against the broker's tax P&L.",
        params: &[COUNTRY],
        personal: true,
    },
    Tool {
        name: "tax_advance",
        title: "Indian advance tax schedule",
        description: "Quarterly Indian advance tax schedule (15 Jun, 15 Sep, 15 Dec, 15 Mar) from \
                      the user's realized capital gains under Section 208/211, with Section \
                      234C(1) capital-gains proviso protections and shortfall interest estimate.",
        params: &[
            opt(
                "fy",
                Kind::Str,
                "Indian financial year, e.g. 2025-26 or AY2026-27 (default current FY)",
            ),
            opt(
                "paid",
                Kind::Number {
                    min: 0.0,
                    max: 1e11,
                },
                "Advance tax already paid for this financial year in INR (default 0). Treated as paid before every due date",
            ),
        ],
        personal: true,
    },
    Tool {
        name: "harvest",
        title: "Tax-loss & LTCG exemption harvesting",
        description: "Lot-level tax-loss harvesting candidates (short-term and long-term \
                      unrealized losses and estimated tax offset) plus unused Section 112A ₹1.25L \
                      annual LTCG tax-gain harvesting capacity for the user's portfolio.",
        params: &[MARKET],
        personal: true,
    },
    Tool {
        name: "rebalance",
        title: "Portfolio rebalance plan",
        description: "Rebalance plan for the user's portfolio against their saved target weights \
                      (symbols, sectors or asset classes; equal weight without any): current vs \
                      target weight, drift tolerance band, whole-share trades whose cash \
                      balances, the lots each sell takes, its STCG/LTCG tax and trade costs. \
                      `inflow` invests new cash buy-only (add `with_sells` for a full \
                      rebalance); `raise` sells only, at the lowest tax, to raise cash.",
        params: &[
            opt(
                "inflow",
                Kind::Number {
                    min: 1.0,
                    max: 1e11,
                },
                "Fresh cash inflow for buy-only rebalancing without selling",
            ),
            opt(
                "with_sells",
                Kind::Bool,
                "With inflow: also sell overweight holdings (a full rebalance)",
            ),
            opt(
                "raise",
                Kind::Number {
                    min: 1.0,
                    max: 1e11,
                },
                "Sell only, to raise this much cash at the lowest tax",
            ),
            opt(
                "cash_buffer",
                Kind::Number {
                    min: 0.0,
                    max: 1e11,
                },
                "Cash to keep uninvested",
            ),
            opt(
                "drift",
                Kind::Number {
                    min: 0.0,
                    max: 50.0,
                },
                "Drift tolerance band in percent, e.g. 2.0 for +/-2%",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "portfolio_pme",
        title: "Public Market Equivalent (Index Twin)",
        description: "Public Market Equivalent (PME) / Index Twin analysis: mirrors the user's \
                      exact historical portfolio cash flows (buys, sells, dividends) on matching \
                      dates into a passive benchmark (e.g. SPY, VOO, VWCE, NIFTY, NIFTYBEES). \
                      Computes Kaplan-Schoar PME, Direct Alpha, Index Twin terminal value, and \
                      exact net currency value added/lost vs the passive index.",
        params: &[
            opt(
                "benchmark",
                Kind::Str,
                "Benchmark ticker or index (e.g. SPY, VOO, VWCE, NIFTY, NIFTYBEES, ^GSPC)",
            ),
            opt(
                "base",
                Kind::Enum(&["inr", "usd", "eur", "gbp"]),
                "Currency for cash flows and valuation (default: portfolio base currency)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "portfolio_sell_preview",
        title: "Pre-sell lot and tax preview",
        description: "Preview selling a held symbol without recording anything: the FIFO lots \
                      that would depart (date, days held, quantity, cost, gain or loss), \
                      short- vs long-term split, capital-gains tax with cess after the \
                      remaining Sec. 112A exemption, lots within 45 days of turning long-term \
                      with the tax difference of waiting, and a mutual-fund exit-load check. \
                      Tax facts and arithmetic only. Never changes the portfolio.",
        params: &[
            req("symbol", Kind::Str, "Held ticker or MF:<AMFI code>"),
            req(
                "quantity",
                Kind::Number {
                    min: 0.000_001,
                    max: 1e12,
                },
                "Shares or units to sell",
            ),
            opt(
                "price",
                Kind::Number {
                    min: 0.0,
                    max: 1e12,
                },
                "Sale price per share or unit (default: live price or latest AMFI NAV)",
            ),
            opt(
                "date",
                Kind::Str,
                "Sale date YYYY-MM-DD (default today; a past date needs price)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "mf_bleed",
        title: "Regular vs Direct fund cost",
        description: "For held Regular-plan mutual funds: the Direct growth twin, the TER gap \
                      from AMFI's disclosure, yearly rupee cost of the gap, its 10-year \
                      compounded effect at a stated gross return, and the FIFO split of units \
                      past 12 months (units that redeem inside the unused Sec. 112A exemption) \
                      versus younger units with tax, exit load and breakeven months.",
        params: &[
            opt(
                "return_pct",
                Kind::Number {
                    min: 0.0,
                    max: 30.0,
                },
                "Assumed gross annual return in percent for the 10-year figure (default 10)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "income_calendar",
        title: "Dividend, coupon and passive-income calendar",
        description: "12-month trailing and projected dividend, coupon and passive-income calendar, \
                      Yield-on-Cost (YoC), and dividend payout safety checks.",
        params: &[
            opt(
                "months",
                Kind::Integer { min: 1, max: 60 },
                "Months to forecast (default 12)",
            ),
            opt(
                "summary_only",
                Kind::Bool,
                "Omit per-month cashflow items and per-lot yield rows (default false)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "calendar",
        title: "Results and dividends calendar",
        description: "Upcoming results dates, ex-dividend dates and splits for given tickers, or \
                      for the user's holdings and watchlist when none are given.",
        params: &[
            opt(
                "symbols",
                Kind::StrList { max: 30 },
                "Tickers (default: holdings and watchlist)",
            ),
            opt(
                "days",
                Kind::Integer { min: 1, max: 365 },
                "Look this many days ahead (default 30)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "ipo_board",
        title: "Indian IPO board",
        description: "Indian IPOs from NSE: issues open for bidding now, upcoming issues and \
                      recently listed ones, each with price band, lot size, minimum application, \
                      issue size, live QIB/NII/retail subscription and a quick signal, plus an \
                      IPO-market regime (share of recent listings above issue price, NIFTY, VIX). \
                      Mainboard and SME issues are labelled.",
        params: &[
            opt(
                "board",
                Kind::Enum(&["all", "mainboard", "sme"]),
                "Which issues to list (default all)",
            ),
            opt(
                "recent",
                Kind::Integer { min: 1, max: 20 },
                "Recently closed or listed issues to include (default 6)",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "ipo_analyze",
        title: "Indian IPO analysis",
        description: "Deep analysis of one Indian IPO by NSE symbol or company name: issue terms \
                      (fresh issue vs offer for sale, anchor, lead managers), subscription by \
                      category (QIB with mutual-fund share, bNII, sNII, retail and cut-off bids), \
                      demand by price, SEBI allotment odds for retail, sNII and bNII (and the odds \
                      at the `capital` amount when given), unofficial GMP from the press, and a \
                      6-pillar score with demand and long-term profile labels. Describes the \
                      data; not a recommendation.",
        params: &[
            req(
                "symbol",
                Kind::Str,
                "NSE IPO symbol (e.g. BCSSL) or part of the company name",
            ),
            opt(
                "board",
                Kind::Enum(&["all", "mainboard", "sme"]),
                "Restrict the match to the mainboard or SME (default all)",
            ),
            opt(
                "capital",
                Kind::Number {
                    min: 1_000.0,
                    max: 1e10,
                },
                "Rupees available to apply with, to compare retail, sNII and bNII odds",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "goal_sip",
        title: "SIP for a goal",
        description: "Monthly SIP needed to reach a target amount by a horizon (years, months or a \
                      date). Without a return it uses a horizon-matched baseline (capital \
                      preservation to 24 months, conservative hybrid to 60, long-term growth \
                      beyond) and estimates tax and exit load at redemption, the SIP needed to \
                      receive the target after tax, P10/P50/P90 outcome bands, the chance of \
                      reaching the target and a return sensitivity. Amounts in the market's \
                      currency (₹ for India).",
        params: &[
            req(
                "target",
                Kind::Number {
                    min: 1.0,
                    max: 1e13,
                },
                "Target amount (1 crore = 10000000, 1 lakh = 100000)",
            ),
            opt(
                "years",
                YEARS,
                "Years to the goal (give years, months or by)",
            ),
            HORIZON_MONTHS,
            HORIZON_BY,
            opt(
                "return_pct",
                RETURN,
                "Expected yearly return in percent (default: matched to the horizon)",
            ),
            opt(
                "inflation_pct",
                INFLATION,
                "Yearly inflation in percent; the target is then in today's money (default 0)",
            ),
            STEP_UP,
            SAVINGS,
            TAX_PARAMS[0],
            TAX_PARAMS[1],
            TAX_PARAMS[2],
            NET,
            GLIDE,
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "goal_retire",
        title: "Retirement corpus",
        description: "Corpus needed to retire on today's monthly expenses (grown by inflation) \
                      for a number of years, and the monthly SIP that builds it.",
        params: &[
            req(
                "monthly_expense",
                Kind::Number {
                    min: 1.0,
                    max: 1e10,
                },
                "Monthly expenses in today's money",
            ),
            req("years_to_retire", YEARS, "Years until retirement"),
            req("years_in_retirement", YEARS, "Years the corpus must last"),
            opt(
                "inflation_pct",
                INFLATION,
                "Yearly inflation of expenses in percent (default 6)",
            ),
            opt(
                "return_pre_pct",
                RETURN,
                "Yearly return until retirement in percent (default 12)",
            ),
            opt(
                "return_post_pct",
                RETURN,
                "Yearly return during retirement in percent (default 7)",
            ),
            opt(
                "monthly_income",
                Kind::Number {
                    min: 0.0,
                    max: 1e10,
                },
                "Pension, rent or other income in today's money that offsets expenses (default 0)",
            ),
            opt(
                "buffer_years",
                Kind::Integer { min: 0, max: 40 },
                "Extra years the corpus must last (default 0)",
            ),
            opt(
                "legacy",
                Kind::Number {
                    min: 0.0,
                    max: 1e13,
                },
                "Amount to leave at the end, in today's money (default 0)",
            ),
            STEP_UP,
            SAVINGS,
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "goal_education",
        title: "Education goal",
        description: "Future cost of an education from today's cost and education inflation, \
                      and the monthly SIP that pays for it, with the same horizon-matched return, \
                      tax, outcome bands and sensitivity as goal_sip.",
        params: &[
            req(
                "cost",
                Kind::Number {
                    min: 1.0,
                    max: 1e12,
                },
                "Cost in today's money",
            ),
            opt(
                "years",
                YEARS,
                "Years until the money is needed (give years, months or by)",
            ),
            HORIZON_MONTHS,
            HORIZON_BY,
            opt(
                "inflation_pct",
                INFLATION,
                "Yearly education inflation in percent (default 8)",
            ),
            opt(
                "return_pct",
                RETURN,
                "Expected yearly return in percent (default: matched to the horizon)",
            ),
            STEP_UP,
            SAVINGS,
            TAX_PARAMS[0],
            TAX_PARAMS[1],
            TAX_PARAMS[2],
            NET,
            GLIDE,
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "goal_swp",
        title: "SWP runway",
        description: "How long a corpus lasts under monthly withdrawals that rise with inflation: \
                      an equity SWP (tax on the gain share only) against an FD (interest at the \
                      slab rate), with the initial withdrawal rate.",
        params: &[
            req(
                "corpus",
                Kind::Number {
                    min: 1.0,
                    max: 1e13,
                },
                "Corpus to withdraw from",
            ),
            req(
                "withdrawal",
                Kind::Number {
                    min: 1.0,
                    max: 1e10,
                },
                "Monthly withdrawal in today's money",
            ),
            opt("years", YEARS, "Years to look ahead (default 30)"),
            opt(
                "inflation_pct",
                INFLATION,
                "Yearly rise of the withdrawal in percent (default 6)",
            ),
            opt(
                "return_pct",
                RETURN,
                "Yearly return of the corpus in percent (default 10)",
            ),
            opt(
                "fd_return_pct",
                RETURN,
                "FD interest rate in percent for the comparison (default 7)",
            ),
            opt(
                "slab_pct",
                Kind::Number {
                    min: 0.0,
                    max: 50.0,
                },
                "Income-tax slab rate in percent (default 30)",
            ),
            opt(
                "cost",
                Kind::Number {
                    min: 0.0,
                    max: 1e13,
                },
                "What the corpus cost, when known (default: its value)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "goal_prepay",
        title: "Loan prepayment vs SIP",
        description: "Compare prepaying a loan (extra monthly amount and/or a lump sum) with \
                      investing the same money in a SIP: exact interest and tenure saved, \
                      P10/P50/P90 after-tax SIP outcomes, the chance the SIP wins and the \
                      before-tax return it needs to break even.",
        params: &[
            req(
                "principal",
                Kind::Number {
                    min: 1.0,
                    max: 1e13,
                },
                "Loan principal outstanding",
            ),
            req(
                "loan_rate_pct",
                Kind::Number {
                    min: 0.0,
                    max: 50.0,
                },
                "Loan interest rate in percent a year",
            ),
            req(
                "remaining_months",
                Kind::Integer { min: 1, max: 720 },
                "Months left on the loan",
            ),
            opt(
                "extra",
                Kind::Number {
                    min: 0.0,
                    max: 1e10,
                },
                "Extra amount every month (give extra and/or lump_sum)",
            ),
            opt(
                "lump_sum",
                Kind::Number {
                    min: 0.0,
                    max: 1e13,
                },
                "One-off amount today (give extra and/or lump_sum)",
            ),
            opt(
                "return_pct",
                RETURN,
                "Yearly SIP return in percent (default: matched to the remaining term)",
            ),
            TAX_PARAMS[0],
            TAX_PARAMS[1],
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "goal_list",
        title: "Saved goals",
        description: "The user's saved goals (goals.json) with months left, target after \
                      inflation, value of the linked holdings (live prices, or cost basis offline: see \
                      `valued_at`), funded share and the monthly SIP still needed.",
        params: &[MARKET],
        personal: true,
    },
    Tool {
        name: "alerts_list",
        title: "Price alerts",
        description: "The user's alerts (price, RSI, moving-average crosses, breakouts, results \
                      dates) checked against live data: triggered, waiting, no_price or no_data.",
        params: &[],
        personal: true,
    },
    Tool {
        name: "watchlist",
        title: "Watchlist",
        description: "The user's watchlist with live prices and day change.",
        params: &[],
        personal: true,
    },
    Tool {
        name: "portfolio_analyze",
        title: "Portfolio analysis",
        description: "The portfolio dashboard: every holding's weight, P&L, today's move, gain by tax term and next lot to turn long-term, place against its 200-day average and 52-week range, volatility, beta and long-term score; the portfolio by market (with each market's own-currency total), currency, cap tier and sector; concentration and risk; and neutral observations with their figures.",
        params: &[
            opt(
                "quick",
                Kind::Bool,
                "Skip the year of prices per holding (no risk figures; faster)",
            ),
            opt(
                "base",
                Kind::Str,
                "Currency for totals, a 3-letter code (default: the market's)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "portfolio_checkup",
        title: "Portfolio checkup",
        description: "A scan of the portfolio for readings worth a look, in six lenses: concentration (single-stock look-through weight and small-cap share against the profile's limits, fund overlap, effective number of bets), forensic flags (Piotroski, Altman Z, cash conversion, promoter pledge and stake, latest-quarter contraction), valuation stretch, downside (beta, drawdown, correlated pairs), fees and tax (unused long-term exemption, harvestable losses, Regular-plan cost, lots turning long-term) and goals and targets. Each finding has what was observed, the amount at stake and the command to inspect it, ranked by position value times severity. Observations only, never advice.",
        params: &[
            opt(
                "offline",
                Kind::Bool,
                "Use only local data and the cache (no statements or fund costs)",
            ),
            opt(
                "base",
                Kind::Str,
                "Currency for totals, a 3-letter code (default: the market's)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "market_conditions",
        title: "Market conditions",
        description: "Where a stock market stands, as readings: the main index against its 200- and 50-day averages and highs, India VIX or VIX against 5 years, breadth (share of stocks above their averages), and valuation (aggregate P/E, earnings yield against the 10-year bond yield), with how today ranks against earlier saved sessions. Readings, not a signal to invest or wait.",
        params: &[UNIVERSE, MARKET],
        personal: false,
    },
    Tool {
        name: "picks",
        title: "Highest-scoring stocks",
        description: "Stocks of the market that clear size and liquidity floors, ranked against each other on five pillars (value, quality, growth, trend, risk), each a percentile among the stocks scanned, with the share of the score that had data. Losses and negative book value rank last; implausible figures are listed separately to check; at most 3 per sector.",
        params: &[
            UNIVERSE,
            opt(
                "cap",
                Kind::Enum(&["mega", "large", "mid", "small", "micro"]),
                "Rank only this market-cap tier",
            ),
            opt(
                "top",
                Kind::Integer { min: 1, max: 50 },
                "How many of the highest scores (default 10)",
            ),
            opt(
                "horizon",
                Kind::Enum(&["long", "short"]),
                "long (years: value, quality, growth lead; default) or short (weeks to months: momentum, volume, risk lead)",
            ),
            opt(
                "min_mcap",
                Kind::Number { min: 0.0, max: 1e7 },
                "Least market cap: crore in India, millions elsewhere. Defaults for the default tier: India 500, US 1000; `cap` small: India 500, US 300; micro: India 100, US 50; other exchanges: none. 0 turns the floor off",
            ),
            opt(
                "min_adv",
                Kind::Number { min: 0.0, max: 1e6 },
                "Least 3-month average value traded a day: crore in India, millions elsewhere. Defaults for the default tier: India 2, US 5; `cap` small: India 1, US 2; micro: India 0.5, US 0.5; other exchanges: none. 0 turns the floor off",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "picks_review",
        title: "Track record of picks",
        description: "How past `picks` lists did: every list (from the CLI or this tool) is logged, and each stock is measured from the day it was first listed against NIFTY 50 or the S&P 500 total return, with hit rate, share that beat the index and mean and median return and excess. Price return, dividends not included; past lists do not predict future ones.",
        params: &[
            opt(
                "since",
                Kind::Str,
                "Look-back window: 90d (default), 12w, 6m, 1y or a number of days",
            ),
            opt(
                "horizon",
                Kind::Enum(&["long", "short"]),
                "Which lists: long (default) or short",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "verdicts",
        title: "6-pillar scores for several stocks",
        description: "The verdict tool's 6-pillar scores for up to 25 tickers in one call, compact: composite (0-100), data coverage (percent of inputs with data), `low_coverage` when under 60%, and each pillar's score and band. Stocks that fail are listed in `failed`. Describes data; it is not a recommendation.",
        params: &[
            req(
                "symbols",
                Kind::StrList { max: 25 },
                "Tickers, e.g. [\"TCS\", \"INFY\", \"HDFCBANK\"]",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "movers",
        title: "Top gainers and losers",
        description: "Today's biggest gainers and losers among the market's large stocks (NIFTY 500, the S&P 1500, or the exchange's main index), with price, change and volume.",
        params: &[
            opt(
                "count",
                Kind::Integer { min: 1, max: 25 },
                "How many on each side (default 5)",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "dividends",
        title: "Dividend yield and income",
        description: "A stock's trailing dividend yield, the yearly income it would pay on a few invested amounts, and the 10-year growth with dividends reinvested at the current yield.",
        params: &[SYMBOL, MARKET],
        personal: false,
    },
    Tool {
        name: "goal_emergency",
        title: "Emergency fund",
        description: "The emergency reserve to keep (monthly expenses × months of cover), how much of it the user's liquid money already covers, and the monthly saving that builds the rest over a horizon.",
        params: &[
            req(
                "monthly_expense",
                Kind::Number {
                    min: 1.0,
                    max: 1.0e9,
                },
                "Monthly expenses in the market's currency",
            ),
            opt(
                "months_cover",
                Kind::Integer { min: 1, max: 36 },
                "Months of expenses to cover (default 6)",
            ),
            opt(
                "liquid",
                Kind::Number {
                    min: 0.0,
                    max: 1.0e12,
                },
                "Money already set aside",
            ),
            opt(
                "horizon_months",
                Kind::Integer { min: 1, max: 120 },
                "Months to build it over (default 12)",
            ),
            opt(
                "return_pct",
                Kind::Number {
                    min: 0.0,
                    max: 20.0,
                },
                "Yearly return on the reserve",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "fit_check",
        title: "Does a stock fit my rules?",
        description: "Yes or no: whether a stock meets the user's own rules: the investor profile's largest weight for one stock and for small and micro caps (with an amount, after buying it), the risk level the profile allows, and the P/E, ROE and weight limits the user saved in a note on the stock. Lists each rule passed, failed or without data. Measured against the user's rules; not a recommendation.",
        params: &[
            SYMBOL,
            opt(
                "amount",
                Kind::Number {
                    min: 0.0,
                    max: 1.0e12,
                },
                "Amount the user is considering adding, in the portfolio's currency",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "changes_list",
        title: "Changes made by assistants",
        description: "The workspace changes made through StockWise's assistant tools (watchlist, alerts, goals, targets, profile, notes, portfolio), newest first, with each change's id for `stockwise undo`.",
        params: &[opt(
            "limit",
            Kind::Integer { min: 1, max: 100 },
            "How many (default 20)",
        )],
        personal: true,
    },
    // India-first extras; handlers in more.rs.
    Tool {
        name: "bonds_yields",
        title: "Indian government bond yields",
        description: "India's government securities yield curve: T-bill and G-sec yields by tenor (91 days to 40 years) with the benchmark security, its yield to maturity and the date, from CCIL (RBI's list when CCIL is down).",
        params: &[],
        personal: false,
    },
    Tool {
        name: "bonds_list",
        title: "Find an Indian government security",
        description: "RBI's list of outstanding central government securities: ISIN, name (e.g. 7.18% GS 2033), coupon, maturity, the NSE symbol and the amount outstanding in crore. Filter by words of the name, an ISIN or a maturity year.",
        params: &[opt(
            "query",
            Kind::Str,
            "Words, an ISIN or a maturity year, e.g. \"2033\", \"718GS2033\"",
        )],
        personal: false,
    },
    Tool {
        name: "bonds_calc",
        title: "G-sec price and yield calculator",
        description: "Price to yield or yield to price for an Indian government security (semi-annual coupons, 30/360) or a T-bill: clean and dirty price, accrued interest, yield to maturity, modified duration, convexity and the cash flows. Give a security (its name or ISIN, looked up in RBI's list) or a coupon and maturity, and a price or a yield.",
        params: &[
            opt(
                "security",
                Kind::Str,
                "Name or ISIN, e.g. 718GS2033 or IN0020230036",
            ),
            opt(
                "coupon",
                Kind::Number {
                    min: 0.0,
                    max: 30.0,
                },
                "Coupon, percent a year (with maturity, instead of a security)",
            ),
            opt("maturity", Kind::Str, "Maturity date YYYY-MM-DD"),
            opt(
                "price",
                Kind::Number {
                    min: 1.0,
                    max: 1000.0,
                },
                "Price per ₹100 face value",
            ),
            opt(
                "ytm",
                Kind::Number {
                    min: 0.0,
                    max: 50.0,
                },
                "Yield to maturity, percent (instead of a price)",
            ),
            opt(
                "settle",
                Kind::Str,
                "Settlement date YYYY-MM-DD (default: today's session)",
            ),
            opt("tbill", Kind::Bool, "A Treasury bill (discount, no coupon)"),
            opt(
                "dirty",
                Kind::Bool,
                "The price includes accrued interest (NSE quotes G-secs dirty)",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "commodity",
        title: "Gold, silver and Sovereign Gold Bonds",
        description: "Gold and silver for Indian investors. spot: international gold and silver in rupees per 10 g and per kg, with USD/INR. futures: MCX-style futures quotes. etfs: gold and silver ETFs on NSE. sgb: every Sovereign Gold Bond series on NSE with its price, the gold value of one unit, the premium or discount to gold, coupon and maturity.",
        params: &[opt(
            "view",
            Kind::Enum(&["spot", "futures", "etfs", "sgb"]),
            "Which view (default spot)",
        )],
        personal: false,
    },
    Tool {
        name: "fx_history",
        title: "Currency against the rupee",
        description: "History of a currency against the rupee (or another base): first and last rate, change, high and low over a period and the recent daily rates. Useful for US holdings of an Indian investor (USD/INR moves change their rupee value).",
        params: &[
            opt(
                "currency",
                Kind::Str,
                "3-letter code, e.g. USD (default), EUR, GBP, AED, SGD, JPY",
            ),
            opt("base", Kind::Str, "Base currency (default INR)"),
            opt(
                "period",
                Kind::Enum(&["1mo", "3mo", "6mo", "ytd", "1y", "2y", "5y", "10y"]),
                "Period (default 1y)",
            ),
        ],
        personal: false,
    },
    Tool {
        name: "returns",
        title: "Trailing returns",
        description: "A stock's, ETF's or index's trailing price returns over 1 week, 1, 3 and 6 months and 1, 2 and 5 years, annualized beyond a year, and what ₹1 lakh (or 100,000 of its currency) became. Price returns: dividends not included.",
        params: &[
            req("symbol", Kind::Str, "Ticker, e.g. NIFTYBEES, ^NSEI, TCS"),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "correlate",
        title: "Correlation of stocks",
        description: "How closely 2 to 10 tickers moved together over the last year: the correlation of daily returns on common trading days for each pair, as correlations[A][B] (1 moves together, 0 unrelated, -1 opposite). Useful to see whether holdings diversify each other.",
        params: &[
            req(
                "symbols",
                Kind::StrList { max: 10 },
                "2 to 10 tickers, e.g. [\"HDFCBANK\", \"ICICIBANK\", \"TCS\"]",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "compare",
        title: "Compare stocks side by side",
        description: "Up to 6 stocks side by side: price, market cap and cap tier, P/E (trailing and forward), P/B, EPS, ROE, margins, debt to equity, revenue and earnings growth, dividend yield, beta and the 52-week range. Ratios are fractions (0.012 = 1.2%).",
        params: &[
            req(
                "symbols",
                Kind::StrList { max: 6 },
                "2 to 6 tickers, e.g. [\"TCS\", \"INFY\", \"HCLTECH\"]",
            ),
            MARKET,
        ],
        personal: false,
    },
    Tool {
        name: "portfolio_wrap",
        title: "Portfolio year in review",
        description: "The portfolio's year in review: return over the year (or an Indian financial year), money in and out, dividends, realized gains, best and worst holdings, and the checkup's main findings. Pass fy for an April-March year such as 2025-26.",
        params: &[
            opt("year", Kind::Str, "Calendar year, e.g. 2025"),
            opt("fy", Kind::Str, "Indian financial year, e.g. 2025-26"),
            opt(
                "redact",
                Kind::Bool,
                "Leave out amounts, keeping percentages (to share)",
            ),
            opt(
                "base",
                Kind::Str,
                "Currency for totals, a 3-letter code (default: the market's)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "whats_hot",
        title: "What's hot",
        description: "Your holdings and watchlist ranked by heat (|excess move| in σ + volume surge + fresh news/filing importance + a date within 3 days, scaled by portfolio weight), each with its move and one-line why; then the market: biggest moves, unusual volume (RVOL), gaps, 52-week highs and lows, most-mentioned stocks in news, sectors, theme baskets (AI, semis, defence, battery, minerals), and for India FII/DII flows, bulk/block deals, the F&O ban list and OI build-up; plus cross-market movers. Data, not recommendations.",
        params: &[
            opt(
                "cap",
                Kind::Enum(&["all", "mega", "large", "mid", "small", "micro"]),
                "Only this market-cap tier for the market lists",
            ),
            opt(
                "limit",
                Kind::Integer { min: 1, max: 25 },
                "Rows per list (default 8)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "today",
        title: "Today in my portfolio",
        description: "What changed in the portfolio today: per holding and watchlist name the day move, day P&L in the base currency, σ of the excess move, volume multiple and why (the most important fresh headline or exchange/SEC filing, \"with the market\", or \"unexplained\" for an unusual move with no news); the portfolio's day P&L, the market backdrop, dates in the next 3 days, and a merged news feed ranked by weight × importance.",
        params: &[
            opt(
                "days",
                Kind::Integer { min: 1, max: 14 },
                "News window in days (default 2)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "news_feed",
        title: "News feed",
        description: "Merged, de-duplicated headlines and filings (Yahoo, Google News by company name in the stock's locale, NSE/BSE announcements, SEC EDGAR) for the given symbols, or for holdings and the watchlist ranked by portfolio weight × importance. Each item has its title, source, link, time and category.",
        params: &[
            opt(
                "symbols",
                Kind::StrList { max: 25 },
                "Tickers (default: holdings and watchlist)",
            ),
            opt(
                "days",
                Kind::Integer { min: 1, max: 14 },
                "Only items from the last N days (default 2)",
            ),
            opt(
                "limit",
                Kind::Integer { min: 1, max: 60 },
                "Most items (default 20)",
            ),
            MARKET,
        ],
        personal: true,
    },
    Tool {
        name: "monitor_status",
        title: "Holding monitor status",
        description: "The automatic holding monitor: its thresholds (day drop %, σ, fall from cost and from peak, gap), whether the background worker runs it, when it last ran, and the signals it sent recently (big drops, 52-week lows, gaps, dates tomorrow, pledge and shareholding changes, rating changes, insider trades).",
        params: &[],
        personal: true,
    },
    Tool {
        name: "digest",
        title: "Daily digest",
        description: "Today's digest as the worker sends it after the close: day P&L, top contributors and detractors with the news or filing behind each move (or flagged unexplained), monitor signals not yet sent, alerts that triggered today and dates in the next week. Does not send anything.",
        params: &[MARKET],
        personal: true,
    },
];

/// Only with `stockwise mcp --allow-orders`: previews an order without
/// sending it. Placing it stays with the user, through `stockwise trade`.
pub const ORDER_PREVIEW: Tool = Tool {
    name: "order_preview",
    title: "Order preview (dry run)",
    description: "Check an NSE order a user is considering (symbol, trigger and limit against \
                  the last price) and estimate its value. NEVER places an order and does not read \
                  the broker account or its limits: it returns the `stockwise trade` command for \
                  the user to review and run themselves; that command applies their \
                  max_order_value cap and asks for typed confirmation.",
    params: &[
        req("side", Kind::Enum(&["buy", "sell"]), "Buy or sell"),
        req(
            "symbol",
            Kind::Str,
            "NSE symbol, e.g. RELIANCE, M&M, BAJAJ-AUTO",
        ),
        req(
            "quantity",
            Kind::Integer {
                min: 1,
                max: 1_000_000,
            },
            "Shares",
        ),
        opt(
            "limit",
            Kind::Number {
                min: 0.01,
                max: 1e9,
            },
            "Limit price (market order without it)",
        ),
        opt(
            "trigger",
            Kind::Number {
                min: 0.01,
                max: 1e9,
            },
            "Stop-loss trigger price",
        ),
        req(
            "broker",
            Kind::Enum(&["angel", "zerodha", "upstox", "dhan", "fyers"]),
            "The user's broker",
        ),
    ],
    personal: false,
};

/// What tools run against: market data and local files, the market for
/// bare tickers, and whether order previews are offered.
pub struct ToolContext<D> {
    pub source: D,
    pub market: Market,
    pub allow_orders: bool,
    /// Where changes go when the host allows them; `None`: read-only.
    pub workspace: Option<Workspace>,
    /// Results are cut to this size (small local models get less).
    pub max_result_bytes: usize,
    /// Longest a read tool may run; `None`: [`default_timeout`] per tool.
    pub timeout: Option<std::time::Duration>,
}

impl<D: DataSource> ToolContext<D> {
    pub fn new(source: D, market: Market) -> Self {
        Self {
            source,
            market,
            allow_orders: false,
            workspace: None,
            max_result_bytes: MAX_RESULT_BYTES,
            timeout: None,
        }
    }

    /// The tools this context offers.
    pub fn tools(&self) -> Vec<&'static Tool> {
        let mut out: Vec<&'static Tool> = TOOLS.iter().collect();
        if let Some(w) = &self.workspace {
            out.extend(WRITE_TOOLS);
            if w.allow_portfolio {
                out.extend(PORTFOLIO_TOOLS);
            }
        }
        if self.allow_orders {
            out.push(&ORDER_PREVIEW);
        }
        out
    }

    pub fn find(&self, name: &str) -> Option<&'static Tool> {
        self.tools().into_iter().find(|t| t.name == name)
    }

    /// Run `name` with `args` (a JSON object, or null for none) and return
    /// the envelope: `{"data": ..., "note": ...}`, plus `"truncated"` when
    /// lists were shortened to fit `max_result_bytes`. Bad arguments and
    /// failed lookups are errors whose message is written for the model.
    pub async fn call(&self, name: &str, args: &Value) -> Result<Value> {
        let Some(tool) = self.find(name) else {
            bail!(
                "Unknown tool '{}'. Available: {}",
                name,
                self.tools()
                    .iter()
                    .map(|t| t.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        };
        let args = Args::parse(tool, args)?;
        let data = match (&self.workspace, access(tool.name)) {
            (Some(w), Access::Workspace | Access::Portfolio) => {
                let m = args.market(self.market);
                workspace::run(w, &self.source, tool.name, &args, m).await?
            }
            _ => {
                let limit = self.timeout.unwrap_or_else(|| default_timeout(tool.name));
                match tokio::time::timeout(limit, handlers::run(self, tool.name, &args)).await {
                    Ok(r) => r?,
                    Err(_) => bail!(
                        "{} did not finish within {} seconds and was stopped. Data sources \
                         may be slow; try again, or ask for less (fewer symbols, a smaller \
                         universe or period).",
                        tool.name,
                        limit.as_secs()
                    ),
                }
            }
        };
        Ok(envelope(data, self.max_result_bytes))
    }
}

/// Wrap `data` with the note, shortening its longest lists until the whole
/// result fits in `max_bytes`.
pub fn envelope(mut data: Value, max_bytes: usize) -> Value {
    let truncated = fit(&mut data, max_bytes.saturating_sub(NOTE.len() + 200));
    let mut out = json!({ "data": data });
    if truncated {
        out["truncated"] = json!(
            "Some lists were shortened to keep this result small; ask for fewer items \
             (limit, period, strikes) to see everything."
        );
    }
    out["note"] = json!(NOTE);
    out
}

/// Halve the longest array in `v` until it serializes within `max_bytes`.
/// Returns whether anything was cut.
fn fit(v: &mut Value, max_bytes: usize) -> bool {
    let mut cut = false;
    while serde_json::to_string(v).map_or(0, |s| s.len()) > max_bytes {
        let longest = longest_len(v);
        if longest <= 1 {
            break;
        }
        halve(v, longest);
        cut = true;
    }
    cut
}

/// Length of the longest array anywhere in `v`.
fn longest_len(v: &Value) -> usize {
    match v {
        Value::Array(a) => a.len().max(a.iter().map(longest_len).max().unwrap_or(0)),
        Value::Object(o) => o.values().map(longest_len).max().unwrap_or(0),
        _ => 0,
    }
}

/// Halve the first array of length `len` found in `v`.
fn halve(v: &mut Value, len: usize) -> bool {
    match v {
        Value::Array(a) if a.len() == len => {
            a.truncate(len / 2);
            true
        }
        Value::Array(a) => a.iter_mut().any(|x| halve(x, len)),
        Value::Object(o) => o.values_mut().any(|x| halve(x, len)),
        _ => false,
    }
}

#[cfg(test)]
mod tests;

/// The investor profile as it is now: read again from the workspace's
/// config file, so a `profile_set` or `profile_onboard` earlier in this
/// process counts at once.
pub(crate) fn current_profile<D: DataSource>(
    ctx: &ToolContext<D>,
) -> crate::config::EffectiveProfile {
    match &ctx.workspace {
        Some(w) => crate::config::effective_profile_at(&w.config),
        None => crate::config::effective_profile(),
    }
}
