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
                      6mo, weekly to 2y, monthly beyond). Returns at most `limit` most recent bars \
                      plus first/last close and the change over the period.",
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
                      Returns at most `strikes` strikes centred on the spot.",
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
                Kind::Integer { min: 4, max: 40 },
                "Strikes to return around the spot (default 16)",
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
        name: "goal_sip",
        title: "SIP for a goal",
        description: "Monthly SIP needed to reach a target amount in a number of years at an \
                      expected return, optionally stepped up every year, with a year-by-year \
                      schedule. Amounts in the market's currency (₹ for India).",
        params: &[
            req(
                "target",
                Kind::Number {
                    min: 1.0,
                    max: 1e13,
                },
                "Target amount (1 crore = 10000000, 1 lakh = 100000)",
            ),
            req("years", YEARS, "Years to the goal"),
            opt(
                "return_pct",
                RETURN,
                "Expected yearly return in percent (default 12)",
            ),
            STEP_UP,
            SAVINGS,
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
                      and the monthly SIP that pays for it.",
        params: &[
            req(
                "cost",
                Kind::Number {
                    min: 1.0,
                    max: 1e12,
                },
                "Cost in today's money",
            ),
            req("years", YEARS, "Years until the money is needed"),
            opt(
                "inflation_pct",
                INFLATION,
                "Yearly education inflation in percent (default 8)",
            ),
            opt(
                "return_pct",
                RETURN,
                "Expected yearly return in percent (default 12)",
            ),
            STEP_UP,
            SAVINGS,
            MARKET,
        ],
        personal: false,
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
    description: "Check an order a user is considering and estimate its value from the last \
                  price, against the max_order_value cap in their broker config. NEVER places an \
                  order: it returns the `stockwise trade` command the user can review and run \
                  themselves (live orders always need their typed confirmation).",
    params: &[
        req("side", Kind::Enum(&["buy", "sell"]), "Buy or sell"),
        SYMBOL,
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
