//! Read-only tools for AI agents: `stockwise mcp` serves them over the Model
//! Context Protocol and `stockwise ask` hands them to the configured model.
//!
//! Each tool is a name, a description, a JSON-schema input built from a
//! [`Param`] list, and a handler that returns JSON from the same functions
//! the commands' `--json` output uses. Nothing here places orders, edits the
//! portfolio or prints. Every result is wrapped in an envelope with a short
//! "not financial advice" note and kept small enough for a model's context.

mod args;
mod handlers;

pub use args::Args;

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
        description: "Live (often delayed) quotes for up to 20 tickers: price, change, day range, \
                      volume, 52-week range, market cap, P/E. Unknown tickers are listed in `missing`.",
        params: &[
            req(
                "symbols",
                Kind::StrList { max: 20 },
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
                      the technical pillar. It describes the data and is not a recommendation: \
                      do not present it as advice or as a call on the stock.",
        params: &[SYMBOL, MARKET],
        personal: false,
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
                      (or equal weight): current vs target weight, drift tolerance band, and \
                      buy/sell trade sizing (or buy-only allocation when `inflow` is provided).",
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
    /// Results are cut to this size (small local models get less).
    pub max_result_bytes: usize,
}

impl<D: DataSource> ToolContext<D> {
    pub fn new(source: D, market: Market) -> Self {
        Self {
            source,
            market,
            allow_orders: false,
            max_result_bytes: MAX_RESULT_BYTES,
        }
    }

    /// The tools this context offers.
    pub fn tools(&self) -> Vec<&'static Tool> {
        let mut out: Vec<&'static Tool> = TOOLS.iter().collect();
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
        let data = handlers::run(self, tool.name, &args).await?;
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
